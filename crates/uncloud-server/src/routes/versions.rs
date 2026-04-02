use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use mongodb::bson::{self, doc, oid::ObjectId};
use std::sync::Arc;
use tokio_util::io::ReaderStream;

use crate::error::{AppError, Result};
use crate::middleware::AuthUser;
use crate::models::{File, FileVersion};
use crate::AppState;
use uncloud_common::FileVersionResponse;

/// GET /api/files/{id}/versions — list all versions of a file.
pub async fn list_versions(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Vec<FileVersionResponse>>> {
    let file_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid file ID".to_string()))?;

    // Verify the file belongs to this user and is not deleted
    let files_coll = state.db.collection::<File>("files");
    files_coll
        .find_one(doc! { "_id": file_id, "owner_id": user.id, "deleted_at": bson::Bson::Null })
        .await?
        .ok_or_else(|| AppError::NotFound("File not found".to_string()))?;

    let versions_coll = state.db.collection::<FileVersion>("file_versions");
    let options = mongodb::options::FindOptions::builder()
        .sort(doc! { "version": -1 })
        .build();
    let mut cursor = versions_coll
        .find(doc! { "file_id": file_id })
        .with_options(options)
        .await?;

    let mut versions = Vec::new();
    while cursor.advance().await? {
        let ver: FileVersion = cursor.deserialize_current()?;
        versions.push(FileVersionResponse {
            id: ver.id.to_hex(),
            version: ver.version,
            size_bytes: ver.size_bytes,
            checksum_sha256: ver.checksum_sha256,
            created_at: ver.created_at.to_rfc3339(),
        });
    }

    Ok(Json(versions))
}

/// GET /api/files/{file_id}/versions/{version_id} — download a specific version.
pub async fn download_version(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path((file_id_str, version_id_str)): Path<(String, String)>,
) -> Result<Response> {
    let file_id = ObjectId::parse_str(&file_id_str)
        .map_err(|_| AppError::BadRequest("Invalid file ID".to_string()))?;
    let version_id = ObjectId::parse_str(&version_id_str)
        .map_err(|_| AppError::BadRequest("Invalid version ID".to_string()))?;

    // Verify ownership
    let files_coll = state.db.collection::<File>("files");
    let file = files_coll
        .find_one(doc! { "_id": file_id, "owner_id": user.id, "deleted_at": bson::Bson::Null })
        .await?
        .ok_or_else(|| AppError::NotFound("File not found".to_string()))?;

    let versions_coll = state.db.collection::<FileVersion>("file_versions");
    let version = versions_coll
        .find_one(doc! { "_id": version_id, "file_id": file_id })
        .await?
        .ok_or_else(|| AppError::NotFound("Version not found".to_string()))?;

    let backend = state.storage.get_backend(file.storage_id).await?;
    let reader = backend.read(&version.storage_path).await?;
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
        .header(header::CONTENT_LENGTH, version.size_bytes)
        .body(body)
        .unwrap())
}

/// POST /api/files/{file_id}/versions/{version_id}/restore — promote a version to current.
pub async fn restore_version(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path((file_id_str, version_id_str)): Path<(String, String)>,
) -> Result<StatusCode> {
    let file_id = ObjectId::parse_str(&file_id_str)
        .map_err(|_| AppError::BadRequest("Invalid file ID".to_string()))?;
    let version_id = ObjectId::parse_str(&version_id_str)
        .map_err(|_| AppError::BadRequest("Invalid version ID".to_string()))?;

    let files_coll = state.db.collection::<File>("files");
    let file = files_coll
        .find_one(doc! { "_id": file_id, "owner_id": user.id, "deleted_at": bson::Bson::Null })
        .await?
        .ok_or_else(|| AppError::NotFound("File not found".to_string()))?;

    let versions_coll = state.db.collection::<FileVersion>("file_versions");
    let version = versions_coll
        .find_one(doc! { "_id": version_id, "file_id": file_id })
        .await?
        .ok_or_else(|| AppError::NotFound("Version not found".to_string()))?;

    let backend = state.storage.get_backend(file.storage_id).await?;

    // Archive the current content as a new version
    let ver_path = crate::routes::files::version_path(&file.storage_path);
    backend.archive_version(&file.storage_path, &ver_path).await?;

    let next_version = versions_coll
        .count_documents(doc! { "file_id": file_id })
        .await? as i32
        + 1;

    let new_version_record = FileVersion::new(
        file_id,
        next_version,
        ver_path,
        file.size_bytes,
        file.checksum_sha256.clone(),
    );
    versions_coll.insert_one(&new_version_record).await?;

    // Copy the old version blob over the current path
    backend.restore_from_trash(&version.storage_path, &file.storage_path).await?;

    // Update file metadata
    let now = chrono::Utc::now();
    let size_delta = version.size_bytes - file.size_bytes;
    files_coll
        .update_one(
            doc! { "_id": file_id },
            doc! { "$set": {
                "size_bytes": version.size_bytes,
                "checksum_sha256": &version.checksum_sha256,
                "updated_at": bson::DateTime::from_chrono(now),
                "processing_tasks": [],
            }},
        )
        .await?;

    if size_delta != 0 {
        state.auth.update_user_bytes(user.id, size_delta).await?;
    }

    // Re-enqueue processing for the restored content
    if let Some(restored) = files_coll.find_one(doc! { "_id": file_id }).await? {
        // Remove stale thumbnail
        let _ = backend.delete(&format!(".thumbs/{}.jpg", file_id.to_hex())).await;
        state.processing.enqueue(&restored, state.clone()).await;
    }

    state.events.emit_file_updated(user.id, &file).await;

    Ok(StatusCode::OK)
}
