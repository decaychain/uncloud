//! Regression coverage for "local-only folders never sync to the server".
//!
//! Before the fix, `run_sync_inner` only created **local** dirs from the
//! **server** folder list. Files inside a brand-new client-side folder
//! were silently skipped because Phase 7's longest-prefix match couldn't
//! find a known parent. This test stands up a fake server with an empty
//! tree, creates a nested folder structure locally, and asserts that
//! after one sync:
//!
//!   * each new local directory was POSTed to `/api/folders`,
//!   * the file inside the deepest directory was uploaded, and
//!   * the engine reports both via `SyncReport::created_remote_folders`
//!     and `SyncReport::uploaded`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::{
    extract::{Json as AxJson, Path as AxPath, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::{oneshot, Mutex};
use uncloud_client::Client;
use uncloud_sync::SyncEngine;

#[derive(Clone, Debug)]
struct CreatedFolder {
    id: String,
    name: String,
    parent_id: Option<String>,
}

#[derive(Clone, Debug)]
struct UploadedFile {
    id: String,
    name: String,
    parent_id: Option<String>,
}

#[derive(Clone, Default)]
struct FakeServer {
    folders: Arc<Mutex<HashMap<String, CreatedFolder>>>,
    files: Arc<Mutex<HashMap<String, UploadedFile>>>,
    create_folder_calls: Arc<AtomicUsize>,
    delete_folder_calls: Arc<AtomicUsize>,
    update_folder_calls: Arc<AtomicUsize>,
    upload_calls: Arc<AtomicUsize>,
    /// Content checksums the tree reports, by file id. Empty by default,
    /// like a server that predates the field.
    checksums: Arc<Mutex<HashMap<String, String>>>,
}

#[derive(serde::Deserialize)]
struct UpdateFolderBody {
    name: Option<String>,
    parent_id: Option<String>,
}

#[derive(serde::Deserialize)]
struct CreateFolderBody {
    name: String,
    parent_id: Option<String>,
}

async fn sync_tree(State(s): State<FakeServer>) -> impl IntoResponse {
    let folders = s.folders.lock().await;
    let files = s.files.lock().await;
    let checksums = s.checksums.lock().await;
    let folders: Vec<_> = folders
        .values()
        .map(|f| {
            serde_json::json!({
                "id": f.id,
                "name": f.name,
                "parent_id": f.parent_id,
                "created_at": "2026-01-01T00:00:00Z",
                "updated_at": "2026-01-01T00:00:00Z",
                "sync_strategy": "inherit",
                "effective_strategy": "two_way",
                "gallery_include": "inherit",
                "effective_gallery_include": "exclude",
                "music_include": "inherit",
                "effective_music_include": "exclude",
                "shared_with_count": 0,
            })
        })
        .collect();
    let files: Vec<_> = files
        .values()
        .map(|f| {
            serde_json::json!({
                "id": f.id,
                "name": f.name,
                "mime_type": "application/octet-stream",
                "size_bytes": 0,
                "parent_id": f.parent_id,
                "created_at": "2026-01-01T00:00:00Z",
                "updated_at": "2026-01-01T00:00:00Z",
                "checksum_sha256": checksums.get(&f.id),
            })
        })
        .collect();
    Json(serde_json::json!({ "files": files, "folders": folders }))
}

async fn create_folder(
    State(s): State<FakeServer>,
    AxJson(body): AxJson<CreateFolderBody>,
) -> impl IntoResponse {
    let n = s.create_folder_calls.fetch_add(1, Ordering::SeqCst) + 1;
    let id = format!("srv-folder-{n}");
    s.folders.lock().await.insert(
        id.clone(),
        CreatedFolder {
            id: id.clone(),
            name: body.name.clone(),
            parent_id: body.parent_id.clone(),
        },
    );
    let resp = serde_json::json!({
        "id": id,
        "name": body.name,
        "parent_id": body.parent_id,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z",
        "sync_strategy": "inherit",
        "effective_strategy": "two_way",
        "gallery_include": "inherit",
        "effective_gallery_include": "exclude",
        "music_include": "inherit",
        "effective_music_include": "exclude",
        "shared_with_count": 0,
    });
    (StatusCode::OK, Json(resp))
}

async fn delete_folder(
    AxPath(id): AxPath<String>,
    State(s): State<FakeServer>,
) -> impl IntoResponse {
    s.delete_folder_calls.fetch_add(1, Ordering::SeqCst);
    s.folders.lock().await.remove(&id);
    s.files
        .lock()
        .await
        .retain(|_, file| file.parent_id.as_deref() != Some(id.as_str()));
    StatusCode::NO_CONTENT
}

async fn update_folder(
    AxPath(id): AxPath<String>,
    State(s): State<FakeServer>,
    AxJson(body): AxJson<UpdateFolderBody>,
) -> impl IntoResponse {
    s.update_folder_calls.fetch_add(1, Ordering::SeqCst);
    let mut folders = s.folders.lock().await;
    let Some(folder) = folders.get_mut(&id) else {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({}))).into_response();
    };
    if let Some(name) = body.name {
        folder.name = name;
    }
    if let Some(parent_id) = body.parent_id {
        folder.parent_id = (!parent_id.is_empty()).then_some(parent_id);
    }
    Json(serde_json::json!({
        "id": folder.id,
        "name": folder.name,
        "parent_id": folder.parent_id,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-02T00:00:00Z",
        "sync_strategy": "inherit",
        "effective_strategy": "two_way",
        "gallery_include": "inherit",
        "effective_gallery_include": "exclude",
        "music_include": "inherit",
        "effective_music_include": "exclude",
        "shared_with_count": 0,
    }))
    .into_response()
}

