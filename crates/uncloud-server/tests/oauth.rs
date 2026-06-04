mod common;

use axum::http::StatusCode;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use common::TestApp;
use serde_json::Value;
use sha2::{Digest, Sha256};

const TEST_REDIRECT: &str = "http://localhost:9999/cb";

// =============================================================================
// Helpers
// =============================================================================

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

async fn register_client(app: &TestApp, scope: &str) -> String {
    let resp = app
        .server
        .post("/oauth/register")
        .json(&serde_json::json!({
            "client_name": "test client",
            "redirect_uris": [TEST_REDIRECT],
            "token_endpoint_auth_method": "none",
            "scope": scope,
        }))
        .await;
    resp.assert_status(StatusCode::CREATED);
    let body: Value = resp.json();
    body["client_id"].as_str().unwrap().to_string()
}

async fn authorize(
    app: &TestApp,
    client_id: &str,
    scope: &str,
    challenge: &str,
    state_value: &str,
    decision: &str,
) -> String {
    let resp = app
        .server
        .post("/api/v1/oauth/authorize")
        .json(&serde_json::json!({
            "client_id": client_id,
            "redirect_uri": TEST_REDIRECT,
            "response_type": "code",
            "scope": scope,
            "state": state_value,
            "code_challenge": challenge,
            "code_challenge_method": "S256",
            "decision": decision,
        }))
        .await;
    resp.assert_status_ok();
    let body: Value = resp.json();
    body["redirect_to"].as_str().unwrap().to_string()
}

fn extract_param(redirect: &str, key: &str) -> Option<String> {
    let qs = redirect.split('?').nth(1)?;
    for kv in qs.split('&') {
        let mut iter = kv.splitn(2, '=');
        let k = iter.next()?;
        let v = iter.next()?;
        if k == key {
            return Some(urlencoding::decode(v).ok()?.into_owned());
        }
    }
    None
}

// =============================================================================
// Discovery
// =============================================================================

#[tokio::test]
async fn discovery_authorization_server_metadata() {
    let app = TestApp::new().await;
    let resp = app
        .server
        .get("/.well-known/oauth-authorization-server")
        .await;
    resp.assert_status_ok();
    let body: Value = resp.json();
    assert!(body["issuer"].as_str().unwrap().starts_with("http"));
    assert!(body["authorization_endpoint"]
        .as_str()
        .unwrap()
        .ends_with("/oauth/authorize"));
    assert!(body["token_endpoint"]
        .as_str()
        .unwrap()
        .ends_with("/oauth/token"));
    assert!(body["registration_endpoint"]
        .as_str()
        .unwrap()
        .ends_with("/oauth/register"));
    let methods = body["code_challenge_methods_supported"].as_array().unwrap();
    assert!(methods.iter().any(|v| v == "S256"));
    let grants = body["grant_types_supported"].as_array().unwrap();
    assert!(grants.iter().any(|v| v == "authorization_code"));
    assert!(grants.iter().any(|v| v == "refresh_token"));
    let scopes = body["scopes_supported"].as_array().unwrap();
    assert!(scopes.iter().any(|v| v == "files:read"));
    app.cleanup().await;
}

#[tokio::test]
async fn discovery_protected_resource_metadata() {
    let app = TestApp::new().await;
    let resp = app
        .server
        .get("/.well-known/oauth-protected-resource")
        .await;
    resp.assert_status_ok();
    let body: Value = resp.json();
    assert!(body["resource"].as_str().unwrap().starts_with("http"));
    assert!(body["authorization_servers"].as_array().is_some());
    let methods = body["bearer_methods_supported"].as_array().unwrap();
    assert!(methods.iter().any(|v| v == "header"));
    app.cleanup().await;
}

// =============================================================================
// Dynamic client registration
// =============================================================================

#[tokio::test]
async fn dynamic_registration_succeeds() {
    let app = TestApp::new().await;
    let resp = app
        .server
        .post("/oauth/register")
        .json(&serde_json::json!({
            "client_name": "test app",
            "redirect_uris": [TEST_REDIRECT],
            "token_endpoint_auth_method": "none",
            "scope": "files:read"
        }))
        .await;
    resp.assert_status(StatusCode::CREATED);
    let body: Value = resp.json();
    assert!(body["client_id"].as_str().unwrap().starts_with("uc_"));
    assert_eq!(body["client_name"], "test app");
    app.cleanup().await;
}

#[tokio::test]
async fn dynamic_registration_rejects_http_non_localhost() {
    let app = TestApp::new().await;
    let resp = app
        .server
        .post("/oauth/register")
        .json(&serde_json::json!({
            "client_name": "evil",
            "redirect_uris": ["http://example.com/cb"],
            "token_endpoint_auth_method": "none"
        }))
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
    app.cleanup().await;
}

