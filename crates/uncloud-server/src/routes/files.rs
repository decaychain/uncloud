use axum::{
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use mongodb::{bson::{doc, oid::ObjectId}, Database};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio_stream::StreamExt;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use std::collections::{HashMap, HashSet};
use crate::error::{AppError, Result};
use crate::middleware::AuthUser;
use crate::models::{File, FileVersion, Folder, ProcessingStatus, TaskType, UploadChunk};
use crate::routes::apps::{deliver_webhooks, EVENT_FILE_CREATED, EVENT_FILE_UPDATED, EVENT_FILE_DELETED};
use crate::AppState;
use uncloud_common::{FileResponse, InheritableSetting};

/// Strip characters that are unsafe in filesystem path components.
pub(crate) fn sanitize_path_component(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '/' | '\\' | '\0' => '_',
            c => c,
        })
        .collect()
}

/// Returns true if any file or folder with this name already exists in the
/// parent location for the given user.  Pass `exclude_file` / `exclude_folder`
/// to skip the item being renamed (so it doesn't conflict with itself).
pub(crate) async fn check_name_conflict(
    db: &Database,
    user_id: ObjectId,
    parent_id: Option<ObjectId>,
    name: &str,
    exclude_file: Option<ObjectId>,
    exclude_folder: Option<ObjectId>,
) -> Result<bool> {
    let parent_bson = parent_id
        .map(mongodb::bson::Bson::ObjectId)
        .unwrap_or(mongodb::bson::Bson::Null);

    // Check existing files
    let files_coll = db.collection::<File>("files");
    let mut filter = doc! { "owner_id": user_id, "parent_id": &parent_bson, "name": name, "deleted_at": mongodb::bson::Bson::Null };
    if let Some(eid) = exclude_file {
        filter.insert("_id", doc! { "$ne": eid });
    }
    if files_coll.find_one(filter).await?.is_some() {
        return Ok(true);
    }

    // Check existing folders
    let folders_coll = db.collection::<Folder>("folders");
    let mut filter = doc! { "owner_id": user_id, "parent_id": &parent_bson, "name": name, "deleted_at": mongodb::bson::Bson::Null };
    if let Some(eid) = exclude_folder {
        filter.insert("_id", doc! { "$ne": eid });
    }
    Ok(folders_coll.find_one(filter).await?.is_some())
}

/// Build a logical storage path: `{username}/{folder/chain}/{filename}`.
/// Walks the parent folder chain upward through the DB to reconstruct the
/// full path, so that on-disk layout mirrors the user's logical folder tree.
pub(crate) async fn resolve_storage_path(
    db: &Database,
    user_id: ObjectId,
    username: &str,
    parent_id: Option<ObjectId>,
    filename: &str,
) -> Result<String> {
    let mut segments: Vec<String> = Vec::new();
    let collection = db.collection::<Folder>("folders");
    let mut current = parent_id;

    while let Some(id) = current {
        let folder = collection
            .find_one(doc! { "_id": id, "owner_id": user_id })
            .await?
            .ok_or_else(|| AppError::NotFound("Parent folder not found".to_string()))?;
        segments.push(sanitize_path_component(&folder.name));
        current = folder.parent_id;
    }

    segments.reverse();
    segments.push(sanitize_path_component(filename));

    Ok(format!("{}/{}", sanitize_path_component(username), segments.join("/")))
}

#[derive(Debug, Deserialize)]
pub struct ListFilesQuery {
    pub parent_id: Option<String>,
}

pub(crate) fn file_to_response(f: &File) -> FileResponse {
    FileResponse {
        id: f.id.to_hex(),
        name: f.name.clone(),
        mime_type: f.mime_type.clone(),
        size_bytes: f.size_bytes,
        parent_id: f.parent_id.map(|id| id.to_hex()),
        created_at: f.created_at.to_rfc3339(),
        updated_at: f.updated_at.to_rfc3339(),
        metadata: f
            .metadata
            .iter()
            .filter_map(|(k, v)| {
                bson::from_bson::<serde_json::Value>(v.clone())
                    .ok()
                    .map(|json| (k.clone(), json))
            })
            .collect(),
    }
}

