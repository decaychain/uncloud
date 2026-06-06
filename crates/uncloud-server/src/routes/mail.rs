use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use axum::{
    Json,
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{StatusCode, header},
    response::Response,
};
use bson::doc;
use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use kuchikiki::traits::TendrilSink;
use mongodb::{
    Database,
    bson::{self, oid::ObjectId},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::time::sleep;
use tokio_util::io::ReaderStream;
use uncloud_common::{
    CreateMailAccountRequest, CreateMailIdentityRequest, MailAccountResponse,
    MailAccountSyncResponse, MailAddressDto, MailAttachmentResponse, MailComposeMode,
    MailConnectionTestResponse, MailCredentialStatusResponse, MailDraftAttachmentResponse,
    MailDraftResponse, MailFolderMarkReadResponse, MailFolderMutationError, MailFolderResponse,
    MailFolderRole, MailFolderRoleSource, MailFolderSyncResponse, MailIdentityResponse,
    MailMessageBulkMutationError, MailMessageBulkMutationRequest, MailMessageBulkMutationResponse,
    MailMessageDetailResponse, MailMessageListResponse, MailMessageMutationAction,
    MailMessageMutationRequest, MailMessageMutationResponse, MailMessageSummaryResponse,
    MailPasswordAuthRequest, MailProviderDiagnosticsResponse, MailProviderEndpointDiagnostics,
    MailProviderErrorDiagnostic, MailProviderFolderDiagnostic, MailProviderRoleDiagnostic,
    MailProviderRoleStatus, MailSentCopyDiagnosticStatus, MailSentCopyDiagnostics,
    MailSentCopyStatus, MailServerSettings, MailSyncRequest, SaveMailAttachmentRequest,
    SaveMailAttachmentResponse, SendMailMessageRequest, SendMailMessageResponse,
    SetMailCredentialRequest, UpdateMailAccountRequest, UpdateMailFolderRequest,
    UpdateMailIdentityRequest, UpsertMailDraftRequest,
};

use crate::AppState;
use crate::error::{AppError, Result};
use crate::middleware::{AuthUser, RequestMeta};
use crate::models::{
    File, Folder, MailAccount, MailAddress, MailAttachment, MailDraft, MailDraftAttachment,
    MailFolder, MailIdentity, MailMessage, MailServerConfig, User,
};
use crate::routes::apps::{EVENT_FILE_CREATED, deliver_webhooks};
use crate::routes::files::{check_name_conflict, file_to_response, resolve_storage_path};
use crate::services::SecretCipher;
use crate::services::mail::{
    RemoteMailAddress, RemoteMailbox, RemoteMessageBody, RemoteMessageFlag, RemoteMessageFlags,
    RemoteMessageSummary, RemoteOutgoingAttachment, RemoteOutgoingMessage, RemoteUidRange,
    decode_mail_header_text,
};
use crate::services::mail_blob::{
    StoredMailBody, read_cached_message_body, store_draft_attachment, store_message_body,
};
use crate::services::sharing::check_folder_access;

const ACCOUNTS: &str = "mail_accounts";
const IDENTITIES: &str = "mail_identities";
const FOLDERS: &str = "mail_folders";
const MESSAGES: &str = "mail_messages";
const ATTACHMENTS: &str = "mail_attachments";
const DRAFTS: &str = "mail_drafts";
const DRAFT_ATTACHMENTS: &str = "mail_draft_attachments";
const DEFAULT_SYNC_LIMIT_PER_FOLDER: u32 = 250;
const MAX_SYNC_LIMIT_PER_FOLDER: u32 = 1_000;
const DEFAULT_MESSAGE_LIST_LIMIT: i64 = 100;
const MAX_MESSAGE_LIST_LIMIT: i64 = 500;
const SENT_COPY_DETECT_ATTEMPTS: usize = 3;
const SENT_COPY_DETECT_DELAY: Duration = Duration::from_millis(750);
const MIN_ACCOUNT_SYNC_INTERVAL_SECS: u64 = 60;
const MAX_ACCOUNT_SYNC_INTERVAL_SECS: u64 = 7 * 24 * 60 * 60;
const BACKGROUND_SYNC_TICK_SECS: u64 = 60;
const MAX_DRAFT_ATTACHMENT_TOTAL_BYTES: u64 = 20 * 1024 * 1024;
const DIAGNOSTIC_FOLDER_ROLES: [MailFolderRole; 7] = [
    MailFolderRole::Inbox,
    MailFolderRole::Sent,
    MailFolderRole::Drafts,
    MailFolderRole::Archive,
    MailFolderRole::Trash,
    MailFolderRole::Spam,
    MailFolderRole::AllMail,
];

#[derive(Debug, Deserialize)]
pub struct MailMessageListQuery {
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    cursor: Option<String>,
}

fn require_mail_available(state: &AppState) -> Result<()> {
    if !state
        .config
        .features
        .is_enabled(crate::config::FEATURE_MAIL)
    {
        return Err(AppError::Forbidden("Access denied".into()));
    }
    Ok(())
}

fn require_mail(state: &AppState, user: &AuthUser) -> Result<()> {
    require_mail_available(state)?;
    if user
        .disabled_features
        .iter()
        .any(|feature| feature == crate::config::FEATURE_MAIL)
    {
        return Err(AppError::Forbidden("Access denied".into()));
    }
    Ok(())
}

fn parse_oid(s: &str, name: &str) -> Result<ObjectId> {
    ObjectId::parse_str(s).map_err(|_| AppError::BadRequest(format!("Invalid {name}")))
}

fn validate_label(input: &str, name: &str) -> Result<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.len() > 256 {
        return Err(AppError::BadRequest(format!(
            "{name} must be between 1 and 256 characters"
        )));
    }
    Ok(trimmed.to_string())
}

fn validate_email(input: &str, name: &str) -> Result<String> {
    let trimmed = input.trim().to_ascii_lowercase();
    if trimmed.len() > 320
        || !trimmed.contains('@')
        || trimmed.starts_with('@')
        || trimmed.ends_with('@')
    {
        return Err(AppError::BadRequest(format!(
            "{name} must be a valid email address"
        )));
    }
    Ok(trimmed)
}

fn validate_server(input: MailServerSettings) -> Result<MailServerConfig> {
    let host = validate_label(&input.host, "mail server host")?;
    let username = validate_label(&input.username, "mail server username")?;
    if input.port == 0 {
        return Err(AppError::BadRequest(
            "mail server port must be non-zero".into(),
        ));
    }
    Ok(MailServerConfig {
        host,
        port: input.port,
        security: input.security,
        username,
    })
}

fn is_duplicate_key_error(err: &mongodb::error::Error) -> bool {
    use mongodb::error::{ErrorKind, WriteFailure};
    match err.kind.as_ref() {
        ErrorKind::Write(WriteFailure::WriteError(we)) => we.code == 11000,
        _ => false,
    }
}

fn server_to_response(input: &MailServerConfig) -> MailServerSettings {
    MailServerSettings {
        host: input.host.clone(),
        port: input.port,
        security: input.security,
        username: input.username.clone(),
    }
}

fn account_to_response(account: &MailAccount, sync_in_progress: bool) -> MailAccountResponse {
    MailAccountResponse {
        id: account.id.to_hex(),
        display_name: account.display_name.clone(),
        email_address: account.email_address.clone(),
        imap: server_to_response(&account.imap),
        smtp: server_to_response(&account.smtp),
        sync_enabled: account.sync_enabled,
        sync_interval_secs: account.sync_interval_secs,
        sync_in_progress,
        credential_configured: account.credential_configured || account.credential.is_some(),
        created_at: account.created_at.to_rfc3339(),
        updated_at: account.updated_at.to_rfc3339(),
        last_sync_at: account.last_sync_at.map(|dt| dt.to_rfc3339()),
    }
}

fn identity_to_response(identity: &MailIdentity) -> MailIdentityResponse {
    MailIdentityResponse {
        id: identity.id.to_hex(),
        account_id: identity.account_id.to_hex(),
        display_name: identity.display_name.clone(),
        email_address: identity.email_address.clone(),
        reply_to: identity.reply_to.clone(),
        signature: identity.signature.clone(),
        is_default: identity.is_default,
        created_at: identity.created_at.to_rfc3339(),
        updated_at: identity.updated_at.to_rfc3339(),
    }
}

fn draft_to_response(
    draft: &MailDraft,
    attachments: Vec<MailDraftAttachment>,
) -> MailDraftResponse {
    MailDraftResponse {
        id: draft.id.to_hex(),
        account_id: draft.account_id.to_hex(),
        identity_id: draft.identity_id.map(|id| id.to_hex()),
        mode: draft.mode,
        source_message_id: draft.source_message_id.map(|id| id.to_hex()),
        to: draft.to.iter().map(address_to_response).collect(),
        cc: draft.cc.iter().map(address_to_response).collect(),
        bcc: draft.bcc.iter().map(address_to_response).collect(),
        subject: draft.subject.clone(),
        body_text: draft.body_text.clone(),
        body_html: draft.body_html.clone(),
        in_reply_to: draft.in_reply_to.clone(),
        references: draft.references.clone(),
        attachments: attachments
            .iter()
            .map(draft_attachment_to_response)
            .collect(),
        created_at: draft.created_at.to_rfc3339(),
        updated_at: draft.updated_at.to_rfc3339(),
    }
}

fn folder_effective_role(folder: &MailFolder) -> Option<MailFolderRole> {
    if folder.role_source == MailFolderRoleSource::User {
        folder.role
    } else {
        infer_folder_role(&folder.path, &folder.name, &folder.attributes)
    }
}

fn folder_sync_completed(folder: &MailFolder) -> bool {
    if folder.exists == Some(0) {
        return true;
    }
    let Some(uid_next) = folder.uid_next else {
        return false;
    };
    if uid_next <= 1 {
        return true;
    }
    let Some(lowest) = folder.lowest_synced_uid else {
        return false;
    };
    let Some(highest) = folder.highest_synced_uid else {
        return false;
    };
    lowest <= 1 && highest.saturating_add(1) >= uid_next
}

fn folder_to_response(folder: &MailFolder, sync_in_progress: bool) -> MailFolderResponse {
    MailFolderResponse {
        id: folder.id.to_hex(),
        account_id: folder.account_id.to_hex(),
        path: folder.path.clone(),
        name: folder.name.clone(),
        delimiter: folder.delimiter.clone(),
        parent_path: folder.parent_path.clone(),
        role: folder_effective_role(folder),
        role_source: folder.role_source,
        selectable: folder.selectable,
        sync_enabled: folder.sync_enabled,
        sync_in_progress,
        attributes: folder.attributes.clone(),
        uid_validity: folder.uid_validity,
        uid_next: folder.uid_next,
        exists: folder.exists,
        unseen: folder.unseen,
        highest_synced_uid: folder.highest_synced_uid,
        lowest_synced_uid: folder.lowest_synced_uid,
        sync_completed: folder_sync_completed(folder),
        last_sync_started_at: folder.last_sync_started_at.map(|dt| dt.to_rfc3339()),
        last_sync_finished_at: folder.last_sync_finished_at.map(|dt| dt.to_rfc3339()),
        last_sync_error: folder.last_sync_error.clone(),
        created_at: folder.created_at.to_rfc3339(),
        updated_at: folder.updated_at.to_rfc3339(),
    }
}

fn address_to_response(address: &MailAddress) -> MailAddressDto {
    MailAddressDto {
        name: address
            .name
            .as_deref()
            .map(decode_mail_header_text)
            .filter(|value| !value.trim().is_empty()),
        address: address.address.clone(),
    }
}

fn attachment_to_response(attachment: &MailAttachment) -> MailAttachmentResponse {
    MailAttachmentResponse {
        id: attachment.id.to_hex(),
        message_id: attachment.message_id.to_hex(),
        filename: attachment.filename.clone(),
        content_type: attachment.content_type.clone(),
        content_id: attachment.content_id.clone(),
        disposition: attachment.disposition.clone(),
        size_bytes: attachment.size_bytes,
    }
}

fn draft_attachment_to_response(attachment: &MailDraftAttachment) -> MailDraftAttachmentResponse {
    MailDraftAttachmentResponse {
        id: attachment.id.to_hex(),
        draft_id: attachment.draft_id.to_hex(),
        filename: attachment.filename.clone(),
        content_type: attachment.content_type.clone(),
        size_bytes: attachment.size_bytes,
        created_at: attachment.created_at.to_rfc3339(),
    }
}

fn message_to_response(message: &MailMessage) -> MailMessageSummaryResponse {
    MailMessageSummaryResponse {
        id: message.id.to_hex(),
        account_id: message.account_id.to_hex(),
        folder_id: message.folder_id.to_hex(),
        folder_path: message.folder_path.clone(),
        uid: message.uid,
        message_id: message.message_id.clone(),
        thread_id: message.thread_id.clone(),
        in_reply_to: message.in_reply_to.clone(),
        references: message.references.clone(),
        subject: message.subject.as_deref().map(decode_mail_header_text),
        from: message.from.iter().map(address_to_response).collect(),
        to: message.to.iter().map(address_to_response).collect(),
        cc: message.cc.iter().map(address_to_response).collect(),
        date: message.date.map(|dt| dt.to_rfc3339()),
        internal_date: message.internal_date.map(|dt| dt.to_rfc3339()),
        flags: message.flags.clone(),
        size_bytes: message.size_bytes,
        has_attachments: message.has_attachments,
        snippet: message.snippet.clone(),
    }
}

async fn find_account(state: &AppState, owner_id: ObjectId, id: ObjectId) -> Result<MailAccount> {
    state
        .db
        .collection::<MailAccount>(ACCOUNTS)
        .find_one(doc! { "_id": id, "owner_id": owner_id })
        .await?
        .ok_or_else(|| AppError::NotFound("Mail account".into()))
}

async fn find_folder(
    state: &AppState,
    owner_id: ObjectId,
    account_id: ObjectId,
    id: ObjectId,
) -> Result<MailFolder> {
    state
        .db
        .collection::<MailFolder>(FOLDERS)
        .find_one(doc! { "_id": id, "owner_id": owner_id, "account_id": account_id })
        .await?
        .ok_or_else(|| AppError::NotFound("Mail folder".into()))
}

async fn find_message(state: &AppState, owner_id: ObjectId, id: ObjectId) -> Result<MailMessage> {
    state
        .db
        .collection::<MailMessage>(MESSAGES)
        .find_one(doc! { "_id": id, "owner_id": owner_id })
        .await?
        .ok_or_else(|| AppError::NotFound("Mail message".into()))
}

async fn find_draft(
    state: &AppState,
    owner_id: ObjectId,
    account_id: ObjectId,
    id: ObjectId,
) -> Result<MailDraft> {
    state
        .db
        .collection::<MailDraft>(DRAFTS)
        .find_one(doc! { "_id": id, "owner_id": owner_id, "account_id": account_id })
        .await?
        .ok_or_else(|| AppError::NotFound("Mail draft".into()))
}

async fn find_draft_by_id(state: &AppState, owner_id: ObjectId, id: ObjectId) -> Result<MailDraft> {
    state
        .db
        .collection::<MailDraft>(DRAFTS)
        .find_one(doc! { "_id": id, "owner_id": owner_id })
        .await?
        .ok_or_else(|| AppError::NotFound("Mail draft".into()))
}

async fn list_draft_attachments(
    state: &AppState,
    owner_id: ObjectId,
    draft_id: ObjectId,
) -> Result<Vec<MailDraftAttachment>> {
    let mut cursor = state
        .db
        .collection::<MailDraftAttachment>(DRAFT_ATTACHMENTS)
        .find(doc! { "owner_id": owner_id, "draft_id": draft_id })
        .sort(doc! { "created_at": 1 })
        .await?;
    let mut out = Vec::new();
    while let Some(attachment) = cursor.try_next().await? {
        out.push(attachment);
    }
    Ok(out)
}

async fn list_message_attachments(
    state: &AppState,
    owner_id: ObjectId,
    message_id: ObjectId,
) -> Result<Vec<MailAttachment>> {
    let mut cursor = state
        .db
        .collection::<MailAttachment>(ATTACHMENTS)
        .find(doc! { "owner_id": owner_id, "message_id": message_id })
        .sort(doc! { "created_at": 1 })
        .await?;
    let mut out = Vec::new();
    while let Some(attachment) = cursor.try_next().await? {
        out.push(attachment);
    }
    Ok(out)
}

fn mail_body_html_for_response(
    body_html: Option<String>,
    attachments: &[MailAttachment],
) -> Option<String> {
    body_html
        .map(|html| rewrite_cid_mail_images(&html, attachments))
        .filter(|html| !html.trim().is_empty())
}

fn rewrite_cid_mail_images(html: &str, attachments: &[MailAttachment]) -> String {
    let cid_urls = attachments
        .iter()
        .filter_map(|attachment| {
            let key = normalize_mail_content_id(attachment.content_id.as_deref()?)?;
            Some((
                key,
                format!("/api/mail/attachments/{}/open", attachment.id.to_hex()),
            ))
        })
        .collect::<HashMap<_, _>>();
    if cid_urls.is_empty() {
        return html.to_string();
    }

    let document = kuchikiki::parse_html().one(html).document_node;
    let Ok(body) = document.select_first("body") else {
        return html.to_string();
    };
    let Ok(images) = body.as_node().select("img") else {
        return html.to_string();
    };

    let mut changed = false;
    for image in images {
        let src = image.attributes.borrow().get("src").map(str::to_string);
        let Some(src) = src else {
            continue;
        };
        let Some(content_id) = normalize_cid_url(&src) else {
            continue;
        };

        let mut attributes = image.attributes.borrow_mut();
        if let Some(url) = cid_urls.get(&content_id) {
            attributes.insert("src", url.clone());
        } else {
            attributes.remove("src");
        }
        changed = true;
    }

    if changed {
        serialize_html_children(body.as_node())
    } else {
        html.to_string()
    }
}

