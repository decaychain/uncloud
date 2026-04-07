use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use mongodb::bson::{doc, oid::ObjectId};
use std::sync::Arc;

use crate::error::{AppError, Result};
use crate::middleware::AuthUser;
use crate::models::{FolderShare, SharePermissionModel, User};
use crate::services::sharing;
use crate::AppState;

use uncloud_common::{
    CreateFolderShareRequest, FolderShareResponse, UpdateFolderShareRequest,
};

/// Build a response by joining folder and user data.
async fn build_response(
    state: &AppState,
    share: &FolderShare,
) -> Result<FolderShareResponse> {
    let folders = state.db.collection::<crate::models::Folder>("folders");
    let users = state.db.collection::<User>("users");

    let folder = folders
        .find_one(doc! { "_id": share.folder_id })
        .await?
        .ok_or_else(|| AppError::NotFound("Folder".to_string()))?;

    let owner = users
        .find_one(doc! { "_id": share.owner_id })
        .await?
        .ok_or_else(|| AppError::NotFound("Owner".to_string()))?;

    let grantee = users
        .find_one(doc! { "_id": share.grantee_id })
        .await?
        .ok_or_else(|| AppError::NotFound("Grantee".to_string()))?;

    Ok(FolderShareResponse {
        id: share.id.to_hex(),
        folder_id: share.folder_id.to_hex(),
        folder_name: folder.name,
        owner_id: share.owner_id.to_hex(),
        owner_username: owner.username,
        grantee_id: share.grantee_id.to_hex(),
        grantee_username: grantee.username,
        permission: share.permission.into(),
        mount_parent_id: share.mount_parent_id.map(|id| id.to_hex()),
        mount_name: share.mount_name.clone(),
        music_include: share.music_include,
        gallery_include: share.gallery_include,
        created_at: share.created_at.to_rfc3339(),
    })
}

/// Build responses for a list of shares, batch-loading users and folders.
async fn build_responses(
    state: &AppState,
    shares: &[FolderShare],
) -> Result<Vec<FolderShareResponse>> {
    use futures::TryStreamExt;

    if shares.is_empty() {
        return Ok(vec![]);
    }

    // Collect unique IDs
    let folder_ids: Vec<ObjectId> = shares.iter().map(|s| s.folder_id).collect();
    let mut user_ids: Vec<ObjectId> = shares.iter().map(|s| s.owner_id).collect();
    user_ids.extend(shares.iter().map(|s| s.grantee_id));
    user_ids.sort();
    user_ids.dedup();

    // Batch load folders
    let folders_coll = state.db.collection::<crate::models::Folder>("folders");
    let folders: Vec<crate::models::Folder> = folders_coll
        .find(doc! { "_id": { "$in": &folder_ids } })
        .await?
        .try_collect()
        .await?;
    let folder_map: std::collections::HashMap<ObjectId, &crate::models::Folder> =
        folders.iter().map(|f| (f.id, f)).collect();

    // Batch load users
    let users_coll = state.db.collection::<User>("users");
    let users: Vec<User> = users_coll
        .find(doc! { "_id": { "$in": &user_ids } })
        .await?
        .try_collect()
        .await?;
    let user_map: std::collections::HashMap<ObjectId, &User> =
        users.iter().map(|u| (u.id, u)).collect();

    let mut results = Vec::with_capacity(shares.len());
    for share in shares {
        let folder_name = folder_map
            .get(&share.folder_id)
            .map(|f| f.name.clone())
            .unwrap_or_else(|| "(deleted)".to_string());
        let owner_username = user_map
            .get(&share.owner_id)
            .map(|u| u.username.clone())
            .unwrap_or_else(|| "(unknown)".to_string());
        let grantee_username = user_map
            .get(&share.grantee_id)
            .map(|u| u.username.clone())
            .unwrap_or_else(|| "(unknown)".to_string());

        results.push(FolderShareResponse {
            id: share.id.to_hex(),
            folder_id: share.folder_id.to_hex(),
            folder_name,
            owner_id: share.owner_id.to_hex(),
            owner_username,
            grantee_id: share.grantee_id.to_hex(),
            grantee_username,
            permission: share.permission.into(),
            mount_parent_id: share.mount_parent_id.map(|id| id.to_hex()),
            mount_name: share.mount_name.clone(),
            music_include: share.music_include,
            gallery_include: share.gallery_include,
            created_at: share.created_at.to_rfc3339(),
        });
    }

    Ok(results)
}

