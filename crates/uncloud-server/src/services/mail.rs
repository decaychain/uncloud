use std::fmt;
use std::future::Future;
use std::time::Duration;

use async_imap::types::{Capability, Flag};
use async_imap::{Client, Session};
use async_native_tls::TlsConnector;
use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use lettre::transport::smtp::{
    authentication::Credentials,
    client::{Tls, TlsParameters},
};
use lettre::{AsyncSmtpTransport, Tokio1Executor};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::time::timeout;
use uncloud_common::{MailConnectionTestResponse, MailSecurity};

use crate::error::{AppError, Result};
use crate::models::MailServerConfig;

const MAIL_OP_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Default)]
pub struct MailService;

impl MailService {
    pub fn new() -> Self {
        Self
    }

    pub async fn test_imap_password(
        &self,
        settings: &MailServerConfig,
        password: &str,
    ) -> Result<MailConnectionTestResponse> {
        let capabilities = match settings.security {
            MailSecurity::Tls => {
                let client = connect_imap_tls(settings, true).await?;
                test_imap_client(client, settings, password).await?
            }
            MailSecurity::StartTls => {
                let client = connect_imap_starttls(settings).await?;
                test_imap_client(client, settings, password).await?
            }
            MailSecurity::Plain => {
                let client = connect_imap_plain(settings).await?;
                test_imap_client(client, settings, password).await?
            }
        };

        Ok(MailConnectionTestResponse {
            ok: true,
            capabilities,
        })
    }

    pub async fn list_imap_mailboxes(
        &self,
        settings: &MailServerConfig,
        password: &str,
    ) -> Result<Vec<RemoteMailbox>> {
        match settings.security {
            MailSecurity::Tls => {
                let client = connect_imap_tls(settings, true).await?;
                list_imap_mailboxes(client, settings, password).await
            }
            MailSecurity::StartTls => {
                let client = connect_imap_starttls(settings).await?;
                list_imap_mailboxes(client, settings, password).await
            }
            MailSecurity::Plain => {
                let client = connect_imap_plain(settings).await?;
                list_imap_mailboxes(client, settings, password).await
            }
        }
    }

    pub async fn fetch_next_imap_message_summaries(
        &self,
        settings: &MailServerConfig,
        password: &str,
        folder_path: &str,
        previous_uid_validity: Option<u32>,
        lowest_synced_uid: Option<u32>,
        highest_synced_uid: Option<u32>,
        limit: u32,
    ) -> Result<RemoteMailboxSync> {
        match settings.security {
            MailSecurity::Tls => {
                let client = connect_imap_tls(settings, true).await?;
                fetch_next_imap_message_summaries(
                    client,
                    settings,
                    password,
                    folder_path,
                    previous_uid_validity,
                    lowest_synced_uid,
                    highest_synced_uid,
                    limit,
                )
                .await
            }
            MailSecurity::StartTls => {
                let client = connect_imap_starttls(settings).await?;
                fetch_next_imap_message_summaries(
                    client,
                    settings,
                    password,
                    folder_path,
                    previous_uid_validity,
                    lowest_synced_uid,
                    highest_synced_uid,
                    limit,
                )
                .await
            }
            MailSecurity::Plain => {
                let client = connect_imap_plain(settings).await?;
                fetch_next_imap_message_summaries(
                    client,
                    settings,
                    password,
                    folder_path,
                    previous_uid_validity,
                    lowest_synced_uid,
                    highest_synced_uid,
                    limit,
                )
                .await
            }
        }
    }

    pub async fn fetch_imap_message_body(
        &self,
        settings: &MailServerConfig,
        password: &str,
        folder_path: &str,
        uid: u32,
    ) -> Result<String> {
        match settings.security {
            MailSecurity::Tls => {
                let client = connect_imap_tls(settings, true).await?;
                fetch_imap_message_body(client, settings, password, folder_path, uid).await
            }
            MailSecurity::StartTls => {
                let client = connect_imap_starttls(settings).await?;
                fetch_imap_message_body(client, settings, password, folder_path, uid).await
            }
            MailSecurity::Plain => {
                let client = connect_imap_plain(settings).await?;
                fetch_imap_message_body(client, settings, password, folder_path, uid).await
            }
        }
    }

