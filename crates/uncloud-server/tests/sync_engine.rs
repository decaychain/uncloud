mod common;

use std::sync::Arc;
use std::time::Duration;

use common::BoundTestApp;
use uncloud_common::{SyncStrategy, UpdateFolderRequest};

// ── Upload direction ───────────────────────────────────────────────────────────

/// A file created locally is uploaded to the server on the first sync.
#[tokio::test]
async fn upload_local_file_appears_on_server() {
    let app = BoundTestApp::new().await;
    let client = app.setup_user("alice").await;
    let (engine, sync_dir) = app.new_sync_engine(Arc::clone(&client)).await;

    tokio::fs::write(sync_dir.path().join("hello.txt"), b"hello world")
        .await
        .unwrap();

    let report = engine.incremental_sync().await.unwrap();

    assert_eq!(report.uploaded, vec!["hello.txt"]);
    assert!(report.errors.is_empty());

    let files = client.list_files(None).await.unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].name, "hello.txt");
    assert_eq!(files[0].size_bytes, 11);
}

/// A locally-modified file (already known to the journal) is re-uploaded.
#[tokio::test]
async fn modify_local_file_updates_server() {
    let app = BoundTestApp::new().await;
    let client = app.setup_user("bob").await;
    let (engine, sync_dir) = app.new_sync_engine(Arc::clone(&client)).await;

    tokio::fs::write(sync_dir.path().join("notes.txt"), b"v1")
        .await
        .unwrap();
    engine.incremental_sync().await.unwrap();

    // Sleep to ensure the filesystem mtime advances past what the journal recorded.
    tokio::time::sleep(Duration::from_secs(1)).await;
    tokio::fs::write(sync_dir.path().join("notes.txt"), b"version two")
        .await
        .unwrap();

    let report = engine.incremental_sync().await.unwrap();

    assert_eq!(report.uploaded, vec!["notes.txt"]);
    assert!(report.errors.is_empty());

    let files = client.list_files(None).await.unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].size_bytes, 11); // "version two"
}

// ── Download direction ────────────────────────────────────────────────────────

/// A file that exists on the server is downloaded on first sync.
#[tokio::test]
async fn server_file_downloads_to_local() {
    let app = BoundTestApp::new().await;
    let client = app.setup_user("carol").await;

    // Pre-upload a file directly via the API (no sync engine involved).
    let tmp = tempfile::TempDir::new().unwrap();
    tokio::fs::write(tmp.path().join("from_server.txt"), b"server content")
        .await
        .unwrap();
    client
        .upload_file(&tmp.path().join("from_server.txt"), None)
        .await
        .unwrap();

    // Fresh engine — its journal is empty.
    let (engine, sync_dir) = app.new_sync_engine(Arc::clone(&client)).await;

    let report = engine.incremental_sync().await.unwrap();

    assert_eq!(report.downloaded, vec!["from_server.txt"]);
    assert!(report.errors.is_empty());

    let content = tokio::fs::read(sync_dir.path().join("from_server.txt"))
        .await
        .unwrap();
    assert_eq!(content, b"server content");
}

/// A file inside an Inherit-strategy folder (no explicit override anywhere)
/// should be downloaded when the root default resolves to TwoWay.
#[tokio::test]
async fn server_file_in_inherit_folder_downloads_to_local() {
    let app = BoundTestApp::new().await;
    let client = app.setup_user("carol_nested").await;

    // Server-side folder with no strategy set (Inherit by default).
    let folder = client.create_folder("photos", None).await.unwrap();

    let tmp = tempfile::TempDir::new().unwrap();
    tokio::fs::write(tmp.path().join("cat.jpg"), b"meow")
        .await
        .unwrap();
    client
        .upload_file(&tmp.path().join("cat.jpg"), Some(&folder.id))
        .await
        .unwrap();

    let (engine, sync_dir) = app.new_sync_engine(Arc::clone(&client)).await;
    let report = engine.incremental_sync().await.unwrap();

    assert!(
        report.errors.is_empty(),
        "expected no errors, got: {:?}",
        report.errors
    );
    assert_eq!(report.downloaded, vec!["cat.jpg"]);

    let local = sync_dir.path().join("photos").join("cat.jpg");
    assert!(local.exists(), "expected {} to exist", local.display());
    assert_eq!(tokio::fs::read(&local).await.unwrap(), b"meow");
}