#[tokio::test]
async fn dynamic_registration_rejects_confidential_clients() {
    let app = TestApp::new().await;
    let resp = app
        .server
        .post("/oauth/register")
        .json(&serde_json::json!({
            "client_name": "secretive",
            "redirect_uris": [TEST_REDIRECT],
            "token_endpoint_auth_method": "client_secret_basic"
        }))
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
    app.cleanup().await;
}

// =============================================================================
// Authorize → token exchange (full flow)
// =============================================================================

#[tokio::test]
async fn authorize_and_exchange_with_pkce() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;
    let client_id = register_client(&app, "files:read").await;
    let (verifier, challenge) = pkce();

    let redirect = authorize(&app, &client_id, "files:read", &challenge, "xyz", "allow").await;
    let code = extract_param(&redirect, "code").expect("code param");
    let returned_state = extract_param(&redirect, "state").expect("state param");
    assert_eq!(returned_state, "xyz");

    let resp = app
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
    resp.assert_status_ok();
    let body: Value = resp.json();
    assert!(body["access_token"].as_str().unwrap().len() > 20);
    assert_eq!(body["token_type"], "Bearer");
    assert_eq!(body["scope"], "files:read");
    assert!(body["refresh_token"].as_str().unwrap().len() > 20);
    app.cleanup().await;
}

#[tokio::test]
async fn token_exchange_rejects_bad_verifier() {
    let app = TestApp::new().await;
    app.register_and_login("bob").await;
    let client_id = register_client(&app, "files:read").await;
    let (_verifier, challenge) = pkce();

    let redirect = authorize(&app, &client_id, "files:read", &challenge, "s", "allow").await;
    let code = extract_param(&redirect, "code").expect("code");

    let resp = app
        .server
        .post("/oauth/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("client_id", &client_id),
            ("redirect_uri", TEST_REDIRECT),
            (
                "code_verifier",
                "definitely-not-the-real-verifier-but-long-enough-to-pass",
            ),
        ])
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
    let body: Value = resp.json();
    assert_eq!(body["error"], "invalid_grant");
    app.cleanup().await;
}

#[tokio::test]
async fn token_exchange_rejects_consumed_code() {
    let app = TestApp::new().await;
    app.register_and_login("carol").await;
    let client_id = register_client(&app, "files:read").await;
    let (verifier, challenge) = pkce();

    let redirect = authorize(&app, &client_id, "files:read", &challenge, "s", "allow").await;
    let code = extract_param(&redirect, "code").expect("code");

    let first = app
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
    first.assert_status_ok();

    // Second exchange with the same code must fail.
    let second = app
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
    second.assert_status(StatusCode::BAD_REQUEST);
    app.cleanup().await;
}

#[tokio::test]
async fn token_exchange_rejects_redirect_mismatch() {
    let app = TestApp::new().await;
    app.register_and_login("dave").await;
    let client_id = register_client(&app, "files:read").await;
    let (verifier, challenge) = pkce();

    let redirect = authorize(&app, &client_id, "files:read", &challenge, "s", "allow").await;
    let code = extract_param(&redirect, "code").expect("code");

    let resp = app
        .server
        .post("/oauth/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("client_id", &client_id),
            ("redirect_uri", "http://localhost:9999/different"),
            ("code_verifier", &verifier),
        ])
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
    app.cleanup().await;
}

// =============================================================================
// Authorize denial
// =============================================================================

#[tokio::test]
async fn authorize_deny_returns_error_redirect() {
    let app = TestApp::new().await;
    app.register_and_login("eve").await;
    let client_id = register_client(&app, "files:read").await;
    let (_, challenge) = pkce();

    let redirect = authorize(&app, &client_id, "files:read", &challenge, "ok", "deny").await;
    let err = extract_param(&redirect, "error").expect("error param");
    assert_eq!(err, "access_denied");
    let st = extract_param(&redirect, "state").expect("state");
    assert_eq!(st, "ok");
    app.cleanup().await;
}

// =============================================================================
// Refresh + revoke
// =============================================================================

#[tokio::test]
async fn refresh_token_rotates_and_old_is_invalid() {
    let app = TestApp::new().await;
    app.register_and_login("frank").await;
    let client_id = register_client(&app, "files:read").await;
    let (verifier, challenge) = pkce();

    let redirect = authorize(&app, &client_id, "files:read", &challenge, "s", "allow").await;
    let code = extract_param(&redirect, "code").expect("code");

    let first: Value = app
        .server
        .post("/oauth/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("client_id", &client_id),
            ("redirect_uri", TEST_REDIRECT),
            ("code_verifier", &verifier),
        ])
        .await
        .json();
    let refresh_one = first["refresh_token"].as_str().unwrap().to_string();

    // Use refresh token → get a new pair.
    let rotated: Value = app
        .server
        .post("/oauth/token")
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh_one),
            ("client_id", &client_id),
        ])
        .await
        .json();
    let refresh_two = rotated["refresh_token"].as_str().unwrap().to_string();
    assert_ne!(refresh_one, refresh_two);

    // Old refresh must no longer work.
    let again = app
        .server
        .post("/oauth/token")
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh_one),
            ("client_id", &client_id),
        ])
        .await;
    again.assert_status(StatusCode::BAD_REQUEST);
    app.cleanup().await;
}

