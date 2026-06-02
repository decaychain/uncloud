use std::{
    collections::HashSet,
    fmt,
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_imap::types::{Capability, Flag};
use async_imap::{Client, Session};
use async_native_tls::TlsConnector;
use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use imap_proto::types::{BodyContentCommon, BodyParams, BodyStructure};
use lettre::transport::smtp::{
    authentication::Credentials,
    client::{Tls, TlsParameters},
    response::Response,
};
use lettre::{message::Mailbox, AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use mail_parser::{MessageParser, MimeHeaders, PartType};
use mongodb::bson::oid::ObjectId;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::time::timeout;
use uncloud_common::{MailConnectionTestResponse, MailSecurity};

use crate::error::{AppError, Result};
use crate::models::MailServerConfig;

const MAIL_OP_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Default)]
pub struct MailService {
    syncing_accounts: Arc<Mutex<HashSet<ObjectId>>>,
}

pub struct MailAccountSyncGuard {
    account_id: ObjectId,
    syncing_accounts: Arc<Mutex<HashSet<ObjectId>>>,
}

impl Drop for MailAccountSyncGuard {
    fn drop(&mut self) {
        let mut syncing = self
            .syncing_accounts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        syncing.remove(&self.account_id);
    }
}

impl MailService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn try_begin_account_sync(&self, account_id: ObjectId) -> Option<MailAccountSyncGuard> {
        let mut syncing = self
            .syncing_accounts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !syncing.insert(account_id) {
            return None;
        }
        Some(MailAccountSyncGuard {
            account_id,
            syncing_accounts: self.syncing_accounts.clone(),
        })
    }

    pub fn is_account_syncing(&self, account_id: ObjectId) -> bool {
        self.syncing_accounts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(&account_id)
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
    ) -> Result<RemoteMessageBody> {
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

    pub async fn set_imap_message_flag(
        &self,
        settings: &MailServerConfig,
        password: &str,
        folder_path: &str,
        uid: u32,
        flag: RemoteMessageFlag,
        enabled: bool,
    ) -> Result<Vec<String>> {
        match settings.security {
            MailSecurity::Tls => {
                let client = connect_imap_tls(settings, true).await?;
                set_imap_message_flag(client, settings, password, folder_path, uid, flag, enabled)
                    .await
            }
            MailSecurity::StartTls => {
                let client = connect_imap_starttls(settings).await?;
                set_imap_message_flag(client, settings, password, folder_path, uid, flag, enabled)
                    .await
            }
            MailSecurity::Plain => {
                let client = connect_imap_plain(settings).await?;
                set_imap_message_flag(client, settings, password, folder_path, uid, flag, enabled)
                    .await
            }
        }
    }

    pub async fn move_imap_message(
        &self,
        settings: &MailServerConfig,
        password: &str,
        source_folder_path: &str,
        uid: u32,
        destination_folder_path: &str,
    ) -> Result<()> {
        match settings.security {
            MailSecurity::Tls => {
                let client = connect_imap_tls(settings, true).await?;
                move_imap_message(
                    client,
                    settings,
                    password,
                    source_folder_path,
                    uid,
                    destination_folder_path,
                )
                .await
            }
            MailSecurity::StartTls => {
                let client = connect_imap_starttls(settings).await?;
                move_imap_message(
                    client,
                    settings,
                    password,
                    source_folder_path,
                    uid,
                    destination_folder_path,
                )
                .await
            }
            MailSecurity::Plain => {
                let client = connect_imap_plain(settings).await?;
                move_imap_message(
                    client,
                    settings,
                    password,
                    source_folder_path,
                    uid,
                    destination_folder_path,
                )
                .await
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
            .map_err(|err| map_smtp_error(err, "SMTP connection test"))?;

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

    pub async fn send_smtp_plain_text(
        &self,
        settings: &MailServerConfig,
        password: &str,
        message: RemoteOutgoingMessage,
    ) -> Result<RemoteSendResult> {
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
        let message_id = message.message_id.clone();
        let smtp_message = build_plain_text_message(message)?;
        let raw_message = smtp_message.formatted();
        let response = imap_timeout(
            "SMTP send",
            transport.send_raw(smtp_message.envelope(), &raw_message),
        )
        .await?
        .map_err(|err| map_smtp_error(err, "SMTP send"))?;
        transport.shutdown().await;

        Ok(RemoteSendResult {
            message_id,
            response: smtp_response_text(&response),
            raw_message,
        })
    }

    pub async fn imap_message_exists_by_message_id(
        &self,
        settings: &MailServerConfig,
        password: &str,
        folder_path: &str,
        message_id: &str,
    ) -> Result<bool> {
        match settings.security {
            MailSecurity::Tls => {
                let client = connect_imap_tls(settings, true).await?;
                imap_message_exists_by_message_id(
                    client,
                    settings,
                    password,
                    folder_path,
                    message_id,
                )
                .await
            }
            MailSecurity::StartTls => {
                let client = connect_imap_starttls(settings).await?;
                imap_message_exists_by_message_id(
                    client,
                    settings,
                    password,
                    folder_path,
                    message_id,
                )
                .await
            }
            MailSecurity::Plain => {
                let client = connect_imap_plain(settings).await?;
                imap_message_exists_by_message_id(
                    client,
                    settings,
                    password,
                    folder_path,
                    message_id,
                )
                .await
            }
        }
    }

    pub async fn append_imap_message(
        &self,
        settings: &MailServerConfig,
        password: &str,
        folder_path: &str,
        raw_message: &[u8],
    ) -> Result<()> {
        match settings.security {
            MailSecurity::Tls => {
                let client = connect_imap_tls(settings, true).await?;
                append_imap_message(client, settings, password, folder_path, raw_message).await
            }
            MailSecurity::StartTls => {
                let client = connect_imap_starttls(settings).await?;
                append_imap_message(client, settings, password, folder_path, raw_message).await
            }
            MailSecurity::Plain => {
                let client = connect_imap_plain(settings).await?;
                append_imap_message(client, settings, password, folder_path, raw_message).await
            }
        }
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

    if mailbox.exists == 0 {
        let _ = session.logout().await;
        return Ok(RemoteMailboxSync {
            status: RemoteMailboxStatus {
                uid_validity: mailbox.uid_validity,
                uid_next: mailbox.uid_next,
                exists: Some(mailbox.exists),
                unseen: mailbox.unseen,
            },
            uid_validity_changed,
            synced_uid_ranges: Vec::new(),
            lowest_synced_uid: None,
            highest_synced_uid: None,
            completed: true,
            messages: Vec::new(),
        });
    }

    let plan = next_uid_sync_plan(effective_lowest, effective_highest, mailbox.uid_next, limit);
    let fetch_plans = sync_fetch_plans(plan, effective_lowest, effective_highest, limit);

    let messages = if fetch_plans.is_empty() {
        Vec::new()
    } else {
        let uid_set = fetch_plans
            .iter()
            .map(|plan| format!("{}:{}", plan.start, plan.end))
            .collect::<Vec<_>>()
            .join(",");
        let fetch_stream = imap_timeout(
            "IMAP UID FETCH",
            session.uid_fetch(
                uid_set,
                "(UID FLAGS INTERNALDATE RFC822.SIZE ENVELOPE BODYSTRUCTURE)",
            ),
        )
        .await?
        .map_err(|e| AppError::Internal(format!("IMAP UID FETCH failed: {e}")))?
        .try_collect();
        let fetches: Vec<_> = imap_timeout("IMAP UID FETCH read", fetch_stream)
            .await?
            .map_err(|e| AppError::Internal(format!("IMAP UID FETCH read failed: {e}")))?;
        fetches.iter().filter_map(remote_message_summary).collect()
    };

    let _ = session.logout().await;
    let synced_uid_ranges = fetch_plans
        .iter()
        .map(|plan| RemoteUidRange {
            start: plan.start,
            end: plan.end,
        })
        .collect();
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
        synced_uid_ranges,
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
) -> Result<RemoteMessageBody>
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
    Ok(parse_message_body(&body))
}

async fn set_imap_message_flag<T>(
    client: Client<T>,
    settings: &MailServerConfig,
    password: &str,
    folder_path: &str,
    uid: u32,
    flag: RemoteMessageFlag,
    enabled: bool,
) -> Result<Vec<String>>
where
    T: AsyncRead + AsyncWrite + Unpin + fmt::Debug + Send,
{
    let mut session = login_imap_client(client, settings, password).await?;
    imap_timeout("IMAP SELECT", session.select(folder_path))
        .await?
        .map_err(|e| AppError::Internal(format!("IMAP SELECT failed: {e}")))?;

    {
        let query = flag_store_query(flag, enabled);
        let updates_stream =
            imap_timeout("IMAP UID STORE", session.uid_store(uid.to_string(), query))
                .await?
                .map_err(|e| AppError::BadRequest(format!("IMAP UID STORE failed: {e}")))?
                .try_collect();
        let _updates: Vec<_> = imap_timeout("IMAP UID STORE read", updates_stream)
            .await?
            .map_err(|e| AppError::Internal(format!("IMAP UID STORE read failed: {e}")))?;
    }

    let flags = fetch_imap_message_flags(&mut session, uid).await?;
    let _ = session.logout().await;
    Ok(flags)
}

async fn move_imap_message<T>(
    client: Client<T>,
    settings: &MailServerConfig,
    password: &str,
    source_folder_path: &str,
    uid: u32,
    destination_folder_path: &str,
) -> Result<()>
where
    T: AsyncRead + AsyncWrite + Unpin + fmt::Debug + Send,
{
    let mut session = login_imap_client(client, settings, password).await?;
    imap_timeout("IMAP SELECT", session.select(source_folder_path))
        .await?
        .map_err(|e| AppError::Internal(format!("IMAP SELECT failed: {e}")))?;
    imap_timeout(
        "IMAP UID MOVE",
        session.uid_mv(uid.to_string(), destination_folder_path),
    )
    .await?
    .map_err(|e| AppError::BadRequest(format!("IMAP UID MOVE failed: {e}")))?;
    let _ = session.logout().await;
    Ok(())
}

async fn fetch_imap_message_flags<T>(session: &mut Session<T>, uid: u32) -> Result<Vec<String>>
where
    T: AsyncRead + AsyncWrite + Unpin + fmt::Debug + Send,
{
    let fetch_stream = imap_timeout(
        "IMAP UID FETCH flags",
        session.uid_fetch(uid.to_string(), "(UID FLAGS)"),
    )
    .await?
    .map_err(|e| AppError::Internal(format!("IMAP UID FETCH flags failed: {e}")))?
    .try_collect();
    let fetches: Vec<_> = imap_timeout("IMAP UID FETCH flags read", fetch_stream)
        .await?
        .map_err(|e| AppError::Internal(format!("IMAP UID FETCH flags read failed: {e}")))?;
    fetches
        .iter()
        .find(|fetch| fetch.uid == Some(uid))
        .map(|fetch| fetch.flags().map(flag_name).collect())
        .ok_or_else(|| AppError::NotFound("Mail message".into()))
}

async fn imap_message_exists_by_message_id<T>(
    client: Client<T>,
    settings: &MailServerConfig,
    password: &str,
    folder_path: &str,
    message_id: &str,
) -> Result<bool>
where
    T: AsyncRead + AsyncWrite + Unpin + fmt::Debug + Send,
{
    let mut session = login_imap_client(client, settings, password).await?;
    imap_timeout("IMAP EXAMINE", session.examine(folder_path))
        .await?
        .map_err(|e| AppError::Internal(format!("IMAP EXAMINE failed: {e}")))?;
    let query = format!(
        "HEADER Message-ID {}",
        quoted_imap_search_string(message_id)?
    );
    let uids = imap_timeout("IMAP UID SEARCH", session.uid_search(query))
        .await?
        .map_err(|e| AppError::Internal(format!("IMAP UID SEARCH failed: {e}")))?;
    let _ = session.logout().await;
    Ok(!uids.is_empty())
}

async fn append_imap_message<T>(
    client: Client<T>,
    settings: &MailServerConfig,
    password: &str,
    folder_path: &str,
    raw_message: &[u8],
) -> Result<()>
where
    T: AsyncRead + AsyncWrite + Unpin + fmt::Debug + Send,
{
    let mut session = login_imap_client(client, settings, password).await?;
    imap_timeout(
        "IMAP APPEND",
        session.append(folder_path, Some("(\\Seen)"), None, raw_message),
    )
    .await?
    .map_err(|e| AppError::Internal(format!("IMAP APPEND failed: {e}")))?;
    let _ = session.logout().await;
    Ok(())
}

fn smtp_tls_parameters(settings: &MailServerConfig) -> Result<TlsParameters> {
    TlsParameters::new(settings.host.clone())
        .map_err(|e| AppError::Internal(format!("SMTP TLS configuration failed: {e}")))
}

fn map_smtp_error(err: lettre::transport::smtp::Error, operation: &str) -> AppError {
    if err.is_client() || err.is_response() || err.is_permanent() {
        AppError::BadRequest(format!("{operation} failed: {err}"))
    } else {
        AppError::Internal(format!("{operation} failed: {err}"))
    }
}

fn build_plain_text_message(message: RemoteOutgoingMessage) -> Result<Message> {
    let mut builder = Message::builder()
        .message_id(Some(message.message_id.clone()))
        .from(mailbox_from_address(&message.from)?)
        .subject(message.subject);

    if let Some(reply_to) = message.reply_to {
        builder = builder.reply_to(mailbox_from_address(&reply_to)?);
    }
    for recipient in message.to {
        builder = builder.to(mailbox_from_address(&recipient)?);
    }
    for recipient in message.cc {
        builder = builder.cc(mailbox_from_address(&recipient)?);
    }
    for recipient in message.bcc {
        builder = builder.bcc(mailbox_from_address(&recipient)?);
    }

    builder
        .body(message.body_text)
        .map_err(|e| AppError::BadRequest(format!("mail message could not be built: {e}")))
}

fn mailbox_from_address(address: &RemoteMailAddress) -> Result<Mailbox> {
    if address.name.as_deref().is_none_or(str::is_empty) {
        if let Ok(mailbox) = address.address.parse::<Mailbox>() {
            return Ok(mailbox);
        }
    }
    let email = address
        .address
        .parse()
        .map_err(|e| AppError::BadRequest(format!("invalid email address: {e}")))?;
    Ok(Mailbox::new(
        address.name.as_ref().map(|value| value.trim().to_string()),
        email,
    ))
}

fn smtp_response_text(response: &Response) -> Option<String> {
    response
        .first_line()
        .map(str::to_string)
        .or_else(|| response.message().next().map(str::to_string))
}

fn quoted_imap_search_string(value: &str) -> Result<String> {
    if value.contains('\r') || value.contains('\n') {
        return Err(AppError::BadRequest(
            "mail search value cannot contain line breaks".into(),
        ));
    }
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        if ch == '"' || ch == '\\' {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
    Ok(out)
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
    pub synced_uid_ranges: Vec<RemoteUidRange>,
    pub lowest_synced_uid: Option<u32>,
    pub highest_synced_uid: Option<u32>,
    pub completed: bool,
    pub messages: Vec<RemoteMessageSummary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteUidRange {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone)]
pub struct RemoteMailAddress {
    pub name: Option<String>,
    pub address: String,
}

#[derive(Debug, Clone)]
pub struct RemoteOutgoingMessage {
    pub message_id: String,
    pub from: RemoteMailAddress,
    pub reply_to: Option<RemoteMailAddress>,
    pub to: Vec<RemoteMailAddress>,
    pub cc: Vec<RemoteMailAddress>,
    pub bcc: Vec<RemoteMailAddress>,
    pub subject: String,
    pub body_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSendResult {
    pub message_id: String,
    pub response: Option<String>,
    pub raw_message: Vec<u8>,
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
    pub has_attachments: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteMessageBody {
    pub raw: Vec<u8>,
    pub text: Option<String>,
    pub html: Option<String>,
    pub attachments: Vec<RemoteAttachment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteAttachment {
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub content_id: Option<String>,
    pub disposition: Option<String>,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteMessageFlag {
    Seen,
    Flagged,
}

fn flag_store_query(flag: RemoteMessageFlag, enabled: bool) -> &'static str {
    match (flag, enabled) {
        (RemoteMessageFlag::Seen, true) => "+FLAGS.SILENT (\\Seen)",
        (RemoteMessageFlag::Seen, false) => "-FLAGS.SILENT (\\Seen)",
        (RemoteMessageFlag::Flagged, true) => "+FLAGS.SILENT (\\Flagged)",
        (RemoteMessageFlag::Flagged, false) => "-FLAGS.SILENT (\\Flagged)",
    }
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
    Refresh,
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
        let end = last_available;
        let start = end.saturating_sub(limit.saturating_sub(1)).max(1);
        return Some(UidSyncPlan {
            start,
            end,
            direction: UidSyncDirection::Refresh,
        });
    }
    let end = lowest.saturating_sub(1);
    let start = end.saturating_sub(limit.saturating_sub(1)).max(1);
    Some(UidSyncPlan {
        start,
        end,
        direction: UidSyncDirection::Older,
    })
}

fn latest_cached_refresh_plan(
    lowest_synced_uid: Option<u32>,
    highest_synced_uid: Option<u32>,
    limit: u32,
) -> Option<UidSyncPlan> {
    let limit = limit.max(1);
    let lowest = lowest_synced_uid?;
    let highest = highest_synced_uid?;
    if lowest > highest {
        return None;
    }

    let end = highest;
    let start = end.saturating_sub(limit.saturating_sub(1)).max(lowest);
    Some(UidSyncPlan {
        start,
        end,
        direction: UidSyncDirection::Refresh,
    })
}

fn sync_fetch_plans(
    primary: Option<UidSyncPlan>,
    lowest_synced_uid: Option<u32>,
    highest_synced_uid: Option<u32>,
    limit: u32,
) -> Vec<UidSyncPlan> {
    let mut plans = primary.into_iter().collect::<Vec<_>>();
    let refresh_needed = primary
        .map(|plan| {
            !matches!(
                plan.direction,
                UidSyncDirection::Latest | UidSyncDirection::Refresh
            )
        })
        .unwrap_or(false);
    if refresh_needed {
        if let Some(refresh) =
            latest_cached_refresh_plan(lowest_synced_uid, highest_synced_uid, limit)
        {
            if !plans
                .iter()
                .any(|plan| plan.start == refresh.start && plan.end == refresh.end)
            {
                plans.push(refresh);
            }
        }
    }
    plans
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
        UidSyncDirection::Refresh => (current_lowest, current_highest),
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
        has_attachments: fetch
            .bodystructure()
            .map(bodystructure_has_attachment)
            .unwrap_or(false),
    })
}

fn bodystructure_has_attachment(structure: &BodyStructure<'_>) -> bool {
    match structure {
        BodyStructure::Basic { common, .. } | BodyStructure::Text { common, .. } => {
            body_common_has_attachment(common)
        }
        BodyStructure::Message { common, body, .. } => {
            body_common_has_attachment(common) || bodystructure_has_attachment(body)
        }
        BodyStructure::Multipart { bodies, .. } => bodies.iter().any(bodystructure_has_attachment),
    }
}

fn body_common_has_attachment(common: &BodyContentCommon<'_>) -> bool {
    let disposition_has_attachment = common
        .disposition
        .as_ref()
        .map(|disposition| {
            disposition.ty.eq_ignore_ascii_case("attachment")
                || body_params_has_key(&disposition.params, "filename")
        })
        .unwrap_or(false);
    disposition_has_attachment || body_params_has_key(&common.ty.params, "name")
}

fn body_params_has_key(params: &BodyParams<'_>, key: &str) -> bool {
    params
        .as_ref()
        .map(|params| params.iter().any(|(name, _)| name.eq_ignore_ascii_case(key)))
        .unwrap_or(false)
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

fn parse_message_body(raw: &[u8]) -> RemoteMessageBody {
    let Some(message) = MessageParser::default().parse(raw) else {
        return RemoteMessageBody {
            raw: raw.to_vec(),
            text: Some(raw_message_to_plain_text_fallback(raw)),
            html: None,
            attachments: Vec::new(),
        };
    };

    let text = message
        .body_text(0)
        .map(|body| body.into_owned())
        .map(|body| normalize_plain_text(&body))
        .filter(|body| !body.is_empty());
    let html = message
        .body_html(0)
        .map(|body| body.into_owned())
        .map(|body| sanitize_message_html(&body))
        .filter(|body| !body.trim().is_empty());

    if text.is_some() || html.is_some() {
        RemoteMessageBody {
            raw: raw.to_vec(),
            text,
            html,
            attachments: extract_message_attachments(&message),
        }
    } else {
        RemoteMessageBody {
            raw: raw.to_vec(),
            text: Some(raw_message_to_plain_text_fallback(raw)),
            html: None,
            attachments: extract_message_attachments(&message),
        }
    }
}

fn extract_message_attachments(message: &mail_parser::Message<'_>) -> Vec<RemoteAttachment> {
    message
        .attachments()
        .filter_map(|part| {
            let data = match &part.body {
                PartType::Binary(bytes) | PartType::InlineBinary(bytes) => bytes.as_ref().to_vec(),
                PartType::Text(text) | PartType::Html(text) => text.as_bytes().to_vec(),
                PartType::Message(message) => message.raw_message.as_ref().to_vec(),
                PartType::Multipart(_) => return None,
            };
            Some(RemoteAttachment {
                filename: part.attachment_name().map(str::to_string),
                content_type: part.content_type().map(content_type_label),
                content_id: part.content_id().map(str::to_string),
                disposition: part
                    .content_disposition()
                    .map(|disposition| disposition.ctype().to_string()),
                data,
            })
        })
        .collect()
}

fn content_type_label(content_type: &mail_parser::ContentType<'_>) -> String {
    match content_type.subtype() {
        Some(subtype) => format!("{}/{}", content_type.ctype(), subtype),
        None => content_type.ctype().to_string(),
    }
}

fn sanitize_message_html(input: &str) -> String {
    let mut builder = ammonia::Builder::default();
    builder.url_relative(ammonia::UrlRelative::Deny);
    builder.attribute_filter(|element, attribute, value| match (element, attribute) {
        ("img", "src") | ("img", "srcset") | ("source", "src") | ("source", "srcset") => None,
        _ => Some(value.into()),
    });
    builder.clean(input).to_string()
}

fn raw_message_to_plain_text_fallback(raw: &[u8]) -> String {
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
        bodystructure_has_attachment, build_plain_text_message, display_name, flag_store_query,
        latest_cached_refresh_plan, mailbox_from_address, next_uid_sync_plan, parent_path,
        parse_message_body, quoted_imap_search_string, sync_completed, sync_fetch_plans,
        updated_uid_cursors, RemoteMailAddress, RemoteMessageFlag, RemoteOutgoingMessage,
        MailService, UidSyncDirection, UidSyncPlan,
    };
    use imap_proto::types::{
        BodyContentCommon, BodyContentSinglePart, BodyStructure, ContentDisposition,
        ContentEncoding, ContentType,
    };
    use mongodb::bson::oid::ObjectId;
    use std::borrow::Cow;

    #[test]
    fn account_sync_guard_prevents_overlap_until_dropped() {
        let service = MailService::new();
        let account_id = ObjectId::new();

        let guard = service.try_begin_account_sync(account_id).unwrap();
        assert!(service.is_account_syncing(account_id));
        assert!(service.try_begin_account_sync(account_id).is_none());

        drop(guard);
        assert!(!service.is_account_syncing(account_id));
        assert!(service.try_begin_account_sync(account_id).is_some());
    }

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
    fn flag_store_query_uses_silent_uid_store_terms() {
        assert_eq!(
            flag_store_query(RemoteMessageFlag::Seen, true),
            "+FLAGS.SILENT (\\Seen)"
        );
        assert_eq!(
            flag_store_query(RemoteMessageFlag::Seen, false),
            "-FLAGS.SILENT (\\Seen)"
        );
        assert_eq!(
            flag_store_query(RemoteMessageFlag::Flagged, true),
            "+FLAGS.SILENT (\\Flagged)"
        );
        assert_eq!(
            flag_store_query(RemoteMessageFlag::Flagged, false),
            "-FLAGS.SILENT (\\Flagged)"
        );
    }

    #[test]
    fn bodystructure_has_attachment_detects_disposition_and_named_parts() {
        let plain_text = BodyStructure::Text {
            common: body_common("TEXT", "PLAIN", None, None),
            other: body_single_part(),
            lines: 1,
            extension: None,
        };
        assert!(!bodystructure_has_attachment(&plain_text));

        let pdf_attachment = BodyStructure::Basic {
            common: body_common(
                "APPLICATION",
                "PDF",
                None,
                Some(("attachment", Some(("filename", "statement.pdf")))),
            ),
            other: body_single_part(),
            extension: None,
        };
        assert!(bodystructure_has_attachment(&pdf_attachment));

        let named_text_attachment = BodyStructure::Text {
            common: body_common("TEXT", "CSV", Some(("name", "export.csv")), None),
            other: body_single_part(),
            lines: 5,
            extension: None,
        };
        assert!(bodystructure_has_attachment(&named_text_attachment));

        let multipart = BodyStructure::Multipart {
            common: body_common("MULTIPART", "MIXED", None, None),
            bodies: vec![plain_text, pdf_attachment],
            extension: None,
        };
        assert!(bodystructure_has_attachment(&multipart));
    }

    fn body_common(
        ty: &'static str,
        subtype: &'static str,
        type_param: Option<(&'static str, &'static str)>,
        disposition: Option<(&'static str, Option<(&'static str, &'static str)>)>,
    ) -> BodyContentCommon<'static> {
        BodyContentCommon {
            ty: ContentType {
                ty: Cow::Borrowed(ty),
                subtype: Cow::Borrowed(subtype),
                params: type_param
                    .map(|(key, value)| vec![(Cow::Borrowed(key), Cow::Borrowed(value))]),
            },
            disposition: disposition.map(|(ty, param)| ContentDisposition {
                ty: Cow::Borrowed(ty),
                params: param.map(|(key, value)| vec![(Cow::Borrowed(key), Cow::Borrowed(value))]),
            }),
            language: None,
            location: None,
        }
    }

    fn body_single_part() -> BodyContentSinglePart<'static> {
        BodyContentSinglePart {
            id: None,
            md5: None,
            description: None,
            transfer_encoding: ContentEncoding::SevenBit,
            octets: 42,
        }
    }

    #[test]
    fn mailbox_from_address_accepts_display_address_syntax() {
        let mailbox = mailbox_from_address(&RemoteMailAddress {
            name: None,
            address: "Example User <user@example.com>".to_string(),
        })
        .unwrap();

        assert_eq!(mailbox.name.as_deref(), Some("Example User"));
        assert_eq!(mailbox.email.to_string(), "user@example.com");
    }

    #[test]
    fn build_plain_text_message_requires_valid_recipients() {
        let err = build_plain_text_message(RemoteOutgoingMessage {
            message_id: "<test@uncloud.local>".to_string(),
            from: RemoteMailAddress {
                name: Some("Sender".to_string()),
                address: "sender@example.com".to_string(),
            },
            reply_to: None,
            to: vec![RemoteMailAddress {
                name: None,
                address: "not-an-address".to_string(),
            }],
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: "Test".to_string(),
            body_text: "Body".to_string(),
        })
        .unwrap_err();

        assert!(err.to_string().contains("invalid email address"));
    }

    #[test]
    fn build_plain_text_message_omits_bcc_header() {
        let message = build_plain_text_message(RemoteOutgoingMessage {
            message_id: "<test@uncloud.local>".to_string(),
            from: RemoteMailAddress {
                name: Some("Sender".to_string()),
                address: "sender@example.com".to_string(),
            },
            reply_to: None,
            to: vec![RemoteMailAddress {
                name: None,
                address: "visible@example.com".to_string(),
            }],
            cc: Vec::new(),
            bcc: vec![RemoteMailAddress {
                name: None,
                address: "hidden@example.com".to_string(),
            }],
            subject: "Test".to_string(),
            body_text: "Body".to_string(),
        })
        .unwrap();
        let formatted = String::from_utf8(message.formatted()).unwrap();

        assert!(!formatted.contains("Bcc:"));
        assert!(!formatted.contains("hidden@example.com"));
    }

    #[test]
    fn quoted_imap_search_string_escapes_special_chars() {
        assert_eq!(
            quoted_imap_search_string(r#"<a"b\c@example.com>"#).unwrap(),
            r#""<a\"b\\c@example.com>""#
        );
        assert!(quoted_imap_search_string("bad\r\nvalue").is_err());
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
    }

    #[test]
    fn next_uid_sync_plan_refreshes_latest_window_when_complete() {
        assert_eq!(
            next_uid_sync_plan(Some(1), Some(105), Some(106), 3),
            Some(UidSyncPlan {
                start: 103,
                end: 105,
                direction: UidSyncDirection::Refresh,
            })
        );
    }

    #[test]
    fn latest_cached_refresh_plan_stays_inside_cached_window() {
        assert_eq!(
            latest_cached_refresh_plan(Some(91), Some(105), 10),
            Some(UidSyncPlan {
                start: 96,
                end: 105,
                direction: UidSyncDirection::Refresh,
            })
        );
        assert_eq!(
            latest_cached_refresh_plan(Some(101), Some(105), 10),
            Some(UidSyncPlan {
                start: 101,
                end: 105,
                direction: UidSyncDirection::Refresh,
            })
        );
        assert_eq!(latest_cached_refresh_plan(None, Some(105), 10), None);
    }

    #[test]
    fn sync_fetch_plans_refresh_latest_cached_window_during_backfill() {
        let primary = Some(UidSyncPlan {
            start: 81,
            end: 90,
            direction: UidSyncDirection::Older,
        });

        assert_eq!(
            sync_fetch_plans(primary, Some(91), Some(105), 3),
            vec![
                UidSyncPlan {
                    start: 81,
                    end: 90,
                    direction: UidSyncDirection::Older,
                },
                UidSyncPlan {
                    start: 103,
                    end: 105,
                    direction: UidSyncDirection::Refresh,
                },
            ]
        );
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
        assert_eq!(
            updated_uid_cursors(
                Some(1),
                Some(105),
                Some(UidSyncPlan {
                    start: 103,
                    end: 105,
                    direction: UidSyncDirection::Refresh,
                })
            ),
            (Some(1), Some(105))
        );
        assert!(sync_completed(Some(1), Some(105), Some(106)));
        assert!(!sync_completed(Some(2), Some(105), Some(106)));
        assert!(!sync_completed(Some(1), Some(104), Some(106)));
    }

    #[test]
    fn parse_message_body_decodes_text_and_sanitizes_html() {
        let raw = b"Subject: Hi\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<html><body><p onclick=\"bad()\">Hello</p><script>alert(1)</script><img src=\"https://example.com/pixel.png\" srcset=\"https://example.com/large.png 2x\" alt=\"pixel\"><a href=\"/local\">Local</a><a href=\"https://example.com\">Example</a><p>World</p></body></html>";
        let body = parse_message_body(raw);

        let text = body.text.as_deref().unwrap_or_default();
        assert!(text.contains("Hello"));
        assert!(text.contains("World"));
        assert!(!text.contains("alert"));
        let html = body.html.as_deref().unwrap_or_default();
        assert!(html.contains("Hello"));
        assert!(html.contains("World"));
        assert!(!html.contains("script"));
        assert!(!html.contains("onclick"));
        assert!(!html.contains("src="));
        assert!(!html.contains("srcset"));
        assert!(!html.contains("/local"));
        assert!(html.contains("https://example.com"));
    }

    #[test]
    fn parse_message_body_falls_back_when_parser_cannot_find_a_body() {
        let raw = b"";
        let body = parse_message_body(raw);

        assert_eq!(body.text.as_deref(), Some(""));
        assert_eq!(body.html, None);
    }

    #[test]
    fn parse_message_body_extracts_attachments() {
        let raw = b"Subject: File\r\nContent-Type: multipart/mixed; boundary=\"x\"\r\n\r\n--x\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nSee attached.\r\n--x\r\nContent-Type: text/plain; name=\"note.txt\"\r\nContent-Disposition: attachment; filename=\"note.txt\"\r\n\r\nhello attachment\r\n--x--\r\n";
        let body = parse_message_body(raw);

        assert_eq!(body.attachments.len(), 1);
        let attachment = &body.attachments[0];
        assert_eq!(attachment.filename.as_deref(), Some("note.txt"));
        assert_eq!(attachment.content_type.as_deref(), Some("text/plain"));
        assert_eq!(attachment.disposition.as_deref(), Some("attachment"));
        assert_eq!(attachment.data, b"hello attachment");
    }
}
