use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use mongodb::bson::{doc, oid::ObjectId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::AsyncReadExt;

use crate::error::{AppError, Result};
use crate::middleware::AuthUser;
use crate::models::{File, Folder, Storage, StorageBackendConfig, StorageBackendType, User};
use crate::routes::files::sanitize_path_component;
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

// ── Rescan ──────────────────────────────────────────────────────────────────
//
// Admin-triggered scan of a storage backend. Walks the on-disk tree under each
// user's root (`{username}/`) and imports any folder / file that exists on disk
// but has no matching DB record — mirroring the homelab workflow of "I rsynced
// a pile of files in, now please ingest them." Existing records are left
// alone; size / mtime mismatches are reported as conflicts, not silently
// overwritten.

#[derive(Debug, Serialize)]
pub struct RescanResponse {
    pub scanned_entries: usize,
    pub imported_folders: usize,
    pub imported_files: usize,
    pub skipped_existing: usize,
    pub conflicts: Vec<RescanConflict>,
}

#[derive(Debug, Serialize)]
pub struct RescanConflict {
    pub path: String,
    pub reason: String,
}

pub async fn rescan_storage(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<RescanResponse>> {
    let storage_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid storage ID".to_string()))?;

    // Sanity-check the storage + backend exists before we scan
    state.storage.get_storage(storage_id).await?;
    let backend = state.storage.get_backend(storage_id).await?;

    let users_coll = state.db.collection::<User>("users");
    let mut cursor = users_coll.find(doc! {}).await?;

    let mut scanned_entries = 0usize;
    let mut imported_folders = 0usize;
    let mut imported_files = 0usize;
    let mut skipped_existing = 0usize;
    let mut conflicts: Vec<RescanConflict> = Vec::new();
    let mut owner_bytes_delta: HashMap<ObjectId, i64> = HashMap::new();

    while cursor.advance().await? {
        let user: User = cursor.deserialize_current()?;
        let user_prefix = sanitize_path_component(&user.username);
        let entries = backend.scan(&user_prefix).await?;

        // Sort shallow-first so parent folders are created before their children.
        let mut entries = entries;
        entries.sort_by_key(|e| e.path.matches('/').count());

        // Map storage_path -> folder_id for folders encountered in this user's tree
        // (seeded with existing folders from the DB).
        let mut folder_by_storage_path: HashMap<String, ObjectId> = HashMap::new();
        preload_user_folders(&state, user.id, &user_prefix, &mut folder_by_storage_path).await?;

        for entry in entries {
            scanned_entries += 1;

            // Skip Uncloud-internal dirs and the upload tempdir.
            let rel_within_user = entry
                .path
                .strip_prefix(&format!("{}/", user_prefix))
                .unwrap_or(&entry.path);
            let first = rel_within_user.split('/').next().unwrap_or("");
            if matches!(first, ".uncloud" | ".thumbs" | ".tmp") {
                continue;
            }
            // Skip the user-root entry itself.
            if entry.path == user_prefix {
                continue;
            }

            if entry.is_dir {
                let (parent_storage_path, name) = split_parent(&entry.path);
                let parent_id = if parent_storage_path == user_prefix {
                    None
                } else {
                    match folder_by_storage_path.get(&parent_storage_path) {
                        Some(id) => Some(*id),
                        None => {
                            conflicts.push(RescanConflict {
                                path: entry.path.clone(),
                                reason: "parent folder not yet imported".into(),
                            });
                            continue;
                        }
                    }
                };

                // Is there already a live folder with this name under that parent?
                if let Some(existing) =
                    find_live_folder(&state, user.id, parent_id, &name).await?
                {
                    folder_by_storage_path.insert(entry.path.clone(), existing.id);
                    skipped_existing += 1;
                    continue;
                }

                let folder = Folder::new(user.id, parent_id, name.clone());
                let folder_id = folder.id;
                state
                    .db
                    .collection::<Folder>("folders")
                    .insert_one(&folder)
                    .await?;
                folder_by_storage_path.insert(entry.path.clone(), folder_id);
                state.events.emit_folder_created(user.id, &folder).await;
                imported_folders += 1;
            } else {
                // File
                let (parent_storage_path, name) = split_parent(&entry.path);
                let parent_id = if parent_storage_path == user_prefix {
                    None
                } else {
                    match folder_by_storage_path.get(&parent_storage_path) {
                        Some(id) => Some(*id),
                        None => {
                            conflicts.push(RescanConflict {
                                path: entry.path.clone(),
                                reason: "parent folder not yet imported".into(),
                            });
                            continue;
                        }
                    }
                };

                // Already tracked under this logical parent?
                if let Some(existing) =
                    find_live_file(&state, user.id, parent_id, &name).await?
                {
                    if existing.size_bytes != entry.size_bytes as i64 {
                        conflicts.push(RescanConflict {
                            path: entry.path.clone(),
                            reason: format!(
                                "on-disk size {} differs from DB record {}",
                                entry.size_bytes, existing.size_bytes
                            ),
                        });
                    } else {
                        skipped_existing += 1;
                    }
                    continue;
                }

                // Import the file: hash it, then insert + enqueue processors.
                let checksum = match hash_file(&backend, &entry.path).await {
                    Ok(c) => c,
                    Err(e) => {
                        conflicts.push(RescanConflict {
                            path: entry.path.clone(),
                            reason: format!("failed to hash: {}", e),
                        });
                        continue;
                    }
                };

                let mime_type = mime_guess::from_path(&name)
                    .first_or_octet_stream()
                    .to_string();

                let file = File::new(
                    storage_id,
                    entry.path.clone(),
                    user.id,
                    parent_id,
                    name,
                    mime_type,
                    entry.size_bytes as i64,
                    checksum,
                );
                state
                    .db
                    .collection::<File>("files")
                    .insert_one(&file)
                    .await?;
                *owner_bytes_delta.entry(user.id).or_default() += entry.size_bytes as i64;

                state.events.emit_file_created(user.id, &file).await;
                state.processing.enqueue(&file, state.clone()).await;
                imported_files += 1;
            }
        }
    }

    for (owner_id, delta) in owner_bytes_delta {
        if delta > 0 {
            let _ = state.auth.update_user_bytes(owner_id, delta).await;
        }
    }

    Ok(Json(RescanResponse {
        scanned_entries,
        imported_folders,
        imported_files,
        skipped_existing,
        conflicts,
    }))
}

/// Seed the storage_path -> folder_id lookup with folders already in the DB,
/// so rescan doesn't re-import them. The storage path is reconstructed from
/// each folder's ancestry (mirroring `resolve_storage_path`).
async fn preload_user_folders(
    state: &AppState,
    owner_id: ObjectId,
    user_prefix: &str,
    map: &mut HashMap<String, ObjectId>,
) -> Result<()> {
    let folders_coll = state.db.collection::<Folder>("folders");
    let mut cursor = folders_coll
        .find(doc! { "owner_id": owner_id, "deleted_at": mongodb::bson::Bson::Null })
        .await?;
    let mut folders: Vec<Folder> = Vec::new();
    while cursor.advance().await? {
        folders.push(cursor.deserialize_current()?);
    }
    let by_id: HashMap<ObjectId, &Folder> = folders.iter().map(|f| (f.id, f)).collect();
    for f in &folders {
        let mut segments = vec![sanitize_path_component(&f.name)];
        let mut cur = f.parent_id;
        while let Some(pid) = cur {
            if let Some(parent) = by_id.get(&pid) {
                segments.push(sanitize_path_component(&parent.name));
                cur = parent.parent_id;
            } else {
                break;
            }
        }
        segments.reverse();
        let storage_path = format!("{}/{}", user_prefix, segments.join("/"));
        map.insert(storage_path, f.id);
    }
    Ok(())
}

async fn find_live_folder(
    state: &AppState,
    owner_id: ObjectId,
    parent_id: Option<ObjectId>,
    name: &str,
) -> Result<Option<Folder>> {
    let parent_bson = parent_id
        .map(mongodb::bson::Bson::ObjectId)
        .unwrap_or(mongodb::bson::Bson::Null);
    Ok(state
        .db
        .collection::<Folder>("folders")
        .find_one(doc! {
            "owner_id": owner_id,
            "parent_id": parent_bson,
            "name": name,
            "deleted_at": mongodb::bson::Bson::Null,
        })
        .await?)
}

async fn find_live_file(
    state: &AppState,
    owner_id: ObjectId,
    parent_id: Option<ObjectId>,
    name: &str,
) -> Result<Option<File>> {
    let parent_bson = parent_id
        .map(mongodb::bson::Bson::ObjectId)
        .unwrap_or(mongodb::bson::Bson::Null);
    Ok(state
        .db
        .collection::<File>("files")
        .find_one(doc! {
            "owner_id": owner_id,
            "parent_id": parent_bson,
            "name": name,
            "deleted_at": mongodb::bson::Bson::Null,
        })
        .await?)
}

fn split_parent(path: &str) -> (String, String) {
    match path.rsplit_once('/') {
        Some((parent, name)) => (parent.to_string(), name.to_string()),
        None => (String::new(), path.to_string()),
    }
}

async fn hash_file(
    backend: &std::sync::Arc<dyn crate::storage::StorageBackend>,
    path: &str,
) -> std::result::Result<String, String> {
    let mut reader = backend.read(path).await.map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}