#[derive(Debug, Deserialize)]
pub struct InitUploadRequest {
    pub filename: String,
    pub size: i64,
    pub parent_id: Option<String>,
    pub chunk_size: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct InitUploadResponse {
    pub upload_id: String,
    pub chunk_size: i64,
    pub total_chunks: i32,
}

#[derive(Debug, Deserialize)]
pub struct UpdateFileRequest {
    pub name: Option<String>,
    pub parent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CopyFileRequest {
    pub parent_id: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ChunkQuery {
    pub index: i32,
}

pub async fn list_files(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(query): Query<ListFilesQuery>,
) -> Result<Json<Vec<FileResponse>>> {
    let parent_id = match &query.parent_id {
        Some(id) if !id.is_empty() => Some(
            ObjectId::parse_str(id)
                .map_err(|_| AppError::BadRequest("Invalid parent ID".to_string()))?,
        ),
        _ => None,
    };

    let filter = match parent_id {
        Some(pid) => doc! { "owner_id": user.id, "parent_id": pid, "deleted_at": mongodb::bson::Bson::Null },
        None => doc! { "owner_id": user.id, "parent_id": null, "deleted_at": mongodb::bson::Bson::Null },
    };

    let collection = state.db.collection::<File>("files");
    let mut cursor = collection.find(filter).await?;

    let mut files = Vec::new();
    while cursor.advance().await? {
        let file: File = cursor.deserialize_current()?;
        files.push(file_to_response(&file));
    }

    Ok(Json(files))
}

pub async fn get_file(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<FileResponse>> {
    let file_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid file ID".to_string()))?;

    let collection = state.db.collection::<File>("files");
    let file = collection
        .find_one(doc! { "_id": file_id, "owner_id": user.id, "deleted_at": mongodb::bson::Bson::Null })
        .await?
        .ok_or_else(|| AppError::NotFound("File not found".to_string()))?;

    Ok(Json(file_to_response(&file)))
}

/// Parse a `Range: bytes=start-end` header value into (start, optional end).
/// Only supports a single byte range (no multipart ranges).
fn parse_range_header(range: &str, total: u64) -> Option<(u64, u64)> {
    let range = range.strip_prefix("bytes=")?;
    // Only handle the first range (ignore multi-range)
    let range = range.split(',').next()?.trim();
    let (start_str, end_str) = range.split_once('-')?;

    if start_str.is_empty() {
        // Suffix range: bytes=-500 means last 500 bytes
        let suffix_len: u64 = end_str.parse().ok()?;
        if suffix_len == 0 || suffix_len > total {
            return None;
        }
        let start = total - suffix_len;
        Some((start, total - 1))
    } else {
        let start: u64 = start_str.parse().ok()?;
        if start >= total {
            return None;
        }
        let end = if end_str.is_empty() {
            total - 1
        } else {
            let end: u64 = end_str.parse().ok()?;
            end.min(total - 1)
        };
        if end < start {
            return None;
        }
        Some((start, end))
    }
}

pub async fn download_file(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response> {
    let file_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid file ID".to_string()))?;

    let collection = state.db.collection::<File>("files");
    let file = collection
        .find_one(doc! { "_id": file_id, "owner_id": user.id, "deleted_at": mongodb::bson::Bson::Null })
        .await?
        .ok_or_else(|| AppError::NotFound("File not found".to_string()))?;

    let backend = state.storage.get_backend(file.storage_id).await?;
    let total = file.size_bytes as u64;

    let disposition_type = if file.mime_type.starts_with("audio/")
        || file.mime_type.starts_with("video/")
        || file.mime_type.starts_with("image/")
    {
        "inline"
    } else {
        "attachment"
    };
    let content_disposition = format!(
        "{}; filename=\"{}\"",
        disposition_type,
        file.name.replace('"', "\\\"")
    );

    // Check for Range header
    if let Some(range_value) = headers.get(header::RANGE) {
        let range_str = range_value
            .to_str()
            .map_err(|_| AppError::BadRequest("Invalid Range header".to_string()))?;

        let (start, end) = parse_range_header(range_str, total)
            .ok_or(AppError::RangeNotSatisfiable(file.size_bytes))?;

        let length = end - start + 1;
        let reader = backend.read_range(&file.storage_path, start, length).await?;
        let stream = ReaderStream::new(reader);
        let body = Body::from_stream(stream);

        Ok(Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_TYPE, &file.mime_type)
            .header(header::CONTENT_DISPOSITION, &content_disposition)
            .header(header::CONTENT_LENGTH, length)
            .header(header::CONTENT_RANGE, format!("bytes {}-{}/{}", start, end, total))
            .header(header::ACCEPT_RANGES, "bytes")
            .body(body)
            .unwrap())
    } else {
        let reader = backend.read(&file.storage_path).await?;
        let stream = ReaderStream::new(reader);
        let body = Body::from_stream(stream);

        Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, &file.mime_type)
            .header(header::CONTENT_DISPOSITION, content_disposition)
            .header(header::CONTENT_LENGTH, file.size_bytes)
            .header(header::ACCEPT_RANGES, "bytes")
            .body(body)
            .unwrap())
    }
}

pub async fn update_file(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<UpdateFileRequest>,
) -> Result<Json<FileResponse>> {
    let file_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid file ID".to_string()))?;

    let collection = state.db.collection::<File>("files");
    let file = collection
        .find_one(doc! { "_id": file_id, "owner_id": user.id, "deleted_at": mongodb::bson::Bson::Null })
        .await?
        .ok_or_else(|| AppError::NotFound("File not found".to_string()))?;

    // Effective name/parent after this update
    let new_name: &str = req.name.as_deref().unwrap_or(&file.name);
    let new_parent_id: Option<ObjectId> = match req.parent_id.as_deref() {
        Some("") => None, // empty string = move to root
        Some(pid) => Some(
            ObjectId::parse_str(pid)
                .map_err(|_| AppError::BadRequest("Invalid parent ID".to_string()))?,
        ),
        None => file.parent_id, // not supplied → unchanged
    };

    let name_changed = new_name != file.name.as_str();
    let parent_changed = new_parent_id != file.parent_id;

    let mut set_doc = doc! { "updated_at": mongodb::bson::DateTime::now() };

    if name_changed || parent_changed {
        // Conflict check
        if check_name_conflict(&state.db, user.id, new_parent_id, new_name, Some(file_id), None)
            .await?
        {
            return Err(AppError::Conflict(
                "A file with this name already exists at this location".to_string(),
            ));
        }

        // Compute new storage path and rename the blob on disk
        let new_path = resolve_storage_path(
            &state.db,
            user.id,
            &user.username,
            new_parent_id,
            new_name,
        )
        .await?;

        if new_path != file.storage_path {
            let backend = state.storage.get_backend(file.storage_id).await?;
            backend.rename(&file.storage_path, &new_path).await?;
        }

        set_doc.insert("storage_path", &new_path);

        if name_changed {
            set_doc.insert("name", new_name);
        }
        if parent_changed {
            set_doc.insert(
                "parent_id",
                new_parent_id
                    .map(mongodb::bson::Bson::ObjectId)
                    .unwrap_or(mongodb::bson::Bson::Null),
            );
        }
    }

    collection
        .update_one(doc! { "_id": file_id, "owner_id": user.id }, doc! { "$set": set_doc })
        .await?;

    let updated = collection
        .find_one(doc! { "_id": file_id })
        .await?
        .ok_or_else(|| AppError::NotFound("File not found".to_string()))?;

    state.events.emit_file_updated(user.id, &updated).await;
    {
        let state_clone = state.clone();
        let file_id = updated.id.to_hex();
        let owner_id = user.id.to_hex();
        let username = user.username.clone();
        let name = updated.name.clone();
        tokio::spawn(async move {
            deliver_webhooks(&state_clone, EVENT_FILE_UPDATED, serde_json::json!({
                "file_id": file_id,
                "owner_id": owner_id,
                "username": username,
                "name": name,
            })).await;
        });
    }

    // Best-effort re-index in search after rename/move
    if state.search.is_enabled() {
        let content_text = updated
            .metadata
            .get("content_text")
            .and_then(|b| if let mongodb::bson::Bson::String(s) = b { Some(s.clone()) } else { None })
            .unwrap_or_default();
        let search_doc = crate::services::search::SearchDocument {
            id: updated.id.to_hex(),
            owner_id: updated.owner_id.to_hex(),
            name: updated.name.clone(),
            mime_type: updated.mime_type.clone(),
            content_text,
            parent_id: updated.parent_id.map(|id| id.to_hex()),
            size_bytes: updated.size_bytes,
            created_at: updated.created_at.to_rfc3339(),
            updated_at: updated.updated_at.to_rfc3339(),
        };
        if let Err(e) = state.search.index_file(search_doc).await {
            tracing::warn!("Failed to re-index file {} in search: {}", updated.id, e);
        }
    }

    Ok(Json(file_to_response(&updated)))
}

pub async fn delete_file(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let file_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid file ID".to_string()))?;

    let collection = state.db.collection::<File>("files");
    let file = collection
        .find_one(doc! { "_id": file_id, "owner_id": user.id, "deleted_at": mongodb::bson::Bson::Null })
        .await?
        .ok_or_else(|| AppError::NotFound("File not found".to_string()))?;

    // Move blob to trash on storage
    let backend = state.storage.get_backend(file.storage_id).await?;
    let tp = trash_path(&file.storage_path);
    backend.move_to_trash(&file.storage_path, &tp).await?;

    // Best-effort thumbnail cleanup (may not exist yet).
    let _ = backend.delete(&format!(".thumbs/{}.jpg", file.id.to_hex())).await;

    // Soft-delete: set deleted_at and trash_path instead of removing the record.
    // Quota is NOT updated on soft delete — only on permanent purge.
    let now = chrono::Utc::now();
    let batch_id = uuid::Uuid::new_v4().to_string();
    collection
        .update_one(
            doc! { "_id": file_id, "owner_id": user.id },
            doc! { "$set": {
                "deleted_at": mongodb::bson::DateTime::from_chrono(now),
                "trash_path": &tp,
                "batch_delete_id": &batch_id,
            }},
        )
        .await?;

    // Remove from any playlists
    let _ = state
        .db
        .collection::<bson::Document>("playlists")
        .update_many(
            doc! { "tracks.file_id": file_id },
            doc! { "$pull": { "tracks": { "file_id": file_id } } },
        )
        .await;

    // Remove from search index
    if let Err(e) = state.search.delete_file(&id).await {
        tracing::warn!("Failed to remove file {} from search index: {}", id, e);
    }

    state.events.emit_file_deleted(user.id, file_id).await;
    {
        let state_clone = state.clone();
        let file_id_str = id.clone();
        let owner_id = user.id.to_hex();
        let username = user.username.clone();
        let name = file.name.clone();
        tokio::spawn(async move {
            deliver_webhooks(&state_clone, EVENT_FILE_DELETED, serde_json::json!({
                "file_id": file_id_str,
                "owner_id": owner_id,
                "username": username,
                "name": name,
            })).await;
        });
    }

    Ok(StatusCode::NO_CONTENT)
}

/// Derive the trash path from a file's current storage path.
///
/// `alice/photos/cat.jpg` at `2024-01-15T10:30:00Z`
/// -> `alice/.uncloud/trash/2024-01-15T103000Z/photos/cat.jpg`
pub(crate) fn trash_path(storage_path: &str) -> String {
    let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H%M%SZ");
    match storage_path.splitn(2, '/').collect::<Vec<_>>()[..] {
        [username, rel] => format!("{}/.uncloud/trash/{}/{}", username, rel, timestamp),
        _ => format!("{}/.uncloud/trash/{}", storage_path, timestamp),
    }
}

/// Derive the version archive path from the file's current storage path.
///
/// `alice/photos/cat.jpg` at `2024-01-15T10:30:00Z`
/// → `alice/.uncloud/versions/photos/cat.jpg/2024-01-15T103000Z`
pub(crate) fn version_path(storage_path: &str) -> String {
    let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H%M%SZ");
    match storage_path.splitn(2, '/').collect::<Vec<_>>()[..] {
        [username, rel] => format!("{}/.uncloud/versions/{}/{}", username, rel, timestamp),
        _ => format!("{}/.versions/{}", storage_path, timestamp),
    }
}

/// Replace a file's content, archiving the current blob as a version first.
///
/// `POST /api/files/{id}/content`
pub async fn update_file_content(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> Result<Json<FileResponse>> {
    let file_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid file ID".to_string()))?;

    let collection = state.db.collection::<File>("files");
    let file = collection
        .find_one(doc! { "_id": file_id, "owner_id": user.id, "deleted_at": mongodb::bson::Bson::Null })
        .await?
        .ok_or_else(|| AppError::NotFound("File not found".to_string()))?;

    // Read new content from multipart
    let mut new_data = Vec::new();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {}", e)))?
    {
        if field.name().unwrap_or("") == "file" {
            new_data = field
                .bytes()
                .await
                .map_err(|e| AppError::BadRequest(format!("Failed to read file: {}", e)))?
                .to_vec();
            break;
        }
    }
    if new_data.is_empty() {
        return Err(AppError::BadRequest("No file data provided".to_string()));
    }

    let backend = state.storage.get_backend(file.storage_id).await?;

    // 1. Count existing versions to determine the next version number.
    let versions_coll = state.db.collection::<FileVersion>("file_versions");
    let version_number = versions_coll
        .count_documents(doc! { "file_id": file_id })
        .await? as i32
        + 1;

    // 2. Archive the current blob.
    let ver_path = version_path(&file.storage_path);
    backend.archive_version(&file.storage_path, &ver_path).await?;

    // 3. Insert a FileVersion record for the old content.
    let file_version = FileVersion::new(
        file_id,
        version_number,
        ver_path,
        file.size_bytes,
        file.checksum_sha256.clone(),
    );
    versions_coll.insert_one(&file_version).await?;

    // 4. Write new bytes over the existing storage path.
    let new_size = new_data.len() as i64;
    let mut hasher = sha2::Sha256::new();
    sha2::Digest::update(&mut hasher, &new_data);
    let new_checksum = hex::encode(hasher.finalize());
    let new_mime = mime_guess::from_path(&file.name)
        .first_or_octet_stream()
        .to_string();

    backend.write(&file.storage_path, &new_data).await?;

    // 5. Update the files record.
    let size_delta = new_size - file.size_bytes;
    let now = chrono::Utc::now();
    collection
        .update_one(
            doc! { "_id": file_id },
            doc! { "$set": {
                "size_bytes": new_size,
                "checksum_sha256": &new_checksum,
                "mime_type": &new_mime,
                "updated_at": bson::DateTime::from_chrono(now),
                "processing_tasks": [],
            }},
        )
        .await?;

    if size_delta != 0 {
        state.auth.update_user_bytes(user.id, size_delta).await?;
    }

    let updated = File {
        id: file.id,
        storage_id: file.storage_id,
        storage_path: file.storage_path,
        owner_id: file.owner_id,
        parent_id: file.parent_id,
        name: file.name,
        mime_type: new_mime,
        size_bytes: new_size,
        checksum_sha256: new_checksum,
        created_at: file.created_at,
        updated_at: now,
        processing_tasks: vec![],
        metadata: std::collections::HashMap::new(),
        deleted_at: None,
        trash_path: None,
        batch_delete_id: None,
    };

    // Also remove any stale thumbnail blob so the new one is generated fresh.
    let _ = backend.delete(&format!(".thumbs/{}.jpg", file_id.to_hex())).await;

    state.events.emit_file_created(user.id, &updated).await;
    {
        let state_clone = state.clone();
        let file_id = updated.id.to_hex();
        let owner_id = user.id.to_hex();
        let username = user.username.clone();
        let name = updated.name.clone();
        tokio::spawn(async move {
            deliver_webhooks(&state_clone, EVENT_FILE_UPDATED, serde_json::json!({
                "file_id": file_id,
                "owner_id": owner_id,
                "username": username,
                "name": name,
            })).await;
        });
    }
    state.processing.enqueue(&updated, state.clone()).await;

    Ok(Json(file_to_response(&updated)))
}

pub async fn simple_upload(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    mut multipart: Multipart,
) -> Result<Json<FileResponse>> {
    let mut filename = None;
    let mut parent_id: Option<ObjectId> = None;
    let mut file_data = Vec::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {}", e)))?
    {
        let name = field.name().unwrap_or("").to_string();

        match name.as_str() {
            "file" => {
                filename = field.file_name().map(|s| s.to_string());
                file_data = field
                    .bytes()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("Failed to read file: {}", e)))?
                    .to_vec();
            }
            "parent_id" => {
                let text = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("Failed to read field: {}", e)))?;
                if !text.is_empty() {
                    parent_id = Some(
                        ObjectId::parse_str(&text)
                            .map_err(|_| AppError::BadRequest("Invalid parent ID".to_string()))?,
                    );
                }
            }
            _ => {}
        }
    }

    let filename = filename.ok_or_else(|| AppError::BadRequest("No file provided".to_string()))?;
    let size = file_data.len() as i64;

    // Check quota
    if !user.has_quota_space(size) {
        return Err(AppError::Forbidden);
    }

    // Get or create default storage (auto-provisions on first upload)
    let storage = state.storage.get_or_create_default(user.id).await?;
    let backend = state.storage.get_backend(storage.id).await?;

    // Build logical storage path: username/folder/chain/filename
    let storage_path = resolve_storage_path(
        &state.db,
        user.id,
        &user.username,
        parent_id,
        &filename,
    ).await?;

    // Calculate checksum
    let mut hasher = Sha256::new();
    hasher.update(&file_data);
    let checksum = hex::encode(hasher.finalize());

    // Write to storage
    backend.write(&storage_path, &file_data).await?;

    // Determine MIME type
    let mime_type = mime_guess::from_path(&filename)
        .first_or_octet_stream()
        .to_string();

    // Create file record
    let file = File::new(
        storage.id,
        storage_path,
        user.id,
        parent_id,
        filename,
        mime_type,
        size,
        checksum,
    );

    let collection = state.db.collection::<File>("files");
    collection.insert_one(&file).await?;

    // Update user's used bytes
    state.auth.update_user_bytes(user.id, size).await?;

    state.events.emit_file_created(user.id, &file).await;
    {
        let state_clone = state.clone();
        let file_id = file.id.to_hex();
        let owner_id = user.id.to_hex();
        let username = user.username.clone();
        let name = file.name.clone();
        tokio::spawn(async move {
            deliver_webhooks(&state_clone, EVENT_FILE_CREATED, serde_json::json!({
                "file_id": file_id,
                "owner_id": owner_id,
                "username": username,
                "name": name,
            })).await;
        });
    }
    state.processing.enqueue(&file, state.clone()).await;

    Ok(Json(file_to_response(&file)))
}

