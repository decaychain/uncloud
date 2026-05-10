mod common;

use axum::http::StatusCode;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use common::TestApp;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const TEST_REDIRECT: &str = "http://localhost:9999/cb";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn pkce() -> (String, String) {
    let verifier: String = (0..64)
        .map(|_| {
            let r = rand::random::<u8>() % 62;
            match r {
                0..=9 => (b'0' + r) as char,
                10..=35 => (b'a' + (r - 10)) as char,
                _ => (b'A' + (r - 36)) as char,
            }
        })
        .collect();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

/// Drive the OAuth flow end-to-end using whatever session cookie is
/// currently in the TestServer's jar. Returns a freshly-issued bearer
/// token whose scopes are exactly `scope` (single-scope strings only).
async fn mint_token(app: &TestApp, scope: &str) -> String {
    // Register a client.
    let reg = app
        .server
        .post("/oauth/register")
        .json(&json!({
            "client_name": "mcp test client",
            "redirect_uris": [TEST_REDIRECT],
            "token_endpoint_auth_method": "none",
            "scope": scope,
        }))
        .await;
    reg.assert_status(StatusCode::CREATED);
    let body: Value = reg.json();
    let client_id = body["client_id"].as_str().unwrap().to_string();

    let (verifier, challenge) = pkce();

    // Consent (uses the session cookie set by register_and_login).
    let consent = app
        .server
        .post("/api/v1/oauth/authorize")
        .json(&json!({
            "client_id": client_id,
            "redirect_uri": TEST_REDIRECT,
            "response_type": "code",
            "scope": scope,
            "state": "s",
            "code_challenge": challenge,
            "code_challenge_method": "S256",
            "decision": "allow",
        }))
        .await;
    consent.assert_status_ok();
    let consent_body: Value = consent.json();
    let redirect = consent_body["redirect_to"].as_str().unwrap().to_string();
    let code = redirect
        .split('?')
        .nth(1)
        .unwrap()
        .split('&')
        .find_map(|kv| kv.strip_prefix("code="))
        .unwrap()
        .to_string();

    let exch = app
        .server
        .post("/oauth/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("client_id", &client_id),
            ("redirect_uri", TEST_REDIRECT),
            ("code_verifier", &verifier),
        ])
        .await;
    exch.assert_status_ok();
    let token_body: Value = exch.json();
    token_body["access_token"].as_str().unwrap().to_string()
}

async fn rpc(app: &TestApp, method: &str, params: Value) -> (StatusCode, Value, Option<String>) {
    let resp = app
        .server
        .post("/mcp")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        }))
        .await;
    let status = resp.status_code();
    let session = resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let body = if status == StatusCode::OK {
        resp.json::<Value>()
    } else {
        Value::Null
    };
    (status, body, session)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn initialize_returns_protocol_and_tools_capability() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let (status, body, session) = rpc(
        &app,
        "initialize",
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "test", "version": "0" }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["result"]["protocolVersion"], "2025-06-18");
    assert!(body["result"]["capabilities"]["tools"].is_object());
    assert_eq!(body["result"]["serverInfo"]["name"], "uncloud");
    assert!(session.is_some(), "expected mcp-session-id header");

    app.cleanup().await;
}

#[tokio::test]
async fn ping_returns_empty_result() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let (status, body, _) = rpc(&app, "ping", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["result"].is_object());
    assert!(body["error"].is_null());

    app.cleanup().await;
}

#[tokio::test]
async fn tools_list_advertises_three_read_only_tools() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let (status, body, _) = rpc(&app, "tools/list", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let tools = body["result"]["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 3);
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"list_files"));
    assert!(names.contains(&"read_file"));
    assert!(names.contains(&"search_files"));
    for t in tools {
        assert!(
            t["inputSchema"].is_object(),
            "tool {} missing inputSchema",
            t["name"]
        );
        assert_eq!(t["inputSchema"]["type"], "object");
    }

    app.cleanup().await;
}

