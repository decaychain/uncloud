use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use bson::doc;
use chrono::Utc;
use mongodb::bson::oid::ObjectId;
use mongodb::options::{FindOneAndUpdateOptions, ReturnDocument};

use crate::error::{AppError, Result};
use crate::middleware::AuthUser;
use crate::models::{RecentVault, VaultRecentsDoc};
use crate::AppState;
use uncloud_common::{AddRecentVaultRequest, RecentVaultEntry};

const MAX_RECENT_VAULTS: usize = 10;

fn vault_to_response(v: &RecentVault) -> RecentVaultEntry {
    RecentVaultEntry {
        file_id: v.file_id.to_hex(),
        file_name: v.file_name.clone(),
        folder_path: v.folder_path.clone(),
        last_opened_at: v.last_opened_at.to_rfc3339(),
    }
}

pub async fn list_recent_vaults(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<RecentVaultEntry>>> {
    let coll = state.db.collection::<VaultRecentsDoc>("user_preferences");
    let prefs = coll
        .find_one(doc! { "user_id": user.id })
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let entries = prefs
        .map(|p| p.recent_vaults.iter().map(vault_to_response).collect())
        .unwrap_or_default();

    Ok(Json(entries))
}

pub async fn add_recent_vault(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(req): Json<AddRecentVaultRequest>,
) -> Result<(StatusCode, Json<Vec<RecentVaultEntry>>)> {
    let file_id = ObjectId::parse_str(&req.file_id)
        .map_err(|_| AppError::BadRequest("Invalid file_id".into()))?;

    let now = Utc::now();
    let new_entry = RecentVault {
        file_id,
        file_name: req.file_name,
        folder_path: req.folder_path,
        last_opened_at: now,
    };

    let coll = state.db.collection::<VaultRecentsDoc>("user_preferences");

    // Pull any existing entry for this file_id first, then push the new one
    // This is done in two operations to act as an upsert-then-reorder.
    let opts = FindOneAndUpdateOptions::builder()
        .upsert(true)
        .return_document(ReturnDocument::After)
        .build();

    // Step 1: ensure doc exists and remove old entry for this file
    coll.find_one_and_update(
        doc! { "user_id": user.id },
        vec![doc! {
            "$set": {
                "user_id": user.id,
                "recent_vaults": {
                    "$ifNull": [
                        { "$filter": {
                            "input": "$recent_vaults",
                            "cond": { "$ne": ["$$this.file_id", file_id] }
                        }},
                        []
                    ]
                }
            }
        }],
    )
    .with_options(opts.clone())
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    // Step 2: push new entry to front and trim to MAX
    let prefs = coll
        .find_one_and_update(
            doc! { "user_id": user.id },
            vec![doc! {
                "$set": {
                    "recent_vaults": {
                        "$slice": [
                            { "$concatArrays": [
                                [bson::to_bson(&new_entry).unwrap()],
                                "$recent_vaults"
                            ]},
                            MAX_RECENT_VAULTS as i32
                        ]
                    }
                }
            }],
        )
        .with_options(opts)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let entries = prefs
        .map(|p| p.recent_vaults.iter().map(vault_to_response).collect())
        .unwrap_or_default();

    Ok((StatusCode::OK, Json(entries)))
}

pub async fn remove_recent_vault(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(file_id): Path<String>,
) -> Result<StatusCode> {
    let file_oid = ObjectId::parse_str(&file_id)
        .map_err(|_| AppError::BadRequest("Invalid file_id".into()))?;

    let coll = state.db.collection::<VaultRecentsDoc>("user_preferences");
    coll.update_one(
        doc! { "user_id": user.id },
        doc! { "$pull": { "recent_vaults": { "file_id": file_oid } } },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}
