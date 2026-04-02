use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::error::{AppError, Result};
use crate::middleware::AuthUser;
use crate::models::{Storage, StorageBackendConfig, StorageBackendType};
use crate::AppState;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum CreateStorageConfig {
    Local { path: String },
    S3 {
        endpoint: String,
        bucket: String,
        access_key: String,
        secret_key: String,
        region: Option<String>,
    },
}

impl From<CreateStorageConfig> for StorageBackendConfig {
    fn from(c: CreateStorageConfig) -> Self {
        match c {
            CreateStorageConfig::Local { path } => StorageBackendConfig::Local { path },
            CreateStorageConfig::S3 {
                endpoint,
                bucket,
                access_key,
                secret_key,
                region,
            } => StorageBackendConfig::S3 {
                endpoint,
                bucket,
                access_key,
                secret_key,
                region,
            },
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateStorageRequest {
    pub name: String,
    pub config: CreateStorageConfig,
    pub is_default: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateStorageRequest {
    pub name: Option<String>,
    pub is_default: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct StorageResponse {
    pub id: String,
    pub name: String,
    pub backend_type: StorageBackendType,
    pub is_default: bool,
    pub created_at: String,
}

impl From<&Storage> for StorageResponse {
    fn from(s: &Storage) -> Self {
        Self {
            id: s.id.to_hex(),
            name: s.name.clone(),
            backend_type: s.backend_type,
            is_default: s.is_default,
            created_at: s.created_at.to_rfc3339(),
        }
    }
}

pub async fn list_storages(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<StorageResponse>>> {
    let storages = state.storage.list_storages().await?;
    Ok(Json(storages.iter().map(StorageResponse::from).collect()))
}

pub async fn create_storage(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(req): Json<CreateStorageRequest>,
) -> Result<(StatusCode, Json<StorageResponse>)> {
    if req.name.is_empty() || req.name.len() > 100 {
        return Err(AppError::Validation(
            "Storage name must be between 1 and 100 characters".to_string(),
        ));
    }

    let storage = state
        .storage
        .create_storage(
            req.name,
            req.config.into(),
            req.is_default.unwrap_or(false),
            user.id,
        )
        .await?;

    Ok((StatusCode::CREATED, Json(StorageResponse::from(&storage))))
}

pub async fn update_storage(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateStorageRequest>,
) -> Result<Json<StorageResponse>> {
    let storage_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid storage ID".to_string()))?;

    let mut storage = state.storage.get_storage(storage_id).await?;

    if let Some(name) = req.name {
        if name.is_empty() || name.len() > 100 {
            return Err(AppError::Validation(
                "Storage name must be between 1 and 100 characters".to_string(),
            ));
        }
        storage.name = name;
    }

    if let Some(is_default) = req.is_default {
        if is_default {
            // Unset other defaults
            let storages_collection = state.db.collection::<Storage>("storages");
            storages_collection
                .update_many(
                    mongodb::bson::doc! {},
                    mongodb::bson::doc! { "$set": { "is_default": false } },
                )
                .await?;
        }
        storage.is_default = is_default;
    }

    let storages_collection = state.db.collection::<Storage>("storages");
    storages_collection
        .replace_one(mongodb::bson::doc! { "_id": storage_id }, &storage)
        .await?;

    Ok(Json(StorageResponse::from(&storage)))
}

pub async fn delete_storage(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let storage_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid storage ID".to_string()))?;

    state.storage.delete_storage(storage_id).await?;

    Ok(StatusCode::NO_CONTENT)
}