async fn upload_simple(
    State(s): State<FakeServer>,
    mut multipart: axum::extract::Multipart,
) -> impl IntoResponse {
    let mut name = String::new();
    let mut parent_id: Option<String> = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name() {
            Some("file") => {
                if let Some(fname) = field.file_name() {
                    name = fname.to_owned();
                }
                let _ = field.bytes().await;
            }
            Some("parent_id") => {
                parent_id = field.text().await.ok().filter(|s| !s.is_empty());
            }
            _ => {}
        }
    }
    let n = s.upload_calls.fetch_add(1, Ordering::SeqCst) + 1;
    let id = format!("srv-file-{n}");
    s.files.lock().await.insert(
        id.clone(),
        UploadedFile {
            id: id.clone(),
            name: name.clone(),
            parent_id: parent_id.clone(),
        },
    );
    let resp = serde_json::json!({
        "id": id,
        "name": name,
        "mime_type": "application/octet-stream",
        "size_bytes": 0,
        "parent_id": parent_id,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z",
    });
    (StatusCode::OK, Json(resp))
}

async fn spawn_server(state: FakeServer) -> (String, oneshot::Sender<()>) {
    let app = Router::new()
        .route("/api/sync/tree", get(sync_tree))
        .route("/api/folders", post(create_folder))
        .route(
            "/api/folders/{id}",
            delete(delete_folder).put(update_folder),
        )
        .route("/api/uploads/simple", post(upload_simple))
        .with_state(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await;
    });

    (format!("http://{}", addr), shutdown_tx)
}

struct TestRig {
    _temp: TempDir,
    db_path: PathBuf,
    root: PathBuf,
    server_url: String,
    _shutdown: oneshot::Sender<()>,
}

async fn new_rig(server: FakeServer) -> TestRig {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("sync.db");
    let root = temp.path().join("data");
    std::fs::create_dir_all(&root).unwrap();
    let (server_url, shutdown) = spawn_server(server).await;
    TestRig {
        _temp: temp,
        db_path,
        root,
        server_url,
        _shutdown: shutdown,
    }
}

