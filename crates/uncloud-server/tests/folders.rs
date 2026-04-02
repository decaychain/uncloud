mod common;

use axum::http::StatusCode;
use common::TestApp;

// ── Create & list ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn create_folder() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let res = app
        .server
        .post("/api/folders")
        .json(&serde_json::json!({ "name": "Photos" }))
        .await;

    res.assert_status(StatusCode::CREATED);
    let body: serde_json::Value = res.json();
    assert_eq!(body["name"], "Photos");
    assert!(body["id"].as_str().is_some());

    // Appears in listing
    let listing: serde_json::Value = app
        .server
        .get("/api/folders")
        .await
        .json();
    let folders = listing.as_array().expect("folders array");
    assert!(folders.iter().any(|f| f["name"] == "Photos"));
}

#[tokio::test]
async fn create_folder_requires_auth() {
    let app = TestApp::new().await;
    let res = app
        .server
        .post("/api/folders")
        .json(&serde_json::json!({ "name": "Test" }))
        .await;
    res.assert_status(StatusCode::UNAUTHORIZED);
}

// ── Nesting & breadcrumb ──────────────────────────────────────────────────────

#[tokio::test]
async fn nested_folders_and_breadcrumb() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    // Create parent
    let parent: serde_json::Value = app
        .server
        .post("/api/folders")
        .json(&serde_json::json!({ "name": "Work" }))
        .await
        .json();
    let parent_id = parent["id"].as_str().expect("parent id");

    // Create child inside parent
    let child: serde_json::Value = app
        .server
        .post("/api/folders")
        .json(&serde_json::json!({ "name": "Projects", "parent_id": parent_id }))
        .await
        .json();
    let child_id = child["id"].as_str().expect("child id");
    assert_eq!(child["parent_id"], parent_id);

    // Breadcrumb for child should include Work → Projects
    let crumb: serde_json::Value = app
        .server
        .get(&format!("/api/folders/{}/breadcrumb", child_id))
        .await
        .json();
    let items = crumb.as_array().expect("breadcrumb array");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["name"], "Work");
    assert_eq!(items[1]["name"], "Projects");
}

// ── Delete ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn delete_folder() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let folder: serde_json::Value = app
        .server
        .post("/api/folders")
        .json(&serde_json::json!({ "name": "Temp" }))
        .await
        .json();
    let folder_id = folder["id"].as_str().expect("folder id");

    app.server
        .delete(&format!("/api/folders/{}", folder_id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let listing: serde_json::Value = app.server.get("/api/folders").await.json();
    let folders = listing.as_array().expect("folders");
    assert!(!folders.iter().any(|f| f["id"] == folder_id));
}

#[tokio::test]
async fn delete_folder_removes_nested_files() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    // Create folder
    let folder: serde_json::Value = app
        .server
        .post("/api/folders")
        .json(&serde_json::json!({ "name": "ToDelete" }))
        .await
        .json();
    let folder_id = folder["id"].as_str().expect("folder id").to_string();

    // Upload a file into it
    use axum_test::multipart::{MultipartForm, Part};
    let form = MultipartForm::new()
        .add_part("file", Part::bytes(b"content".to_vec()).file_name("f.txt").mime_type("text/plain"))
        .add_part("parent_id", Part::text(folder_id.clone()));
    let file: serde_json::Value = app
        .server
        .post("/api/uploads/simple")
        .multipart(form)
        .await
        .json();
    let file_id = file["id"].as_str().expect("file id");

    // Delete the folder
    app.server
        .delete(&format!("/api/folders/{}", folder_id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // The file inside should also be gone
    app.server
        .get(&format!("/api/files/{}", file_id))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}
