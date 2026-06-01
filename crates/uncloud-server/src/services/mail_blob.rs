use mongodb::bson::oid::ObjectId;

use crate::error::Result;
use crate::models::{MailAccount, MailFolder, MailMessage};
use crate::services::mail::RemoteMessageBody;
use crate::services::StorageService;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredMailBody {
    pub storage_id: ObjectId,
    pub raw_path: String,
    pub raw_size_bytes: u64,
    pub text_path: Option<String>,
    pub text_size_bytes: Option<u64>,
    pub html_path: Option<String>,
    pub html_size_bytes: Option<u64>,
}

fn sanitize_storage_component(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' | '\\' | '\0' => '_',
            c => c,
        })
        .collect()
}

fn message_body_prefix(
    username: &str,
    account: &MailAccount,
    folder: &MailFolder,
    message: &MailMessage,
) -> String {
    let user_prefix = sanitize_storage_component(username);
    let uid_validity = folder
        .uid_validity
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    format!(
        "{user_prefix}/.uncloud/mail/v1/accounts/{}/folders/{}/uidvalidity-{uid_validity}/uid-{}",
        account.id.to_hex(),
        folder.id.to_hex(),
        message.uid,
    )
}

pub async fn read_cached_message_body(
    storage: &StorageService,
    message: &MailMessage,
) -> Result<Option<RemoteMessageBody>> {
    let Some(storage_id) = message.mail_storage_id else {
        return Ok(None);
    };
    let text_path = message.text_storage_path.as_deref();
    let html_path = message.html_storage_path.as_deref();
    if text_path.is_none() && html_path.is_none() {
        return Ok(None);
    }

    let backend = storage.get_backend(storage_id).await?;
    let text = if let Some(path) = text_path {
        Some(String::from_utf8_lossy(&backend.read_all(path).await?).into_owned())
    } else {
        None
    };
    let html = if let Some(path) = html_path {
        Some(String::from_utf8_lossy(&backend.read_all(path).await?).into_owned())
    } else {
        None
    };

    Ok(Some(RemoteMessageBody {
        raw: Vec::new(),
        text,
        html,
    }))
}

pub async fn store_message_body(
    storage: &StorageService,
    username: &str,
    account: &MailAccount,
    folder: &MailFolder,
    message: &MailMessage,
    body: &RemoteMessageBody,
) -> Result<StoredMailBody> {
    let storage_id = account
        .mail_storage_id
        .unwrap_or_else(|| storage.default_storage_id());
    let backend = storage.get_backend(storage_id).await?;
    let prefix = message_body_prefix(username, account, folder, message);

    let raw_path = format!("{prefix}/raw.eml");
    backend.write(&raw_path, &body.raw).await?;
    let raw_size_bytes = body.raw.len() as u64;

    let (text_path, text_size_bytes) = if let Some(text) = body.text.as_ref() {
        let path = format!("{prefix}/body.txt");
        let bytes = text.as_bytes();
        backend.write(&path, bytes).await?;
        (Some(path), Some(bytes.len() as u64))
    } else {
        (None, None)
    };

    let (html_path, html_size_bytes) = if let Some(html) = body.html.as_ref() {
        let path = format!("{prefix}/body.html");
        let bytes = html.as_bytes();
        backend.write(&path, bytes).await?;
        (Some(path), Some(bytes.len() as u64))
    } else {
        (None, None)
    };

    Ok(StoredMailBody {
        storage_id,
        raw_path,
        raw_size_bytes,
        text_path,
        text_size_bytes,
        html_path,
        html_size_bytes,
    })
}