/// A file inside a nested subfolder (two levels deep) with all folders
/// using default Inherit strategy should be downloaded, and its directory
/// structure created on disk under the client root.
#[tokio::test]
async fn server_file_in_nested_inherit_folders_downloads() {
    let app = BoundTestApp::new().await;
    let client = app.setup_user("carol_deep").await;

    // photos/vacation/beach.jpg — no strategies set anywhere.
    let photos = client.create_folder("photos", None).await.unwrap();
    let vacation = client
        .create_folder("vacation", Some(&photos.id))
        .await
        .unwrap();

    let tmp = tempfile::TempDir::new().unwrap();
    tokio::fs::write(tmp.path().join("beach.jpg"), b"sandy")
        .await
        .unwrap();
    client
        .upload_file(&tmp.path().join("beach.jpg"), Some(&vacation.id))
        .await
        .unwrap();

    let (engine, sync_dir) = app.new_sync_engine(Arc::clone(&client)).await;
    let report = engine.incremental_sync().await.unwrap();

    assert!(
        report.errors.is_empty(),
        "expected no errors, got: {:?}",
        report.errors
    );
    assert_eq!(report.downloaded, vec!["beach.jpg"]);

    let local = sync_dir
        .path()
        .join("photos")
        .join("vacation")
        .join("beach.jpg");
    assert!(local.exists(), "expected {} to exist", local.display());
}

/// After a server-side deletion the local copy is removed on next sync.
#[tokio::test]
async fn server_delete_removes_local_file() {
    let app = BoundTestApp::new().await;
    let client = app.setup_user("dave").await;
    let (engine, sync_dir) = app.new_sync_engine(Arc::clone(&client)).await;

    tokio::fs::write(sync_dir.path().join("temp.txt"), b"temporary")
        .await
        .unwrap();
    engine.incremental_sync().await.unwrap();

    // Delete the file on the server.
    let files = client.list_files(None).await.unwrap();
    client.delete_file(&files[0].id).await.unwrap();

    let report = engine.incremental_sync().await.unwrap();

    assert_eq!(report.deleted_local, vec!["temp.txt"]);
    assert!(!sync_dir.path().join("temp.txt").exists());
}

// ── Conflict resolution ───────────────────────────────────────────────────────

/// When both the local copy and the server copy diverge, the engine keeps
/// the server version and writes a conflict copy of the local changes.
#[tokio::test]
async fn conflict_creates_copy() {
    let app = BoundTestApp::new().await;
    let client = app.setup_user("eve").await;
    let (engine, sync_dir) = app.new_sync_engine(Arc::clone(&client)).await;

    // Initial state: create and sync.
    tokio::fs::write(sync_dir.path().join("shared.txt"), b"v1")
        .await
        .unwrap();
    engine.incremental_sync().await.unwrap();

    // Sleep so the local mtime will be strictly newer than what the journal recorded.
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Local change.
    tokio::fs::write(sync_dir.path().join("shared.txt"), b"local edit")
        .await
        .unwrap();

    // Server change: update the file content via the API.
    let files = client.list_files(None).await.unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    tokio::fs::write(tmp.path().join("shared.txt"), b"server edit")
        .await
        .unwrap();
    client
        .update_file_content(&files[0].id, &tmp.path().join("shared.txt"))
        .await
        .unwrap();

    let report = engine.incremental_sync().await.unwrap();

    assert_eq!(report.conflicts.len(), 1, "expected exactly one conflict");
    assert!(report.errors.is_empty());

    // The canonical path should now contain the server version.
    let canonical = tokio::fs::read(sync_dir.path().join("shared.txt"))
        .await
        .unwrap();
    assert_eq!(canonical, b"server edit");

    // The conflict copy should contain our local changes.
    let conflict_path = std::path::Path::new(&report.conflicts[0].conflict_copy);
    assert!(conflict_path.exists(), "conflict copy file must exist");
    let conflict_content = tokio::fs::read(conflict_path).await.unwrap();
    assert_eq!(conflict_content, b"local edit");
}

// ── Strategy enforcement ──────────────────────────────────────────────────────

/// Files inside a DoNotSync folder are not downloaded even when the server has them.
#[tokio::test]
async fn do_not_sync_folder_skips_download() {
    let app = BoundTestApp::new().await;
    let client = app.setup_user("frank").await;

    // Create a server-side folder and mark it DoNotSync.
    let folder = client.create_folder("private", None).await.unwrap();
    client
        .update_folder(
            &folder.id,
            &UpdateFolderRequest {
                name: None,
                parent_id: None,
                sync_strategy: Some(SyncStrategy::DoNotSync),
                gallery_include: None,
                music_include: None,
            },
        )
        .await
        .unwrap();

    // Upload a file into that folder.
    let tmp = tempfile::TempDir::new().unwrap();
    tokio::fs::write(tmp.path().join("secret.txt"), b"top secret")
        .await
        .unwrap();
    client
        .upload_file(&tmp.path().join("secret.txt"), Some(&folder.id))
        .await
        .unwrap();

    // Sync with a fresh engine — nothing should come down.
    let (engine, sync_dir) = app.new_sync_engine(Arc::clone(&client)).await;
    let report = engine.incremental_sync().await.unwrap();

    assert!(
        report.downloaded.is_empty(),
        "DoNotSync folder must not produce downloads"
    );
    assert!(report.errors.is_empty());
    assert!(!sync_dir.path().join("secret.txt").exists());
}

