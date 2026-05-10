use std::sync::Arc;

use mongodb::bson::{doc, oid::ObjectId, Bson};
use serde_json::{json, Value};

use crate::middleware::auth::AuthUser;
use crate::models::file::File;
use crate::models::folder::Folder;
use crate::AppState;

use super::ToolError;

pub fn input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "folder_id": {
                "type": "string",
                "description": "Folder ObjectId hex. Omit or empty for the root folder."
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": 200,
                "default": 50,
                "description": "Max combined entries (folders + files) returned."
            }
        }
    })
}

pub async fn call(
    args: &Value,
    state: &Arc<AppState>,
    user: &AuthUser,
) -> Result<Value, ToolError> {
    let folder_id_str = args
        .get("folder_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();

    let limit = args
        .get("limit")
        .and_then(|v| v.as_i64())
        .map(|n| n.clamp(1, 200) as usize)
        .unwrap_or(50);

    let parent_id: Option<ObjectId> = if folder_id_str.is_empty() {
        None
    } else {
        Some(
            ObjectId::parse_str(folder_id_str)
                .map_err(|_| ToolError::invalid("folder_id is not a valid ObjectId"))?,
        )
    };

    let folders_coll = state.db.collection::<Folder>("folders");
    let files_coll = state.db.collection::<File>("files");

    // Resolve the folder (and confirm ownership) for non-root listings.
    let folder_summary = if let Some(pid) = parent_id {
        let folder = folders_coll
            .find_one(doc! { "_id": pid, "deleted_at": Bson::Null })
            .await
            .map_err(|e| ToolError::exec(format!("folder lookup failed: {}", e)))?
            .ok_or_else(|| ToolError::exec("folder not found"))?;
        if folder.owner_id != user.id {
            // No share resolution from MCP in v1 — keeps the surface
            // narrow. Re-evaluate when MCP needs to expose shared folders.
            return Err(ToolError::exec("folder not found"));
        }
        Some(folder)
    } else {
        None
    };

    // Folders in this parent.
    let folder_filter = match parent_id {
        Some(pid) => doc! {
            "owner_id": user.id,
            "parent_id": pid,
            "deleted_at": Bson::Null,
        },
        None => doc! {
            "owner_id": user.id,
            "parent_id": Bson::Null,
            "deleted_at": Bson::Null,
        },
    };

    let mut child_folders: Vec<Folder> = Vec::new();
    let mut cursor = folders_coll
        .find(folder_filter)
        .await
        .map_err(|e| ToolError::exec(format!("folders query failed: {}", e)))?;
    while cursor.advance().await.unwrap_or(false) {
        if let Ok(f) = cursor.deserialize_current() {
            child_folders.push(f);
            if child_folders.len() >= limit {
                break;
            }
        }
    }

    // Files in this parent — only fetch up to the remaining budget so
    // the combined response stays under `limit` entries.
    let remaining = limit.saturating_sub(child_folders.len());
    let mut child_files: Vec<File> = Vec::new();
    if remaining > 0 {
        let file_filter = match parent_id {
            Some(pid) => doc! {
                "owner_id": user.id,
                "parent_id": pid,
                "deleted_at": Bson::Null,
            },
            None => doc! {
                "owner_id": user.id,
                "parent_id": Bson::Null,
                "deleted_at": Bson::Null,
            },
        };
        let mut cursor = files_coll
            .find(file_filter)
            .await
            .map_err(|e| ToolError::exec(format!("files query failed: {}", e)))?;
        while cursor.advance().await.unwrap_or(false) {
            if let Ok(f) = cursor.deserialize_current() {
                child_files.push(f);
                if child_files.len() >= remaining {
                    break;
                }
            }
        }
    }

    let folder_field = folder_summary.as_ref().map(|f| {
        json!({
            "id": f.id.to_hex(),
            "name": f.name,
            "parent_id": f.parent_id.map(|p| p.to_hex()),
        })
    });

    Ok(json!({
        "folder": folder_field,
        "folders": child_folders.iter().map(|f| json!({
            "id": f.id.to_hex(),
            "name": f.name,
        })).collect::<Vec<_>>(),
        "files": child_files.iter().map(|f| json!({
            "id": f.id.to_hex(),
            "name": f.name,
            "mime_type": f.mime_type,
            "size_bytes": f.size_bytes,
            "updated_at": f.updated_at.to_rfc3339(),
        })).collect::<Vec<_>>(),
        "next_cursor": Value::Null,
    }))
}
