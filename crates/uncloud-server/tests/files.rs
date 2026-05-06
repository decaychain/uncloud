mod common;

use axum::http::StatusCode;
use common::TestApp;
use mongodb::bson::doc;

// ── Upload & listing ──────────────────────────────────────────────────────────

#[tokio::test]
async fn upload_creates_file() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let file = app.upload("hello.txt", b"hello world", "text/plain").await;
    let file_id = file["id"].as_str().expect("file id");
    assert_eq!(file["name"], "hello.txt");
    assert_eq!(file["size_bytes"], 11);

    // File appears in listing
    let listing: serde_json::Value = app.server.get("/api/files").await.json();
    let files = listing.as_array().expect("files array");
    assert!(files.iter().any(|f| f["id"] == file_id));
}

#[tokio::test]
async fn upload_requires_auth() {
    let app = TestApp::new().await;
    use axum_test::multipart::{MultipartForm, Part};
    let form = MultipartForm::new()
        .add_part("file", Part::bytes(b"data".to_vec()).file_name("f.txt").mime_type("text/plain"));
    let res = app.server.post("/api/uploads/simple").multipart(form).await;
    res.assert_status(StatusCode::UNAUTHORIZED);
}

// ── Download ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn download_returns_content() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let content = b"the quick brown fox";
    let file = app.upload("fox.txt", content, "text/plain").await;
    let file_id = file["id"].as_str().expect("file id");

    let res = app
        .server
        .get(&format!("/api/files/{}/download", file_id))
        .await;
    res.assert_status_ok();
    assert_eq!(res.as_bytes().as_ref(), content.as_ref());
}

// ── Rename ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn rename_file() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let file = app.upload("old.txt", b"content", "text/plain").await;
    let file_id = file["id"].as_str().expect("file id");

    let res = app
        .server
        .put(&format!("/api/files/{}", file_id))
        .json(&serde_json::json!({ "name": "new.txt" }))
        .await;
    res.assert_status_ok();
    assert_eq!(res.json::<serde_json::Value>()["name"], "new.txt");

    // Old name gone, new name present in listing
    let listing: serde_json::Value = app.server.get("/api/files").await.json();
    let files = listing.as_array().expect("files");
    assert!(files.iter().any(|f| f["name"] == "new.txt"));
    assert!(!files.iter().any(|f| f["name"] == "old.txt"));
}