pub async fn init_upload(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(req): Json<InitUploadRequest>,
) -> Result<Json<InitUploadResponse>> {
    // Check quota
    if !user.has_quota_space(req.size) {
        return Err(AppError::Forbidden);
    }

    let chunk_size = req.chunk_size.unwrap_or(state.config.uploads.max_chunk_size as i64);

    // Get or create default storage (auto-provisions on first upload)
    let storage = state.storage.get_or_create_default(user.id).await?;
    let backend = state.storage.get_backend(storage.id).await?;

    // Create temp file
    let temp_path = backend.create_temp().await?;
    let upload_id = Uuid::new_v4().to_string();

    let parent_id = match &req.parent_id {
        Some(id) if !id.is_empty() => Some(
            ObjectId::parse_str(id)
                .map_err(|_| AppError::BadRequest("Invalid parent ID".to_string()))?,
        ),
        _ => None,
    };

    let upload = UploadChunk::new(
        upload_id.clone(),
        user.id,
        req.filename,
        parent_id,
        storage.id,
        req.size,
        chunk_size,
        temp_path,
    );

    let collection = state.db.collection::<UploadChunk>("upload_chunks");
    collection.insert_one(&upload).await?;

    Ok(Json(InitUploadResponse {
        upload_id,
        chunk_size,
        total_chunks: upload.total_chunks(),
    }))
}

