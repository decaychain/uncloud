mod common;

use axum::http::StatusCode;
use common::TestApp;

// ── Registration ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn register_success() {
    let app = TestApp::new().await;
    let res = app
        .server
        .post("/api/auth/register")
        .json(&serde_json::json!({
            "username": "alice",
            "email": "alice@example.com",
            "password": "password123!"
        }))
        .await;

    res.assert_status(StatusCode::CREATED);
    let body: serde_json::Value = res.json();
    assert_eq!(body["username"], "alice");
    assert_eq!(body["email"], "alice@example.com");
    assert!(body["id"].as_str().is_some());
}

#[tokio::test]
async fn register_duplicate_username() {
    let app = TestApp::new().await;
    app.register("alice", "alice@example.com", "password123!").await;

    let res = app
        .server
        .post("/api/auth/register")
        .json(&serde_json::json!({
            "username": "alice",
            "email": "alice2@example.com",
            "password": "password123!"
        }))
        .await;

    res.assert_status(StatusCode::CONFLICT);
}

#[tokio::test]
async fn register_duplicate_email() {
    let app = TestApp::new().await;
    app.register("alice", "alice@example.com", "password123!").await;

    let res = app
        .server
        .post("/api/auth/register")
        .json(&serde_json::json!({
            "username": "alice2",
            "email": "alice@example.com",
            "password": "password123!"
        }))
        .await;

    res.assert_status(StatusCode::CONFLICT);
}

#[tokio::test]
async fn register_short_password() {
    let app = TestApp::new().await;
    let res = app
        .server
        .post("/api/auth/register")
        .json(&serde_json::json!({
            "username": "alice",
            "email": "alice@example.com",
            "password": "short"
        }))
        .await;

    res.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn register_short_username() {
    let app = TestApp::new().await;
    let res = app
        .server
        .post("/api/auth/register")
        .json(&serde_json::json!({
            "username": "ab",
            "email": "ab@example.com",
            "password": "password123!"
        }))
        .await;

    res.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn register_invalid_email() {
    let app = TestApp::new().await;
    let res = app
        .server
        .post("/api/auth/register")
        .json(&serde_json::json!({
            "username": "alice",
            "email": "not-an-email",
            "password": "password123!"
        }))
        .await;

    res.assert_status(StatusCode::BAD_REQUEST);
}

// ── Login ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn login_success() {
    let app = TestApp::new().await;
    app.register("alice", "alice@example.com", "password123!").await;

    let res = app
        .server
        .post("/api/auth/login")
        .json(&serde_json::json!({
            "username": "alice",
            "password": "password123!"
        }))
        .await;

    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["username"], "alice");
}

#[tokio::test]
async fn login_wrong_password() {
    let app = TestApp::new().await;
    app.register("alice", "alice@example.com", "password123!").await;

    let res = app
        .server
        .post("/api/auth/login")
        .json(&serde_json::json!({
            "username": "alice",
            "password": "wrongpassword"
        }))
        .await;

    res.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_unknown_user() {
    let app = TestApp::new().await;
    let res = app
        .server
        .post("/api/auth/login")
        .json(&serde_json::json!({
            "username": "nobody",
            "password": "password123!"
        }))
        .await;

    res.assert_status(StatusCode::UNAUTHORIZED);
}

// ── /me ───────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn me_authenticated() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let res = app.server.get("/api/auth/me").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["username"], "alice");
}

#[tokio::test]
async fn me_unauthenticated() {
    let app = TestApp::new().await;
    let res = app.server.get("/api/auth/me").await;
    res.assert_status(StatusCode::UNAUTHORIZED);
}

// ── Logout ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn logout_clears_session() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    // Confirm authenticated before logout
    app.server.get("/api/auth/me").await.assert_status_ok();

    // Logout
    app.server
        .post("/api/auth/logout")
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Cookie should be gone — /me returns 401
    app.server
        .get("/api/auth/me")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

// ── Sessions ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn session_list_includes_current() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let res = app.server.get("/api/auth/sessions").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    let sessions = body.as_array().expect("sessions array");
    assert!(!sessions.is_empty());
}

#[tokio::test]
async fn revoke_session_invalidates_access() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    // Get the session ID
    let sessions_res = app.server.get("/api/auth/sessions").await;
    let sessions: serde_json::Value = sessions_res.json();
    let session_id = sessions[0]["id"].as_str().expect("session id");

    // Revoke it
    app.server
        .delete(&format!("/api/auth/sessions/{}", session_id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Should now be unauthorised
    app.server
        .get("/api/auth/me")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}
