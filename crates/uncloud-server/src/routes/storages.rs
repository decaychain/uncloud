use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use chrono::Utc;
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
use crate::services::rescan::{RescanConflict, RescanJob, RescanJobHandle, RescanStatus};
use crate::storage::StorageBackend;
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
//
// A full rescan of a real library takes minutes to hours, well past any HTTP
// timeout, so the endpoint returns 202 with a job id and the actual work runs
// in a spawned task. Progress is visible via GET /admin/rescan-jobs/{id}.

/// Cap on how many conflicts we keep in the job handle — unbounded growth would
/// be a memory leak on badly-broken libraries.
const MAX_CONFLICTS: usize = 500;

/// How often the worker flushes counters into the shared job state. One write
/// per entry would serialise the worker on the RwLock for no benefit.
const COUNTER_FLUSH_EVERY: u64 = 32;

pub async fn rescan_storage(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<RescanJob>)> {
    let storage_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid storage ID".to_string()))?;

    // Sanity-check the storage + backend exists before we enqueue.
    state.storage.get_storage(storage_id).await?;
    let backend = state.storage.get_backend(storage_id).await?;

    let (_job_id, handle) = state
        .rescan
        .start_job(storage_id)
        .await
        .map_err(AppError::Conflict)?;

    let worker_state = state.clone();
    let worker_handle = handle.clone();
    tokio::spawn(async move {
        let outcome = run_rescan_worker(
            worker_state.clone(),
            storage_id,
            backend,
            worker_handle.clone(),
        )
        .await;

        let mut job = worker_handle.job.write().await;
        match outcome {
            Ok(()) if worker_handle.is_cancelled() => job.status = RescanStatus::Cancelled,
            Ok(()) => job.status = RescanStatus::Completed,
            Err(e) => {
                tracing::error!("Rescan job {} failed: {}", job.id, e);
                job.status = RescanStatus::Failed;
                job.error = Some(e);
            }
        }
        job.finished_at = Some(Utc::now());
        let final_snapshot = job.clone();
        drop(job);
        worker_state.events.emit_rescan_finished(&final_snapshot).await;
        worker_state.rescan.release(storage_id).await;
    });

    let snapshot = handle.job.read().await.clone();
    Ok((StatusCode::ACCEPTED, Json(snapshot)))
}

pub async fn get_rescan_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<RescanJob>> {
    let job_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid job ID".to_string()))?;
    match state.rescan.get(job_id).await {
        Some(handle) => Ok(Json(handle.job.read().await.clone())),
        None => Err(AppError::NotFound("Rescan job".to_string())),
    }
}

/// Returns the currently-running rescan job, if any. Used by the frontend to
/// restore the live-progress panel on reload or in a fresh admin session.
pub async fn get_active_rescan_job(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Option<RescanJob>>> {
    match state.rescan.any_active().await {
        Some(handle) => Ok(Json(Some(handle.job.read().await.clone()))),
        None => Ok(Json(None)),
    }
}

pub async fn cancel_rescan_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let job_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid job ID".to_string()))?;
    if state.rescan.cancel(job_id).await {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::NotFound("Rescan job".to_string()))
    }
}