#[tokio::test]
async fn revoke_invalidates_access_token() {
    let app = TestApp::new().await;
    app.register_and_login("grace").await;
    let client_id = register_client(&app, "files:read").await;
    let (verifier, challenge) = pkce();

    let redirect = authorize(&app, &client_id, "files:read", &challenge, "s", "allow").await;
    let code = extract_param(&redirect, "code").expect("code");

    let token: Value = app
        .server
        .post("/oauth/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("client_id", &client_id),
            ("redirect_uri", TEST_REDIRECT),
            ("code_verifier", &verifier),
        ])
        .await
        .json();
    let access = token["access_token"].as_str().unwrap().to_string();

    // Use it once — works.
    let me_ok = app
        .server
        .get("/api/v1/auth/me")
        .add_header("Authorization", format!("Bearer {}", access))
        .clear_cookies()
        .await;
    me_ok.assert_status_ok();

    // Revoke.
    let rev = app
        .server
        .post("/oauth/revoke")
        .form(&[("token", access.as_str())])
        .await;
    rev.assert_status_ok();

    // Now the access token should fail.
    let me_fail = app
        .server
        .get("/api/v1/auth/me")
        .add_header("Authorization", format!("Bearer {}", access))
        .clear_cookies()
        .await;
    me_fail.assert_status(StatusCode::UNAUTHORIZED);
    app.cleanup().await;
}

// =============================================================================
// OAuth bearer authenticates real routes; legacy PAT still works
// =============================================================================

#[tokio::test]
async fn oauth_bearer_authenticates_request() {
    let app = TestApp::new().await;
    app.register_and_login("hank").await;
    let client_id = register_client(&app, "files:read").await;
    let (verifier, challenge) = pkce();

    let redirect = authorize(&app, &client_id, "files:read", &challenge, "s", "allow").await;
    let code = extract_param(&redirect, "code").expect("code");

    let token: Value = app
        .server
        .post("/oauth/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("client_id", &client_id),
            ("redirect_uri", TEST_REDIRECT),
            ("code_verifier", &verifier),
        ])
        .await
        .json();
    let access = token["access_token"].as_str().unwrap().to_string();

    let resp = app
        .server
        .get("/api/v1/auth/me")
        .add_header("Authorization", format!("Bearer {}", access))
        .clear_cookies()
        .await;
    resp.assert_status_ok();
    let body: Value = resp.json();
    assert_eq!(body["username"], "hank");
    app.cleanup().await;
}

#[tokio::test]
async fn legacy_pat_still_works_after_oauth_changes() {
    let app = TestApp::new().await;
    app.register_and_login("ivy").await;

    // Mint a legacy PAT.
    let pat: Value = app
        .server
        .post("/api/v1/auth/tokens")
        .json(&serde_json::json!({"name": "test PAT"}))
        .await
        .json();
    let token = pat["token"].as_str().unwrap().to_string();

    // Use it on a normal authenticated route — sessions and PATs both attach
    // Scopes(None), which means full access.
    let resp = app
        .server
        .get("/api/v1/auth/me")
        .add_header("Authorization", format!("Bearer {}", token))
        .clear_cookies()
        .await;
    resp.assert_status_ok();
    app.cleanup().await;
}

// =============================================================================
// Connected-apps management
// =============================================================================

#[tokio::test]
async fn connected_apps_lists_and_revokes() {
    let app = TestApp::new().await;
    app.register_and_login("jen").await;
    let client_id = register_client(&app, "files:read").await;
    let (verifier, challenge) = pkce();

    let redirect = authorize(&app, &client_id, "files:read", &challenge, "s", "allow").await;
    let code = extract_param(&redirect, "code").expect("code");

    let _: Value = app
        .server
        .post("/oauth/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("client_id", &client_id),
            ("redirect_uri", TEST_REDIRECT),
            ("code_verifier", &verifier),
        ])
        .await
        .json();

    let listed: Value = app.server.get("/api/v1/oauth/connected-apps").await.json();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["client_id"], client_id);

    let rev = app
        .server
        .delete(&format!("/api/v1/oauth/connected-apps/{}", client_id))
        .await;
    rev.assert_status(StatusCode::NO_CONTENT);

    let after: Value = app.server.get("/api/v1/oauth/connected-apps").await.json();
    assert_eq!(after.as_array().unwrap().len(), 0);
    app.cleanup().await;
}
