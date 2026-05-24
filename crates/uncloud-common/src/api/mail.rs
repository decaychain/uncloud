use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailSecurity {
    Tls,
    StartTls,
    Plain,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailServerSettings {
    pub host: String,
    pub port: u16,
    pub security: MailSecurity,
    pub username: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailAccountResponse {
    pub id: String,
    pub display_name: String,
    pub email_address: String,
    pub imap: MailServerSettings,
    pub smtp: MailServerSettings,
    pub enabled: bool,
    pub sync_enabled: bool,
    pub credential_configured: bool,
    pub created_at: String,
    pub updated_at: String,
    pub last_sync_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateMailAccountRequest {
    pub display_name: String,
    pub email_address: String,
    pub imap: MailServerSettings,
    pub smtp: MailServerSettings,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub sync_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UpdateMailAccountRequest {
    pub display_name: Option<String>,
    pub email_address: Option<String>,
    pub imap: Option<MailServerSettings>,
    pub smtp: Option<MailServerSettings>,
    pub enabled: Option<bool>,
    pub sync_enabled: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailIdentityResponse {
    pub id: String,
    pub account_id: String,
    pub display_name: String,
    pub email_address: String,
    pub reply_to: Option<String>,
    pub signature: Option<String>,
    pub is_default: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateMailIdentityRequest {
    pub account_id: String,
    pub display_name: String,
    pub email_address: String,
    #[serde(default)]
    pub reply_to: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default)]
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UpdateMailIdentityRequest {
    pub display_name: Option<String>,
    pub email_address: Option<String>,
    pub reply_to: Option<Option<String>>,
    pub signature: Option<Option<String>>,
    pub is_default: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailFolderResponse {
    pub id: String,
    pub account_id: String,
    pub path: String,
    pub name: String,
    pub delimiter: Option<String>,
    pub parent_path: Option<String>,
    pub role: Option<String>,
    pub selectable: bool,
    pub attributes: Vec<String>,
    pub uid_validity: Option<u32>,
    pub uid_next: Option<u32>,
    pub exists: Option<u32>,
    pub unseen: Option<u32>,
    pub highest_synced_uid: Option<u32>,
    pub lowest_synced_uid: Option<u32>,
    pub last_sync_started_at: Option<String>,
    pub last_sync_finished_at: Option<String>,
    pub last_sync_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailAddressDto {
    pub name: Option<String>,
    pub address: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailMessageSummaryResponse {
    pub id: String,
    pub account_id: String,
    pub folder_id: String,
    pub folder_path: String,
    pub uid: u32,
    pub message_id: Option<String>,
    pub thread_id: Option<String>,
    pub subject: Option<String>,
    pub from: Vec<MailAddressDto>,
    pub to: Vec<MailAddressDto>,
    pub cc: Vec<MailAddressDto>,
    pub date: Option<String>,
    pub internal_date: Option<String>,
    pub flags: Vec<String>,
    pub size_bytes: Option<u64>,
    pub has_attachments: bool,
    pub snippet: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailMessageDetailResponse {
    pub message: MailMessageSummaryResponse,
    pub body_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailAttachmentResponse {
    pub id: String,
    pub message_id: String,
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub content_id: Option<String>,
    pub disposition: Option<String>,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailPasswordAuthRequest {
    #[serde(default)]
    pub password: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetMailCredentialRequest {
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MailSyncRequest {
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub limit_per_folder: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailFolderSyncResponse {
    pub account_id: String,
    pub folder_id: String,
    pub folder_path: String,
    pub fetched_messages: usize,
    pub stored_messages: usize,
    pub uid_validity: Option<u32>,
    pub uid_next: Option<u32>,
    pub exists: Option<u32>,
    pub unseen: Option<u32>,
    pub highest_synced_uid: Option<u32>,
    pub lowest_synced_uid: Option<u32>,
    pub completed: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailAccountSyncResponse {
    pub account_id: String,
    pub fetched_messages: usize,
    pub stored_messages: usize,
    pub errors: usize,
    pub folders: Vec<MailFolderSyncResponse>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailCredentialStatusResponse {
    pub account_id: String,
    pub credential_configured: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailConnectionTestResponse {
    pub ok: bool,
    pub capabilities: Vec<String>,
}

fn default_true() -> bool {
    true
}