fn normalize_cid_url(value: &str) -> Option<String> {
    let value = value.trim();
    let rest = value.get(..4).and_then(|prefix| {
        if prefix.eq_ignore_ascii_case("cid:") {
            Some(&value[4..])
        } else {
            None
        }
    })?;
    normalize_mail_content_id(rest)
}

fn normalize_mail_content_id(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let decoded = urlencoding::decode(trimmed)
        .map(|value| value.into_owned())
        .unwrap_or_else(|_| trimmed.to_string());
    let content_id = decoded
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim();
    if content_id.is_empty() {
        None
    } else {
        Some(content_id.to_ascii_lowercase())
    }
}

fn serialize_html_children(node: &kuchikiki::NodeRef) -> String {
    node.children()
        .map(|child| child.to_string())
        .collect::<Vec<_>>()
        .join("")
}

async fn find_identity(
    state: &AppState,
    owner_id: ObjectId,
    account_id: ObjectId,
    id: ObjectId,
) -> Result<MailIdentity> {
    state
        .db
        .collection::<MailIdentity>(IDENTITIES)
        .find_one(doc! { "_id": id, "owner_id": owner_id, "account_id": account_id })
        .await?
        .ok_or_else(|| AppError::NotFound("Mail identity".into()))
}

async fn find_folder_by_role(
    state: &AppState,
    owner_id: ObjectId,
    account_id: ObjectId,
    roles: &[MailFolderRole],
) -> Result<MailFolder> {
    if let Some(folder) = find_optional_folder_by_role(state, owner_id, account_id, roles).await? {
        return Ok(folder);
    }

    Err(AppError::BadRequest(format!(
        "No selectable mail folder is configured as {}",
        role_list_label(roles)
    )))
}

async fn find_optional_folder_by_role(
    state: &AppState,
    owner_id: ObjectId,
    account_id: ObjectId,
    roles: &[MailFolderRole],
) -> Result<Option<MailFolder>> {
    let mut cursor = state
        .db
        .collection::<MailFolder>(FOLDERS)
        .find(doc! { "owner_id": owner_id, "account_id": account_id, "selectable": true })
        .await?;
    while let Some(folder) = cursor.try_next().await? {
        if let Some(role) = folder_effective_role(&folder) {
            if roles.contains(&role) {
                return Ok(Some(folder));
            }
        }
    }

    Ok(None)
}

async fn load_account_folders(
    state: &AppState,
    owner_id: ObjectId,
    account_id: ObjectId,
) -> Result<Vec<MailFolder>> {
    let mut cursor = state
        .db
        .collection::<MailFolder>(FOLDERS)
        .find(doc! { "owner_id": owner_id, "account_id": account_id })
        .sort(doc! { "path": 1 })
        .await?;
    let mut out = Vec::new();
    while let Some(folder) = cursor.try_next().await? {
        out.push(folder);
    }
    Ok(out)
}

fn role_list_label(roles: &[MailFolderRole]) -> String {
    roles
        .iter()
        .map(|role| match role {
            MailFolderRole::Inbox => "Inbox",
            MailFolderRole::Sent => "Sent",
            MailFolderRole::Drafts => "Drafts",
            MailFolderRole::Trash => "Trash",
            MailFolderRole::Archive => "Archive",
            MailFolderRole::Spam => "Spam",
            MailFolderRole::AllMail => "All Mail",
        })
        .collect::<Vec<_>>()
        .join(" or ")
}

fn credential_status(account: &MailAccount) -> MailCredentialStatusResponse {
    MailCredentialStatusResponse {
        account_id: account.id.to_hex(),
        credential_configured: account.credential_configured || account.credential.is_some(),
    }
}

pub async fn migrate_mail_account_enabled_flag(db: &Database) -> Result<()> {
    let accounts = db.collection::<bson::Document>(ACCOUNTS);
    let disabled = accounts
        .update_many(
            doc! { "enabled": false },
            doc! {
                "$set": { "sync_enabled": false },
                "$unset": { "enabled": "" },
            },
        )
        .await?;
    let remaining = accounts
        .update_many(
            doc! { "enabled": { "$exists": true } },
            doc! { "$unset": { "enabled": "" } },
        )
        .await?;
    let modified = disabled.modified_count + remaining.modified_count;
    if modified > 0 {
        tracing::info!(
            modified,
            "migrated obsolete mail account enabled flag into sync_enabled"
        );
    }
    Ok(())
}

fn provider_endpoint_diagnostics(settings: &MailServerConfig) -> MailProviderEndpointDiagnostics {
    MailProviderEndpointDiagnostics {
        host: settings.host.clone(),
        port: settings.port,
        security: settings.security,
        username: settings.username.clone(),
        ok: None,
        capabilities: Vec::new(),
        error: None,
    }
}

fn folder_to_provider_diagnostics(folder: &MailFolder) -> MailProviderFolderDiagnostic {
    MailProviderFolderDiagnostic {
        folder_id: folder.id.to_hex(),
        path: folder.path.clone(),
        name: folder.name.clone(),
        role: folder_effective_role(folder),
        role_source: folder.role_source,
        selectable: folder.selectable,
        sync_enabled: folder.sync_enabled,
        attributes: folder.attributes.clone(),
        last_sync_finished_at: folder.last_sync_finished_at.map(|dt| dt.to_rfc3339()),
        last_sync_error: folder.last_sync_error.clone(),
    }
}

fn first_selectable_folder_for_role(
    folders: &[MailFolder],
    role: MailFolderRole,
) -> Option<&MailFolder> {
    folders
        .iter()
        .find(|folder| folder.selectable && folder_effective_role(folder) == Some(role))
}

fn role_diagnostics(folders: &[MailFolder]) -> Vec<MailProviderRoleDiagnostic> {
    DIAGNOSTIC_FOLDER_ROLES
        .iter()
        .map(|role| {
            let folder = first_selectable_folder_for_role(folders, *role);
            MailProviderRoleDiagnostic {
                role: *role,
                status: if folder.is_some() {
                    MailProviderRoleStatus::Found
                } else {
                    MailProviderRoleStatus::Missing
                },
                folder_id: folder.map(|folder| folder.id.to_hex()),
                folder_path: folder.map(|folder| folder.path.clone()),
                role_source: folder.map(|folder| folder.role_source),
            }
        })
        .collect()
}

fn sent_copy_diagnostics(folders: &[MailFolder]) -> MailSentCopyDiagnostics {
    if let Some(folder) = first_selectable_folder_for_role(folders, MailFolderRole::Sent) {
        MailSentCopyDiagnostics {
            status: MailSentCopyDiagnosticStatus::Ready,
            sent_folder_id: Some(folder.id.to_hex()),
            sent_folder_path: Some(folder.path.clone()),
            provider_saved_detection: true,
            append_fallback: true,
            detail: "Sent folder is configured; provider-saved detection and append fallback are enabled.".to_string(),
        }
    } else {
        MailSentCopyDiagnostics {
            status: MailSentCopyDiagnosticStatus::MissingSentFolder,
            sent_folder_id: None,
            sent_folder_path: None,
            provider_saved_detection: false,
            append_fallback: false,
            detail: "No selectable folder is configured as Sent; sent copies will be skipped."
                .to_string(),
        }
    }
}

fn provider_error_diagnostic(
    scope: &str,
    operation: &str,
    message: String,
    at: Option<String>,
) -> MailProviderErrorDiagnostic {
    MailProviderErrorDiagnostic {
        scope: scope.to_string(),
        operation: operation.to_string(),
        folder_id: None,
        folder_path: None,
        message,
        at,
    }
}

fn folder_error_diagnostic(folder: &MailFolder) -> Option<MailProviderErrorDiagnostic> {
    folder
        .last_sync_error
        .as_ref()
        .map(|message| MailProviderErrorDiagnostic {
            scope: "folder".to_string(),
            operation: "sync".to_string(),
            folder_id: Some(folder.id.to_hex()),
            folder_path: Some(folder.path.clone()),
            message: message.clone(),
            at: folder.last_sync_finished_at.map(|dt| dt.to_rfc3339()),
        })
}

fn sync_limit(input: Option<u32>) -> u32 {
    input
        .unwrap_or(DEFAULT_SYNC_LIMIT_PER_FOLDER)
        .clamp(1, MAX_SYNC_LIMIT_PER_FOLDER)
}

fn validate_account_sync_interval(input: Option<u64>) -> Result<Option<u64>> {
    match input {
        Some(value) => {
            if value < MIN_ACCOUNT_SYNC_INTERVAL_SECS || value > MAX_ACCOUNT_SYNC_INTERVAL_SECS {
                return Err(AppError::BadRequest(format!(
                    "mail account sync interval must be between {} and {} seconds",
                    MIN_ACCOUNT_SYNC_INTERVAL_SECS, MAX_ACCOUNT_SYNC_INTERVAL_SECS
                )));
            }
            Ok(Some(value))
        }
        None => Ok(None),
    }
}

fn effective_account_sync_interval_secs(account: &MailAccount, default_interval: u64) -> u64 {
    account
        .sync_interval_secs
        .unwrap_or(default_interval)
        .clamp(
            MIN_ACCOUNT_SYNC_INTERVAL_SECS,
            MAX_ACCOUNT_SYNC_INTERVAL_SECS,
        )
}

fn account_due_for_scheduled_sync(
    account: &MailAccount,
    now: DateTime<Utc>,
    default_interval: u64,
) -> bool {
    let interval = effective_account_sync_interval_secs(account, default_interval);
    let last_attempt = account.last_sync_attempt_at.or(account.last_sync_at);
    let Some(last_attempt) = last_attempt else {
        return true;
    };
    now.signed_duration_since(last_attempt)
        >= chrono::Duration::seconds(interval.min(i64::MAX as u64) as i64)
}

fn message_list_limit(input: Option<i64>) -> i64 {
    input
        .unwrap_or(DEFAULT_MESSAGE_LIST_LIMIT)
        .clamp(1, MAX_MESSAGE_LIST_LIMIT)
}

fn parse_message_list_cursor(input: Option<&str>) -> Result<Option<u32>> {
    let Some(input) = input else {
        return Ok(None);
    };
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let uid = trimmed
        .parse::<u32>()
        .map_err(|_| AppError::BadRequest("Invalid message list cursor".into()))?;
    Ok(Some(uid))
}

fn remote_address_to_model(address: &RemoteMailAddress) -> MailAddress {
    MailAddress {
        name: address.name.clone(),
        address: address.address.clone(),
    }
}

fn response_address_to_remote(address: &MailAddressDto) -> RemoteMailAddress {
    RemoteMailAddress {
        name: address
            .name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        address: address.address.trim().to_string(),
    }
}

fn response_address_to_model(address: &MailAddressDto) -> MailAddress {
    MailAddress {
        name: address
            .name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        address: address.address.trim().to_string(),
    }
}

fn clean_optional_message_header_value(
    value: Option<String>,
    name: &str,
) -> Result<Option<String>> {
    let Some(value) = value.map(|value| value.trim().to_string()) else {
        return Ok(None);
    };
    if value.is_empty() {
        return Ok(None);
    }
    if value.contains('\r') || value.contains('\n') {
        return Err(AppError::BadRequest(format!(
            "{name} cannot contain line breaks"
        )));
    }
    Ok(Some(value))
}

fn clean_message_header_values(values: Vec<String>, name: &str) -> Result<Vec<String>> {
    values
        .into_iter()
        .filter_map(
            |value| match clean_optional_message_header_value(Some(value), name) {
                Ok(Some(value)) => Some(Ok(value)),
                Ok(None) => None,
                Err(err) => Some(Err(err)),
            },
        )
        .collect()
}

