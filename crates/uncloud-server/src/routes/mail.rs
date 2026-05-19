use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use bson::doc;
use chrono::Utc;
use futures::TryStreamExt;
use mongodb::bson::{self, oid::ObjectId};
use uncloud_common::{
    CreateMailAccountRequest, CreateMailIdentityRequest, MailAccountResponse,
    MailConnectionTestResponse, MailCredentialStatusResponse, MailFolderResponse,
    MailIdentityResponse, MailPasswordAuthRequest, MailServerSettings, SetMailCredentialRequest,
    UpdateMailAccountRequest, UpdateMailIdentityRequest,
};

use crate::error::{AppError, Result};
use crate::middleware::AuthUser;
use crate::models::{MailAccount, MailFolder, MailIdentity, MailServerConfig};
use crate::services::SecretCipher;
use crate::AppState;

const ACCOUNTS: &str = "mail_accounts";
const IDENTITIES: &str = "mail_identities";
const FOLDERS: &str = "mail_folders";
const MESSAGES: &str = "mail_messages";
const ATTACHMENTS: &str = "mail_attachments";

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

fn folder_to_response(folder: &MailFolder) -> MailFolderResponse {
    MailFolderResponse {
        id: folder.id.to_hex(),
        account_id: folder.account_id.to_hex(),
        path: folder.path.clone(),
        name: folder.name.clone(),
        delimiter: folder.delimiter.clone(),
        parent_path: folder.parent_path.clone(),
        role: folder.role.clone(),
        selectable: folder.selectable,
        attributes: folder.attributes.clone(),
        uid_validity: folder.uid_validity,
        uid_next: folder.uid_next,
        exists: folder.exists,
        unseen: folder.unseen,
        created_at: folder.created_at.to_rfc3339(),
        updated_at: folder.updated_at.to_rfc3339(),
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

fn credential_status(account: &MailAccount) -> MailCredentialStatusResponse {
    MailCredentialStatusResponse {
        account_id: account.id.to_hex(),
        credential_configured: account.credential_configured || account.credential.is_some(),
    }
}

fn resolve_imap_password(
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
    let password = resolve_imap_password(&state, &account, req.password.as_deref())?;
    Ok(Json(
        state
            .mail
            .test_imap_password(&account.imap, &password)
            .await?,
    ))
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

pub async fn refresh_folders(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(account_id): Path<String>,
    Json(req): Json<MailPasswordAuthRequest>,
) -> Result<Json<Vec<MailFolderResponse>>> {
    require_mail(&state)?;
    let account_id = parse_oid(&account_id, "mail account id")?;
    let account = find_account(&state, user.id, account_id).await?;
    let password = resolve_imap_password(&state, &account, req.password.as_deref())?;
    let remote = state
        .mail
        .list_imap_mailboxes(&account.imap, &password)
        .await?;

    let coll = state.db.collection::<MailFolder>(FOLDERS);
    coll.delete_many(doc! { "owner_id": user.id, "account_id": account_id })
        .await?;

    let now = Utc::now();
    let folders: Vec<MailFolder> = remote
        .into_iter()
        .map(|folder| MailFolder {
            id: ObjectId::new(),
            owner_id: user.id,
            account_id,
            path: folder.path,
            name: folder.name,
            delimiter: folder.delimiter,
            parent_path: folder.parent_path,
            role: None,
            selectable: folder.selectable,
            attributes: folder.attributes,
            uid_validity: None,
            uid_next: None,
            exists: None,
            unseen: None,
            created_at: now,
            updated_at: now,
        })
        .collect();
    if !folders.is_empty() {
        coll.insert_many(&folders).await?;
    }
    Ok(Json(folders.iter().map(folder_to_response).collect()))
}
