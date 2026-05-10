//! `write_file` — create a new text file or overwrite an existing one.
//!
//! Reuses the existing helpers in `routes::files` (quota checks,
//! storage-path resolution, name-conflict detection) but is wired up
//! directly against the storage backend + collections rather than the
//! REST handler, because the REST handler is multipart-only and
//! constructing a synthetic multipart body just to call it would be
//! ceremony with no upside.
//!
//! v1 deliberately omits the share-aware ownership branching of the
//! REST upload — MCP write tools only operate on the bearer's own tree.
//! Sharing-via-MCP can be a future scope.

use std::sync::Arc;

use mongodb::bson::{self, doc, oid::ObjectId, Bson};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::mcp::path;
use crate::middleware::auth::AuthUser;
use crate::models::file::{File, FileVersion};
use crate::models::User;
use crate::routes::audit;
use crate::routes::files::{check_name_conflict, resolve_storage_path, version_path};
use crate::AppState;

use super::{mcp_request_meta, ToolError};

const MAX_CONTENT_BYTES: usize = 1_048_576; // 1 MiB

pub fn input_schema() -> Value {
    json!({
        "type": "object",
        "required": ["path", "content"],
        "properties": {
            "path": {
                "type": "string",
                "description": "Absolute, case-sensitive file path. Parent folder must already exist."
            },
            "content": {
                "type": "string",
                "description": "UTF-8 text content. Hard cap 1 MiB."
            },
            "overwrite": {
                "type": "boolean",
                "default": false,
                "description": "If true, replace existing file (creates a version of the previous content). If false (default), the call fails when the path already exists."
            },
            "mime_type": {
                "type": "string",
                "description": "Optional override; defaults to mime_guess from filename."
            }
        }
    })
}

