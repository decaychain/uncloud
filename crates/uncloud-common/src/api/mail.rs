use serde::{Deserialize, Serialize};

use super::files::FileResponse;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailSecurity {
    Tls,
    StartTls,
    Plain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailFolderRole {
    Inbox,
    Sent,
    Drafts,
    Trash,
    Archive,
    Spam,
    AllMail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MailFolderRoleSource {
    #[default]
    Inferred,
    User,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MailComposeMode {
    #[default]
    New,
    Reply,
    ReplyAll,
    Forward,
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
    pub sync_enabled: bool,
    pub sync_interval_secs: Option<u64>,
    pub sync_in_progress: bool,
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
    #[serde(default)]
    pub sync_enabled: bool,
    #[serde(default)]
    pub sync_interval_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UpdateMailAccountRequest {
    pub display_name: Option<String>,
    pub email_address: Option<String>,
    pub imap: Option<MailServerSettings>,
    pub smtp: Option<MailServerSettings>,
    pub sync_enabled: Option<bool>,
    pub sync_interval_secs: Option<Option<u64>>,
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
    pub role: Option<MailFolderRole>,
    pub role_source: MailFolderRoleSource,
    pub selectable: bool,
    pub sync_enabled: bool,
    pub sync_in_progress: bool,
    pub attributes: Vec<String>,
    pub uid_validity: Option<u32>,
    pub uid_next: Option<u32>,
    pub exists: Option<u32>,
    pub unseen: Option<u32>,
    pub highest_synced_uid: Option<u32>,
    pub lowest_synced_uid: Option<u32>,
    pub sync_completed: bool,
    pub last_sync_started_at: Option<String>,
    pub last_sync_finished_at: Option<String>,
    pub last_sync_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UpdateMailFolderRequest {
    pub role: Option<MailFolderRole>,
    #[serde(default)]
    pub infer_role: bool,
    #[serde(default)]
    pub clear_role: bool,
    pub sync_enabled: Option<bool>,
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
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
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
pub struct MailMessageListResponse {
    pub messages: Vec<MailMessageSummaryResponse>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailMessageDetailResponse {
    pub message: MailMessageSummaryResponse,
    pub body_text: Option<String>,
    pub body_html: Option<String>,
    pub attachments: Vec<MailAttachmentResponse>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailMessageMutationAction {
    MarkRead,
    MarkUnread,
    Star,
    Unstar,
    Move,
    Archive,
    Trash,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailMessageMutationRequest {
    pub action: MailMessageMutationAction,
    #[serde(default)]
    pub target_folder_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailMessageMutationResponse {
    pub message: Option<MailMessageSummaryResponse>,
    pub removed_from_folder: bool,
    pub destination_folder_id: Option<String>,
    pub destination_folder_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailMessageBulkMutationRequest {
    pub message_ids: Vec<String>,
    pub action: MailMessageMutationAction,
    #[serde(default)]
    pub target_folder_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailMessageBulkMutationError {
    pub message_id: String,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailMessageBulkMutationResponse {
    pub requested: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub messages: Vec<MailMessageSummaryResponse>,
    pub removed_message_ids: Vec<String>,
    pub destination_folder_id: Option<String>,
    pub destination_folder_path: Option<String>,
    pub errors: Vec<MailMessageBulkMutationError>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailFolderMutationError {
    pub folder_id: String,
    pub folder_path: String,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailFolderMarkReadResponse {
    pub account_id: String,
    pub requested: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub updated_cached_messages: u64,
    pub folders: Vec<MailFolderResponse>,
    pub errors: Vec<MailFolderMutationError>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SendMailMessageRequest {
    #[serde(default)]
    pub identity_id: Option<String>,
    #[serde(default)]
    pub draft_id: Option<String>,
    pub to: Vec<MailAddressDto>,
    #[serde(default)]
    pub cc: Vec<MailAddressDto>,
    #[serde(default)]
    pub bcc: Vec<MailAddressDto>,
    pub subject: String,
    pub body_text: String,
    #[serde(default)]
    pub body_html: Option<String>,
    #[serde(default)]
    pub in_reply_to: Option<String>,
    #[serde(default)]
    pub references: Vec<String>,
    #[serde(default)]
    pub attachment_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailDraftResponse {
    pub id: String,
    pub account_id: String,
    pub identity_id: Option<String>,
    pub mode: MailComposeMode,
    pub source_message_id: Option<String>,
    pub to: Vec<MailAddressDto>,
    pub cc: Vec<MailAddressDto>,
    pub bcc: Vec<MailAddressDto>,
    pub subject: String,
    pub body_text: String,
    pub body_html: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub attachments: Vec<MailDraftAttachmentResponse>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UpsertMailDraftRequest {
    #[serde(default)]
    pub identity_id: Option<String>,
    #[serde(default)]
    pub mode: MailComposeMode,
    #[serde(default)]
    pub source_message_id: Option<String>,
    #[serde(default)]
    pub to: Vec<MailAddressDto>,
    #[serde(default)]
    pub cc: Vec<MailAddressDto>,
    #[serde(default)]
    pub bcc: Vec<MailAddressDto>,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub body_text: String,
    #[serde(default)]
    pub body_html: Option<String>,
    #[serde(default)]
    pub in_reply_to: Option<String>,
    #[serde(default)]
    pub references: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailSentCopyStatus {
    ProviderSaved,
    Appended,
    SkippedNoSentFolder,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SendMailMessageResponse {
    pub account_id: String,
    pub identity_id: Option<String>,
    pub message_id: String,
    pub accepted_recipients: usize,
    pub smtp_response: Option<String>,
    pub sent_copy_status: MailSentCopyStatus,
    pub sent_copy_folder_id: Option<String>,
    pub sent_copy_folder_path: Option<String>,
    pub sent_copy_error: Option<String>,
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
pub struct MailDraftAttachmentResponse {
    pub id: String,
    pub draft_id: String,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: u64,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SaveMailAttachmentRequest {
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub filename: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SaveMailAttachmentResponse {
    pub file: FileResponse,
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
    pub new_messages: usize,
    pub refreshed_messages: usize,
    pub removed_messages: usize,
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
    pub new_messages: usize,
    pub refreshed_messages: usize,
    pub removed_messages: usize,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailProviderDiagnosticsResponse {
    pub account_id: String,
    pub generated_at: String,
    pub credential_configured: bool,
    pub sync_in_progress: bool,
    pub last_sync_at: Option<String>,
    pub imap: MailProviderEndpointDiagnostics,
    pub smtp: MailProviderEndpointDiagnostics,
    pub roles: Vec<MailProviderRoleDiagnostic>,
    pub folders: Vec<MailProviderFolderDiagnostic>,
    pub sent_copy: MailSentCopyDiagnostics,
    pub recent_errors: Vec<MailProviderErrorDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailProviderEndpointDiagnostics {
    pub host: String,
    pub port: u16,
    pub security: MailSecurity,
    pub username: String,
    pub ok: Option<bool>,
    pub capabilities: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailProviderRoleStatus {
    Found,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailProviderRoleDiagnostic {
    pub role: MailFolderRole,
    pub status: MailProviderRoleStatus,
    pub folder_id: Option<String>,
    pub folder_path: Option<String>,
    pub role_source: Option<MailFolderRoleSource>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailProviderFolderDiagnostic {
    pub folder_id: String,
    pub path: String,
    pub name: String,
    pub role: Option<MailFolderRole>,
    pub role_source: MailFolderRoleSource,
    pub selectable: bool,
    pub sync_enabled: bool,
    pub attributes: Vec<String>,
    pub last_sync_finished_at: Option<String>,
    pub last_sync_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailSentCopyDiagnosticStatus {
    Ready,
    MissingSentFolder,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailSentCopyDiagnostics {
    pub status: MailSentCopyDiagnosticStatus,
    pub sent_folder_id: Option<String>,
    pub sent_folder_path: Option<String>,
    pub provider_saved_detection: bool,
    pub append_fallback: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailProviderErrorDiagnostic {
    pub scope: String,
    pub operation: String,
    pub folder_id: Option<String>,
    pub folder_path: Option<String>,
    pub message: String,
    pub at: Option<String>,
}
