use std::{collections::HashMap, sync::Arc, time::Duration};

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::Response,
    Json,
};
use bson::doc;
use chrono::Utc;
use futures::TryStreamExt;
use mongodb::bson::{self, oid::ObjectId};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::time::sleep;
use tokio_util::io::ReaderStream;
use uncloud_common::{
    CreateMailAccountRequest, CreateMailIdentityRequest, MailAccountResponse,
    MailAccountSyncResponse, MailAddressDto, MailAttachmentResponse, MailConnectionTestResponse,
    MailCredentialStatusResponse, MailFolderResponse, MailFolderRole, MailFolderRoleSource,
    MailFolderSyncResponse, MailIdentityResponse, MailMessageDetailResponse,
    MailMessageListResponse, MailMessageMutationAction, MailMessageMutationRequest,
    MailMessageMutationResponse, MailMessageSummaryResponse, MailPasswordAuthRequest,
    MailSentCopyStatus, MailServerSettings, MailSyncRequest, SaveMailAttachmentRequest,
    SaveMailAttachmentResponse, SendMailMessageRequest, SendMailMessageResponse,
    SetMailCredentialRequest, UpdateMailAccountRequest, UpdateMailFolderRequest,
    UpdateMailIdentityRequest,
};

use crate::error::{AppError, Result};
use crate::middleware::{AuthUser, RequestMeta};
use crate::models::{
    File, Folder, MailAccount, MailAddress, MailAttachment, MailFolder, MailIdentity, MailMessage,
    MailServerConfig,
    User,
};
use crate::routes::apps::{deliver_webhooks, EVENT_FILE_CREATED};
use crate::routes::files::{check_name_conflict, file_to_response, resolve_storage_path};
use crate::services::mail::{
    RemoteMailAddress, RemoteMailbox, RemoteMessageBody, RemoteMessageFlag, RemoteMessageSummary,
    RemoteOutgoingMessage,
};
use crate::services::mail_blob::{read_cached_message_body, store_message_body, StoredMailBody};
use crate::services::sharing::check_folder_access;
use crate::services::SecretCipher;
use crate::AppState;

const ACCOUNTS: &str = "mail_accounts";
const IDENTITIES: &str = "mail_identities";
const FOLDERS: &str = "mail_folders";
const MESSAGES: &str = "mail_messages";
const ATTACHMENTS: &str = "mail_attachments";
const DEFAULT_SYNC_LIMIT_PER_FOLDER: u32 = 250;
const MAX_SYNC_LIMIT_PER_FOLDER: u32 = 1_000;
const DEFAULT_MESSAGE_LIST_LIMIT: i64 = 100;
const MAX_MESSAGE_LIST_LIMIT: i64 = 500;
const SENT_COPY_DETECT_ATTEMPTS: usize = 3;
const SENT_COPY_DETECT_DELAY: Duration = Duration::from_millis(750);

#[derive(Debug, Deserialize)]
pub struct MailMessageListQuery {
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    cursor: Option<String>,
}