async fn build_engine(rig: &TestRig) -> Arc<SyncEngine> {
    let client = Arc::new(Client::new(&rig.server_url));
    let engine = SyncEngine::new(
        &rig.db_path,
        client,
        Some(rig.root.to_string_lossy().into_owned()),
    )
    .await
    .unwrap()
    .with_delete_confirm_delay(std::time::Duration::ZERO);
    Arc::new(engine)
}

#[tokio::test]
async fn matching_timestamps_with_different_content_are_not_a_rename() {
    for contents in [b"different".as_slice(), b"different and longer".as_slice()] {
        let server = FakeServer::default();
        let uploads = server.upload_calls.clone();
        let updates = server.update_folder_calls.clone();
        let rig = new_rig(server).await;
        let old = rig.root.join("Old");
        let new = rig.root.join("New");
        std::fs::create_dir(&old).unwrap();
        std::fs::write(old.join("note.txt"), b"original!").unwrap();
        let engine = build_engine(&rig).await;
        engine.incremental_sync().await.unwrap();
        let mtime = std::fs::metadata(old.join("note.txt"))
            .unwrap()
            .modified()
            .unwrap();
        std::fs::remove_dir_all(&old).unwrap();
        std::fs::create_dir(&new).unwrap();
        std::fs::write(new.join("note.txt"), contents).unwrap();
        std::fs::File::options()
            .write(true)
            .open(new.join("note.txt"))
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(mtime))
            .unwrap();
        for _ in 0..2 {
            let report = engine.incremental_sync().await.unwrap();
            assert!(report.errors.is_empty(), "{:?}", report.errors);
            assert!(report.renamed_remote_folders.is_empty());
        }
        assert_eq!(
            uploads.load(Ordering::SeqCst),
            2,
            "new content must be uploaded"
        );
        assert_eq!(updates.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn legacy_journal_without_checksums_falls_back_to_upload() {
    let server = FakeServer::default();
    let uploads = server.upload_calls.clone();
    let rig = new_rig(server).await;
    std::fs::create_dir(rig.root.join("Old")).unwrap();
    std::fs::write(rig.root.join("Old/note.txt"), b"content").unwrap();
    let engine = build_engine(&rig).await;
    engine.incremental_sync().await.unwrap();
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", rig.db_path.display()))
        .await
        .unwrap();
    sqlx::query("UPDATE sync_state SET checksum = NULL")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    std::fs::rename(rig.root.join("Old"), rig.root.join("New")).unwrap();
    let report = engine.incremental_sync().await.unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(report.renamed_remote_folders.is_empty());
    assert_eq!(uploads.load(Ordering::SeqCst), 2);
}

/// Rows written before checksums were recorded take theirs from the server
/// tree, so a folder synced by an older build is still renamed in place.
#[tokio::test]
async fn legacy_journal_takes_checksums_from_the_server_and_renames() {
    let server = FakeServer::default();
    let uploads = server.upload_calls.clone();
    let updates = server.update_folder_calls.clone();
    let checksums = server.checksums.clone();
    let files = server.files.clone();
    let rig = new_rig(server).await;
    std::fs::create_dir(rig.root.join("Old")).unwrap();
    std::fs::write(rig.root.join("Old/note.txt"), b"content").unwrap();
    let engine = build_engine(&rig).await;
    engine.incremental_sync().await.unwrap();
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", rig.db_path.display()))
        .await
        .unwrap();
    sqlx::query("UPDATE sync_state SET checksum = NULL")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    let file_id = files.lock().await.keys().next().unwrap().clone();
    checksums.lock().await.insert(
        file_id,
        uncloud_sync::sha256_hex_reader(&b"content"[..]).unwrap(),
    );

    std::fs::rename(rig.root.join("Old"), rig.root.join("New")).unwrap();
    let report = engine.incremental_sync().await.unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.renamed_remote_folders.len(), 1, "{:?}", report);
    assert_eq!(updates.load(Ordering::SeqCst), 1);
    assert_eq!(uploads.load(Ordering::SeqCst), 1, "nothing is re-uploaded");
}

/// Local-only folder + file → server gets the folder created and the file
/// uploaded under that folder.
#[tokio::test]
async fn local_only_top_level_folder_is_created_on_server() {
    let server = FakeServer::default();
    let folders_seen = server.folders.clone();
    let files_seen = server.files.clone();
    let create_calls = server.create_folder_calls.clone();
    let upload_calls = server.upload_calls.clone();

    let rig = new_rig(server).await;

    // Local: a single new folder with a file inside.
    std::fs::create_dir(rig.root.join("MyFolder")).unwrap();
    std::fs::write(rig.root.join("MyFolder/cat.txt"), b"meow").unwrap();

    let engine = build_engine(&rig).await;
    let report = engine.incremental_sync().await.unwrap();

    assert_eq!(create_calls.load(Ordering::SeqCst), 1, "one folder POST");
    assert_eq!(upload_calls.load(Ordering::SeqCst), 1, "one file upload");

    let folders = folders_seen.lock().await;
    assert_eq!(folders.len(), 1);
    let f = folders.values().next().unwrap();
    assert_eq!(f.name, "MyFolder");
    assert_eq!(f.parent_id, None);

    let files = files_seen.lock().await;
    assert_eq!(files.len(), 1);
    let file = files.values().next().unwrap();
    assert_eq!(file.name, "cat.txt");
    assert_eq!(
        file.parent_id.as_deref(),
        Some(f.id.as_str()),
        "uploaded file must be parented under the freshly-created server folder"
    );

    assert_eq!(report.created_remote_folders, vec!["MyFolder".to_string()]);
    assert_eq!(report.uploaded, vec!["cat.txt".to_string()]);
    assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
}

/// Nested local-only folders: parent must be created before child, and the
/// file at the deepest level must be uploaded under the deepest folder id.
#[tokio::test]
async fn nested_local_only_folders_are_created_in_order() {
    let server = FakeServer::default();
    let folders_seen = server.folders.clone();
    let files_seen = server.files.clone();

    let rig = new_rig(server).await;

    std::fs::create_dir_all(rig.root.join("Photos/2026")).unwrap();
    std::fs::write(rig.root.join("Photos/2026/img.jpg"), b"jpg-bytes").unwrap();

    let engine = build_engine(&rig).await;
    let report = engine.incremental_sync().await.unwrap();

    let folders = folders_seen.lock().await;
    assert_eq!(folders.len(), 2, "two folders should be created");

    let photos = folders
        .values()
        .find(|f| f.name == "Photos")
        .expect("Photos folder created");
    assert_eq!(photos.parent_id, None);

    let year = folders
        .values()
        .find(|f| f.name == "2026")
        .expect("2026 folder created");
    assert_eq!(
        year.parent_id.as_deref(),
        Some(photos.id.as_str()),
        "2026 must be parented under Photos, not at root"
    );

    let files = files_seen.lock().await;
    assert_eq!(files.len(), 1);
    let file = files.values().next().unwrap();
    assert_eq!(file.name, "img.jpg");
    assert_eq!(
        file.parent_id.as_deref(),
        Some(year.id.as_str()),
        "img.jpg must land under 2026, not Photos"
    );

    assert_eq!(report.created_remote_folders.len(), 2);
    assert_eq!(report.uploaded.len(), 1);
    assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
}

/// Re-running sync with no changes must NOT create the same folders again
/// or duplicate uploads — the journal entries from run #1 should make
/// run #2 a no-op.
#[tokio::test]
async fn second_sync_is_a_noop() {
    let server = FakeServer::default();
    let create_calls = server.create_folder_calls.clone();
    let upload_calls = server.upload_calls.clone();

    let rig = new_rig(server).await;
    std::fs::create_dir(rig.root.join("Stuff")).unwrap();
    std::fs::write(rig.root.join("Stuff/note.txt"), b"hi").unwrap();

    let engine = build_engine(&rig).await;
    let _ = engine.incremental_sync().await.unwrap();
    assert_eq!(create_calls.load(Ordering::SeqCst), 1);
    assert_eq!(upload_calls.load(Ordering::SeqCst), 1);

    let report = engine.incremental_sync().await.unwrap();
    assert_eq!(
        create_calls.load(Ordering::SeqCst),
        1,
        "second run must not re-create the folder"
    );
    assert_eq!(
        upload_calls.load(Ordering::SeqCst),
        1,
        "second run must not re-upload the file"
    );
    assert!(report.created_remote_folders.is_empty());
    assert!(report.uploaded.is_empty());
}

#[tokio::test]
async fn local_folder_rename_is_mirrored_as_a_server_rename() {
    let server = FakeServer::default();
    let folders = server.folders.clone();
    let files = server.files.clone();
    let create_calls = server.create_folder_calls.clone();
    let delete_calls = server.delete_folder_calls.clone();
    let update_calls = server.update_folder_calls.clone();
    let upload_calls = server.upload_calls.clone();

    let rig = new_rig(server).await;
    let old_path = rig.root.join("Old name");
    let new_path = rig.root.join("New name");
    std::fs::create_dir_all(old_path.join("inner")).unwrap();
    std::fs::write(old_path.join("note.txt"), b"hello").unwrap();
    std::fs::write(old_path.join("inner/deep.txt"), b"deep").unwrap();

    let engine = build_engine(&rig).await;
    engine.incremental_sync().await.unwrap();
    assert_eq!(create_calls.load(Ordering::SeqCst), 2);
    assert_eq!(upload_calls.load(Ordering::SeqCst), 2);
    let folder_ids_before: std::collections::BTreeSet<String> =
        folders.lock().await.keys().cloned().collect();
    let file_ids_before: std::collections::BTreeSet<String> =
        files.lock().await.keys().cloned().collect();

    std::fs::rename(&old_path, &new_path).unwrap();

    let report = engine.incremental_sync().await.unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.renamed_remote_folders.len(), 1);
    assert_eq!(update_calls.load(Ordering::SeqCst), 1);
    assert!(!old_path.exists(), "the old folder name must stay absent");
    assert!(new_path.join("inner/deep.txt").is_file());

    // The rename is settled: nothing is deleted, recreated or re-uploaded
    // now or on later scans, so ids (and with them shares and version
    // history) survive.
    for _ in 0..2 {
        let report = engine.incremental_sync().await.unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(report.uploaded.is_empty(), "{:?}", report.uploaded);
    }
    assert_eq!(create_calls.load(Ordering::SeqCst), 2);
    assert_eq!(upload_calls.load(Ordering::SeqCst), 2);
    assert_eq!(delete_calls.load(Ordering::SeqCst), 0);
    assert_eq!(update_calls.load(Ordering::SeqCst), 1);

    let folders = folders.lock().await;
    assert_eq!(
        folders
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        folder_ids_before
    );
    assert!(folders.values().any(|f| f.name == "New name"));
    assert!(!folders.values().any(|f| f.name == "Old name"));
    assert_eq!(
        files
            .lock()
            .await
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        file_ids_before
    );
}

