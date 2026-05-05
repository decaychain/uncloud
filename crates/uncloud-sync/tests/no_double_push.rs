//! Regression coverage for the "downloaded then immediately re-uploaded"
//! bug. The first sync against a server that has files but a local data
//! folder that is empty (or stale-journal scenarios) must NOT bounce
//! freshly-downloaded files back as new local files.
//!
//! These tests stand up a tiny axum app that mimics the small slice of the
//! Uncloud API the sync engine actually drives: `/api/sync/tree`,
//! `/api/files/{id}/download`, `/api/uploads/simple`, and
//! `/api/files/{id}/content`. The upload + content-replace endpoints
//! increment counters; an assertion at the end of the test fails the run
//! if either was hit.
//!
//! Tests are tokio-based and self-contained — each spins up its own server
//! on an ephemeral port and points a real `uncloud_client::Client` at it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::{
    Json, Router,
    extract::{Path as AxPath, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use uncloud_client::Client;
use uncloud_sync::SyncEngine;

// ── Fake server state ────────────────────────────────────────────────────────

#[derive(Clone)]
struct ServerFile {
    id: String,
    name: String,
    parent_id: Option<String>,
    bytes: Vec<u8>,
    updated_at: String,
}

#[derive(Clone)]
struct ServerFolder {
    id: String,
    name: String,
    parent_id: Option<String>,
    updated_at: String,
}

#[derive(Clone)]
struct FakeServer {
    files: Arc<HashMap<String, ServerFile>>,
    folders: Arc<HashMap<String, ServerFolder>>,
    /// Hits on `/api/uploads/simple` — a non-zero value means the engine
    /// tried to push something it shouldn't have.
    upload_hits: Arc<AtomicUsize>,
    /// Hits on `/api/files/{id}/content` — same alarm bell, but for the
    /// "Updated on server" path.
    content_hits: Arc<AtomicUsize>,
    /// Hits on `/api/files/{id}/download` — used by the concurrency test to
    /// confirm the second sync skips downloads because the lock made it
    /// run after the first one finished.
    download_hits: Arc<AtomicUsize>,
    /// Optional artificial delay on each download response so the
    /// concurrency test can guarantee two syncs are in flight at the same
    /// time.
    download_delay_ms: Arc<std::sync::atomic::AtomicU64>,
    /// Peak number of overlapping download handlers — set to 1 with the
    /// engine-level mutex working, ≥ 2 if it isn't.
    in_flight_downloads: Arc<AtomicUsize>,
    peak_in_flight_downloads: Arc<AtomicUsize>,
}

impl FakeServer {
    fn new(files: Vec<ServerFile>, folders: Vec<ServerFolder>) -> Self {
        let files = files.into_iter().map(|f| (f.id.clone(), f)).collect();
        let folders = folders.into_iter().map(|f| (f.id.clone(), f)).collect();
        Self {
            files: Arc::new(files),
            folders: Arc::new(folders),
            upload_hits: Arc::new(AtomicUsize::new(0)),
            content_hits: Arc::new(AtomicUsize::new(0)),
            download_hits: Arc::new(AtomicUsize::new(0)),
            download_delay_ms: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            in_flight_downloads: Arc::new(AtomicUsize::new(0)),
            peak_in_flight_downloads: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn set_download_delay_ms(&self, ms: u64) {
        self.download_delay_ms
            .store(ms, std::sync::atomic::Ordering::SeqCst);
    }
}

// ── Handlers ────────────────────────────────────────────────────────────────

async fn sync_tree(State(s): State<FakeServer>) -> impl IntoResponse {
    // Build a SyncTreeResponse-shaped JSON. Folder defaults: TwoWay /
    // Inherit / no gallery / no music — the engine only inspects
    // `effective_strategy`, `parent_id`, and `name`.
    let folders: Vec<_> = s
        .folders
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
    Json(serde_json::json!({ "files": files, "folders": folders }))
}

async fn download(
    AxPath(id): AxPath<String>,
    State(s): State<FakeServer>,
) -> impl IntoResponse {
    use std::sync::atomic::Ordering;

    s.download_hits.fetch_add(1, Ordering::SeqCst);
    let now_in_flight = s.in_flight_downloads.fetch_add(1, Ordering::SeqCst) + 1;
    // Track the peak so the concurrency test can assert that the
    // engine-level mutex really did serialize the runs.
    let mut prev = s.peak_in_flight_downloads.load(Ordering::SeqCst);
    while now_in_flight > prev {
        match s.peak_in_flight_downloads.compare_exchange(
            prev,
            now_in_flight,
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            Ok(_) => break,
            Err(actual) => prev = actual,
        }
    }

    let delay = s.download_delay_ms.load(Ordering::SeqCst);
    if delay > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
    }

    let response = match s.files.get(&id) {
        Some(f) => (StatusCode::OK, f.bytes.clone()).into_response(),
        None => (StatusCode::NOT_FOUND, "no such file").into_response(),
    };
    s.in_flight_downloads.fetch_sub(1, Ordering::SeqCst);
    response
}

async fn upload_simple(State(s): State<FakeServer>) -> impl IntoResponse {
    // Don't even try to parse the multipart — bumping the counter is the
    // whole point. 409 mimics the duplicate-key behaviour the real server
    // would produce for these calls.
    s.upload_hits.fetch_add(1, Ordering::SeqCst);
    (StatusCode::CONFLICT, "spurious upload — file already exists").into_response()
}

async fn content_replace(
    AxPath(_id): AxPath<String>,
    State(s): State<FakeServer>,
) -> impl IntoResponse {
    s.content_hits.fetch_add(1, Ordering::SeqCst);
    (StatusCode::CONFLICT, "spurious content replace").into_response()
}

// ── Test harness ────────────────────────────────────────────────────────────

/// Spin up the fake server on an ephemeral port. Returns the bound URL and
/// a oneshot sender to gracefully shut the server down at end of test.
async fn spawn_server(state: FakeServer) -> (String, oneshot::Sender<()>) {
    let app = Router::new()
        .route("/api/sync/tree", get(sync_tree))
        .route("/api/files/{id}/download", get(download))
        .route("/api/uploads/simple", post(upload_simple))
        .route("/api/files/{id}/content", post(content_replace))
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

// ── Tests ───────────────────────────────────────────────────────────────────

/// Empty journal, empty data folder, server has files at root and inside
/// nested folders. Phase 5 should download them; Phase 7 must NOT push them
/// back up.
#[tokio::test]
async fn fresh_sync_does_not_push_back_downloaded_files() {
    let server = FakeServer::new(
        vec![
            ServerFile {
                id: "f-root".into(),
                name: "hello.txt".into(),
                parent_id: None,
                bytes: b"hello\n".to_vec(),
                updated_at: "2026-01-01T00:00:00Z".into(),
            },
            ServerFile {
                id: "f-nested".into(),
                name: "nested.txt".into(),
                parent_id: Some("dir-docs".into()),
                bytes: b"nested\n".to_vec(),
                updated_at: "2026-01-01T00:00:00Z".into(),
            },
        ],
        vec![ServerFolder {
            id: "dir-docs".into(),
            name: "docs".into(),
            parent_id: None,
            updated_at: "2026-01-01T00:00:00Z".into(),
        }],
    );
    let upload_hits = server.upload_hits.clone();
    let content_hits = server.content_hits.clone();

    let rig = new_rig(server).await;
    let engine = build_engine(&rig).await;

    let report = engine.incremental_sync().await.unwrap();

    assert_eq!(report.downloaded.len(), 2, "should download both files");
    assert_eq!(
        report.uploaded.len(),
        0,
        "must not upload anything; report.uploaded={:?}",
        report.uploaded
    );
    assert_eq!(report.errors.len(), 0, "no errors expected: {:?}", report.errors);
    assert_eq!(
        upload_hits.load(Ordering::SeqCst),
        0,
        "upload endpoint must not be hit"
    );
    assert_eq!(
        content_hits.load(Ordering::SeqCst),
        0,
        "content-replace endpoint must not be hit"
    );

    // The actual files should be on disk.
    assert!(rig.root.join("hello.txt").is_file());
    assert!(rig.root.join("docs/nested.txt").is_file());
}

/// Two consecutive syncs: nothing should be uploaded across either run when
/// the server hasn't changed and the local data folder hasn't been touched
/// between runs. Catches the "second run sees Some(j) and trips
/// local_newer" failure mode.
#[tokio::test]
async fn back_to_back_syncs_dont_drift() {
    let server = FakeServer::new(
        vec![ServerFile {
            id: "f-root".into(),
            name: "stable.txt".into(),
            parent_id: None,
            bytes: b"stable\n".to_vec(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        }],
        vec![],
    );
    let upload_hits = server.upload_hits.clone();
    let content_hits = server.content_hits.clone();

    let rig = new_rig(server).await;
    let engine = build_engine(&rig).await;

    let r1 = engine.incremental_sync().await.unwrap();
    assert_eq!(r1.downloaded.len(), 1);
    assert_eq!(r1.uploaded.len(), 0);

    let r2 = engine.incremental_sync().await.unwrap();
    assert_eq!(
        r2.downloaded.len(),
        0,
        "second run must not re-download: {:?}",
        r2.downloaded
    );
    assert_eq!(
        r2.uploaded.len(),
        0,
        "second run must not upload: {:?}",
        r2.uploaded
    );

    assert_eq!(
        upload_hits.load(Ordering::SeqCst),
        0,
        "upload endpoint must not be hit across either run"
    );
    assert_eq!(
        content_hits.load(Ordering::SeqCst),
        0,
        "content-replace endpoint must not be hit across either run"
    );
}

/// Deeper nesting + a file directly at root and inside a sub-sub-folder.
/// Catches path-construction bugs (Phase 5's `local_path_str` vs Phase 7's
/// walkdir result) on any folder level.
#[tokio::test]
async fn nested_folders_dont_push_back() {
    let server = FakeServer::new(
        vec![
            ServerFile {
                id: "f1".into(),
                name: "top.txt".into(),
                parent_id: None,
                bytes: b"top\n".to_vec(),
                updated_at: "2026-01-01T00:00:00Z".into(),
            },
            ServerFile {
                id: "f2".into(),
                name: "deep.txt".into(),
                parent_id: Some("d-c".into()),
                bytes: b"deep\n".to_vec(),
                updated_at: "2026-01-01T00:00:00Z".into(),
            },
        ],
        vec![
            ServerFolder {
                id: "d-a".into(),
                name: "a".into(),
                parent_id: None,
                updated_at: "2026-01-01T00:00:00Z".into(),
            },
            ServerFolder {
                id: "d-b".into(),
                name: "b".into(),
                parent_id: Some("d-a".into()),
                updated_at: "2026-01-01T00:00:00Z".into(),
            },
            ServerFolder {
                id: "d-c".into(),
                name: "c".into(),
                parent_id: Some("d-b".into()),
                updated_at: "2026-01-01T00:00:00Z".into(),
            },
        ],
    );
    let upload_hits = server.upload_hits.clone();

    let rig = new_rig(server).await;
    let engine = build_engine(&rig).await;

    let report = engine.incremental_sync().await.unwrap();

    assert_eq!(report.downloaded.len(), 2);
    assert_eq!(
        report.uploaded.len(),
        0,
        "nested file must not be pushed back: {:?}",
        report.uploaded
    );
    assert_eq!(upload_hits.load(Ordering::SeqCst), 0);

    assert!(rig.root.join("top.txt").is_file());
    assert!(rig.root.join("a/b/c/deep.txt").is_file());
}

/// User-reported reproducer: data folder wiped between syncs while the
/// journal DB persists. After the wipe, the journal has rows pointing to
/// `local_path`s that no longer exist on disk; a re-sync should re-download
/// every file but must NOT push them back as new local files.
#[tokio::test]
async fn stale_journal_with_wiped_local_does_not_push_back() {
    let server = FakeServer::new(
        vec![
            ServerFile {
                id: "f-root".into(),
                name: "hello.txt".into(),
                parent_id: None,
                bytes: b"hello\n".to_vec(),
                updated_at: "2026-01-01T00:00:00Z".into(),
            },
            ServerFile {
                id: "f-nested".into(),
                name: "nested.txt".into(),
                parent_id: Some("dir-docs".into()),
                bytes: b"nested\n".to_vec(),
                updated_at: "2026-01-01T00:00:00Z".into(),
            },
        ],
        vec![ServerFolder {
            id: "dir-docs".into(),
            name: "docs".into(),
            parent_id: None,
            updated_at: "2026-01-01T00:00:00Z".into(),
        }],
    );
    let upload_hits = server.upload_hits.clone();
    let content_hits = server.content_hits.clone();

    let rig = new_rig(server).await;

    // First sync: populates journal + writes files locally.
    let engine = build_engine(&rig).await;
    let r1 = engine.incremental_sync().await.unwrap();
    assert_eq!(r1.downloaded.len(), 2);
    assert_eq!(r1.uploaded.len(), 0);
    drop(engine);

    // Wipe the local data folder while leaving the journal DB intact —
    // exactly the scenario where the bug shows up. The user reset their
    // sync root but kept `~/.local/share/uncloud/sync.db`.
    std::fs::remove_dir_all(&rig.root).unwrap();
    std::fs::create_dir_all(&rig.root).unwrap();

    // Second sync against the same journal. The first run minted a
    // `.uncloud-root.json` sentinel inside the (now-wiped) root, so the
    // sentinel's absence — combined with the journal still holding a
    // `sync_bases` row for this path — is the engine's signal that
    // something's off (volume unmounted, user wiped, journal copied,
    // …). The run aborts rather than guess. The user is expected to
    // explicitly reattach via the desktop UI, which clears the base row
    // so the next sync mints a fresh sentinel and downloads from
    // scratch — but the engine no longer assumes that intent.
    let engine = build_engine(&rig).await;
    let r2 = engine.incremental_sync().await.unwrap();

    assert!(!r2.errors.is_empty(), "sync should have aborted: {:?}", r2);
    assert!(
        r2.errors.iter().any(|e| e.reason.contains("uncloud-root.json")
            || e.reason.contains("Sync root")),
        "expected sentinel error, got {:?}",
        r2.errors
    );
    assert_eq!(r2.uploaded.len(), 0, "no uploads on aborted run");
    assert_eq!(r2.downloaded.len(), 0, "no downloads on aborted run");
    assert_eq!(upload_hits.load(Ordering::SeqCst), 0);
    assert_eq!(content_hits.load(Ordering::SeqCst), 0);
}

/// Directly seed the SQLite journal with a stale row before any sync
/// runs, then start the engine for the first time. This is the "I deleted
/// my data folder but kept ~/.local/share/uncloud-dev/sync.db" case at
/// its sharpest: the engine wakes up cold, has stale metadata in front of
/// it, and must not turn that into a phantom upload.
///
/// We seed three flavours of stale row:
///   1. Same server_id as a current server file, but a `local_path`
///      pointing somewhere else and an old `local_mtime` — the row that
///      the existing fix has to tolerate.
///   2. A row whose server_id is no longer in the server tree at all —
///      Phase 6 must clean it up without trying to upload anything.
///   3. A row whose `local_path` happens to equal where Phase 5 will
///      write a brand-new file (different server_id) — the most
///      adversarial case, where journal lookup by server_id misses but
///      lookup by local_path would (incorrectly) hit.
#[tokio::test]
async fn cold_start_with_stale_journal_record_does_not_push_back() {
    let server = FakeServer::new(
        vec![
            ServerFile {
                id: "current-1".into(),
                name: "hello.txt".into(),
                parent_id: None,
                bytes: b"hello\n".to_vec(),
                updated_at: "2026-04-25T00:00:00Z".into(),
            },
            ServerFile {
                id: "current-2".into(),
                name: "world.txt".into(),
                parent_id: None,
                bytes: b"world\n".to_vec(),
                updated_at: "2026-04-25T00:00:00Z".into(),
            },
        ],
        vec![],
    );
    let upload_hits = server.upload_hits.clone();
    let content_hits = server.content_hits.clone();

    let rig = new_rig(server).await;

    // Run migrations + seed the journal directly. We open our own pool
    // against the same DB file, then close it before the engine connects.
    let url = format!("sqlite://{}?mode=rwc", rig.db_path.display());
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();

    let stale_path_for_current = rig
        .root
        .join("hello.txt")
        .to_string_lossy()
        .into_owned();
    let stale_world_path = rig
        .root
        .join("world.txt")
        .to_string_lossy()
        .into_owned();

    // (1) same server_id as current-1, mismatched local_path + old mtime.
    insert_state_row(
        &pool,
        "current-1",
        "hello.txt",
        "/old/root/from/previous/install/hello.txt",
        Some(1_700_000_000),
        "2025-01-01T00:00:00Z",
    )
    .await;

    // (2) server_id that no longer exists on the server — Phase 6 will
    // visit this row and call `fs.remove_file` on a non-existent path.
    insert_state_row(
        &pool,
        "deleted-server-side",
        "ghost.txt",
        "/old/root/from/previous/install/ghost.txt",
        Some(1_700_000_000),
        "2025-01-01T00:00:00Z",
    )
    .await;

    // (3) a row whose local_path collides with where Phase 5 will write
    // current-2 (different server_id). Phase 7's `already_tracked` lookup
    // would (with a stale snapshot) match the wrong row.
    insert_state_row(
        &pool,
        "ghost-id-overlapping-path",
        "world.txt",
        &stale_world_path,
        Some(1_700_000_000),
        "2025-01-01T00:00:00Z",
    )
    .await;

    // Sanity check: the seeded path-for-current actually equals where the
    // engine will resolve current-1. If this assertion ever fires, the
    // test is meaningless — we'd be testing two different code paths.
    assert_eq!(
        std::path::Path::new(&stale_path_for_current),
        rig.root.join("hello.txt").as_path(),
    );

    pool.close().await;

    // Cold-start engine. The journal is non-empty before the first sync
    // ever runs.
    let engine = build_engine(&rig).await;
    let report = engine.incremental_sync().await.unwrap();

    assert_eq!(
        report.downloaded.len(),
        2,
        "should download both current files: {:?}",
        report.downloaded
    );
    assert_eq!(
        report.uploaded.len(),
        0,
        "must NOT push anything back: {:?}",
        report.uploaded
    );
    assert_eq!(
        report.errors.len(),
        0,
        "no errors expected: {:?}",
        report.errors
    );
    assert_eq!(
        upload_hits.load(Ordering::SeqCst),
        0,
        "upload endpoint must not be hit"
    );
    assert_eq!(
        content_hits.load(Ordering::SeqCst),
        0,
        "content-replace endpoint must not be hit"
    );
}

async fn insert_state_row(
    pool: &sqlx::SqlitePool,
    server_id: &str,
    server_path: &str,
    local_path: &str,
    local_mtime: Option<i64>,
    server_updated_at: &str,
) {
    sqlx::query(
        r#"
        INSERT INTO sync_state
            (server_id, item_type, server_path, local_path, size_bytes, checksum,
             server_updated_at, local_mtime, last_synced_at, sync_status)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(server_id)
    .bind("file")
    .bind(server_path)
    .bind(local_path)
    .bind(0_i64)
    .bind(None::<&str>)
    .bind(server_updated_at)
    .bind(local_mtime)
    .bind(server_updated_at)
    .bind("synced")
    .execute(pool)
    .await
    .unwrap();
}

/// Same shape as the wipe scenario, but the user deletes the local data
/// folder, restarts the desktop app (which means a new `SyncEngine`
/// instance opens against the same SQLite journal), and a sync runs
/// immediately. Catches anything that depends on engine-local in-memory
/// state surviving an instance boundary — the only durable knowledge is
/// what's in the journal.
#[tokio::test]
async fn second_engine_instance_after_wipe_doesnt_push_back() {
    let server = FakeServer::new(
        vec![ServerFile {
            id: "f-root".into(),
            name: "stable.txt".into(),
            parent_id: None,
            bytes: b"stable\n".to_vec(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        }],
        vec![],
    );
    let upload_hits = server.upload_hits.clone();
    let content_hits = server.content_hits.clone();

    let rig = new_rig(server).await;

    // Engine A.
    {
        let engine = build_engine(&rig).await;
        engine.incremental_sync().await.unwrap();
    }

    // Wipe local files but keep the journal DB.
    std::fs::remove_dir_all(&rig.root).unwrap();
    std::fs::create_dir_all(&rig.root).unwrap();

    // Engine B opens the same journal and syncs. Wiping the root took
    // out the sentinel that engine A minted, so engine B sees a journal
    // that knows about a base whose sentinel is gone — the catastrophic-
    // wipe signal — and aborts rather than re-downloading or re-pushing
    // anything. (The user reattaches deliberately via UI.)
    let engine_b = build_engine(&rig).await;
    let report = engine_b.incremental_sync().await.unwrap();

    assert!(
        !report.errors.is_empty(),
        "engine B should have aborted: {:?}",
        report
    );
    assert_eq!(report.downloaded.len(), 0, "no downloads on aborted run");
    assert_eq!(report.uploaded.len(), 0, "no uploads on aborted run");
    assert_eq!(upload_hits.load(Ordering::SeqCst), 0);
    assert_eq!(content_hits.load(Ordering::SeqCst), 0);
}

/// Server returns multiple `tree.files` entries with the same
/// `(parent_id, name)` — a server-side data-integrity violation that the
/// client must defend against. Pre-2026-04-25 the engine downloaded each
/// duplicate over the same local path, then on the next iteration found
/// the file already on disk with a fresh mtime, decided the local copy
/// was newer than the (stale) journal row for the new server_id, and
/// pushed it back as "Updated on server". Now Phase 5 short-circuits any
/// iteration whose `local_path` was already touched in the same run.
#[tokio::test]
async fn duplicate_name_server_files_dont_loop_back_as_uploads() {
    let updated = "2026-04-25T11:59:18.463+00:00".to_string();
    // Three server documents, three different ids, one filename. This is
    // exactly the shape we saw in the production diagnostic: the unique
    // `(owner_id, parent_id, name)` index didn't catch them.
    let server = FakeServer::new(
        vec![
            ServerFile {
                id: "dup-old".into(),
                name: "report.pdf".into(),
                parent_id: None,
                bytes: b"old\n".to_vec(),
                updated_at: updated.clone(),
            },
            ServerFile {
                id: "dup-mid".into(),
                name: "report.pdf".into(),
                parent_id: None,
                bytes: b"mid\n".to_vec(),
                updated_at: updated.clone(),
            },
            ServerFile {
                id: "dup-new".into(),
                name: "report.pdf".into(),
                parent_id: None,
                bytes: b"new\n".to_vec(),
                updated_at: updated.clone(),
            },
        ],
        vec![],
    );
    let upload_hits = server.upload_hits.clone();
    let content_hits = server.content_hits.clone();

    let rig = new_rig(server).await;

    // Pre-seed the journal with stale rows for ALL three server_ids — old
    // mtime (the OS uses seconds, so use a unix epoch a minute earlier).
    let url = format!("sqlite://{}?mode=rwc", rig.db_path.display());
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    let local_path = rig
        .root
        .join("report.pdf")
        .to_string_lossy()
        .into_owned();
    for server_id in &["dup-old", "dup-mid", "dup-new"] {
        insert_state_row(
            &pool,
            server_id,
            "report.pdf",
            &local_path,
            Some(1_700_000_000), // a long time ago
            "2025-01-01T00:00:00Z",
        )
        .await;
    }
    pool.close().await;

    let engine = build_engine(&rig).await;
    let report = engine.incremental_sync().await.unwrap();

    // Exactly one iteration should win and download. The other two are
    // duplicates and must be silently skipped — NOT pushed back as
    // "Updated on server".
    assert_eq!(
        report.downloaded.len(),
        1,
        "exactly one duplicate-name iteration should win and download: {:?}",
        report.downloaded
    );
    assert_eq!(
        report.uploaded.len(),
        0,
        "duplicate-name server files must NOT trip the local-newer arm: {:?}",
        report.uploaded
    );
    assert_eq!(
        upload_hits.load(Ordering::SeqCst),
        0,
        "upload endpoint must not be hit"
    );
    assert_eq!(
        content_hits.load(Ordering::SeqCst),
        0,
        "content-replace endpoint must not be hit"
    );
    assert!(rig.root.join("report.pdf").is_file());
}

/// Two `incremental_sync()` calls fired in parallel must serialize on the
/// engine-level mutex, not race against each other on the journal + local
/// filesystem. With a 200ms artificial delay on each download response, an
/// unguarded engine would have both syncs in flight at the same time and
/// the FakeServer would observe peak in-flight downloads ≥ 2; the lock
/// pins it at 1. Either way, the second run must produce 0 downloads
/// (already done) and 0 uploads.
#[tokio::test]
async fn concurrent_syncs_serialize_via_engine_lock() {
    let server = FakeServer::new(
        vec![
            ServerFile {
                id: "f1".into(),
                name: "a.txt".into(),
                parent_id: None,
                bytes: b"a\n".to_vec(),
                updated_at: "2026-04-25T00:00:00Z".into(),
            },
            ServerFile {
                id: "f2".into(),
                name: "b.txt".into(),
                parent_id: None,
                bytes: b"b\n".to_vec(),
                updated_at: "2026-04-25T00:00:00Z".into(),
            },
            ServerFile {
                id: "f3".into(),
                name: "c.txt".into(),
                parent_id: None,
                bytes: b"c\n".to_vec(),
                updated_at: "2026-04-25T00:00:00Z".into(),
            },
        ],
        vec![],
    );
    let upload_hits = server.upload_hits.clone();
    let content_hits = server.content_hits.clone();
    let download_hits = server.download_hits.clone();
    let peak_in_flight = server.peak_in_flight_downloads.clone();

    // Slow the downloads so the two syncs definitely overlap if nothing
    // serializes them.
    server.set_download_delay_ms(200);

    let rig = new_rig(server).await;
    let engine = build_engine(&rig).await;

    // Fire both syncs against the same engine. `Box<dyn Error>` from the
    // engine isn't `Send`, so we use `tokio::join!` (same-task) rather
    // than `tokio::spawn` — the engine's `tokio::sync::Mutex` is async-
    // aware, so two futures on the same task still serialize against it.
    //
    // The lock guarantees first-come-first-served regardless of which
    // future polls first; the test only cares that *one* of them does the
    // download work and the other sees a clean journal afterward.
    let (r_a, r_b) =
        tokio::join!(engine.incremental_sync(), engine.incremental_sync());
    let r_a = r_a.unwrap();
    let r_b = r_b.unwrap();

    // Whichever sync ran first did the downloads; the other ran after and
    // saw an up-to-date journal.
    let (winner, loser) = if r_a.downloaded.len() >= r_b.downloaded.len() {
        (&r_a, &r_b)
    } else {
        (&r_b, &r_a)
    };
    let r_a = winner;
    let r_b = loser;

    // Sync A is the one that actually downloads.
    assert_eq!(r_a.downloaded.len(), 3, "sync A should download all 3 files");
    assert_eq!(r_a.uploaded.len(), 0, "sync A must not upload");

    // Sync B runs after A, sees the journal populated, and skips
    // everything.
    assert_eq!(
        r_b.downloaded.len(),
        0,
        "sync B should download nothing — A already did it: {:?}",
        r_b.downloaded
    );
    assert_eq!(
        r_b.uploaded.len(),
        0,
        "sync B must not upload anything: {:?}",
        r_b.uploaded
    );

    // The fake server should have served exactly one download per file —
    // total 3, not 6.
    assert_eq!(
        download_hits.load(Ordering::SeqCst),
        3,
        "exactly one download per file should be served"
    );

    // The mutex pins concurrent downloads at 1.
    assert_eq!(
        peak_in_flight.load(Ordering::SeqCst),
        1,
        "peak in-flight downloads should be 1 (serialized); was {}",
        peak_in_flight.load(Ordering::SeqCst)
    );

    // No uploads ever, of course.
    assert_eq!(upload_hits.load(Ordering::SeqCst), 0);
    assert_eq!(content_hits.load(Ordering::SeqCst), 0);
}
