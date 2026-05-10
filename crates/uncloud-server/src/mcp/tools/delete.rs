use std::sync::Arc;

use axum::extract::{Path as AxumPath, State};
use serde_json::{json, Value};

use crate::mcp::path;
use crate::middleware::auth::AuthUser;
use crate::routes::files::delete_file;
use crate::routes::folders::delete_folder;
use crate::AppState;

use super::{mcp_request_meta, ToolError};

pub fn input_schema() -> Value {
    json!({
        "type": "object",
        "required": ["path"],
        "properties": {
            "path": {
                "type": "string",
                "description": "Absolute path of the file or folder to soft-delete. Folders are deleted recursively."
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

    let target = path::resolve_target(state, user.0.id, &segments).await?;

    match target {
        path::Target::File(file) => {
            let id = file.id.to_hex();
            delete_file(
                State(state.clone()),
                user.clone(),
                mcp_request_meta(),
                AxumPath(id.clone()),
            )
            .await
            .map_err(|e| ToolError::exec(e.to_string()))?;
            Ok(json!({
                "kind": "file",
                "id": id,
                "path": path_str.trim_end_matches('/'),
                "trashed": true,
            }))
        }
        path::Target::Folder(folder) => {
            let id = folder.id.to_hex();
            delete_folder(
                State(state.clone()),
                user.clone(),
                mcp_request_meta(),
                AxumPath(id.clone()),
            )
            .await
            .map_err(|e| ToolError::exec(e.to_string()))?;
            Ok(json!({
                "kind": "folder",
                "id": id,
                "path": path_str.trim_end_matches('/'),
                "trashed": true,
            }))
        }
    }
}