#[tokio::test]
async fn local_folder_move_changes_the_server_parent() {
    let server = FakeServer::default();
    let folders = server.folders.clone();
    let create_calls = server.create_folder_calls.clone();
    let delete_calls = server.delete_folder_calls.clone();
    let update_calls = server.update_folder_calls.clone();

    let rig = new_rig(server).await;
    std::fs::create_dir_all(rig.root.join("Album")).unwrap();
    std::fs::create_dir_all(rig.root.join("Archive")).unwrap();
    std::fs::write(rig.root.join("Album/track.txt"), b"audio").unwrap();

    let engine = build_engine(&rig).await;
    engine.incremental_sync().await.unwrap();
    assert_eq!(create_calls.load(Ordering::SeqCst), 2);

    std::fs::rename(rig.root.join("Album"), rig.root.join("Archive/Album")).unwrap();
    let report = engine.incremental_sync().await.unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    engine.incremental_sync().await.unwrap();

    assert_eq!(update_calls.load(Ordering::SeqCst), 1);
    assert_eq!(create_calls.load(Ordering::SeqCst), 2);
    assert_eq!(delete_calls.load(Ordering::SeqCst), 0);
    let folders = folders.lock().await;
    let archive = folders.values().find(|f| f.name == "Archive").unwrap();
    let album = folders.values().find(|f| f.name == "Album").unwrap();
    assert_eq!(album.parent_id.as_deref(), Some(archive.id.as_str()));
}