/// A locally-modified file in a ServerToClient folder is NOT uploaded;
/// the server version remains authoritative.
#[tokio::test]
async fn server_to_client_folder_blocks_local_upload() {
    let app = BoundTestApp::new().await;
    let client = app.setup_user("grace").await;

    // Create a ServerToClient folder and upload a file into it.
    let folder = client.create_folder("readonly", None).await.unwrap();
    client
        .update_folder(
            &folder.id,
            &UpdateFolderRequest {
                name: None,
                parent_id: None,
                sync_strategy: Some(SyncStrategy::ServerToClient),
                gallery_include: None,
                music_include: None,
            },
        )
        .await
        .unwrap();

    let tmp = tempfile::TempDir::new().unwrap();
    tokio::fs::write(tmp.path().join("doc.txt"), b"original").await.unwrap();
    client
        .upload_file(&tmp.path().join("doc.txt"), Some(&folder.id))
        .await
        .unwrap();

    // First sync — file should be downloaded into the folder's subdir.
    let (engine, sync_dir) = app.new_sync_engine(Arc::clone(&client)).await;
    let report = engine.incremental_sync().await.unwrap();
    assert_eq!(report.downloaded, vec!["doc.txt"]);

    // Modify local copy — now lives inside `readonly/` because the engine
    // mirrors the server folder structure on disk.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let local_file = sync_dir.path().join("readonly").join("doc.txt");
    tokio::fs::write(&local_file, b"local change").await.unwrap();

    // Second sync — local change must NOT be uploaded.
    let report2 = engine.incremental_sync().await.unwrap();
    assert!(
        report2.uploaded.is_empty(),
        "ServerToClient folder must not allow uploads"
    );
    assert!(report2.errors.is_empty());

    // Server file should still be 8 bytes ("original").
    let files = client.list_files(Some(&folder.id)).await.unwrap();
    assert_eq!(files[0].size_bytes, 8);
}

// ── Idempotency ───────────────────────────────────────────────────────────────

/// Running sync a second time when nothing has changed produces an empty report.
#[tokio::test]
async fn idempotent_sync() {
    let app = BoundTestApp::new().await;
    let client = app.setup_user("hank").await;
    let (engine, sync_dir) = app.new_sync_engine(Arc::clone(&client)).await;

    tokio::fs::write(sync_dir.path().join("stable.txt"), b"unchanged")
        .await
        .unwrap();
    engine.incremental_sync().await.unwrap();

    // Second sync: nothing should happen.
    let report = engine.incremental_sync().await.unwrap();

    assert!(report.uploaded.is_empty());
    assert!(report.downloaded.is_empty());
    assert!(report.deleted_local.is_empty());
    assert!(report.conflicts.is_empty());
    assert!(report.errors.is_empty());
}

// ── Multi-client sharing ──────────────────────────────────────────────────────

/// Files uploaded by one client are downloaded by a second client syncing as
/// the same user.
#[tokio::test]
async fn two_clients_share_files() {
    let app = BoundTestApp::new().await;

    // Client A: upload several files through the sync engine.
    let client_a = app.setup_user("irene").await;
    let (engine_a, sync_dir_a) = app.new_sync_engine(Arc::clone(&client_a)).await;

    for i in 0..5u8 {
        tokio::fs::write(
            sync_dir_a.path().join(format!("file{}.txt", i)),
            format!("content {}", i).as_bytes(),
        )
        .await
        .unwrap();
    }
    let report_a = engine_a.incremental_sync().await.unwrap();
    assert_eq!(report_a.uploaded.len(), 5);

    // Client B: fresh login as the same user, fresh local directory.
    let client_b = app.login_client("irene", "password123!").await;
    let (engine_b, sync_dir_b) = app.new_sync_engine(Arc::clone(&client_b)).await;

    let report_b = engine_b.incremental_sync().await.unwrap();
    assert_eq!(report_b.downloaded.len(), 5);
    assert!(report_b.errors.is_empty());

    // Every file should exist locally with the correct content.
    for i in 0..5u8 {
        let content = tokio::fs::read(sync_dir_b.path().join(format!("file{}.txt", i)))
            .await
            .unwrap();
        assert_eq!(content, format!("content {}", i).as_bytes());
    }
}
