use std::future::Future;
use std::time::Duration;

use async_imap::types::Capability;
use async_native_tls::TlsConnector;
use futures::TryStreamExt;
use tokio::net::TcpStream;
use tokio::time::timeout;
use uncloud_common::{MailConnectionTestResponse, MailSecurity};

use crate::error::{AppError, Result};
use crate::models::MailServerConfig;

const IMAP_OP_TIMEOUT: Duration = Duration::from_secs(30);

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
        if settings.security != MailSecurity::Tls {
            return Err(AppError::BadRequest(
                "Only implicit TLS IMAP is wired for the experimental mail foundation".into(),
            ));
        }

        let tcp = imap_timeout(
            "IMAP TCP connection",
            TcpStream::connect((settings.host.as_str(), settings.port)),
        )
        .await?
        .map_err(|e| AppError::Internal(format!("IMAP TCP connection failed: {e}")))?;
        let tls = imap_timeout(
            "IMAP TLS handshake",
            TlsConnector::new().connect(settings.host.as_str(), tcp),
        )
        .await?
        .map_err(|e| AppError::Internal(format!("IMAP TLS handshake failed: {e}")))?;
        let mut client = async_imap::Client::new(tls);
        let _ = imap_timeout("IMAP greeting", client.read_response())
            .await?
            .map_err(|e| AppError::Internal(format!("IMAP greeting failed: {e}")))?;

        let mut session = imap_timeout("IMAP login", client.login(&settings.username, password))
            .await?
            .map_err(|(e, _)| AppError::BadRequest(format!("IMAP login failed: {e}")))?;

        let capabilities = imap_timeout("IMAP CAPABILITY", session.capabilities())
            .await?
            .map_err(|e| AppError::Internal(format!("IMAP CAPABILITY failed: {e}")))?
            .iter()
            .map(capability_name)
            .collect();

        let _ = session.logout().await;

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
        if settings.security != MailSecurity::Tls {
            return Err(AppError::BadRequest(
                "Only implicit TLS IMAP is wired for the experimental mail foundation".into(),
            ));
        }

        let tcp = imap_timeout(
            "IMAP TCP connection",
            TcpStream::connect((settings.host.as_str(), settings.port)),
        )
        .await?
        .map_err(|e| AppError::Internal(format!("IMAP TCP connection failed: {e}")))?;
        let tls = imap_timeout(
            "IMAP TLS handshake",
            TlsConnector::new().connect(settings.host.as_str(), tcp),
        )
        .await?
        .map_err(|e| AppError::Internal(format!("IMAP TLS handshake failed: {e}")))?;
        let mut client = async_imap::Client::new(tls);
        let _ = imap_timeout("IMAP greeting", client.read_response())
            .await?
            .map_err(|e| AppError::Internal(format!("IMAP greeting failed: {e}")))?;

        let mut session = imap_timeout("IMAP login", client.login(&settings.username, password))
            .await?
            .map_err(|(e, _)| AppError::BadRequest(format!("IMAP login failed: {e}")))?;

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
            let attributes: Vec<String> =
                name.attributes().iter().map(|a| format!("{a:?}")).collect();
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
}

async fn imap_timeout<T, F>(operation: &'static str, future: F) -> Result<T>
where
    F: Future<Output = T>,
{
    timeout(IMAP_OP_TIMEOUT, future)
        .await
        .map_err(|_| AppError::Internal(format!("{operation} timed out after 30s")))
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