// An empty folder carries nothing to recognise it by, so its rename is
// still a delete of the old name plus a create of the new one.
#[tokio::test]
async fn empty_folder_rename_falls_back_to_delete_and_create() {
    let server = FakeServer::default();
    let folders = server.folders.clone();
    let create_calls = server.create_folder_calls.clone();
    let delete_calls = server.delete_folder_calls.clone();
    let update_calls = server.update_folder_calls.clone();

    let rig = new_rig(server).await;
    std::fs::create_dir(rig.root.join("Old name")).unwrap();
    let engine = build_engine(&rig).await;
    engine.incremental_sync().await.unwrap();

    std::fs::rename(rig.root.join("Old name"), rig.root.join("New name")).unwrap();
    engine.incremental_sync().await.unwrap();
    assert!(!rig.root.join("Old name").exists());
    assert_eq!(create_calls.load(Ordering::SeqCst), 2);
    assert_eq!(delete_calls.load(Ordering::SeqCst), 0);

    engine.incremental_sync().await.unwrap();
    assert_eq!(delete_calls.load(Ordering::SeqCst), 1);
    assert_eq!(update_calls.load(Ordering::SeqCst), 0);
    let folders = folders.lock().await;
    assert_eq!(folders.len(), 1);
    assert_eq!(folders.values().next().unwrap().name, "New name");
}