/// Runs the actual scan. All progress goes through `handle.job` so the client
/// can poll while we're still working.
async fn run_rescan_worker(
    state: Arc<AppState>,
    storage_id: ObjectId,
    backend: Arc<dyn StorageBackend>,
    handle: Arc<RescanJobHandle>,
) -> std::result::Result<(), String> {
    // Load all users up-front so we can compute a real total for the UI.
    let users: Vec<User> = {
        let coll = state.db.collection::<User>("users");
        let mut cursor = coll.find(doc! {}).await.map_err(|e| e.to_string())?;
        let mut users = Vec::new();
        while cursor.advance().await.map_err(|e| e.to_string())? {
            users.push(cursor.deserialize_current().map_err(|e| e.to_string())?);
        }
        users
    };

    // Scan each user's tree and tally the total before we start importing.
    let mut per_user: Vec<(User, String, Vec<crate::storage::ScanEntry>)> = Vec::new();
    let mut total = 0u64;
    for user in users {
        if handle.is_cancelled() {
            return Ok(());
        }
        let user_prefix = sanitize_path_component(&user.username);
        let mut entries = backend.scan(&user_prefix).await.map_err(|e| e.to_string())?;
        // Sort shallow-first so parent folders get created before their children.
        entries.sort_by_key(|e| e.path.matches('/').count());
        total = total.saturating_add(entries.len() as u64);
        per_user.push((user, user_prefix, entries));
    }
    handle.job.write().await.total_entries = Some(total);

    let mut owner_bytes_delta: HashMap<ObjectId, i64> = HashMap::new();

    for (user, user_prefix, entries) in per_user {
        if handle.is_cancelled() {
            break;
        }

        let mut folder_by_storage_path: HashMap<String, ObjectId> = HashMap::new();
        preload_user_folders(&state, user.id, &user_prefix, &mut folder_by_storage_path)
            .await
            .map_err(|e| e.to_string())?;

        // Thread-local counters flushed periodically into the shared job.
        let mut processed = 0u64;
        let mut imported_folders = 0u64;
        let mut imported_files = 0u64;
        let mut skipped = 0u64;
        let mut pending_conflicts: Vec<RescanConflict> = Vec::new();

        for entry in entries {
            if handle.is_cancelled() {
                break;
            }
            processed += 1;

            let rel_within_user = entry
                .path
                .strip_prefix(&format!("{}/", user_prefix))
                .unwrap_or(&entry.path);
            let first = rel_within_user.split('/').next().unwrap_or("");
            if matches!(first, ".uncloud" | ".thumbs" | ".tmp") || entry.path == user_prefix {
                maybe_flush(
                    &state,
                    &handle,
                    &mut processed,
                    &mut imported_folders,
                    &mut imported_files,
                    &mut skipped,
                    &mut pending_conflicts,
                )
                .await;
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
                            pending_conflicts.push(RescanConflict {
                                path: entry.path.clone(),
                                reason: "parent folder not yet imported".into(),
                            });
                            maybe_flush(
                                &state,
                                &handle,
                                &mut processed,
                                &mut imported_folders,
                                &mut imported_files,
                                &mut skipped,
                                &mut pending_conflicts,
                            )
                            .await;
                            continue;
                        }
                    }
                };

                if let Some(existing) = find_live_folder(&state, user.id, parent_id, &name)
                    .await
                    .map_err(|e| e.to_string())?
                {
                    folder_by_storage_path.insert(entry.path.clone(), existing.id);
                    skipped += 1;
                } else {
                    let folder = Folder::new(user.id, parent_id, name.clone());
                    let folder_id = folder.id;
                    state
                        .db
                        .collection::<Folder>("folders")
                        .insert_one(&folder)
                        .await
                        .map_err(|e| e.to_string())?;
                    folder_by_storage_path.insert(entry.path.clone(), folder_id);
                    state.events.emit_folder_created(user.id, &folder).await;
                    imported_folders += 1;
                }
            } else {
                let (parent_storage_path, name) = split_parent(&entry.path);
                let parent_id = if parent_storage_path == user_prefix {
                    None
                } else {
                    match folder_by_storage_path.get(&parent_storage_path) {
                        Some(id) => Some(*id),
                        None => {
                            pending_conflicts.push(RescanConflict {
                                path: entry.path.clone(),
                                reason: "parent folder not yet imported".into(),
                            });
                            maybe_flush(
                                &state,
                                &handle,
                                &mut processed,
                                &mut imported_folders,
                                &mut imported_files,
                                &mut skipped,
                                &mut pending_conflicts,
                            )
                            .await;
                            continue;
                        }
                    }
                };

                if let Some(existing) = find_live_file(&state, user.id, parent_id, &name)
                    .await
                    .map_err(|e| e.to_string())?
                {
                    if existing.size_bytes != entry.size_bytes as i64 {
                        pending_conflicts.push(RescanConflict {
                            path: entry.path.clone(),
                            reason: format!(
                                "on-disk size {} differs from DB record {}",
                                entry.size_bytes, existing.size_bytes
                            ),
                        });
                    } else {
                        skipped += 1;
                    }
                    maybe_flush(
                        &state,
                        &handle,
                        &mut processed,
                        &mut imported_folders,
                        &mut imported_files,
                        &mut skipped,
                        &mut pending_conflicts,
                    )
                    .await;
                    continue;
                }

                let checksum = match hash_file(&backend, &entry.path).await {
                    Ok(c) => c,
                    Err(e) => {
                        pending_conflicts.push(RescanConflict {
                            path: entry.path.clone(),
                            reason: format!("failed to hash: {}", e),
                        });
                        maybe_flush(
                            &state,
                            &handle,
                            &mut processed,
                            &mut imported_folders,
                            &mut imported_files,
                            &mut skipped,
                            &mut pending_conflicts,
                        )
                        .await;
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
                    .await
                    .map_err(|e| e.to_string())?;
                *owner_bytes_delta.entry(user.id).or_default() += entry.size_bytes as i64;

                state.events.emit_file_created(user.id, &file).await;
                state.processing.enqueue(&file, state.clone()).await;
                imported_files += 1;
            }

            maybe_flush(
                &state,
                &handle,
                &mut processed,
                &mut imported_folders,
                &mut imported_files,
                &mut skipped,
                &mut pending_conflicts,
            )
            .await;
        }

        // Final flush for this user.
        flush_counters(
            &state,
            &handle,
            &mut processed,
            &mut imported_folders,
            &mut imported_files,
            &mut skipped,
            &mut pending_conflicts,
        )
        .await;
    }

    for (owner_id, delta) in owner_bytes_delta {
        if delta > 0 {
            let _ = state.auth.update_user_bytes(owner_id, delta).await;
        }
    }

    Ok(())
}

