//! Tool registry + per-tool implementations.
//!
//! Each tool has a static descriptor (name, description, JSON schema,
//! required scope) returned by `tools/list`, and an async `call` that
//! takes the parsed arguments + `AppState` + `AuthUser` and produces
//! either text content or a structured error message. The dispatcher in
//! `handler.rs` routes by name and converts errors into JSON-RPC error
//! responses.
//!
//! Output convention: a tool returns its result as a single JSON-encoded
//! string in a `text` content block. MCP supports richer content shapes
//! but every client today renders text fine and we don't have to invent
//! a per-tool output schema.

pub mod copy;
pub mod create_folder;
pub mod delete;
pub mod list_files;
pub mod move_;
pub mod read_file;
pub mod search_files;
pub mod write_file;

use serde_json::{json, Value};
use uncloud_common::SyncEventSource;

use crate::middleware::auth::AuthUser;
use crate::middleware::RequestMeta;
use crate::AppState;
use std::sync::Arc;

/// `RequestMeta` value used when an MCP tool calls a route handler.
/// Tagging the audit event with `SyncEventSource::Mcp` keeps the
/// activity feed honest about which surface caused the change.
pub fn mcp_request_meta() -> RequestMeta {
    RequestMeta {
        source: SyncEventSource::Mcp,
        client_id: None,
        client_os: None,
    }
}

pub struct ToolDescriptor {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: fn() -> Value,
    pub required_scope: &'static str,
}

/// Static list of every tool we expose via `tools/list`. New tools land
/// here. Order matters only for human-friendly listing.
pub const TOOLS: &[ToolDescriptor] = &[
    ToolDescriptor {
        name: "list_files",
        description: "List folders and files at the given absolute, case-sensitive path (e.g. \"/Documents\"). Use \"/\" or omit for the root. Each returned entry includes its absolute path.",
        input_schema: list_files::input_schema,
        required_scope: "files:read",
    },
    ToolDescriptor {
        name: "read_file",
        description: "Read a text-like file (text/*, JSON, XML, JS) or the cached extracted text of a PDF. Path is absolute and case-sensitive (e.g. \"/Documents/notes.txt\"). Output is capped to max_bytes (default 64 KiB, hard cap 1 MiB).",
        input_schema: read_file::input_schema,
        required_scope: "files:read",
    },
    ToolDescriptor {
        name: "search_files",
        description: "Full-text search over the user's files (filename + extracted content). Returns hits with absolute path, name, and mime_type.",
        input_schema: search_files::input_schema,
        required_scope: "files:read",
    },
    ToolDescriptor {
        name: "create_folder",
        description: "Create a new folder at the given absolute path. The parent must already exist; folders are not auto-created. Fails if a file or folder already exists at the path.",
        input_schema: create_folder::input_schema,
        required_scope: "files:write",
    },
    ToolDescriptor {
        name: "write_file",
        description: "Create a new text file at the given absolute path with UTF-8 content. The parent folder must exist. By default fails if the path already exists; pass overwrite: true to replace (Uncloud keeps the previous content as a version). Content cap is 1 MiB; binary uploads belong on the REST API.",
        input_schema: write_file::input_schema,
        required_scope: "files:write",
    },
    ToolDescriptor {
        name: "move",
        description: "Move or rename a file or folder. The source must exist; the destination's parent must exist; the destination itself must not. Works for both files and folders — the tool detects which.",
        input_schema: move_::input_schema,
        required_scope: "files:write",
    },
    ToolDescriptor {
        name: "copy",
        description: "Copy a file or folder to a new path. Folders are copied recursively. The destination's parent must exist; the destination itself must not. Works for both files and folders.",
        input_schema: copy::input_schema,
        required_scope: "files:write",
    },
    ToolDescriptor {
        name: "delete",
        description: "Move a file or folder to trash (soft-delete). Recursive for folders. Trash retention and permanent deletion are governed by the user's settings.",
        input_schema: delete::input_schema,
        required_scope: "files:delete",
    },
];

pub fn find(name: &str) -> Option<&'static ToolDescriptor> {
    TOOLS.iter().find(|t| t.name == name)
}

/// Dispatch a tool by name. The caller has already validated the scope.
/// Returns the JSON value to embed inside `result.content[0].text`.
pub async fn dispatch(
    name: &str,
    args: &Value,
    state: &Arc<AppState>,
    user: &AuthUser,
) -> Result<Value, ToolError> {
    match name {
        "list_files" => list_files::call(args, state, user).await,
        "read_file" => read_file::call(args, state, user).await,
        "search_files" => search_files::call(args, state, user).await,
        "create_folder" => create_folder::call(args, state, user).await,
        "write_file" => write_file::call(args, state, user).await,
        "move" => move_::call(args, state, user).await,
        "copy" => copy::call(args, state, user).await,
        "delete" => delete::call(args, state, user).await,
        _ => Err(ToolError::not_found(name)),
    }
}

/// Tool-side errors. The handler maps these to JSON-RPC errors or to a
/// successful response with `isError: true` depending on the kind.
#[derive(Debug)]
pub enum ToolError {
    /// Bad input — JSON-RPC -32602.
    InvalidParams(String),
    /// Unknown tool name — JSON-RPC -32601.
    NotFound(String),
    /// Tool ran but the operation failed (file not found, denied, etc).
    /// Returned as a successful JSON-RPC response with `isError: true`.
    Execution(String),
}

impl ToolError {
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::InvalidParams(msg.into())
    }
    pub fn not_found(name: &str) -> Self {
        Self::NotFound(format!("Unknown tool: {}", name))
    }
    pub fn exec(msg: impl Into<String>) -> Self {
        Self::Execution(msg.into())
    }
}

/// Helper: build the `result` body for a successful tool call.
pub fn success_result(content_json: Value) -> Value {
    json!({
        "content": [
            {
                "type": "text",
                "text": serde_json::to_string_pretty(&content_json)
                    .unwrap_or_else(|_| content_json.to_string())
            }
        ],
        "isError": false
    })
}

/// Helper: build the `result` body for an execution failure (the tool
/// ran, but the operation didn't succeed). Per MCP, this is a successful
/// JSON-RPC response with `isError: true`, not a JSON-RPC error.
pub fn error_result(message: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": message }],
        "isError": true
    })
}