#[tokio::test]
async fn tools_call_list_files_returns_user_root_listing() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;
    app.upload("greeting.txt", b"hello world", "text/plain")
        .await;

    let (status, body, _) = rpc(
        &app,
        "tools/call",
        json!({ "name": "list_files", "arguments": { "path": "/" } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let result = &body["result"];
    assert_eq!(result["isError"], false);
    let text = result["content"][0]["text"].as_str().expect("text content");
    let parsed: Value = serde_json::from_str(text).expect("inner json");
    let files = parsed["files"].as_array().expect("files array");
    let entry = files
        .iter()
        .find(|f| f["name"] == "greeting.txt")
        .expect("greeting.txt in listing");
    assert_eq!(entry["path"], "/greeting.txt");

    app.cleanup().await;
}

#[tokio::test]
async fn tools_call_read_file_returns_text_content() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;
    app.upload("notes.txt", b"line one\nline two", "text/plain")
        .await;

    let (status, body, _) = rpc(
        &app,
        "tools/call",
        json!({ "name": "read_file", "arguments": { "path": "/notes.txt" } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = body["result"]["content"][0]["text"]
        .as_str()
        .expect("text content");
    let parsed: Value = serde_json::from_str(text).expect("inner json");
    assert_eq!(parsed["content"], "line one\nline two");
    assert_eq!(parsed["source"], "raw");
    assert_eq!(parsed["file"]["path"], "/notes.txt");

    app.cleanup().await;
}

#[tokio::test]
async fn tools_call_read_file_refuses_binary_mime() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;
    app.upload("blob.bin", &[0u8, 1, 2, 3], "application/octet-stream")
        .await;

    let (status, body, _) = rpc(
        &app,
        "tools/call",
        json!({ "name": "read_file", "arguments": { "path": "/blob.bin" } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"]["isError"], true);
    let msg = body["result"]["content"][0]["text"]
        .as_str()
        .expect("error message");
    assert!(
        msg.contains("text-like"),
        "expected text-like refusal, got: {}",
        msg
    );

    app.cleanup().await;
}

#[tokio::test]
async fn tools_call_search_files_handles_disabled_search() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    // Test fixtures have search disabled (no Meilisearch URL configured).
    let (status, body, _) = rpc(
        &app,
        "tools/call",
        json!({ "name": "search_files", "arguments": { "query": "anything" } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let parsed: Value =
        serde_json::from_str(body["result"]["content"][0]["text"].as_str().unwrap())
            .expect("inner json");
    assert_eq!(parsed["disabled"], true);

    app.cleanup().await;
}

#[tokio::test]
async fn tools_call_unknown_tool_returns_method_not_found() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let (status, body, _) = rpc(
        &app,
        "tools/call",
        json!({ "name": "nonexistent_tool", "arguments": {} }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["error"]["code"], -32601);

    app.cleanup().await;
}

#[tokio::test]
async fn tools_call_invalid_params_for_read_file() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let (status, body, _) = rpc(
        &app,
        "tools/call",
        json!({ "name": "read_file", "arguments": {} }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["error"]["code"], -32602);

    app.cleanup().await;
}

#[tokio::test]
async fn missing_bearer_returns_401() {
    let app = TestApp::new().await;
    // No login — no cookie, no bearer.
    let resp = app
        .server
        .post("/mcp")
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }))
        .await;
    assert_eq!(resp.status_code(), StatusCode::UNAUTHORIZED);
    app.cleanup().await;
}

#[tokio::test]
async fn batch_request_rejected_with_invalid_request() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let resp = app
        .server
        .post("/mcp")
        .json(&json!([
            { "jsonrpc": "2.0", "id": 1, "method": "ping" },
            { "jsonrpc": "2.0", "id": 2, "method": "ping" }
        ]))
        .await;
    assert_eq!(resp.status_code(), StatusCode::OK);
    let body: Value = resp.json();
    assert_eq!(body["error"]["code"], -32600);

    app.cleanup().await;
}

#[tokio::test]
async fn notification_returns_202() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let resp = app
        .server
        .post("/mcp")
        .json(&json!({ "jsonrpc": "2.0", "method": "ping" }))
        .await;
    assert_eq!(resp.status_code(), StatusCode::ACCEPTED);

    app.cleanup().await;
}

#[tokio::test]
async fn oauth_token_with_files_read_can_call_list_files() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;
    app.upload("readable.txt", b"contents", "text/plain").await;
    let token = mint_token(&app, "files:read").await;

    // Logout so the cookie no longer satisfies auth — bearer must do it.
    let _ = app.server.post("/api/auth/logout").await;

    let resp = app
        .server
        .post("/mcp")
        .add_header("authorization", format!("Bearer {}", token))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "list_files", "arguments": { "path": "/" } }
        }))
        .await;
    assert_eq!(resp.status_code(), StatusCode::OK);
    let body: Value = resp.json();
    assert_eq!(body["result"]["isError"], false);

    app.cleanup().await;
}

#[tokio::test]
async fn oauth_token_without_files_read_is_blocked() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;
    let token = mint_token(&app, "files:write").await;

    // Logout so the bearer is the only credential.
    let _ = app.server.post("/api/auth/logout").await;

    let resp = app
        .server
        .post("/mcp")
        .add_header("authorization", format!("Bearer {}", token))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "list_files", "arguments": {} }
        }))
        .await;
    assert_eq!(resp.status_code(), StatusCode::OK);
    let body: Value = resp.json();
    assert_eq!(body["error"]["code"], -32002);

    // The unscoped bearer must still be allowed to discover what the
    // server can do — initialize/tools/list don't require any scope.
    let list = app
        .server
        .post("/mcp")
        .add_header("authorization", format!("Bearer {}", token))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list"
        }))
        .await;
    assert_eq!(list.status_code(), StatusCode::OK);
    let list_body: Value = list.json();
    assert!(list_body["result"]["tools"].is_array());

    app.cleanup().await;
}

#[tokio::test]
async fn read_file_rejects_path_with_dotdot() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let (status, body, _) = rpc(
        &app,
        "tools/call",
        json!({ "name": "read_file", "arguments": { "path": "/../etc/passwd" } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["error"]["code"], -32602);

    app.cleanup().await;
}

#[tokio::test]
async fn read_file_rejects_path_with_backslash() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let (status, body, _) = rpc(
        &app,
        "tools/call",
        json!({ "name": "read_file", "arguments": { "path": r"/foo\bar" } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["error"]["code"], -32602);

    app.cleanup().await;
}

#[tokio::test]
async fn read_file_rejects_relative_path() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let (status, body, _) = rpc(
        &app,
        "tools/call",
        json!({ "name": "read_file", "arguments": { "path": "notes.txt" } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["error"]["code"], -32602);

    app.cleanup().await;
}

#[tokio::test]
async fn read_file_returns_is_error_when_path_does_not_exist() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let (status, body, _) = rpc(
        &app,
        "tools/call",
        json!({ "name": "read_file", "arguments": { "path": "/missing.txt" } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"]["isError"], true);

    app.cleanup().await;
}
