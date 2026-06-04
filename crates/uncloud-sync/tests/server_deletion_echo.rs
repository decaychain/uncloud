//! Regression coverage for the "server-side folder delete bounced back as
//! a fresh upload" bug.
//!
//! Scenario: a folder is synced (server tree + local dir + journal row).
//! The user deletes it via the web UI. On the next sync, the server tree
//! no longer contains the folder. Phase 6.1 must echo the delete locally
//! (dropping the journal row and, if the local directory is empty,
//! removing it) so Phase 6.5 does not see the dangling local dir and
//! re-create it on the server.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::{oneshot, Mutex};
use uncloud_client::Client;
use uncloud_sync::SyncEngine;

#[derive(Clone)]
struct ServerFolder {
    id: String,
    name: String,
    parent_id: Option<String>,
    updated_at: String,
}

#[derive(Clone)]
struct FakeServer {
    folders: Arc<Mutex<HashMap<String, ServerFolder>>>,
    folder_create_hits: Arc<AtomicUsize>,
}

impl FakeServer {
    fn with(folders: Vec<ServerFolder>) -> Self {
        let folders = folders.into_iter().map(|f| (f.id.clone(), f)).collect();
        Self {
            folders: Arc::new(Mutex::new(folders)),
            folder_create_hits: Arc::new(AtomicUsize::new(0)),
        }
    }
}

async fn sync_tree(State(s): State<FakeServer>) -> impl IntoResponse {
    let folders_guard = s.folders.lock().await;
    let folders: Vec<_> = folders_guard
        .values()
        .map(|f| {
            serde_json::json!({
                "id": f.id,
                "name": f.name,
                "parent_id": f.parent_id,
                "created_at": f.updated_at,
                "updated_at": f.updated_at,
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
    Json(serde_json::json!({ "files": [], "folders": folders }))
}

async fn create_folder_handler(
    State(s): State<FakeServer>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let hits = s.folder_create_hits.fetch_add(1, Ordering::SeqCst) + 1;
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("anon")
        .to_owned();
    let parent_id = body
        .get("parent_id")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let id = format!("fld_created_{hits}");
    let updated_at = "2026-02-01T00:00:00Z".to_owned();
    s.folders.lock().await.insert(
        id.clone(),
        ServerFolder {
            id: id.clone(),
            name: name.clone(),
            parent_id: parent_id.clone(),
            updated_at: updated_at.clone(),
        },
    );
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "id": id,
            "name": name,
            "parent_id": parent_id,
            "created_at": updated_at,
            "updated_at": updated_at,
            "sync_strategy": "inherit",
            "effective_strategy": "two_way",
            "gallery_include": "inherit",
            "effective_gallery_include": "exclude",
            "music_include": "inherit",
            "effective_music_include": "exclude",
            "shared_with_count": 0,
        })),
    )
        .into_response()
}

async fn spawn_server(state: FakeServer) -> (String, oneshot::Sender<()>) {
    let app = Router::new()
        .route("/api/sync/tree", get(sync_tree))
        .route("/api/folders", post(create_folder_handler))
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

#[tokio::test]
async fn empty_local_dir_for_server_deleted_folder_is_removed_and_not_re_uploaded() {
    let server = FakeServer::with(vec![ServerFolder {
        id: "fld_test_hetzner".into(),
        name: "Test_Hetzner".into(),
        parent_id: None,
        updated_at: "2026-01-01T00:00:00Z".into(),
    }]);
    let create_hits = server.folder_create_hits.clone();
    let folders = server.folders.clone();

    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("sync.db");
    let root = temp.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    let (url, _shutdown) = spawn_server(server).await;
    let client = Arc::new(Client::new(&url));
    let engine = SyncEngine::new(&db_path, client, Some(root.to_string_lossy().into_owned()))
        .await
        .unwrap();

    // 1. First sync: server has Test_Hetzner. Engine creates the local
    //    dir and records the journal row.
    engine.run_sync_manual().await.unwrap();
    let local_dir = root.join("Test_Hetzner");
    assert!(local_dir.is_dir(), "first sync must create local dir");

    // 2. User deletes the folder via the web UI — the server tree no
    //    longer contains it.
    folders.lock().await.remove("fld_test_hetzner");

    // 3. Second sync: Phase 6.1 should remove the orphan local dir and
    //    drop the journal row; Phase 6.5 must NOT re-create it on the
    //    server.
    engine.run_sync_manual().await.unwrap();

    assert_eq!(
        create_hits.load(Ordering::SeqCst),
        0,
        "engine resurrected the server-deleted folder"
    );
    assert!(
        !local_dir.exists(),
        "empty local dir for server-deleted folder must be removed"
    );

    // 4. Third sync confirms the journal row is gone — no zombie loop.
    engine.run_sync_manual().await.unwrap();
    assert_eq!(
        create_hits.load(Ordering::SeqCst),
        0,
        "later syncs must not bounce-back the folder either"
    );
}

#[tokio::test]
async fn non_empty_local_dir_keeps_contents_and_recreates_parent() {
    let server = FakeServer::with(vec![ServerFolder {
        id: "fld_keep".into(),
        name: "KeepMyStuff".into(),
        parent_id: None,
        updated_at: "2026-01-01T00:00:00Z".into(),
    }]);
    let create_hits = server.folder_create_hits.clone();
    let folders = server.folders.clone();

    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("sync.db");
    let root = temp.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    let (url, _shutdown) = spawn_server(server).await;
    let client = Arc::new(Client::new(&url));
    let engine = SyncEngine::new(&db_path, client, Some(root.to_string_lossy().into_owned()))
        .await
        .unwrap();

    engine.run_sync_manual().await.unwrap();
    let local_dir = root.join("KeepMyStuff");
    assert!(local_dir.is_dir(), "first sync must create local dir");

    // Drop a local-only file the engine never knew about.
    std::fs::write(local_dir.join("unsynced.txt"), b"local only").unwrap();

    // Server deletes the folder.
    folders.lock().await.remove("fld_keep");

    // Phase 6.1 should drop the journal row but NOT remove the non-empty
    // dir. Phase 6.5 re-creates the folder on the server so the orphan
    // file has a parent to live under. We accept that single re-create —
    // the contract is "don't lose data", not "never call create after a
    // server delete".
    engine.run_sync_manual().await.unwrap();

    assert!(
        local_dir.join("unsynced.txt").exists(),
        "unsynced local file must survive a server-side parent delete"
    );
    // Folder create call is expected (the dir still has content), but
    // it's a one-shot — the journal now tracks the *new* folder id,
    // breaking the loop.
    let first = create_hits.load(Ordering::SeqCst);
    assert!(
        first >= 1,
        "engine must re-attach the non-empty local dir to the server"
    );

    // Subsequent sync must not keep creating folders — the loop is the
    // bug we're fixing.
    engine.run_sync_manual().await.unwrap();
    engine.run_sync_manual().await.unwrap();
    let later = create_hits.load(Ordering::SeqCst);
    assert_eq!(later, first, "no zombie loop after the one-shot re-create");
}