    pub async fn test_smtp_password(
        &self,
        settings: &MailServerConfig,
        password: &str,
    ) -> Result<MailConnectionTestResponse> {
        let tls = match settings.security {
            MailSecurity::Tls => Tls::Wrapper(smtp_tls_parameters(settings)?),
            MailSecurity::StartTls => Tls::Required(smtp_tls_parameters(settings)?),
            MailSecurity::Plain => Tls::None,
        };
        let transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&settings.host)
            .port(settings.port)
            .tls(tls)
            .credentials(Credentials::new(
                settings.username.clone(),
                password.to_string(),
            ))
            .timeout(Some(MAIL_OP_TIMEOUT))
            .build::<Tokio1Executor>();
        let connected = imap_timeout("SMTP connection test", transport.test_connection())
            .await?
            .map_err(map_smtp_error)?;

        if !connected {
            return Err(AppError::Internal(
                "SMTP connection test did not complete successfully".into(),
            ));
        }

        Ok(MailConnectionTestResponse {
            ok: true,
            capabilities: Vec::new(),
        })
    }
}

async fn imap_timeout<T, F>(operation: &'static str, future: F) -> Result<T>
where
    F: Future<Output = T>,
{
    timeout(MAIL_OP_TIMEOUT, future)
        .await
        .map_err(|_| AppError::Internal(format!("{operation} timed out after 30s")))
}

async fn connect_imap_tcp(settings: &MailServerConfig) -> Result<TcpStream> {
    imap_timeout(
        "IMAP TCP connection",
        TcpStream::connect((settings.host.as_str(), settings.port)),
    )
    .await?
    .map_err(|e| AppError::Internal(format!("IMAP TCP connection failed: {e}")))
}

async fn connect_imap_plain(settings: &MailServerConfig) -> Result<Client<TcpStream>> {
    let mut client = Client::new(connect_imap_tcp(settings).await?);
    read_imap_greeting(&mut client).await?;
    Ok(client)
}

async fn connect_imap_tls(
    settings: &MailServerConfig,
    read_greeting: bool,
) -> Result<Client<async_native_tls::TlsStream<TcpStream>>> {
    let tcp = connect_imap_tcp(settings).await?;
    let tls = imap_timeout(
        "IMAP TLS handshake",
        TlsConnector::new().connect(settings.host.as_str(), tcp),
    )
    .await?
    .map_err(|e| AppError::Internal(format!("IMAP TLS handshake failed: {e}")))?;
    let mut client = Client::new(tls);
    if read_greeting {
        read_imap_greeting(&mut client).await?;
    }
    Ok(client)
}

async fn connect_imap_starttls(
    settings: &MailServerConfig,
) -> Result<Client<async_native_tls::TlsStream<TcpStream>>> {
    let mut client = connect_imap_plain(settings).await?;
    imap_timeout(
        "IMAP STARTTLS",
        client.run_command_and_check_ok("STARTTLS", None),
    )
    .await?
    .map_err(|e| AppError::BadRequest(format!("IMAP STARTTLS failed: {e}")))?;
    let tcp = client.into_inner();
    let tls = imap_timeout(
        "IMAP STARTTLS handshake",
        TlsConnector::new().connect(settings.host.as_str(), tcp),
    )
    .await?
    .map_err(|e| AppError::Internal(format!("IMAP STARTTLS handshake failed: {e}")))?;
    Ok(Client::new(tls))
}

async fn read_imap_greeting<T>(client: &mut Client<T>) -> Result<()>
where
    T: AsyncRead + AsyncWrite + Unpin + fmt::Debug + Send,
{
    imap_timeout("IMAP greeting", client.read_response())
        .await?
        .map_err(|e| AppError::Internal(format!("IMAP greeting failed: {e}")))?;
    Ok(())
}

async fn login_imap_client<T>(
    client: Client<T>,
    settings: &MailServerConfig,
    password: &str,
) -> Result<Session<T>>
where
    T: AsyncRead + AsyncWrite + Unpin + fmt::Debug + Send,
{
    imap_timeout("IMAP login", client.login(&settings.username, password))
        .await?
        .map_err(|(e, _)| AppError::BadRequest(format!("IMAP login failed: {e}")))
}

async fn test_imap_client<T>(
    client: Client<T>,
    settings: &MailServerConfig,
    password: &str,
) -> Result<Vec<String>>
where
    T: AsyncRead + AsyncWrite + Unpin + fmt::Debug + Send,
{
    let mut session = login_imap_client(client, settings, password).await?;
    let capabilities = imap_timeout("IMAP CAPABILITY", session.capabilities())
        .await?
        .map_err(|e| AppError::Internal(format!("IMAP CAPABILITY failed: {e}")))?
        .iter()
        .map(capability_name)
        .collect();
    let _ = session.logout().await;
    Ok(capabilities)
}

async fn list_imap_mailboxes<T>(
    client: Client<T>,
    settings: &MailServerConfig,
    password: &str,
) -> Result<Vec<RemoteMailbox>>
where
    T: AsyncRead + AsyncWrite + Unpin + fmt::Debug + Send,
{
    let mut session = login_imap_client(client, settings, password).await?;

    let names_stream = imap_timeout("IMAP LIST", session.list(None, Some("*")))
        .await?
        .map_err(|e| AppError::Internal(format!("IMAP LIST failed: {e}")))?
        .try_collect();
    let names: Vec<_> = imap_timeout("IMAP LIST read", names_stream)
        .await?
        .map_err(|e| AppError::Internal(format!("IMAP LIST read failed: {e}")))?;

    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let path = name.name().to_string();
        let delimiter = name.delimiter().map(|d| d.to_string());
        let attributes: Vec<String> = name.attributes().iter().map(|a| format!("{a:?}")).collect();
        let selectable = !attributes
            .iter()
            .any(|a| a.eq_ignore_ascii_case("NoSelect"));
        let parent_path = parent_path(&path, delimiter.as_deref());
        out.push(RemoteMailbox {
            path: path.clone(),
            name: display_name(&path, delimiter.as_deref()),
            delimiter,
            parent_path,
            attributes,
            selectable,
        });
    }

    let _ = session.logout().await;
    Ok(out)
}

