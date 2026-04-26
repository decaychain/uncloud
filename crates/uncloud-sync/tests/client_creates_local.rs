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
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::{
    Json, Router,
    extract::{Json as AxJson, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, oneshot};
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
    upload_calls: Arc<AtomicUsize>,
}

#[derive(serde::Deserialize)]
struct CreateFolderBody {
    name: String,
    parent_id: Option<String>,
}

async fn sync_tree(State(s): State<FakeServer>) -> impl IntoResponse {
    let folders = s.folders.lock().await;
    let files = s.files.lock().await;
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
    .unwrap();
    Arc::new(engine)
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

/// The engine's `activity()` watch broadcasts coarse state to embedding
/// apps so the desktop tray icon can flag "I'm actually transferring
/// bytes" vs "I'm only checking the tree." This test runs a sync that
/// uploads one file and asserts the broadcast goes through both
/// `Polling` (run started, no transfers yet) and `Transferring` (the
/// upload is in flight) before returning to `Idle`.
#[tokio::test]
async fn activity_watch_reports_polling_and_transferring() {
    use uncloud_sync::SyncActivity;

    let server = FakeServer::default();
    let rig = new_rig(server).await;
    std::fs::create_dir(rig.root.join("Folder")).unwrap();
    std::fs::write(rig.root.join("Folder/data.txt"), b"x").unwrap();

    let engine = build_engine(&rig).await;
    let mut rx = engine.activity();
    assert_eq!(*rx.borrow(), SyncActivity::Idle);

    // Collect every transition the engine emits during the sync run.
    let collector = tokio::spawn(async move {
        let mut seen = Vec::<SyncActivity>::new();
        while rx.changed().await.is_ok() {
            seen.push(*rx.borrow());
            // Once we are back at Idle the run is over and we can stop.
            if matches!(seen.last(), Some(SyncActivity::Idle)) {
                break;
            }
        }
        seen
    });

    let report = engine.incremental_sync().await.unwrap();
    assert_eq!(report.uploaded, vec!["data.txt".to_string()]);

    let seen = collector.await.unwrap();
    assert!(
        seen.contains(&SyncActivity::Polling),
        "expected Polling transition, got {:?}",
        seen
    );
    assert!(
        seen.contains(&SyncActivity::Transferring),
        "expected Transferring transition, got {:?}",
        seen
    );
    assert_eq!(
        seen.last().copied(),
        Some(SyncActivity::Idle),
        "run must end at Idle, got {:?}",
        seen
    );
}
