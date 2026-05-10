use std::sync::Arc;

use mongodb::bson::{doc, oid::ObjectId, Bson};
use serde_json::{json, Value};

use crate::middleware::auth::AuthUser;
use crate::models::file::File;
use crate::AppState;

use super::ToolError;

const MAX_BYTES_HARD_CAP: usize = 1_048_576; // 1 MiB
const MAX_BYTES_DEFAULT: usize = 65_536; // 64 KiB

pub fn input_schema() -> Value {
    json!({
        "type": "object",
        "required": ["file_id"],
        "properties": {
            "file_id": {
                "type": "string",
                "description": "File ObjectId hex."
            },
            "max_bytes": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_BYTES_HARD_CAP,
                "default": MAX_BYTES_DEFAULT,
                "description": "Maximum bytes to return. Output is truncated on a UTF-8 char boundary."
            }
        }
    })
}

pub async fn call(
    args: &Value,
    state: &Arc<AppState>,
    user: &AuthUser,
) -> Result<Value, ToolError> {
    let file_id_str = args
        .get("file_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::invalid("file_id is required"))?
        .trim();
    if file_id_str.is_empty() {
        return Err(ToolError::invalid("file_id is required"));
    }
    let file_id = ObjectId::parse_str(file_id_str)
        .map_err(|_| ToolError::invalid("file_id is not a valid ObjectId"))?;

    let max_bytes = args
        .get("max_bytes")
        .and_then(|v| v.as_i64())
        .map(|n| (n.max(1) as usize).min(MAX_BYTES_HARD_CAP))
        .unwrap_or(MAX_BYTES_DEFAULT);

    let files_coll = state.db.collection::<File>("files");
    let file = files_coll
        .find_one(doc! {
            "_id": file_id,
            "owner_id": user.id,
            "deleted_at": Bson::Null,
        })
        .await
        .map_err(|e| ToolError::exec(format!("file lookup failed: {}", e)))?
        .ok_or_else(|| ToolError::exec("file not found"))?;

    // PDFs: prefer the cached extracted text from the text-extract
    // pipeline. Falling back to live extraction here would mean either
    // running the subprocess on the request path (slow) or duplicating
    // its plumbing — neither earns its keep until somebody hits a real
    // gap. v1: cached or nothing.
    if file.mime_type == "application/pdf" {
        return match file.metadata.get("content_text") {
            Some(Bson::String(text)) => {
                let truncated = truncate_on_char_boundary(text.clone(), max_bytes);
                Ok(json!({
                    "file": file_summary(&file),
                    "content": truncated,
                    "source": "cached_extract",
                    "truncated": text.len() > max_bytes,
                }))
            }
            _ => Err(ToolError::exec(
                "PDF text extraction is still pending or failed; no cached text is available yet. Try again later.",
            )),
        };
    }

    if !is_text_like(&file.mime_type) {
        return Err(ToolError::exec(format!(
            "read_file only supports text-like content in v1; refusing to return raw bytes for mime type {}",
            file.mime_type
        )));
    }

    let backend = state
        .storage
        .get_backend(file.storage_id)
        .await
        .map_err(|e| ToolError::exec(format!("storage backend unavailable: {}", e)))?;
    let bytes = backend
        .read_all(&file.storage_path)
        .await
        .map_err(|e| ToolError::exec(format!("read failed: {}", e)))?;

    let read_size = bytes.len().min(max_bytes);
    let text = String::from_utf8_lossy(&bytes[..read_size]).into_owned();
    let truncated = truncate_on_char_boundary(text, max_bytes);

    Ok(json!({
        "file": file_summary(&file),
        "content": truncated,
        "source": "raw",
        "truncated": (bytes.len() as i64) > (max_bytes as i64),
    }))
}

fn file_summary(file: &File) -> Value {
    json!({
        "id": file.id.to_hex(),
        "name": file.name,
        "mime_type": file.mime_type,
        "size_bytes": file.size_bytes,
    })
}

fn is_text_like(mime: &str) -> bool {
    mime.starts_with("text/")
        || mime == "application/json"
        || mime == "application/xml"
        || mime == "application/javascript"
}

fn truncate_on_char_boundary(mut text: String, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text
}