// ── Delete ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn delete_file() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let file = app.upload("delete_me.txt", b"bye", "text/plain").await;
    let file_id = file["id"].as_str().expect("file id");

    app.server
        .delete(&format!("/api/files/{}", file_id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // No longer in listing
    let listing: serde_json::Value = app.server.get("/api/files").await.json();
    let files = listing.as_array().expect("files");
    assert!(!files.iter().any(|f| f["id"] == file_id));
}

#[tokio::test]
async fn delete_requires_auth() {
    let app = TestApp::new().await;
    // Use a fake ID — still returns 401 before any DB lookup
    let res = app
        .server
        .delete("/api/files/000000000000000000000001")
        .await;
    res.assert_status(StatusCode::UNAUTHORIZED);
}

// ── User isolation ────────────────────────────────────────────────────────────

#[tokio::test]
async fn user_isolation() {
    let app_alice = TestApp::new().await;
    app_alice.register_and_login("alice").await;
    let file = app_alice.upload("secret.txt", b"private", "text/plain").await;
    let file_id = file["id"].as_str().expect("file id").to_string();

    // Bob registers and logs in on a fresh TestApp pointed at the same DB
    // (same container, different TestApp instance so different cookie jar)
    let app_bob = TestApp::new().await;
    app_bob.register_and_login("bob").await;

    // Bob cannot read Alice's file
    app_bob
        .server
        .get(&format!("/api/files/{}", file_id))
        .await
        .assert_status(StatusCode::NOT_FOUND);

    // Bob cannot delete Alice's file
    app_bob
        .server
        .delete(&format!("/api/files/{}", file_id))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

// ── Move with conflict resolution ────────────────────────────────────────────

#[tokio::test]
async fn move_file_conflict_retry_with_new_name() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    // 1. Upload "photo.jpg" to root
    let file_a = app.upload("photo.jpg", b"root photo", "image/jpeg").await;
    let file_a_id = file_a["id"].as_str().expect("file A id");

    // 2. Create a subfolder "docs"
    let folder: serde_json::Value = app
        .server
        .post("/api/folders")
        .json(&serde_json::json!({ "name": "docs" }))
        .await
        .json();
    let folder_id = folder["id"].as_str().expect("folder id");

    // 3. Upload another "photo.jpg" into "docs" to create a conflict target
    app.upload_to_folder("photo.jpg", b"docs photo", "image/jpeg", folder_id)
        .await;

    // 4. Move the root file to "docs" — should get 409 conflict
    let conflict_res = app
        .server
        .put(&format!("/api/files/{}", file_a_id))
        .json(&serde_json::json!({ "name": "photo.jpg", "parent_id": folder_id }))
        .await;
    conflict_res.assert_status(StatusCode::CONFLICT);

    // 5. Retry the move with a new name — should succeed
    let retry_res = app
        .server
        .put(&format!("/api/files/{}", file_a_id))
        .json(&serde_json::json!({ "name": "photo (1).jpg", "parent_id": folder_id }))
        .await;
    retry_res.assert_status_ok();
    let updated: serde_json::Value = retry_res.json();
    assert_eq!(updated["name"], "photo (1).jpg");
    assert_eq!(updated["parent_id"], folder_id);

    // 6. Verify the file no longer exists at root
    let root_files: serde_json::Value = app.server.get("/api/files").await.json();
    let root_list = root_files.as_array().expect("root files");
    assert!(
        !root_list.iter().any(|f| f["id"] == file_a_id),
        "moved file should not appear at root"
    );

    // 7. Verify the file exists in "docs" with the new name
    let docs_files: serde_json::Value = app
        .server
        .get(&format!("/api/files?parent_id={}", folder_id))
        .await
        .json();
    let docs_list = docs_files.as_array().expect("docs files");
    assert!(
        docs_list.iter().any(|f| f["id"] == file_a_id && f["name"] == "photo (1).jpg"),
        "moved file should appear in docs with new name"
    );

    // 8. Verify the file content is still accessible
    let download_res = app
        .server
        .get(&format!("/api/files/{}/download", file_a_id))
        .await;
    download_res.assert_status_ok();
    assert_eq!(download_res.as_bytes().as_ref(), b"root photo");
}

#[tokio::test]
async fn move_folder_conflict_retry_with_new_name() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    // 1. Create folder "photos" at root with a file inside
    let folder_a: serde_json::Value = app
        .server
        .post("/api/folders")
        .json(&serde_json::json!({ "name": "photos" }))
        .await
        .json();
    let folder_a_id = folder_a["id"].as_str().expect("folder A id");

    let file_in_a = app
        .upload_to_folder("cat.jpg", b"meow", "image/jpeg", folder_a_id)
        .await;
    let file_in_a_id = file_in_a["id"].as_str().expect("file id");

    // 2. Create destination folder "archive"
    let dest: serde_json::Value = app
        .server
        .post("/api/folders")
        .json(&serde_json::json!({ "name": "archive" }))
        .await
        .json();
    let dest_id = dest["id"].as_str().expect("dest id");

    // 3. Create another "photos" inside "archive" to cause conflict
    app.server
        .post("/api/folders")
        .json(&serde_json::json!({ "name": "photos", "parent_id": dest_id }))
        .await
        .assert_status(StatusCode::CREATED);

    // 4. Move folder "photos" to "archive" — should get 409
    let conflict_res = app
        .server
        .put(&format!("/api/folders/{}", folder_a_id))
        .json(&serde_json::json!({ "name": "photos", "parent_id": dest_id }))
        .await;
    conflict_res.assert_status(StatusCode::CONFLICT);

    // 5. Retry with a new name — should succeed
    let retry_res = app
        .server
        .put(&format!("/api/folders/{}", folder_a_id))
        .json(&serde_json::json!({ "name": "photos (1)", "parent_id": dest_id }))
        .await;
    retry_res.assert_status_ok();
    let updated: serde_json::Value = retry_res.json();
    assert_eq!(updated["name"], "photos (1)");
    assert_eq!(updated["parent_id"], dest_id);

    // 6. Verify the file inside the moved folder is still accessible
    let download_res = app
        .server
        .get(&format!("/api/files/{}/download", file_in_a_id))
        .await;
    download_res.assert_status_ok();
    assert_eq!(download_res.as_bytes().as_ref(), b"meow");
}