/// The engine's `state()` watch broadcasts the connection/activity state
/// to embedding apps so the desktop tray maps one icon per real state.
/// The state starts at `NotConnected`, becomes `Transferring` once on the
/// first transfer of a run (sticky: subsequent transfers in the same run
/// emit no events), and settles at `Connected` when the run completes
/// successfully.
#[tokio::test]
async fn state_watch_is_sticky_transferring_and_settles_at_connected() {
    use uncloud_sync::SyncState;

    let server = FakeServer::default();
    let rig = new_rig(server).await;
    std::fs::create_dir(rig.root.join("Folder")).unwrap();
    // Two files so we can verify Transferring isn't re-emitted between
    // sequential ops.
    std::fs::write(rig.root.join("Folder/a.txt"), b"x").unwrap();
    std::fs::write(rig.root.join("Folder/b.txt"), b"y").unwrap();

    let engine = build_engine(&rig).await;
    let mut rx = engine.state();
    assert_eq!(*rx.borrow(), SyncState::NotConnected);

    let collector = tokio::spawn(async move {
        let mut seen = Vec::<SyncState>::new();
        while rx.changed().await.is_ok() {
            seen.push(*rx.borrow());
            if matches!(seen.last(), Some(SyncState::Connected)) {
                break;
            }
        }
        seen
    });

    let report = engine.incremental_sync().await.unwrap();
    assert_eq!(report.uploaded.len(), 2);

    let seen = collector.await.unwrap();
    let transferring_count = seen
        .iter()
        .filter(|s| **s == SyncState::Transferring)
        .count();
    assert_eq!(
        transferring_count, 1,
        "expected exactly one Transferring transition for two transfers (sticky), got {:?}",
        seen
    );
    assert_eq!(
        seen.last().copied(),
        Some(SyncState::Connected),
        "run must end at Connected, got {:?}",
        seen
    );
}
