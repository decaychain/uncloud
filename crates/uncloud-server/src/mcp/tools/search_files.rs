use std::collections::HashMap;
use std::sync::Arc;

use mongodb::bson::{doc, oid::ObjectId, Bson};
use serde_json::{json, Value};

use crate::mcp::path;
use crate::middleware::auth::AuthUser;
use crate::models::file::File;
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

    // Hits arrive as the indexed snapshot; refetch to get the live
    // parent_id chain (and to drop hits the caller no longer owns or
    // that have since been soft-deleted). Single Mongo query for the
    // whole set, then build paths on the resolved Files.
    let ids: Vec<ObjectId> = hits
        .iter()
        .filter_map(|h| ObjectId::parse_str(&h.id).ok())
        .collect();
    let files: Vec<File> = if ids.is_empty() {
        Vec::new()
    } else {
        let coll = state.db.collection::<File>("files");
        let mut out = Vec::new();
        let mut cursor = coll
            .find(doc! {
                "_id": { "$in": ids },
                "owner_id": user.id,
                "deleted_at": Bson::Null,
            })
            .await
            .map_err(|e| ToolError::exec(format!("file lookup failed: {}", e)))?;
        while cursor.advance().await.unwrap_or(false) {
            if let Ok(f) = cursor.deserialize_current() {
                out.push(f);
            }
        }
        out
    };

    // Preserve Meilisearch's relevance order by index lookup.
    let by_id: HashMap<ObjectId, File> = files.into_iter().map(|f| (f.id, f)).collect();
    let mut out_hits = Vec::new();
    for hit in &hits {
        let Ok(oid) = ObjectId::parse_str(&hit.id) else {
            continue;
        };
        let Some(file) = by_id.get(&oid) else {
            continue;
        };
        let p = path::build_for_file(state, file).await;
        out_hits.push(json!({
            "id": file.id.to_hex(),
            "path": p,
            "name": file.name,
            "mime_type": file.mime_type,
            "size_bytes": file.size_bytes,
        }));
    }

    Ok(json!({
        "disabled": false,
        "hits": out_hits,
    }))
}