// ── Metadata on FileResponse ─────────────────────────────────────────────────

#[tokio::test]
async fn file_listing_metadata_empty_by_default() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let file = app.upload("notes.txt", b"some notes", "text/plain").await;
    let file_id = file["id"].as_str().expect("file id");

    // Fetch listing and find our file
    let listing: serde_json::Value = app.server.get("/api/files").await.json();
    let files = listing.as_array().expect("files array");
    let found = files
        .iter()
        .find(|f| f["id"] == file_id)
        .expect("uploaded file should appear in listing");

    // metadata should either be absent (null) or an empty object
    let meta = &found["metadata"];
    assert!(
        meta.is_null() || (meta.is_object() && meta.as_object().unwrap().is_empty()),
        "newly uploaded file should have no metadata, got: {}",
        meta
    );
}

#[tokio::test]
async fn file_response_metadata_populated_after_direct_db_update() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let file = app.upload("song.mp3", b"fake audio", "audio/mpeg").await;
    let file_id_str = file["id"].as_str().expect("file id");
    let file_oid =
        mongodb::bson::oid::ObjectId::parse_str(file_id_str).expect("valid ObjectId");

    // Directly set metadata.audio on the MongoDB document
    let collection = app.db.collection::<mongodb::bson::Document>("files");
    collection
        .update_one(
            doc! { "_id": file_oid },
            doc! { "$set": {
                "metadata.audio": {
                    "title": "Test Song",
                    "artist": "Test Artist",
                    "duration_secs": 180.0
                }
            }},
        )
        .await
        .expect("direct DB update");

    // Fetch listing and find our file
    let listing: serde_json::Value = app.server.get("/api/files").await.json();
    let files = listing.as_array().expect("files array");
    let found = files
        .iter()
        .find(|f| f["id"] == file_id_str)
        .expect("file should appear in listing");

    // Verify the metadata was converted from BSON to JSON correctly
    let meta = &found["metadata"];
    assert!(meta.is_object(), "metadata should be an object");
    let audio = &meta["audio"];
    assert_eq!(audio["title"], "Test Song");
    assert_eq!(audio["artist"], "Test Artist");
    assert_eq!(audio["duration_secs"], 180.0);
}

#[tokio::test]
async fn file_response_metadata_omitted_in_json_when_empty() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    app.upload("plain.txt", b"hello", "text/plain").await;

    // Get the raw response text so we can check the literal JSON
    let res = app.server.get("/api/files").await;
    res.assert_status_ok();
    let body = res.text();

    // The string "metadata" should not appear anywhere in the response
    // because skip_serializing_if = "HashMap::is_empty" omits it entirely
    assert!(
        !body.contains("\"metadata\""),
        "empty metadata should be omitted from JSON, but body contained: {}",
        body
    );
}