async fn fetch_next_imap_message_summaries<T>(
    client: Client<T>,
    settings: &MailServerConfig,
    password: &str,
    folder_path: &str,
    previous_uid_validity: Option<u32>,
    lowest_synced_uid: Option<u32>,
    highest_synced_uid: Option<u32>,
    limit: u32,
) -> Result<RemoteMailboxSync>
where
    T: AsyncRead + AsyncWrite + Unpin + fmt::Debug + Send,
{
    let limit = limit.max(1);
    let mut session = login_imap_client(client, settings, password).await?;
    let mailbox = imap_timeout("IMAP EXAMINE", session.examine(folder_path))
        .await?
        .map_err(|e| AppError::Internal(format!("IMAP EXAMINE failed: {e}")))?;
    if mailbox.exists > 0 && (mailbox.uid_next.is_none() || mailbox.uid_validity.is_none()) {
        let _ = session.logout().await;
        return Err(AppError::Internal(
            "IMAP mailbox does not expose UIDNEXT/UIDVALIDITY for safe sync".into(),
        ));
    }

    let uid_validity_changed =
        previous_uid_validity.is_some() && mailbox.uid_validity != previous_uid_validity;
    let effective_lowest = if uid_validity_changed {
        None
    } else {
        lowest_synced_uid
    };
    let effective_highest = if uid_validity_changed {
        None
    } else {
        highest_synced_uid
    };
    let plan = next_uid_sync_plan(effective_lowest, effective_highest, mailbox.uid_next, limit);

    let messages = if let Some(plan) = plan {
        let start = plan.start;
        let end = plan.end;
        let uid_set = format!("{start}:{end}");
        let fetch_stream = imap_timeout(
            "IMAP UID FETCH",
            session.uid_fetch(uid_set, "(UID FLAGS INTERNALDATE RFC822.SIZE ENVELOPE)"),
        )
        .await?
        .map_err(|e| AppError::Internal(format!("IMAP UID FETCH failed: {e}")))?
        .try_collect();
        let fetches: Vec<_> = imap_timeout("IMAP UID FETCH read", fetch_stream)
            .await?
            .map_err(|e| AppError::Internal(format!("IMAP UID FETCH read failed: {e}")))?;
        fetches.iter().filter_map(remote_message_summary).collect()
    } else {
        Vec::new()
    };

    let _ = session.logout().await;
    let (lowest_synced_uid, highest_synced_uid) =
        updated_uid_cursors(effective_lowest, effective_highest, plan);
    let completed = sync_completed(lowest_synced_uid, highest_synced_uid, mailbox.uid_next);

    Ok(RemoteMailboxSync {
        status: RemoteMailboxStatus {
            uid_validity: mailbox.uid_validity,
            uid_next: mailbox.uid_next,
            exists: Some(mailbox.exists),
            unseen: mailbox.unseen,
        },
        uid_validity_changed,
        lowest_synced_uid,
        highest_synced_uid,
        completed,
        messages,
    })
}

