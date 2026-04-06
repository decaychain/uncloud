use mongodb::bson::{doc, oid::ObjectId};
use mongodb::Database;

use crate::error::{AppError, Result};
use crate::models::{FolderShare, SharePermissionModel};

/// The resolved access level a user has on a folder or file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessLevel {
    /// The user owns the resource.
    Owner,
    /// The user has been granted access (directly or via an ancestor folder).
    Shared(SharePermissionModel),
    /// The user has no access.
    None,
}

impl AccessLevel {
    /// Returns `true` if the user has at least read access.
    pub fn can_read(&self) -> bool {
        !matches!(self, AccessLevel::None)
    }

    /// Returns `true` if the user can write (Owner, ReadWrite, or Admin share).
    pub fn can_write(&self) -> bool {
        matches!(
            self,
            AccessLevel::Owner
                | AccessLevel::Shared(SharePermissionModel::ReadWrite)
                | AccessLevel::Shared(SharePermissionModel::Admin)
        )
    }

    /// Returns `true` if the user can manage shares (Owner or Admin share).
    pub fn can_admin(&self) -> bool {
        matches!(
            self,
            AccessLevel::Owner | AccessLevel::Shared(SharePermissionModel::Admin)
        )
    }
}

/// Maximum folder depth we'll walk when resolving inherited shares.
const MAX_DEPTH: usize = 50;

/// Check if `user_id` can access `folder_id`.
///
/// Returns `Owner` if they own the folder, `Shared(perm)` if they have a share
/// on this folder or any ancestor, `None` otherwise.
pub async fn check_folder_access(
    db: &Database,
    user_id: ObjectId,
    folder_id: ObjectId,
) -> Result<AccessLevel> {
    let folders_coll = db.collection::<crate::models::Folder>("folders");
    let shares_coll = db.collection::<FolderShare>("folder_shares");

    let mut current_id = folder_id;

    for _ in 0..MAX_DEPTH {
        // Load the folder
        let folder = folders_coll
            .find_one(doc! { "_id": current_id })
            .await?
            .ok_or_else(|| AppError::NotFound("Folder".to_string()))?;

        // Check ownership
        if folder.owner_id == user_id {
            return Ok(AccessLevel::Owner);
        }

        // Check for a direct share on this folder
        if let Some(share) = shares_coll
            .find_one(doc! {
                "folder_id": current_id,
                "grantee_id": user_id,
            })
            .await?
        {
            return Ok(AccessLevel::Shared(share.permission));
        }

        // Walk up to parent
        match folder.parent_id {
            Some(parent_id) => current_id = parent_id,
            None => return Ok(AccessLevel::None),
        }
    }

    // Depth limit reached — treat as no access
    Ok(AccessLevel::None)
}

/// Check if `user_id` can access a file.
///
/// Loads the file, checks if owner, then delegates to `check_folder_access`
/// on its parent folder. Files at root (no parent) are only accessible by the owner.
pub async fn check_file_access(
    db: &Database,
    user_id: ObjectId,
    file_id: ObjectId,
) -> Result<AccessLevel> {
    let files_coll = db.collection::<crate::models::File>("files");

    let file = files_coll
        .find_one(doc! { "_id": file_id, "deleted_at": null })
        .await?
        .ok_or_else(|| AppError::NotFound("File".to_string()))?;

    // Owner always has full access
    if file.owner_id == user_id {
        return Ok(AccessLevel::Owner);
    }

    // For shared access, check the file's parent folder
    match file.parent_id {
        Some(parent_id) => check_folder_access(db, user_id, parent_id).await,
        // File at root with no parent — only the owner can access it
        None => Ok(AccessLevel::None),
    }
}
