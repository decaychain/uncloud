mod common;

use axum::http::StatusCode;
use common::TestApp;

// ── Default strategy ──────────────────────────────────────────────────────────

#[tokio::test]
async fn default_effective_strategy_is_two_way() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let folder: serde_json::Value = app
        .server
        .post("/api/folders")
        .json(&serde_json::json!({ "name": "Docs" }))
        .await
        .json();

    // A newly created folder with no explicit strategy should inherit the
    // system default (TwoWay).
    assert_eq!(folder["sync_strategy"], "inherit");
    assert_eq!(folder["effective_strategy"], "two_way");
}

// ── Explicit strategy roundtrip ───────────────────────────────────────────────

#[tokio::test]
async fn explicit_strategy_roundtrip() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let folder: serde_json::Value = app
        .server
        .post("/api/folders")
        .json(&serde_json::json!({ "name": "Archive" }))
        .await
        .json();
    let folder_id = folder["id"].as_str().expect("folder id");

    // Set to DoNotSync
    let updated: serde_json::Value = app
        .server
        .put(&format!("/api/folders/{}", folder_id))
        .json(&serde_json::json!({ "sync_strategy": "do_not_sync" }))
        .await
        .json();

    assert_eq!(updated["sync_strategy"], "do_not_sync");
    assert_eq!(updated["effective_strategy"], "do_not_sync");
}

// ── Inheritance ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn child_inherits_parent_strategy() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    // Parent with explicit strategy
    let parent: serde_json::Value = app
        .server
        .post("/api/folders")
        .json(&serde_json::json!({ "name": "Backups" }))
        .await
        .json();
    let parent_id = parent["id"].as_str().expect("parent id");

    app.server
        .put(&format!("/api/folders/{}", parent_id))
        .json(&serde_json::json!({ "sync_strategy": "server_to_client" }))
        .await
        .assert_status_ok();

    // Child with Inherit (default)
    let child: serde_json::Value = app
        .server
        .post("/api/folders")
        .json(&serde_json::json!({ "name": "2024", "parent_id": parent_id }))
        .await
        .json();
    let child_id = child["id"].as_str().expect("child id");

    // Child's effective strategy should pick up the parent's
    let effective: serde_json::Value = app
        .server
        .get(&format!("/api/folders/{}/effective-strategy", child_id))
        .await
        .json();

    assert_eq!(effective["strategy"], "server_to_client");
}

// ── effective-strategy route ──────────────────────────────────────────────────

#[tokio::test]
async fn effective_strategy_route_requires_auth() {
    let app = TestApp::new().await;
    app.server
        .get("/api/folders/000000000000000000000001/effective-strategy")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

// ── Sync tree ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn sync_tree_contains_all_folders() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    // Create two top-level folders and a child
    let a: serde_json::Value = app
        .server
        .post("/api/folders")
        .json(&serde_json::json!({ "name": "FolderA" }))
        .await
        .json();
    let a_id = a["id"].as_str().expect("a id");

    app.server
        .post("/api/folders")
        .json(&serde_json::json!({ "name": "FolderB" }))
        .await;

    app.server
        .post("/api/folders")
        .json(&serde_json::json!({ "name": "ChildA", "parent_id": a_id }))
        .await;

    let tree: serde_json::Value = app.server.get("/api/sync/tree").await.json();
    // sync/tree returns { files: [...], folders: [...] }
    let folders = tree["folders"].as_array().expect("sync tree folders array");

    // All three folders should appear in the flat tree
    let names: Vec<&str> = folders
        .iter()
        .filter_map(|f| f["name"].as_str())
        .collect();
    assert!(names.contains(&"FolderA"));
    assert!(names.contains(&"FolderB"));
    assert!(names.contains(&"ChildA"));
}

#[tokio::test]
async fn sync_tree_requires_auth() {
    let app = TestApp::new().await;
    app.server
        .get("/api/sync/tree")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}
