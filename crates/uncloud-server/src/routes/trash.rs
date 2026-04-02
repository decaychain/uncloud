use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use mongodb::bson::{self, doc, oid::ObjectId};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashSet;
use std::sync::Arc;

use crate::error::{AppError, Result};
use crate::middleware::AuthUser;
use crate::models::{File, FileVersion, Folder};
use crate::routes::files::{check_name_conflict, file_to_response, resolve_storage_path};
use crate::AppState;
use uncloud_common::TrashItemResponse;

#[derive(Deserialize, Default)]
pub struct RestoreRequest {
    pub name: Option<String>,
}

/// Increment the counter suffix: "foo.txt" -> "foo (1).txt", "foo (1).txt" -> "foo (2).txt".
fn bump_name(name: &str) -> String {
    let (base, ext) = match name.rfind('.') {
        Some(dot) => (&name[..dot], &name[dot..]),
        None => (name, ""),
    };
    if let Some(open) = base.rfind(" (") {
        let inner = &base[open + 2..];
        if inner.ends_with(')') {
            if let Ok(n) = inner[..inner.len() - 1].parse::<u32>() {
                return format!("{} ({}){}", &base[..open], n + 1, ext);
            }
        }
    }
    format!("{} (1){}", base, ext)
}

/// Find the first available name at the given location by querying the DB.
async fn suggest_name(
    db: &mongodb::Database,
    owner_id: ObjectId,
    parent_id: Option<ObjectId>,
    name: &str,
    exclude_file: Option<ObjectId>,
    exclude_folder: Option<ObjectId>,
) -> Result<String> {
    let mut candidate = bump_name(name);
    loop {
        if !check_name_conflict(db, owner_id, parent_id, &candidate, exclude_file, exclude_folder).await? {
            return Ok(candidate);
        }
        candidate = bump_name(&candidate);
    }
}