async fn fetch_imap_message_body<T>(
    client: Client<T>,
    settings: &MailServerConfig,
    password: &str,
    folder_path: &str,
    uid: u32,
) -> Result<String>
where
    T: AsyncRead + AsyncWrite + Unpin + fmt::Debug + Send,
{
    let mut session = login_imap_client(client, settings, password).await?;
    imap_timeout("IMAP EXAMINE", session.examine(folder_path))
        .await?
        .map_err(|e| AppError::Internal(format!("IMAP EXAMINE failed: {e}")))?;

    let fetch_stream = imap_timeout(
        "IMAP UID FETCH body",
        session.uid_fetch(uid.to_string(), "(UID BODY.PEEK[])"),
    )
    .await?
    .map_err(|e| AppError::Internal(format!("IMAP UID FETCH body failed: {e}")))?
    .try_collect();
    let fetches: Vec<_> = imap_timeout("IMAP UID FETCH body read", fetch_stream)
        .await?
        .map_err(|e| AppError::Internal(format!("IMAP UID FETCH body read failed: {e}")))?;
    let body = fetches
        .iter()
        .find_map(|fetch| fetch.body().map(|body| body.to_vec()))
        .ok_or_else(|| AppError::NotFound("Mail message body".into()))?;
    let _ = session.logout().await;
    Ok(raw_message_to_plain_text(&body))
}

fn smtp_tls_parameters(settings: &MailServerConfig) -> Result<TlsParameters> {
    TlsParameters::new(settings.host.clone())
        .map_err(|e| AppError::Internal(format!("SMTP TLS configuration failed: {e}")))
}

fn map_smtp_error(err: lettre::transport::smtp::Error) -> AppError {
    if err.is_client() || err.is_response() || err.is_permanent() {
        AppError::BadRequest(format!("SMTP connection test failed: {err}"))
    } else {
        AppError::Internal(format!("SMTP connection test failed: {err}"))
    }
}

fn capability_name(cap: &Capability) -> String {
    match cap {
        Capability::Imap4rev1 => "IMAP4rev1".to_string(),
        Capability::Auth(value) => format!("AUTH={value}"),
        Capability::Atom(value) => value.clone(),
    }
}

#[derive(Debug, Clone)]
pub struct RemoteMailbox {
    pub path: String,
    pub name: String,
    pub delimiter: Option<String>,
    pub parent_path: Option<String>,
    pub attributes: Vec<String>,
    pub selectable: bool,
}