async fn maybe_flush(
    state: &AppState,
    handle: &RescanJobHandle,
    processed: &mut u64,
    imported_folders: &mut u64,
    imported_files: &mut u64,
    skipped: &mut u64,
    conflicts: &mut Vec<RescanConflict>,
) {
    if *processed >= COUNTER_FLUSH_EVERY || !conflicts.is_empty() {
        flush_counters(
            state,
            handle,
            processed,
            imported_folders,
            imported_files,
            skipped,
            conflicts,
        )
        .await;
    }
}

async fn flush_counters(
    state: &AppState,
    handle: &RescanJobHandle,
    processed: &mut u64,
    imported_folders: &mut u64,
    imported_files: &mut u64,
    skipped: &mut u64,
    conflicts: &mut Vec<RescanConflict>,
) {
    if *processed == 0
        && *imported_folders == 0
        && *imported_files == 0
        && *skipped == 0
        && conflicts.is_empty()
    {
        return;
    }
    let snapshot = {
        let mut job = handle.job.write().await;
        job.processed_entries = job.processed_entries.saturating_add(*processed);
        job.imported_folders = job.imported_folders.saturating_add(*imported_folders);
        job.imported_files = job.imported_files.saturating_add(*imported_files);
        job.skipped_existing = job.skipped_existing.saturating_add(*skipped);
        for c in conflicts.drain(..) {
            if job.conflicts.len() < MAX_CONFLICTS {
                job.conflicts.push(c);
            }
        }
        job.clone()
    };
    *processed = 0;
    *imported_folders = 0;
    *imported_files = 0;
    *skipped = 0;
    state.events.emit_rescan_progress(&snapshot).await;
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
    backend: &Arc<dyn StorageBackend>,
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