/// POST /api/folder-shares
pub async fn create_share(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(req): Json<CreateFolderShareRequest>,
) -> Result<(StatusCode, Json<FolderShareResponse>)> {
    let folder_id = ObjectId::parse_str(&req.folder_id)
        .map_err(|_| AppError::BadRequest("Invalid folder ID".to_string()))?;

    // Check the caller has permission to share this folder
    let access = sharing::check_folder_access(&state.db, user.id, folder_id).await?;
    if !access.can_admin() {
        return Err(AppError::Forbidden("Access denied".into()));
    }

    // Look up grantee by username
    let users_coll = state.db.collection::<User>("users");
    let grantee = users_coll
        .find_one(doc! { "username": &req.grantee_username })
        .await?
        .ok_or_else(|| AppError::NotFound("User".to_string()))?;

    // Cannot share with yourself
    if grantee.id == user.id {
        return Err(AppError::BadRequest(
            "Cannot share a folder with yourself".to_string(),
        ));
    }

    // Determine the owner of the folder (may differ from caller if caller has Admin share)
    let folders_coll = state.db.collection::<crate::models::Folder>("folders");
    let folder = folders_coll
        .find_one(doc! { "_id": folder_id })
        .await?
        .ok_or_else(|| AppError::NotFound("Folder".to_string()))?;

    let shares_coll = state.db.collection::<FolderShare>("folder_shares");

    // Check for existing share — reject duplicate
    if shares_coll
        .find_one(doc! {
            "folder_id": folder_id,
            "grantee_id": grantee.id,
        })
        .await?
        .is_some()
    {
        return Err(AppError::Conflict(
            "This folder is already shared with that user".to_string(),
        ));
    }

    let now = chrono::Utc::now();
    let share = FolderShare {
        id: ObjectId::new(),
        folder_id,
        owner_id: folder.owner_id,
        grantee_id: grantee.id,
        permission: req.permission.into(),
        mount_parent_id: None,
        mount_name: None,
        music_include: Default::default(),
        gallery_include: Default::default(),
        created_at: now,
        updated_at: now,
    };

    shares_coll.insert_one(&share).await?;

    // Notify the grantee that a folder has been shared with them
    state
        .events
        .emit_folder_shared(share.grantee_id, share.folder_id, share.id)
        .await;

    let response = build_response(&state, &share).await?;
    Ok((StatusCode::CREATED, Json(response)))
}

