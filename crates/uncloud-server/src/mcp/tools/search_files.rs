use std::sync::Arc;

use serde_json::{json, Value};

use crate::middleware::auth::AuthUser;
use crate::AppState;

use super::ToolError;

pub fn input_schema() -> Value {
    json!({
        "type": "object",
        "required": ["query"],
        "properties": {
            "query": {
                "type": "string",
                "minLength": 1,
                "description": "Full-text query (filenames + extracted content). Same query language as Meilisearch."
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": 50,
                "default": 10
            }
        }
    })
}

pub async fn call(
    args: &Value,
    state: &Arc<AppState>,
    user: &AuthUser,
) -> Result<Value, ToolError> {
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::invalid("query is required"))?
        .trim();
    if query.is_empty() {
        return Err(ToolError::invalid("query is required"));
    }
    let limit = args
        .get("limit")
        .and_then(|v| v.as_i64())
        .map(|n| n.clamp(1, 50) as usize)
        .unwrap_or(10);

    if !state.search.is_enabled() {
        // Don't synthesise a substring-on-Mongo fallback — the search
        // service already made that policy choice and the model gets
        // nothing useful from a half-broken result.
        return Ok(json!({
            "disabled": true,
            "hits": [],
        }));
    }

    let hits = state
        .search
        .search(user.id, query, limit)
        .await
        .map_err(ToolError::exec)?;

    Ok(json!({
        "disabled": false,
        "hits": hits.iter().map(|h| json!({
            "id": h.id,
            "name": h.name,
            "mime_type": h.mime_type,
            "parent_id": h.parent_id,
            "size_bytes": h.size_bytes,
        })).collect::<Vec<_>>(),
    }))
}
