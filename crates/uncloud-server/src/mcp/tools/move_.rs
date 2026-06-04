use std::sync::Arc;

use axum::{
    extract::{Path as AxumPath, State},
    Json,
};
use serde_json::{json, Value};

use crate::mcp::path;
use crate::middleware::auth::AuthUser;
use crate::routes::files::{update_file, UpdateFileRequest};
use crate::routes::folders::{update_folder, UpdateFolderRequest};
use crate::AppState;

use super::{mcp_request_meta, ToolError};

pub fn input_schema() -> Value {
    json!({
        "type": "object",
        "required": ["source_path", "destination_path"],
        "properties": {
            "source_path": {
                "type": "string",
                "description": "Absolute path of the file or folder to move."
            },
            "destination_path": {
                "type": "string",
                "description": "Absolute path the source should end up at. The destination's parent folder must exist; the destination itself must not."
            }
        }
    })
}

pub async fn call(
    args: &Value,
    state: &Arc<AppState>,
    user: &AuthUser,
) -> Result<Value, ToolError> {
    let src_str = args
        .get("source_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::invalid("source_path is required"))?;
    let dst_str = args
        .get("destination_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::invalid("destination_path is required"))?;

    let src_segments = path::parse(src_str)?;
    let dst_segments = path::parse(dst_str)?;
    let (dst_name, dst_parent_segments) = dst_segments
        .split_last()
        .ok_or_else(|| ToolError::invalid("destination_path must include a name"))?;

    let target = path::resolve_target(state, user.0.id, &src_segments).await?;
    let dst_parent = path::resolve_folder(state, user.0.id, dst_parent_segments).await?;
    let dst_parent_id = dst_parent
        .as_ref()
        .map(|f| f.id.to_hex())
        .unwrap_or_default();

    match target {
        path::Target::File(file) => {
            let req = UpdateFileRequest {
                name: Some(dst_name.clone()),
                parent_id: Some(dst_parent_id),
            };
            let resp = update_file(
                State(state.clone()),
                user.clone(),
                mcp_request_meta(),
                AxumPath(file.id.to_hex()),
                Json(req),
            )
            .await
            .map_err(|e| ToolError::exec(e.to_string()))?;
            let body = resp.0;
            Ok(json!({
                "kind": "file",
                "id": body.id,
                "path": dst_str.trim_end_matches('/'),
                "name": body.name,
            }))
        }
        path::Target::Folder(folder) => {
            let req = UpdateFolderRequest {
                name: Some(dst_name.clone()),
                parent_id: Some(dst_parent_id),
                sync_strategy: None,
                gallery_include: None,
                music_include: None,
            };
            let resp = update_folder(
                State(state.clone()),
                user.clone(),
                mcp_request_meta(),
                AxumPath(folder.id.to_hex()),
                Json(req),
            )
            .await
            .map_err(|e| ToolError::exec(e.to_string()))?;
            let body = resp.0;
            Ok(json!({
                "kind": "folder",
                "id": body.id,
                "path": dst_str.trim_end_matches('/'),
                "name": body.name,
            }))
        }
    }
}
