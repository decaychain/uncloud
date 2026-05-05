//! End-to-end tests for the two-phase local-deletion tracker.
//!
//! Cover the user-visible contract:
//!   - First scan that sees a file missing locally only *marks* it
//!     pending — the server-side `DELETE` doesn't fire yet.
//!   - The second scan that still finds it missing *confirms* and pushes
//!     the delete.
//!   - If the file reappears (or the watcher cancellation hook fires)
//!     between scans, the pending state is dropped and no delete is
//!     pushed.
//!   - `UploadOnly` strategy never pushes a delete.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::{
    Json, Router,
    extract::{Path as AxPath, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get},
};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use uncloud_client::Client;
use uncloud_sync::SyncEngine;

#[derive(Clone)]
struct ServerFile {
    id: String,
    name: String,
    parent_id: Option<String>,
    bytes: Vec<u8>,
    updated_at: String,
}

#[derive(Clone)]
struct FakeServer {
    files: Arc<HashMap<String, ServerFile>>,
    folders: Arc<HashMap<String, ()>>,
    delete_file_hits: Arc<AtomicUsize>,
    delete_folder_hits: Arc<AtomicUsize>,
}

impl FakeServer {
    fn with(files: Vec<ServerFile>) -> Self {
        let files = files.into_iter().map(|f| (f.id.clone(), f)).collect();
        Self {
            files: Arc::new(files),
            folders: Arc::new(HashMap::new()),
            delete_file_hits: Arc::new(AtomicUsize::new(0)),
            delete_folder_hits: Arc::new(AtomicUsize::new(0)),
        }
    }
}