/// GET /api/folder-shares/by-me
pub async fn list_shares_by_me(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<FolderShareResponse>>> {
    use futures::TryStreamExt;

    let shares_coll = state.db.collection::<FolderShare>("folder_shares");
    let shares: Vec<FolderShare> = shares_coll
        .find(doc! { "owner_id": user.id })
        .await?
        .try_collect()
        .await?;

    let responses = build_responses(&state, &shares).await?;
    Ok(Json(responses))
}

/// GET /api/folder-shares/with-me
pub async fn list_shares_with_me(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<FolderShareResponse>>> {
    use futures::TryStreamExt;

    let shares_coll = state.db.collection::<FolderShare>("folder_shares");
    let shares: Vec<FolderShare> = shares_coll
        .find(doc! { "grantee_id": user.id })
        .await?
        .try_collect()
        .await?;

    let responses = build_responses(&state, &shares).await?;
    Ok(Json(responses))
}

/// GET /api/folder-shares/folder/{id}
pub async fn list_folder_shares(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Vec<FolderShareResponse>>> {
    use futures::TryStreamExt;

    let folder_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid folder ID".to_string()))?;

    // Caller must be owner or have Admin share permission
    let access = sharing::check_folder_access(&state.db, user.id, folder_id).await?;
    if !access.can_admin() {
        return Err(AppError::Forbidden("Access denied".into()));
    }

    let shares_coll = state.db.collection::<FolderShare>("folder_shares");
    let shares: Vec<FolderShare> = shares_coll
        .find(doc! { "folder_id": folder_id })
        .await?
        .try_collect()
        .await?;

    let responses = build_responses(&state, &shares).await?;
    Ok(Json(responses))
}

/// PUT /api/folder-shares/{id}
pub async fn update_share(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<UpdateFolderShareRequest>,
) -> Result<Json<FolderShareResponse>> {
    let share_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid share ID".to_string()))?;

    let shares_coll = state.db.collection::<FolderShare>("folder_shares");
    let share = shares_coll
        .find_one(doc! { "_id": share_id })
        .await?
        .ok_or_else(|| AppError::NotFound("Share".to_string()))?;

    let is_owner_or_admin = {
        let access = sharing::check_folder_access(&state.db, user.id, share.folder_id).await?;
        access.can_admin()
    };
    let is_grantee = share.grantee_id == user.id;

    if !is_owner_or_admin && !is_grantee {
        return Err(AppError::Forbidden("Access denied".into()));
    }

    let mut update = doc! {};

    // Permission can only be changed by owner/admin
    if let Some(permission) = req.permission {
        if !is_owner_or_admin {
            return Err(AppError::Forbidden("Access denied".into()));
        }
        let perm_model: SharePermissionModel = permission.into();
        update.insert(
            "permission",
            mongodb::bson::to_bson(&perm_model)
                .map_err(|e| AppError::Internal(e.to_string()))?,
        );
    }

    // Mount settings can only be changed by the grantee
    if let Some(ref mount_parent_id) = req.mount_parent_id {
        if !is_grantee {
            return Err(AppError::Forbidden("Access denied".into()));
        }
        if mount_parent_id.is_empty() {
            update.insert("mount_parent_id", mongodb::bson::Bson::Null);
        } else {
            let parent_oid = ObjectId::parse_str(mount_parent_id)
                .map_err(|_| AppError::BadRequest("Invalid mount_parent_id".to_string()))?;
            update.insert("mount_parent_id", parent_oid);
        }
    }

    if let Some(ref mount_name) = req.mount_name {
        if !is_grantee {
            return Err(AppError::Forbidden("Access denied".into()));
        }
        if mount_name.is_empty() {
            update.insert("mount_name", mongodb::bson::Bson::Null);
        } else {
            update.insert("mount_name", mount_name.as_str());
        }
    }

    // Music/gallery inclusion can only be changed by the grantee
    if let Some(music) = req.music_include {
        if !is_grantee {
            return Err(AppError::Forbidden("Access denied".into()));
        }
        update.insert(
            "music_include",
            serde_json::to_string(&music)
                .map(|s| s.trim_matches('"').to_string())
                .unwrap_or_default(),
        );
    }
    if let Some(gallery) = req.gallery_include {
        if !is_grantee {
            return Err(AppError::Forbidden("Access denied".into()));
        }
        update.insert(
            "gallery_include",
            serde_json::to_string(&gallery)
                .map(|s| s.trim_matches('"').to_string())
                .unwrap_or_default(),
        );
    }

    if update.is_empty() {
        return Err(AppError::BadRequest("No fields to update".to_string()));
    }

    update.insert("updated_at", mongodb::bson::DateTime::from_chrono(chrono::Utc::now()));

    shares_coll
        .update_one(doc! { "_id": share_id }, doc! { "$set": update })
        .await?;

    // Re-fetch the updated share
    let updated = shares_coll
        .find_one(doc! { "_id": share_id })
        .await?
        .ok_or_else(|| AppError::NotFound("Share".to_string()))?;

    let response = build_response(&state, &updated).await?;
    Ok(Json(response))
}

/// DELETE /api/folder-shares/{id}
pub async fn delete_share(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let share_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid share ID".to_string()))?;

    let shares_coll = state.db.collection::<FolderShare>("folder_shares");
    let share = shares_coll
        .find_one(doc! { "_id": share_id })
        .await?
        .ok_or_else(|| AppError::NotFound("Share".to_string()))?;

    // Owner/admin can revoke, grantee can leave
    let is_owner_or_admin = {
        let access = sharing::check_folder_access(&state.db, user.id, share.folder_id).await?;
        access.can_admin()
    };
    let is_grantee = share.grantee_id == user.id;

    if !is_owner_or_admin && !is_grantee {
        return Err(AppError::Forbidden("Access denied".into()));
    }

    shares_coll
        .delete_one(doc! { "_id": share_id })
        .await?;

    // Notify the other party: if the owner/admin is revoking, notify the grantee;
    // if the grantee is leaving, notify the owner.
    if is_grantee {
        state
            .events
            .emit_folder_share_revoked(share.owner_id, share.folder_id, share.id)
            .await;
    } else {
        state
            .events
            .emit_folder_share_revoked(share.grantee_id, share.folder_id, share.id)
            .await;
    }

    Ok(StatusCode::NO_CONTENT)
}
