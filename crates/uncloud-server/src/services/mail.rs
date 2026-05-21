use std::fmt;
use std::future::Future;
use std::time::Duration;

use async_imap::types::Capability;
use async_imap::{Client, Session};
use async_native_tls::TlsConnector;
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

#[cfg(test)]
mod tests {
    use super::{display_name, parent_path};

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
}