fn require_mail(state: &AppState) -> Result<()> {
    if !state.config.features.mail {
        return Err(AppError::Forbidden("Mail feature disabled".into()));
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

fn account_to_response(account: &MailAccount) -> MailAccountResponse {
    MailAccountResponse {
        id: account.id.to_hex(),
        display_name: account.display_name.clone(),
        email_address: account.email_address.clone(),
        imap: server_to_response(&account.imap),
        smtp: server_to_response(&account.smtp),
        enabled: account.enabled,
        sync_enabled: account.sync_enabled,
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

fn folder_effective_role(folder: &MailFolder) -> Option<MailFolderRole> {
    if folder.role_source == MailFolderRoleSource::User {
        folder.role
    } else {
        infer_folder_role(&folder.path, &folder.name, &folder.attributes)
    }
}

fn folder_sync_completed(folder: &MailFolder) -> bool {
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

fn folder_to_response(folder: &MailFolder) -> MailFolderResponse {
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
        name: address.name.clone(),
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

fn message_to_response(message: &MailMessage) -> MailMessageSummaryResponse {
    MailMessageSummaryResponse {
        id: message.id.to_hex(),
        account_id: message.account_id.to_hex(),
        folder_id: message.folder_id.to_hex(),
        folder_path: message.folder_path.clone(),
        uid: message.uid,
        message_id: message.message_id.clone(),
        thread_id: message.thread_id.clone(),
        subject: message.subject.clone(),
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

fn sync_limit(input: Option<u32>) -> u32 {
    input
        .unwrap_or(DEFAULT_SYNC_LIMIT_PER_FOLDER)
        .clamp(1, MAX_SYNC_LIMIT_PER_FOLDER)
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
    require_mail(&state)?;
    let mut cursor = state
        .db
        .collection::<MailAccount>(ACCOUNTS)
        .find(doc! { "owner_id": user.id })
        .sort(doc! { "email_address": 1 })
        .await?;
    let mut out = Vec::new();
    while let Some(account) = cursor.try_next().await? {
        out.push(account_to_response(&account));
    }
    Ok(Json(out))
}

pub async fn create_account(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(req): Json<CreateMailAccountRequest>,
) -> Result<(StatusCode, Json<MailAccountResponse>)> {
    require_mail(&state)?;
    let now = Utc::now();
    let account = MailAccount {
        id: ObjectId::new(),
        owner_id: user.id,
        display_name: validate_label(&req.display_name, "display name")?,
        email_address: validate_email(&req.email_address, "email address")?,
        imap: validate_server(req.imap)?,
        smtp: validate_server(req.smtp)?,
        enabled: req.enabled,
        sync_enabled: req.sync_enabled,
        credential_configured: false,
        credential: None,
        mail_storage_id: None,
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
    Ok((StatusCode::CREATED, Json(account_to_response(&account))))
}

pub async fn update_account(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<UpdateMailAccountRequest>,
) -> Result<Json<MailAccountResponse>> {
    require_mail(&state)?;
    let id = parse_oid(&id, "mail account id")?;
    let _ = find_account(&state, user.id, id).await?;

    let mut set = doc! { "updated_at": bson::DateTime::from_chrono(Utc::now()) };
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
    if let Some(value) = req.enabled {
        set.insert("enabled", value);
    }
    if let Some(value) = req.sync_enabled {
        set.insert("sync_enabled", value);
    }

    state
        .db
        .collection::<MailAccount>(ACCOUNTS)
        .update_one(
            doc! { "_id": id, "owner_id": user.id },
            doc! { "$set": set },
        )
        .await?;

    Ok(Json(account_to_response(
        &find_account(&state, user.id, id).await?,
    )))
}

pub async fn delete_account(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    require_mail(&state)?;
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
    require_mail(&state)?;
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
    require_mail(&state)?;
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
    require_mail(&state)?;
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
    require_mail(&state)?;
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
    require_mail(&state)?;
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

pub async fn send_account_message(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<SendMailMessageRequest>,
) -> Result<Json<SendMailMessageResponse>> {
    require_mail(&state)?;
    validate_send_request(&req)?;
    let id = parse_oid(&id, "mail account id")?;
    let account = find_account(&state, user.id, id).await?;
    let password = resolve_mail_password(&state, &account, None)?;
    let identity =
        resolve_send_identity(&state, user.id, &account, req.identity_id.as_deref()).await?;
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
    };
    let sent = state
        .mail
        .send_smtp_plain_text(&account.smtp, &password, remote)
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
    require_mail(&state)?;
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
    require_mail(&state)?;
    let account_id = parse_oid(&req.account_id, "mail account id")?;
    let _ = find_account(&state, user.id, account_id).await?;
    if req.is_default {
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
        is_default: req.is_default,
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
    require_mail(&state)?;
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
        .await?;
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
    require_mail(&state)?;
    let id = parse_oid(&id, "mail identity id")?;
    let result = state
        .db
        .collection::<MailIdentity>(IDENTITIES)
        .delete_one(doc! { "_id": id, "owner_id": user.id })
        .await?;
    if result.deleted_count == 0 {
        return Err(AppError::NotFound("Mail identity".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_folders(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(account_id): Path<String>,
) -> Result<Json<Vec<MailFolderResponse>>> {
    require_mail(&state)?;
    let account_id = parse_oid(&account_id, "mail account id")?;
    let _ = find_account(&state, user.id, account_id).await?;
    let mut cursor = state
        .db
        .collection::<MailFolder>(FOLDERS)
        .find(doc! { "owner_id": user.id, "account_id": account_id })
        .sort(doc! { "path": 1 })
        .await?;
    let mut out = Vec::new();
    while let Some(folder) = cursor.try_next().await? {
        out.push(folder_to_response(&folder));
    }
    Ok(Json(out))
}

pub async fn update_folder(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path((account_id, folder_id)): Path<(String, String)>,
    Json(req): Json<UpdateMailFolderRequest>,
) -> Result<Json<MailFolderResponse>> {
    require_mail(&state)?;
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
        return Ok(Json(folder_to_response(&folder)));
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
    Ok(Json(folder_to_response(&updated)))
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
    require_mail(&state)?;
    let account_id = parse_oid(&account_id, "mail account id")?;
    let account = find_account(&state, user.id, account_id).await?;
    let password = resolve_mail_password(&state, &account, req.password.as_deref())?;
    let remote = state
        .mail
        .list_imap_mailboxes(&account.imap, &password)
        .await?;

    let folders = upsert_remote_folders(&state, user.id, account_id, remote).await?;
    Ok(Json(folders.iter().map(folder_to_response).collect()))
}

async fn store_message_summary(
    state: &AppState,
    owner_id: ObjectId,
    account_id: ObjectId,
    folder: &MailFolder,
    summary: &RemoteMessageSummary,
    now: chrono::DateTime<Utc>,
) -> Result<()> {
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
    state
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
    Ok(())
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

    if remote.uid_validity_changed {
        state
            .db
            .collection::<MailMessage>(MESSAGES)
            .delete_many(doc! {
                "owner_id": owner_id,
                "account_id": account.id,
                "folder_id": folder.id,
            })
            .await?;
    }

    let now = Utc::now();
    for message in &remote.messages {
        store_message_summary(state, owner_id, account.id, folder, message, now).await?;
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

pub async fn sync_folder(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path((account_id, folder_id)): Path<(String, String)>,
    Json(req): Json<MailSyncRequest>,
) -> Result<Json<MailFolderSyncResponse>> {
    require_mail(&state)?;
    let account_id = parse_oid(&account_id, "mail account id")?;
    let folder_id = parse_oid(&folder_id, "mail folder id")?;
    let account = find_account(&state, user.id, account_id).await?;
    let folder = find_folder(&state, user.id, account_id, folder_id).await?;
    let password = resolve_mail_password(&state, &account, req.password.as_deref())?;
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
    require_mail(&state)?;
    let account_id = parse_oid(&account_id, "mail account id")?;
    let account = find_account(&state, user.id, account_id).await?;
    let password = resolve_mail_password(&state, &account, req.password.as_deref())?;
    let limit = sync_limit(req.limit_per_folder);

    let remote = state
        .mail
        .list_imap_mailboxes(&account.imap, &password)
        .await?;
    let folders = upsert_remote_folders(&state, user.id, account_id, remote).await?;

    let mut folder_results = Vec::new();
    for folder in folders
        .iter()
        .filter(|folder| folder.selectable && folder.sync_enabled)
    {
        match sync_one_folder(&state, user.id, &account, folder, &password, limit).await {
            Ok(result) => folder_results.push(result),
            Err(err) => folder_results.push(MailFolderSyncResponse {
                account_id: account.id.to_hex(),
                folder_id: folder.id.to_hex(),
                folder_path: folder.path.clone(),
                fetched_messages: 0,
                stored_messages: 0,
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
    let errors = folder_results
        .iter()
        .filter(|folder| folder.error.is_some())
        .count();
    if errors == 0 {
        state
            .db
            .collection::<MailAccount>(ACCOUNTS)
            .update_one(
                doc! { "_id": account.id, "owner_id": user.id },
                doc! {
                    "$set": {
                        "last_sync_at": bson::DateTime::from_chrono(Utc::now()),
                        "updated_at": bson::DateTime::from_chrono(Utc::now()),
                    }
                },
            )
            .await?;
    }

    Ok(Json(MailAccountSyncResponse {
        account_id: account.id.to_hex(),
        fetched_messages,
        stored_messages,
        errors,
        folders: folder_results,
    }))
}

pub async fn list_messages(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path((account_id, folder_id)): Path<(String, String)>,
    Query(query): Query<MailMessageListQuery>,
) -> Result<Json<MailMessageListResponse>> {
    require_mail(&state)?;
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
    require_mail(&state)?;
    let message_id = parse_oid(&message_id, "mail message id")?;
    let message = find_message(&state, user.id, message_id).await?;
    match read_cached_message_body(&state.storage, &message).await {
        Ok(Some(body)) => {
            let attachments = list_message_attachments(&state, user.id, message.id).await?;
            return Ok(Json(MailMessageDetailResponse {
                message: message_to_response(&message),
                body_text: body.text,
                body_html: body.html,
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
        body_html: body.and_then(|body| body.html),
        attachments: attachments.iter().map(attachment_to_response).collect(),
    }))
}

pub async fn download_attachment(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(attachment_id): Path<String>,
) -> Result<Response> {
    require_mail(&state)?;
    stream_attachment_response(&state, user.id, &attachment_id, "attachment").await
}

pub async fn open_attachment(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(attachment_id): Path<String>,
) -> Result<Response> {
    require_mail(&state)?;
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
    let content_disposition = format!("{disposition}; filename=\"{}\"", filename.replace('"', "\\\""));
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
    require_mail(&state)?;
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
        if let Some(owner) = users_coll.find_one(doc! { "_id": effective_owner_id }).await? {
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

    state.auth.update_user_bytes(effective_owner_id, size).await?;
    state.events.emit_file_created(effective_owner_id, &file).await;
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

pub async fn mutate_message(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(message_id): Path<String>,
    Json(req): Json<MailMessageMutationRequest>,
) -> Result<Json<MailMessageMutationResponse>> {
    require_mail(&state)?;
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
                return Err(AppError::BadRequest(
                    "message is already in the target folder".into(),
                ));
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

    #[test]
    fn infer_folder_role_prefers_inbox_exact_path() {
        assert_eq!(
            infer_folder_role("INBOX", "INBOX", &[]),
            Some(MailFolderRole::Inbox)
        );
        assert_eq!(infer_folder_role("INBOX/Travel", "Travel", &[]), None);
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
            to: Vec::new(),
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: "Test".to_string(),
            body_text: "Body".to_string(),
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
            folder_to_response(&folder).role,
            Some(MailFolderRole::Inbox)
        );
    }

    #[test]
    fn folder_response_preserves_user_cleared_role() {
        let folder = test_folder("INBOX", MailFolderRoleSource::User, None);

        assert_eq!(folder_to_response(&folder).role, None);
    }

    #[test]
    fn folder_response_marks_completed_uid_window() {
        let mut folder = test_folder("INBOX", MailFolderRoleSource::Inferred, None);
        folder.uid_next = Some(12);
        folder.lowest_synced_uid = Some(1);
        folder.highest_synced_uid = Some(11);

        assert!(folder_to_response(&folder).sync_completed);
    }

    #[test]
    fn message_list_cursor_accepts_uid_only() {
        assert_eq!(parse_message_list_cursor(Some("42")).unwrap(), Some(42));
        assert_eq!(parse_message_list_cursor(Some("")).unwrap(), None);
        assert!(parse_message_list_cursor(Some("not-a-uid")).is_err());
    }
}
