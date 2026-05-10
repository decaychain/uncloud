use std::sync::Arc;

use axum::{extract::State, Json};
use serde_json::{json, Value};

use crate::mcp::path;
use crate::middleware::auth::AuthUser;
use crate::routes::folders::{create_folder, CreateFolderRequest};
use crate::AppState;

use super::{mcp_request_meta, ToolError};

pub fn input_schema() -> Value {
    json!({
        "type": "object",
        "required": ["path"],
        "properties": {
            "path": {
                "type": "string",
                "description": "Absolute, case-sensitive path of the folder to create. The parent must already exist; new folders are not auto-created. Example: \"/Documents/Drafts\"."
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
    let segments = path::parse(path_str)?;
    let (name, parent_segments) = segments
        .split_last()
        .ok_or_else(|| ToolError::invalid("cannot create the root folder"))?;

    let parent = path::resolve_folder(state, user.0.id, parent_segments).await?;
    let parent_id_str = parent.as_ref().map(|f| f.id.to_hex()).unwrap_or_default();

    let req = CreateFolderRequest {
        name: name.clone(),
        parent_id: Some(parent_id_str),
        storage_id: None,
    };

    let resp = create_folder(
        State(state.clone()),
        user.clone(),
        mcp_request_meta(),
        Json(req),
    )
    .await
    .map_err(|e| ToolError::exec(e.to_string()))?;
    let body = resp.1 .0;

    Ok(json!({
        "id": body.id,
        "path": path_str.trim_end_matches('/').to_string(),
        "name": body.name,
        "created_at": body.created_at,
    }))
}