pub async fn upload_chunk(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(upload_id): Path<String>,
    Query(query): Query<ChunkQuery>,
    body: axum::body::Bytes,
) -> Result<StatusCode> {
    let collection = state.db.collection::<UploadChunk>("upload_chunks");

    let upload = collection
        .find_one(doc! { "upload_id": &upload_id, "user_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Upload not found".to_string()))?;

    if query.index >= upload.total_chunks() {
        return Err(AppError::BadRequest("Invalid chunk index".to_string()));
    }

    if upload.chunks_received.contains(&query.index) {
        return Ok(StatusCode::OK); // Already received
    }

    let backend = state.storage.get_backend(upload.storage_id).await?;
    backend.append_temp(&upload.temp_path, &body).await?;

    // Update chunks received
    collection
        .update_one(
            doc! { "upload_id": &upload_id },
            doc! { "$push": { "chunks_received": query.index } },
        )
        .await?;

    // Emit progress
    let progress = (upload.chunks_received.len() + 1) as f64 / upload.total_chunks() as f64;
    state
        .events
        .emit_upload_progress(user.id, &upload_id, progress)
        .await;

    Ok(StatusCode::OK)
}

pub async fn complete_upload(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(upload_id): Path<String>,
) -> Result<Json<FileResponse>> {
    let collection = state.db.collection::<UploadChunk>("upload_chunks");

    let upload = collection
        .find_one(doc! { "upload_id": &upload_id, "user_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Upload not found".to_string()))?;

    if !upload.is_complete() {
        return Err(AppError::BadRequest(format!(
            "Upload incomplete: {}/{} chunks received",
            upload.chunks_received.len(),
            upload.total_chunks()
        )));
    }

    let backend = state.storage.get_backend(upload.storage_id).await?;

    // Build logical storage path: username/folder/chain/filename
    let storage_path = resolve_storage_path(
        &state.db,
        user.id,
        &user.username,
        upload.parent_id,
        &upload.filename,
    ).await?;

    // Finalize temp file
    backend
        .finalize_temp(&upload.temp_path, &storage_path)
        .await?;

    // Read file to calculate checksum
    let reader = backend.read(&storage_path).await?;
    let mut hasher = Sha256::new();
    let mut reader = tokio::io::BufReader::new(reader);
    let mut buf = [0u8; 8192];
    loop {
        let n = reader.read(&mut buf).await.map_err(|e| {
            AppError::Storage(format!("Failed to read for checksum: {}", e))
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let checksum = hex::encode(hasher.finalize());

    // Determine MIME type
    let mime_type = mime_guess::from_path(&upload.filename)
        .first_or_octet_stream()
        .to_string();

    // Create file record
    let file = File::new(
        upload.storage_id,
        storage_path,
        user.id,
        upload.parent_id,
        upload.filename.clone(),
        mime_type,
        upload.total_size,
        checksum,
    );

    let files_collection = state.db.collection::<File>("files");
    files_collection.insert_one(&file).await?;

    // Update user's used bytes
    state
        .auth
        .update_user_bytes(user.id, upload.total_size)
        .await?;

    // Delete upload record
    collection
        .delete_one(doc! { "upload_id": &upload_id })
        .await?;

    state.events.emit_file_created(user.id, &file).await;
    {
        let state_clone = state.clone();
        let file_id = file.id.to_hex();
        let owner_id = user.id.to_hex();
        let username = user.username.clone();
        let name = file.name.clone();
        tokio::spawn(async move {
            deliver_webhooks(&state_clone, EVENT_FILE_CREATED, serde_json::json!({
                "file_id": file_id,
                "owner_id": owner_id,
                "username": username,
                "name": name,
            })).await;
        });
    }
    state.processing.enqueue(&file, state.clone()).await;

    Ok(Json(file_to_response(&file)))
}

pub async fn copy_file(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<CopyFileRequest>,
) -> Result<Json<FileResponse>> {
    let file_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid file ID".to_string()))?;

    let collection = state.db.collection::<File>("files");
    let file = collection
        .find_one(doc! { "_id": file_id, "owner_id": user.id, "deleted_at": mongodb::bson::Bson::Null })
        .await?
        .ok_or_else(|| AppError::NotFound("File not found".to_string()))?;

    if !user.has_quota_space(file.size_bytes) {
        return Err(AppError::Forbidden);
    }

    // Destination parent: None (same as source) | Some("") = root | Some(id) = folder
    let dest_parent_id = match req.parent_id.as_deref() {
        Some("") => None,
        Some(pid) => Some(
            ObjectId::parse_str(pid)
                .map_err(|_| AppError::BadRequest("Invalid parent ID".to_string()))?,
        ),
        None => file.parent_id,
    };

    let dest_name = req.name
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("Copy of {}", file.name));

    // Conflict check
    if check_name_conflict(&state.db, user.id, dest_parent_id, &dest_name, None, None).await? {
        return Err(AppError::Conflict(
            "A file with this name already exists at this location".to_string(),
        ));
    }

    let dst_path = resolve_storage_path(
        &state.db,
        user.id,
        &user.username,
        dest_parent_id,
        &dest_name,
    )
    .await?;

    // Read source bytes and write to destination
    let backend = state.storage.get_backend(file.storage_id).await?;
    let mut reader = backend.read(&file.storage_path).await?;
    let mut data = Vec::new();
    reader.read_to_end(&mut data).await.map_err(|e| {
        AppError::Storage(format!("Failed to read source file: {}", e))
    })?;
    backend.write(&dst_path, &data).await?;

    let new_file = File::new(
        file.storage_id,
        dst_path,
        user.id,
        dest_parent_id,
        dest_name,
        file.mime_type.clone(),
        file.size_bytes,
        file.checksum_sha256.clone(),
    );

    collection.insert_one(&new_file).await?;
    state.auth.update_user_bytes(user.id, file.size_bytes).await?;
    state.processing.enqueue(&new_file, state.clone()).await;
    state.events.emit_file_created(user.id, &new_file).await;
    {
        let state_clone = state.clone();
        let file_id = new_file.id.to_hex();
        let owner_id = user.id.to_hex();
        let username = user.username.clone();
        let name = new_file.name.clone();
        tokio::spawn(async move {
            deliver_webhooks(&state_clone, EVENT_FILE_CREATED, serde_json::json!({
                "file_id": file_id,
                "owner_id": owner_id,
                "username": username,
                "name": name,
            })).await;
        });
    }

    Ok(Json(file_to_response(&new_file)))
}

pub async fn cancel_upload(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(upload_id): Path<String>,
) -> Result<StatusCode> {
    let collection = state.db.collection::<UploadChunk>("upload_chunks");

    let upload = collection
        .find_one(doc! { "upload_id": &upload_id, "user_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Upload not found".to_string()))?;

    let backend = state.storage.get_backend(upload.storage_id).await?;
    backend.abort_temp(&upload.temp_path).await?;

    collection
        .delete_one(doc! { "upload_id": &upload_id })
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

// --------------------------------------------------------------------------
// Gallery
// --------------------------------------------------------------------------

/// Resolve which folder IDs are "included" according to an opt-in field.
///
/// `get_include(folder)` returns `Some(true)` = Include, `Some(false)` = Exclude,
/// `None` = Inherit. Root default for Inherit is Exclude (all library features
/// are opt-in).
pub(crate) fn resolve_included_folder_ids_by(
    folders: &[Folder],
    get_include: impl Fn(&Folder) -> Option<bool>,
) -> HashSet<Option<ObjectId>> {
    let by_id: HashMap<ObjectId, &Folder> = folders.iter().map(|f| (f.id, f)).collect();
    let mut cache: HashMap<ObjectId, bool> = HashMap::new();

    fn resolve_inner(
        folder_id: ObjectId,
        by_id: &HashMap<ObjectId, &Folder>,
        get_include: &dyn Fn(&Folder) -> Option<bool>,
        cache: &mut HashMap<ObjectId, bool>,
    ) -> bool {
        if let Some(&v) = cache.get(&folder_id) {
            return v;
        }
        let result = match by_id.get(&folder_id) {
            None => false,
            Some(folder) => match get_include(folder) {
                Some(v) => v,
                None => folder
                    .parent_id
                    .map(|pid| resolve_inner(pid, by_id, get_include, cache))
                    .unwrap_or(false), // root Inherit → Exclude
            },
        };
        cache.insert(folder_id, result);
        result
    }

    let mut included = HashSet::new();
    for folder in folders {
        if resolve_inner(folder.id, &by_id, &get_include, &mut cache) {
            included.insert(Some(folder.id));
        }
    }
    included
}

/// Build a breadcrumb-style path string for a folder.
pub(crate) fn build_folder_path(folder_id: ObjectId, by_id: &HashMap<ObjectId, &Folder>) -> String {
    let mut segments = Vec::new();
    let mut current = by_id.get(&folder_id);
    while let Some(f) = current {
        segments.push(f.name.clone());
        current = f.parent_id.and_then(|pid| by_id.get(&pid));
    }
    segments.reverse();
    segments.join(" / ")
}

#[derive(Debug, Deserialize)]
pub struct GalleryQuery {
    pub cursor: Option<String>,
    pub limit: Option<u32>,
    pub folder_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct GalleryResponse {
    pub files: Vec<FileResponse>,
    pub next_cursor: Option<String>,
}

pub async fn list_gallery(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(query): Query<GalleryQuery>,
) -> Result<Json<GalleryResponse>> {
    let limit = query.limit.unwrap_or(60).min(200) as i64;

    if let Some(ref folder_id_str) = query.folder_id {
        // Album mode — scope to one folder
        let folder_id = ObjectId::parse_str(folder_id_str)
            .map_err(|_| AppError::BadRequest("Invalid folder ID".to_string()))?;

        let files_coll = state.db.collection::<File>("files");
        let mut filter = doc! {
            "owner_id": user.id,
            "parent_id": folder_id,
            "mime_type": { "$regex": "^image/" },
            "deleted_at": mongodb::bson::Bson::Null,
        };
        if let Some(ref cursor_str) = query.cursor {
            let cursor_dt = chrono::DateTime::parse_from_rfc3339(cursor_str)
                .map_err(|_| AppError::BadRequest("Invalid cursor".to_string()))?;
            filter.insert("created_at", doc! { "$lt": bson::DateTime::from_chrono(cursor_dt.with_timezone(&chrono::Utc)) });
        }

        let options = mongodb::options::FindOptions::builder()
            .sort(doc! { "created_at": -1 })
            .limit(limit + 1)
            .build();

        let mut cursor = files_coll.find(filter).with_options(options).await?;
        let mut files = Vec::new();
        while cursor.advance().await? {
            let file: File = cursor.deserialize_current()?;
            files.push(file_to_response(&file));
        }

        let next_cursor = if files.len() as i64 > limit {
            files.pop();
            files.last().map(|f| f.created_at.clone())
        } else {
            None
        };

        Ok(Json(GalleryResponse { files, next_cursor }))
    } else {
        // Timeline mode — all included folders
        let folders_coll = state.db.collection::<Folder>("folders");
        let mut folder_cursor = folders_coll.find(doc! { "owner_id": user.id, "deleted_at": mongodb::bson::Bson::Null }).await?;
        let mut all_folders = Vec::new();
        while folder_cursor.advance().await? {
            all_folders.push(folder_cursor.deserialize_current()?);
        }

        let included = resolve_included_folder_ids_by(&all_folders, |f| f.gallery_include.as_include_flag());

        if included.is_empty() {
            return Ok(Json(GalleryResponse { files: Vec::new(), next_cursor: None }));
        }

        let parent_ids: Vec<mongodb::bson::Bson> = included
            .into_iter()
            .map(|opt| match opt {
                Some(id) => mongodb::bson::Bson::ObjectId(id),
                None => mongodb::bson::Bson::Null,
            })
            .collect();

        let files_coll = state.db.collection::<File>("files");
        let mut filter = doc! {
            "owner_id": user.id,
            "parent_id": { "$in": &parent_ids },
            "mime_type": { "$regex": "^image/" },
            "deleted_at": mongodb::bson::Bson::Null,
        };
        if let Some(ref cursor_str) = query.cursor {
            let cursor_dt = chrono::DateTime::parse_from_rfc3339(cursor_str)
                .map_err(|_| AppError::BadRequest("Invalid cursor".to_string()))?;
            filter.insert("created_at", doc! { "$lt": bson::DateTime::from_chrono(cursor_dt.with_timezone(&chrono::Utc)) });
        }

        let options = mongodb::options::FindOptions::builder()
            .sort(doc! { "created_at": -1 })
            .limit(limit + 1)
            .build();

        let mut cursor = files_coll.find(filter).with_options(options).await?;
        let mut files = Vec::new();
        while cursor.advance().await? {
            let file: File = cursor.deserialize_current()?;
            files.push(file_to_response(&file));
        }

        let next_cursor = if files.len() as i64 > limit {
            files.pop();
            files.last().map(|f| f.created_at.clone())
        } else {
            None
        };

        Ok(Json(GalleryResponse { files, next_cursor }))
    }
}

#[derive(Debug, Serialize)]
pub struct AlbumResponse {
    pub folder_id: String,
    pub parent_folder_id: Option<String>,
    pub name: String,
    pub path: String,
    pub image_count: i64,
    pub cover_image_id: Option<String>,
}

pub async fn list_gallery_albums(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<AlbumResponse>>> {
    let folders_coll = state.db.collection::<Folder>("folders");
    let files_coll = state.db.collection::<File>("files");

    let mut folder_cursor = folders_coll.find(doc! { "owner_id": user.id, "deleted_at": mongodb::bson::Bson::Null }).await?;
    let mut all_folders: Vec<Folder> = Vec::new();
    while folder_cursor.advance().await? {
        all_folders.push(folder_cursor.deserialize_current()?);
    }

    let included = resolve_included_folder_ids_by(&all_folders, |f| f.gallery_include.as_include_flag());
    let by_id: HashMap<ObjectId, &Folder> = all_folders.iter().map(|f| (f.id, f)).collect();

    let mut albums = Vec::new();
    for opt_id in &included {
        let folder_id = match opt_id {
            Some(id) => *id,
            None => continue, // root is not an album
        };

        let image_count = files_coll
            .count_documents(doc! {
                "owner_id": user.id,
                "parent_id": folder_id,
                "mime_type": { "$regex": "^image/" },
                "deleted_at": mongodb::bson::Bson::Null,
            })
            .await?;

        let cover = files_coll
            .find_one(doc! {
                "owner_id": user.id,
                "parent_id": folder_id,
                "mime_type": { "$regex": "^image/" },
                "deleted_at": mongodb::bson::Bson::Null,
            })
            .sort(doc! { "created_at": -1 })
            .await?;

        let folder = match by_id.get(&folder_id) {
            Some(f) => f,
            None => continue,
        };

        let parent_folder_id = folder
            .parent_id
            .filter(|pid| included.contains(&Some(*pid)))
            .map(|pid| pid.to_hex());

        albums.push(AlbumResponse {
            folder_id: folder_id.to_hex(),
            parent_folder_id,
            name: folder.name.clone(),
            path: build_folder_path(folder_id, &by_id),
            image_count: image_count as i64,
            cover_image_id: cover.map(|f| f.id.to_hex()),
        });
    }

    albums.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    Ok(Json(albums))
}

// --------------------------------------------------------------------------
// Tests
// --------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::parse_range_header;

    // --- parse_range_header: standard byte range ---

    #[test]
    fn range_first_500_bytes() {
        assert_eq!(parse_range_header("bytes=0-499", 1000), Some((0, 499)));
    }

    #[test]
    fn range_middle_of_file() {
        assert_eq!(parse_range_header("bytes=200-499", 1000), Some((200, 499)));
    }

    #[test]
    fn range_single_byte() {
        assert_eq!(parse_range_header("bytes=0-0", 1000), Some((0, 0)));
    }

    #[test]
    fn range_last_byte_explicit() {
        assert_eq!(parse_range_header("bytes=999-999", 1000), Some((999, 999)));
    }

    // --- parse_range_header: open-ended (bytes=500-) ---

    #[test]
    fn range_open_ended() {
        assert_eq!(parse_range_header("bytes=500-", 1000), Some((500, 999)));
    }

    #[test]
    fn range_open_ended_from_zero() {
        assert_eq!(parse_range_header("bytes=0-", 1000), Some((0, 999)));
    }

    #[test]
    fn range_open_ended_last_byte() {
        assert_eq!(parse_range_header("bytes=999-", 1000), Some((999, 999)));
    }

    // --- parse_range_header: suffix range (bytes=-N) ---

    #[test]
    fn range_suffix_last_200() {
        assert_eq!(parse_range_header("bytes=-200", 1000), Some((800, 999)));
    }

    #[test]
    fn range_suffix_last_1_byte() {
        assert_eq!(parse_range_header("bytes=-1", 1000), Some((999, 999)));
    }

    #[test]
    fn range_suffix_entire_file() {
        assert_eq!(parse_range_header("bytes=-1000", 1000), Some((0, 999)));
    }

    // --- parse_range_header: clamping ---

    #[test]
    fn range_end_clamped_to_file_size() {
        // End beyond file size should be clamped to total - 1
        assert_eq!(parse_range_header("bytes=0-99999", 100), Some((0, 99)));
    }

    #[test]
    fn range_end_clamped_partial() {
        assert_eq!(parse_range_header("bytes=50-200", 100), Some((50, 99)));
    }

    // --- parse_range_header: error cases → None ---

    #[test]
    fn range_start_beyond_file_size() {
        assert_eq!(parse_range_header("bytes=1000-1500", 1000), None);
    }

    #[test]
    fn range_start_equals_file_size() {
        assert_eq!(parse_range_header("bytes=100-", 100), None);
    }

    #[test]
    fn range_start_greater_than_end() {
        assert_eq!(parse_range_header("bytes=200-100", 1000), None);
    }

    #[test]
    fn range_suffix_zero() {
        // bytes=-0 makes no sense
        assert_eq!(parse_range_header("bytes=-0", 1000), None);
    }

    #[test]
    fn range_suffix_exceeds_file_size() {
        // bytes=-2000 on a 1000-byte file
        assert_eq!(parse_range_header("bytes=-2000", 1000), None);
    }

    #[test]
    fn range_missing_prefix() {
        assert_eq!(parse_range_header("0-499", 1000), None);
    }

    #[test]
    fn range_wrong_prefix() {
        assert_eq!(parse_range_header("chars=0-499", 1000), None);
    }

    #[test]
    fn range_garbage_input() {
        assert_eq!(parse_range_header("not-a-range", 1000), None);
    }

    #[test]
    fn range_empty_string() {
        assert_eq!(parse_range_header("", 1000), None);
    }

    #[test]
    fn range_non_numeric_start() {
        assert_eq!(parse_range_header("bytes=abc-499", 1000), None);
    }

    #[test]
    fn range_non_numeric_end() {
        assert_eq!(parse_range_header("bytes=0-xyz", 1000), None);
    }

    #[test]
    fn range_negative_values() {
        // Negative numbers won't parse as u64
        assert_eq!(parse_range_header("bytes=-1--5", 1000), None);
    }

    // --- parse_range_header: multi-range (only first is used) ---

    #[test]
    fn range_multi_range_uses_first() {
        // Implementation takes only the first range
        assert_eq!(
            parse_range_header("bytes=0-499, 600-999", 1000),
            Some((0, 499))
        );
    }

    // --- parse_range_header: edge case file sizes ---

    #[test]
    fn range_zero_size_file() {
        // No valid range on a zero-length file
        assert_eq!(parse_range_header("bytes=0-0", 0), None);
        assert_eq!(parse_range_header("bytes=0-", 0), None);
        assert_eq!(parse_range_header("bytes=-1", 0), None);
    }

    #[test]
    fn range_one_byte_file() {
        assert_eq!(parse_range_header("bytes=0-0", 1), Some((0, 0)));
        assert_eq!(parse_range_header("bytes=0-", 1), Some((0, 0)));
        assert_eq!(parse_range_header("bytes=-1", 1), Some((0, 0)));
        assert_eq!(parse_range_header("bytes=1-", 1), None);
    }
}

pub async fn get_thumbnail(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Response> {
    let file_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid file ID".to_string()))?;

    let collection = state.db.collection::<File>("files");
    let file = collection
        .find_one(doc! { "_id": file_id, "owner_id": user.id, "deleted_at": mongodb::bson::Bson::Null })
        .await?
        .ok_or_else(|| AppError::NotFound("File not found".to_string()))?;

    // Both ThumbnailProcessor (images) and AudioMetadataProcessor (audio cover art)
    // write to the same .thumbs/{id}.jpg path — check either task type.
    let thumb_task = file
        .processing_tasks
        .iter()
        .find(|t| t.task_type == TaskType::Thumbnail || t.task_type == TaskType::AudioMetadata);

    match thumb_task.map(|t| &t.status) {
        Some(ProcessingStatus::Done) => {
            let backend = state.storage.get_backend(file.storage_id).await?;
            let thumb_path = format!(".thumbs/{}.jpg", file_id.to_hex());
            let reader = backend.read(&thumb_path).await?;
            let stream = ReaderStream::new(reader);
            let body = Body::from_stream(stream);
            Ok(Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "image/jpeg")
                .body(body)
                .unwrap())
        }
        Some(ProcessingStatus::Pending) => Ok(Response::builder()
            .status(StatusCode::ACCEPTED)
            .body(Body::empty())
            .unwrap()),
        _ => Err(AppError::NotFound("Thumbnail not available".to_string())),
    }
}