fn sanitize_optional_mail_html(value: Option<String>) -> Option<String> {
    let value = value?.trim().to_string();
    if value.is_empty() {
        return None;
    }

    let mut builder = ammonia::Builder::default();
    builder.url_relative(ammonia::UrlRelative::Deny);
    builder.attribute_filter(|element, attribute, value| match (element, attribute) {
        ("img", "src") | ("img", "srcset") | ("source", "src") | ("source", "srcset") => None,
        _ => Some(value.into()),
    });
    let cleaned = builder.clean(&value).to_string();
    if cleaned.trim().is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

fn clean_attachment_filename(value: Option<&str>) -> Result<String> {
    let filename = value
        .and_then(|value| {
            value
                .rsplit(['/', '\\'])
                .next()
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .unwrap_or("attachment.bin");
    if filename.len() > 255
        || filename.contains('\0')
        || filename.contains('\r')
        || filename.contains('\n')
    {
        return Err(AppError::BadRequest(
            "attachment filename must be 255 characters or fewer and cannot contain control characters".into(),
        ));
    }
    Ok(filename.to_string())
}

fn clean_attachment_content_type(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 127
                && !value.contains('\r')
                && !value.contains('\n')
        })
        .unwrap_or("application/octet-stream")
        .to_string()
}

fn recipient_count(req: &SendMailMessageRequest) -> usize {
    req.to.len() + req.cc.len() + req.bcc.len()
}

fn validate_send_request(req: &SendMailMessageRequest) -> Result<()> {
    if recipient_count(req) == 0 {
        return Err(AppError::BadRequest(
            "at least one recipient is required".into(),
        ));
    }
    for address in req.to.iter().chain(req.cc.iter()).chain(req.bcc.iter()) {
        if address.address.trim().is_empty() {
            return Err(AppError::BadRequest(
                "recipient address cannot be empty".into(),
            ));
        }
    }
    if req.subject.len() > 998 {
        return Err(AppError::BadRequest(
            "mail subject must be 998 characters or fewer".into(),
        ));
    }
    clean_optional_message_header_value(req.in_reply_to.clone(), "In-Reply-To")?;
    clean_message_header_values(req.references.clone(), "References")?;
    Ok(())
}

fn message_id_for_sender(email_address: &str) -> String {
    let domain = email_address
        .rsplit_once('@')
        .map(|(_, domain)| domain.trim())
        .filter(|domain| !domain.is_empty())
        .unwrap_or("uncloud.local");
    format!("<{}@{}>", ObjectId::new().to_hex(), domain)
}

fn mail_flag_eq(candidate: &str, flag: &str) -> bool {
    candidate
        .trim()
        .trim_start_matches('\\')
        .eq_ignore_ascii_case(flag.trim_start_matches('\\'))
}

fn message_has_flag(message: &MailMessage, flag: &str) -> bool {
    message.flags.iter().any(|value| mail_flag_eq(value, flag))
}

fn optional_u32_bson(value: Option<u32>) -> bson::Bson {
    value
        .map(|value| bson::Bson::Int64(value as i64))
        .unwrap_or(bson::Bson::Null)
}

fn optional_u64_bson(value: Option<u64>) -> bson::Bson {
    value
        .map(|value| bson::Bson::Int64(value as i64))
        .unwrap_or(bson::Bson::Null)
}

fn optional_string_bson(value: Option<String>) -> bson::Bson {
    value.map(bson::Bson::String).unwrap_or(bson::Bson::Null)
}

fn optional_role_bson(value: Option<MailFolderRole>) -> Result<bson::Bson> {
    bson::to_bson(&value).map_err(|e| AppError::Internal(e.to_string()))
}

fn role_source_bson(value: MailFolderRoleSource) -> Result<bson::Bson> {
    bson::to_bson(&value).map_err(|e| AppError::Internal(e.to_string()))
}

fn infer_folder_role(path: &str, name: &str, attributes: &[String]) -> Option<MailFolderRole> {
    for attr in attributes.iter().map(|value| value.to_ascii_lowercase()) {
        if attr.contains("inbox") {
            return Some(MailFolderRole::Inbox);
        }
        if attr.contains("sent") {
            return Some(MailFolderRole::Sent);
        }
        if attr.contains("draft") {
            return Some(MailFolderRole::Drafts);
        }
        if attr.contains("trash") || attr.contains("deleted") || attr.contains("bin") {
            return Some(MailFolderRole::Trash);
        }
        if attr.contains("archive") {
            return Some(MailFolderRole::Archive);
        }
        if attr.contains("junk") || attr.contains("spam") {
            return Some(MailFolderRole::Spam);
        }
        if attr.contains("all") {
            return Some(MailFolderRole::AllMail);
        }
    }

    let path_lower = path.trim().to_ascii_lowercase();
    if path_lower == "inbox" {
        return Some(MailFolderRole::Inbox);
    }

    match normalized_folder_name(name).as_str() {
        "inbox" => Some(MailFolderRole::Inbox),
        "sent" | "sent mail" | "sent items" | "sent messages" | "gesendet"
        | "gesendete elemente" => Some(MailFolderRole::Sent),
        "draft" | "drafts" | "entwurfe" | "entwuerfe" => Some(MailFolderRole::Drafts),
        "trash" | "bin" | "deleted" | "deleted items" | "deleted messages" | "papierkorb" => {
            Some(MailFolderRole::Trash)
        }
        "archive" | "archives" | "archiv" => Some(MailFolderRole::Archive),
        "spam" | "junk" | "junk mail" | "junk email" | "junk e mail" | "bulk mail" => {
            Some(MailFolderRole::Spam)
        }
        "all" | "all mail" | "all messages" | "alle nachrichten" => Some(MailFolderRole::AllMail),
        _ => None,
    }
}

fn normalized_folder_name(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn resolve_mail_password(
    state: &AppState,
    account: &MailAccount,
    password: Option<&str>,
) -> Result<String> {
    if let Some(password) = password.map(str::trim).filter(|value| !value.is_empty()) {
        return Ok(password.to_string());
    }
    let Some(credential) = account.credential.as_ref() else {
        return Err(AppError::BadRequest(
            "mail account has no stored credential; provide a password or set the credential"
                .into(),
        ));
    };
    SecretCipher::from_config(&state.config.secrets)?.decrypt_mail_credential(credential)
}

pub async fn list_accounts(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<MailAccountResponse>>> {
    require_mail(&state, &user)?;
    let mut cursor = state
        .db
        .collection::<MailAccount>(ACCOUNTS)
        .find(doc! { "owner_id": user.id })
        .sort(doc! { "email_address": 1 })
        .await?;
    let mut out = Vec::new();
    while let Some(account) = cursor.try_next().await? {
        out.push(account_to_response(
            &account,
            state.mail.is_account_syncing(account.id),
        ));
    }
    Ok(Json(out))
}

pub async fn create_account(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(req): Json<CreateMailAccountRequest>,
) -> Result<(StatusCode, Json<MailAccountResponse>)> {
    require_mail(&state, &user)?;
    let now = Utc::now();
    let display_name = validate_label(&req.display_name, "display name")?;
    let email_address = validate_email(&req.email_address, "email address")?;
    let account = MailAccount {
        id: ObjectId::new(),
        owner_id: user.id,
        display_name: display_name.clone(),
        email_address: email_address.clone(),
        imap: validate_server(req.imap)?,
        smtp: validate_server(req.smtp)?,
        sync_enabled: req.sync_enabled,
        sync_interval_secs: validate_account_sync_interval(req.sync_interval_secs)?,
        credential_configured: false,
        credential: None,
        mail_storage_id: None,
        last_sync_attempt_at: None,
        last_sync_at: None,
        created_at: now,
        updated_at: now,
    };
    state
        .db
        .collection::<MailAccount>(ACCOUNTS)
        .insert_one(&account)
        .await
        .map_err(|e| {
            if is_duplicate_key_error(&e) {
                AppError::BadRequest("Another mail account already uses this address".into())
            } else {
                AppError::from(e)
            }
        })?;
    let identity = MailIdentity {
        id: ObjectId::new(),
        owner_id: user.id,
        account_id: account.id,
        display_name,
        email_address,
        reply_to: None,
        signature: None,
        is_default: true,
        created_at: now,
        updated_at: now,
    };
    state
        .db
        .collection::<MailIdentity>(IDENTITIES)
        .insert_one(&identity)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(account_to_response(
            &account,
            state.mail.is_account_syncing(account.id),
        )),
    ))
}

pub async fn update_account(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<UpdateMailAccountRequest>,
) -> Result<Json<MailAccountResponse>> {
    require_mail(&state, &user)?;
    let id = parse_oid(&id, "mail account id")?;
    let _ = find_account(&state, user.id, id).await?;

    let mut set = doc! { "updated_at": bson::DateTime::from_chrono(Utc::now()) };
    let mut unset = doc! {};
    if let Some(value) = req.display_name {
        set.insert("display_name", validate_label(&value, "display name")?);
    }
    if let Some(value) = req.email_address {
        set.insert("email_address", validate_email(&value, "email address")?);
    }
    if let Some(value) = req.imap {
        let server = validate_server(value)?;
        set.insert(
            "imap",
            bson::to_bson(&server).map_err(|e| AppError::Internal(e.to_string()))?,
        );
    }
    if let Some(value) = req.smtp {
        let server = validate_server(value)?;
        set.insert(
            "smtp",
            bson::to_bson(&server).map_err(|e| AppError::Internal(e.to_string()))?,
        );
    }
    if let Some(value) = req.sync_enabled {
        set.insert("sync_enabled", value);
    }
    if let Some(value) = req.sync_interval_secs {
        match validate_account_sync_interval(value)? {
            Some(interval) => {
                set.insert("sync_interval_secs", bson::Bson::Int64(interval as i64));
            }
            None => {
                unset.insert("sync_interval_secs", "");
            }
        }
    }

    let update = if unset.is_empty() {
        doc! { "$set": set }
    } else {
        doc! { "$set": set, "$unset": unset }
    };
    state
        .db
        .collection::<MailAccount>(ACCOUNTS)
        .update_one(doc! { "_id": id, "owner_id": user.id }, update)
        .await?;

    let account = find_account(&state, user.id, id).await?;
    Ok(Json(account_to_response(
        &account,
        state.mail.is_account_syncing(account.id),
    )))
}

pub async fn delete_account(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    require_mail(&state, &user)?;
    let id = parse_oid(&id, "mail account id")?;
    let result = state
        .db
        .collection::<MailAccount>(ACCOUNTS)
        .delete_one(doc! { "_id": id, "owner_id": user.id })
        .await?;
    if result.deleted_count == 0 {
        return Err(AppError::NotFound("Mail account".into()));
    }
    state
        .db
        .collection::<MailIdentity>(IDENTITIES)
        .delete_many(doc! { "account_id": id, "owner_id": user.id })
        .await?;
    state
        .db
        .collection::<MailFolder>(FOLDERS)
        .delete_many(doc! { "account_id": id, "owner_id": user.id })
        .await?;
    state
        .db
        .collection::<mongodb::bson::Document>(MESSAGES)
        .delete_many(doc! { "account_id": id, "owner_id": user.id })
        .await?;
    state
        .db
        .collection::<mongodb::bson::Document>(ATTACHMENTS)
        .delete_many(doc! { "account_id": id, "owner_id": user.id })
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_account_credential(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<MailCredentialStatusResponse>> {
    require_mail(&state, &user)?;
    let id = parse_oid(&id, "mail account id")?;
    let account = find_account(&state, user.id, id).await?;
    Ok(Json(credential_status(&account)))
}

pub async fn set_account_credential(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<SetMailCredentialRequest>,
) -> Result<Json<MailCredentialStatusResponse>> {
    require_mail(&state, &user)?;
    let id = parse_oid(&id, "mail account id")?;
    let _ = find_account(&state, user.id, id).await?;
    let password = req.password.trim();
    if password.is_empty() {
        return Err(AppError::BadRequest(
            "mail credential cannot be empty".into(),
        ));
    }
    let credential =
        SecretCipher::from_config(&state.config.secrets)?.encrypt_mail_credential(password)?;
    let credential_bson =
        bson::to_bson(&credential).map_err(|e| AppError::Internal(e.to_string()))?;
    state
        .db
        .collection::<MailAccount>(ACCOUNTS)
        .update_one(
            doc! { "_id": id, "owner_id": user.id },
            doc! {
                "$set": {
                    "credential": credential_bson,
                    "credential_configured": true,
                    "updated_at": bson::DateTime::from_chrono(Utc::now()),
                }
            },
        )
        .await?;
    let account = find_account(&state, user.id, id).await?;
    Ok(Json(credential_status(&account)))
}

pub async fn clear_account_credential(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<MailCredentialStatusResponse>> {
    require_mail(&state, &user)?;
    let id = parse_oid(&id, "mail account id")?;
    let _ = find_account(&state, user.id, id).await?;
    state
        .db
        .collection::<MailAccount>(ACCOUNTS)
        .update_one(
            doc! { "_id": id, "owner_id": user.id },
            doc! {
                "$set": {
                    "credential_configured": false,
                    "updated_at": bson::DateTime::from_chrono(Utc::now()),
                },
                "$unset": { "credential": "" },
            },
        )
        .await?;
    let account = find_account(&state, user.id, id).await?;
    Ok(Json(credential_status(&account)))
}

pub async fn test_account_imap(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<MailPasswordAuthRequest>,
) -> Result<Json<MailConnectionTestResponse>> {
    require_mail(&state, &user)?;
    let id = parse_oid(&id, "mail account id")?;
    let account = find_account(&state, user.id, id).await?;
    let password = resolve_mail_password(&state, &account, req.password.as_deref())?;
    Ok(Json(
        state
            .mail
            .test_imap_password(&account.imap, &password)
            .await?,
    ))
}

pub async fn test_account_smtp(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<MailPasswordAuthRequest>,
) -> Result<Json<MailConnectionTestResponse>> {
    require_mail(&state, &user)?;
    let id = parse_oid(&id, "mail account id")?;
    let account = find_account(&state, user.id, id).await?;
    let password = resolve_mail_password(&state, &account, req.password.as_deref())?;
    Ok(Json(
        state
            .mail
            .test_smtp_password(&account.smtp, &password)
            .await?,
    ))
}

pub async fn diagnose_account_provider(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<MailPasswordAuthRequest>,
) -> Result<Json<MailProviderDiagnosticsResponse>> {
    require_mail(&state, &user)?;
    let id = parse_oid(&id, "mail account id")?;
    let account = find_account(&state, user.id, id).await?;
    let folders = load_account_folders(&state, user.id, account.id).await?;
    let generated_at = Utc::now().to_rfc3339();
    let mut imap = provider_endpoint_diagnostics(&account.imap);
    let mut smtp = provider_endpoint_diagnostics(&account.smtp);
    let mut recent_errors = folders
        .iter()
        .filter_map(folder_error_diagnostic)
        .collect::<Vec<_>>();

    match resolve_mail_password(&state, &account, req.password.as_deref()) {
        Ok(password) => {
            match state
                .mail
                .test_imap_password(&account.imap, &password)
                .await
            {
                Ok(result) => {
                    imap.ok = Some(result.ok);
                    imap.capabilities = result.capabilities;
                }
                Err(err) => {
                    let message = err.to_string();
                    imap.ok = Some(false);
                    imap.error = Some(message.clone());
                    recent_errors.push(provider_error_diagnostic(
                        "imap",
                        "connection_test",
                        message,
                        Some(generated_at.clone()),
                    ));
                }
            }

            match state
                .mail
                .test_smtp_password(&account.smtp, &password)
                .await
            {
                Ok(result) => {
                    smtp.ok = Some(result.ok);
                    smtp.capabilities = result.capabilities;
                }
                Err(err) => {
                    let message = err.to_string();
                    smtp.ok = Some(false);
                    smtp.error = Some(message.clone());
                    recent_errors.push(provider_error_diagnostic(
                        "smtp",
                        "connection_test",
                        message,
                        Some(generated_at.clone()),
                    ));
                }
            }
        }
        Err(err) => {
            let message = err.to_string();
            imap.error = Some(message.clone());
            smtp.error = Some(message);
        }
    }

    recent_errors.sort_by(|a, b| {
        b.at.cmp(&a.at)
            .then_with(|| a.scope.cmp(&b.scope))
            .then_with(|| a.operation.cmp(&b.operation))
    });
    recent_errors.truncate(10);

    Ok(Json(MailProviderDiagnosticsResponse {
        account_id: account.id.to_hex(),
        generated_at,
        credential_configured: account.credential_configured || account.credential.is_some(),
        sync_in_progress: state.mail.is_account_syncing(account.id),
        last_sync_at: account.last_sync_at.map(|dt| dt.to_rfc3339()),
        imap,
        smtp,
        roles: role_diagnostics(&folders),
        folders: folders.iter().map(folder_to_provider_diagnostics).collect(),
        sent_copy: sent_copy_diagnostics(&folders),
        recent_errors,
    }))
}

struct ResolvedSendIdentity {
    id: Option<ObjectId>,
    display_name: String,
    email_address: String,
    reply_to: Option<String>,
}

struct SentCopyResult {
    status: MailSentCopyStatus,
    folder_id: Option<String>,
    folder_path: Option<String>,
    error: Option<String>,
}

struct NormalizedDraftPayload {
    identity_id: Option<ObjectId>,
    mode: MailComposeMode,
    source_message_id: Option<ObjectId>,
    to: Vec<MailAddress>,
    cc: Vec<MailAddress>,
    bcc: Vec<MailAddress>,
    subject: String,
    body_text: String,
    body_html: Option<String>,
    in_reply_to: Option<String>,
    references: Vec<String>,
}

async fn resolve_send_identity(
    state: &AppState,
    owner_id: ObjectId,
    account: &MailAccount,
    identity_id: Option<&str>,
) -> Result<ResolvedSendIdentity> {
    if let Some(identity_id) = identity_id {
        let id = parse_oid(identity_id, "mail identity id")?;
        let identity = find_identity(state, owner_id, account.id, id).await?;
        return Ok(ResolvedSendIdentity {
            id: Some(identity.id),
            display_name: identity.display_name,
            email_address: identity.email_address,
            reply_to: identity.reply_to,
        });
    }

    let identity = state
        .db
        .collection::<MailIdentity>(IDENTITIES)
        .find_one(doc! {
            "owner_id": owner_id,
            "account_id": account.id,
            "is_default": true,
        })
        .await?;
    let identity = match identity {
        Some(identity) => Some(identity),
        None => {
            state
                .db
                .collection::<MailIdentity>(IDENTITIES)
                .find_one(doc! { "owner_id": owner_id, "account_id": account.id })
                .sort(doc! { "email_address": 1 })
                .await?
        }
    };

    if let Some(identity) = identity {
        Ok(ResolvedSendIdentity {
            id: Some(identity.id),
            display_name: identity.display_name,
            email_address: identity.email_address,
            reply_to: identity.reply_to,
        })
    } else {
        Ok(ResolvedSendIdentity {
            id: None,
            display_name: account.display_name.clone(),
            email_address: account.email_address.clone(),
            reply_to: None,
        })
    }
}

async fn handle_sent_copy(
    state: &AppState,
    owner_id: ObjectId,
    account: &MailAccount,
    password: &str,
    message_id: &str,
    raw_message: &[u8],
) -> SentCopyResult {
    let sent_folder =
        match find_optional_folder_by_role(state, owner_id, account.id, &[MailFolderRole::Sent])
            .await
        {
            Ok(Some(folder)) => folder,
            Ok(None) => {
                return SentCopyResult {
                    status: MailSentCopyStatus::SkippedNoSentFolder,
                    folder_id: None,
                    folder_path: None,
                    error: None,
                };
            }
            Err(err) => {
                return SentCopyResult {
                    status: MailSentCopyStatus::Failed,
                    folder_id: None,
                    folder_path: None,
                    error: Some(err.to_string()),
                };
            }
        };

    let folder_id = Some(sent_folder.id.to_hex());
    let folder_path = Some(sent_folder.path.clone());
    match wait_for_provider_sent_copy(state, account, password, &sent_folder, message_id).await {
        Ok(true) => SentCopyResult {
            status: MailSentCopyStatus::ProviderSaved,
            folder_id,
            folder_path,
            error: None,
        },
        Ok(false) => match state
            .mail
            .append_imap_message(&account.imap, password, &sent_folder.path, raw_message)
            .await
        {
            Ok(()) => SentCopyResult {
                status: MailSentCopyStatus::Appended,
                folder_id,
                folder_path,
                error: None,
            },
            Err(err) => SentCopyResult {
                status: MailSentCopyStatus::Failed,
                folder_id,
                folder_path,
                error: Some(err.to_string()),
            },
        },
        Err(err) => SentCopyResult {
            status: MailSentCopyStatus::Failed,
            folder_id,
            folder_path,
            error: Some(err.to_string()),
        },
    }
}

async fn wait_for_provider_sent_copy(
    state: &AppState,
    account: &MailAccount,
    password: &str,
    sent_folder: &MailFolder,
    message_id: &str,
) -> Result<bool> {
    for attempt in 0..SENT_COPY_DETECT_ATTEMPTS {
        if state
            .mail
            .imap_message_exists_by_message_id(
                &account.imap,
                password,
                &sent_folder.path,
                message_id,
            )
            .await?
        {
            return Ok(true);
        }
        if attempt + 1 < SENT_COPY_DETECT_ATTEMPTS {
            sleep(SENT_COPY_DETECT_DELAY).await;
        }
    }
    Ok(false)
}

async fn normalize_draft_payload(
    state: &AppState,
    owner_id: ObjectId,
    account_id: ObjectId,
    req: UpsertMailDraftRequest,
) -> Result<NormalizedDraftPayload> {
    if req.subject.len() > 998 {
        return Err(AppError::BadRequest(
            "mail subject must be 998 characters or fewer".into(),
        ));
    }

    let identity_id = match req
        .identity_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) => {
            let id = parse_oid(value, "mail identity id")?;
            let _ = find_identity(state, owner_id, account_id, id).await?;
            Some(id)
        }
        None => None,
    };

    let source_message_id = match req
        .source_message_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) => {
            let id = parse_oid(value, "mail source message id")?;
            let message = find_message(state, owner_id, id).await?;
            if message.account_id != account_id {
                return Err(AppError::BadRequest(
                    "mail source message belongs to a different account".into(),
                ));
            }
            Some(id)
        }
        None => None,
    };

    Ok(NormalizedDraftPayload {
        identity_id,
        mode: req.mode,
        source_message_id,
        to: req.to.iter().map(response_address_to_model).collect(),
        cc: req.cc.iter().map(response_address_to_model).collect(),
        bcc: req.bcc.iter().map(response_address_to_model).collect(),
        subject: req.subject,
        body_text: req.body_text,
        body_html: sanitize_optional_mail_html(req.body_html),
        in_reply_to: clean_optional_message_header_value(req.in_reply_to, "In-Reply-To")?,
        references: clean_message_header_values(req.references, "References")?,
    })
}

async fn delete_sent_draft(
    state: &AppState,
    owner_id: ObjectId,
    account_id: ObjectId,
    draft_id: Option<ObjectId>,
) {
    let Some(draft_id) = draft_id else {
        return;
    };
    if let Err(err) = delete_draft_attachments(state, owner_id, draft_id).await {
        tracing::warn!(%draft_id, "failed to delete sent mail draft attachments: {err}");
    }
    if let Err(err) = state
        .db
        .collection::<MailDraft>(DRAFTS)
        .delete_one(doc! { "_id": draft_id, "owner_id": owner_id, "account_id": account_id })
        .await
    {
        tracing::warn!(%draft_id, "failed to delete sent mail draft: {err}");
    }
}