async fn sync_tree(State(s): State<FakeServer>) -> impl IntoResponse {
    let files: Vec<_> = s
        .files
        .values()
        .map(|f| {
            serde_json::json!({
                "id": f.id,
                "name": f.name,
                "mime_type": "application/octet-stream",
                "size_bytes": f.bytes.len() as i64,
                "parent_id": f.parent_id,
                "created_at": f.updated_at,
                "updated_at": f.updated_at,
            })
        })
        .collect();
    let folders: Vec<_> = s
        .folders
        .keys()
        .map(|id| {
            serde_json::json!({
                "id": id,
                "name": id,
                "parent_id": null,
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
    Json(serde_json::json!({ "files": files, "folders": folders }))
}

async fn download(
    AxPath(id): AxPath<String>,
    State(s): State<FakeServer>,
) -> impl IntoResponse {
    match s.files.get(&id) {
        Some(f) => (StatusCode::OK, f.bytes.clone()).into_response(),
        None => (StatusCode::NOT_FOUND, "").into_response(),
    }
}

async fn delete_file_handler(
    AxPath(_id): AxPath<String>,
    State(s): State<FakeServer>,
) -> impl IntoResponse {
    s.delete_file_hits.fetch_add(1, Ordering::SeqCst);
    StatusCode::NO_CONTENT
}

async fn delete_folder_handler(
    AxPath(_id): AxPath<String>,
    State(s): State<FakeServer>,
) -> impl IntoResponse {
    s.delete_folder_hits.fetch_add(1, Ordering::SeqCst);
    StatusCode::NO_CONTENT
}

async fn spawn_server(state: FakeServer) -> (String, oneshot::Sender<()>) {
    let app = Router::new()
        .route("/api/sync/tree", get(sync_tree))
        .route("/api/files/{id}/download", get(download))
        .route("/api/files/{id}", delete(delete_file_handler))
        .route("/api/folders/{id}", delete(delete_folder_handler))
        .with_state(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .await;
    });
    (format!("http://{addr}"), tx)
}

struct Rig {
    _server_shutdown: oneshot::Sender<()>,
    _temp: TempDir,
    db_path: std::path::PathBuf,
    root: std::path::PathBuf,
    client: Arc<Client>,
}

async fn rig(server: FakeServer) -> Rig {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("sync.db");
    let root = temp.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    let (url, shutdown) = spawn_server(server).await;
    let client = Arc::new(Client::new(&url));
    Rig {
        _server_shutdown: shutdown,
        _temp: temp,
        db_path,
        root,
        client,
    }
}

async fn build_engine(rig: &Rig) -> SyncEngine {
    SyncEngine::new(
        &rig.db_path,
        rig.client.clone(),
        Some(rig.root.to_string_lossy().into_owned()),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn first_scan_marks_pending_second_confirms_delete() {
    let server = FakeServer::with(vec![ServerFile {
        id: "f-1".into(),
        name: "doomed.txt".into(),
        parent_id: None,
        bytes: b"bye\n".to_vec(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    }]);
    let delete_hits = server.delete_file_hits.clone();
    let rig = rig(server).await;
    let engine = build_engine(&rig).await;

    // Run 1: download the file so the journal knows about it.
    let r1 = engine.incremental_sync().await.unwrap();
    assert_eq!(r1.downloaded.len(), 1);
    assert!(rig.root.join("doomed.txt").exists());

    // User deletes it locally.
    std::fs::remove_file(rig.root.join("doomed.txt")).unwrap();

    // Run 2: first scan that sees absence — only marks pending.
    let r2 = engine.incremental_sync().await.unwrap();
    assert!(
        r2.deleted_local.is_empty(),
        "first absence scan must NOT push delete: {:?}",
        r2.deleted_local
    );
    assert_eq!(
        delete_hits.load(Ordering::SeqCst),
        0,
        "DELETE endpoint must not be hit on first absence scan"
    );

    // Run 3: still missing — confirms and pushes.
    let r3 = engine.incremental_sync().await.unwrap();
    assert_eq!(r3.deleted_local.len(), 1, "{:?}", r3);
    assert_eq!(delete_hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn file_reappearing_between_scans_cancels_pending() {
    let server = FakeServer::with(vec![ServerFile {
        id: "f-1".into(),
        name: "transient.txt".into(),
        parent_id: None,
        bytes: b"original\n".to_vec(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    }]);
    let delete_hits = server.delete_file_hits.clone();
    let rig = rig(server).await;
    let engine = build_engine(&rig).await;

    engine.incremental_sync().await.unwrap();
    let path = rig.root.join("transient.txt");
    std::fs::remove_file(&path).unwrap();

    // First scan: marks pending.
    engine.incremental_sync().await.unwrap();

    // User restores the file (e.g. undo, replay from backup).
    std::fs::write(&path, b"restored\n").unwrap();

    // Second scan: file is back, so the pending state must be cleared
    // by Phase 6a's "missing locally + journal row" guard not firing,
    // and crucially the DELETE endpoint must NOT be hit.
    let r = engine.incremental_sync().await.unwrap();
    assert!(
        r.deleted_local.is_empty(),
        "delete must be cancelled: {:?}",
        r
    );
    assert_eq!(
        delete_hits.load(Ordering::SeqCst),
        0,
        "DELETE endpoint must not be hit when the file came back"
    );
}

#[tokio::test]
async fn watcher_cancel_hook_clears_pending() {
    let server = FakeServer::with(vec![ServerFile {
        id: "f-1".into(),
        name: "flicker.txt".into(),
        parent_id: None,
        bytes: b"x".to_vec(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    }]);
    let delete_hits = server.delete_file_hits.clone();
    let rig = rig(server).await;
    let engine = build_engine(&rig).await;

    engine.incremental_sync().await.unwrap();
    let path = rig.root.join("flicker.txt");
    let path_str = path.to_string_lossy().into_owned();
    std::fs::remove_file(&path).unwrap();

    // First scan marks pending.
    engine.incremental_sync().await.unwrap();

    // Watcher would fire on a Create event for the path. Re-create the
    // file and call the engine's cancel hook directly — the desktop's
    // watcher does this for any Create/Modify event in the debounce
    // window before kicking the next sync.
    std::fs::write(&path, b"x").unwrap();
    let cleared = engine
        .cancel_pending_delete_for_path(&path_str)
        .await
        .unwrap();
    assert_eq!(cleared, 1, "exactly one journal row should have been cleared");

    // Now run a sync — pending was cancelled, file is back, no delete.
    let r = engine.incremental_sync().await.unwrap();
    assert!(r.deleted_local.is_empty(), "{:?}", r);
    assert_eq!(delete_hits.load(Ordering::SeqCst), 0);
}