pub async fn call(
    args: &Value,
    state: &Arc<AppState>,
    user: &AuthUser,
) -> Result<Value, ToolError> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::invalid("path is required"))?;
    let content = args
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::invalid("content is required"))?;
    if content.len() > MAX_CONTENT_BYTES {
        return Err(ToolError::invalid(format!(
            "content exceeds 1 MiB cap ({} bytes)",
            content.len()
        )));
    }
    let overwrite = args
        .get("overwrite")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mime_override = args
        .get("mime_type")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let segments = path::parse(path_str)?;
    let (filename, parent_segments) = segments
        .split_last()
        .ok_or_else(|| ToolError::invalid("file path must include a filename"))?;

    let parent = path::resolve_folder(state, user.0.id, parent_segments).await?;
    let parent_id: Option<ObjectId> = parent.as_ref().map(|f| f.id);

    let owner_id = user.0.id;
    let username = user.0.username.clone();
    let bytes = content.as_bytes().to_vec();
    let size = bytes.len() as i64;
    let mime_type = mime_override.unwrap_or_else(|| {
        mime_guess::from_path(filename)
            .first_or_octet_stream()
            .to_string()
    });

    // Find any existing live file at this path.
    let files_coll = state.db.collection::<File>("files");
    let existing = files_coll
        .find_one(doc! {
            "owner_id": owner_id,
            "parent_id": match parent_id {
                Some(pid) => Bson::ObjectId(pid),
                None => Bson::Null,
            },
            "name": filename.as_str(),
            "deleted_at": Bson::Null,
        })
        .await
        .map_err(|e| ToolError::exec(format!("file lookup failed: {}", e)))?;

    if existing.is_some() && !overwrite {
        return Err(ToolError::exec(format!(
            "{} already exists; pass overwrite: true to replace",
            path_str
        )));
    }

    // Quota check against the user.
    {
        let users_coll = state.db.collection::<User>("users");
        if let Some(owner) = users_coll
            .find_one(doc! { "_id": owner_id })
            .await
            .map_err(|e| ToolError::exec(format!("user lookup failed: {}", e)))?
        {
            let delta = match &existing {
                Some(e) => size - e.size_bytes,
                None => size,
            };
            if delta > 0 && !owner.has_quota_space(delta) {
                return Err(ToolError::exec("Quota exceeded"));
            }
        }
    }

    let meta = mcp_request_meta();
    let resolved_path = format!(
        "/{}",
        segments.join("/")
    );

    if let Some(file) = existing {
        // ---------- overwrite path: snapshot existing version, then replace ----------
        let backend = state
            .storage
            .get_backend(file.storage_id)
            .await
            .map_err(|e| ToolError::exec(format!("storage backend unavailable: {}", e)))?;

        let versions_coll = state.db.collection::<FileVersion>("file_versions");
        let version_number = versions_coll
            .count_documents(doc! { "file_id": file.id })
            .await
            .map_err(|e| ToolError::exec(format!("version count failed: {}", e)))? as i32
            + 1;

        let ver_path = version_path(&file.storage_path);
        backend
            .archive_version(&file.storage_path, &ver_path)
            .await
            .map_err(|e| ToolError::exec(format!("archive previous version failed: {}", e)))?;

        let file_version = FileVersion::new(
            file.id,
            version_number,
            ver_path,
            file.size_bytes,
            file.checksum_sha256.clone(),
        );
        versions_coll
            .insert_one(&file_version)
            .await
            .map_err(|e| ToolError::exec(format!("version insert failed: {}", e)))?;

        let new_checksum = hex::encode(Sha256::digest(&bytes));
        backend
            .write(&file.storage_path, &bytes)
            .await
            .map_err(|e| ToolError::exec(format!("storage write failed: {}", e)))?;

        let now = chrono::Utc::now();
        files_coll
            .update_one(
                doc! { "_id": file.id },
                doc! { "$set": {
                    "size_bytes": size,
                    "checksum_sha256": &new_checksum,
                    "mime_type": &mime_type,
                    "updated_at": bson::DateTime::from_chrono(now),
                    "processing_tasks": [],
                } },
            )
            .await
            .map_err(|e| ToolError::exec(format!("file update failed: {}", e)))?;

        let size_delta = size - file.size_bytes;
        if size_delta != 0 {
            state
                .auth
                .update_user_bytes(owner_id, size_delta)
                .await
                .map_err(|e| ToolError::exec(format!("quota update failed: {}", e)))?;
        }

        let _ = backend
            .delete(&format!(".thumbs/{}.jpg", file.id.to_hex()))
            .await;

        let updated = File {
            id: file.id,
            storage_id: file.storage_id,
            storage_path: file.storage_path.clone(),
            owner_id,
            parent_id: file.parent_id,
            name: file.name.clone(),
            mime_type: mime_type.clone(),
            size_bytes: size,
            checksum_sha256: new_checksum,
            created_at: file.created_at,
            updated_at: now,
            captured_at: None,
            processing_tasks: vec![],
            metadata: std::collections::HashMap::new(),
            deleted_at: None,
            trash_path: None,
            batch_delete_id: None,
        };
        state.events.emit_file_created(owner_id, &updated).await;
        state
            .sync_log
            .record(audit::file_event(
                owner_id,
                uncloud_common::SyncOperation::ContentReplaced,
                updated.id,
                updated.storage_path.clone(),
                None,
                &meta,
            ))
            .await;
        state.processing.enqueue(&updated, state.clone()).await;

        return Ok(json!({
            "id": updated.id.to_hex(),
            "path": resolved_path,
            "name": updated.name,
            "mime_type": updated.mime_type,
            "size_bytes": updated.size_bytes,
            "overwrote": true,
        }));
    }

    // ---------- new-file path ----------

    // Defensive duplicate check (the partial unique index is the
    // authoritative guard but this gives a clean error before storage).
    if check_name_conflict(&state.db, owner_id, parent_id, filename, None, None)
        .await
        .map_err(|e| ToolError::exec(format!("dupe check failed: {}", e)))?
    {
        return Err(ToolError::exec(format!(
            "{} already exists",
            path_str
        )));
    }

    let storage_id = state
        .storage
        .resolve_storage_for_parent(parent_id)
        .await
        .map_err(|e| ToolError::exec(format!("storage selection failed: {}", e)))?;
    let storage = state
        .storage
        .get_storage(storage_id)
        .await
        .map_err(|e| ToolError::exec(format!("storage lookup failed: {}", e)))?;
    let backend = state
        .storage
        .get_backend(storage.id)
        .await
        .map_err(|e| ToolError::exec(format!("storage backend unavailable: {}", e)))?;

    let storage_path = resolve_storage_path(&state.db, owner_id, &username, parent_id, filename)
        .await
        .map_err(|e| ToolError::exec(format!("storage path resolve failed: {}", e)))?;

    let checksum = hex::encode(Sha256::digest(&bytes));
    backend
        .write(&storage_path, &bytes)
        .await
        .map_err(|e| ToolError::exec(format!("storage write failed: {}", e)))?;

    let new_file = File::new(
        storage.id,
        storage_path.clone(),
        owner_id,
        parent_id,
        filename.clone(),
        mime_type.clone(),
        size,
        checksum,
    );

    files_coll
        .insert_one(&new_file)
        .await
        .map_err(|e| ToolError::exec(format!("file insert failed: {}", e)))?;

    state
        .auth
        .update_user_bytes(owner_id, size)
        .await
        .map_err(|e| ToolError::exec(format!("quota update failed: {}", e)))?;
    state.events.emit_file_created(owner_id, &new_file).await;
    state
        .sync_log
        .record(audit::file_event(
            owner_id,
            uncloud_common::SyncOperation::Created,
            new_file.id,
            new_file.storage_path.clone(),
            None,
            &meta,
        ))
        .await;
    state.processing.enqueue(&new_file, state.clone()).await;

    Ok(json!({
        "id": new_file.id.to_hex(),
        "path": resolved_path,
        "name": new_file.name,
        "mime_type": new_file.mime_type,
        "size_bytes": new_file.size_bytes,
        "overwrote": false,
    }))
}
