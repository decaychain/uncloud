mod common;

use axum::http::StatusCode;
use common::TestApp;
use uncloud_server::config::RegistrationMode;

// =============================================================================
// Registration modes
// =============================================================================

#[tokio::test]
async fn register_open_mode_succeeds() {
    let app = TestApp::with_registration(RegistrationMode::Open).await;
    let res = app
        .server
        .post("/api/auth/register")
        .json(&serde_json::json!({
            "username": "alice",
            "password": "password123!"
        }))
        .await;
    res.assert_status(StatusCode::CREATED);
    let body: serde_json::Value = res.json();
    assert_eq!(body["username"], "alice");
    assert_eq!(body["status"], "active");
    app.cleanup().await;
}

#[tokio::test]
async fn register_disabled_mode_rejects() {
    let app = TestApp::with_registration(RegistrationMode::Disabled).await;
    let res = app
        .server
        .post("/api/auth/register")
        .json(&serde_json::json!({
            "username": "alice",
            "password": "password123!"
        }))
        .await;
    res.assert_status(StatusCode::FORBIDDEN);
    app.cleanup().await;
}

#[tokio::test]
async fn register_approval_mode_creates_pending_user() {
    let app = TestApp::with_registration(RegistrationMode::Approval).await;
    let res = app
        .server
        .post("/api/auth/register")
        .json(&serde_json::json!({
            "username": "alice",
            "password": "password123!"
        }))
        .await;
    // Approval mode returns 202 Accepted for pending users
    res.assert_status(StatusCode::ACCEPTED);
    let body: serde_json::Value = res.json();
    assert_eq!(body["status"], "pending");
    app.cleanup().await;
}

#[tokio::test]
async fn pending_user_cannot_login() {
    let app = TestApp::with_registration(RegistrationMode::Approval).await;
    app.server
        .post("/api/auth/register")
        .json(&serde_json::json!({
            "username": "alice",
            "password": "password123!"
        }))
        .await;

    let res = app
        .server
        .post("/api/auth/login")
        .json(&serde_json::json!({
            "username": "alice",
            "password": "password123!"
        }))
        .await;
    res.assert_status(StatusCode::FORBIDDEN);
    app.cleanup().await;
}

#[tokio::test]
async fn invite_only_rejects_without_token() {
    let app = TestApp::with_registration(RegistrationMode::InviteOnly).await;
    let res = app
        .server
        .post("/api/auth/register")
        .json(&serde_json::json!({
            "username": "alice",
            "password": "password123!"
        }))
        .await;
    res.assert_status(StatusCode::FORBIDDEN);
    app.cleanup().await;
}

// =============================================================================
// Optional email
// =============================================================================

#[tokio::test]
async fn register_without_email_succeeds() {
    let app = TestApp::new().await;
    let res = app
        .server
        .post("/api/auth/register")
        .json(&serde_json::json!({
            "username": "alice",
            "password": "password123!"
        }))
        .await;
    res.assert_status(StatusCode::CREATED);
    let body: serde_json::Value = res.json();
    assert!(body["email"].is_null() || body["email"] == "");
    app.cleanup().await;
}