async fn delete_draft_attachments(
    state: &AppState,
    owner_id: ObjectId,
    draft_id: ObjectId,
) -> Result<()> {
    let attachments = list_draft_attachments(state, owner_id, draft_id).await?;
    for attachment in &attachments {
        match state.storage.get_backend(attachment.storage_id).await {
            Ok(backend) => {
                if let Err(err) = backend.delete(&attachment.storage_path).await {
                    tracing::warn!(
                        "failed to delete mail draft attachment blob {} at {}: {err}",
                        attachment.id,
                        attachment.storage_path
                    );
                }
            }
            Err(err) => {
                tracing::warn!(
                    "failed to open storage for mail draft attachment blob {}: {err}",
                    attachment.id
                );
            }
        }
    }
    state
        .db
        .collection::<MailDraftAttachment>(DRAFT_ATTACHMENTS)
        .delete_many(doc! { "owner_id": owner_id, "draft_id": draft_id })
        .await?;
    Ok(())
}

async fn load_outgoing_attachments(
    state: &AppState,
    owner_id: ObjectId,
    account_id: ObjectId,
    draft_id: Option<ObjectId>,
    attachment_ids: &[String],
) -> Result<Vec<RemoteOutgoingAttachment>> {
    if draft_id.is_none() && !attachment_ids.is_empty() {
        return Err(AppError::BadRequest(
            "mail attachments require a saved draft".into(),
        ));
    }
    let Some(draft_id) = draft_id else {
        return Ok(Vec::new());
    };

    let attachments = if attachment_ids.is_empty() {
        list_draft_attachments(state, owner_id, draft_id).await?
    } else {
        let ids = attachment_ids
            .iter()
            .map(|id| parse_oid(id, "mail draft attachment id"))
            .collect::<Result<Vec<_>>>()?;
        let mut cursor = state
            .db
            .collection::<MailDraftAttachment>(DRAFT_ATTACHMENTS)
            .find(doc! {
                "_id": { "$in": ids },
                "owner_id": owner_id,
                "account_id": account_id,
                "draft_id": draft_id,
            })
            .sort(doc! { "created_at": 1 })
            .await?;
        let mut out = Vec::new();
        while let Some(attachment) = cursor.try_next().await? {
            out.push(attachment);
        }
        if out.len() != attachment_ids.len() {
            return Err(AppError::BadRequest(
                "one or more mail draft attachments were not found".into(),
            ));
        }
        out
    };

    let total_size: u64 = attachments
        .iter()
        .map(|attachment| attachment.size_bytes)
        .sum();
    if total_size > MAX_DRAFT_ATTACHMENT_TOTAL_BYTES {
        return Err(AppError::BadRequest(format!(
            "mail attachments are limited to {} MiB total",
            MAX_DRAFT_ATTACHMENT_TOTAL_BYTES / 1024 / 1024
        )));
    }

    let mut out = Vec::new();
    for attachment in attachments {
        let backend = state.storage.get_backend(attachment.storage_id).await?;
        let data = backend.read_all(&attachment.storage_path).await?;
        out.push(RemoteOutgoingAttachment {
            filename: attachment.filename,
            content_type: attachment.content_type,
            data,
        });
    }
    Ok(out)
}

pub async fn send_account_message(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<SendMailMessageRequest>,
) -> Result<Json<SendMailMessageResponse>> {
    require_mail(&state, &user)?;
    validate_send_request(&req)?;
    let id = parse_oid(&id, "mail account id")?;
    let account = find_account(&state, user.id, id).await?;
    let draft_id = match req
        .draft_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) => {
            let id = parse_oid(value, "mail draft id")?;
            let _ = find_draft(&state, user.id, account.id, id).await?;
            Some(id)
        }
        None => None,
    };
    let password = resolve_mail_password(&state, &account, None)?;
    let identity =
        resolve_send_identity(&state, user.id, &account, req.identity_id.as_deref()).await?;
    let outgoing_attachments =
        load_outgoing_attachments(&state, user.id, account.id, draft_id, &req.attachment_ids)
            .await?;
    let message_id = message_id_for_sender(&identity.email_address);
    let accepted_recipients = recipient_count(&req);
    let remote = RemoteOutgoingMessage {
        message_id: message_id.clone(),
        from: RemoteMailAddress {
            name: Some(identity.display_name.clone()),
            address: identity.email_address.clone(),
        },
        reply_to: identity.reply_to.as_ref().map(|address| RemoteMailAddress {
            name: None,
            address: address.clone(),
        }),
        to: req.to.iter().map(response_address_to_remote).collect(),
        cc: req.cc.iter().map(response_address_to_remote).collect(),
        bcc: req.bcc.iter().map(response_address_to_remote).collect(),
        subject: req.subject,
        body_text: req.body_text,
        body_html: sanitize_optional_mail_html(req.body_html),
        in_reply_to: clean_optional_message_header_value(req.in_reply_to, "In-Reply-To")?,
        references: clean_message_header_values(req.references, "References")?,
        attachments: outgoing_attachments,
    };
    let sent = state
        .mail
        .send_smtp_message(&account.smtp, &password, remote)
        .await?;
    let sent_copy = handle_sent_copy(
        &state,
        user.id,
        &account,
        &password,
        &sent.message_id,
        &sent.raw_message,
    )
    .await;
    delete_sent_draft(&state, user.id, account.id, draft_id).await;

    Ok(Json(SendMailMessageResponse {
        account_id: account.id.to_hex(),
        identity_id: identity.id.map(|id| id.to_hex()),
        message_id: sent.message_id,
        accepted_recipients,
        smtp_response: sent.response,
        sent_copy_status: sent_copy.status,
        sent_copy_folder_id: sent_copy.folder_id,
        sent_copy_folder_path: sent_copy.folder_path,
        sent_copy_error: sent_copy.error,
    }))
}