#[tokio::test]
async fn copy_file_does_not_preserve_metadata() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let file = app.upload("track.mp3", b"audio data", "audio/mpeg").await;
    let file_id_str = file["id"].as_str().expect("file id");
    let file_oid =
        mongodb::bson::oid::ObjectId::parse_str(file_id_str).expect("valid ObjectId");

    // Set metadata.audio on the original file via direct DB update
    let collection = app.db.collection::<mongodb::bson::Document>("files");
    collection
        .update_one(
            doc! { "_id": file_oid },
            doc! { "$set": {
                "metadata.audio": {
                    "title": "Original",
                    "artist": "Artist",
                    "duration_secs": 240.0
                }
            }},
        )
        .await
        .expect("direct DB update");

    // Copy the file
    let copy_res = app
        .server
        .post(&format!("/api/files/{}/copy", file_id_str))
        .json(&serde_json::json!({ "name": "track_copy.mp3" }))
        .await;
    copy_res.assert_status_ok();
    let copy: serde_json::Value = copy_res.json();

    // The copy's metadata should be empty (absent or empty object)
    let meta = &copy["metadata"];
    assert!(
        meta.is_null() || (meta.is_object() && meta.as_object().unwrap().is_empty()),
        "copied file should have empty metadata (processor must re-generate it), got: {}",
        meta
    );
}

// ── Name-uniqueness enforcement ───────────────────────────────────────────────
//
// `(owner_id, parent_id, name)` is the logical identity for a live file —
// the on-disk layout `{username}/{chain}/{name}` would be ambiguous
// otherwise. Two layers enforce it:
//   1. Handler-level pre-flight check in `simple_upload` /
//      `complete_upload` / `copy_file` returns 409 cleanly.
//   2. The MongoDB partial unique index on `(owner_id, parent_id, name)`
//      filtered to `deleted_at: null`, set up in `db::setup_indexes`.
// These tests cover both layers, plus the partial-filter behaviour: a
// trashed file must not block re-using its name.

#[tokio::test]
async fn simple_upload_rejects_duplicate_name() {
    use axum_test::multipart::{MultipartForm, Part};

    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    // First upload: succeeds normally.
    app.upload("doc.pdf", b"first", "application/pdf").await;

    // Second upload at the same logical path: 409 Conflict.
    let form = MultipartForm::new().add_part(
        "file",
        Part::bytes(b"second".to_vec())
            .file_name("doc.pdf")
            .mime_type("application/pdf"),
    );
    let res = app
        .server
        .post("/api/uploads/simple")
        .multipart(form)
        .await;
    res.assert_status(StatusCode::CONFLICT);
}

#[tokio::test]
async fn complete_upload_rejects_duplicate_name() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    // Pre-existing live file at the target path.
    app.upload("big.bin", b"already", "application/octet-stream")
        .await;

    // Init a chunked upload with the same name. `init_upload` itself
    // doesn't check (it just allocates an upload session), so we get
    // through this step and the chunk upload before hitting the gate.
    let init: serde_json::Value = app
        .server
        .post("/api/uploads/init")
        .json(&serde_json::json!({ "filename": "big.bin", "size": 4 }))
        .await
        .json();
    let upload_id = init["upload_id"].as_str().expect("upload_id");

    // One chunk covers the whole 4-byte payload.
    app.server
        .post(&format!("/api/uploads/{}/chunk?index=0", upload_id))
        .bytes(b"data"[..].into())
        .await
        .assert_status_ok();

    // Complete: must hit the duplicate-name guard and return 409, not
    // silently insert a second File document.
    let res = app
        .server
        .post(&format!("/api/uploads/{}/complete", upload_id))
        .await;
    res.assert_status(StatusCode::CONFLICT);
}

#[tokio::test]
async fn upload_after_trash_succeeds_partial_filter() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    // Upload a file, then send it to trash (soft-delete, sets `deleted_at`).
    let first = app.upload("notes.md", b"v1", "text/markdown").await;
    let first_id = first["id"].as_str().expect("file id");
    app.server
        .delete(&format!("/api/files/{}", first_id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Re-uploading the same name must now succeed — the partial unique
    // index excludes `deleted_at != null` rows, and `check_name_conflict`
    // mirrors the same filter. If either side gets it wrong, this test
    // fails with 409.
    let second = app.upload("notes.md", b"v2", "text/markdown").await;
    assert_eq!(second["name"], "notes.md");
    assert_ne!(
        first_id,
        second["id"].as_str().expect("second id"),
        "trashed and live files must be distinct documents"
    );
}
