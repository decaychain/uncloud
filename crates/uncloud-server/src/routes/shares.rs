use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use mongodb::bson::{doc, oid::ObjectId};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio_util::io::ReaderStream;

use crate::error::{AppError, Result};
use crate::middleware::AuthUser;
use crate::models::{File, Folder, Share, ShareResourceType};
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct CreateShareRequest {
    pub resource_type: ShareResourceType,
    pub resource_id: String,
    pub password: Option<String>,
    pub expires_hours: Option<u64>,
    pub max_downloads: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct VerifyPasswordRequest {
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct ShareResponse {
    pub id: String,
    pub token: String,
    pub resource_type: ShareResourceType,
    pub resource_id: String,
    pub has_password: bool,
    pub expires_at: Option<String>,
    pub download_count: i64,
    pub max_downloads: Option<i64>,
    pub created_at: String,
}

impl From<&Share> for ShareResponse {
    fn from(s: &Share) -> Self {
        Self {
            id: s.id.to_hex(),
            token: s.token.clone(),
            resource_type: s.resource_type,
            resource_id: s.resource_id.to_hex(),
            has_password: s.password_hash.is_some(),
            expires_at: s.expires_at.map(|dt| dt.to_rfc3339()),
            download_count: s.download_count,
            max_downloads: s.max_downloads,
            created_at: s.created_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct PublicShareResponse {
    pub resource_type: ShareResourceType,
    pub name: String,
    pub size_bytes: Option<i64>,
    pub has_password: bool,
}

fn generate_share_token() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub async fn create_share(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(req): Json<CreateShareRequest>,
) -> Result<(StatusCode, Json<ShareResponse>)> {
    let resource_id = ObjectId::parse_str(&req.resource_id)
        .map_err(|_| AppError::BadRequest("Invalid resource ID".to_string()))?;

    // Verify resource exists and user owns it
    match req.resource_type {
        ShareResourceType::File => {
            let collection = state.db.collection::<File>("files");
            if collection
                .find_one(doc! { "_id": resource_id, "owner_id": user.id })
                .await?
                .is_none()
            {
                return Err(AppError::NotFound("File not found".to_string()));
            }
        }
        ShareResourceType::Folder => {
            let collection = state.db.collection::<Folder>("folders");
            if collection
                .find_one(doc! { "_id": resource_id, "owner_id": user.id })
                .await?
                .is_none()
            {
                return Err(AppError::NotFound("Folder not found".to_string()));
            }
        }
    }

    let token = generate_share_token();
    let mut share = Share::new(token, req.resource_type, resource_id, user.id);

    // Set password if provided
    if let Some(password) = &req.password {
        let hash = state.auth.hash_password(password)?;
        share.password_hash = Some(hash);
    }

    // Set expiration
    if let Some(hours) = req.expires_hours {
        share.expires_at = Some(chrono::Utc::now() + chrono::Duration::hours(hours as i64));
    }

    // Set max downloads
    share.max_downloads = req.max_downloads;

    let collection = state.db.collection::<Share>("shares");
    collection.insert_one(&share).await?;

    Ok((StatusCode::CREATED, Json(ShareResponse::from(&share))))
}

pub async fn list_shares(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<ShareResponse>>> {
    let collection = state.db.collection::<Share>("shares");
    let mut cursor = collection.find(doc! { "owner_id": user.id }).await?;

    let mut shares = Vec::new();
    while cursor.advance().await? {
        let share: Share = cursor.deserialize_current()?;
        shares.push(ShareResponse::from(&share));
    }

    Ok(Json(shares))
}

pub async fn delete_share(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let share_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid share ID".to_string()))?;

    let collection = state.db.collection::<Share>("shares");
    let result = collection
        .delete_one(doc! { "_id": share_id, "owner_id": user.id })
        .await?;

    if result.deleted_count == 0 {
        return Err(AppError::NotFound("Share not found".to_string()));
    }

    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_public_share(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Json<PublicShareResponse>> {
    let collection = state.db.collection::<Share>("shares");
    let share = collection
        .find_one(doc! { "token": &token })
        .await?
        .ok_or_else(|| AppError::NotFound("Share not found".to_string()))?;

    if !share.is_valid() {
        return Err(AppError::NotFound("Share expired or invalid".to_string()));
    }

    let (name, size_bytes) = match share.resource_type {
        ShareResourceType::File => {
            let files = state.db.collection::<File>("files");
            let file = files
                .find_one(doc! { "_id": share.resource_id })
                .await?
                .ok_or_else(|| AppError::NotFound("File not found".to_string()))?;
            (file.name, Some(file.size_bytes))
        }
        ShareResourceType::Folder => {
            let folders = state.db.collection::<Folder>("folders");
            let folder = folders
                .find_one(doc! { "_id": share.resource_id })
                .await?
                .ok_or_else(|| AppError::NotFound("Folder not found".to_string()))?;
            (folder.name, None)
        }
    };

    Ok(Json(PublicShareResponse {
        resource_type: share.resource_type,
        name,
        size_bytes,
        has_password: share.password_hash.is_some(),
    }))
}

pub async fn verify_share_password(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    Json(req): Json<VerifyPasswordRequest>,
) -> Result<StatusCode> {
    let collection = state.db.collection::<Share>("shares");
    let share = collection
        .find_one(doc! { "token": &token })
        .await?
        .ok_or_else(|| AppError::NotFound("Share not found".to_string()))?;

    if !share.is_valid() {
        return Err(AppError::NotFound("Share expired or invalid".to_string()));
    }

    match &share.password_hash {
        Some(hash) => {
            if state.auth.verify_password(&req.password, hash)? {
                Ok(StatusCode::OK)
            } else {
                Err(AppError::Unauthorized)
            }
        }
        None => Ok(StatusCode::OK), // No password required
    }
}

pub async fn download_public(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Response> {
    let collection = state.db.collection::<Share>("shares");
    let share = collection
        .find_one(doc! { "token": &token })
        .await?
        .ok_or_else(|| AppError::NotFound("Share not found".to_string()))?;

    if !share.is_valid() {
        return Err(AppError::NotFound("Share expired or invalid".to_string()));
    }

    // Only support file downloads for now
    if share.resource_type != ShareResourceType::File {
        return Err(AppError::BadRequest(
            "Folder downloads not yet supported".to_string(),
        ));
    }

    let files = state.db.collection::<File>("files");
    let file = files
        .find_one(doc! { "_id": share.resource_id })
        .await?
        .ok_or_else(|| AppError::NotFound("File not found".to_string()))?;

    // Increment download count
    collection
        .update_one(
            doc! { "token": &token },
            doc! { "$inc": { "download_count": 1 } },
        )
        .await?;

    let backend = state.storage.get_backend(file.storage_id).await?;
    let reader = backend.read(&file.storage_path).await?;
    let stream = ReaderStream::new(reader);
    let body = Body::from_stream(stream);

    let content_disposition = format!(
        "attachment; filename=\"{}\"",
        file.name.replace('"', "\\\"")
    );

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, &file.mime_type)
        .header(header::CONTENT_DISPOSITION, content_disposition)
        .header(header::CONTENT_LENGTH, file.size_bytes)
        .body(body)
        .unwrap())
}