pub async fn list_identities(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<MailIdentityResponse>>> {
    require_mail(&state, &user)?;
    let mut cursor = state
        .db
        .collection::<MailIdentity>(IDENTITIES)
        .find(doc! { "owner_id": user.id })
        .sort(doc! { "account_id": 1, "email_address": 1 })
        .await?;
    let mut out = Vec::new();
    while let Some(identity) = cursor.try_next().await? {
        out.push(identity_to_response(&identity));
    }
    Ok(Json(out))
}

pub async fn create_identity(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(req): Json<CreateMailIdentityRequest>,
) -> Result<(StatusCode, Json<MailIdentityResponse>)> {
    require_mail(&state, &user)?;
    let account_id = parse_oid(&req.account_id, "mail account id")?;
    let _ = find_account(&state, user.id, account_id).await?;
    let existing_count = state
        .db
        .collection::<MailIdentity>(IDENTITIES)
        .count_documents(doc! { "owner_id": user.id, "account_id": account_id })
        .await?;
    let make_default = req.is_default || existing_count == 0;
    if make_default {
        state
            .db
            .collection::<MailIdentity>(IDENTITIES)
            .update_many(
                doc! { "owner_id": user.id, "account_id": account_id },
                doc! { "$set": { "is_default": false } },
            )
            .await?;
    }
    let now = Utc::now();
    let identity = MailIdentity {
        id: ObjectId::new(),
        owner_id: user.id,
        account_id,
        display_name: validate_label(&req.display_name, "display name")?,
        email_address: validate_email(&req.email_address, "email address")?,
        reply_to: req
            .reply_to
            .map(|v| validate_email(&v, "reply-to"))
            .transpose()?,
        signature: req.signature,
        is_default: make_default,
        created_at: now,
        updated_at: now,
    };
    state
        .db
        .collection::<MailIdentity>(IDENTITIES)
        .insert_one(&identity)
        .await
        .map_err(|e| {
            if is_duplicate_key_error(&e) {
                AppError::BadRequest(
                    "Another identity on this account already uses this address".into(),
                )
            } else {
                AppError::from(e)
            }
        })?;
    Ok((StatusCode::CREATED, Json(identity_to_response(&identity))))
}

pub async fn update_identity(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<UpdateMailIdentityRequest>,
) -> Result<Json<MailIdentityResponse>> {
    require_mail(&state, &user)?;
    let id = parse_oid(&id, "mail identity id")?;
    let existing = state
        .db
        .collection::<MailIdentity>(IDENTITIES)
        .find_one(doc! { "_id": id, "owner_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Mail identity".into()))?;
    if req.is_default == Some(true) {
        state
            .db
            .collection::<MailIdentity>(IDENTITIES)
            .update_many(
                doc! { "owner_id": user.id, "account_id": existing.account_id },
                doc! { "$set": { "is_default": false } },
            )
            .await?;
    }

    let mut set = doc! { "updated_at": bson::DateTime::from_chrono(Utc::now()) };
    if let Some(value) = req.display_name {
        set.insert("display_name", validate_label(&value, "display name")?);
    }
    if let Some(value) = req.email_address {
        set.insert("email_address", validate_email(&value, "email address")?);
    }
    if let Some(value) = req.is_default {
        set.insert("is_default", value);
    }
    let mut unset = doc! {};
    if let Some(value) = req.reply_to {
        match value {
            Some(email) => {
                set.insert("reply_to", validate_email(&email, "reply-to")?);
            }
            None => {
                unset.insert("reply_to", "");
            }
        }
    }
    if let Some(value) = req.signature {
        match value {
            Some(signature) => {
                set.insert("signature", signature);
            }
            None => {
                unset.insert("signature", "");
            }
        }
    }
    let update = if unset.is_empty() {
        doc! { "$set": set }
    } else {
        doc! { "$set": set, "$unset": unset }
    };
    state
        .db
        .collection::<MailIdentity>(IDENTITIES)
        .update_one(doc! { "_id": id, "owner_id": user.id }, update)
        .await
        .map_err(|e| {
            if is_duplicate_key_error(&e) {
                AppError::BadRequest(
                    "Another identity on this account already uses this address".into(),
                )
            } else {
                AppError::from(e)
            }
        })?;
    let identity = state
        .db
        .collection::<MailIdentity>(IDENTITIES)
        .find_one(doc! { "_id": id, "owner_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Mail identity".into()))?;
    Ok(Json(identity_to_response(&identity)))
}

pub async fn delete_identity(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    require_mail(&state, &user)?;
    let id = parse_oid(&id, "mail identity id")?;
    let identity = state
        .db
        .collection::<MailIdentity>(IDENTITIES)
        .find_one(doc! { "_id": id, "owner_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Mail identity".into()))?;
    let result = state
        .db
        .collection::<MailIdentity>(IDENTITIES)
        .delete_one(doc! { "_id": id, "owner_id": user.id })
        .await?;
    if result.deleted_count == 0 {
        return Err(AppError::NotFound("Mail identity".into()));
    }
    state
        .db
        .collection::<MailDraft>(DRAFTS)
        .update_many(
            doc! { "owner_id": user.id, "account_id": identity.account_id, "identity_id": id },
            doc! { "$unset": { "identity_id": "" } },
        )
        .await?;
    if identity.is_default {
        if let Some(next) = state
            .db
            .collection::<MailIdentity>(IDENTITIES)
            .find_one(doc! {
                "owner_id": user.id,
                "account_id": identity.account_id,
            })
            .sort(doc! { "email_address": 1 })
            .await?
        {
            state
                .db
                .collection::<MailIdentity>(IDENTITIES)
                .update_one(
                    doc! { "_id": next.id, "owner_id": user.id },
                    doc! {
                        "$set": {
                            "is_default": true,
                            "updated_at": bson::DateTime::from_chrono(Utc::now()),
                        }
                    },
                )
                .await?;
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_drafts(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(account_id): Path<String>,
) -> Result<Json<Vec<MailDraftResponse>>> {
    require_mail(&state, &user)?;
    let account_id = parse_oid(&account_id, "mail account id")?;
    let _ = find_account(&state, user.id, account_id).await?;
    let mut cursor = state
        .db
        .collection::<MailDraft>(DRAFTS)
        .find(doc! { "owner_id": user.id, "account_id": account_id })
        .sort(doc! { "updated_at": -1 })
        .await?;
    let mut out = Vec::new();
    while let Some(draft) = cursor.try_next().await? {
        let attachments = list_draft_attachments(&state, user.id, draft.id).await?;
        out.push(draft_to_response(&draft, attachments));
    }
    Ok(Json(out))
}

pub async fn create_draft(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(account_id): Path<String>,
    Json(req): Json<UpsertMailDraftRequest>,
) -> Result<(StatusCode, Json<MailDraftResponse>)> {
    require_mail(&state, &user)?;
    let account_id = parse_oid(&account_id, "mail account id")?;
    let _ = find_account(&state, user.id, account_id).await?;
    let payload = normalize_draft_payload(&state, user.id, account_id, req).await?;
    let now = Utc::now();
    let draft = MailDraft {
        id: ObjectId::new(),
        owner_id: user.id,
        account_id,
        identity_id: payload.identity_id,
        mode: payload.mode,
        source_message_id: payload.source_message_id,
        to: payload.to,
        cc: payload.cc,
        bcc: payload.bcc,
        subject: payload.subject,
        body_text: payload.body_text,
        body_html: payload.body_html,
        in_reply_to: payload.in_reply_to,
        references: payload.references,
        created_at: now,
        updated_at: now,
    };
    state
        .db
        .collection::<MailDraft>(DRAFTS)
        .insert_one(&draft)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(draft_to_response(&draft, Vec::new())),
    ))
}

pub async fn update_draft(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<UpsertMailDraftRequest>,
) -> Result<Json<MailDraftResponse>> {
    require_mail(&state, &user)?;
    let id = parse_oid(&id, "mail draft id")?;
    let draft = state
        .db
        .collection::<MailDraft>(DRAFTS)
        .find_one(doc! { "_id": id, "owner_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Mail draft".into()))?;
    let payload = normalize_draft_payload(&state, user.id, draft.account_id, req).await?;
    let now = Utc::now();
    let mode = bson::to_bson(&payload.mode)
        .map_err(|e| AppError::Internal(format!("mail draft mode could not be serialized: {e}")))?;
    let to = bson::to_bson(&payload.to).map_err(|e| {
        AppError::Internal(format!(
            "mail draft recipients could not be serialized: {e}"
        ))
    })?;
    let cc = bson::to_bson(&payload.cc).map_err(|e| {
        AppError::Internal(format!(
            "mail draft cc recipients could not be serialized: {e}"
        ))
    })?;
    let bcc = bson::to_bson(&payload.bcc).map_err(|e| {
        AppError::Internal(format!(
            "mail draft bcc recipients could not be serialized: {e}"
        ))
    })?;
    state
        .db
        .collection::<MailDraft>(DRAFTS)
        .update_one(
            doc! { "_id": id, "owner_id": user.id },
            doc! {
                "$set": {
                    "identity_id": payload.identity_id.map(bson::Bson::ObjectId).unwrap_or(bson::Bson::Null),
                    "mode": mode,
                    "source_message_id": payload.source_message_id.map(bson::Bson::ObjectId).unwrap_or(bson::Bson::Null),
                    "to": to,
                    "cc": cc,
                    "bcc": bcc,
                    "subject": payload.subject,
                    "body_text": payload.body_text,
                    "body_html": payload.body_html.map(bson::Bson::String).unwrap_or(bson::Bson::Null),
                    "in_reply_to": payload.in_reply_to.map(bson::Bson::String).unwrap_or(bson::Bson::Null),
                    "references": payload.references,
                    "updated_at": bson::DateTime::from_chrono(now),
                }
            },
        )
        .await?;
    let updated = state
        .db
        .collection::<MailDraft>(DRAFTS)
        .find_one(doc! { "_id": id, "owner_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Mail draft".into()))?;
    let attachments = list_draft_attachments(&state, user.id, updated.id).await?;
    Ok(Json(draft_to_response(&updated, attachments)))
}

pub async fn delete_draft(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    require_mail(&state, &user)?;
    let id = parse_oid(&id, "mail draft id")?;
    let _ = find_draft_by_id(&state, user.id, id).await?;
    delete_draft_attachments(&state, user.id, id).await?;
    let result = state
        .db
        .collection::<MailDraft>(DRAFTS)
        .delete_one(doc! { "_id": id, "owner_id": user.id })
        .await?;
    if result.deleted_count == 0 {
        return Err(AppError::NotFound("Mail draft".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn upload_draft_attachment(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<MailDraftAttachmentResponse>)> {
    require_mail(&state, &user)?;
    let draft_id = parse_oid(&id, "mail draft id")?;
    let draft = find_draft_by_id(&state, user.id, draft_id).await?;
    let account = find_account(&state, user.id, draft.account_id).await?;

    let mut upload: Option<(String, String, Vec<u8>)> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let filename = clean_attachment_filename(field.file_name())?;
        let content_type = clean_attachment_content_type(field.content_type());
        let data = field
            .bytes()
            .await
            .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
            .to_vec();
        upload = Some((filename, content_type, data));
        break;
    }

    let Some((filename, content_type, data)) = upload else {
        return Err(AppError::BadRequest("attachment file is required".into()));
    };
    let size = data.len() as u64;
    let existing_total: u64 = list_draft_attachments(&state, user.id, draft.id)
        .await?
        .iter()
        .map(|attachment| attachment.size_bytes)
        .sum();
    if existing_total.saturating_add(size) > MAX_DRAFT_ATTACHMENT_TOTAL_BYTES {
        return Err(AppError::BadRequest(format!(
            "mail attachments are limited to {} MiB total",
            MAX_DRAFT_ATTACHMENT_TOTAL_BYTES / 1024 / 1024
        )));
    }

    let now = Utc::now();
    let attachment_id = ObjectId::new();
    let stored = store_draft_attachment(
        &state.storage,
        &user.username,
        &account,
        draft.id,
        attachment_id,
        &filename,
        &data,
    )
    .await?;
    let attachment = MailDraftAttachment {
        id: attachment_id,
        owner_id: user.id,
        account_id: draft.account_id,
        draft_id: draft.id,
        filename,
        content_type,
        size_bytes: stored.size_bytes,
        storage_id: stored.storage_id,
        storage_path: stored.storage_path,
        created_at: now,
    };

    if let Err(err) = state
        .db
        .collection::<MailDraftAttachment>(DRAFT_ATTACHMENTS)
        .insert_one(&attachment)
        .await
    {
        if let Ok(backend) = state.storage.get_backend(attachment.storage_id).await {
            let _ = backend.delete(&attachment.storage_path).await;
        }
        return Err(err.into());
    }
    state
        .db
        .collection::<MailDraft>(DRAFTS)
        .update_one(
            doc! { "_id": draft.id, "owner_id": user.id },
            doc! { "$set": { "updated_at": bson::DateTime::from_chrono(now) } },
        )
        .await?;

    Ok((
        StatusCode::CREATED,
        Json(draft_attachment_to_response(&attachment)),
    ))
}

pub async fn delete_draft_attachment(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path((draft_id, attachment_id)): Path<(String, String)>,
) -> Result<StatusCode> {
    require_mail(&state, &user)?;
    let draft_id = parse_oid(&draft_id, "mail draft id")?;
    let attachment_id = parse_oid(&attachment_id, "mail draft attachment id")?;
    let draft = find_draft_by_id(&state, user.id, draft_id).await?;
    let attachment = state
        .db
        .collection::<MailDraftAttachment>(DRAFT_ATTACHMENTS)
        .find_one(doc! {
            "_id": attachment_id,
            "owner_id": user.id,
            "account_id": draft.account_id,
            "draft_id": draft.id,
        })
        .await?
        .ok_or_else(|| AppError::NotFound("Mail draft attachment".into()))?;

    match state.storage.get_backend(attachment.storage_id).await {
        Ok(backend) => {
            if let Err(err) = backend.delete(&attachment.storage_path).await {
                tracing::warn!(
                    "failed to delete mail draft attachment blob {} at {}: {err}",
                    attachment.id,
                    attachment.storage_path
                );
            }
        }
        Err(err) => {
            tracing::warn!(
                "failed to open storage for mail draft attachment blob {}: {err}",
                attachment.id
            );
        }
    }
    state
        .db
        .collection::<MailDraftAttachment>(DRAFT_ATTACHMENTS)
        .delete_one(doc! { "_id": attachment.id, "owner_id": user.id })
        .await?;
    state
        .db
        .collection::<MailDraft>(DRAFTS)
        .update_one(
            doc! { "_id": draft.id, "owner_id": user.id },
            doc! { "$set": { "updated_at": bson::DateTime::from_chrono(Utc::now()) } },
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_folders(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(account_id): Path<String>,
) -> Result<Json<Vec<MailFolderResponse>>> {
    require_mail(&state, &user)?;
    let account_id = parse_oid(&account_id, "mail account id")?;
    let _ = find_account(&state, user.id, account_id).await?;
    let sync_in_progress = state.mail.is_account_syncing(account_id);
    Ok(Json(
        load_account_folders(&state, user.id, account_id)
            .await?
            .iter()
            .map(|folder| folder_to_response(folder, sync_in_progress))
            .collect(),
    ))
}

pub async fn update_folder(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path((account_id, folder_id)): Path<(String, String)>,
    Json(req): Json<UpdateMailFolderRequest>,
) -> Result<Json<MailFolderResponse>> {
    require_mail(&state, &user)?;
    let account_id = parse_oid(&account_id, "mail account id")?;
    let folder_id = parse_oid(&folder_id, "mail folder id")?;
    let _ = find_account(&state, user.id, account_id).await?;
    let folder = find_folder(&state, user.id, account_id, folder_id).await?;

    let mut set = doc! {};
    if req.infer_role {
        let role = infer_folder_role(&folder.path, &folder.name, &folder.attributes);
        set.insert("role", optional_role_bson(role)?);
        set.insert(
            "role_source",
            role_source_bson(MailFolderRoleSource::Inferred)?,
        );
    } else if req.clear_role {
        set.insert("role", bson::Bson::Null);
        set.insert("role_source", role_source_bson(MailFolderRoleSource::User)?);
    } else if let Some(role) = req.role {
        set.insert("role", optional_role_bson(Some(role))?);
        set.insert("role_source", role_source_bson(MailFolderRoleSource::User)?);
    }
    if let Some(sync_enabled) = req.sync_enabled {
        set.insert("sync_enabled", sync_enabled);
    }

    if set.is_empty() {
        return Ok(Json(folder_to_response(
            &folder,
            state.mail.is_account_syncing(folder.account_id),
        )));
    }

    let now = Utc::now();
    set.insert("updated_at", bson::DateTime::from_chrono(now));
    state
        .db
        .collection::<MailFolder>(FOLDERS)
        .update_one(
            doc! { "_id": folder.id, "owner_id": user.id, "account_id": account_id },
            doc! { "$set": set },
        )
        .await?;

    let updated = find_folder(&state, user.id, account_id, folder.id).await?;
    Ok(Json(folder_to_response(
        &updated,
        state.mail.is_account_syncing(updated.account_id),
    )))
}

async fn upsert_remote_folders(
    state: &AppState,
    owner_id: ObjectId,
    account_id: ObjectId,
    remote: Vec<RemoteMailbox>,
) -> Result<Vec<MailFolder>> {
    let coll = state.db.collection::<MailFolder>(FOLDERS);
    let now = Utc::now();
    let mut remote_paths = Vec::new();
    let mut existing_by_path = HashMap::<String, MailFolder>::new();
    let mut existing_cursor = coll
        .find(doc! { "owner_id": owner_id, "account_id": account_id })
        .await?;
    while let Some(folder) = existing_cursor.try_next().await? {
        existing_by_path.insert(folder.path.clone(), folder);
    }

    for folder in remote {
        remote_paths.push(folder.path.clone());
        let (role, role_source) = match existing_by_path.get(&folder.path) {
            Some(existing) if existing.role_source == MailFolderRoleSource::User => {
                (existing.role, MailFolderRoleSource::User)
            }
            _ => (
                infer_folder_role(&folder.path, &folder.name, &folder.attributes),
                MailFolderRoleSource::Inferred,
            ),
        };
        coll.update_one(
            doc! { "owner_id": owner_id, "account_id": account_id, "path": &folder.path },
            doc! {
                "$set": {
                    "name": folder.name,
                    "delimiter": folder.delimiter.map(bson::Bson::String).unwrap_or(bson::Bson::Null),
                    "parent_path": folder.parent_path.map(bson::Bson::String).unwrap_or(bson::Bson::Null),
                    "role": optional_role_bson(role)?,
                    "role_source": role_source_bson(role_source)?,
                    "selectable": folder.selectable,
                    "attributes": folder.attributes,
                    "updated_at": bson::DateTime::from_chrono(now),
                },
                "$setOnInsert": {
                    "_id": ObjectId::new(),
                    "owner_id": owner_id,
                    "account_id": account_id,
                    "path": folder.path,
                    "sync_enabled": true,
                    "created_at": bson::DateTime::from_chrono(now),
                },
            },
        )
        .with_options(
            mongodb::options::UpdateOptions::builder()
                .upsert(true)
                .build(),
        )
        .await?;
    }

    let stale_filter = if remote_paths.is_empty() {
        doc! { "owner_id": owner_id, "account_id": account_id }
    } else {
        doc! {
            "owner_id": owner_id,
            "account_id": account_id,
            "path": { "$nin": remote_paths },
        }
    };
    let mut stale_cursor = coll.find(stale_filter.clone()).await?;
    let mut stale_ids = Vec::new();
    while let Some(folder) = stale_cursor.try_next().await? {
        stale_ids.push(folder.id);
    }
    if !stale_ids.is_empty() {
        state
            .db
            .collection::<MailMessage>(MESSAGES)
            .delete_many(doc! {
                "owner_id": owner_id,
                "account_id": account_id,
                "folder_id": { "$in": stale_ids.clone() },
            })
            .await?;
        coll.delete_many(doc! {
            "owner_id": owner_id,
            "account_id": account_id,
            "_id": { "$in": stale_ids },
        })
        .await?;
    }

    let mut cursor = coll
        .find(doc! { "owner_id": owner_id, "account_id": account_id })
        .sort(doc! { "path": 1 })
        .await?;
    let mut folders = Vec::new();
    while let Some(folder) = cursor.try_next().await? {
        folders.push(folder);
    }
    Ok(folders)
}

pub async fn refresh_folders(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(account_id): Path<String>,
    Json(req): Json<MailPasswordAuthRequest>,
) -> Result<Json<Vec<MailFolderResponse>>> {
    require_mail(&state, &user)?;
    let account_id = parse_oid(&account_id, "mail account id")?;
    let account = find_account(&state, user.id, account_id).await?;
    let password = resolve_mail_password(&state, &account, req.password.as_deref())?;
    let remote = state
        .mail
        .list_imap_mailboxes(&account.imap, &password)
        .await?;

    let folders = upsert_remote_folders(&state, user.id, account_id, remote).await?;
    let sync_in_progress = state.mail.is_account_syncing(account_id);
    Ok(Json(
        folders
            .iter()
            .map(|folder| folder_to_response(folder, sync_in_progress))
            .collect(),
    ))
}

async fn store_message_summary(
    state: &AppState,
    owner_id: ObjectId,
    account_id: ObjectId,
    folder: &MailFolder,
    summary: &RemoteMessageSummary,
    now: chrono::DateTime<Utc>,
) -> Result<StoreMessageSummaryResult> {
    let message = MailMessage {
        id: ObjectId::new(),
        owner_id,
        account_id,
        folder_id: folder.id,
        folder_path: folder.path.clone(),
        uid: summary.uid,
        message_id: summary.message_id.clone(),
        thread_id: None,
        in_reply_to: summary.in_reply_to.clone(),
        references: Vec::new(),
        subject: summary.subject.clone(),
        from: summary.from.iter().map(remote_address_to_model).collect(),
        to: summary.to.iter().map(remote_address_to_model).collect(),
        cc: summary.cc.iter().map(remote_address_to_model).collect(),
        bcc: summary.bcc.iter().map(remote_address_to_model).collect(),
        date: summary.date,
        internal_date: summary.internal_date,
        flags: summary.flags.clone(),
        size_bytes: summary.size_bytes,
        has_attachments: summary.has_attachments,
        snippet: None,
        mail_storage_id: None,
        raw_storage_path: None,
        raw_storage_size_bytes: None,
        text_storage_path: None,
        text_storage_size_bytes: None,
        html_storage_path: None,
        html_storage_size_bytes: None,
        created_at: now,
        updated_at: now,
    };
    let message_bson = bson::to_bson(&message).map_err(|e| AppError::Internal(e.to_string()))?;
    let mut set = message_bson
        .as_document()
        .cloned()
        .ok_or_else(|| AppError::Internal("failed to serialize mail message".into()))?;
    set.remove("_id");
    set.remove("created_at");
    let result = state
        .db
        .collection::<MailMessage>(MESSAGES)
        .update_one(
            doc! {
                "owner_id": owner_id,
                "account_id": account_id,
                "folder_id": folder.id,
                "uid": summary.uid,
            },
            doc! {
                "$set": set,
                "$setOnInsert": {
                    "_id": message.id,
                    "created_at": bson::DateTime::from_chrono(now),
                },
            },
        )
        .with_options(
            mongodb::options::UpdateOptions::builder()
                .upsert(true)
                .build(),
        )
        .await?;
    if result.upserted_id.is_some() {
        Ok(StoreMessageSummaryResult::Inserted)
    } else {
        Ok(StoreMessageSummaryResult::Refreshed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StoreMessageSummaryResult {
    Inserted,
    Refreshed,
}

async fn delete_cached_messages_matching(
    state: &AppState,
    mut filter: bson::Document,
) -> Result<usize> {
    let coll = state.db.collection::<MailMessage>(MESSAGES);
    let mut cursor = coll.find(filter.clone()).await?;
    let mut message_ids = Vec::new();
    while let Some(message) = cursor.try_next().await? {
        message_ids.push(message.id);
    }
    if message_ids.is_empty() {
        return Ok(0);
    }

    state
        .db
        .collection::<MailAttachment>(ATTACHMENTS)
        .delete_many(doc! { "message_id": { "$in": message_ids.clone() } })
        .await?;

    filter.insert("_id", doc! { "$in": message_ids });
    let result = coll.delete_many(filter).await?;
    Ok(result.deleted_count as usize)
}

async fn delete_cached_messages_missing_from_uid_range(
    state: &AppState,
    owner_id: ObjectId,
    account_id: ObjectId,
    folder_id: ObjectId,
    range: RemoteUidRange,
    remote_messages: &[RemoteMessageSummary],
) -> Result<usize> {
    let remote_uids = remote_messages
        .iter()
        .map(|message| message.uid)
        .collect::<HashSet<_>>();
    let mut uid_filter = doc! {
        "$gte": range.start as i64,
        "$lte": range.end as i64,
    };
    if !remote_uids.is_empty() {
        uid_filter.insert(
            "$nin",
            remote_uids
                .iter()
                .map(|uid| bson::Bson::Int64(*uid as i64))
                .collect::<Vec<_>>(),
        );
    }

    delete_cached_messages_matching(
        state,
        doc! {
            "owner_id": owner_id,
            "account_id": account_id,
            "folder_id": folder_id,
            "uid": uid_filter,
        },
    )
    .await
}

async fn record_folder_sync_error(
    state: &AppState,
    owner_id: ObjectId,
    account_id: ObjectId,
    folder_id: ObjectId,
    error: &str,
) -> Result<()> {
    state
        .db
        .collection::<MailFolder>(FOLDERS)
        .update_one(
            doc! { "_id": folder_id, "owner_id": owner_id, "account_id": account_id },
            doc! {
                "$set": {
                    "last_sync_finished_at": bson::DateTime::from_chrono(Utc::now()),
                    "last_sync_error": error,
                    "updated_at": bson::DateTime::from_chrono(Utc::now()),
                }
            },
        )
        .await?;
    Ok(())
}

async fn sync_one_folder(
    state: &AppState,
    owner_id: ObjectId,
    account: &MailAccount,
    folder: &MailFolder,
    password: &str,
    limit: u32,
) -> Result<MailFolderSyncResponse> {
    if !folder.selectable {
        return Err(AppError::BadRequest("mail folder is not selectable".into()));
    }

    let started_at = Utc::now();
    let coll = state.db.collection::<MailFolder>(FOLDERS);
    coll.update_one(
        doc! { "_id": folder.id, "owner_id": owner_id, "account_id": account.id },
        doc! {
            "$set": {
                "last_sync_started_at": bson::DateTime::from_chrono(started_at),
                "updated_at": bson::DateTime::from_chrono(started_at),
            },
            "$unset": { "last_sync_error": "" },
        },
    )
    .await?;

    let remote = match state
        .mail
        .fetch_next_imap_message_summaries(
            &account.imap,
            password,
            &folder.path,
            folder.uid_validity,
            folder.lowest_synced_uid,
            folder.highest_synced_uid,
            limit,
        )
        .await
    {
        Ok(remote) => remote,
        Err(err) => {
            let message = err.to_string();
            let _ =
                record_folder_sync_error(state, owner_id, account.id, folder.id, &message).await;
            return Err(err);
        }
    };

    let mut removed_messages = 0usize;
    if remote.uid_validity_changed {
        removed_messages += delete_cached_messages_matching(
            state,
            doc! {
                "owner_id": owner_id,
                "account_id": account.id,
                "folder_id": folder.id,
            },
        )
        .await?;
    } else if remote.status.exists == Some(0) {
        removed_messages += delete_cached_messages_matching(
            state,
            doc! {
                "owner_id": owner_id,
                "account_id": account.id,
                "folder_id": folder.id,
            },
        )
        .await?;
    } else {
        for range in &remote.synced_uid_ranges {
            removed_messages += delete_cached_messages_missing_from_uid_range(
                state,
                owner_id,
                account.id,
                folder.id,
                *range,
                &remote.messages,
            )
            .await?;
        }
    }

    let now = Utc::now();
    let mut new_messages = 0usize;
    let mut refreshed_messages = 0usize;
    for message in &remote.messages {
        match store_message_summary(state, owner_id, account.id, folder, message, now).await? {
            StoreMessageSummaryResult::Inserted => new_messages += 1,
            StoreMessageSummaryResult::Refreshed => refreshed_messages += 1,
        }
    }

    let lowest_synced_uid = remote.lowest_synced_uid;
    let highest_synced_uid = remote.highest_synced_uid;
    let mut set = doc! {
        "uid_validity": optional_u32_bson(remote.status.uid_validity),
        "uid_next": optional_u32_bson(remote.status.uid_next),
        "exists": optional_u32_bson(remote.status.exists),
        "unseen": optional_u32_bson(remote.status.unseen),
        "last_sync_finished_at": bson::DateTime::from_chrono(now),
        "updated_at": bson::DateTime::from_chrono(now),
    };
    match highest_synced_uid {
        Some(uid) => {
            set.insert("highest_synced_uid", bson::Bson::Int64(uid as i64));
        }
        None => {
            set.insert("highest_synced_uid", bson::Bson::Null);
        }
    }
    match lowest_synced_uid {
        Some(uid) => {
            set.insert("lowest_synced_uid", bson::Bson::Int64(uid as i64));
        }
        None => {
            set.insert("lowest_synced_uid", bson::Bson::Null);
        }
    }
    coll.update_one(
        doc! { "_id": folder.id, "owner_id": owner_id, "account_id": account.id },
        doc! {
            "$set": set,
            "$unset": { "last_sync_error": "" },
        },
    )
    .await?;

    Ok(MailFolderSyncResponse {
        account_id: account.id.to_hex(),
        folder_id: folder.id.to_hex(),
        folder_path: folder.path.clone(),
        fetched_messages: remote.messages.len(),
        stored_messages: remote.messages.len(),
        new_messages,
        refreshed_messages,
        removed_messages,
        uid_validity: remote.status.uid_validity,
        uid_next: remote.status.uid_next,
        exists: remote.status.exists,
        unseen: remote.status.unseen,
        highest_synced_uid,
        lowest_synced_uid,
        completed: remote.completed,
        error: None,
    })
}

async fn sync_account_inner(
    state: &AppState,
    owner_id: ObjectId,
    account: &MailAccount,
    password: &str,
    limit: u32,
) -> Result<MailAccountSyncResponse> {
    let started_at = Utc::now();
    state
        .db
        .collection::<MailAccount>(ACCOUNTS)
        .update_one(
            doc! { "_id": account.id, "owner_id": owner_id },
            doc! {
                "$set": {
                    "last_sync_attempt_at": bson::DateTime::from_chrono(started_at),
                }
            },
        )
        .await?;

    let remote = state
        .mail
        .list_imap_mailboxes(&account.imap, password)
        .await?;
    let folders = upsert_remote_folders(state, owner_id, account.id, remote).await?;

    let mut folder_results = Vec::new();
    for folder in folders
        .iter()
        .filter(|folder| folder.selectable && folder.sync_enabled)
    {
        match sync_one_folder(state, owner_id, account, folder, password, limit).await {
            Ok(result) => folder_results.push(result),
            Err(err) => folder_results.push(MailFolderSyncResponse {
                account_id: account.id.to_hex(),
                folder_id: folder.id.to_hex(),
                folder_path: folder.path.clone(),
                fetched_messages: 0,
                stored_messages: 0,
                new_messages: 0,
                refreshed_messages: 0,
                removed_messages: 0,
                uid_validity: folder.uid_validity,
                uid_next: folder.uid_next,
                exists: folder.exists,
                unseen: folder.unseen,
                highest_synced_uid: folder.highest_synced_uid,
                lowest_synced_uid: folder.lowest_synced_uid,
                completed: false,
                error: Some(err.to_string()),
            }),
        }
    }

    let fetched_messages = folder_results
        .iter()
        .map(|folder| folder.fetched_messages)
        .sum();
    let stored_messages = folder_results
        .iter()
        .map(|folder| folder.stored_messages)
        .sum();
    let new_messages = folder_results
        .iter()
        .map(|folder| folder.new_messages)
        .sum();
    let refreshed_messages = folder_results
        .iter()
        .map(|folder| folder.refreshed_messages)
        .sum();
    let removed_messages = folder_results
        .iter()
        .map(|folder| folder.removed_messages)
        .sum();
    let errors = folder_results
        .iter()
        .filter(|folder| folder.error.is_some())
        .count();
    if errors == 0 {
        state
            .db
            .collection::<MailAccount>(ACCOUNTS)
            .update_one(
                doc! { "_id": account.id, "owner_id": owner_id },
                doc! {
                    "$set": {
                        "last_sync_at": bson::DateTime::from_chrono(Utc::now()),
                        "updated_at": bson::DateTime::from_chrono(Utc::now()),
                    }
                },
            )
            .await?;
    }

    Ok(MailAccountSyncResponse {
        account_id: account.id.to_hex(),
        fetched_messages,
        stored_messages,
        new_messages,
        refreshed_messages,
        removed_messages,
        errors,
        folders: folder_results,
    })
}

fn account_sync_conflict() -> AppError {
    AppError::Conflict("Mail sync already in progress for this account".to_string())
}

pub async fn sync_folder(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path((account_id, folder_id)): Path<(String, String)>,
    Json(req): Json<MailSyncRequest>,
) -> Result<Json<MailFolderSyncResponse>> {
    require_mail(&state, &user)?;
    let account_id = parse_oid(&account_id, "mail account id")?;
    let folder_id = parse_oid(&folder_id, "mail folder id")?;
    let account = find_account(&state, user.id, account_id).await?;
    let folder = find_folder(&state, user.id, account_id, folder_id).await?;
    let password = resolve_mail_password(&state, &account, req.password.as_deref())?;
    let _guard = state
        .mail
        .try_begin_account_sync(account.id)
        .ok_or_else(account_sync_conflict)?;
    Ok(Json(
        sync_one_folder(
            &state,
            user.id,
            &account,
            &folder,
            &password,
            sync_limit(req.limit_per_folder),
        )
        .await?,
    ))
}

pub async fn sync_account(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(account_id): Path<String>,
    Json(req): Json<MailSyncRequest>,
) -> Result<Json<MailAccountSyncResponse>> {
    require_mail(&state, &user)?;
    let account_id = parse_oid(&account_id, "mail account id")?;
    let account = find_account(&state, user.id, account_id).await?;
    let password = resolve_mail_password(&state, &account, req.password.as_deref())?;
    let limit = sync_limit(req.limit_per_folder);
    let _guard = state
        .mail
        .try_begin_account_sync(account.id)
        .ok_or_else(account_sync_conflict)?;
    Ok(Json(
        sync_account_inner(&state, user.id, &account, &password, limit).await?,
    ))
}

pub fn spawn_mail_sync_scheduler(state: Arc<AppState>) {
    if !state
        .config
        .features
        .is_enabled(crate::config::FEATURE_MAIL)
        || !state.config.mail_sync.enabled
    {
        tracing::debug!("mail background sync scheduler disabled");
        return;
    }

    let default_interval = state.config.mail_sync.interval_secs.clamp(
        MIN_ACCOUNT_SYNC_INTERVAL_SECS,
        MAX_ACCOUNT_SYNC_INTERVAL_SECS,
    );
    let tick_interval = Duration::from_secs(BACKGROUND_SYNC_TICK_SECS);
    let startup_delay = Duration::from_secs(state.config.mail_sync.startup_delay_secs);
    tracing::info!(
        "mail background sync scheduler enabled: default_interval={}s, tick={}s, startup_delay={}s",
        default_interval,
        tick_interval.as_secs(),
        startup_delay.as_secs()
    );

    tokio::spawn(async move {
        if !startup_delay.is_zero() {
            sleep(startup_delay).await;
        }

        loop {
            if let Err(err) = run_scheduled_mail_sync_tick(state.clone()).await {
                tracing::warn!("mail background sync tick failed: {}", err);
            }
            sleep(tick_interval).await;
        }
    });
}

pub async fn run_scheduled_mail_sync_tick(state: Arc<AppState>) -> Result<usize> {
    require_mail_available(&state)?;
    let now = Utc::now();
    let default_interval = state.config.mail_sync.interval_secs.clamp(
        MIN_ACCOUNT_SYNC_INTERVAL_SECS,
        MAX_ACCOUNT_SYNC_INTERVAL_SECS,
    );
    let limit = sync_limit(Some(state.config.mail_sync.limit_per_folder));
    let mut cursor = state
        .db
        .collection::<MailAccount>(ACCOUNTS)
        .find(doc! {
            "sync_enabled": true,
        })
        .sort(doc! { "email_address": 1 })
        .await?;
    let mut synced_accounts = 0usize;

    while let Some(account) = cursor.try_next().await? {
        if !account_due_for_scheduled_sync(&account, now, default_interval) {
            continue;
        }
        if account.credential.is_none() {
            continue;
        }
        if !mail_sync_owner_enabled(&state, account.owner_id).await? {
            continue;
        }
        let Some(_guard) = state.mail.try_begin_account_sync(account.id) else {
            tracing::debug!(
                account_id = %account.id,
                email = %account.email_address,
                "mail background sync skipped account already in progress"
            );
            continue;
        };
        let password = match resolve_mail_password(&state, &account, None) {
            Ok(password) => password,
            Err(err) => {
                tracing::warn!(
                    account_id = %account.id,
                    email = %account.email_address,
                    "mail background sync skipped account credential error: {}",
                    err
                );
                continue;
            }
        };

        match sync_account_inner(&state, account.owner_id, &account, &password, limit).await {
            Ok(result) => {
                synced_accounts += 1;
                log_scheduled_sync_result(&account, &result);
            }
            Err(err) => {
                tracing::warn!(
                    account_id = %account.id,
                    email = %account.email_address,
                    "mail background sync failed: {}",
                    err
                );
            }
        }
    }

    Ok(synced_accounts)
}

async fn mail_sync_owner_enabled(state: &AppState, owner_id: ObjectId) -> Result<bool> {
    let user = state
        .db
        .collection::<User>("users")
        .find_one(doc! { "_id": owner_id })
        .await?;
    let Some(user) = user else {
        return Ok(false);
    };
    Ok(user.is_active()
        && !user
            .disabled_features
            .iter()
            .any(|feature| feature == "mail"))
}

fn log_scheduled_sync_result(account: &MailAccount, result: &MailAccountSyncResponse) {
    let changes = result.new_messages + result.refreshed_messages + result.removed_messages;
    if result.errors > 0 {
        tracing::warn!(
            account_id = %account.id,
            email = %account.email_address,
            errors = result.errors,
            new_messages = result.new_messages,
            refreshed_messages = result.refreshed_messages,
            removed_messages = result.removed_messages,
            "mail background sync completed with folder errors"
        );
    } else if changes > 0 {
        tracing::info!(
            account_id = %account.id,
            email = %account.email_address,
            new_messages = result.new_messages,
            refreshed_messages = result.refreshed_messages,
            removed_messages = result.removed_messages,
            "mail background sync completed"
        );
    } else {
        tracing::debug!(
            account_id = %account.id,
            email = %account.email_address,
            "mail background sync completed with no changes"
        );
    }
}

pub async fn list_messages(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path((account_id, folder_id)): Path<(String, String)>,
    Query(query): Query<MailMessageListQuery>,
) -> Result<Json<MailMessageListResponse>> {
    require_mail(&state, &user)?;
    let account_id = parse_oid(&account_id, "mail account id")?;
    let folder_id = parse_oid(&folder_id, "mail folder id")?;
    let _ = find_account(&state, user.id, account_id).await?;
    let _ = find_folder(&state, user.id, account_id, folder_id).await?;
    let limit = message_list_limit(query.limit);
    let cursor_uid = parse_message_list_cursor(query.cursor.as_deref())?;
    let mut filter = doc! { "owner_id": user.id, "account_id": account_id, "folder_id": folder_id };
    if let Some(uid) = cursor_uid {
        filter.insert("uid", doc! { "$lt": uid as i64 });
    }
    let mut cursor = state
        .db
        .collection::<MailMessage>(MESSAGES)
        .find(filter)
        .sort(doc! { "uid": -1 })
        .limit(limit + 1)
        .await?;
    let mut messages = Vec::new();
    while let Some(message) = cursor.try_next().await? {
        messages.push(message);
    }
    let has_more = messages.len() > limit as usize;
    if has_more {
        messages.truncate(limit as usize);
    }
    let next_cursor = if has_more {
        messages.last().map(|message| message.uid.to_string())
    } else {
        None
    };
    Ok(Json(MailMessageListResponse {
        messages: messages.iter().map(message_to_response).collect::<Vec<_>>(),
        next_cursor,
        has_more,
    }))
}

async fn update_cached_message_flags(
    state: &AppState,
    owner_id: ObjectId,
    folder: &MailFolder,
    message: &MailMessage,
    flags: Vec<String>,
) -> Result<MailMessage> {
    let was_seen = message_has_flag(message, "\\Seen");
    let is_seen = flags.iter().any(|flag| mail_flag_eq(flag, "\\Seen"));
    let now = Utc::now();
    state
        .db
        .collection::<MailMessage>(MESSAGES)
        .update_one(
            doc! { "_id": message.id, "owner_id": owner_id },
            doc! {
                "$set": {
                    "flags": flags,
                    "updated_at": bson::DateTime::from_chrono(now),
                }
            },
        )
        .await?;

    if was_seen != is_seen {
        update_folder_unseen_after_seen_change(state, owner_id, folder, is_seen).await?;
    }

    find_message(state, owner_id, message.id).await
}

async fn update_cached_messages_flags(
    state: &AppState,
    owner_id: ObjectId,
    folder: &MailFolder,
    messages: &[MailMessage],
    flags: Vec<RemoteMessageFlags>,
) -> Result<Vec<MailMessage>> {
    let flags_by_uid = flags
        .into_iter()
        .map(|row| (row.uid, row.flags))
        .collect::<HashMap<_, _>>();
    let now = Utc::now();
    let mut updated = Vec::new();
    let mut unseen_delta = 0i64;
    for message in messages {
        let Some(flags) = flags_by_uid.get(&message.uid).cloned() else {
            continue;
        };
        let was_seen = message_has_flag(message, "\\Seen");
        let is_seen = flags.iter().any(|flag| mail_flag_eq(flag, "\\Seen"));
        state
            .db
            .collection::<MailMessage>(MESSAGES)
            .update_one(
                doc! { "_id": message.id, "owner_id": owner_id },
                doc! {
                    "$set": {
                        "flags": &flags,
                        "updated_at": bson::DateTime::from_chrono(now),
                    }
                },
            )
            .await?;
        if was_seen != is_seen {
            unseen_delta += if is_seen { -1 } else { 1 };
        }
        let mut row = message.clone();
        row.flags = flags;
        row.updated_at = now;
        updated.push(row);
    }

    update_folder_unseen_delta(state, owner_id, folder, unseen_delta).await?;
    Ok(updated)
}

async fn update_folder_unseen_after_seen_change(
    state: &AppState,
    owner_id: ObjectId,
    folder: &MailFolder,
    is_seen: bool,
) -> Result<()> {
    let Some(unseen) = folder.unseen else {
        return Ok(());
    };
    let next = if is_seen {
        unseen.saturating_sub(1)
    } else {
        unseen.saturating_add(1)
    };
    state
        .db
        .collection::<MailFolder>(FOLDERS)
        .update_one(
            doc! { "_id": folder.id, "owner_id": owner_id, "account_id": folder.account_id },
            doc! {
                "$set": {
                    "unseen": bson::Bson::Int64(next as i64),
                    "updated_at": bson::DateTime::from_chrono(Utc::now()),
                }
            },
        )
        .await?;
    Ok(())
}

async fn update_folder_unseen_delta(
    state: &AppState,
    owner_id: ObjectId,
    folder: &MailFolder,
    delta: i64,
) -> Result<()> {
    if delta == 0 {
        return Ok(());
    }
    let Some(unseen) = folder.unseen else {
        return Ok(());
    };
    let next = if delta.is_negative() {
        unseen.saturating_sub(delta.unsigned_abs() as u32)
    } else {
        unseen.saturating_add(delta as u32)
    };
    state
        .db
        .collection::<MailFolder>(FOLDERS)
        .update_one(
            doc! { "_id": folder.id, "owner_id": owner_id, "account_id": folder.account_id },
            doc! {
                "$set": {
                    "unseen": bson::Bson::Int64(next as i64),
                    "updated_at": bson::DateTime::from_chrono(Utc::now()),
                }
            },
        )
        .await?;
    Ok(())
}

async fn mark_cached_folder_read(
    state: &AppState,
    owner_id: ObjectId,
    folder: &MailFolder,
) -> Result<u64> {
    let mut cursor = state
        .db
        .collection::<MailMessage>(MESSAGES)
        .find(doc! {
            "owner_id": owner_id,
            "account_id": folder.account_id,
            "folder_id": folder.id,
        })
        .await?;
    let mut unread_ids = Vec::new();
    while let Some(message) = cursor.try_next().await? {
        if !message_has_flag(&message, "\\Seen") {
            unread_ids.push(message.id);
        }
    }

    let now = Utc::now();
    if !unread_ids.is_empty() {
        state
            .db
            .collection::<MailMessage>(MESSAGES)
            .update_many(
                doc! { "_id": { "$in": &unread_ids }, "owner_id": owner_id },
                doc! {
                    "$addToSet": { "flags": "\\Seen" },
                    "$set": { "updated_at": bson::DateTime::from_chrono(now) },
                },
            )
            .await?;
    }
    state
        .db
        .collection::<MailFolder>(FOLDERS)
        .update_one(
            doc! { "_id": folder.id, "owner_id": owner_id, "account_id": folder.account_id },
            doc! {
                "$set": {
                    "unseen": bson::Bson::Int64(0),
                    "updated_at": bson::DateTime::from_chrono(now),
                }
            },
        )
        .await?;
    Ok(unread_ids.len() as u64)
}

async fn remove_cached_message_after_move(
    state: &AppState,
    owner_id: ObjectId,
    source_folder: &MailFolder,
    destination_folder: &MailFolder,
    message: &MailMessage,
) -> Result<()> {
    state
        .db
        .collection::<MailMessage>(MESSAGES)
        .delete_one(doc! { "_id": message.id, "owner_id": owner_id })
        .await?;

    let now = Utc::now();
    let was_unseen = !message_has_flag(message, "\\Seen");
    let mut source_set = doc! { "updated_at": bson::DateTime::from_chrono(now) };
    if let Some(exists) = source_folder.exists {
        source_set.insert("exists", bson::Bson::Int64(exists.saturating_sub(1) as i64));
    }
    if was_unseen {
        if let Some(unseen) = source_folder.unseen {
            source_set.insert("unseen", bson::Bson::Int64(unseen.saturating_sub(1) as i64));
        }
    }
    state
        .db
        .collection::<MailFolder>(FOLDERS)
        .update_one(
            doc! {
                "_id": source_folder.id,
                "owner_id": owner_id,
                "account_id": source_folder.account_id,
            },
            doc! { "$set": source_set },
        )
        .await?;

    let mut destination_set = doc! { "updated_at": bson::DateTime::from_chrono(now) };
    if let Some(exists) = destination_folder.exists {
        destination_set.insert("exists", bson::Bson::Int64(exists.saturating_add(1) as i64));
    }
    if was_unseen {
        if let Some(unseen) = destination_folder.unseen {
            destination_set.insert("unseen", bson::Bson::Int64(unseen.saturating_add(1) as i64));
        }
    }
    state
        .db
        .collection::<MailFolder>(FOLDERS)
        .update_one(
            doc! {
                "_id": destination_folder.id,
                "owner_id": owner_id,
                "account_id": destination_folder.account_id,
            },
            doc! { "$set": destination_set },
        )
        .await?;

    Ok(())
}

async fn remove_cached_messages_after_move(
    state: &AppState,
    owner_id: ObjectId,
    source_folder: &MailFolder,
    destination_folder: &MailFolder,
    messages: &[MailMessage],
) -> Result<()> {
    if messages.is_empty() {
        return Ok(());
    }

    let message_ids = messages
        .iter()
        .map(|message| message.id)
        .collect::<Vec<_>>();
    state
        .db
        .collection::<MailMessage>(MESSAGES)
        .delete_many(doc! { "_id": { "$in": &message_ids }, "owner_id": owner_id })
        .await?;

    let now = Utc::now();
    let message_count = messages.len() as u32;
    let unseen_count = messages
        .iter()
        .filter(|message| !message_has_flag(message, "\\Seen"))
        .count() as u32;

    let mut source_set = doc! { "updated_at": bson::DateTime::from_chrono(now) };
    if let Some(exists) = source_folder.exists {
        source_set.insert(
            "exists",
            bson::Bson::Int64(exists.saturating_sub(message_count) as i64),
        );
    }
    if unseen_count > 0 {
        if let Some(unseen) = source_folder.unseen {
            source_set.insert(
                "unseen",
                bson::Bson::Int64(unseen.saturating_sub(unseen_count) as i64),
            );
        }
    }
    state
        .db
        .collection::<MailFolder>(FOLDERS)
        .update_one(
            doc! {
                "_id": source_folder.id,
                "owner_id": owner_id,
                "account_id": source_folder.account_id,
            },
            doc! { "$set": source_set },
        )
        .await?;

    let mut destination_set = doc! { "updated_at": bson::DateTime::from_chrono(now) };
    if let Some(exists) = destination_folder.exists {
        destination_set.insert(
            "exists",
            bson::Bson::Int64(exists.saturating_add(message_count) as i64),
        );
    }
    if unseen_count > 0 {
        if let Some(unseen) = destination_folder.unseen {
            destination_set.insert(
                "unseen",
                bson::Bson::Int64(unseen.saturating_add(unseen_count) as i64),
            );
        }
    }
    state
        .db
        .collection::<MailFolder>(FOLDERS)
        .update_one(
            doc! {
                "_id": destination_folder.id,
                "owner_id": owner_id,
                "account_id": destination_folder.account_id,
            },
            doc! { "$set": destination_set },
        )
        .await?;

    Ok(())
}

async fn mutation_destination_folder(
    state: &AppState,
    owner_id: ObjectId,
    account_id: ObjectId,
    req: &MailMessageMutationRequest,
) -> Result<MailFolder> {
    match req.action {
        MailMessageMutationAction::Move => {
            let target = req
                .target_folder_id
                .as_deref()
                .ok_or_else(|| AppError::BadRequest("target_folder_id is required".into()))?;
            let target = parse_oid(target, "target mail folder id")?;
            let folder = find_folder(state, owner_id, account_id, target).await?;
            if !folder.selectable {
                return Err(AppError::BadRequest(
                    "target mail folder is not selectable".into(),
                ));
            }
            Ok(folder)
        }
        MailMessageMutationAction::Archive => {
            find_folder_by_role(
                state,
                owner_id,
                account_id,
                &[MailFolderRole::Archive, MailFolderRole::AllMail],
            )
            .await
        }
        MailMessageMutationAction::Trash => {
            find_folder_by_role(state, owner_id, account_id, &[MailFolderRole::Trash]).await
        }
        _ => Err(AppError::BadRequest(
            "mail mutation action does not have a destination".into(),
        )),
    }
}

async fn replace_message_attachments(
    state: &AppState,
    owner_id: ObjectId,
    account_id: ObjectId,
    message_id: ObjectId,
    body: &RemoteMessageBody,
    stored: &StoredMailBody,
    now: chrono::DateTime<Utc>,
) -> Result<Vec<MailAttachment>> {
    let coll = state.db.collection::<MailAttachment>(ATTACHMENTS);
    coll.delete_many(doc! { "owner_id": owner_id, "message_id": message_id })
        .await?;

    let mut attachments = Vec::new();
    for stored_attachment in &stored.attachments {
        let Some(remote) = body.attachments.get(stored_attachment.index) else {
            continue;
        };
        attachments.push(MailAttachment {
            id: ObjectId::new(),
            owner_id,
            account_id,
            message_id,
            filename: remote.filename.clone(),
            content_type: remote.content_type.clone(),
            content_id: remote.content_id.clone(),
            disposition: remote.disposition.clone(),
            size_bytes: Some(stored_attachment.size_bytes),
            storage_id: Some(stored.storage_id),
            storage_path: Some(stored_attachment.storage_path.clone()),
            created_at: now,
        });
    }

    if !attachments.is_empty() {
        coll.insert_many(&attachments).await?;
    }

    Ok(attachments)
}

pub async fn get_message(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(message_id): Path<String>,
) -> Result<Json<MailMessageDetailResponse>> {
    require_mail(&state, &user)?;
    let message_id = parse_oid(&message_id, "mail message id")?;
    let message = find_message(&state, user.id, message_id).await?;
    match read_cached_message_body(&state.storage, &message).await {
        Ok(Some(body)) => {
            let attachments = list_message_attachments(&state, user.id, message.id).await?;
            return Ok(Json(MailMessageDetailResponse {
                message: message_to_response(&message),
                body_text: body.text,
                body_html: mail_body_html_for_response(body.html, &attachments),
                attachments: attachments.iter().map(attachment_to_response).collect(),
            }));
        }
        Ok(None) => {}
        Err(err) => {
            tracing::warn!(
                "mail cached body read failed for message {}: {}",
                message.id,
                err
            );
        }
    }

    let account = find_account(&state, user.id, message.account_id).await?;
    let folder = find_folder(&state, user.id, message.account_id, message.folder_id).await?;
    let password = resolve_mail_password(&state, &account, None)?;
    let body = state
        .mail
        .fetch_imap_message_body(&account.imap, &password, &folder.path, message.uid)
        .await
        .ok();
    let mut response_message = message.clone();
    let mut attachments = Vec::new();
    if let Some(body) = body.as_ref() {
        match store_message_body(
            &state.storage,
            &user.username,
            &account,
            &folder,
            &message,
            body,
        )
        .await
        {
            Ok(stored) => {
                let now = Utc::now();
                attachments = replace_message_attachments(
                    &state, user.id, account.id, message.id, body, &stored, now,
                )
                .await?;
                response_message.has_attachments = !attachments.is_empty();
                state
                    .db
                    .collection::<MailMessage>(MESSAGES)
                    .update_one(
                        doc! { "_id": message.id, "owner_id": user.id },
                        doc! {
                            "$set": {
                                "mail_storage_id": stored.storage_id,
                                "raw_storage_path": stored.raw_path,
                                "raw_storage_size_bytes": stored.raw_size_bytes as i64,
                                "text_storage_path": optional_string_bson(stored.text_path),
                                "text_storage_size_bytes": optional_u64_bson(stored.text_size_bytes),
                                "html_storage_path": optional_string_bson(stored.html_path),
                                "html_storage_size_bytes": optional_u64_bson(stored.html_size_bytes),
                                "has_attachments": response_message.has_attachments,
                                "updated_at": bson::DateTime::from_chrono(now),
                            },
                        },
                    )
                    .await?;
            }
            Err(err) => {
                tracing::warn!(
                    "mail cached body write failed for message {}: {}",
                    message.id,
                    err
                );
            }
        }
    }

    Ok(Json(MailMessageDetailResponse {
        message: message_to_response(&response_message),
        body_text: body.as_ref().and_then(|body| body.text.clone()),
        body_html: mail_body_html_for_response(body.and_then(|body| body.html), &attachments),
        attachments: attachments.iter().map(attachment_to_response).collect(),
    }))
}

pub async fn download_attachment(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(attachment_id): Path<String>,
) -> Result<Response> {
    require_mail(&state, &user)?;
    stream_attachment_response(&state, user.id, &attachment_id, "attachment").await
}

pub async fn open_attachment(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(attachment_id): Path<String>,
) -> Result<Response> {
    require_mail(&state, &user)?;
    stream_attachment_response(&state, user.id, &attachment_id, "inline").await
}

async fn stream_attachment_response(
    state: &AppState,
    owner_id: ObjectId,
    attachment_id: &str,
    disposition: &str,
) -> Result<Response> {
    let attachment_id = parse_oid(attachment_id, "mail attachment id")?;
    let attachment = state
        .db
        .collection::<MailAttachment>(ATTACHMENTS)
        .find_one(doc! { "_id": attachment_id, "owner_id": owner_id })
        .await?
        .ok_or_else(|| AppError::NotFound("Mail attachment".into()))?;
    let storage_id = attachment
        .storage_id
        .ok_or_else(|| AppError::NotFound("Mail attachment blob".into()))?;
    let storage_path = attachment
        .storage_path
        .as_deref()
        .filter(|path| !path.trim().is_empty())
        .ok_or_else(|| AppError::NotFound("Mail attachment blob".into()))?;

    let backend = state.storage.get_backend(storage_id).await?;
    let reader = backend.read(storage_path).await?;
    let stream = ReaderStream::new(reader);
    let body = Body::from_stream(stream);

    let filename = attachment
        .filename
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("attachment");
    let content_disposition = format!(
        "{disposition}; filename=\"{}\"",
        filename.replace('"', "\\\"")
    );
    let content_type = attachment
        .content_type
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("application/octet-stream");

    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_DISPOSITION, content_disposition);
    if let Some(size) = attachment.size_bytes {
        response = response.header(header::CONTENT_LENGTH, size);
    }
    Ok(response.body(body).unwrap())
}

async fn files_destination_owner(
    state: &AppState,
    user: &AuthUser,
    parent_id: Option<ObjectId>,
) -> Result<(ObjectId, String)> {
    let Some(parent_id) = parent_id else {
        return Ok((user.id, user.username.clone()));
    };

    let parent = state
        .db
        .collection::<Folder>("folders")
        .find_one(doc! { "_id": parent_id, "deleted_at": mongodb::bson::Bson::Null })
        .await?
        .ok_or_else(|| AppError::NotFound("Parent folder not found".into()))?;
    if parent.owner_id == user.id {
        return Ok((user.id, user.username.clone()));
    }

    let access = check_folder_access(&state.db, user.id, parent_id).await?;
    if !access.can_write() {
        return Err(if access.can_read() {
            AppError::Forbidden("Read-only access".into())
        } else {
            AppError::NotFound("Parent folder not found".into())
        });
    }

    let owner_username = state
        .db
        .collection::<User>("users")
        .find_one(doc! { "_id": parent.owner_id })
        .await?
        .map(|owner| owner.username)
        .unwrap_or_else(|| user.username.clone());
    Ok((parent.owner_id, owner_username))
}

pub async fn save_attachment_to_files(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    meta: RequestMeta,
    Path(attachment_id): Path<String>,
    Json(req): Json<SaveMailAttachmentRequest>,
) -> Result<Json<SaveMailAttachmentResponse>> {
    require_mail(&state, &user)?;
    let attachment_id = parse_oid(&attachment_id, "mail attachment id")?;
    let attachment = state
        .db
        .collection::<MailAttachment>(ATTACHMENTS)
        .find_one(doc! { "_id": attachment_id, "owner_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Mail attachment".into()))?;
    let source_storage_id = attachment
        .storage_id
        .ok_or_else(|| AppError::NotFound("Mail attachment blob".into()))?;
    let source_storage_path = attachment
        .storage_path
        .as_deref()
        .filter(|path| !path.trim().is_empty())
        .ok_or_else(|| AppError::NotFound("Mail attachment blob".into()))?;

    let parent_id = match req.parent_id.as_deref().map(str::trim) {
        Some("") | None => None,
        Some(id) => Some(parse_oid(id, "parent folder id")?),
    };
    let filename = req
        .filename
        .as_deref()
        .or(attachment.filename.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("attachment");
    let filename = validate_label(filename, "attachment filename")?;
    let (effective_owner_id, effective_username) =
        files_destination_owner(&state, &user, parent_id).await?;

    if check_name_conflict(
        &state.db,
        effective_owner_id,
        parent_id,
        &filename,
        None,
        None,
    )
    .await?
    {
        return Err(AppError::Conflict(
            "A file with this name already exists at this location".to_string(),
        ));
    }

    let source_backend = state.storage.get_backend(source_storage_id).await?;
    let file_data = source_backend.read_all(source_storage_path).await?;
    let size = file_data.len() as i64;
    {
        let users_coll = state.db.collection::<User>("users");
        if let Some(owner) = users_coll
            .find_one(doc! { "_id": effective_owner_id })
            .await?
        {
            if !owner.has_quota_space(size) {
                return Err(AppError::Forbidden("Quota exceeded".into()));
            }
        }
    }

    let storage_id = state.storage.resolve_storage_for_parent(parent_id).await?;
    let storage = state.storage.get_storage(storage_id).await?;
    let backend = state.storage.get_backend(storage.id).await?;
    let storage_path = resolve_storage_path(
        &state.db,
        effective_owner_id,
        &effective_username,
        parent_id,
        &filename,
    )
    .await?;

    let mut hasher = Sha256::new();
    hasher.update(&file_data);
    let checksum = hex::encode(hasher.finalize());
    backend.write(&storage_path, &file_data).await?;

    let mime_type = attachment
        .content_type
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            mime_guess::from_path(&filename)
                .first_or_octet_stream()
                .to_string()
        });
    let file = File::new(
        storage.id,
        storage_path,
        effective_owner_id,
        parent_id,
        filename,
        mime_type,
        size,
        checksum,
    );

    let collection = state.db.collection::<File>("files");
    if let Err(err) = collection.insert_one(&file).await {
        if is_duplicate_key_error(&err) {
            return Err(AppError::Conflict(
                "A file with this name already exists at this location".to_string(),
            ));
        }
        return Err(err.into());
    }

    state
        .auth
        .update_user_bytes(effective_owner_id, size)
        .await?;
    state
        .events
        .emit_file_created(effective_owner_id, &file)
        .await;
    state
        .sync_log
        .record(super::audit::file_event(
            effective_owner_id,
            uncloud_common::SyncOperation::Created,
            file.id,
            file.storage_path.clone(),
            None,
            &meta,
        ))
        .await;

    {
        let state_clone = state.clone();
        let file_id = file.id.to_hex();
        let owner_id = effective_owner_id.to_hex();
        let username = effective_username.clone();
        let name = file.name.clone();
        tokio::spawn(async move {
            deliver_webhooks(
                &state_clone,
                EVENT_FILE_CREATED,
                serde_json::json!({
                    "file_id": file_id,
                    "owner_id": owner_id,
                    "username": username,
                    "name": name,
                }),
            )
            .await;
        });
    }
    state.processing.enqueue(&file, state.clone()).await;

    Ok(Json(SaveMailAttachmentResponse {
        file: file_to_response(&file),
    }))
}

async fn list_account_mail_folders(
    state: &AppState,
    owner_id: ObjectId,
    account_id: ObjectId,
) -> Result<Vec<MailFolder>> {
    let mut cursor = state
        .db
        .collection::<MailFolder>(FOLDERS)
        .find(doc! { "owner_id": owner_id, "account_id": account_id })
        .sort(doc! { "path": 1 })
        .await?;
    let mut folders = Vec::new();
    while let Some(folder) = cursor.try_next().await? {
        folders.push(folder);
    }
    Ok(folders)
}

async fn mark_folder_read_inner(
    state: &AppState,
    owner_id: ObjectId,
    account: &MailAccount,
    folder: &MailFolder,
    password: &str,
) -> Result<u64> {
    if !folder.selectable {
        return Err(AppError::BadRequest("mail folder is not selectable".into()));
    }
    state
        .mail
        .mark_imap_folder_read(&account.imap, password, &folder.path)
        .await?;
    mark_cached_folder_read(state, owner_id, folder).await
}

pub async fn mark_folder_read(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path((account_id, folder_id)): Path<(String, String)>,
) -> Result<Json<MailFolderMarkReadResponse>> {
    require_mail(&state, &user)?;
    let account_id = parse_oid(&account_id, "mail account id")?;
    let folder_id = parse_oid(&folder_id, "mail folder id")?;
    let account = find_account(&state, user.id, account_id).await?;
    let folder = find_folder(&state, user.id, account_id, folder_id).await?;
    let password = resolve_mail_password(&state, &account, None)?;
    let mut errors = Vec::new();
    let mut updated_cached_messages = 0u64;
    match mark_folder_read_inner(&state, user.id, &account, &folder, &password).await {
        Ok(updated) => updated_cached_messages = updated,
        Err(err) => errors.push(MailFolderMutationError {
            folder_id: folder.id.to_hex(),
            folder_path: folder.path.clone(),
            error: err.to_string(),
        }),
    }
    let folders = list_account_mail_folders(&state, user.id, account_id).await?;
    let sync_in_progress = state.mail.is_account_syncing(account_id);
    let failed = errors.len();
    Ok(Json(MailFolderMarkReadResponse {
        account_id: account_id.to_hex(),
        requested: 1,
        succeeded: usize::from(failed == 0),
        failed,
        updated_cached_messages,
        folders: folders
            .iter()
            .map(|folder| folder_to_response(folder, sync_in_progress))
            .collect(),
        errors,
    }))
}

pub async fn mark_account_read(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(account_id): Path<String>,
) -> Result<Json<MailFolderMarkReadResponse>> {
    require_mail(&state, &user)?;
    let account_id = parse_oid(&account_id, "mail account id")?;
    let account = find_account(&state, user.id, account_id).await?;
    let password = resolve_mail_password(&state, &account, None)?;
    let folders = list_account_mail_folders(&state, user.id, account_id).await?;
    let target_folders = folders
        .iter()
        .filter(|folder| folder.selectable)
        .cloned()
        .collect::<Vec<_>>();
    let mut errors = Vec::new();
    let mut updated_cached_messages = 0u64;
    for folder in &target_folders {
        match mark_folder_read_inner(&state, user.id, &account, folder, &password).await {
            Ok(updated) => {
                updated_cached_messages = updated_cached_messages.saturating_add(updated)
            }
            Err(err) => errors.push(MailFolderMutationError {
                folder_id: folder.id.to_hex(),
                folder_path: folder.path.clone(),
                error: err.to_string(),
            }),
        }
    }

    let folders = list_account_mail_folders(&state, user.id, account_id).await?;
    let sync_in_progress = state.mail.is_account_syncing(account_id);
    let requested = target_folders.len();
    let failed = errors.len();
    Ok(Json(MailFolderMarkReadResponse {
        account_id: account_id.to_hex(),
        requested,
        succeeded: requested.saturating_sub(failed),
        failed,
        updated_cached_messages,
        folders: folders
            .iter()
            .map(|folder| folder_to_response(folder, sync_in_progress))
            .collect(),
        errors,
    }))
}

pub async fn mutate_message(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(message_id): Path<String>,
    Json(req): Json<MailMessageMutationRequest>,
) -> Result<Json<MailMessageMutationResponse>> {
    require_mail(&state, &user)?;
    let message_id = parse_oid(&message_id, "mail message id")?;
    let message = find_message(&state, user.id, message_id).await?;
    let account = find_account(&state, user.id, message.account_id).await?;
    let folder = find_folder(&state, user.id, message.account_id, message.folder_id).await?;
    let password = resolve_mail_password(&state, &account, None)?;

    match req.action {
        MailMessageMutationAction::MarkRead
        | MailMessageMutationAction::MarkUnread
        | MailMessageMutationAction::Star
        | MailMessageMutationAction::Unstar => {
            let (flag, enabled) = match req.action {
                MailMessageMutationAction::MarkRead => (RemoteMessageFlag::Seen, true),
                MailMessageMutationAction::MarkUnread => (RemoteMessageFlag::Seen, false),
                MailMessageMutationAction::Star => (RemoteMessageFlag::Flagged, true),
                MailMessageMutationAction::Unstar => (RemoteMessageFlag::Flagged, false),
                _ => unreachable!(),
            };
            let flags = state
                .mail
                .set_imap_message_flag(
                    &account.imap,
                    &password,
                    &folder.path,
                    message.uid,
                    flag,
                    enabled,
                )
                .await?;
            let updated =
                update_cached_message_flags(&state, user.id, &folder, &message, flags).await?;

            Ok(Json(MailMessageMutationResponse {
                message: Some(message_to_response(&updated)),
                removed_from_folder: false,
                destination_folder_id: None,
                destination_folder_path: None,
            }))
        }
        MailMessageMutationAction::Move
        | MailMessageMutationAction::Archive
        | MailMessageMutationAction::Trash => {
            let destination =
                mutation_destination_folder(&state, user.id, account.id, &req).await?;
            if destination.id == folder.id {
                return Ok(Json(MailMessageMutationResponse {
                    message: Some(message_to_response(&message)),
                    removed_from_folder: false,
                    destination_folder_id: Some(destination.id.to_hex()),
                    destination_folder_path: Some(destination.path),
                }));
            }

            state
                .mail
                .move_imap_message(
                    &account.imap,
                    &password,
                    &folder.path,
                    message.uid,
                    &destination.path,
                )
                .await?;
            remove_cached_message_after_move(&state, user.id, &folder, &destination, &message)
                .await?;

            Ok(Json(MailMessageMutationResponse {
                message: None,
                removed_from_folder: true,
                destination_folder_id: Some(destination.id.to_hex()),
                destination_folder_path: Some(destination.path),
            }))
        }
    }
}

pub async fn bulk_mutate_messages(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(req): Json<MailMessageBulkMutationRequest>,
) -> Result<Json<MailMessageBulkMutationResponse>> {
    require_mail(&state, &user)?;
    let requested = req.message_ids.len();
    if requested == 0 {
        return Err(AppError::BadRequest(
            "at least one mail message id is required".into(),
        ));
    }

    let mut errors = Vec::new();
    let mut parsed_ids = Vec::new();
    let mut seen = HashSet::new();
    for message_id in &req.message_ids {
        if !seen.insert(message_id.clone()) {
            continue;
        }
        match ObjectId::parse_str(message_id) {
            Ok(id) => parsed_ids.push((message_id.clone(), id)),
            Err(_) => errors.push(MailMessageBulkMutationError {
                message_id: message_id.clone(),
                error: "Invalid mail message id".to_string(),
            }),
        }
    }

    let parsed_oid_values = parsed_ids.iter().map(|(_, id)| *id).collect::<Vec<_>>();
    let mut messages = Vec::new();
    if !parsed_oid_values.is_empty() {
        let mut cursor = state
            .db
            .collection::<MailMessage>(MESSAGES)
            .find(doc! { "_id": { "$in": &parsed_oid_values }, "owner_id": user.id })
            .await?;
        while let Some(message) = cursor.try_next().await? {
            messages.push(message);
        }
    }
    let found_ids = messages
        .iter()
        .map(|message| message.id)
        .collect::<HashSet<_>>();
    for (raw, id) in &parsed_ids {
        if !found_ids.contains(id) {
            errors.push(MailMessageBulkMutationError {
                message_id: raw.clone(),
                error: "Mail message not found".to_string(),
            });
        }
    }

    if messages.is_empty() {
        let failed = errors.len();
        return Ok(Json(MailMessageBulkMutationResponse {
            requested,
            succeeded: 0,
            failed,
            messages: Vec::new(),
            removed_message_ids: Vec::new(),
            destination_folder_id: None,
            destination_folder_path: None,
            errors,
        }));
    }

    let account_id = messages[0].account_id;
    let folder_id = messages[0].folder_id;
    if messages
        .iter()
        .any(|message| message.account_id != account_id || message.folder_id != folder_id)
    {
        return Err(AppError::BadRequest(
            "bulk mail mutations must target messages from one folder".into(),
        ));
    }

    messages.sort_by_key(|message| message.uid);
    let account = find_account(&state, user.id, account_id).await?;
    let folder = find_folder(&state, user.id, account_id, folder_id).await?;
    let password = resolve_mail_password(&state, &account, None)?;
    let uids = messages
        .iter()
        .map(|message| message.uid)
        .collect::<Vec<_>>();

    match req.action {
        MailMessageMutationAction::MarkRead
        | MailMessageMutationAction::MarkUnread
        | MailMessageMutationAction::Star
        | MailMessageMutationAction::Unstar => {
            let (flag, enabled) = match req.action {
                MailMessageMutationAction::MarkRead => (RemoteMessageFlag::Seen, true),
                MailMessageMutationAction::MarkUnread => (RemoteMessageFlag::Seen, false),
                MailMessageMutationAction::Star => (RemoteMessageFlag::Flagged, true),
                MailMessageMutationAction::Unstar => (RemoteMessageFlag::Flagged, false),
                _ => unreachable!(),
            };
            let flags = state
                .mail
                .set_imap_messages_flag(
                    &account.imap,
                    &password,
                    &folder.path,
                    &uids,
                    flag,
                    enabled,
                )
                .await?;
            let updated =
                update_cached_messages_flags(&state, user.id, &folder, &messages, flags).await?;
            let updated_ids = updated
                .iter()
                .map(|message| message.id)
                .collect::<HashSet<_>>();
            for message in &messages {
                if !updated_ids.contains(&message.id) {
                    errors.push(MailMessageBulkMutationError {
                        message_id: message.id.to_hex(),
                        error: "Mail provider did not return updated flags".to_string(),
                    });
                }
            }
            let failed = errors.len();
            Ok(Json(MailMessageBulkMutationResponse {
                requested,
                succeeded: updated.len(),
                failed,
                messages: updated.iter().map(message_to_response).collect(),
                removed_message_ids: Vec::new(),
                destination_folder_id: None,
                destination_folder_path: None,
                errors,
            }))
        }
        MailMessageMutationAction::Move
        | MailMessageMutationAction::Archive
        | MailMessageMutationAction::Trash => {
            let destination_req = MailMessageMutationRequest {
                action: req.action,
                target_folder_id: req.target_folder_id.clone(),
            };
            let destination =
                mutation_destination_folder(&state, user.id, account.id, &destination_req).await?;
            if destination.id == folder.id {
                let failed = errors.len();
                return Ok(Json(MailMessageBulkMutationResponse {
                    requested,
                    succeeded: messages.len(),
                    failed,
                    messages: messages.iter().map(message_to_response).collect(),
                    removed_message_ids: Vec::new(),
                    destination_folder_id: Some(destination.id.to_hex()),
                    destination_folder_path: Some(destination.path),
                    errors,
                }));
            }

            state
                .mail
                .move_imap_messages(
                    &account.imap,
                    &password,
                    &folder.path,
                    &uids,
                    &destination.path,
                )
                .await?;
            remove_cached_messages_after_move(&state, user.id, &folder, &destination, &messages)
                .await?;
            let failed = errors.len();
            Ok(Json(MailMessageBulkMutationResponse {
                requested,
                succeeded: messages.len(),
                failed,
                messages: Vec::new(),
                removed_message_ids: messages.iter().map(|message| message.id.to_hex()).collect(),
                destination_folder_id: Some(destination.id.to_hex()),
                destination_folder_path: Some(destination.path),
                errors,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_folder(
        path: &str,
        role_source: MailFolderRoleSource,
        role: Option<MailFolderRole>,
    ) -> MailFolder {
        let now = Utc::now();
        MailFolder {
            id: ObjectId::new(),
            owner_id: ObjectId::new(),
            account_id: ObjectId::new(),
            path: path.to_string(),
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            delimiter: Some("/".to_string()),
            parent_path: None,
            role,
            role_source,
            selectable: true,
            sync_enabled: true,
            attributes: Vec::new(),
            uid_validity: None,
            uid_next: None,
            exists: None,
            unseen: None,
            highest_synced_uid: None,
            lowest_synced_uid: None,
            last_sync_started_at: None,
            last_sync_finished_at: None,
            last_sync_error: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn test_attachment(content_id: Option<&str>) -> MailAttachment {
        MailAttachment {
            id: ObjectId::new(),
            owner_id: ObjectId::new(),
            account_id: ObjectId::new(),
            message_id: ObjectId::new(),
            filename: Some("logo.png".to_string()),
            content_type: Some("image/png".to_string()),
            content_id: content_id.map(str::to_string),
            disposition: Some("inline".to_string()),
            size_bytes: Some(42),
            storage_id: Some(ObjectId::new()),
            storage_path: Some("mail/logo.png".to_string()),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn infer_folder_role_prefers_inbox_exact_path() {
        assert_eq!(
            infer_folder_role("INBOX", "INBOX", &[]),
            Some(MailFolderRole::Inbox)
        );
        assert_eq!(infer_folder_role("INBOX/Travel", "Travel", &[]), None);
    }

    #[test]
    fn mail_body_html_response_rewrites_cid_images_to_attachment_urls() {
        let attachment = test_attachment(Some("<logo@example.com>"));
        let attachment_id = attachment.id.to_hex();
        let html = mail_body_html_for_response(
            Some(
                "<p><img src=\"cid:logo%40example.com\" alt=\"logo\"><img src=\"cid:missing\" alt=\"missing\"></p>"
                    .to_string(),
            ),
            &[attachment],
        )
        .unwrap();

        assert!(html.contains(&format!(
            "src=\"/api/mail/attachments/{attachment_id}/open\""
        )));
        assert!(!html.contains("cid:logo"));
        assert!(!html.contains("cid:missing"));
    }

    #[test]
    fn mail_flag_eq_ignores_case_and_leading_slash() {
        assert!(mail_flag_eq("\\Seen", "seen"));
        assert!(mail_flag_eq("flagged", "\\Flagged"));
        assert!(!mail_flag_eq("\\Answered", "\\Seen"));
    }

    #[test]
    fn validate_send_request_requires_recipient() {
        let req = SendMailMessageRequest {
            identity_id: None,
            draft_id: None,
            to: Vec::new(),
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: "Test".to_string(),
            body_text: "Body".to_string(),
            body_html: None,
            in_reply_to: None,
            references: Vec::new(),
            attachment_ids: Vec::new(),
        };

        assert!(validate_send_request(&req).is_err());
    }

    #[test]
    fn message_id_for_sender_uses_sender_domain() {
        let value = message_id_for_sender("sender@example.com");

        assert!(value.starts_with('<'));
        assert!(value.ends_with("@example.com>"));
    }

    #[test]
    fn infer_folder_role_uses_common_special_names() {
        assert_eq!(
            infer_folder_role("[Gmail]/Sent Mail", "Sent Mail", &[]),
            Some(MailFolderRole::Sent)
        );
        assert_eq!(
            infer_folder_role("[Gmail]/All Mail", "All Mail", &[]),
            Some(MailFolderRole::AllMail)
        );
        assert_eq!(
            infer_folder_role("Deleted Items", "Deleted Items", &[]),
            Some(MailFolderRole::Trash)
        );
    }

    #[test]
    fn infer_folder_role_uses_special_use_attributes() {
        assert_eq!(
            infer_folder_role("Provider/Whatever", "Whatever", &["Sent".to_string()]),
            Some(MailFolderRole::Sent)
        );
        assert_eq!(
            infer_folder_role("Provider/Whatever", "Whatever", &["Junk".to_string()]),
            Some(MailFolderRole::Spam)
        );
    }

    #[test]
    fn folder_response_applies_inferred_role_for_cached_folders() {
        let folder = test_folder("INBOX", MailFolderRoleSource::Inferred, None);

        assert_eq!(
            folder_to_response(&folder, false).role,
            Some(MailFolderRole::Inbox)
        );
    }

    #[test]
    fn folder_response_preserves_user_cleared_role() {
        let folder = test_folder("INBOX", MailFolderRoleSource::User, None);

        assert_eq!(folder_to_response(&folder, false).role, None);
    }

    #[test]
    fn folder_response_marks_completed_uid_window() {
        let mut folder = test_folder("INBOX", MailFolderRoleSource::Inferred, None);
        folder.uid_next = Some(12);
        folder.lowest_synced_uid = Some(1);
        folder.highest_synced_uid = Some(11);

        assert!(folder_to_response(&folder, false).sync_completed);
    }

    #[test]
    fn folder_response_marks_empty_folder_completed() {
        let mut folder = test_folder("INBOX", MailFolderRoleSource::Inferred, None);
        folder.uid_next = Some(500);
        folder.exists = Some(0);
        folder.lowest_synced_uid = None;
        folder.highest_synced_uid = None;

        assert!(folder_to_response(&folder, false).sync_completed);
    }

    #[test]
    fn provider_role_diagnostics_report_inferred_folders() {
        let folders = vec![
            test_folder("INBOX", MailFolderRoleSource::Inferred, None),
            test_folder("Sent Mail", MailFolderRoleSource::Inferred, None),
        ];

        let rows = role_diagnostics(&folders);
        let sent = rows
            .iter()
            .find(|row| row.role == MailFolderRole::Sent)
            .unwrap();
        let archive = rows
            .iter()
            .find(|row| row.role == MailFolderRole::Archive)
            .unwrap();

        assert_eq!(sent.status, MailProviderRoleStatus::Found);
        assert_eq!(sent.folder_path.as_deref(), Some("Sent Mail"));
        assert_eq!(archive.status, MailProviderRoleStatus::Missing);
        assert!(archive.folder_path.is_none());
    }

    #[test]
    fn sent_copy_diagnostics_require_sent_folder() {
        let folders = vec![test_folder(
            "Sent Mail",
            MailFolderRoleSource::Inferred,
            None,
        )];

        let ready = sent_copy_diagnostics(&folders);
        assert_eq!(ready.status, MailSentCopyDiagnosticStatus::Ready);
        assert!(ready.provider_saved_detection);
        assert!(ready.append_fallback);

        let missing = sent_copy_diagnostics(&[]);
        assert_eq!(
            missing.status,
            MailSentCopyDiagnosticStatus::MissingSentFolder
        );
        assert!(!missing.provider_saved_detection);
        assert!(!missing.append_fallback);
    }

    #[test]
    fn message_list_cursor_accepts_uid_only() {
        assert_eq!(parse_message_list_cursor(Some("42")).unwrap(), Some(42));
        assert_eq!(parse_message_list_cursor(Some("")).unwrap(), None);
        assert!(parse_message_list_cursor(Some("not-a-uid")).is_err());
    }
}
