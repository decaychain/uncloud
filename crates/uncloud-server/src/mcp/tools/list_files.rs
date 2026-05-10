use std::sync::Arc;

use mongodb::bson::{doc, oid::ObjectId, Bson};
use serde_json::{json, Value};

use crate::mcp::path;
use crate::middleware::auth::AuthUser;
use crate::models::file::File;
use crate::models::folder::Folder;
use crate::AppState;

use super::ToolError;

pub fn input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "description": "Absolute, case-sensitive folder path (e.g. \"/Documents\"). Use \"/\" or omit for the root folder. Must not contain '..' or backslashes."
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
    let path_str = args.get("path").and_then(|v| v.as_str()).unwrap_or("/");
    let segments = path::parse(path_str)?;

    let limit = args
        .get("limit")
        .and_then(|v| v.as_i64())
        .map(|n| n.clamp(1, 200) as usize)
        .unwrap_or(50);

    let folder = path::resolve_folder(state, user.id, &segments).await?;
    let parent_id: Option<ObjectId> = folder.as_ref().map(|f| f.id);

    let folders_coll = state.db.collection::<Folder>("folders");
    let files_coll = state.db.collection::<File>("files");

    let parent_filter = match parent_id {
        Some(pid) => Bson::ObjectId(pid),
        None => Bson::Null,
    };

    let mut child_folders: Vec<Folder> = Vec::new();
    let mut cursor = folders_coll
        .find(doc! {
            "owner_id": user.id,
            "parent_id": parent_filter.clone(),
            "deleted_at": Bson::Null,
        })
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

    let remaining = limit.saturating_sub(child_folders.len());
    let mut child_files: Vec<File> = Vec::new();
    if remaining > 0 {
        let mut cursor = files_coll
            .find(doc! {
                "owner_id": user.id,
                "parent_id": parent_filter,
                "deleted_at": Bson::Null,
            })
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

    // Compose the absolute path for each child by joining the folder
    // path we already resolved with the child's name. Cheaper than
    // calling path::build_for_folder/file per entry, which would walk
    // the parent chain redundantly.
    let folder_path = match folder.as_ref() {
        Some(f) => path::build_for_folder(state, f).await,
        None => "/".to_string(),
    };
    let join = |name: &str| -> String {
        if folder_path == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", folder_path, name)
        }
    };

    let folder_field = folder.as_ref().map(|f| {
        json!({
            "id": f.id.to_hex(),
            "path": folder_path,
            "name": f.name,
        })
    });

    Ok(json!({
        "folder": folder_field,
        "folders": child_folders.iter().map(|f| json!({
            "id": f.id.to_hex(),
            "path": join(&f.name),
            "name": f.name,
        })).collect::<Vec<_>>(),
        "files": child_files.iter().map(|f| json!({
            "id": f.id.to_hex(),
            "path": join(&f.name),
            "name": f.name,
            "mime_type": f.mime_type,
            "size_bytes": f.size_bytes,
            "updated_at": f.updated_at.to_rfc3339(),
        })).collect::<Vec<_>>(),
        "next_cursor": Value::Null,
    }))
}