/// GET /api/trash — list top-level soft-deleted files and folders for the current user.
///
/// When a folder is deleted, all its contents are soft-deleted together with
/// a shared `batch_delete_id`. This handler filters out children whose parent
/// is also soft-deleted, so the user sees one folder in the trash rather than
/// hundreds of individual files.
pub async fn list_trash(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<TrashItemResponse>>> {
    let mut items = Vec::new();

    let folders_coll = state.db.collection::<Folder>("folders");

    // Collect IDs of all soft-deleted folders so we can filter out children
    let mut deleted_folder_ids = HashSet::new();
    let mut del_folder_cursor = folders_coll
        .find(doc! { "owner_id": user.id, "deleted_at": { "$ne": bson::Bson::Null } })
        .await?;
    let mut deleted_folders: Vec<Folder> = Vec::new();
    while del_folder_cursor.advance().await? {
        let f: Folder = del_folder_cursor.deserialize_current()?;
        deleted_folder_ids.insert(f.id);
        deleted_folders.push(f);
    }

    // Soft-deleted files — only include top-level (parent not also deleted)
    let files_coll = state.db.collection::<File>("files");
    let mut file_cursor = files_coll
        .find(doc! { "owner_id": user.id, "deleted_at": { "$ne": bson::Bson::Null } })
        .await?;
    while file_cursor.advance().await? {
        let f: File = file_cursor.deserialize_current()?;
        if let Some(deleted_at) = f.deleted_at {
            // Skip if parent folder is also soft-deleted
            let is_top_level = match f.parent_id {
                None => true,
                Some(pid) => !deleted_folder_ids.contains(&pid),
            };
            if !is_top_level {
                continue;
            }
            items.push(TrashItemResponse {
                id: f.id.to_hex(),
                name: f.name,
                is_folder: false,
                mime_type: Some(f.mime_type),
                size_bytes: Some(f.size_bytes),
                original_path: Some(f.storage_path),
                parent_id: f.parent_id.map(|pid| pid.to_hex()),
                deleted_at: deleted_at.to_rfc3339(),
                batch_delete_id: f.batch_delete_id,
            });
        }
    }

    // Soft-deleted folders — only include top-level (parent not also deleted)
    for f in deleted_folders {
        if let Some(deleted_at) = f.deleted_at {
            let is_top_level = match f.parent_id {
                None => true,
                Some(pid) => !deleted_folder_ids.contains(&pid),
            };
            if !is_top_level {
                continue;
            }
            items.push(TrashItemResponse {
                id: f.id.to_hex(),
                name: f.name,
                is_folder: true,
                mime_type: None,
                size_bytes: None,
                original_path: None,
                parent_id: f.parent_id.map(|pid| pid.to_hex()),
                deleted_at: deleted_at.to_rfc3339(),
                batch_delete_id: f.batch_delete_id,
            });
        }
    }

    // Sort by most recently deleted first
    items.sort_by(|a, b| b.deleted_at.cmp(&a.deleted_at));

    Ok(Json(items))
}

/// POST /api/trash/{id}/restore — restore a soft-deleted file or folder.
///
/// Accepts an optional JSON body `{ "name": "new name" }` to rename the item
/// during restore (used for conflict resolution).
///
/// On name conflict returns 409 with `{ "error": "CONFLICT", "suggest": "name (1).ext" }`.
pub async fn restore_from_trash(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    body: Option<Json<RestoreRequest>>,
) -> Result<axum::response::Response> {
    let item_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid ID".to_string()))?;

    let rename_to = body.and_then(|b| b.0.name).filter(|n| !n.is_empty());

    // Try file first
    let files_coll = state.db.collection::<File>("files");
    if let Some(file) = files_coll
        .find_one(doc! { "_id": item_id, "owner_id": user.id, "deleted_at": { "$ne": bson::Bson::Null } })
        .await?
    {
        let restore_name = rename_to.as_deref().unwrap_or(&file.name);

        // If the parent folder is soft-deleted, restore the ancestor chain first
        if let Some(parent_id) = file.parent_id {
            restore_ancestor_chain(&state, user.id, parent_id).await?;
        }

        // Check name conflict at original location
        if check_name_conflict(
            &state.db,
            user.id,
            file.parent_id,
            restore_name,
            Some(file.id),
            None,
        )
        .await?
        {
            let suggested = suggest_name(&state.db, user.id, file.parent_id, restore_name, Some(file.id), None).await?;
            return Ok((
                StatusCode::CONFLICT,
                Json(json!({ "error": "CONFLICT", "suggest": suggested })),
            ).into_response());
        }

        // Compute new storage path if the name changed
        let new_storage_path = if rename_to.is_some() {
            Some(resolve_storage_path(&state.db, user.id, &user.username, file.parent_id, restore_name).await?)
        } else {
            None
        };

        let restore_path = new_storage_path.as_deref().unwrap_or(&file.storage_path);

        // Restore blob from trash
        if let Some(ref tp) = file.trash_path {
            if let Ok(backend) = state.storage.get_backend(file.storage_id).await {
                backend.restore_from_trash(tp, restore_path).await?;
            }
        }

        // Clear soft-delete fields (and update name + storage_path if renamed)
        let mut update_doc = doc! {
            "deleted_at": bson::Bson::Null,
            "trash_path": bson::Bson::Null,
            "batch_delete_id": bson::Bson::Null,
        };
        if let Some(ref new_name) = rename_to {
            update_doc.insert("name", new_name.as_str());
        }
        if let Some(ref sp) = new_storage_path {
            update_doc.insert("storage_path", sp.as_str());
        }
        files_coll
            .update_one(
                doc! { "_id": item_id },
                doc! { "$set": update_doc },
            )
            .await?;

        // Re-enqueue processing (e.g. rebuild thumbnail)
        let restored = files_coll.find_one(doc! { "_id": item_id }).await?.unwrap();
        state.processing.enqueue(&restored, state.clone()).await;
        state.events.emit_file_restored(user.id, item_id).await;

        return Ok(StatusCode::OK.into_response());
    }

    // Try folder
    let folders_coll = state.db.collection::<Folder>("folders");
    if let Some(folder) = folders_coll
        .find_one(doc! { "_id": item_id, "owner_id": user.id, "deleted_at": { "$ne": bson::Bson::Null } })
        .await?
    {
        let restore_name = rename_to.as_deref().unwrap_or(&folder.name);

        // If the parent folder is soft-deleted, restore the ancestor chain first
        if let Some(parent_id) = folder.parent_id {
            restore_ancestor_chain(&state, user.id, parent_id).await?;
        }

        // Check name conflict
        if check_name_conflict(
            &state.db,
            user.id,
            folder.parent_id,
            restore_name,
            None,
            Some(folder.id),
        )
        .await?
        {
            let suggested = suggest_name(&state.db, user.id, folder.parent_id, restore_name, None, Some(folder.id)).await?;
            return Ok((
                StatusCode::CONFLICT,
                Json(json!({ "error": "CONFLICT", "suggest": suggested })),
            ).into_response());
        }

        // Update folder name if renamed
        if let Some(ref new_name) = rename_to {
            folders_coll
                .update_one(
                    doc! { "_id": item_id },
                    doc! { "$set": { "name": new_name.as_str() } },
                )
                .await?;
        }

        // Restore the folder and all its contents recursively
        restore_folder_recursive(&state, user.id, item_id).await?;

        state.events.emit_file_restored(user.id, item_id).await;

        return Ok(StatusCode::OK.into_response());
    }

    Err(AppError::NotFound("Item not found in trash".to_string()))
}

/// Recursively restore a soft-deleted folder and all its children.
async fn restore_folder_recursive(
    state: &Arc<AppState>,
    user_id: ObjectId,
    folder_id: ObjectId,
) -> Result<()> {
    let folders_coll = state.db.collection::<Folder>("folders");
    let files_coll = state.db.collection::<File>("files");

    // Restore this folder
    folders_coll
        .update_one(
            doc! { "_id": folder_id },
            doc! { "$set": { "deleted_at": bson::Bson::Null, "batch_delete_id": bson::Bson::Null } },
        )
        .await?;

    // Restore files in this folder (only those that were deleted, i.e. have a trash_path)
    let mut file_cursor = files_coll
        .find(doc! { "owner_id": user_id, "parent_id": folder_id, "deleted_at": { "$ne": bson::Bson::Null } })
        .await?;

    while file_cursor.advance().await? {
        let file: File = file_cursor.deserialize_current()?;
        if let Some(ref tp) = file.trash_path {
            if let Ok(backend) = state.storage.get_backend(file.storage_id).await {
                let _ = backend.restore_from_trash(tp, &file.storage_path).await;
            }
        }
        files_coll
            .update_one(
                doc! { "_id": file.id },
                doc! { "$set": {
                    "deleted_at": bson::Bson::Null,
                    "trash_path": bson::Bson::Null,
                    "batch_delete_id": bson::Bson::Null,
                }},
            )
            .await?;

        // Re-enqueue processing
        if let Some(restored) = files_coll.find_one(doc! { "_id": file.id }).await? {
            state.processing.enqueue(&restored, state.clone()).await;
        }
    }

    // Recurse into subfolders
    let mut sf_cursor = folders_coll
        .find(doc! { "owner_id": user_id, "parent_id": folder_id, "deleted_at": { "$ne": bson::Bson::Null } })
        .await?;
    let mut subfolder_ids = Vec::new();
    while sf_cursor.advance().await? {
        let sf: Folder = sf_cursor.deserialize_current()?;
        subfolder_ids.push(sf.id);
    }
    for sf_id in subfolder_ids {
        Box::pin(restore_folder_recursive(state, user_id, sf_id)).await?;
    }

    Ok(())
}

/// Walk up the folder tree, restoring any soft-deleted ancestors.
/// This ensures that when restoring an orphaned file (whose parent folder was
/// deleted), the entire folder chain is recreated first.
async fn restore_ancestor_chain(
    state: &Arc<AppState>,
    user_id: ObjectId,
    folder_id: ObjectId,
) -> Result<()> {
    let folders_coll = state.db.collection::<Folder>("folders");
    let folder = folders_coll
        .find_one(doc! { "_id": folder_id, "owner_id": user_id })
        .await?;

    let Some(folder) = folder else {
        // Folder doesn't exist at all — nothing to restore
        return Ok(());
    };

    if folder.deleted_at.is_none() {
        // Already alive — base case
        return Ok(());
    }

    // Restore parent first (if it exists and is deleted)
    if let Some(parent_id) = folder.parent_id {
        Box::pin(restore_ancestor_chain(state, user_id, parent_id)).await?;
    }

    // Now restore this folder (just the folder record, not its contents)
    folders_coll
        .update_one(
            doc! { "_id": folder_id },
            doc! { "$set": {
                "deleted_at": bson::Bson::Null,
                "batch_delete_id": bson::Bson::Null,
            }},
        )
        .await?;

    Ok(())
}

/// DELETE /api/trash/{id} — permanently delete a trashed item.
pub async fn permanently_delete(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let item_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid ID".to_string()))?;

    // Try file first
    let files_coll = state.db.collection::<File>("files");
    if let Some(file) = files_coll
        .find_one(doc! { "_id": item_id, "owner_id": user.id, "deleted_at": { "$ne": bson::Bson::Null } })
        .await?
    {
        // Delete blob from trash
        if let Ok(backend) = state.storage.get_backend(file.storage_id).await {
            if let Some(ref tp) = file.trash_path {
                let _ = backend.delete(tp).await;
            }
        }

        // Delete all versions
        let versions_coll = state.db.collection::<FileVersion>("file_versions");
        let mut ver_cursor = versions_coll
            .find(doc! { "file_id": item_id })
            .await?;
        while ver_cursor.advance().await? {
            let ver: FileVersion = ver_cursor.deserialize_current()?;
            if let Ok(backend) = state.storage.get_backend(file.storage_id).await {
                let _ = backend.delete(&ver.storage_path).await;
            }
        }
        versions_coll.delete_many(doc! { "file_id": item_id }).await?;

        // Delete DB record
        files_coll.delete_one(doc! { "_id": item_id }).await?;

        // Update user's used bytes (quota released on permanent purge)
        state.auth.update_user_bytes(user.id, -file.size_bytes).await?;

        return Ok(StatusCode::NO_CONTENT);
    }

    // Try folder
    let folders_coll = state.db.collection::<Folder>("folders");
    if folders_coll
        .find_one(doc! { "_id": item_id, "owner_id": user.id, "deleted_at": { "$ne": bson::Bson::Null } })
        .await?
        .is_some()
    {
        // Permanently delete folder and its contents recursively
        permanently_delete_folder_recursive(&state, user.id, item_id).await?;

        return Ok(StatusCode::NO_CONTENT);
    }

    Err(AppError::NotFound("Item not found in trash".to_string()))
}

async fn permanently_delete_folder_recursive(
    state: &AppState,
    user_id: ObjectId,
    folder_id: ObjectId,
) -> Result<()> {
    let folders_coll = state.db.collection::<Folder>("folders");
    let files_coll = state.db.collection::<File>("files");
    let versions_coll = state.db.collection::<FileVersion>("file_versions");

    // Permanently delete files in this folder
    let mut cursor = files_coll
        .find(doc! { "owner_id": user_id, "parent_id": folder_id })
        .await?;

    let mut total_size = 0i64;
    let mut file_ids = Vec::new();
    while cursor.advance().await? {
        let file: File = cursor.deserialize_current()?;
        if let Ok(backend) = state.storage.get_backend(file.storage_id).await {
            if let Some(ref tp) = file.trash_path {
                let _ = backend.delete(tp).await;
            } else {
                let _ = backend.delete(&file.storage_path).await;
            }
        }
        total_size += file.size_bytes;
        file_ids.push(file.id);

        // Delete versions
        let mut ver_cursor = versions_coll
            .find(doc! { "file_id": file.id })
            .await?;
        while ver_cursor.advance().await? {
            let ver: FileVersion = ver_cursor.deserialize_current()?;
            if let Ok(backend) = state.storage.get_backend(file.storage_id).await {
                let _ = backend.delete(&ver.storage_path).await;
            }
        }
        versions_coll.delete_many(doc! { "file_id": file.id }).await?;
    }

    if !file_ids.is_empty() {
        let file_bson_ids: Vec<bson::Bson> = file_ids.iter().map(|id| bson::Bson::ObjectId(*id)).collect();
        files_coll
            .delete_many(doc! { "_id": { "$in": &file_bson_ids } })
            .await?;
    }

    if total_size > 0 {
        state.auth.update_user_bytes(user_id, -total_size).await?;
    }

    // Recurse into subfolders
    let mut sf_cursor = folders_coll
        .find(doc! { "owner_id": user_id, "parent_id": folder_id })
        .await?;
    let mut subfolder_ids = Vec::new();
    while sf_cursor.advance().await? {
        let sf: Folder = sf_cursor.deserialize_current()?;
        subfolder_ids.push(sf.id);
    }
    for sf_id in subfolder_ids {
        Box::pin(permanently_delete_folder_recursive(state, user_id, sf_id)).await?;
    }

    // Delete this folder record
    folders_coll.delete_one(doc! { "_id": folder_id }).await?;

    Ok(())
}

/// DELETE /api/trash — empty the entire trash for the current user.
pub async fn empty_trash(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<StatusCode> {
    let files_coll = state.db.collection::<File>("files");
    let folders_coll = state.db.collection::<Folder>("folders");
    let versions_coll = state.db.collection::<FileVersion>("file_versions");

    // Find all trashed files
    let mut cursor = files_coll
        .find(doc! { "owner_id": user.id, "deleted_at": { "$ne": bson::Bson::Null } })
        .await?;

    let mut total_size = 0i64;
    let mut file_ids = Vec::new();
    while cursor.advance().await? {
        let file: File = cursor.deserialize_current()?;
        if let Ok(backend) = state.storage.get_backend(file.storage_id).await {
            if let Some(ref tp) = file.trash_path {
                let _ = backend.delete(tp).await;
            }
        }
        total_size += file.size_bytes;
        file_ids.push(file.id);

        // Delete versions
        versions_coll.delete_many(doc! { "file_id": file.id }).await?;
    }

    // Delete all trashed file records
    files_coll
        .delete_many(doc! { "owner_id": user.id, "deleted_at": { "$ne": bson::Bson::Null } })
        .await?;

    // Delete all trashed folder records
    folders_coll
        .delete_many(doc! { "owner_id": user.id, "deleted_at": { "$ne": bson::Bson::Null } })
        .await?;

    // Update quota
    if total_size > 0 {
        state.auth.update_user_bytes(user.id, -total_size).await?;
    }

    Ok(StatusCode::NO_CONTENT)
}
