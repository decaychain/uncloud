mod common;

use axum::http::StatusCode;
use common::TestApp;
use mongodb::bson::{doc, DateTime};

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
    app.register("alice", "alice@example.com", "password123!")
        .await;

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
    app.register("alice", "alice@example.com", "password123!")
        .await;

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
    app.register("alice", "alice@example.com", "password123!")
        .await;

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
    app.register("alice", "alice@example.com", "password123!")
        .await;

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
async fn active_session_near_expiry_is_renewed() {
    let app = TestApp::new().await;
    app.register("alice", "alice@example.com", "password123!")
        .await;

    let sessions = app.db.collection::<mongodb::bson::Document>("sessions");
    let fresh_session = sessions
        .find_one(doc! {})
        .await
        .expect("query fresh session")
        .expect("session exists");
    let session_id = fresh_session.get_object_id("_id").expect("session ID");
    let fresh_expiry = *fresh_session
        .get_datetime("expires_at")
        .expect("fresh session expiry");

    app.server.get("/api/auth/me").await.assert_status_ok();

    let unchanged_expiry = *sessions
        .find_one(doc! { "_id": session_id })
        .await
        .expect("query fresh session after activity")
        .expect("session exists")
        .get_datetime("expires_at")
        .expect("fresh session expiry");
    assert_eq!(fresh_expiry, unchanged_expiry);

    let near_expiry = DateTime::from_chrono(chrono::Utc::now() + chrono::Duration::minutes(5));
    sessions
        .update_one(
            doc! { "_id": session_id },
            doc! { "$set": { "expires_at": near_expiry } },
        )
        .await
        .expect("move session near expiry");

    app.server.get("/api/auth/me").await.assert_status_ok();

    let renewed = sessions
        .find_one(doc! { "_id": session_id })
        .await
        .expect("query renewed session")
        .expect("session exists")
        .get_datetime("expires_at")
        .expect("session expiry")
        .to_chrono();
    assert!(
        renewed > chrono::Utc::now() + chrono::Duration::minutes(50),
        "expected authenticated activity to renew the session, got {renewed}"
    );
}

#[tokio::test]
async fn revoke_session_invalidates_access() {
    let app = TestApp::new().await;
    // Use register only (not register_and_login) to ensure a single session.
    // register_and_login creates two sessions (register auto-login + explicit login),
    // and revoking sessions[0] might not match the cookie jar's active session.
    app.register("alice", "alice@example.com", "password123!")
        .await;

    // Get the session ID — should be exactly one
    let sessions_res = app.server.get("/api/auth/sessions").await;
    let sessions: serde_json::Value = sessions_res.json();
    let sessions_arr = sessions.as_array().expect("sessions array");
    assert_eq!(sessions_arr.len(), 1, "expected exactly one session");
    let session_id = sessions_arr[0]["id"].as_str().expect("session id");

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