#[derive(Debug, Clone)]
pub struct RemoteMailboxStatus {
    pub uid_validity: Option<u32>,
    pub uid_next: Option<u32>,
    pub exists: Option<u32>,
    pub unseen: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct RemoteMailboxSync {
    pub status: RemoteMailboxStatus,
    pub uid_validity_changed: bool,
    pub lowest_synced_uid: Option<u32>,
    pub highest_synced_uid: Option<u32>,
    pub completed: bool,
    pub messages: Vec<RemoteMessageSummary>,
}

#[derive(Debug, Clone)]
pub struct RemoteMailAddress {
    pub name: Option<String>,
    pub address: String,
}

#[derive(Debug, Clone)]
pub struct RemoteMessageSummary {
    pub uid: u32,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub subject: Option<String>,
    pub from: Vec<RemoteMailAddress>,
    pub to: Vec<RemoteMailAddress>,
    pub cc: Vec<RemoteMailAddress>,
    pub bcc: Vec<RemoteMailAddress>,
    pub date: Option<DateTime<Utc>>,
    pub internal_date: Option<DateTime<Utc>>,
    pub flags: Vec<String>,
    pub size_bytes: Option<u64>,
}

fn display_name(path: &str, delimiter: Option<&str>) -> String {
    delimiter
        .and_then(|d| path.rsplit(d).next())
        .filter(|s| !s.is_empty())
        .unwrap_or(path)
        .to_string()
}

fn parent_path(path: &str, delimiter: Option<&str>) -> Option<String> {
    let delimiter = delimiter?;
    let (parent, _) = path.rsplit_once(delimiter)?;
    if parent.is_empty() {
        None
    } else {
        Some(parent.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UidSyncPlan {
    start: u32,
    end: u32,
    direction: UidSyncDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UidSyncDirection {
    Latest,
    Newer,
    Older,
}

fn next_uid_sync_plan(
    lowest_synced_uid: Option<u32>,
    highest_synced_uid: Option<u32>,
    uid_next: Option<u32>,
    limit: u32,
) -> Option<UidSyncPlan> {
    let limit = limit.max(1);
    let uid_next = uid_next?;
    let last_available = uid_next.checked_sub(1)?;
    if last_available == 0 {
        return None;
    }

    if lowest_synced_uid.is_none() || highest_synced_uid.is_none() {
        let end = last_available;
        let start = end.saturating_sub(limit.saturating_sub(1)).max(1);
        return Some(UidSyncPlan {
            start,
            end,
            direction: UidSyncDirection::Latest,
        });
    }

    if let Some(highest) = highest_synced_uid {
        if highest < last_available {
            let start = highest.saturating_add(1);
            let end = start
                .saturating_add(limit.saturating_sub(1))
                .min(last_available);
            return Some(UidSyncPlan {
                start,
                end,
                direction: UidSyncDirection::Newer,
            });
        }
    }

    let lowest = lowest_synced_uid?;
    if lowest <= 1 {
        return None;
    }
    let end = lowest.saturating_sub(1);
    let start = end.saturating_sub(limit.saturating_sub(1)).max(1);
    Some(UidSyncPlan {
        start,
        end,
        direction: UidSyncDirection::Older,
    })
}

fn updated_uid_cursors(
    current_lowest: Option<u32>,
    current_highest: Option<u32>,
    plan: Option<UidSyncPlan>,
) -> (Option<u32>, Option<u32>) {
    let Some(plan) = plan else {
        return (current_lowest, current_highest);
    };

    match plan.direction {
        UidSyncDirection::Latest => (Some(plan.start), Some(plan.end)),
        UidSyncDirection::Newer => (current_lowest, Some(plan.end)),
        UidSyncDirection::Older => (Some(plan.start), current_highest),
    }
}

fn sync_completed(
    lowest_synced_uid: Option<u32>,
    highest_synced_uid: Option<u32>,
    uid_next: Option<u32>,
) -> bool {
    let Some(uid_next) = uid_next else {
        return true;
    };
    let Some(last_available) = uid_next.checked_sub(1) else {
        return true;
    };
    let Some(lowest) = lowest_synced_uid else {
        return false;
    };
    let Some(highest) = highest_synced_uid else {
        return false;
    };
    lowest <= 1 && highest >= last_available
}

fn remote_message_summary(fetch: &async_imap::types::Fetch) -> Option<RemoteMessageSummary> {
    let uid = fetch.uid?;
    let envelope = fetch.envelope();
    Some(RemoteMessageSummary {
        uid,
        message_id: envelope
            .and_then(|e| e.message_id.as_deref())
            .and_then(decode_bytes),
        in_reply_to: envelope
            .and_then(|e| e.in_reply_to.as_deref())
            .and_then(decode_bytes),
        subject: envelope
            .and_then(|e| e.subject.as_deref())
            .and_then(decode_bytes),
        from: envelope
            .and_then(|e| e.from.as_ref())
            .map(|addresses| {
                addresses
                    .iter()
                    .filter_map(|address| {
                        let mailbox = address.mailbox.as_deref().and_then(decode_bytes)?;
                        let host = address.host.as_deref().and_then(decode_bytes)?;
                        Some(RemoteMailAddress {
                            name: address.name.as_deref().and_then(decode_bytes),
                            address: format!("{mailbox}@{host}"),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        to: envelope
            .and_then(|e| e.to.as_ref())
            .map(|addresses| {
                addresses
                    .iter()
                    .filter_map(|address| {
                        let mailbox = address.mailbox.as_deref().and_then(decode_bytes)?;
                        let host = address.host.as_deref().and_then(decode_bytes)?;
                        Some(RemoteMailAddress {
                            name: address.name.as_deref().and_then(decode_bytes),
                            address: format!("{mailbox}@{host}"),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        cc: envelope
            .and_then(|e| e.cc.as_ref())
            .map(|addresses| {
                addresses
                    .iter()
                    .filter_map(|address| {
                        let mailbox = address.mailbox.as_deref().and_then(decode_bytes)?;
                        let host = address.host.as_deref().and_then(decode_bytes)?;
                        Some(RemoteMailAddress {
                            name: address.name.as_deref().and_then(decode_bytes),
                            address: format!("{mailbox}@{host}"),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        bcc: envelope
            .and_then(|e| e.bcc.as_ref())
            .map(|addresses| {
                addresses
                    .iter()
                    .filter_map(|address| {
                        let mailbox = address.mailbox.as_deref().and_then(decode_bytes)?;
                        let host = address.host.as_deref().and_then(decode_bytes)?;
                        Some(RemoteMailAddress {
                            name: address.name.as_deref().and_then(decode_bytes),
                            address: format!("{mailbox}@{host}"),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        date: envelope
            .and_then(|e| e.date.as_deref())
            .and_then(parse_envelope_date),
        internal_date: fetch.internal_date().map(|dt| dt.with_timezone(&Utc)),
        flags: fetch.flags().map(flag_name).collect(),
        size_bytes: fetch.size.map(u64::from),
    })
}

fn decode_bytes(bytes: &[u8]) -> Option<String> {
    let value = String::from_utf8_lossy(bytes).trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn parse_envelope_date(bytes: &[u8]) -> Option<DateTime<Utc>> {
    let value = String::from_utf8_lossy(bytes);
    DateTime::parse_from_rfc2822(value.trim())
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

fn flag_name(flag: Flag<'_>) -> String {
    match flag {
        Flag::Seen => "\\Seen".to_string(),
        Flag::Answered => "\\Answered".to_string(),
        Flag::Flagged => "\\Flagged".to_string(),
        Flag::Deleted => "\\Deleted".to_string(),
        Flag::Draft => "\\Draft".to_string(),
        Flag::Recent => "\\Recent".to_string(),
        Flag::MayCreate => "\\*".to_string(),
        Flag::Custom(value) => value.to_string(),
    }
}

fn raw_message_to_plain_text(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let body = text
        .split_once("\r\n\r\n")
        .or_else(|| text.split_once("\n\n"))
        .map(|(_, body)| body)
        .unwrap_or(text.as_ref());
    normalize_plain_text(&strip_html_tags(body))
}

fn strip_html_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

fn normalize_plain_text(input: &str) -> String {
    let mut out = String::new();
    let mut blank_lines = 0;
    for line in input.lines() {
        let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
        let trimmed = collapsed.trim();
        if trimmed.is_empty() {
            blank_lines += 1;
            if blank_lines <= 1 {
                out.push('\n');
            }
        } else {
            blank_lines = 0;
            out.push_str(trimmed);
            out.push('\n');
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        display_name, next_uid_sync_plan, parent_path, raw_message_to_plain_text, sync_completed,
        updated_uid_cursors, UidSyncDirection, UidSyncPlan,
    };

    #[test]
    fn folder_display_name_uses_hierarchy_delimiter() {
        assert_eq!(display_name("INBOX/Projects/Uncloud", Some("/")), "Uncloud");
        assert_eq!(display_name("Archive.2026", Some(".")), "2026");
        assert_eq!(display_name("INBOX", Some("/")), "INBOX");
        assert_eq!(display_name("Flat", None), "Flat");
    }

    #[test]
    fn folder_parent_path_uses_hierarchy_delimiter() {
        assert_eq!(
            parent_path("INBOX/Projects/Uncloud", Some("/")).as_deref(),
            Some("INBOX/Projects")
        );
        assert_eq!(parent_path("INBOX", Some("/")), None);
        assert_eq!(parent_path("Flat", None), None);
    }

    #[test]
    fn next_uid_sync_plan_starts_with_latest_window() {
        assert_eq!(
            next_uid_sync_plan(None, None, Some(101), 10),
            Some(UidSyncPlan {
                start: 91,
                end: 100,
                direction: UidSyncDirection::Latest,
            })
        );
        assert_eq!(
            next_uid_sync_plan(None, Some(50), Some(101), 10),
            Some(UidSyncPlan {
                start: 91,
                end: 100,
                direction: UidSyncDirection::Latest,
            })
        );
        assert_eq!(next_uid_sync_plan(None, None, Some(1), 5), None);
        assert_eq!(next_uid_sync_plan(None, None, None, 5), None);
    }

    #[test]
    fn next_uid_sync_plan_prioritizes_new_mail_then_backfills() {
        assert_eq!(
            next_uid_sync_plan(Some(91), Some(100), Some(106), 3),
            Some(UidSyncPlan {
                start: 101,
                end: 103,
                direction: UidSyncDirection::Newer,
            })
        );
        assert_eq!(
            next_uid_sync_plan(Some(91), Some(105), Some(106), 3),
            Some(UidSyncPlan {
                start: 88,
                end: 90,
                direction: UidSyncDirection::Older,
            })
        );
        assert_eq!(next_uid_sync_plan(Some(1), Some(105), Some(106), 3), None);
    }

    #[test]
    fn uid_sync_cursor_updates_match_scan_direction() {
        assert_eq!(
            updated_uid_cursors(
                None,
                None,
                Some(UidSyncPlan {
                    start: 91,
                    end: 100,
                    direction: UidSyncDirection::Latest,
                })
            ),
            (Some(91), Some(100))
        );
        assert_eq!(
            updated_uid_cursors(
                Some(91),
                Some(100),
                Some(UidSyncPlan {
                    start: 101,
                    end: 105,
                    direction: UidSyncDirection::Newer,
                })
            ),
            (Some(91), Some(105))
        );
        assert_eq!(
            updated_uid_cursors(
                Some(91),
                Some(105),
                Some(UidSyncPlan {
                    start: 81,
                    end: 90,
                    direction: UidSyncDirection::Older,
                })
            ),
            (Some(81), Some(105))
        );
        assert!(sync_completed(Some(1), Some(105), Some(106)));
        assert!(!sync_completed(Some(2), Some(105), Some(106)));
        assert!(!sync_completed(Some(1), Some(104), Some(106)));
    }

    #[test]
    fn raw_message_to_plain_text_strips_headers_and_tags() {
        let raw = b"Subject: Hi\r\nContent-Type: text/html\r\n\r\n<html><body><p>Hello</p><p>World</p></body></html>";
        assert_eq!(raw_message_to_plain_text(raw), "Hello World");
    }
}
