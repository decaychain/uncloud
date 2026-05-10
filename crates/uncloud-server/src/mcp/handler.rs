//! `POST /mcp` — JSON-RPC dispatcher.
//!
//! The auth middleware has already resolved the bearer; if the request
//! reached us, `AuthUser` is in extensions (the middleware 401s
//! otherwise). `Scopes` may be `Some(...)` for OAuth-issued tokens or
//! `None` for sessions / legacy PATs (full access).
//!
//! Per the design doc:
//!   - `initialize` / `ping` / `tools/list` are open to any
//!     authenticated bearer.
//!   - `tools/call` checks the named tool's `required_scope` against
//!     the request's `Scopes` and returns -32002 if missing.
//!   - JSON-RPC batches are rejected with -32600. Notifications (no `id`)
//!     are accepted but produce no response per spec.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use mongodb::bson::oid::ObjectId;
use serde_json::{json, Value};

use crate::middleware::auth::{AuthUser, Scopes};
use crate::AppState;

use super::jsonrpc::{
    JsonRpcRequest, JsonRpcResponse, AUTH_REQUIRED, INTERNAL_ERROR, INVALID_PARAMS,
    INVALID_REQUEST, METHOD_NOT_FOUND, PROTOCOL_VERSION, SCOPE_REQUIRED,
};
use super::tools;

const MCP_SESSION_HEADER: &str = "mcp-session-id";

/// Single Axum entry point for `/mcp`. Extracts the bearer-derived
/// `AuthUser` and `Scopes`, parses the JSON-RPC body, dispatches, and
/// echoes a fresh `Mcp-Session-Id` on the response (clients send it
/// back; we don't key state off it in v1).
pub async fn mcp_handler(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Scopes(scopes): Scopes,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    // Reject batches early: an array body is the JSON-RPC batch shape.
    // Inspector and Claude.ai don't batch; revisit if a real client asks.
    let leading = body.iter().find(|b| !b.is_ascii_whitespace()).copied();
    if leading == Some(b'[') {
        return jsonrpc_response(
            JsonRpcResponse::err(
                Value::Null,
                INVALID_REQUEST,
                "JSON-RPC batches are not supported",
            ),
            None,
        );
    }

    let req: JsonRpcRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return jsonrpc_response(
                JsonRpcResponse::err(
                    Value::Null,
                    INVALID_REQUEST,
                    format!("Invalid JSON-RPC request: {}", e),
                ),
                None,
            );
        }
    };

    // The session header is opaque in v1 — we mint one on initialize and
    // echo what the client sends on subsequent calls so logs correlate.
    let session_id = headers
        .get(MCP_SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| ObjectId::new().to_hex());

    // Notifications (no id) per JSON-RPC: process and return an empty
    // body. Per MCP, we don't ship notifications inbound today, but the
    // spec mandates we accept them silently.
    let id = match &req.id {
        Some(v) => v.clone(),
        None => {
            // Accept-and-drop. 202 with empty body matches the
            // Streamable-HTTP recommendation.
            let mut resp = (StatusCode::ACCEPTED, "").into_response();
            apply_session_header(resp.headers_mut(), &session_id);
            return resp;
        }
    };

    let response = dispatch(
        &state,
        &auth_user,
        scopes.as_deref(),
        req.method.as_str(),
        &req.params,
        id,
    )
    .await;
    jsonrpc_response(response, Some(&session_id))
}

async fn dispatch(
    state: &Arc<AppState>,
    user: &AuthUser,
    scopes: Option<&[String]>,
    method: &str,
    params: &Value,
    id: Value,
) -> JsonRpcResponse {
    match method {
        "initialize" => JsonRpcResponse::ok(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": {
                    "name": "uncloud",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "instructions": "Uncloud personal cloud storage. Read-only file tools (list_files, read_file, search_files). All tools require the files:read OAuth scope.",
            }),
        ),

        "ping" => JsonRpcResponse::ok(id, json!({})),

        "tools/list" => JsonRpcResponse::ok(
            id,
            json!({
                "tools": tools::TOOLS.iter().map(|t| json!({
                    "name": t.name,
                    "description": t.description,
                    "inputSchema": (t.input_schema)(),
                })).collect::<Vec<_>>()
            }),
        ),

        "tools/call" => tools_call(state, user, scopes, params, id).await,

        // Notifications and unsupported methods. The spec lists a few
        // (e.g. logging/setLevel) we don't implement; -32601 is the
        // correct response per JSON-RPC.
        other => JsonRpcResponse::err(id, METHOD_NOT_FOUND, format!("Method not found: {}", other)),
    }
}

async fn tools_call(
    state: &Arc<AppState>,
    user: &AuthUser,
    scopes: Option<&[String]>,
    params: &Value,
    id: Value,
) -> JsonRpcResponse {
    let name = match params.get("name").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => {
            return JsonRpcResponse::err(id, INVALID_PARAMS, "tools/call requires `name`");
        }
    };

    let descriptor = match tools::find(name) {
        Some(d) => d,
        None => {
            return JsonRpcResponse::err(id, METHOD_NOT_FOUND, format!("Unknown tool: {}", name));
        }
    };

    if !scope_allows(scopes, descriptor.required_scope) {
        return JsonRpcResponse::err(
            id,
            SCOPE_REQUIRED,
            format!("Scope `{}` required", descriptor.required_scope),
        );
    }

    let args = params.get("arguments").cloned().unwrap_or(Value::Null);
    let args = if args.is_null() { json!({}) } else { args };

    match tools::dispatch(name, &args, state, user).await {
        Ok(content) => JsonRpcResponse::ok(id, tools::success_result(content)),
        Err(tools::ToolError::InvalidParams(msg)) => JsonRpcResponse::err(id, INVALID_PARAMS, msg),
        Err(tools::ToolError::NotFound(msg)) => JsonRpcResponse::err(id, METHOD_NOT_FOUND, msg),
        Err(tools::ToolError::Execution(msg)) => {
            // Tool ran but the operation failed — JSON-RPC success with
            // isError: true, per MCP convention.
            JsonRpcResponse::ok(id, tools::error_result(&msg))
        }
    }
}

/// `Some(scopes)` from an OAuth token: must include `required`.
/// `None` (session cookie / legacy PAT): full access, allow.
fn scope_allows(scopes: Option<&[String]>, required: &str) -> bool {
    match scopes {
        None => true,
        Some(s) => s.iter().any(|sc| sc == required),
    }
}

fn jsonrpc_response(response: JsonRpcResponse, session_id: Option<&str>) -> Response {
    let mut resp = Json(response).into_response();
    if let Some(sid) = session_id {
        apply_session_header(resp.headers_mut(), sid);
    }
    resp
}

fn apply_session_header(headers: &mut HeaderMap, session_id: &str) {
    if let Ok(val) = HeaderValue::from_str(session_id) {
        headers.insert(HeaderName::from_static(MCP_SESSION_HEADER), val);
    }
}

// AUTH_REQUIRED is referenced by the auth middleware path: when a
// request arrives without a valid bearer the middleware returns 401
// before reaching this handler, so AUTH_REQUIRED is currently only
// surfaced via the constant for documentation/test-fixture parity.
// Suppress dead-code warning by referencing it here.
#[allow(dead_code)]
const _UNUSED_AUTH_REQUIRED: i32 = AUTH_REQUIRED;
#[allow(dead_code)]
const _UNUSED_INTERNAL: i32 = INTERNAL_ERROR;
