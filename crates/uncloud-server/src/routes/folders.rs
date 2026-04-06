use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::Utc;
use mongodb::{Database, bson::{self, doc, oid::ObjectId}};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use uncloud_common::{EffectiveStrategyResponse, GalleryInclude, InheritableSetting, MusicInclude, SyncStrategy};

use crate::error::{AppError, Result};
use crate::middleware::AuthUser;
use crate::models::{File, Folder, FolderShare, User};
use crate::routes::files::{check_name_conflict, resolve_storage_path};
use crate::services::sharing::check_folder_access;
use crate::AppState;

/// Look up a username by user ID.
async fn get_username(db: &Database, user_id: ObjectId) -> Result<String> {
    let users_coll = db.collection::<User>("users");
    let user = users_coll
        .find_one(doc! { "_id": user_id })
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;
    Ok(user.username)
}

#[derive(Debug, Deserialize)]
pub struct ListFoldersQuery {
    pub parent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateFolderRequest {
    pub name: String,
    pub parent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateFolderRequest {
    pub name: Option<String>,
    pub parent_id: Option<String>,
    pub sync_strategy: Option<SyncStrategy>,
    pub gallery_include: Option<GalleryInclude>,
    pub music_include: Option<MusicInclude>,
}

#[derive(Debug, Serialize)]
pub struct FolderResponse {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub sync_strategy: SyncStrategy,
    pub effective_strategy: SyncStrategy,
    pub gallery_include: GalleryInclude,
    pub effective_gallery_include: GalleryInclude,
    pub music_include: MusicInclude,
    pub effective_music_include: MusicInclude,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_by: Option<String>,
}

/// Walk the parent chain to find the first non-Inherit value for a folder setting.
/// Returns `(effective_value, source_folder_id)` where `source_folder_id` is the
/// folder where the value is explicitly set (`None` = system default / root).
async fn resolve_setting<T>(
    db: &Database,
    user_id: ObjectId,
    folder: &Folder,
    get: fn(&Folder) -> T,
) -> Result<(T, Option<ObjectId>)>
where
    T: InheritableSetting,
{
    let value = get(folder);
    if !value.is_inherit() {
        return Ok((value, Some(folder.id)));
    }

    let collection = db.collection::<Folder>("folders");
    let mut current_parent_id = folder.parent_id;

    while let Some(pid) = current_parent_id {
        let parent = collection
            .find_one(doc! { "_id": pid, "owner_id": user_id })
            .await?
            .ok_or_else(|| AppError::NotFound("Parent folder not found".to_string()))?;

        let pval = get(&parent);
        if !pval.is_inherit() {
            return Ok((pval, Some(parent.id)));
        }
        current_parent_id = parent.parent_id;
    }

    Ok((T::root_default(), None))
}

async fn folder_to_response(
    db: &Database,
    user_id: ObjectId,
    folder: &Folder,
) -> Result<FolderResponse> {
    folder_to_response_with_shared(db, user_id, folder, None).await
}

async fn folder_to_response_with_shared(
    db: &Database,
    user_id: ObjectId,
    folder: &Folder,
    shared_by: Option<String>,
) -> Result<FolderResponse> {
    // For resolve_setting, use the folder's owner_id so parent chain lookups succeed
    // even when the caller is a share grantee (not the owner).
    let resolve_id = folder.owner_id;
    let (effective, _source) = resolve_setting(db, resolve_id, folder, |f| f.sync_strategy).await?;
    let (effective_gallery, _) = resolve_setting(db, resolve_id, folder, |f| f.gallery_include).await?;
    let (effective_music, _) = resolve_setting(db, resolve_id, folder, |f| f.music_include).await?;
    Ok(FolderResponse {
        id: folder.id.to_hex(),
        name: folder.name.clone(),
        parent_id: folder.parent_id.map(|id| id.to_hex()),
        created_at: folder.created_at.to_rfc3339(),
        updated_at: folder.updated_at.to_rfc3339(),
        sync_strategy: folder.sync_strategy,
        effective_strategy: effective,
        gallery_include: folder.gallery_include,
        effective_gallery_include: effective_gallery,
        music_include: folder.music_include,
        effective_music_include: effective_music,
        shared_by,
    })
}

pub async fn list_folders(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(query): Query<ListFoldersQuery>,
) -> Result<Json<Vec<FolderResponse>>> {
    let parent_id = match &query.parent_id {
        Some(id) if !id.is_empty() => Some(
            ObjectId::parse_str(id)
                .map_err(|_| AppError::BadRequest("Invalid parent ID".to_string()))?,
        ),
        _ => None,
    };

    let collection = state.db.collection::<Folder>("folders");

    // Determine the effective owner_id for the query.
    // If parent_id is set and the user doesn't own it, check share access.
    let effective_owner_id = if let Some(pid) = parent_id {
        let parent = collection
            .find_one(doc! { "_id": pid, "deleted_at": bson::Bson::Null })
            .await?
            .ok_or_else(|| AppError::NotFound("Parent folder not found".to_string()))?;
        if parent.owner_id == user.id {
            user.id
        } else {
            let access = check_folder_access(&state.db, user.id, pid).await?;
            if !access.can_read() {
                return Err(AppError::NotFound("Parent folder not found".to_string()));
            }
            parent.owner_id
        }
    } else {
        user.id
    };

    let filter = match parent_id {
        Some(pid) => doc! { "owner_id": effective_owner_id, "parent_id": pid, "deleted_at": bson::Bson::Null },
        None => doc! { "owner_id": user.id, "parent_id": null, "deleted_at": bson::Bson::Null },
    };

    let mut cursor = collection.find(filter).await?;

    let mut folders = Vec::new();
    while cursor.advance().await? {
        let folder: Folder = cursor.deserialize_current()?;
        folders.push(folder_to_response(&state.db, effective_owner_id, &folder).await?);
    }

    // Append mounted shares: folder_shares where grantee_id == user.id
    // and mount_parent_id matches the current listing context.
    let shares_coll = state.db.collection::<FolderShare>("folder_shares");
    let mount_filter = match parent_id {
        Some(pid) => doc! { "grantee_id": user.id, "mount_parent_id": pid },
        None => doc! { "grantee_id": user.id, "mount_parent_id": null },
    };
    let mut share_cursor = shares_coll.find(mount_filter).await?;
    while share_cursor.advance().await? {
        let share: FolderShare = share_cursor.deserialize_current()?;
        // Load the actual shared folder
        if let Some(shared_folder) = collection
            .find_one(doc! { "_id": share.folder_id, "deleted_at": bson::Bson::Null })
            .await?
        {
            let owner_username = get_username(&state.db, shared_folder.owner_id).await?;
            let display_name = share.mount_name.unwrap_or_else(|| shared_folder.name.clone());
            let mut resp = folder_to_response_with_shared(
                &state.db,
                shared_folder.owner_id,
                &shared_folder,
                Some(owner_username),
            )
            .await?;
            resp.name = display_name;
            folders.push(resp);
        }
    }

    Ok(Json(folders))
}

pub async fn create_folder(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(req): Json<CreateFolderRequest>,
) -> Result<(StatusCode, Json<FolderResponse>)> {
    if req.name.is_empty() || req.name.len() > 255 {
        return Err(AppError::Validation(
            "Folder name must be between 1 and 255 characters".to_string(),
        ));
    }

    let parent_id = match &req.parent_id {
        Some(id) if !id.is_empty() => Some(
            ObjectId::parse_str(id)
                .map_err(|_| AppError::BadRequest("Invalid parent ID".to_string()))?,
        ),
        _ => None,
    };

    // Determine the effective owner: if creating inside a shared folder,
    // the new folder belongs to the folder's owner, not the grantee.
    let effective_owner_id = if let Some(pid) = parent_id {
        let collection = state.db.collection::<Folder>("folders");
        let parent = collection
            .find_one(doc! { "_id": pid, "deleted_at": bson::Bson::Null })
            .await?
            .ok_or_else(|| AppError::NotFound("Parent folder not found".to_string()))?;
        if parent.owner_id == user.id {
            user.id
        } else {
            let access = check_folder_access(&state.db, user.id, pid).await?;
            if !access.can_write() {
                return Err(if access.can_read() {
                    AppError::Forbidden("Read-only access".into())
                } else {
                    AppError::NotFound("Parent folder not found".into())
                });
            }
            parent.owner_id
        }
    } else {
        user.id
    };

    let folder = Folder::new(effective_owner_id, parent_id, req.name);

    let collection = state.db.collection::<Folder>("folders");

    // Check for duplicate name in same parent
    let exists = collection
        .find_one(doc! {
            "owner_id": effective_owner_id,
            "parent_id": parent_id.map(mongodb::bson::Bson::ObjectId).unwrap_or(bson::Bson::Null),
            "name": &folder.name,
            "deleted_at": bson::Bson::Null,
        })
        .await?
        .is_some();

    if exists {
        return Err(AppError::Conflict(
            "A folder with this name already exists".to_string(),
        ));
    }

    collection.insert_one(&folder).await?;

    state.events.emit_folder_created(effective_owner_id, &folder).await;

    let resp = folder_to_response(&state.db, effective_owner_id, &folder).await?;
    Ok((StatusCode::CREATED, Json(resp)))
}

pub async fn get_folder(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<FolderResponse>> {
    let folder_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid folder ID".to_string()))?;

    let collection = state.db.collection::<Folder>("folders");
    let folder = collection
        .find_one(doc! { "_id": folder_id, "deleted_at": bson::Bson::Null })
        .await?
        .ok_or_else(|| AppError::NotFound("Folder not found".to_string()))?;

    let access = check_folder_access(&state.db, user.id, folder_id).await?;
    if !access.can_read() {
        return Err(AppError::NotFound("Folder not found".to_string()));
    }

    let shared_by = if folder.owner_id != user.id {
        Some(get_username(&state.db, folder.owner_id).await?)
    } else {
        None
    };

    Ok(Json(
        folder_to_response_with_shared(&state.db, folder.owner_id, &folder, shared_by).await?,
    ))
}

pub async fn update_folder(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<UpdateFolderRequest>,
) -> Result<Json<FolderResponse>> {
    let folder_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid folder ID".to_string()))?;

    let collection = state.db.collection::<Folder>("folders");
    let folder = collection
        .find_one(doc! { "_id": folder_id, "deleted_at": bson::Bson::Null })
        .await?
        .ok_or_else(|| AppError::NotFound("Folder not found".to_string()))?;

    let access = check_folder_access(&state.db, user.id, folder_id).await?;
    if !access.can_write() {
        return Err(if access.can_read() {
            AppError::Forbidden("Read-only access".into())
        } else {
            AppError::NotFound("Folder not found".into())
        });
    }

    let owner_id = folder.owner_id;

    // Effective name/parent after this update
    let new_name: &str = req.name.as_deref().unwrap_or(&folder.name);
    let new_parent_id: Option<ObjectId> = match req.parent_id.as_deref() {
        Some("") => None,
        Some(pid) => {
            let pid = ObjectId::parse_str(pid)
                .map_err(|_| AppError::BadRequest("Invalid parent ID".to_string()))?;
            if pid == folder_id {
                return Err(AppError::BadRequest(
                    "Cannot move folder into itself".to_string(),
                ));
            }
            if collection
                .find_one(doc! { "_id": pid, "owner_id": owner_id, "deleted_at": bson::Bson::Null })
                .await?
                .is_none()
            {
                return Err(AppError::NotFound("Parent folder not found".to_string()));
            }
            Some(pid)
        }
        None => folder.parent_id,
    };

    if req.name.as_deref().map(|n| n.is_empty() || n.len() > 255).unwrap_or(false) {
        return Err(AppError::Validation(
            "Folder name must be between 1 and 255 characters".to_string(),
        ));
    }

    let name_changed = new_name != folder.name.as_str();
    let parent_changed = new_parent_id != folder.parent_id;

    if name_changed || parent_changed {
        if check_name_conflict(
            &state.db,
            owner_id,
            new_parent_id,
            new_name,
            None,
            Some(folder_id),
        )
        .await?
        {
            return Err(AppError::Conflict(
                "A folder with this name already exists at this location".to_string(),
            ));
        }
    }

    let mut set_doc = doc! { "updated_at": mongodb::bson::DateTime::now() };
    if name_changed {
        set_doc.insert("name", new_name);
    }
    if parent_changed {
        set_doc.insert(
            "parent_id",
            new_parent_id
                .map(mongodb::bson::Bson::ObjectId)
                .unwrap_or(bson::Bson::Null),
        );
    }
    if let Some(strategy) = req.sync_strategy {
        set_doc.insert(
            "sync_strategy",
            serde_json::to_string(&strategy)
                .map(|s| s.trim_matches('"').to_string())
                .unwrap_or_default(),
        );
    }
    if let Some(gallery) = req.gallery_include {
        set_doc.insert(
            "gallery_include",
            serde_json::to_string(&gallery)
                .map(|s| s.trim_matches('"').to_string())
                .unwrap_or_default(),
        );
    }
    if let Some(music) = req.music_include {
        set_doc.insert(
            "music_include",
            serde_json::to_string(&music)
                .map(|s| s.trim_matches('"').to_string())
                .unwrap_or_default(),
        );
    }

    let owner_username = get_username(&state.db, owner_id).await?;

    collection
        .update_one(doc! { "_id": folder_id, "owner_id": owner_id }, doc! { "$set": set_doc })
        .await?;

    // After the DB update, recursively update storage_path for all contained files
    // so the on-disk layout stays in sync with the logical folder structure.
    if name_changed || parent_changed {
        update_folder_file_paths(&state, owner_id, folder_id, &owner_username).await?;
    }

    let updated = collection
        .find_one(doc! { "_id": folder_id })
        .await?
        .ok_or_else(|| AppError::NotFound("Folder not found".to_string()))?;

    state.events.emit_folder_updated(owner_id, &updated).await;

    Ok(Json(folder_to_response(&state.db, owner_id, &updated).await?))
}

/// GET /api/folders/{id}/effective-strategy
pub async fn get_effective_strategy(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<EffectiveStrategyResponse>> {
    let folder_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid folder ID".to_string()))?;

    let collection = state.db.collection::<Folder>("folders");
    let folder = collection
        .find_one(doc! { "_id": folder_id, "owner_id": user.id, "deleted_at": bson::Bson::Null })
        .await?
        .ok_or_else(|| AppError::NotFound("Folder not found".to_string()))?;

    let (strategy, source_id) = resolve_setting(&state.db, user.id, &folder, |f| f.sync_strategy).await?;

    Ok(Json(EffectiveStrategyResponse {
        strategy,
        source_folder_id: source_id.map(|id| id.to_hex()),
    }))
}

/// GET /api/sync/tree — flat listing of all files + folders under `parent_id`
/// (recursive), honouring `do_not_sync` folders by excluding their subtrees.
#[derive(Debug, Deserialize)]
pub struct SyncTreeQuery {
    pub parent_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SyncTreeResponse {
    pub files: Vec<SyncFileEntry>,
    pub folders: Vec<FolderResponse>,
}

#[derive(Debug, Serialize)]
pub struct SyncFileEntry {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub parent_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

pub async fn sync_tree(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(query): Query<SyncTreeQuery>,
) -> Result<Json<SyncTreeResponse>> {
    let parent_id = match query.parent_id.as_deref() {
        Some(id) if !id.is_empty() => Some(
            ObjectId::parse_str(id)
                .map_err(|_| AppError::BadRequest("Invalid parent ID".to_string()))?,
        ),
        _ => None,
    };

    let mut all_files = Vec::new();
    let mut all_folders = Vec::new();

    collect_tree(
        &state,
        user.id,
        parent_id,
        &mut all_files,
        &mut all_folders,
    )
    .await?;

    Ok(Json(SyncTreeResponse {
        files: all_files,
        folders: all_folders,
    }))
}

async fn collect_tree(
    state: &AppState,
    user_id: ObjectId,
    parent_id: Option<ObjectId>,
    all_files: &mut Vec<SyncFileEntry>,
    all_folders: &mut Vec<FolderResponse>,
) -> Result<()> {
    let folders_coll = state.db.collection::<Folder>("folders");
    let files_coll = state.db.collection::<File>("files");

    let parent_filter = match parent_id {
        Some(pid) => doc! { "owner_id": user_id, "parent_id": pid, "deleted_at": bson::Bson::Null },
        None => doc! { "owner_id": user_id, "parent_id": null, "deleted_at": bson::Bson::Null },
    };

    // Collect files in this level
    let mut file_cursor = files_coll.find(parent_filter.clone()).await?;
    while file_cursor.advance().await? {
        let file: File = file_cursor.deserialize_current()?;
        all_files.push(SyncFileEntry {
            id: file.id.to_hex(),
            name: file.name.clone(),
            mime_type: file.mime_type.clone(),
            size_bytes: file.size_bytes,
            parent_id: file.parent_id.map(|id| id.to_hex()),
            created_at: file.created_at.to_rfc3339(),
            updated_at: file.updated_at.to_rfc3339(),
        });
    }

    // Collect folders and recurse (skip do_not_sync subtrees)
    let mut folder_cursor = folders_coll.find(parent_filter).await?;
    let mut subfolder_ids = Vec::new();
    while folder_cursor.advance().await? {
        let folder: Folder = folder_cursor.deserialize_current()?;
        let resp = folder_to_response(&state.db, user_id, &folder).await?;
        let skip = resp.effective_strategy == SyncStrategy::DoNotSync;
        let sf_id = folder.id;
        all_folders.push(resp);
        if !skip {
            subfolder_ids.push(sf_id);
        }
    }

    for sf_id in subfolder_ids {
        Box::pin(collect_tree(state, user_id, Some(sf_id), all_files, all_folders)).await?;
    }

    Ok(())
}

/// Recursively rename all files within `folder_id` (and its descendants) on
/// disk and update their `storage_path` in the database.  Called after the
/// folder's own `name` or `parent_id` has already been persisted to MongoDB,
/// so `resolve_storage_path` naturally picks up the new folder structure.
async fn update_folder_file_paths(
    state: &AppState,
    user_id: ObjectId,
    folder_id: ObjectId,
    username: &str,
) -> Result<()> {
    let files_coll = state.db.collection::<File>("files");
    let folders_coll = state.db.collection::<Folder>("folders");

    // Update every file directly inside this folder
    let mut cursor = files_coll
        .find(doc! { "owner_id": user_id, "parent_id": folder_id, "deleted_at": bson::Bson::Null })
        .await?;

    while cursor.advance().await? {
        let file: File = cursor.deserialize_current()?;
        let new_path =
            resolve_storage_path(&state.db, user_id, username, Some(folder_id), &file.name)
                .await?;

        if new_path != file.storage_path {
            let disk_ok = match state.storage.get_backend(file.storage_id).await {
                Ok(backend) => backend.rename(&file.storage_path, &new_path).await.is_ok(),
                Err(_) => false,
            };
            if disk_ok {
                files_coll
                    .update_one(
                        doc! { "_id": file.id },
                        doc! { "$set": { "storage_path": &new_path } },
                    )
                    .await?;
            }
        }
    }

    // Recurse into subfolders
    let mut sf_cursor = folders_coll
        .find(doc! { "owner_id": user_id, "parent_id": folder_id, "deleted_at": bson::Bson::Null })
        .await?;
    let mut subfolder_ids = Vec::new();
    while sf_cursor.advance().await? {
        let sf: Folder = sf_cursor.deserialize_current()?;
        subfolder_ids.push(sf.id);
    }

    for sf_id in subfolder_ids {
        Box::pin(update_folder_file_paths(state, user_id, sf_id, username)).await?;
    }

    Ok(())
}

pub async fn get_folder_breadcrumb(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Vec<FolderResponse>>> {
    let folder_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid folder ID".to_string()))?;

    let collection = state.db.collection::<Folder>("folders");
    let shares_coll = state.db.collection::<FolderShare>("folder_shares");
    let mut chain: Vec<FolderResponse> = Vec::new();
    let mut current_id = Some(folder_id);

    while let Some(id) = current_id {
        let folder = collection
            .find_one(doc! { "_id": id, "deleted_at": bson::Bson::Null })
            .await?
            .ok_or_else(|| AppError::NotFound("Folder not found".to_string()))?;

        if folder.owner_id == user.id {
            // User owns this folder — continue up normally
            current_id = folder.parent_id;
            chain.push(folder_to_response(&state.db, user.id, &folder).await?);
        } else {
            // Not the owner — check if user has share access on this folder
            // or an ancestor. Walk up until we find a share or run out.
            let access = check_folder_access(&state.db, user.id, id).await?;
            if !access.can_read() {
                return Err(AppError::NotFound("Folder not found".to_string()));
            }
            // Include this folder (the shared root or a descendant of it)
            let owner_username = get_username(&state.db, folder.owner_id).await?;

            // Check if this specific folder has a direct share — if so, it's the
            // share root and we should stop the breadcrumb here.
            let has_direct_share = shares_coll
                .find_one(doc! { "folder_id": id, "grantee_id": user.id })
                .await?
                .is_some();

            let resp = folder_to_response_with_shared(
                &state.db,
                folder.owner_id,
                &folder,
                Some(owner_username),
            )
            .await?;

            if has_direct_share {
                // This is the share root; stop the breadcrumb here
                chain.push(resp);
                break;
            } else {
                // Folder is a child of the shared root; keep walking up
                current_id = folder.parent_id;
                chain.push(resp);
            }
        }
    }

    chain.reverse();
    Ok(Json(chain))
}

#[derive(Debug, Deserialize)]
pub struct CopyFolderRequest {
    pub parent_id: Option<String>,
    pub name: Option<String>,
}

pub async fn copy_folder(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<CopyFolderRequest>,
) -> Result<(StatusCode, Json<FolderResponse>)> {
    let source_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid folder ID".to_string()))?;

    let folders_coll = state.db.collection::<Folder>("folders");
    let source = folders_coll
        .find_one(doc! { "_id": source_id, "owner_id": user.id, "deleted_at": bson::Bson::Null })
        .await?
        .ok_or_else(|| AppError::NotFound("Folder not found".to_string()))?;

    let dest_parent_id = match req.parent_id.as_deref() {
        Some("") => None,
        Some(pid) => Some(
            ObjectId::parse_str(pid)
                .map_err(|_| AppError::BadRequest("Invalid parent ID".to_string()))?,
        ),
        None => source.parent_id,
    };

    let dest_name = req
        .name
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("Copy of {}", source.name));

    if check_name_conflict(&state.db, user.id, dest_parent_id, &dest_name, None, None).await? {
        return Err(AppError::Conflict(
            "A folder with this name already exists at this location".to_string(),
        ));
    }

    let new_folder = Folder::new(user.id, dest_parent_id, dest_name);
    folders_coll.insert_one(&new_folder).await?;
    state.events.emit_folder_created(user.id, &new_folder).await;

    copy_folder_contents(&state, user.id, &user.username, source_id, new_folder.id).await?;

    let resp = folder_to_response(&state.db, user.id, &new_folder).await?;
    Ok((StatusCode::CREATED, Json(resp)))
}

async fn copy_folder_contents(
    state: &AppState,
    user_id: ObjectId,
    username: &str,
    source_id: ObjectId,
    dest_id: ObjectId,
) -> Result<()> {
    let files_coll = state.db.collection::<File>("files");
    let folders_coll = state.db.collection::<Folder>("folders");

    // Copy files in this folder
    let mut cursor = files_coll
        .find(doc! { "owner_id": user_id, "parent_id": source_id, "deleted_at": bson::Bson::Null })
        .await?;

    let mut total_size = 0i64;
    while cursor.advance().await? {
        let file: File = cursor.deserialize_current()?;
        let dst_path =
            resolve_storage_path(&state.db, user_id, username, Some(dest_id), &file.name).await?;

        if let Ok(backend) = state.storage.get_backend(file.storage_id).await {
            let mut reader = backend.read(&file.storage_path).await?;
            let mut data = Vec::new();
            reader.read_to_end(&mut data).await.map_err(|e| {
                AppError::Storage(format!("Failed to read source file: {}", e))
            })?;
            backend.write(&dst_path, &data).await?;
        }

        let new_file = File::new(
            file.storage_id,
            dst_path,
            user_id,
            Some(dest_id),
            file.name.clone(),
            file.mime_type.clone(),
            file.size_bytes,
            file.checksum_sha256.clone(),
        );
        files_coll.insert_one(&new_file).await?;
        total_size += file.size_bytes;
        state.events.emit_file_created(user_id, &new_file).await;
    }

    if total_size > 0 {
        state.auth.update_user_bytes(user_id, total_size).await?;
    }

    // Recursively copy subfolders
    let mut sf_cursor = folders_coll
        .find(doc! { "owner_id": user_id, "parent_id": source_id, "deleted_at": bson::Bson::Null })
        .await?;
    let mut subfolders = Vec::new();
    while sf_cursor.advance().await? {
        let sf: Folder = sf_cursor.deserialize_current()?;
        subfolders.push(sf);
    }

    for sf in subfolders {
        let new_sf = Folder::new(user_id, Some(dest_id), sf.name.clone());
        folders_coll.insert_one(&new_sf).await?;
        state.events.emit_folder_created(user_id, &new_sf).await;
        Box::pin(copy_folder_contents(state, user_id, username, sf.id, new_sf.id)).await?;
    }

    Ok(())
}

pub async fn delete_folder(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let folder_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid folder ID".to_string()))?;

    let folders_collection = state.db.collection::<Folder>("folders");

    // Verify folder exists
    let folder = folders_collection
        .find_one(doc! { "_id": folder_id, "deleted_at": bson::Bson::Null })
        .await?
        .ok_or_else(|| AppError::NotFound("Folder not found".to_string()))?;

    let access = check_folder_access(&state.db, user.id, folder_id).await?;
    if !access.can_write() {
        return Err(if access.can_read() {
            AppError::Forbidden("Read-only access".into())
        } else {
            AppError::NotFound("Folder not found".into())
        });
    }

    let owner_id = folder.owner_id;

    let now = Utc::now();
    let now_bson = bson::DateTime::from_chrono(now);
    let batch_id = uuid::Uuid::new_v4().to_string();

    // Recursively soft-delete contents
    soft_delete_folder_contents(&state, owner_id, folder_id, now_bson, &batch_id).await?;

    // Soft-delete the folder itself
    folders_collection
        .update_one(
            doc! { "_id": folder_id, "owner_id": owner_id },
            doc! { "$set": { "deleted_at": now_bson, "batch_delete_id": &batch_id } },
        )
        .await?;

    state.events.emit_folder_deleted(owner_id, folder_id).await;

    Ok(StatusCode::NO_CONTENT)
}

async fn soft_delete_folder_contents(
    state: &AppState,
    user_id: ObjectId,
    folder_id: ObjectId,
    now_bson: bson::DateTime,
    batch_id: &str,
) -> Result<()> {
    let folders_collection = state.db.collection::<Folder>("folders");
    let files_collection = state.db.collection::<File>("files");

    // Soft-delete files in this folder
    let mut cursor = files_collection
        .find(doc! { "owner_id": user_id, "parent_id": folder_id, "deleted_at": bson::Bson::Null })
        .await?;

    let mut deleted_file_ids: Vec<bson::Bson> = Vec::new();
    while cursor.advance().await? {
        let file: File = cursor.deserialize_current()?;

        // Move blob to trash on storage
        let tp = crate::routes::files::trash_path(&file.storage_path);
        if let Ok(backend) = state.storage.get_backend(file.storage_id).await {
            let _ = backend.move_to_trash(&file.storage_path, &tp).await;
            // Best-effort thumbnail cleanup
            let _ = backend.delete(&format!(".thumbs/{}.jpg", file.id.to_hex())).await;
        }

        // Soft-delete the file record
        let _ = files_collection
            .update_one(
                doc! { "_id": file.id },
                doc! { "$set": { "deleted_at": now_bson, "trash_path": &tp, "batch_delete_id": batch_id } },
            )
            .await;

        deleted_file_ids.push(bson::Bson::ObjectId(file.id));
        if let Err(e) = state.search.delete_file(&file.id.to_hex()).await {
            tracing::warn!("Failed to remove file {} from search index: {}", file.id, e);
        }
        state.events.emit_file_deleted(user_id, file.id).await;
    }

    // Remove deleted files from any playlists
    if !deleted_file_ids.is_empty() {
        let _ = state
            .db
            .collection::<bson::Document>("playlists")
            .update_many(
                doc! { "owner_id": user_id },
                doc! { "$pull": { "tracks": { "file_id": { "$in": &deleted_file_ids } } } },
            )
            .await;
    }

    // Recursively soft-delete subfolders
    let mut subfolder_cursor = folders_collection
        .find(doc! { "owner_id": user_id, "parent_id": folder_id, "deleted_at": bson::Bson::Null })
        .await?;

    let mut subfolder_ids = Vec::new();
    while subfolder_cursor.advance().await? {
        let subfolder: Folder = subfolder_cursor.deserialize_current()?;
        subfolder_ids.push(subfolder.id);
    }

    for subfolder_id in subfolder_ids {
        Box::pin(soft_delete_folder_contents(state, user_id, subfolder_id, now_bson, batch_id)).await?;
        folders_collection
            .update_one(
                doc! { "_id": subfolder_id },
                doc! { "$set": { "deleted_at": now_bson, "batch_delete_id": batch_id } },
            )
            .await?;
        state.events.emit_folder_deleted(user_id, subfolder_id).await;
    }

    Ok(())
}