#[tokio::test]
async fn multiple_users_without_email_succeeds() {
    let app = TestApp::new().await;
    app.server
        .post("/api/auth/register")
        .json(&serde_json::json!({
            "username": "alice",
            "password": "password123!"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let res = app
        .server
        .post("/api/auth/register")
        .json(&serde_json::json!({
            "username": "bob",
            "password": "password123!"
        }))
        .await;
    res.assert_status(StatusCode::CREATED);
    app.cleanup().await;
}

#[tokio::test]
async fn register_with_email_succeeds() {
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
    assert_eq!(body["email"], "alice@example.com");
    app.cleanup().await;
}

// =============================================================================
// Registration creates a session (auto-login)
// =============================================================================

#[tokio::test]
async fn register_active_user_gets_session() {
    let app = TestApp::new().await;
    app.server
        .post("/api/auth/register")
        .json(&serde_json::json!({
            "username": "alice",
            "password": "password123!"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    // Should be authenticated immediately — no login needed
    let me = app.server.get("/api/auth/me").await;
    me.assert_status_ok();
    let body: serde_json::Value = me.json();
    assert_eq!(body["username"], "alice");
    app.cleanup().await;
}

// =============================================================================
// Login with email
// =============================================================================

#[tokio::test]
async fn login_with_email() {
    let app = TestApp::new().await;
    app.register("alice", "alice@example.com", "password123!").await;

    // Login using email instead of username
    let res = app
        .server
        .post("/api/auth/login")
        .json(&serde_json::json!({
            "username": "alice@example.com",
            "password": "password123!"
        }))
        .await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["username"], "alice");
    app.cleanup().await;
}

// =============================================================================
// Server info
// =============================================================================

#[tokio::test]
async fn server_info_returns_registration_mode() {
    let app = TestApp::with_registration(RegistrationMode::InviteOnly).await;
    let res = app.server.get("/api/auth/server-info").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["registration_mode"], "invite_only");
    assert!(body["version"].as_str().is_some());
    app.cleanup().await;
}

#[tokio::test]
async fn server_info_no_auth_required() {
    let app = TestApp::new().await;
    // No login — should still work
    app.server
        .get("/api/auth/server-info")
        .await
        .assert_status_ok();
    app.cleanup().await;
}

// =============================================================================
// Change password (self)
// =============================================================================

#[tokio::test]
async fn change_password_success() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let res = app
        .server
        .post("/api/auth/change-password")
        .json(&serde_json::json!({
            "current_password": "password123!",
            "new_password": "newpass456!"
        }))
        .await;
    res.assert_status(StatusCode::NO_CONTENT);

    // Logout first
    app.server.post("/api/auth/logout").await;

    // Old password no longer works
    let fail = app
        .server
        .post("/api/auth/login")
        .json(&serde_json::json!({
            "username": "alice",
            "password": "password123!"
        }))
        .await;
    fail.assert_status(StatusCode::UNAUTHORIZED);

    // New password works
    let ok = app
        .server
        .post("/api/auth/login")
        .json(&serde_json::json!({
            "username": "alice",
            "password": "newpass456!"
        }))
        .await;
    ok.assert_status_ok();
    app.cleanup().await;
}

#[tokio::test]
async fn change_password_wrong_current_fails() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let res = app
        .server
        .post("/api/auth/change-password")
        .json(&serde_json::json!({
            "current_password": "wrongpassword",
            "new_password": "newpass456!"
        }))
        .await;
    res.assert_status(StatusCode::BAD_REQUEST);
    app.cleanup().await;
}

#[tokio::test]
async fn change_password_too_short_fails() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let res = app
        .server
        .post("/api/auth/change-password")
        .json(&serde_json::json!({
            "current_password": "password123!",
            "new_password": "short"
        }))
        .await;
    res.assert_status(StatusCode::BAD_REQUEST);
    app.cleanup().await;
}

#[tokio::test]
async fn change_password_requires_auth() {
    let app = TestApp::new().await;
    let res = app
        .server
        .post("/api/auth/change-password")
        .json(&serde_json::json!({
            "current_password": "password123!",
            "new_password": "newpass456!"
        }))
        .await;
    res.assert_status(StatusCode::UNAUTHORIZED);
    app.cleanup().await;
}

// =============================================================================
// Admin: user management
// =============================================================================

#[tokio::test]
async fn admin_list_users() {
    let app = TestApp::new().await;
    app.create_admin_and_login("admin", "password123!").await;

    // Create a second user
    app.server
        .post("/api/admin/users")
        .json(&serde_json::json!({
            "username": "bob",
            "password": "password123!"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let res = app.server.get("/api/admin/users").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    let users = body.as_array().expect("users array");
    assert!(users.len() >= 2);
    app.cleanup().await;
}

#[tokio::test]
async fn admin_create_user() {
    let app = TestApp::new().await;
    app.create_admin_and_login("admin", "password123!").await;

    let res = app
        .server
        .post("/api/admin/users")
        .json(&serde_json::json!({
            "username": "newuser",
            "password": "password123!",
            "role": "user"
        }))
        .await;
    res.assert_status(StatusCode::CREATED);
    let body: serde_json::Value = res.json();
    assert_eq!(body["username"], "newuser");
    app.cleanup().await;
}

#[tokio::test]
async fn admin_reset_password() {
    let app = TestApp::new().await;
    app.create_admin_and_login("admin", "password123!").await;

    // Create a user
    let user: serde_json::Value = app
        .server
        .post("/api/admin/users")
        .json(&serde_json::json!({
            "username": "bob",
            "password": "oldpass123!"
        }))
        .await
        .json();
    let user_id = user["id"].as_str().expect("user id");

    // Admin resets password
    app.server
        .post(&format!("/api/admin/users/{}/reset-password", user_id))
        .json(&serde_json::json!({
            "new_password": "resetpass123!"
        }))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Bob can log in with new password (logout admin first)
    app.server.post("/api/auth/logout").await;
    let res = app
        .server
        .post("/api/auth/login")
        .json(&serde_json::json!({
            "username": "bob",
            "password": "resetpass123!"
        }))
        .await;
    res.assert_status_ok();
    app.cleanup().await;
}

#[tokio::test]
async fn admin_change_role_promote() {
    let app = TestApp::new().await;
    app.create_admin_and_login("admin", "password123!").await;

    // Create a regular user
    let user: serde_json::Value = app
        .server
        .post("/api/admin/users")
        .json(&serde_json::json!({
            "username": "bob",
            "password": "password123!"
        }))
        .await
        .json();
    let user_id = user["id"].as_str().expect("user id");

    // Promote to admin
    app.server
        .post(&format!("/api/admin/users/{}/role", user_id))
        .json(&serde_json::json!({ "role": "admin" }))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Verify in user list
    let users: serde_json::Value = app.server.get("/api/admin/users").await.json();
    let bob = users
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["username"] == "bob")
        .expect("bob in user list");
    assert_eq!(bob["role"], "admin");
    app.cleanup().await;
}

#[tokio::test]
async fn admin_change_role_demote() {
    let app = TestApp::new().await;
    app.create_admin_and_login("admin", "password123!").await;

    // Create user, promote, then demote
    let user: serde_json::Value = app
        .server
        .post("/api/admin/users")
        .json(&serde_json::json!({
            "username": "bob",
            "password": "password123!"
        }))
        .await
        .json();
    let user_id = user["id"].as_str().expect("user id");

    // Promote first
    app.server
        .post(&format!("/api/admin/users/{}/role", user_id))
        .json(&serde_json::json!({ "role": "admin" }))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Then demote
    app.server
        .post(&format!("/api/admin/users/{}/role", user_id))
        .json(&serde_json::json!({ "role": "user" }))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let users: serde_json::Value = app.server.get("/api/admin/users").await.json();
    let bob = users
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["username"] == "bob")
        .expect("bob in user list");
    assert_eq!(bob["role"], "user");
    app.cleanup().await;
}

#[tokio::test]
async fn admin_cannot_self_demote() {
    let app = TestApp::new().await;
    let admin: serde_json::Value =
        app.create_admin_and_login("admin", "password123!").await;
    let admin_id = admin["id"].as_str().expect("admin id");

    let res = app
        .server
        .post(&format!("/api/admin/users/{}/role", admin_id))
        .json(&serde_json::json!({ "role": "user" }))
        .await;
    res.assert_status(StatusCode::BAD_REQUEST);
    app.cleanup().await;
}

#[tokio::test]
async fn admin_approve_pending_user() {
    let app = TestApp::with_registration(RegistrationMode::Approval).await;

    // Register (creates pending user)
    app.server
        .post("/api/auth/register")
        .json(&serde_json::json!({
            "username": "alice",
            "password": "password123!"
        }))
        .await
        .assert_status(StatusCode::ACCEPTED);

    // Create admin and log in
    app.create_admin_and_login("admin", "password123!").await;

    // Find alice's ID
    let users: serde_json::Value = app.server.get("/api/admin/users").await.json();
    let alice = users
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["username"] == "alice")
        .expect("alice");
    let alice_id = alice["id"].as_str().expect("alice id");
    assert_eq!(alice["status"], "pending");

    // Approve
    app.server
        .post(&format!("/api/admin/users/{}/approve", alice_id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Alice can now log in (logout admin first)
    app.server.post("/api/auth/logout").await;
    let res = app
        .server
        .post("/api/auth/login")
        .json(&serde_json::json!({
            "username": "alice",
            "password": "password123!"
        }))
        .await;
    res.assert_status_ok();
    app.cleanup().await;
}

#[tokio::test]
async fn admin_disable_user_blocks_login() {
    let app = TestApp::new().await;
    app.create_admin_and_login("admin", "password123!").await;

    // Create user
    let user: serde_json::Value = app
        .server
        .post("/api/admin/users")
        .json(&serde_json::json!({
            "username": "bob",
            "password": "password123!"
        }))
        .await
        .json();
    let user_id = user["id"].as_str().expect("user id");

    // Disable
    app.server
        .post(&format!("/api/admin/users/{}/disable", user_id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Bob can't log in (logout admin first)
    app.server.post("/api/auth/logout").await;
    let res = app
        .server
        .post("/api/auth/login")
        .json(&serde_json::json!({
            "username": "bob",
            "password": "password123!"
        }))
        .await;
    res.assert_status(StatusCode::FORBIDDEN);
    app.cleanup().await;
}

#[tokio::test]
async fn admin_enable_user_restores_login() {
    let app = TestApp::new().await;
    app.create_admin_and_login("admin", "password123!").await;

    // Create and disable user
    let user: serde_json::Value = app
        .server
        .post("/api/admin/users")
        .json(&serde_json::json!({
            "username": "bob",
            "password": "password123!"
        }))
        .await
        .json();
    let user_id = user["id"].as_str().expect("user id");

    app.server
        .post(&format!("/api/admin/users/{}/disable", user_id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Re-enable
    app.server
        .post(&format!("/api/admin/users/{}/enable", user_id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Bob can log in again (logout admin first)
    app.server.post("/api/auth/logout").await;
    let res = app
        .server
        .post("/api/auth/login")
        .json(&serde_json::json!({
            "username": "bob",
            "password": "password123!"
        }))
        .await;
    res.assert_status_ok();
    app.cleanup().await;
}

#[tokio::test]
async fn non_admin_cannot_access_admin_routes() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    app.server
        .get("/api/admin/users")
        .await
        .assert_status(StatusCode::FORBIDDEN);

    app.server
        .post("/api/admin/users")
        .json(&serde_json::json!({
            "username": "bob",
            "password": "password123!"
        }))
        .await
        .assert_status(StatusCode::FORBIDDEN);
    app.cleanup().await;
}

// =============================================================================
// Admin: invites
// =============================================================================

#[tokio::test]
async fn admin_create_and_use_invite() {
    let app = TestApp::with_registration(RegistrationMode::InviteOnly).await;
    app.create_admin_and_login("admin", "password123!").await;

    // Create invite
    let invite: serde_json::Value = app
        .server
        .post("/api/admin/invites")
        .json(&serde_json::json!({ "comment": "For Bob" }))
        .await
        .json();
    let token = invite["token"].as_str().expect("invite token");

    // Validate invite (public endpoint)
    let info: serde_json::Value = app
        .server
        .get(&format!("/api/auth/invite/{}", token))
        .await
        .json();
    assert_eq!(info["valid"], true);

    // Register using invite (logout admin first)
    app.server.post("/api/auth/logout").await;
    let res = app
        .server
        .post("/api/auth/register")
        .json(&serde_json::json!({
            "username": "bob",
            "password": "password123!",
            "invite_token": token
        }))
        .await;
    res.assert_status(StatusCode::CREATED);
    let body: serde_json::Value = res.json();
    assert_eq!(body["username"], "bob");
    assert_eq!(body["status"], "active");
    app.cleanup().await;
}

#[tokio::test]
async fn invite_cannot_be_reused() {
    let app = TestApp::with_registration(RegistrationMode::InviteOnly).await;
    app.create_admin_and_login("admin", "password123!").await;

    let invite: serde_json::Value = app
        .server
        .post("/api/admin/invites")
        .json(&serde_json::json!({}))
        .await
        .json();
    let token = invite["token"].as_str().expect("invite token");

    // First use succeeds (logout admin first)
    app.server.post("/api/auth/logout").await;
    app.server
        .post("/api/auth/register")
        .json(&serde_json::json!({
            "username": "bob",
            "password": "password123!",
            "invite_token": token
        }))
        .await
        .assert_status(StatusCode::CREATED);

    // Second use fails
    app.server.post("/api/auth/logout").await;
    let res = app
        .server
        .post("/api/auth/register")
        .json(&serde_json::json!({
            "username": "carol",
            "password": "password123!",
            "invite_token": token
        }))
        .await;
    res.assert_status(StatusCode::BAD_REQUEST);
    app.cleanup().await;
}

#[tokio::test]
async fn admin_delete_invite() {
    let app = TestApp::new().await;
    app.create_admin_and_login("admin", "password123!").await;

    let invite: serde_json::Value = app
        .server
        .post("/api/admin/invites")
        .json(&serde_json::json!({}))
        .await
        .json();
    let invite_id = invite["id"].as_str().expect("invite id");

    app.server
        .delete(&format!("/api/admin/invites/{}", invite_id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Invite list should be empty
    let list: serde_json::Value = app.server.get("/api/admin/invites").await.json();
    let invites = list.as_array().expect("invites array");
    assert!(invites.is_empty());
    app.cleanup().await;
}

// =============================================================================
// Demo mode
// =============================================================================

#[tokio::test]
async fn demo_login_creates_ephemeral_user() {
    let app = TestApp::with_registration(RegistrationMode::Demo).await;

    let res = app.server.post("/api/auth/demo").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert!(body["username"].as_str().unwrap().starts_with("demo-"));

    // Should be authenticated
    let me = app.server.get("/api/auth/me").await;
    me.assert_status_ok();
    app.cleanup().await;
}

#[tokio::test]
async fn demo_login_rejected_in_non_demo_mode() {
    let app = TestApp::with_registration(RegistrationMode::Open).await;
    let res = app.server.post("/api/auth/demo").await;
    res.assert_status(StatusCode::FORBIDDEN);
    app.cleanup().await;
}

// =============================================================================
// Disabled user's existing session is rejected
// =============================================================================

#[tokio::test]
async fn disabled_user_existing_session_rejected() {
    let app = TestApp::new().await;

    // Register bob and verify he's authenticated
    app.register("bob", "", "password123!").await;
    app.login("bob", "password123!").await;
    app.server.get("/api/auth/me").await.assert_status_ok();

    // Admin disables bob (directly in DB since we can't have two sessions on same TestApp easily)
    let collection = app.db.collection::<mongodb::bson::Document>("users");
    collection
        .update_one(
            mongodb::bson::doc! { "username": "bob" },
            mongodb::bson::doc! { "$set": { "status": "disabled" } },
        )
        .await
        .expect("disable bob");

    // Bob's existing session should now be rejected
    app.server
        .get("/api/auth/me")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    app.cleanup().await;
}
