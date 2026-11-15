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
//!
//! Engines here run with a zero confirmation delay unless a test is about
//! the delay itself, so two back-to-back scans are enough to confirm.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Multipart, Path as AxPath, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::{oneshot, Mutex};
use uncloud_client::Client;
use uncloud_common::SyncStrategy;
use uncloud_sync::fs::{LocalFs, LocalFsError, NativeFs, WalkEntry};
use uncloud_sync::SyncEngine;

/// What `FaultyFs` reports as the size of a file flagged `short_size`.
const SHORT_SIZE: u64 = 10;

#[derive(Default)]
struct FaultyFs {
    inner: NativeFs,
    failures: std::sync::RwLock<HashSet<(&'static str, String)>>,
    /// Behave like a backend that cannot move a directory in one step.
    moves_unsupported: AtomicBool,
    copy_calls: AtomicUsize,
    sha256_calls: AtomicUsize,
}

impl FaultyFs {
    fn fail(&self, operation: &'static str, path: &std::path::Path) {
        self.failures
            .write()
            .unwrap()
            .insert((operation, path.to_str().unwrap().to_owned()));
    }

    fn check(&self, operation: &'static str, path: &str) -> Result<(), LocalFsError> {
        if self
            .failures
            .read()
            .unwrap()
            .contains(&(operation, path.to_owned()))
        {
            Err(LocalFsError::io(
                path,
                std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            ))
        } else {
            Ok(())
        }
    }
}

#[async_trait::async_trait]
impl LocalFs for FaultyFs {
    async fn rename(&self, from: &str, to: &str) -> Result<(), LocalFsError> {
        self.check("rename", from)?;
        if self.moves_unsupported.load(Ordering::SeqCst) {
            return Err(LocalFsError::MoveUnsupported("no directory moves".into()));
        }
        self.inner.rename(from, to).await
    }

    async fn sweep_staging(&self, root: &str) -> Result<usize, LocalFsError> {
        self.inner.sweep_staging(root).await
    }

    async fn uncopyable(&self, root: &str) -> Result<Vec<String>, LocalFsError> {
        self.inner.uncopyable(root).await
    }

    async fn size(&self, path: &str) -> Result<Option<u64>, LocalFsError> {
        if self.check("short_size", path).is_err() {
            return Ok(Some(SHORT_SIZE));
        }
        self.inner.size(path).await
    }

    async fn sha256(&self, path: &str) -> Result<String, LocalFsError> {
        self.sha256_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.sha256(path).await
    }

    // Read and write, like a backend that cannot carry the mtime across or
    // replace a file in one step.
    async fn copy_file(&self, from: &str, to: &str) -> Result<(), LocalFsError> {
        self.check("copy_file", from)?;
        self.copy_calls.fetch_add(1, Ordering::SeqCst);
        let data = self.inner.read(from).await?;
        if self.check("partial_copy", from).is_err() {
            self.inner.write(to, &data[..data.len() / 2]).await?;
            return Err(LocalFsError::other("interrupted copy"));
        }
        self.inner.write(to, &data).await?;
        if self.check("edit_source_after_copy", from).is_err() {
            let modified = std::fs::metadata(from).unwrap().modified().unwrap();
            std::fs::write(from, b"edited during copy").unwrap();
            std::fs::File::options()
                .write(true)
                .open(from)
                .unwrap()
                .set_modified(modified)
                .unwrap();
        }
        Ok(())
    }

    async fn walk(&self, root: &str) -> Result<Vec<WalkEntry>, LocalFsError> {
        let mut entries = self.inner.walk(root).await?;
        entries.retain(|entry| {
            self.check("hide_from_walk", &self.inner.join(root, &entry.rel_path))
                .is_ok()
        });
        Ok(entries)
    }

    async fn walk_dirs(&self, root: &str) -> Result<Vec<String>, LocalFsError> {
        let mut entries = self.inner.walk_dirs(root).await?;
        entries.retain(|rel| {
            self.check("hide_from_walk", &self.inner.join(root, rel))
                .is_ok()
        });
        Ok(entries)
    }

    async fn create_dir_all(&self, path: &str) -> Result<(), LocalFsError> {
        self.check("create_dir_all", path)?;
        self.inner.create_dir_all(path).await
    }

    async fn read(&self, path: &str) -> Result<Vec<u8>, LocalFsError> {
        self.inner.read(path).await
    }

    async fn write(&self, path: &str, data: &[u8]) -> Result<(), LocalFsError> {
        if self.check("partial_write", path).is_err() {
            self.inner.write(path, b"incomplete").await?;
            return Err(LocalFsError::other("interrupted write"));
        }
        self.check("write", path)?;
        self.inner.write(path, data).await
    }

    async fn remove_file(&self, path: &str) -> Result<(), LocalFsError> {
        self.check("remove_file", path)?;
        self.inner.remove_file(path).await
    }

    async fn remove_dir(&self, path: &str) -> Result<(), LocalFsError> {
        self.inner.remove_dir(path).await
    }

    async fn mtime(&self, path: &str) -> Result<Option<i64>, LocalFsError> {
        self.inner.mtime(path).await
    }

    async fn is_file(&self, path: &str) -> Result<bool, LocalFsError> {
        self.check("is_file", path)?;
        self.inner.is_file(path).await
    }

    async fn is_dir(&self, path: &str) -> Result<bool, LocalFsError> {
        self.check("is_dir", path)?;
        self.inner.is_dir(path).await
    }

    async fn confirm_absent(&self, path: &str) -> Result<bool, LocalFsError> {
        self.check("confirm_absent", path)?;
        self.check("is_file", path)?;
        self.check("is_dir", path)?;
        self.inner.confirm_absent(path).await
    }

    fn join(&self, parent: &str, child: &str) -> String {
        self.inner.join(parent, child)
    }
}

/// A filesystem that finds an entry under any spelling of its name, as
/// NTFS and Android shared storage do.
#[derive(Default)]
struct CaseFoldFs {
    inner: NativeFs,
    /// Refuse to change only the case of a name, as SAF providers do.
    case_rename_unsupported: AtomicBool,
}

impl CaseFoldFs {
    fn resolve(path: &str) -> String {
        let mut resolved = std::path::PathBuf::new();
        for component in std::path::Path::new(path).components() {
            let candidate = resolved.join(component);
            if resolved.as_os_str().is_empty() || candidate.symlink_metadata().is_ok() {
                resolved = candidate;
                continue;
            }
            let wanted = component.as_os_str().to_string_lossy().to_lowercase();
            let found = std::fs::read_dir(&resolved).ok().and_then(|entries| {
                entries
                    .flatten()
                    .find(|entry| entry.file_name().to_string_lossy().to_lowercase() == wanted)
            });
            resolved = found.map_or(candidate, |entry| entry.path());
        }
        resolved.to_string_lossy().into_owned()
    }
}

#[async_trait::async_trait]
impl LocalFs for CaseFoldFs {
    async fn rename(&self, from: &str, to: &str) -> Result<(), LocalFsError> {
        let source = Self::resolve(from);
        let existing = Self::resolve(to);
        if std::path::Path::new(&existing).exists() && existing != source {
            return Err(LocalFsError::other("rename destination exists"));
        }
        let to = std::path::Path::new(to);
        let parent = Self::resolve(to.parent().unwrap().to_str().unwrap());
        let dest = std::path::Path::new(&parent).join(to.file_name().unwrap());
        if self.case_rename_unsupported.load(Ordering::SeqCst)
            && existing == source
            && dest != std::path::Path::new(&source)
        {
            return Err(LocalFsError::MoveUnsupported(
                "cannot change only the case".into(),
            ));
        }
        std::fs::rename(&source, &dest).map_err(|e| LocalFsError::io(source, e))
    }

    async fn same_entry(&self, a: &str, b: &str) -> Result<bool, LocalFsError> {
        let resolved = Self::resolve(a);
        Ok(a != b && resolved == Self::resolve(b) && std::path::Path::new(&resolved).exists())
    }

    async fn walk(&self, root: &str) -> Result<Vec<WalkEntry>, LocalFsError> {
        self.inner.walk(&Self::resolve(root)).await
    }

    async fn walk_dirs(&self, root: &str) -> Result<Vec<String>, LocalFsError> {
        self.inner.walk_dirs(&Self::resolve(root)).await
    }

    async fn create_dir_all(&self, path: &str) -> Result<(), LocalFsError> {
        self.inner.create_dir_all(&Self::resolve(path)).await
    }

    async fn read(&self, path: &str) -> Result<Vec<u8>, LocalFsError> {
        self.inner.read(&Self::resolve(path)).await
    }

    async fn write(&self, path: &str, data: &[u8]) -> Result<(), LocalFsError> {
        self.inner.write(&Self::resolve(path), data).await
    }

    async fn remove_file(&self, path: &str) -> Result<(), LocalFsError> {
        self.inner.remove_file(&Self::resolve(path)).await
    }

    async fn remove_dir(&self, path: &str) -> Result<(), LocalFsError> {
        self.inner.remove_dir(&Self::resolve(path)).await
    }

    async fn mtime(&self, path: &str) -> Result<Option<i64>, LocalFsError> {
        self.inner.mtime(&Self::resolve(path)).await
    }

    async fn is_file(&self, path: &str) -> Result<bool, LocalFsError> {
        self.inner.is_file(&Self::resolve(path)).await
    }

    async fn is_dir(&self, path: &str) -> Result<bool, LocalFsError> {
        self.inner.is_dir(&Self::resolve(path)).await
    }

    async fn confirm_absent(&self, path: &str) -> Result<bool, LocalFsError> {
        self.inner.confirm_absent(&Self::resolve(path)).await
    }

    fn join(&self, parent: &str, child: &str) -> String {
        self.inner.join(parent, child)
    }
}

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

impl ServerFolder {
    fn root(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: id.to_owned(),
            parent_id: None,
            updated_at: "2026-01-01T00:00:00Z".to_owned(),
        }
    }
}

#[derive(Clone)]
struct FakeServer {
    files: Arc<Mutex<HashMap<String, ServerFile>>>,
    folders: Arc<Mutex<HashMap<String, ServerFolder>>>,
    delete_file_hits: Arc<AtomicUsize>,
    delete_folder_hits: Arc<AtomicUsize>,
    failed_downloads: Arc<Mutex<HashSet<String>>>,
    /// Downloads that start normally and are cut off half-way through.
    cut_off_downloads: Arc<Mutex<HashSet<String>>>,
    download_hits: Arc<AtomicUsize>,
    tree_hits: Arc<AtomicUsize>,
    /// A file that shows up on the server from the given tree fetch on.
    arrives_on_tree_fetch: Arc<Mutex<Option<(usize, ServerFile)>>>,
    /// The file id and declared `Content-Length` of every content update.
    content_updates: Arc<Mutex<Vec<(String, Option<u64>)>>>,
    create_folder_hits: Arc<AtomicUsize>,
}

impl FakeServer {
    fn with(files: Vec<ServerFile>) -> Self {
        Self::with_tree(files, Vec::new())
    }

    fn with_tree(files: Vec<ServerFile>, folder_ids: Vec<&str>) -> Self {
        let folders = folder_ids.into_iter().map(ServerFolder::root).collect();
        Self::with_folders(files, folders)
    }

    fn with_folders(files: Vec<ServerFile>, folders: Vec<ServerFolder>) -> Self {
        let files = files.into_iter().map(|f| (f.id.clone(), f)).collect();
        let folders = folders
            .into_iter()
            .map(|folder| (folder.id.clone(), folder))
            .collect();
        Self {
            files: Arc::new(Mutex::new(files)),
            folders: Arc::new(Mutex::new(folders)),
            delete_file_hits: Arc::new(AtomicUsize::new(0)),
            delete_folder_hits: Arc::new(AtomicUsize::new(0)),
            failed_downloads: Arc::new(Mutex::new(HashSet::new())),
            cut_off_downloads: Arc::new(Mutex::new(HashSet::new())),
            download_hits: Arc::new(AtomicUsize::new(0)),
            tree_hits: Arc::new(AtomicUsize::new(0)),
            arrives_on_tree_fetch: Arc::new(Mutex::new(None)),
            content_updates: Arc::new(Mutex::new(Vec::new())),
            create_folder_hits: Arc::new(AtomicUsize::new(0)),
        }
    }
}

async fn sync_tree(State(s): State<FakeServer>) -> impl IntoResponse {
    let fetch = s.tree_hits.fetch_add(1, Ordering::SeqCst) + 1;
    {
        let mut arrival = s.arrives_on_tree_fetch.lock().await;
        if arrival.as_ref().is_some_and(|(at, _)| fetch >= *at) {
            let (_, file) = arrival.take().unwrap();
            s.files.lock().await.insert(file.id.clone(), file);
        }
    }
    let files_guard = s.files.lock().await;
    let files: Vec<_> = files_guard
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
    drop(files_guard);
    let folders_guard = s.folders.lock().await;
    let folders: Vec<_> = folders_guard
        .values()
        .map(|folder| {
            serde_json::json!({
                "id": folder.id,
                "name": folder.name,
                "parent_id": folder.parent_id,
                "created_at": folder.updated_at,
                "updated_at": folder.updated_at,
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

async fn download(AxPath(id): AxPath<String>, State(s): State<FakeServer>) -> impl IntoResponse {
    s.download_hits.fetch_add(1, Ordering::SeqCst);
    if s.failed_downloads.lock().await.contains(&id) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "temporary download failure",
        )
            .into_response();
    }
    let cut_off = s.cut_off_downloads.lock().await.contains(&id);
    match s.files.lock().await.get(&id) {
        Some(f) if cut_off => {
            let half = f.bytes[..f.bytes.len() / 2].to_vec();
            let body = axum::body::Body::from_stream(futures::stream::iter([
                Ok(half),
                Err(std::io::Error::other("connection lost")),
            ]));
            (StatusCode::OK, body).into_response()
        }
        Some(f) => (StatusCode::OK, f.bytes.clone()).into_response(),
        None => (StatusCode::NOT_FOUND, "").into_response(),
    }
}

async fn delete_file_handler(
    AxPath(id): AxPath<String>,
    State(s): State<FakeServer>,
) -> impl IntoResponse {
    s.delete_file_hits.fetch_add(1, Ordering::SeqCst);
    s.files.lock().await.remove(&id);
    StatusCode::NO_CONTENT
}

async fn delete_folder_handler(
    AxPath(id): AxPath<String>,
    State(s): State<FakeServer>,
) -> impl IntoResponse {
    s.delete_folder_hits.fetch_add(1, Ordering::SeqCst);
    let mut folders = s.folders.lock().await;
    let mut deleted = HashSet::from([id]);
    loop {
        let before = deleted.len();
        for folder in folders.values() {
            if folder
                .parent_id
                .as_ref()
                .is_some_and(|id| deleted.contains(id))
            {
                deleted.insert(folder.id.clone());
            }
        }
        if deleted.len() == before {
            break;
        }
    }
    folders.retain(|id, _| !deleted.contains(id));
    drop(folders);
    s.files.lock().await.retain(|_, file| {
        !file
            .parent_id
            .as_ref()
            .is_some_and(|id| deleted.contains(id))
    });
    StatusCode::NO_CONTENT
}

async fn update_content(
    AxPath(id): AxPath<String>,
    State(s): State<FakeServer>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let length = headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok()?.parse().ok());
    let mut bytes = Vec::new();
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            bytes = field.bytes().await.unwrap().to_vec();
        }
    }
    s.content_updates.lock().await.push((id.clone(), length));
    let mut files = s.files.lock().await;
    let Some(file) = files.get_mut(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    file.bytes = bytes;
    file.updated_at = format!(
        "2027-01-01T00:00:{:02}Z",
        s.content_updates.lock().await.len()
    );
    Json(serde_json::json!({
        "id": file.id,
        "name": file.name,
        "mime_type": "application/octet-stream",
        "size_bytes": file.bytes.len() as i64,
        "parent_id": file.parent_id,
        "created_at": file.updated_at,
        "updated_at": file.updated_at,
    }))
    .into_response()
}

async fn create_folder(
    State(s): State<FakeServer>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let n = s.create_folder_hits.fetch_add(1, Ordering::SeqCst) + 1;
    let folder = ServerFolder {
        id: format!("created-{n}"),
        name: body["name"].as_str().unwrap_or_default().to_owned(),
        parent_id: body["parent_id"].as_str().map(str::to_owned),
        updated_at: "2026-01-01T00:00:00Z".to_owned(),
    };
    s.folders
        .lock()
        .await
        .insert(folder.id.clone(), folder.clone());
    Json(serde_json::json!({
        "id": folder.id,
        "name": folder.name,
        "parent_id": folder.parent_id,
        "created_at": folder.updated_at,
        "updated_at": folder.updated_at,
        "sync_strategy": "inherit",
        "effective_strategy": "two_way",
        "gallery_include": "inherit",
        "effective_gallery_include": "exclude",
        "music_include": "inherit",
        "effective_music_include": "exclude",
        "shared_with_count": 0,
    }))
}

async fn spawn_server(state: FakeServer) -> (String, oneshot::Sender<()>) {
    let app = Router::new()
        .route("/api/sync/tree", get(sync_tree))
        .route("/api/files/{id}/download", get(download))
        .route("/api/files/{id}/content", post(update_content))
        .route("/api/files/{id}", delete(delete_file_handler))
        .route("/api/folders", post(create_folder))
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
    build_engine_with_delay(rig, Duration::ZERO).await
}

async fn build_engine_with_delay(rig: &Rig, delay: Duration) -> SyncEngine {
    SyncEngine::new(
        &rig.db_path,
        rig.client.clone(),
        Some(rig.root.to_string_lossy().into_owned()),
    )
    .await
    .unwrap()
    .with_delete_confirm_delay(delay)
}

async fn build_engine_with_fs(rig: &Rig, fs: Arc<dyn LocalFs>) -> SyncEngine {
    SyncEngine::with_fs(
        &rig.db_path,
        rig.client.clone(),
        fs,
        Some(rig.root.to_string_lossy().into_owned()),
    )
    .await
    .unwrap()
    .with_delete_confirm_delay(Duration::ZERO)
}

async fn backdate_pending(rig: &Rig, id: &str, item_type: &str, since: &str) {
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", rig.db_path.display()))
        .await
        .unwrap();
    sqlx::query(
        "UPDATE sync_state SET delete_pending_since = ? WHERE server_id = ? AND item_type = ?",
    )
    .bind(since)
    .bind(id)
    .bind(item_type)
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;
}

fn server_file(id: &str, name: &str, parent: Option<&str>, bytes: &[u8]) -> ServerFile {
    ServerFile {
        id: id.into(),
        name: name.into(),
        parent_id: parent.map(str::to_owned),
        bytes: bytes.to_vec(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    }
}

fn child_folder(id: &str, parent: &str) -> ServerFolder {
    ServerFolder {
        id: id.into(),
        name: id.into(),
        parent_id: Some(parent.into()),
        updated_at: "2026-01-01T00:00:00Z".into(),
    }
}

async fn journal_status(rig: &Rig, id: &str, item_type: &str) -> (String, Option<String>) {
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", rig.db_path.display()))
        .await
        .unwrap();
    let result = sqlx::query_as(
        "SELECT sync_status, delete_pending_since FROM sync_state WHERE server_id = ? AND item_type = ?",
    )
    .bind(id)
    .bind(item_type)
    .fetch_one(&pool)
    .await
    .unwrap();
    pool.close().await;
    result
}

#[tokio::test]
async fn incoming_folder_rename_handles_restart_failures_and_collisions() {
    for scenario in ["restart", "rename_error", "collision", "unreadable_child"] {
        let server = FakeServer::with_folders(
            vec![server_file("file", "note.txt", Some("child"), b"original")],
            vec![
                ServerFolder::root("parent"),
                child_folder("child", "parent"),
            ],
        );
        let folders = server.folders.clone();
        let deletes = server.delete_file_hits.clone();
        let rig = rig(server).await;
        let fs = Arc::new(FaultyFs::default());
        let engine = build_engine_with_fs(&rig, fs.clone()).await;
        engine.incremental_sync().await.unwrap();
        let old = rig.root.join("parent");
        let new = rig.root.join("renamed");
        {
            let mut folders = folders.lock().await;
            let parent = folders.get_mut("parent").unwrap();
            parent.name = "renamed".into();
            parent.updated_at = "2026-02-01T00:00:00Z".into();
        }
        match scenario {
            "restart" => {
                let pool =
                    sqlx::SqlitePool::connect(&format!("sqlite://{}", rig.db_path.display()))
                        .await
                        .unwrap();
                sqlx::query("INSERT INTO sync_config (key, value) VALUES (?, ?)")
                    .bind("pending_folder_move:parent")
                    .bind(
                        serde_json::to_string(&(old.to_str().unwrap(), new.to_str().unwrap()))
                            .unwrap(),
                    )
                    .execute(&pool)
                    .await
                    .unwrap();
                pool.close().await;
                std::fs::rename(&old, &new).unwrap();
            }
            "rename_error" => fs.fail("rename", &old),
            "collision" => {
                std::fs::create_dir(&new).unwrap();
                std::fs::write(new.join("local.txt"), b"keep local data").unwrap();
            }
            "unreadable_child" => fs.fail("is_dir", &old.join("child")),
            _ => unreachable!(),
        }
        drop(engine);
        let engine = build_engine_with_fs(&rig, fs.clone()).await;
        let report = engine.incremental_sync().await.unwrap();
        if scenario != "restart" {
            assert_eq!(report.errors.len(), 1, "{scenario}: {:?}", report.errors);
            assert!(old.join("child/note.txt").is_file());
            assert!(!new.join("child/note.txt").exists());
            if scenario == "collision" {
                assert_eq!(
                    std::fs::read(new.join("local.txt")).unwrap(),
                    b"keep local data"
                );
                std::fs::remove_dir_all(&new).unwrap();
            }
            fs.failures.write().unwrap().clear();
            assert!(engine.incremental_sync().await.unwrap().errors.is_empty());
        } else {
            assert!(report.errors.is_empty(), "{:?}", report.errors);
        }
        assert_eq!(
            std::fs::read(new.join("child/note.txt")).unwrap(),
            b"original"
        );
        assert!(!old.exists());
        for _ in 0..2 {
            let report = engine.incremental_sync().await.unwrap();
            assert!(report.errors.is_empty(), "{:?}", report.errors);
            assert!(report.uploaded.is_empty());
            assert!(report.created_remote_folders.is_empty());
        }
        assert_eq!(deletes.load(Ordering::SeqCst), 0);
    }
}

fn dir_names(path: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with(".uncloud"))
        .collect();
    names.sort();
    names
}

async fn rename_on_server(server: &FakeServer, id: &str, name: &str, parent: Option<&str>) {
    let mut folders = server.folders.lock().await;
    let folder = folders.get_mut(id).unwrap();
    folder.name = name.into();
    folder.parent_id = parent.map(str::to_owned);
    folder.updated_at = "2026-02-01T00:00:00Z".into();
}

async fn pending_move_intents(rig: &Rig) -> i64 {
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", rig.db_path.display()))
        .await
        .unwrap();
    let count = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sync_config WHERE key LIKE 'pending_folder_move:%'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    pool.close().await;
    count
}

/// A copy-move that stops between copying and removing the originals leaves
/// two copies of every file. An edit made to either one before the next run
/// is carried to the new location and uploaded. Only when both were edited
/// is there nothing to go by, and both are kept.
#[tokio::test]
async fn resumed_copy_carries_an_edit_made_to_either_copy() {
    for edited in ["destination", "source", "both"] {
        let server = FakeServer::with_tree(
            vec![server_file("file", "note.txt", Some("parent"), b"original")],
            vec!["parent"],
        );
        let handle = server.clone();
        let rig = rig(server).await;
        let fs = Arc::new(FaultyFs::default());
        let engine = build_engine_with_fs(&rig, fs.clone()).await;
        assert!(engine.incremental_sync().await.unwrap().errors.is_empty());
        let old = rig.root.join("parent");
        let new = rig.root.join("renamed");
        fs.moves_unsupported.store(true, Ordering::SeqCst);
        fs.fail("remove_file", &old.join("note.txt"));
        rename_on_server(&handle, "parent", "renamed", None).await;
        let failed = engine.incremental_sync().await.unwrap();
        assert_eq!(failed.errors.len(), 1);
        assert_eq!(std::fs::read(new.join("note.txt")).unwrap(), b"original");
        assert_eq!(pending_move_intents(&rig).await, 1);
        drop(engine);

        if edited != "source" {
            std::fs::write(new.join("note.txt"), b"edit at the new location").unwrap();
        }
        if edited != "destination" {
            std::fs::write(old.join("note.txt"), b"edit at the old location").unwrap();
        }
        fs.failures.write().unwrap().clear();
        let engine = build_engine_with_fs(&rig, fs).await;
        if edited == "both" {
            for _ in 0..2 {
                let report = engine.incremental_sync().await.unwrap();
                assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
                assert!(report.errors[0].reason.contains("destination differs"));
                assert!(report.uploaded.is_empty());
                assert!(report.downloaded.is_empty());
                assert!(report.deleted_local.is_empty());
                assert!(report.created_remote_folders.is_empty());
                assert_eq!(
                    std::fs::read(new.join("note.txt")).unwrap(),
                    b"edit at the new location"
                );
                assert_eq!(
                    std::fs::read(old.join("note.txt")).unwrap(),
                    b"edit at the old location"
                );
            }
            continue;
        }
        let expected: &[u8] = if edited == "destination" {
            b"edit at the new location"
        } else {
            b"edit at the old location"
        };
        let report = engine.incremental_sync().await.unwrap();
        assert!(report.errors.is_empty(), "{edited}: {:?}", report.errors);
        assert_eq!(report.uploaded.len(), 1, "{edited}");
        assert_eq!(std::fs::read(new.join("note.txt")).unwrap(), expected);
        assert!(!old.exists());
        assert_eq!(handle.files.lock().await["file"].bytes, expected);
        assert_eq!(pending_move_intents(&rig).await, 0);
        let settled = engine.incremental_sync().await.unwrap();
        assert!(settled.errors.is_empty(), "{:?}", settled.errors);
        assert!(settled.uploaded.is_empty());
        assert!(settled.downloaded.is_empty());
        assert_eq!(handle.delete_file_hits.load(Ordering::SeqCst), 0);
    }
}

/// A copy that fails part-way leaves a truncated destination on a backend
/// that cannot replace a file in one step. What happens to it depends on
/// what can still be known about it:
///   - the copy returned an error: the engine wrote those bytes a moment
///     ago, so it removes them and the next run copies the file again;
///   - the client was killed: after a restart the truncated file is still
///     possibly edited, even if it is a strict prefix of its source;
///   - a differing destination waits for the user, including a deliberately
///     shortened or emptied copy; an identical one can be reused.
#[tokio::test]
async fn interrupted_copy_preserves_ambiguous_destinations_until_resolved() {
    for scenario in [
        "error",
        "killed",
        "edited",
        "truncated",
        "emptied",
        "identical",
    ] {
        let server = FakeServer::with_tree(
            vec![
                server_file("first", "a.txt", Some("parent"), b"first file"),
                server_file("second", "b.txt", Some("parent"), b"second file"),
            ],
            vec!["parent"],
        );
        let handle = server.clone();
        let rig = rig(server).await;
        let fs = Arc::new(FaultyFs::default());
        let engine = build_engine_with_fs(&rig, fs.clone()).await;
        engine.incremental_sync().await.unwrap();
        let downloads = handle.download_hits.load(Ordering::SeqCst);
        let old = rig.root.join("parent");
        let new = rig.root.join("renamed");
        fs.moves_unsupported.store(true, Ordering::SeqCst);
        fs.fail("partial_copy", &old.join("b.txt"));
        if scenario != "error" {
            // A killed client gets no chance to clear what it was writing.
            fs.fail("remove_file", &new.join("b.txt"));
        }
        rename_on_server(&handle, "parent", "renamed", None).await;

        let report = engine.incremental_sync().await.unwrap();
        assert_eq!(report.errors.len(), 1, "{scenario}: {:?}", report.errors);
        assert_eq!(std::fs::read(old.join("b.txt")).unwrap(), b"second file");
        if scenario == "error" {
            assert!(!new.join("b.txt").exists());
        } else {
            assert_eq!(std::fs::read(new.join("b.txt")).unwrap(), b"secon");
        }
        assert_eq!(pending_move_intents(&rig).await, 1);
        drop(engine);

        let expected: &[u8] = match scenario {
            "edited" => b"second thoughts",
            "truncated" => b"sec",
            "emptied" => b"",
            "identical" => b"second file",
            _ => b"secon",
        };
        if !matches!(scenario, "error" | "killed") {
            std::fs::write(new.join("b.txt"), expected).unwrap();
        }
        fs.failures.write().unwrap().clear();
        let engine = build_engine_with_fs(&rig, fs).await;
        if !matches!(scenario, "error" | "identical") {
            for _ in 0..2 {
                let report = engine.incremental_sync().await.unwrap();
                assert_eq!(report.errors.len(), 1, "{report:?}");
                assert!(report.errors[0].reason.contains("manual resolution"));
                assert!(report.uploaded.is_empty());
                assert_eq!(std::fs::read(new.join("b.txt")).unwrap(), expected);
                assert_eq!(std::fs::read(old.join("b.txt")).unwrap(), b"second file");
                assert_eq!(pending_move_intents(&rig).await, 1);
            }
            std::fs::remove_file(new.join("b.txt")).unwrap();
        }
        for _ in 0..3 {
            let report = engine.incremental_sync().await.unwrap();
            assert!(report.errors.is_empty(), "{scenario}: {:?}", report.errors);
            assert!(report.uploaded.is_empty());
        }
        assert_eq!(std::fs::read(new.join("a.txt")).unwrap(), b"first file");
        assert_eq!(std::fs::read(new.join("b.txt")).unwrap(), b"second file");
        assert!(!old.exists());
        assert_eq!(handle.download_hits.load(Ordering::SeqCst), downloads);
        assert_eq!(handle.delete_file_hits.load(Ordering::SeqCst), 0);
        assert!(handle.content_updates.lock().await.is_empty());
        assert_eq!(pending_move_intents(&rig).await, 0);
    }
}

#[tokio::test]
async fn inflight_destination_edits_survive_recovery() {
    for original_changed in [false, true] {
        let server = FakeServer::with_tree(
            vec![server_file("file", "note.txt", Some("parent"), b"original")],
            vec!["parent"],
        );
        let handle = server.clone();
        let rig = rig(server).await;
        let fs = Arc::new(FaultyFs::default());
        let engine = build_engine_with_fs(&rig, fs.clone()).await;
        engine.incremental_sync().await.unwrap();
        let source = rig.root.join("parent/note.txt");
        let dest = rig.root.join("renamed/note.txt");
        fs.moves_unsupported.store(true, Ordering::SeqCst);
        fs.fail("partial_copy", &source);
        rename_on_server(&handle, "parent", "renamed", None).await;
        assert_eq!(engine.incremental_sync().await.unwrap().errors.len(), 1);
        drop(engine);
        std::fs::write(&dest, b"user edit after the interruption").unwrap();
        if original_changed {
            std::fs::write(&source, b"source edit").unwrap();
        }
        let expected_source = std::fs::read(&source).unwrap();
        fs.failures.write().unwrap().clear();
        let engine = build_engine_with_fs(&rig, fs).await;
        for _ in 0..2 {
            let report = engine.incremental_sync().await.unwrap();
            assert_eq!(report.errors.len(), 1, "{report:?}");
            assert!(report.errors[0].reason.contains("manual resolution"));
            assert!(report.uploaded.is_empty());
            assert!(report.downloaded.is_empty());
            assert!(report.deleted_local.is_empty());
            assert!(report.created_remote_folders.is_empty());
            assert_eq!(
                std::fs::read(&dest).unwrap(),
                b"user edit after the interruption"
            );
            assert_eq!(std::fs::read(&source).unwrap(), expected_source);
            assert_eq!(pending_move_intents(&rig).await, 1);
        }
        assert_eq!(handle.files.lock().await["file"].bytes, b"original");
    }
}

#[tokio::test]
async fn unresolved_inflight_copy_is_checked_before_starting_another_copy() {
    let server = FakeServer::with_tree(
        vec![
            server_file("first", "a.txt", Some("parent"), b"first file"),
            server_file("second", "b.txt", Some("parent"), b"second file"),
        ],
        vec!["parent"],
    );
    let handle = server.clone();
    let rig = rig(server).await;
    let fs = Arc::new(FaultyFs::default());
    let engine = build_engine_with_fs(&rig, fs.clone()).await;
    engine.incremental_sync().await.unwrap();
    let old = rig.root.join("parent");
    let new = rig.root.join("renamed");
    let files = NativeFs.walk(old.to_str().unwrap()).await.unwrap();
    let earlier = &files[0].rel_path;
    let unfinished = &files[1].rel_path;
    fs.moves_unsupported.store(true, Ordering::SeqCst);
    fs.fail("partial_copy", &old.join(unfinished));
    rename_on_server(&handle, "parent", "renamed", None).await;
    assert_eq!(engine.incremental_sync().await.unwrap().errors.len(), 1);
    drop(engine);
    let copies = fs.copy_calls.load(Ordering::SeqCst);
    let before = b"edited after the interruption".to_vec();
    std::fs::write(new.join(unfinished), &before).unwrap();
    std::fs::write(old.join(earlier), b"source edit").unwrap();
    fs.failures.write().unwrap().clear();
    fs.fail("partial_copy", &old.join(earlier));
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", rig.db_path.display()))
        .await
        .unwrap();
    let intent: String = sqlx::query_scalar(
        "SELECT value FROM sync_config WHERE key = 'pending_folder_move:parent'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let engine = build_engine_with_fs(&rig, fs.clone()).await;
    for _ in 0..2 {
        let report = engine.incremental_sync().await.unwrap();
        assert_eq!(report.errors.len(), 1, "{report:?}");
        assert!(report.errors[0].reason.contains("manual resolution"));
        assert_eq!(fs.copy_calls.load(Ordering::SeqCst), copies);
        assert_eq!(std::fs::read(new.join(unfinished)).unwrap(), before);
        let current: String = sqlx::query_scalar(
            "SELECT value FROM sync_config WHERE key = 'pending_folder_move:parent'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(current, intent);
    }
    pool.close().await;
}

#[tokio::test]
async fn cached_checksums_do_not_authorize_overwriting_destination_edits() {
    let server = FakeServer::with_tree(
        vec![server_file("file", "note.txt", Some("parent"), b"original")],
        vec!["parent"],
    );
    let handle = server.clone();
    let rig = rig(server).await;
    let fs = Arc::new(FaultyFs::default());
    let engine = build_engine_with_fs(&rig, fs.clone()).await;
    engine.incremental_sync().await.unwrap();
    let source = rig.root.join("parent/note.txt");
    let dest = rig.root.join("renamed/note.txt");
    fs.moves_unsupported.store(true, Ordering::SeqCst);
    fs.fail("remove_file", &source);
    rename_on_server(&handle, "parent", "renamed", None).await;
    assert_eq!(engine.incremental_sync().await.unwrap().errors.len(), 1);
    // Before the fix this second attempt retained both checksums across runs.
    assert_eq!(engine.incremental_sync().await.unwrap().errors.len(), 1);
    let dest_mtime = std::fs::metadata(&dest).unwrap().modified().unwrap();
    std::fs::write(&dest, b"destination user edit").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&dest)
        .unwrap()
        .set_modified(dest_mtime)
        .unwrap();
    std::fs::write(&source, b"source user edit").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&source)
        .unwrap()
        .set_modified(std::time::SystemTime::now() + Duration::from_secs(60))
        .unwrap();
    fs.failures.write().unwrap().clear();
    for _ in 0..2 {
        let report = engine.incremental_sync().await.unwrap();
        assert_eq!(report.errors.len(), 1, "{report:?}");
        assert!(report.errors[0].reason.contains("destination differs"));
        assert!(report.uploaded.is_empty());
        assert_eq!(std::fs::read(&dest).unwrap(), b"destination user edit");
        assert_eq!(std::fs::read(&source).unwrap(), b"source user edit");
    }
    assert_eq!(handle.files.lock().await["file"].bytes, b"original");
}

#[tokio::test]
async fn interrupted_move_commit_preserves_paths_mtimes_and_intent() {
    for edited in ["none", "source", "destination"] {
        let server = FakeServer::with_tree(
            vec![server_file("file", "note.txt", Some("parent"), b"original")],
            vec!["parent"],
        );
        let handle = server.clone();
        let rig = rig(server).await;
        let fs = Arc::new(FaultyFs::default());
        let engine = build_engine_with_fs(&rig, fs.clone()).await;
        engine.incremental_sync().await.unwrap();
        let source = rig.root.join("parent/note.txt");
        let dest = rig.root.join("renamed/note.txt");
        let original_time = std::time::SystemTime::now() + Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&source)
            .unwrap()
            .set_modified(original_time)
            .unwrap();
        assert_eq!(engine.incremental_sync().await.unwrap().uploaded.len(), 1);
        fs.moves_unsupported.store(true, Ordering::SeqCst);
        fs.fail("remove_file", &source);
        rename_on_server(&handle, "parent", "renamed", None).await;
        assert_eq!(engine.incremental_sync().await.unwrap().errors.len(), 1);
        drop(engine);
        let expected: &[u8] = if edited == "none" {
            b"original"
        } else {
            b"edited content"
        };
        if edited != "none" {
            let path = if edited == "source" { &source } else { &dest };
            std::fs::write(path, expected).unwrap();
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(original_time)
                .unwrap();
        }
        fs.failures.write().unwrap().clear();
        let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", rig.db_path.display()))
            .await
            .unwrap();
        sqlx::query("CREATE TRIGGER fail_mtime BEFORE UPDATE OF local_mtime ON sync_state BEGIN SELECT RAISE(ABORT, 'injected interruption'); END")
            .execute(&pool).await.unwrap();
        let engine = build_engine_with_fs(&rig, fs.clone()).await;
        assert!(engine.incremental_sync().await.is_err(), "{edited}");
        assert!(!source.exists());
        assert_eq!(std::fs::read(&dest).unwrap(), expected);
        assert_eq!(pending_move_intents(&rig).await, 1);
        let recorded_path: String =
            sqlx::query_scalar("SELECT local_path FROM sync_state WHERE server_id = 'file'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(recorded_path, source.to_str().unwrap());
        sqlx::query("DROP TRIGGER fail_mtime")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
        drop(engine);
        let engine = build_engine_with_fs(&rig, fs).await;
        let report = engine.incremental_sync().await.unwrap();
        assert!(report.errors.is_empty(), "{edited}: {report:?}");
        assert_eq!(
            report.uploaded.len(),
            usize::from(edited != "none"),
            "{edited}"
        );
        assert_eq!(handle.files.lock().await["file"].bytes, expected);
        assert_eq!(pending_move_intents(&rig).await, 0);
        let report = engine.incremental_sync().await.unwrap();
        assert!(report.errors.is_empty(), "{report:?}");
        assert!(report.uploaded.is_empty());
        assert!(report.downloaded.is_empty());
    }
}

/// An interrupted copy leaves the old directory complete and the new one
/// partial. Clearing away the old one as a leftover must not turn the files
/// that never made it across into deletions on the server: they are
/// downloaded again instead.
#[tokio::test]
async fn files_an_interrupted_copy_never_carried_are_restored_not_deleted() {
    for removed in ["old directory", "old contents", "both directories"] {
        let contents = |i: usize| format!("content {i}").into_bytes();
        let files = (0..4)
            .map(|i| {
                server_file(
                    &format!("f{i}"),
                    &format!("f{i}.txt"),
                    Some("parent"),
                    &contents(i),
                )
            })
            .collect();
        let server = FakeServer::with_folders(
            files,
            vec![ServerFolder::root("parent"), child_folder("sub", "parent")],
        );
        let handle = server.clone();
        let rig = rig(server).await;
        let fs = Arc::new(FaultyFs::default());
        let engine = build_engine_with_fs(&rig, fs.clone()).await;
        engine.incremental_sync().await.unwrap();
        let old = rig.root.join("parent");
        let new = rig.root.join("renamed");
        let listed = NativeFs.walk(old.to_str().unwrap()).await.unwrap();
        let cut_off = &listed[1].rel_path;
        fs.moves_unsupported.store(true, Ordering::SeqCst);
        fs.fail("partial_copy", &old.join(cut_off));
        fs.fail("remove_file", &new.join(cut_off));
        rename_on_server(&handle, "parent", "renamed", None).await;
        assert_eq!(engine.incremental_sync().await.unwrap().errors.len(), 1);
        assert_eq!(
            dir_names(&new).len(),
            3,
            "one copy, one cut off, one folder"
        );
        drop(engine);

        fs.failures.write().unwrap().clear();
        match removed {
            "old directory" => std::fs::remove_dir_all(&old).unwrap(),
            "old contents" => {
                for file in &listed {
                    std::fs::remove_file(old.join(&file.rel_path)).unwrap();
                }
            }
            _ => {
                std::fs::remove_dir_all(&old).unwrap();
                std::fs::remove_dir_all(&new).unwrap();
            }
        }
        let engine = build_engine_with_fs(&rig, fs).await;
        if removed != "both directories" {
            // The cut-off copy has no original left to be checked against.
            for _ in 0..2 {
                let report = engine.incremental_sync().await.unwrap();
                assert_eq!(report.errors.len(), 1, "{removed}: {report:?}");
                assert!(report.errors[0].reason.contains("manual resolution"));
            }
            std::fs::remove_file(new.join(cut_off)).unwrap();
        }
        for _ in 0..3 {
            let report = engine.incremental_sync().await.unwrap();
            assert!(report.errors.is_empty(), "{removed}: {report:?}");
            assert!(report.uploaded.is_empty());
        }
        for i in 0..4 {
            assert_eq!(
                std::fs::read(new.join(format!("f{i}.txt"))).unwrap(),
                contents(i),
                "{removed}"
            );
        }
        assert!(new.join("sub").is_dir(), "{removed}");
        assert!(!old.exists(), "{removed}");
        assert_eq!(handle.files.lock().await.len(), 4, "{removed}");
        assert_eq!(handle.folders.lock().await.len(), 2, "{removed}");
        assert_eq!(handle.delete_file_hits.load(Ordering::SeqCst), 0);
        assert_eq!(handle.delete_folder_hits.load(Ordering::SeqCst), 0);
        assert_eq!(handle.create_folder_hits.load(Ordering::SeqCst), 0);
        assert_eq!(pending_move_intents(&rig).await, 0);
    }
}

#[tokio::test]
async fn missing_copy_rows_remain_recoverable_after_failed_completion() {
    for destination_exists in [false, true] {
        let server = FakeServer::with_folders(
            vec![server_file("file", "note.txt", Some("child"), b"original")],
            vec![
                ServerFolder::root("parent"),
                child_folder("child", "parent"),
            ],
        );
        let handle = server.clone();
        let rig = rig(server).await;
        let engine = build_engine(&rig).await;
        engine.incremental_sync().await.unwrap();
        drop(engine);
        let old = rig.root.join("parent");
        let new = rig.root.join("renamed");
        std::fs::remove_dir_all(&old).unwrap();
        if destination_exists {
            std::fs::create_dir(&new).unwrap();
        }
        rename_on_server(&handle, "parent", "renamed", None).await;
        let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", rig.db_path.display()))
            .await
            .unwrap();
        sqlx::query("INSERT INTO sync_config (key, value) VALUES (?, ?)")
            .bind("pending_folder_move:parent")
            .bind(
                serde_json::to_string(&(old.to_str().unwrap(), new.to_str().unwrap(), true))
                    .unwrap(),
            )
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE TRIGGER fail_completion BEFORE DELETE ON sync_config WHEN OLD.key = 'pending_folder_move:parent' BEGIN SELECT RAISE(ABORT, 'interrupted completion'); END")
            .execute(&pool).await.unwrap();
        let engine = build_engine(&rig).await;
        assert!(engine.incremental_sync().await.is_err());
        drop(engine);
        let recorded: Vec<(String, String)> =
            sqlx::query_as("SELECT server_id, local_path FROM sync_state")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(recorded.len(), 3);
        for (id, path) in recorded {
            let expected = match id.as_str() {
                "parent" => old.clone(),
                "child" => old.join("child"),
                "file" => old.join("child/note.txt"),
                _ => panic!("unexpected row {id}"),
            };
            assert_eq!(path, expected.to_str().unwrap());
        }
        assert_eq!(pending_move_intents(&rig).await, 1);
        sqlx::query("DROP TRIGGER fail_completion")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
        let engine = build_engine(&rig).await;
        for _ in 0..3 {
            let report = engine.incremental_sync().await.unwrap();
            assert!(report.errors.is_empty(), "{report:?}");
            assert!(report.uploaded.is_empty());
        }
        assert_eq!(
            std::fs::read(new.join("child/note.txt")).unwrap(),
            b"original"
        );
        assert_eq!(handle.delete_file_hits.load(Ordering::SeqCst), 0);
        assert_eq!(handle.delete_folder_hits.load(Ordering::SeqCst), 0);
        assert_eq!(handle.files.lock().await.len(), 1);
        assert_eq!(handle.folders.lock().await.len(), 2);
        assert_eq!(pending_move_intents(&rig).await, 0);
    }
}

#[tokio::test]
async fn copy_recovery_requires_confirmed_absence_for_unlisted_entries() {
    for hidden in ["file", "folder", "unconfirmed absence"] {
        let server = FakeServer::with_folders(
            vec![server_file("file", "note.txt", Some("child"), b"original")],
            vec![
                ServerFolder::root("parent"),
                child_folder("child", "parent"),
            ],
        );
        let handle = server.clone();
        let rig = rig(server).await;
        let fs = Arc::new(FaultyFs::default());
        let engine = build_engine_with_fs(&rig, fs.clone()).await;
        engine.incremental_sync().await.unwrap();
        let old = rig.root.join("parent");
        let new = rig.root.join("renamed");
        let dest = new.join("child/note.txt");
        fs.moves_unsupported.store(true, Ordering::SeqCst);
        fs.fail("remove_file", &old.join("child/note.txt"));
        rename_on_server(&handle, "parent", "renamed", None).await;
        assert_eq!(engine.incremental_sync().await.unwrap().errors.len(), 1);
        drop(engine);
        std::fs::remove_dir_all(&old).unwrap();
        std::fs::write(&dest, b"unique destination edit").unwrap();
        fs.failures.write().unwrap().clear();
        match hidden {
            "file" => fs.fail("hide_from_walk", &dest),
            "folder" => fs.fail("hide_from_walk", &new.join("child")),
            _ => {
                std::fs::remove_file(&dest).unwrap();
                fs.fail("confirm_absent", &dest);
            }
        }
        let engine = build_engine_with_fs(&rig, fs.clone()).await;
        for _ in 0..2 {
            let report = engine.incremental_sync().await.unwrap();
            assert_eq!(report.errors.len(), 1, "{hidden}: {report:?}");
            assert!(report.uploaded.is_empty());
            assert!(report.downloaded.is_empty());
            assert_eq!(pending_move_intents(&rig).await, 1);
            if hidden != "unconfirmed absence" {
                assert_eq!(std::fs::read(&dest).unwrap(), b"unique destination edit");
            }
        }
        fs.failures.write().unwrap().clear();
        for _ in 0..3 {
            let report = engine.incremental_sync().await.unwrap();
            assert!(report.errors.is_empty(), "{hidden}: {report:?}");
        }
        let expected: &[u8] = if hidden == "unconfirmed absence" {
            b"original"
        } else {
            b"unique destination edit"
        };
        assert_eq!(std::fs::read(&dest).unwrap(), expected);
        assert_eq!(handle.files.lock().await["file"].bytes, expected);
        assert_eq!(handle.delete_file_hits.load(Ordering::SeqCst), 0);
        assert_eq!(handle.delete_folder_hits.load(Ordering::SeqCst), 0);
        assert_eq!(pending_move_intents(&rig).await, 0);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn copy_recovery_preserves_edits_in_a_directory_that_cannot_be_listed() {
    use std::os::unix::fs::PermissionsExt;
    let server = FakeServer::with_tree(
        vec![server_file("file", "note.txt", Some("parent"), b"original")],
        vec!["parent"],
    );
    let handle = server.clone();
    let rig = rig(server).await;
    let fs = Arc::new(FaultyFs::default());
    let engine = build_engine_with_fs(&rig, fs.clone()).await;
    engine.incremental_sync().await.unwrap();
    let old = rig.root.join("parent");
    let new = rig.root.join("renamed");
    let dest = new.join("note.txt");
    fs.moves_unsupported.store(true, Ordering::SeqCst);
    fs.fail("remove_file", &old.join("note.txt"));
    rename_on_server(&handle, "parent", "renamed", None).await;
    assert_eq!(engine.incremental_sync().await.unwrap().errors.len(), 1);
    drop(engine);
    std::fs::remove_dir_all(&old).unwrap();
    std::fs::write(&dest, b"unique destination edit").unwrap();
    let permissions = std::fs::metadata(&new).unwrap().permissions();
    std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o333)).unwrap();
    if std::fs::read_dir(&new).is_ok() {
        // Root can enumerate despite the mode bits.
        std::fs::set_permissions(&new, permissions).unwrap();
        return;
    }
    let engine = build_engine(&rig).await;
    let report = engine.incremental_sync().await;
    std::fs::set_permissions(&new, permissions).unwrap();
    let report = report.unwrap();
    assert_eq!(report.errors.len(), 1, "{report:?}");
    assert!(report.errors[0]
        .reason
        .contains("exists but could not be listed"));
    assert!(report.uploaded.is_empty());
    assert!(report.downloaded.is_empty());
    assert_eq!(std::fs::read(&dest).unwrap(), b"unique destination edit");
    assert_eq!(pending_move_intents(&rig).await, 1);
    let report = engine.incremental_sync().await.unwrap();
    assert!(report.errors.is_empty(), "{report:?}");
    assert_eq!(report.uploaded.len(), 1);
    assert_eq!(
        handle.files.lock().await["file"].bytes,
        b"unique destination edit"
    );
    assert_eq!(handle.delete_file_hits.load(Ordering::SeqCst), 0);
    assert_eq!(pending_move_intents(&rig).await, 0);
}

/// The file an interrupted run was writing is not held back when its
/// destination still holds exactly what was last synced: nothing there can
/// be an edit. A staged copy killed before its rename leaves that, and so
/// does a copy of an appended source cut off at the old length.
#[tokio::test]
async fn interrupted_recopy_over_a_stale_destination_is_redone() {
    for scenario in ["staged", "cut off at the old length"] {
        let server = FakeServer::with_tree(
            vec![server_file("file", "note.txt", Some("parent"), b"original")],
            vec!["parent"],
        );
        let handle = server.clone();
        let rig = rig(server).await;
        let fs = Arc::new(FaultyFs::default());
        let engine = build_engine_with_fs(&rig, fs.clone()).await;
        engine.incremental_sync().await.unwrap();
        let source = rig.root.join("parent/note.txt");
        let dest = rig.root.join("renamed/note.txt");
        fs.moves_unsupported.store(true, Ordering::SeqCst);
        fs.fail("remove_file", &source);
        rename_on_server(&handle, "parent", "renamed", None).await;
        assert_eq!(engine.incremental_sync().await.unwrap().errors.len(), 1);
        drop(engine);
        assert_eq!(std::fs::read(&dest).unwrap(), b"original");

        // Twice the synced length, so half a copy is the synced content.
        std::fs::write(&source, b"original, longer").unwrap();
        fs.failures.write().unwrap().clear();
        let interruption = if scenario == "staged" {
            "copy_file"
        } else {
            "partial_copy"
        };
        fs.fail(interruption, &source);
        // A killed client gets no chance to clear what it was writing.
        fs.fail("remove_file", &dest);
        let engine = build_engine_with_fs(&rig, fs.clone()).await;
        assert_eq!(engine.incremental_sync().await.unwrap().errors.len(), 1);
        drop(engine);
        assert_eq!(std::fs::read(&dest).unwrap(), b"original", "{scenario}");
        let intent: String = {
            let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", rig.db_path.display()))
                .await
                .unwrap();
            let value = sqlx::query_scalar(
                "SELECT value FROM sync_config WHERE key = 'pending_folder_move:parent'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            pool.close().await;
            value
        };
        assert!(intent.contains("note.txt"), "{scenario}: {intent}");

        fs.failures.write().unwrap().clear();
        let engine = build_engine_with_fs(&rig, fs).await;
        for _ in 0..3 {
            let report = engine.incremental_sync().await.unwrap();
            assert!(report.errors.is_empty(), "{scenario}: {report:?}");
        }
        assert_eq!(std::fs::read(&dest).unwrap(), b"original, longer");
        assert!(!rig.root.join("parent").exists());
        assert_eq!(handle.files.lock().await["file"].bytes, b"original, longer");
        assert_eq!(handle.content_updates.lock().await.len(), 1);
        assert_eq!(handle.delete_file_hits.load(Ordering::SeqCst), 0);
        assert_eq!(pending_move_intents(&rig).await, 0);
    }
}

/// A move waiting for the user is not read again on every run: the verdict
/// stands until one of the two files changes.
#[tokio::test]
async fn stuck_copy_move_is_not_reread_until_something_changes() {
    for stuck_on in ["both edited", "edited interrupted copy"] {
        let files = (0..6)
            .map(|i| {
                server_file(
                    &format!("f{i}"),
                    &format!("f{i}.txt"),
                    Some("parent"),
                    format!("content {i}").as_bytes(),
                )
            })
            .collect();
        let server = FakeServer::with_tree(files, vec!["parent"]);
        let handle = server.clone();
        let rig = rig(server).await;
        let fs = Arc::new(FaultyFs::default());
        let engine = build_engine_with_fs(&rig, fs.clone()).await;
        engine.incremental_sync().await.unwrap();
        let old = rig.root.join("parent");
        let new = rig.root.join("renamed");
        let listed = NativeFs.walk(old.to_str().unwrap()).await.unwrap();
        let last = &listed.last().unwrap().rel_path;
        fs.moves_unsupported.store(true, Ordering::SeqCst);
        if stuck_on == "both edited" {
            fs.fail("remove_file", &old.join(&listed[0].rel_path));
        } else {
            fs.fail("partial_copy", &old.join(last));
        }
        rename_on_server(&handle, "parent", "renamed", None).await;
        assert_eq!(engine.incremental_sync().await.unwrap().errors.len(), 1);
        fs.failures.write().unwrap().clear();
        std::fs::write(new.join(last), b"edited at the new location").unwrap();
        if stuck_on == "both edited" {
            std::fs::write(old.join(last), b"edited at the old location").unwrap();
        }

        let stuck_run = || async {
            fs.sha256_calls.store(0, Ordering::SeqCst);
            let report = engine.incremental_sync().await.unwrap();
            assert_eq!(report.errors.len(), 1, "{stuck_on}: {report:?}");
            assert!(report.errors[0].reason.contains("manual resolution"));
            (
                report.errors[0].reason.clone(),
                fs.sha256_calls.load(Ordering::SeqCst),
            )
        };
        let (first_reason, first_reads) = stuck_run().await;
        assert!(first_reads > 0);
        for _ in 0..3 {
            assert_eq!(stuck_run().await, (first_reason.clone(), 0));
        }

        // Any change to either file has the verdict worked out afresh.
        std::fs::File::options()
            .write(true)
            .open(new.join(last))
            .unwrap()
            .set_modified(std::time::SystemTime::now() + Duration::from_secs(90))
            .unwrap();
        assert!(stuck_run().await.1 > 0);
        assert_eq!(stuck_run().await.1, 0);

        std::fs::remove_file(new.join(last)).unwrap();
        let report = engine.incremental_sync().await.unwrap();
        assert!(report.errors.is_empty(), "{stuck_on}: {report:?}");
        assert!(!old.exists());
        assert_eq!(handle.delete_file_hits.load(Ordering::SeqCst), 0);
    }
}

/// A copy-move whose files all arrived but whose journal update was lost
/// is completed from what is at the destination. A file with no checksum on
/// record cannot be compared, which is no reason to upload it again.
#[tokio::test]
async fn recovered_move_does_not_reupload_files_without_a_recorded_checksum() {
    let server = FakeServer::with_tree(
        vec![server_file("file", "note.txt", Some("parent"), b"original")],
        vec!["parent"],
    );
    let handle = server.clone();
    let rig = rig(server).await;
    let fs = Arc::new(FaultyFs::default());
    let engine = build_engine_with_fs(&rig, fs.clone()).await;
    engine.incremental_sync().await.unwrap();
    drop(engine);
    let source = rig.root.join("parent/note.txt");
    let dest = rig.root.join("renamed/note.txt");
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", rig.db_path.display()))
        .await
        .unwrap();
    sqlx::query("UPDATE sync_state SET checksum = NULL")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER fail_paths BEFORE UPDATE OF local_path ON sync_state BEGIN SELECT RAISE(ABORT, 'injected interruption'); END")
        .execute(&pool)
        .await
        .unwrap();
    fs.moves_unsupported.store(true, Ordering::SeqCst);
    rename_on_server(&handle, "parent", "renamed", None).await;
    let engine = build_engine_with_fs(&rig, fs.clone()).await;
    assert!(engine.incremental_sync().await.is_err());
    assert!(!source.exists());
    drop(engine);
    // As on a backend that carries the mtime across a copy.
    let recorded: i64 =
        sqlx::query_scalar("SELECT local_mtime FROM sync_state WHERE server_id = 'file'")
            .fetch_one(&pool)
            .await
            .unwrap();
    std::fs::File::options()
        .write(true)
        .open(&dest)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(recorded as u64))
        .unwrap();
    sqlx::query("DROP TRIGGER fail_paths")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;

    let engine = build_engine_with_fs(&rig, fs).await;
    for _ in 0..2 {
        let report = engine.incremental_sync().await.unwrap();
        assert!(report.errors.is_empty(), "{report:?}");
        assert!(report.uploaded.is_empty());
        assert!(report.downloaded.is_empty());
    }
    assert_eq!(std::fs::read(&dest).unwrap(), b"original");
    assert!(handle.content_updates.lock().await.is_empty());
    assert_eq!(pending_move_intents(&rig).await, 0);
}

/// Moving by copy reads each file once to copy it and once more on each
/// side to compare them before the original goes.
#[tokio::test]
async fn copy_move_hashes_each_side_of_a_file_once() {
    let server = FakeServer::with_tree(
        vec![
            server_file("first", "a.txt", Some("parent"), b"first file"),
            server_file("second", "b.txt", Some("parent"), b"second file"),
        ],
        vec!["parent"],
    );
    let handle = server.clone();
    let rig = rig(server).await;
    let fs = Arc::new(FaultyFs::default());
    let engine = build_engine_with_fs(&rig, fs.clone()).await;
    engine.incremental_sync().await.unwrap();
    fs.moves_unsupported.store(true, Ordering::SeqCst);
    fs.sha256_calls.store(0, Ordering::SeqCst);
    rename_on_server(&handle, "parent", "renamed", None).await;
    let report = engine.incremental_sync().await.unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(rig.root.join("renamed/b.txt").is_file());
    assert_eq!(fs.copy_calls.load(Ordering::SeqCst), 2);
    assert_eq!(fs.sha256_calls.load(Ordering::SeqCst), 4);
}

/// A folder that holds something a copy cannot carry is not moved by
/// copying at all, instead of being split between the old and new name.
#[cfg(unix)]
#[tokio::test]
async fn copy_move_refuses_a_folder_holding_something_it_cannot_carry() {
    let server = FakeServer::with_tree(
        vec![server_file("file", "note.txt", Some("parent"), b"original")],
        vec!["parent"],
    );
    let handle = server.clone();
    let rig = rig(server).await;
    let fs = Arc::new(FaultyFs::default());
    let engine = build_engine_with_fs(&rig, fs.clone()).await;
    engine.incremental_sync().await.unwrap();
    let old = rig.root.join("parent");
    let new = rig.root.join("renamed");
    std::os::unix::fs::symlink(old.join("note.txt"), old.join("link")).unwrap();
    fs.moves_unsupported.store(true, Ordering::SeqCst);
    rename_on_server(&handle, "parent", "renamed", None).await;

    for _ in 0..2 {
        let report = engine.incremental_sync().await.unwrap();
        assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
        assert!(report.errors[0].reason.contains("cannot be copied"));
        assert!(report.uploaded.is_empty());
        assert!(report.created_remote_folders.is_empty());
        assert_eq!(std::fs::read(old.join("note.txt")).unwrap(), b"original");
        assert!(!new.exists());
        assert_eq!(pending_move_intents(&rig).await, 0);
    }

    std::fs::remove_file(old.join("link")).unwrap();
    let report = engine.incremental_sync().await.unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(std::fs::read(new.join("note.txt")).unwrap(), b"original");
    assert!(!old.exists());
    assert_eq!(handle.delete_file_hits.load(Ordering::SeqCst), 0);
    assert_eq!(handle.create_folder_hits.load(Ordering::SeqCst), 0);
}

/// A walker that passes over a file it could not read must not make the
/// file look moved: its journal row would follow the folder, and the file
/// would then be deleted on the server as missing.
#[tokio::test]
async fn copy_move_stops_when_a_tracked_file_was_not_listed() {
    let server = FakeServer::with_tree(
        vec![
            server_file("first", "a.txt", Some("parent"), b"first file"),
            server_file("second", "b.txt", Some("parent"), b"second file"),
        ],
        vec!["parent"],
    );
    let handle = server.clone();
    let rig = rig(server).await;
    let fs = Arc::new(FaultyFs::default());
    let engine = build_engine_with_fs(&rig, fs.clone()).await;
    engine.incremental_sync().await.unwrap();
    let old = rig.root.join("parent");
    let new = rig.root.join("renamed");
    fs.moves_unsupported.store(true, Ordering::SeqCst);
    fs.fail("hide_from_walk", &old.join("b.txt"));
    rename_on_server(&handle, "parent", "renamed", None).await;

    for _ in 0..3 {
        let report = engine.incremental_sync().await.unwrap();
        assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
        assert!(report.errors[0].reason.contains("could not be listed"));
        assert!(old.join("a.txt").is_file());
        assert!(old.join("b.txt").is_file());
        assert!(!new.exists());
    }
    assert_eq!(handle.delete_file_hits.load(Ordering::SeqCst), 0);

    fs.failures.write().unwrap().clear();
    let report = engine.incremental_sync().await.unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(std::fs::read(new.join("b.txt")).unwrap(), b"second file");
    assert!(!old.exists());
    assert_eq!(handle.delete_file_hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn copying_defers_source_edits_even_when_the_mtime_does_not_change() {
    let server = FakeServer::with_tree(
        vec![server_file("file", "note.txt", Some("parent"), b"original")],
        vec!["parent"],
    );
    let handle = server.clone();
    let rig = rig(server).await;
    let fs = Arc::new(FaultyFs::default());
    let engine = build_engine_with_fs(&rig, fs.clone()).await;
    engine.incremental_sync().await.unwrap();
    let old = rig.root.join("parent/note.txt");
    let new = rig.root.join("renamed/note.txt");
    let mtime = NativeFs.mtime(old.to_str().unwrap()).await.unwrap();
    fs.moves_unsupported.store(true, Ordering::SeqCst);
    fs.fail("edit_source_after_copy", &old);
    rename_on_server(&handle, "parent", "renamed", None).await;
    let report = engine.incremental_sync().await.unwrap();
    assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
    assert!(report.errors[0].reason.contains("file changed during move"));
    assert!(report.uploaded.is_empty());
    assert!(report.created_remote_folders.is_empty());
    assert_eq!(NativeFs.mtime(old.to_str().unwrap()).await.unwrap(), mtime);
    assert_eq!(std::fs::read(old).unwrap(), b"edited during copy");
    assert_eq!(std::fs::read(new).unwrap(), b"original");
}

#[tokio::test]
async fn empty_folder_rename_requires_a_move_intent_to_adopt_a_destination() {
    for (with_intent, populated) in [(false, false), (false, true), (true, false)] {
        let server = FakeServer::with_tree(vec![], vec!["parent"]);
        let handle = server.clone();
        let rig = rig(server).await;
        let engine = build_engine(&rig).await;
        engine.incremental_sync().await.unwrap();
        let old = rig.root.join("parent");
        let new = rig.root.join("renamed");
        std::fs::remove_dir(&old).unwrap();
        std::fs::create_dir(&new).unwrap();
        if populated {
            std::fs::write(new.join("private.txt"), b"unrelated private data").unwrap();
        }
        rename_on_server(&handle, "parent", "renamed", None).await;
        if with_intent {
            let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", rig.db_path.display()))
                .await
                .unwrap();
            sqlx::query("INSERT INTO sync_config (key, value) VALUES (?, ?)")
                .bind("pending_folder_move:parent")
                .bind(
                    serde_json::to_string(&(old.to_str().unwrap(), new.to_str().unwrap(), false))
                        .unwrap(),
                )
                .execute(&pool)
                .await
                .unwrap();
            pool.close().await;
        }
        drop(engine);
        let engine = build_engine(&rig).await;
        for _ in 0..2 {
            let report = engine.incremental_sync().await.unwrap();
            assert!(report.uploaded.is_empty());
            assert!(report.created_remote_folders.is_empty());
            if with_intent {
                assert!(report.errors.is_empty(), "{:?}", report.errors);
            } else {
                assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
                assert!(report.errors[0].reason.contains("a different folder"));
            }
        }
        if populated {
            assert_eq!(
                std::fs::read(new.join("private.txt")).unwrap(),
                b"unrelated private data"
            );
        }
    }
}

#[tokio::test]
async fn remapping_an_override_keeps_all_retired_copies_out_of_discovery() {
    let server = FakeServer::with_folders(
        vec![server_file("file", "note.txt", Some("child"), b"original")],
        vec![
            ServerFolder::root("parent"),
            child_folder("child", "parent"),
        ],
    );
    let deletes = server.delete_file_hits.clone();
    let folder_deletes = server.delete_folder_hits.clone();
    let rig = rig(server).await;
    let engine = build_engine(&rig).await;
    assert!(engine.incremental_sync().await.unwrap().errors.is_empty());
    drop(engine);
    for name in ["mapped-one", "mapped-two", "mapped-one"] {
        let engine = build_engine(&rig).await;
        let mapped = rig.root.join(name);
        std::fs::create_dir_all(&mapped).unwrap();
        engine
            .set_folder_local_path("parent", Some(mapped.to_str().unwrap()))
            .await
            .unwrap();
        drop(engine);
        let engine = build_engine(&rig).await;
        for _ in 0..3 {
            let report = engine.incremental_sync().await.unwrap();
            assert!(report.errors.is_empty(), "{name}: {:?}", report.errors);
            assert!(report.created_remote_folders.is_empty());
            assert!(report.uploaded.is_empty());
        }
        assert_eq!(
            std::fs::read(mapped.join("child/note.txt")).unwrap(),
            b"original"
        );
    }
    for name in ["parent", "mapped-one", "mapped-two"] {
        assert_eq!(
            std::fs::read(rig.root.join(name).join("child/note.txt")).unwrap(),
            b"original"
        );
    }
    assert_eq!(deletes.load(Ordering::SeqCst), 0);
    assert_eq!(folder_deletes.load(Ordering::SeqCst), 0);
}

/// Giving an already-synced folder its own local path is a mapping change,
/// not a server rename: the folder syncs at the new location and the old
/// copy is neither moved, re-uploaded nor read as a deletion.
#[tokio::test]
async fn local_path_override_on_synced_folder_syncs_at_the_new_location() {
    let server = FakeServer::with_tree(
        vec![server_file("file", "note.txt", Some("parent"), b"original")],
        vec!["parent"],
    );
    let files = server.files.clone();
    let deletes = server.delete_file_hits.clone();
    let folder_deletes = server.delete_folder_hits.clone();
    let rig = rig(server).await;
    let engine = build_engine(&rig).await;
    engine.incremental_sync().await.unwrap();

    let mapped = rig.root.parent().unwrap().join("mapped");
    std::fs::create_dir(&mapped).unwrap();
    engine
        .set_folder_local_path("parent", Some(mapped.to_str().unwrap()))
        .await
        .unwrap();
    let mut changed = server_file("file", "note.txt", Some("parent"), b"server change");
    changed.updated_at = "2026-02-01T00:00:00Z".into();
    files.lock().await.insert("file".into(), changed);

    for _ in 0..3 {
        let report = engine.incremental_sync().await.unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(report.created_remote_folders.is_empty());
        assert!(report.uploaded.is_empty());
    }
    assert_eq!(
        std::fs::read(mapped.join("note.txt")).unwrap(),
        b"server change"
    );
    assert_eq!(
        std::fs::read(rig.root.join("parent/note.txt")).unwrap(),
        b"original"
    );
    assert_eq!(deletes.load(Ordering::SeqCst), 0);
    assert_eq!(folder_deletes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn case_only_server_rename_is_followed_on_a_case_insensitive_filesystem() {
    let server = FakeServer::with_tree(
        vec![server_file("file", "note.txt", Some("docs"), b"original")],
        vec!["docs"],
    );
    let downloads = server.download_hits.clone();
    let deletes = server.delete_file_hits.clone();
    let handle = server.clone();
    let rig = rig(server).await;
    let engine = build_engine_with_fs(&rig, Arc::new(CaseFoldFs::default())).await;
    engine.incremental_sync().await.unwrap();
    let initial_downloads = downloads.load(Ordering::SeqCst);
    assert_eq!(dir_names(&rig.root), ["docs"]);

    rename_on_server(&handle, "docs", "Docs", None).await;
    for _ in 0..3 {
        let report = engine.incremental_sync().await.unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
    }
    assert_eq!(dir_names(&rig.root), ["Docs"]);
    assert_eq!(
        std::fs::read(rig.root.join("Docs/note.txt")).unwrap(),
        b"original"
    );
    assert_eq!(downloads.load(Ordering::SeqCst), initial_downloads);
    assert_eq!(deletes.load(Ordering::SeqCst), 0);
}

/// A backend that cannot change only the case of a name in one step gets
/// there through a temporary name, with the folders beneath it waiting for
/// it instead of moving on their own.
#[tokio::test]
async fn case_only_server_rename_goes_through_a_temporary_name_when_it_must() {
    let server = FakeServer::with_folders(
        vec![
            server_file("file", "note.txt", Some("docs"), b"original"),
            server_file("inner", "inner.txt", Some("sub"), b"inner"),
        ],
        vec![ServerFolder::root("docs"), child_folder("sub", "docs")],
    );
    let handle = server.clone();
    let rig = rig(server).await;
    let fs = Arc::new(CaseFoldFs::default());
    fs.case_rename_unsupported.store(true, Ordering::SeqCst);
    let engine = build_engine_with_fs(&rig, fs).await;
    engine.incremental_sync().await.unwrap();
    let downloads = handle.download_hits.load(Ordering::SeqCst);
    assert_eq!(dir_names(&rig.root), ["docs"]);

    rename_on_server(&handle, "docs", "Docs", None).await;
    for _ in 0..3 {
        let report = engine.incremental_sync().await.unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(report.uploaded.is_empty());
        assert!(report.created_remote_folders.is_empty());
        assert_eq!(dir_names(&rig.root), ["Docs"]);
    }
    assert_eq!(
        std::fs::read(rig.root.join("Docs/note.txt")).unwrap(),
        b"original"
    );
    assert_eq!(
        std::fs::read(rig.root.join("Docs/sub/inner.txt")).unwrap(),
        b"inner"
    );
    assert_eq!(handle.download_hits.load(Ordering::SeqCst), downloads);
    assert_eq!(handle.delete_file_hits.load(Ordering::SeqCst), 0);
    assert_eq!(handle.delete_folder_hits.load(Ordering::SeqCst), 0);
    assert_eq!(pending_move_intents(&rig).await, 0);
}

/// Folders that swap names take their subfolders with them: a subfolder
/// does not move on its own while its parent is still waiting for its turn.
#[tokio::test]
async fn swapped_folders_keep_their_subfolders() {
    let server = FakeServer::with_folders(
        vec![
            server_file("fx", "x.txt", Some("x"), b"X"),
            server_file("fy", "y.txt", Some("y"), b"Y"),
        ],
        vec![
            ServerFolder::root("a"),
            ServerFolder::root("b"),
            child_folder("x", "a"),
            child_folder("y", "b"),
        ],
    );
    let handle = server.clone();
    let rig = rig(server).await;
    let engine = build_engine(&rig).await;
    engine.incremental_sync().await.unwrap();
    let downloads = handle.download_hits.load(Ordering::SeqCst);

    rename_on_server(&handle, "a", "b", None).await;
    rename_on_server(&handle, "b", "a", None).await;
    for _ in 0..3 {
        let report = engine.incremental_sync().await.unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(report.uploaded.is_empty());
        assert!(report.created_remote_folders.is_empty());
        assert_eq!(dir_names(&rig.root), ["a", "b"]);
        assert_eq!(std::fs::read(rig.root.join("b/x/x.txt")).unwrap(), b"X");
        assert_eq!(std::fs::read(rig.root.join("a/y/y.txt")).unwrap(), b"Y");
        assert_eq!(dir_names(&rig.root.join("a")), ["y"]);
        assert_eq!(dir_names(&rig.root.join("b")), ["x"]);
    }
    assert_eq!(handle.download_hits.load(Ordering::SeqCst), downloads);
    assert_eq!(handle.delete_file_hits.load(Ordering::SeqCst), 0);
    assert_eq!(handle.delete_folder_hits.load(Ordering::SeqCst), 0);
    assert_eq!(pending_move_intents(&rig).await, 0);
}

/// What an interrupted download left in a folder is invisible to the
/// walkers. It must not keep the folder alive once the server deleted it,
/// or the folder would be created on the server again.
#[tokio::test]
async fn leftover_download_file_does_not_bring_back_a_folder_deleted_on_the_server() {
    let server = FakeServer::with_tree(
        vec![server_file("file", "note.txt", Some("parent"), b"original")],
        vec!["parent"],
    );
    let handle = server.clone();
    let rig = rig(server).await;
    let engine = build_engine(&rig).await;
    engine.incremental_sync().await.unwrap();
    let dir = rig.root.join("parent");
    // The client was stopped in the middle of a download.
    drop(engine);
    std::fs::write(
        dir.join(".uncloud-download-0b0e6a3e-6f0f-4c59-9d0c-2f0e3f6f7a11.uncloud-part"),
        b"half a download",
    )
    .unwrap();
    handle.folders.lock().await.clear();
    handle.files.lock().await.clear();
    let engine = build_engine(&rig).await;

    for _ in 0..2 {
        let report = engine.incremental_sync().await.unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(report.created_remote_folders.is_empty());
    }
    assert!(!dir.exists());
    assert_eq!(handle.create_folder_hits.load(Ordering::SeqCst), 0);
}

/// The conflict copy is the only place the local version survives the
/// download that follows. When it cannot be written, the download does not
/// happen.
#[tokio::test]
async fn failed_conflict_copy_keeps_the_local_version() {
    let server = FakeServer::with(vec![server_file("file", "note.txt", None, b"original")]);
    let handle = server.clone();
    let rig = rig(server).await;
    let fs = Arc::new(FaultyFs::default());
    let engine = build_engine_with_fs(&rig, fs.clone()).await;
    engine.incremental_sync().await.unwrap();
    let local = rig.root.join("note.txt");

    std::fs::write(&local, b"local edit").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&local)
        .unwrap()
        .set_modified(std::time::SystemTime::now() + Duration::from_secs(60))
        .unwrap();
    let mut changed = server_file("file", "note.txt", None, b"server change");
    changed.updated_at = "2026-02-01T00:00:00Z".into();
    handle.files.lock().await.insert("file".into(), changed);
    let downloads = handle.download_hits.load(Ordering::SeqCst);

    fs.fail("copy_file", &local);
    for _ in 0..2 {
        let report = engine.incremental_sync().await.unwrap();
        assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
        assert!(report.errors[0].reason.contains("conflict copy"));
        assert!(report.conflicts.is_empty());
        assert_eq!(std::fs::read(&local).unwrap(), b"local edit");
    }
    assert_eq!(handle.download_hits.load(Ordering::SeqCst), downloads);

    fs.failures.write().unwrap().clear();
    let report = engine.incremental_sync().await.unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.conflicts.len(), 1);
    assert_eq!(std::fs::read(&local).unwrap(), b"server change");
    assert_eq!(
        std::fs::read(&report.conflicts[0].conflict_copy).unwrap(),
        b"local edit"
    );
}

/// An upload states the length the file had when it was opened and sends
/// exactly that much, so the checksum on record is the checksum of what
/// the server received even when the file grew in the meantime.
#[tokio::test]
async fn upload_declares_its_length_and_stops_there() {
    let server = FakeServer::with(vec![server_file("file", "note.txt", None, b"original")]);
    let handle = server.clone();
    let rig = rig(server).await;
    let fs = Arc::new(FaultyFs::default());
    let engine = build_engine_with_fs(&rig, fs.clone()).await;
    engine.incremental_sync().await.unwrap();
    let local = rig.root.join("note.txt");
    let write = |content: &[u8], ahead: u64| {
        std::fs::write(&local, content).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&local)
            .unwrap()
            .set_modified(std::time::SystemTime::now() + Duration::from_secs(ahead))
            .unwrap();
    };
    let recorded_checksum = || async {
        let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", rig.db_path.display()))
            .await
            .unwrap();
        let checksum: String =
            sqlx::query_scalar("SELECT checksum FROM sync_state WHERE server_id = 'file'")
                .fetch_one(&pool)
                .await
                .unwrap();
        pool.close().await;
        checksum
    };

    write(b"a local edit", 60);
    let report = engine.incremental_sync().await.unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(handle.files.lock().await["file"].bytes, b"a local edit");
    let length = handle.content_updates.lock().await[0].1.unwrap();
    assert!(
        length > 12,
        "the multipart framing is counted too: {length}"
    );

    // The file is longer than it was when its size was taken.
    write(b"0123456789 and what was appended since", 120);
    fs.fail("short_size", &local);
    let report = engine.incremental_sync().await.unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(handle.files.lock().await["file"].bytes, b"0123456789");
    assert_eq!(
        recorded_checksum().await,
        format!("{:x}", Sha256::digest(b"0123456789"))
    );
}

/// A retired path is only kept out of discovery while the old copy is
/// still there. Once it is gone, a directory created at the same path is a
/// new folder like any other.
#[tokio::test]
async fn retired_path_is_released_once_the_old_copy_is_gone() {
    let server = FakeServer::with_tree(
        vec![server_file("file", "note.txt", Some("parent"), b"original")],
        vec!["parent"],
    );
    let handle = server.clone();
    let rig = rig(server).await;
    let retired = || async {
        let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", rig.db_path.display()))
            .await
            .unwrap();
        let mut paths: Vec<String> = sqlx::query_scalar(
            "SELECT value FROM sync_config WHERE key LIKE 'retired_local_path:%'",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        pool.close().await;
        paths.sort();
        paths
    };
    let engine = build_engine(&rig).await;
    engine.incremental_sync().await.unwrap();
    let first = rig.root.join("mapped-one");
    let second = rig.root.join("mapped-two");
    for mapped in [&first, &second] {
        std::fs::create_dir_all(mapped).unwrap();
        engine
            .set_folder_local_path("parent", Some(mapped.to_str().unwrap()))
            .await
            .unwrap();
        for _ in 0..2 {
            let report = engine.incremental_sync().await.unwrap();
            assert!(report.errors.is_empty(), "{:?}", report.errors);
        }
    }
    assert!(retired()
        .await
        .contains(&first.to_string_lossy().into_owned()));
    assert_eq!(handle.create_folder_hits.load(Ordering::SeqCst), 0);

    std::fs::remove_dir_all(&first).unwrap();
    let report = engine.incremental_sync().await.unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(!retired()
        .await
        .contains(&first.to_string_lossy().into_owned()));

    std::fs::create_dir(&first).unwrap();
    let report = engine.incremental_sync().await.unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.created_remote_folders, ["mapped-one"]);
}

/// A move the backend cannot do as one rename (another SAF directory,
/// another volume) is done by copying. An interrupted copy leaves the
/// original complete and is resumed, and files whose mtime the copy could
/// not preserve are not mistaken for local edits.
#[tokio::test]
async fn server_move_falls_back_to_copying_and_resumes_after_a_failure() {
    let server = FakeServer::with_folders(
        vec![
            server_file("top", "top.txt", Some("parent"), b"top"),
            server_file("note", "note.txt", Some("child"), b"note"),
        ],
        vec![
            ServerFolder::root("archive"),
            ServerFolder::root("parent"),
            child_folder("child", "parent"),
            child_folder("empty", "parent"),
        ],
    );
    let downloads = server.download_hits.clone();
    let deletes = server.delete_file_hits.clone();
    let folder_deletes = server.delete_folder_hits.clone();
    let handle = server.clone();
    let rig = rig(server).await;
    let fs = Arc::new(FaultyFs::default());
    let engine = build_engine_with_fs(&rig, fs.clone()).await;
    engine.incremental_sync().await.unwrap();
    let initial_downloads = downloads.load(Ordering::SeqCst);
    let old = rig.root.join("parent");
    let new = rig.root.join("archive/parent");

    // Copies made from here on carry a later mtime than the journal has.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    fs.moves_unsupported.store(true, Ordering::SeqCst);
    fs.fail("copy_file", &old.join("child/note.txt"));
    rename_on_server(&handle, "parent", "parent", Some("archive")).await;

    let report = engine.incremental_sync().await.unwrap();
    assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
    assert_eq!(std::fs::read(old.join("top.txt")).unwrap(), b"top");
    assert_eq!(std::fs::read(old.join("child/note.txt")).unwrap(), b"note");

    fs.failures.write().unwrap().clear();
    for _ in 0..3 {
        let report = engine.incremental_sync().await.unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(report.uploaded.is_empty());
    }
    assert_eq!(std::fs::read(new.join("top.txt")).unwrap(), b"top");
    assert_eq!(std::fs::read(new.join("child/note.txt")).unwrap(), b"note");
    assert!(new.join("empty").is_dir());
    assert!(!old.exists());
    assert_eq!(downloads.load(Ordering::SeqCst), initial_downloads);
    assert_eq!(deletes.load(Ordering::SeqCst), 0);
    assert_eq!(folder_deletes.load(Ordering::SeqCst), 0);
    assert_eq!(pending_move_intents(&rig).await, 0);
}

/// Under a strategy that never applies server changes locally, a server
/// rename leaves the local directory where it is and the folder stays
/// linked. Once the strategy takes server changes again, the rename lands.
#[tokio::test]
async fn server_rename_leaves_an_upload_only_folder_in_place() {
    let server = FakeServer::with_tree(
        vec![server_file("file", "note.txt", Some("parent"), b"original")],
        vec!["parent"],
    );
    let downloads = server.download_hits.clone();
    let deletes = server.delete_file_hits.clone();
    let folder_deletes = server.delete_folder_hits.clone();
    let handle = server.clone();
    let rig = rig(server).await;
    let engine = build_engine(&rig).await;
    engine.incremental_sync().await.unwrap();
    let initial_downloads = downloads.load(Ordering::SeqCst);

    for strategy in [SyncStrategy::UploadOnly, SyncStrategy::ClientToServer] {
        engine
            .set_folder_local_strategy("parent", Some(strategy))
            .await
            .unwrap();
        rename_on_server(&handle, "parent", "renamed", None).await;
        for _ in 0..3 {
            let report = engine.incremental_sync().await.unwrap();
            assert!(report.errors.is_empty(), "{:?}", report.errors);
            assert!(report.created_remote_folders.is_empty());
        }
        assert_eq!(dir_names(&rig.root), ["parent"]);
        assert!(rig.root.join("parent/note.txt").is_file());
    }

    engine
        .set_folder_local_strategy("parent", None)
        .await
        .unwrap();
    for _ in 0..2 {
        let report = engine.incremental_sync().await.unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
    }
    assert_eq!(dir_names(&rig.root), ["renamed"]);
    assert_eq!(
        std::fs::read(rig.root.join("renamed/note.txt")).unwrap(),
        b"original"
    );
    assert_eq!(downloads.load(Ordering::SeqCst), initial_downloads);
    assert_eq!(deletes.load(Ordering::SeqCst), 0);
    assert_eq!(folder_deletes.load(Ordering::SeqCst), 0);
}

/// Two folders that swap names, and a chain where one folder takes the name
/// another one just gave up, are followed within a single run.
#[tokio::test]
async fn server_swap_and_chain_renames_are_followed_in_one_run() {
    for _ in 0..8 {
        let server = FakeServer::with_tree(
            vec![
                server_file("fa", "a.txt", Some("a"), b"A"),
                server_file("fb", "b.txt", Some("b"), b"B"),
                server_file("fc", "c.txt", Some("c"), b"C"),
                server_file("fe", "e.txt", Some("e"), b"E"),
            ],
            vec!["a", "b", "c", "e"],
        );
        let downloads = server.download_hits.clone();
        let deletes = server.delete_file_hits.clone();
        let handle = server.clone();
        let rig = rig(server).await;
        let engine = build_engine(&rig).await;
        engine.incremental_sync().await.unwrap();
        let initial_downloads = downloads.load(Ordering::SeqCst);

        rename_on_server(&handle, "a", "b", None).await;
        rename_on_server(&handle, "b", "a", None).await;
        rename_on_server(&handle, "c", "d", None).await;
        rename_on_server(&handle, "e", "c", None).await;

        for _ in 0..2 {
            let report = engine.incremental_sync().await.unwrap();
            assert!(report.errors.is_empty(), "{:?}", report.errors);
        }
        assert_eq!(dir_names(&rig.root), ["a", "b", "c", "d"]);
        assert_eq!(std::fs::read(rig.root.join("b/a.txt")).unwrap(), b"A");
        assert_eq!(std::fs::read(rig.root.join("a/b.txt")).unwrap(), b"B");
        assert_eq!(std::fs::read(rig.root.join("d/c.txt")).unwrap(), b"C");
        assert_eq!(std::fs::read(rig.root.join("c/e.txt")).unwrap(), b"E");
        assert_eq!(downloads.load(Ordering::SeqCst), initial_downloads);
        assert_eq!(deletes.load(Ordering::SeqCst), 0);
        assert_eq!(pending_move_intents(&rig).await, 0);
    }
}

/// The server deleted one folder and gave its name to another. The deleted
/// folder's local copy is cleared first and the rename lands on the next
/// run, without an error and without the two being mixed up.
#[tokio::test]
async fn rename_onto_the_name_of_a_server_deleted_folder_waits_for_the_cleanup() {
    let server = FakeServer::with_tree(
        vec![
            server_file("fa", "a.txt", Some("a"), b"A"),
            server_file("fb", "b.txt", Some("b"), b"B"),
        ],
        vec!["a", "b"],
    );
    let downloads = server.download_hits.clone();
    let deletes = server.delete_file_hits.clone();
    let folder_deletes = server.delete_folder_hits.clone();
    let handle = server.clone();
    let rig = rig(server).await;
    let engine = build_engine(&rig).await;
    engine.incremental_sync().await.unwrap();
    let initial_downloads = downloads.load(Ordering::SeqCst);

    handle.folders.lock().await.remove("b");
    handle.files.lock().await.remove("fb");
    rename_on_server(&handle, "a", "b", None).await;

    for _ in 0..3 {
        let report = engine.incremental_sync().await.unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(report.created_remote_folders.is_empty());
        assert!(report.uploaded.is_empty());
    }
    assert_eq!(dir_names(&rig.root), ["b"]);
    assert_eq!(dir_names(&rig.root.join("b")), ["a.txt"]);
    assert_eq!(std::fs::read(rig.root.join("b/a.txt")).unwrap(), b"A");
    assert_eq!(downloads.load(Ordering::SeqCst), initial_downloads);
    assert_eq!(deletes.load(Ordering::SeqCst), 0);
    assert_eq!(folder_deletes.load(Ordering::SeqCst), 0);
}

/// A folder renamed to a name whose local directory belongs to a folder
/// that is staying put is reported and skipped. It is not parked under a
/// temporary name, and the folder that holds the name keeps syncing.
#[tokio::test]
async fn rename_onto_a_name_held_by_a_folder_that_stays_is_skipped() {
    let server = FakeServer::with_tree(
        vec![
            server_file("fp", "p.txt", Some("parent"), b"P"),
            server_file("fq", "q.txt", Some("q"), b"Q"),
        ],
        vec!["parent", "q"],
    );
    let files = server.files.clone();
    let deletes = server.delete_file_hits.clone();
    let folder_deletes = server.delete_folder_hits.clone();
    let handle = server.clone();
    let rig = rig(server).await;
    let engine = build_engine(&rig).await;
    engine.incremental_sync().await.unwrap();

    // "parent" keeps its local name under a strategy that ignores server
    // renames, while "q" is renamed to exactly that name.
    engine
        .set_folder_local_strategy("parent", Some(SyncStrategy::UploadOnly))
        .await
        .unwrap();
    rename_on_server(&handle, "parent", "renamed", None).await;
    rename_on_server(&handle, "q", "parent", None).await;
    let mut changed = server_file("fq", "q.txt", Some("q"), b"Q changed");
    changed.updated_at = "2026-03-01T00:00:00Z".into();
    files.lock().await.insert("fq".into(), changed);

    for _ in 0..3 {
        let report = engine.incremental_sync().await.unwrap();
        assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
        assert!(report.created_remote_folders.is_empty());
        assert!(report.uploaded.is_empty());
        assert!(report.downloaded.is_empty());
    }
    assert_eq!(dir_names(&rig.root), ["parent", "q"]);
    assert_eq!(std::fs::read(rig.root.join("parent/p.txt")).unwrap(), b"P");
    assert_eq!(std::fs::read(rig.root.join("q/q.txt")).unwrap(), b"Q");
    assert_eq!(deletes.load(Ordering::SeqCst), 0);
    assert_eq!(folder_deletes.load(Ordering::SeqCst), 0);
}

/// A swap interrupted right after one folder was parked under its temporary
/// name is finished by the next run.
#[tokio::test]
async fn interrupted_swap_is_finished_after_a_restart() {
    let server = FakeServer::with_tree(
        vec![
            server_file("fa", "a.txt", Some("a"), b"A"),
            server_file("fb", "b.txt", Some("b"), b"B"),
        ],
        vec!["a", "b"],
    );
    let handle = server.clone();
    let rig = rig(server).await;
    let engine = build_engine(&rig).await;
    engine.incremental_sync().await.unwrap();
    drop(engine);

    rename_on_server(&handle, "a", "b", None).await;
    rename_on_server(&handle, "b", "a", None).await;
    let parked = rig.root.join("a.uncloud-moving");
    std::fs::rename(rig.root.join("a"), &parked).unwrap();
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", rig.db_path.display()))
        .await
        .unwrap();
    sqlx::query("INSERT INTO sync_config (key, value) VALUES (?, ?)")
        .bind("pending_folder_move:a")
        .bind(
            serde_json::to_string(&(
                rig.root.join("a").to_str().unwrap(),
                parked.to_str().unwrap(),
                false,
            ))
            .unwrap(),
        )
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;

    let engine = build_engine(&rig).await;
    for _ in 0..2 {
        let report = engine.incremental_sync().await.unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(report.created_remote_folders.is_empty());
    }
    assert_eq!(dir_names(&rig.root), ["a", "b"]);
    assert_eq!(std::fs::read(rig.root.join("b/a.txt")).unwrap(), b"A");
    assert_eq!(std::fs::read(rig.root.join("a/b.txt")).unwrap(), b"B");
}

/// The same rename made by hand on this device and on the server: the
/// directory at the new name holds exactly the tracked files, so it is the
/// folder and is adopted without an error or a transfer.
#[tokio::test]
async fn incoming_rename_adopts_a_destination_that_is_the_same_folder() {
    let server = FakeServer::with_tree(
        vec![server_file("file", "note.txt", Some("parent"), b"original")],
        vec!["parent"],
    );
    let downloads = server.download_hits.clone();
    let deletes = server.delete_file_hits.clone();
    let folder_deletes = server.delete_folder_hits.clone();
    let handle = server.clone();
    let rig = rig(server).await;
    let engine = build_engine(&rig).await;
    engine.incremental_sync().await.unwrap();
    let initial_downloads = downloads.load(Ordering::SeqCst);

    std::fs::rename(rig.root.join("parent"), rig.root.join("renamed")).unwrap();
    rename_on_server(&handle, "parent", "renamed", None).await;
    for _ in 0..3 {
        let report = engine.incremental_sync().await.unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
    }
    assert_eq!(dir_names(&rig.root), ["renamed"]);
    assert_eq!(downloads.load(Ordering::SeqCst), initial_downloads);
    assert_eq!(deletes.load(Ordering::SeqCst), 0);
    assert_eq!(folder_deletes.load(Ordering::SeqCst), 0);
}

/// A download that is cut off half-way leaves the local file exactly as it
/// was, with no partial file beside it, and the next run completes it.
#[tokio::test]
async fn download_cut_off_mid_stream_leaves_the_local_file_untouched() {
    let server = FakeServer::with_tree(
        vec![server_file("file", "note.txt", Some("parent"), b"original")],
        vec!["parent"],
    );
    let files = server.files.clone();
    let cut_off = server.cut_off_downloads.clone();
    let rig = rig(server).await;
    let engine = build_engine(&rig).await;
    engine.incremental_sync().await.unwrap();

    let mut changed = server_file(
        "file",
        "note.txt",
        Some("parent"),
        b"a newer version from the server",
    );
    changed.updated_at = "2026-02-01T00:00:00Z".into();
    files.lock().await.insert("file".into(), changed);
    cut_off.lock().await.insert("file".into());

    for _ in 0..2 {
        let report = engine.incremental_sync().await.unwrap();
        assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
        assert!(report.uploaded.is_empty());
        assert_eq!(
            std::fs::read(rig.root.join("parent/note.txt")).unwrap(),
            b"original"
        );
        assert_eq!(dir_names(&rig.root.join("parent")), ["note.txt"]);
    }

    cut_off.lock().await.clear();
    let report = engine.incremental_sync().await.unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(
        std::fs::read(rig.root.join("parent/note.txt")).unwrap(),
        b"a newer version from the server"
    );
}

#[tokio::test]
async fn failed_incoming_rename_leaves_no_move_intent_behind() {
    let server = FakeServer::with_tree(
        vec![server_file("file", "note.txt", Some("parent"), b"original")],
        vec!["parent"],
    );
    let handle = server.clone();
    let rig = rig(server).await;
    let fs = Arc::new(FaultyFs::default());
    let engine = build_engine_with_fs(&rig, fs.clone()).await;
    engine.incremental_sync().await.unwrap();

    rename_on_server(&handle, "parent", "renamed", None).await;
    fs.fail("rename", &rig.root.join("parent"));
    let report = engine.incremental_sync().await.unwrap();
    assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
    assert!(
        report.errors[0]
            .reason
            .contains("could not follow the server rename"),
        "{:?}",
        report.errors
    );
    assert_eq!(pending_move_intents(&rig).await, 0);
}

#[tokio::test]
async fn incoming_rename_does_not_adopt_an_unrelated_destination() {
    let server = FakeServer::with_tree(
        vec![server_file(
            "file",
            "note.txt",
            Some("parent"),
            b"remote content",
        )],
        vec!["parent"],
    );
    let folders = server.folders.clone();
    let deletes = server.delete_file_hits.clone();
    let rig = rig(server).await;
    let engine = build_engine(&rig).await;
    engine.incremental_sync().await.unwrap();
    std::fs::remove_dir_all(rig.root.join("parent")).unwrap();
    std::fs::create_dir(rig.root.join("renamed")).unwrap();
    std::fs::write(
        rig.root.join("renamed/note.txt"),
        b"unrelated local content",
    )
    .unwrap();
    {
        let mut folders = folders.lock().await;
        let parent = folders.get_mut("parent").unwrap();
        parent.name = "renamed".into();
        parent.updated_at = "2026-02-01T00:00:00Z".into();
    }
    for _ in 0..3 {
        let report = engine.incremental_sync().await.unwrap();
        assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
        assert!(report.uploaded.is_empty());
        assert!(report.downloaded.is_empty());
    }
    assert_eq!(
        std::fs::read(rig.root.join("renamed/note.txt")).unwrap(),
        b"unrelated local content"
    );
    assert_eq!(deletes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unreadable_folder_protects_its_descendant_deletions() {
    let server = FakeServer::with_tree(
        vec![server_file("file", "note.txt", Some("parent"), b"original")],
        vec!["parent"],
    );
    let deletes = server.delete_file_hits.clone();
    let rig = rig(server).await;
    let fs = Arc::new(FaultyFs::default());
    let engine = build_engine_with_fs(&rig, fs.clone()).await;
    engine.incremental_sync().await.unwrap();
    std::fs::remove_file(rig.root.join("parent/note.txt")).unwrap();
    fs.fail("is_dir", &rig.root.join("parent"));
    for _ in 0..3 {
        let report = engine.incremental_sync().await.unwrap();
        assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
        assert_eq!(journal_status(&rig, "file", "file").await.1, None);
    }
    assert_eq!(deletes.load(Ordering::SeqCst), 0);
    fs.failures.write().unwrap().clear();
    engine.incremental_sync().await.unwrap();
    assert_eq!(deletes.load(Ordering::SeqCst), 0);
    engine.incremental_sync().await.unwrap();
    assert_eq!(deletes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn unreadable_folder_blocks_descendant_transfers_and_discovery() {
    let server = FakeServer::with_tree(
        vec![server_file("file", "note.txt", Some("parent"), b"original")],
        vec!["parent"],
    );
    let files = server.files.clone();
    let downloads = server.download_hits.clone();
    let rig = rig(server).await;
    let fs = Arc::new(FaultyFs::default());
    let engine = build_engine_with_fs(&rig, fs.clone()).await;
    engine.incremental_sync().await.unwrap();
    let initial_downloads = downloads.load(Ordering::SeqCst);
    std::fs::create_dir(rig.root.join("parent/new-child")).unwrap();
    std::fs::write(rig.root.join("parent/new.txt"), b"not uploaded").unwrap();
    fs.fail("is_dir", &rig.root.join("parent"));
    let mut changed = server_file("file", "note.txt", Some("parent"), b"server change");
    changed.updated_at = "2026-02-01T00:00:00Z".into();
    files.lock().await.insert("file".into(), changed);
    let report = engine.incremental_sync().await.unwrap();
    assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
    assert_eq!(downloads.load(Ordering::SeqCst), initial_downloads);
    assert_eq!(
        std::fs::read(rig.root.join("parent/note.txt")).unwrap(),
        b"original"
    );
    assert!(report.created_remote_folders.is_empty());
    assert!(report.uploaded.is_empty());
    fs.failures.write().unwrap().clear();
    engine.incremental_sync().await.unwrap();
    assert_eq!(
        std::fs::read(rig.root.join("parent/note.txt")).unwrap(),
        b"server change"
    );
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
async fn locally_deleted_empty_folder_stays_absent_and_is_deleted_remotely() {
    let server = FakeServer::with_tree(Vec::new(), vec!["empty"]);
    let delete_hits = server.delete_folder_hits.clone();
    let rig = rig(server).await;
    let engine = build_engine(&rig).await;

    engine.incremental_sync().await.unwrap();
    let folder = rig.root.join("empty");
    assert!(folder.is_dir(), "first sync should create the folder");

    std::fs::remove_dir(&folder).unwrap();

    let first_absence = engine.incremental_sync().await.unwrap();
    assert!(
        !folder.exists(),
        "the first absence scan must not recreate a tracked folder"
    );
    assert!(first_absence.deleted_local.is_empty());
    assert_eq!(delete_hits.load(Ordering::SeqCst), 0);

    let confirmed = engine.incremental_sync().await.unwrap();
    assert_eq!(delete_hits.load(Ordering::SeqCst), 1);
    assert_eq!(confirmed.deleted_local, vec!["empty".to_owned()]);
    assert!(!folder.exists());
}

#[tokio::test]
async fn locally_deleted_folder_collapses_its_files_into_one_remote_delete() {
    let server = FakeServer::with_tree(
        vec![ServerFile {
            id: "f-inside".into(),
            name: "inside.txt".into(),
            parent_id: Some("documents".into()),
            bytes: b"contents\n".to_vec(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        }],
        vec!["documents"],
    );
    let delete_file_hits = server.delete_file_hits.clone();
    let delete_folder_hits = server.delete_folder_hits.clone();
    let rig = rig(server).await;
    let engine = build_engine(&rig).await;

    engine.incremental_sync().await.unwrap();
    let folder = rig.root.join("documents");
    assert!(folder.join("inside.txt").is_file());

    std::fs::remove_dir_all(&folder).unwrap();

    engine.incremental_sync().await.unwrap();
    assert!(
        !folder.exists(),
        "the first absence scan must preserve evidence of the folder deletion"
    );
    assert_eq!(delete_folder_hits.load(Ordering::SeqCst), 0);
    assert_eq!(delete_file_hits.load(Ordering::SeqCst), 0);

    engine.incremental_sync().await.unwrap();
    assert_eq!(delete_folder_hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        delete_file_hits.load(Ordering::SeqCst),
        0,
        "the server folder delete should cascade instead of deleting every file separately"
    );
    assert!(!folder.exists());
}

#[tokio::test]
async fn remote_change_restores_only_the_changed_item() {
    let server = FakeServer::with_folders(
        vec![
            server_file("changed-file", "changed.txt", Some("parent"), b"original"),
            server_file("plain-file", "plain.txt", Some("parent"), b"unchanged"),
            server_file("sibling-file", "sibling.txt", Some("sibling"), b"unchanged"),
        ],
        vec![
            ServerFolder::root("parent"),
            child_folder("sibling", "parent"),
        ],
    );
    let files = server.files.clone();
    let folders = server.folders.clone();
    let downloads = server.download_hits.clone();
    let delete_file_hits = server.delete_file_hits.clone();
    let delete_folder_hits = server.delete_folder_hits.clone();
    let rig = rig(server).await;
    let engine = build_engine(&rig).await;

    engine.incremental_sync().await.unwrap();
    let parent = rig.root.join("parent");
    assert!(parent.join("sibling/sibling.txt").is_file());
    let downloads_before = downloads.load(Ordering::SeqCst);

    std::fs::remove_dir_all(&parent).unwrap();
    {
        let mut files = files.lock().await;
        let changed = files.get_mut("changed-file").unwrap();
        changed.bytes = b"changed remotely".to_vec();
        changed.updated_at = "2026-02-01T00:00:00Z".into();
    }

    // The server change wins for the changed file alone: it comes back,
    // together with the folder it lives in. Nothing else is re-downloaded.
    let report = engine.incremental_sync().await.unwrap();
    assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
    assert_eq!(
        std::fs::read(parent.join("changed.txt")).unwrap(),
        b"changed remotely"
    );
    assert!(!parent.join("plain.txt").exists());
    assert!(!parent.join("sibling").exists());
    assert_eq!(downloads.load(Ordering::SeqCst), downloads_before + 1);
    assert_eq!(delete_file_hits.load(Ordering::SeqCst), 0);
    assert_eq!(delete_folder_hits.load(Ordering::SeqCst), 0);

    // The rest of the local deletion still goes through once confirmed.
    engine.incremental_sync().await.unwrap();
    assert_eq!(delete_file_hits.load(Ordering::SeqCst), 1);
    assert_eq!(delete_folder_hits.load(Ordering::SeqCst), 1);
    let files = files.lock().await;
    assert!(files.contains_key("changed-file"));
    assert!(!files.contains_key("plain-file"));
    assert!(!files.contains_key("sibling-file"));
    let folders = folders.lock().await;
    assert!(folders.contains_key("parent"));
    assert!(!folders.contains_key("sibling"));
}

#[tokio::test]
async fn single_file_changed_on_server_while_deleted_locally_is_restored() {
    let server = FakeServer::with(vec![server_file("f-1", "note.txt", None, b"original")]);
    let files = server.files.clone();
    let delete_hits = server.delete_file_hits.clone();
    let rig = rig(server).await;
    let engine = build_engine(&rig).await;
    engine.incremental_sync().await.unwrap();
    let path = rig.root.join("note.txt");
    std::fs::remove_file(&path).unwrap();
    engine.incremental_sync().await.unwrap();
    assert!(journal_status(&rig, "f-1", "file").await.1.is_some());

    {
        let mut files = files.lock().await;
        let file = files.get_mut("f-1").unwrap();
        file.bytes = b"edited on server".to_vec();
        file.updated_at = "2026-02-01T00:00:00Z".into();
    }

    for _ in 0..3 {
        let report = engine.incremental_sync().await.unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
    }
    assert_eq!(std::fs::read(&path).unwrap(), b"edited on server");
    assert_eq!(delete_hits.load(Ordering::SeqCst), 0);
    assert_eq!(
        journal_status(&rig, "f-1", "file").await,
        ("synced".to_owned(), None)
    );
}

#[tokio::test]
async fn deletion_waits_for_the_confirmation_delay() {
    let server = FakeServer::with_tree(
        vec![
            server_file("f-inside", "inside.txt", Some("documents"), b"contents"),
            server_file("f-root", "root.txt", None, b"contents"),
        ],
        vec!["documents"],
    );
    let delete_file_hits = server.delete_file_hits.clone();
    let delete_folder_hits = server.delete_folder_hits.clone();
    let rig = rig(server).await;
    let engine = build_engine_with_delay(&rig, Duration::from_secs(3600)).await;
    engine.incremental_sync().await.unwrap();

    std::fs::remove_dir_all(rig.root.join("documents")).unwrap();
    std::fs::remove_file(rig.root.join("root.txt")).unwrap();

    // However many scans run inside the window, nothing is pushed.
    for _ in 0..4 {
        engine.incremental_sync().await.unwrap();
    }
    assert_eq!(delete_file_hits.load(Ordering::SeqCst), 0);
    assert_eq!(delete_folder_hits.load(Ordering::SeqCst), 0);
    assert!(journal_status(&rig, "documents", "folder")
        .await
        .1
        .is_some());
    assert!(journal_status(&rig, "f-root", "file").await.1.is_some());

    let two_hours_ago = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
    backdate_pending(&rig, "documents", "folder", &two_hours_ago).await;
    backdate_pending(&rig, "f-root", "file", &two_hours_ago).await;
    engine.incremental_sync().await.unwrap();
    assert_eq!(delete_folder_hits.load(Ordering::SeqCst), 1);
    assert_eq!(delete_file_hits.load(Ordering::SeqCst), 1);
}

// A marker that cannot be read, or that lies in the future after a clock
// change, restarts the wait instead of confirming.
#[tokio::test]
async fn unusable_pending_marker_restarts_the_wait() {
    for marker in ["not a timestamp", "2999-01-01T00:00:00Z"] {
        let server = FakeServer::with(vec![server_file("f-1", "note.txt", None, b"x")]);
        let delete_hits = server.delete_file_hits.clone();
        let rig = rig(server).await;
        let engine = build_engine_with_delay(&rig, Duration::from_secs(3600)).await;
        engine.incremental_sync().await.unwrap();
        std::fs::remove_file(rig.root.join("note.txt")).unwrap();
        engine.incremental_sync().await.unwrap();
        backdate_pending(&rig, "f-1", "file", marker).await;

        engine.incremental_sync().await.unwrap();
        assert_eq!(delete_hits.load(Ordering::SeqCst), 0, "{marker}");
        let since = journal_status(&rig, "f-1", "file").await.1.unwrap();
        assert_ne!(since, marker);
        assert!(chrono::DateTime::parse_from_rfc3339(&since).is_ok());
    }
}

// The server tree is fetched once at the start of a run. Something that
// lands on the server while the run is busy must not be trashed by a
// delete that was decided against the older snapshot.
#[tokio::test]
async fn delete_is_postponed_when_the_server_changed_during_the_run() {
    let server = FakeServer::with_tree(
        vec![server_file(
            "f-inside",
            "inside.txt",
            Some("documents"),
            b"contents",
        )],
        vec!["documents"],
    );
    let files = server.files.clone();
    let tree_hits = server.tree_hits.clone();
    let arrival = server.arrives_on_tree_fetch.clone();
    let delete_folder_hits = server.delete_folder_hits.clone();
    let rig = rig(server).await;
    let engine = build_engine(&rig).await;
    engine.incremental_sync().await.unwrap();
    std::fs::remove_dir_all(rig.root.join("documents")).unwrap();
    engine.incremental_sync().await.unwrap();

    // The upload appears between the run's first tree fetch and the
    // re-check made right before the delete.
    *arrival.lock().await = Some((
        tree_hits.load(Ordering::SeqCst) + 2,
        server_file("f-late", "late.txt", Some("documents"), b"just uploaded"),
    ));
    engine.incremental_sync().await.unwrap();
    assert_eq!(delete_folder_hits.load(Ordering::SeqCst), 0);

    // Later runs see the new file as a server change: the folder stays,
    // the new file is downloaded, only the old file is deleted.
    for _ in 0..3 {
        engine.incremental_sync().await.unwrap();
    }
    assert_eq!(delete_folder_hits.load(Ordering::SeqCst), 0);
    assert_eq!(
        std::fs::read(rig.root.join("documents/late.txt")).unwrap(),
        b"just uploaded"
    );
    let files = files.lock().await;
    assert!(files.contains_key("f-late"));
    assert!(!files.contains_key("f-inside"));
}

// Freeing local space under a strategy that never pushes deletes must not
// turn into server deletions when the strategy later changes back.
#[tokio::test]
async fn regaining_a_delete_pushing_strategy_does_not_delete_freed_items() {
    for (parked, scan_while_parked) in [
        (SyncStrategy::DoNotSync, true),
        (SyncStrategy::DoNotSync, false),
        (SyncStrategy::UploadOnly, true),
    ] {
        let server = FakeServer::with_tree(
            vec![server_file(
                "f-inside",
                "inside.txt",
                Some("documents"),
                b"contents",
            )],
            vec!["documents"],
        );
        let files = server.files.clone();
        let folders = server.folders.clone();
        let delete_file_hits = server.delete_file_hits.clone();
        let delete_folder_hits = server.delete_folder_hits.clone();
        let rig = rig(server).await;
        let engine = build_engine(&rig).await;
        engine.incremental_sync().await.unwrap();

        engine
            .set_folder_local_strategy("documents", Some(parked))
            .await
            .unwrap();
        engine.incremental_sync().await.unwrap();
        std::fs::remove_dir_all(rig.root.join("documents")).unwrap();
        if scan_while_parked {
            engine.incremental_sync().await.unwrap();
        }

        engine
            .set_folder_local_strategy("documents", None)
            .await
            .unwrap();
        for _ in 0..3 {
            let report = engine.incremental_sync().await.unwrap();
            assert!(report.errors.is_empty(), "{:?}", report.errors);
        }
        let label = format!("{parked:?}, scanned while parked: {scan_while_parked}");
        assert_eq!(delete_file_hits.load(Ordering::SeqCst), 0, "{label}");
        assert_eq!(delete_folder_hits.load(Ordering::SeqCst), 0, "{label}");
        assert!(files.lock().await.contains_key("f-inside"), "{label}");
        assert!(folders.lock().await.contains_key("documents"), "{label}");
        assert_eq!(
            std::fs::read(rig.root.join("documents/inside.txt")).unwrap(),
            b"contents",
            "{label}"
        );
    }
}

// A file that can never be downloaded keeps its folders alive, but it must
// not make every local deletion around it bounce back.
#[tokio::test]
async fn undownloadable_file_does_not_undo_deletions_around_it() {
    let server = FakeServer::with_tree(
        vec![
            server_file("f-good", "good.txt", Some("documents"), b"fine"),
            server_file("f-bad", "bad.txt", Some("documents"), b"never arrives"),
        ],
        vec!["documents"],
    );
    server.failed_downloads.lock().await.insert("f-bad".into());
    let files = server.files.clone();
    let folders = server.folders.clone();
    let downloads = server.download_hits.clone();
    let delete_folder_hits = server.delete_folder_hits.clone();
    let rig = rig(server).await;
    let engine = build_engine(&rig).await;
    engine.incremental_sync().await.unwrap();
    assert!(rig.root.join("documents/good.txt").is_file());

    std::fs::remove_dir_all(rig.root.join("documents")).unwrap();
    for _ in 0..3 {
        engine.incremental_sync().await.unwrap();
    }
    assert!(!rig.root.join("documents/good.txt").exists());
    assert!(!files.lock().await.contains_key("f-good"));
    assert!(files.lock().await.contains_key("f-bad"));
    assert!(folders.lock().await.contains_key("documents"));
    assert_eq!(delete_folder_hits.load(Ordering::SeqCst), 0);

    // Only the failing file is retried; the good one is never fetched again.
    let good_fetches = downloads.load(Ordering::SeqCst);
    engine.incremental_sync().await.unwrap();
    assert_eq!(downloads.load(Ordering::SeqCst), good_fetches + 1);
}

#[tokio::test]
async fn surviving_mapped_descendant_clears_parent_delete_pending() {
    let server = FakeServer::with_folders(
        Vec::new(),
        vec![
            ServerFolder::root("parent"),
            ServerFolder {
                id: "child".into(),
                name: "child".into(),
                parent_id: Some("parent".into()),
                updated_at: "2026-01-01T00:00:00Z".into(),
            },
        ],
    );
    let delete_hits = server.delete_folder_hits.clone();
    let rig = rig(server).await;
    let engine = build_engine(&rig).await;
    let child_override = rig.root.join("mapped-child");
    std::fs::create_dir(&child_override).unwrap();
    engine
        .set_folder_local_path("child", Some(child_override.to_str().unwrap()))
        .await
        .unwrap();

    engine.incremental_sync().await.unwrap();
    let parent = rig.root.join("parent");
    assert!(parent.is_dir());
    assert!(child_override.is_dir());
    std::fs::remove_dir(&parent).unwrap();

    let db_url = format!("sqlite://{}", rig.db_path.display());
    let pool = sqlx::SqlitePool::connect(&db_url).await.unwrap();
    sqlx::query(
        "UPDATE sync_state SET delete_pending_since = ? \
         WHERE server_id = 'parent' AND item_type = 'folder'",
    )
    .bind("2026-01-02T00:00:00Z")
    .execute(&pool)
    .await
    .unwrap();

    engine.incremental_sync().await.unwrap();

    let pending: Option<String> = sqlx::query_scalar(
        "SELECT delete_pending_since FROM sync_state \
         WHERE server_id = 'parent' AND item_type = 'folder'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(pending, None, "surviving child must cancel parent deletion");
    assert_eq!(delete_hits.load(Ordering::SeqCst), 0);
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
    assert_eq!(
        journal_status(&rig, "f-1", "file").await.1,
        None,
        "a later deletion must start its own confirmation from scratch"
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
    assert_eq!(
        cleared, 1,
        "exactly one journal row should have been cleared"
    );

    // Now run a sync — pending was cancelled, file is back, no delete.
    let r = engine.incremental_sync().await.unwrap();
    assert!(r.deleted_local.is_empty(), "{:?}", r);
    assert_eq!(delete_hits.load(Ordering::SeqCst), 0);
}

// A path whose state cannot be read is neither deleted nor overwritten,
// and it does not stop the rest of the sync.
#[tokio::test]
async fn unreadable_path_is_skipped_while_the_rest_keeps_syncing() {
    for (operation, populated) in [("is_dir", false), ("is_dir", true), ("is_file", true)] {
        let server = FakeServer::with_folders(
            if populated {
                vec![server_file("file", "file.txt", Some("child"), b"untouched")]
            } else {
                Vec::new()
            },
            vec![
                ServerFolder::root("parent"),
                child_folder("child", "parent"),
            ],
        );
        let files = server.files.clone();
        let folder_deletes = server.delete_folder_hits.clone();
        let file_deletes = server.delete_file_hits.clone();
        let rig = rig(server).await;
        let fs = Arc::new(FaultyFs::default());
        let engine = build_engine_with_fs(&rig, fs.clone()).await;
        engine.incremental_sync().await.unwrap();
        let path = if operation == "is_dir" {
            rig.root.join("parent/child")
        } else {
            rig.root.join("parent/child/file.txt")
        };
        fs.fail(operation, &path);
        files.lock().await.insert(
            "elsewhere".into(),
            server_file("elsewhere", "elsewhere.txt", None, b"unrelated"),
        );

        for _ in 0..3 {
            let report = engine.incremental_sync().await.unwrap();
            assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
            assert!(
                report.errors[0].reason.contains("unreadable"),
                "{:?}",
                report.errors
            );
        }
        assert_eq!(
            std::fs::read(rig.root.join("elsewhere.txt")).unwrap(),
            b"unrelated",
            "an unreadable path must not hold up unrelated items"
        );

        assert_eq!(folder_deletes.load(Ordering::SeqCst), 0);
        assert_eq!(file_deletes.load(Ordering::SeqCst), 0);
        assert_eq!(journal_status(&rig, "child", "folder").await.1, None);
        if populated {
            assert_eq!(journal_status(&rig, "file", "file").await.1, None);
        }
        fs.failures.write().unwrap().clear();
        assert!(engine.incremental_sync().await.unwrap().errors.is_empty());
        if populated {
            assert_eq!(
                std::fs::read(rig.root.join("parent/child/file.txt")).unwrap(),
                b"untouched"
            );
        } else {
            assert!(rig.root.join("parent/child").is_dir());
        }
    }
}

// The activity log gets one row when a path starts failing, not one per run.
#[tokio::test]
async fn unreadable_path_is_logged_once_until_it_recovers() {
    let server = FakeServer::with_tree(
        vec![server_file("file", "file.txt", Some("parent"), b"x")],
        vec!["parent"],
    );
    let rig = rig(server).await;
    let fs = Arc::new(FaultyFs::default());
    let engine = build_engine_with_fs(&rig, fs.clone()).await;
    engine.incremental_sync().await.unwrap();
    let path = rig.root.join("parent/file.txt");
    let error_rows = || async {
        engine
            .recent_sync_log(1000)
            .await
            .unwrap()
            .into_iter()
            .filter(|row| row.operation == "Error")
            .count()
    };

    fs.fail("is_file", &path);
    for _ in 0..3 {
        engine.incremental_sync().await.unwrap();
    }
    assert_eq!(error_rows().await, 1);

    fs.failures.write().unwrap().clear();
    engine.incremental_sync().await.unwrap();
    fs.fail("is_file", &path);
    engine.incremental_sync().await.unwrap();
    assert_eq!(error_rows().await, 2);
}

// An absence is only pushed as a deletion once the backend positively
// confirms nothing is there; a backend that cannot tell keeps the server
// copy.
#[tokio::test]
async fn unconfirmed_absence_is_never_pushed_as_a_deletion() {
    let server = FakeServer::with_tree(
        vec![
            server_file("f-inside", "inside.txt", Some("documents"), b"contents"),
            server_file("f-root", "root.txt", None, b"contents"),
        ],
        vec!["documents"],
    );
    let delete_file_hits = server.delete_file_hits.clone();
    let delete_folder_hits = server.delete_folder_hits.clone();
    let rig = rig(server).await;
    let fs = Arc::new(FaultyFs::default());
    let engine = build_engine_with_fs(&rig, fs.clone()).await;
    engine.incremental_sync().await.unwrap();
    std::fs::remove_dir_all(rig.root.join("documents")).unwrap();
    std::fs::remove_file(rig.root.join("root.txt")).unwrap();
    fs.fail("confirm_absent", &rig.root.join("documents"));
    fs.fail("confirm_absent", &rig.root.join("root.txt"));

    for _ in 0..3 {
        let report = engine.incremental_sync().await.unwrap();
        assert_eq!(delete_file_hits.load(Ordering::SeqCst), 0);
        assert_eq!(delete_folder_hits.load(Ordering::SeqCst), 0);
        let _ = report;
    }

    fs.failures.write().unwrap().clear();
    engine.incremental_sync().await.unwrap();
    assert_eq!(delete_folder_hits.load(Ordering::SeqCst), 1);
    assert_eq!(delete_file_hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn failed_restore_retries_after_restart_instead_of_deleting_remote_file() {
    for failure in ["download", "write", "partial_write", "create_dir_all"] {
        let server = FakeServer::with_folders(
            vec![
                server_file("changed", "changed.txt", Some("parent"), b"original"),
                server_file("sibling-file", "sibling.txt", Some("sibling"), b"unchanged"),
            ],
            vec![
                ServerFolder::root("parent"),
                child_folder("sibling", "parent"),
            ],
        );
        let files = server.files.clone();
        let folders = server.folders.clone();
        let failed_downloads = server.failed_downloads.clone();
        let downloads = server.download_hits.clone();
        let file_deletes = server.delete_file_hits.clone();
        let rig = rig(server).await;
        let fs = Arc::new(FaultyFs::default());
        let engine = build_engine_with_fs(&rig, fs.clone()).await;
        engine.incremental_sync().await.unwrap();
        let parent = rig.root.join("parent");
        std::fs::remove_dir_all(&parent).unwrap();

        // The restore must also cancel an already-pending local subtree deletion.
        engine.incremental_sync().await.unwrap();
        assert!(journal_status(&rig, "parent", "folder").await.1.is_some());
        {
            let mut files = files.lock().await;
            let changed = files.get_mut("changed").unwrap();
            changed.bytes = b"changed remotely".to_vec();
            changed.updated_at = "2026-02-01T00:00:00Z".into();
        }
        match failure {
            "download" => {
                failed_downloads.lock().await.insert("changed".into());
            }
            "write" => fs.fail("write", &parent.join("changed.txt")),
            "partial_write" => fs.fail("partial_write", &parent.join("changed.txt")),
            "create_dir_all" => fs.fail("create_dir_all", &parent),
            _ => unreachable!(),
        }
        let report = engine.incremental_sync().await.unwrap();
        assert!(!report.errors.is_empty(), "{failure}");
        assert_eq!(
            journal_status(&rig, "changed", "file").await,
            ("pending_download".to_owned(), None),
            "{failure}"
        );
        assert_eq!(journal_status(&rig, "parent", "folder").await.1, None);
        drop(engine);

        let engine = build_engine_with_fs(&rig, fs.clone()).await;
        for _ in 0..3 {
            let report = engine.incremental_sync().await.unwrap();
            assert!(!report.errors.is_empty(), "{failure}");
            assert_eq!(file_deletes.load(Ordering::SeqCst), 0, "{failure}");
            assert!(folders.lock().await.contains_key("parent"), "{failure}");
            assert!(files.lock().await.contains_key("changed"), "{failure}");
        }

        failed_downloads.lock().await.clear();
        fs.failures.write().unwrap().clear();
        let recovered = engine.incremental_sync().await.unwrap();
        assert!(recovered.errors.is_empty(), "{:?}", recovered.errors);
        assert_eq!(
            std::fs::read(parent.join("changed.txt")).unwrap(),
            b"changed remotely"
        );
        assert_eq!(
            std::fs::read_dir(&parent).unwrap().count(),
            1,
            "a half-written restore must not surface as a conflict copy ({failure})"
        );
        assert_eq!(
            journal_status(&rig, "changed", "file").await,
            ("synced".to_owned(), None)
        );
        let download_count = downloads.load(Ordering::SeqCst);
        engine.incremental_sync().await.unwrap();
        assert_eq!(
            downloads.load(Ordering::SeqCst),
            download_count,
            "a successful restore must clear the retry state"
        );
        assert_eq!(file_deletes.load(Ordering::SeqCst), 0);
        // The unchanged sibling was part of the local deletion and is gone.
        assert!(!folders.lock().await.contains_key("sibling"));
    }
}

#[tokio::test]
async fn parent_delete_preserves_excluded_descendants() {
    for exclude_before_sync in [false, true] {
        let server = FakeServer::with_folders(
            vec![
                ServerFile {
                    id: "eligible".into(),
                    name: "eligible.txt".into(),
                    parent_id: Some("parent".into()),
                    bytes: b"delete me".to_vec(),
                    updated_at: "2026-01-01T00:00:00Z".into(),
                },
                ServerFile {
                    id: "protected".into(),
                    name: "protected.txt".into(),
                    parent_id: Some("child".into()),
                    bytes: b"keep me".to_vec(),
                    updated_at: "2026-01-01T00:00:00Z".into(),
                },
            ],
            vec![
                ServerFolder::root("parent"),
                ServerFolder {
                    id: "child".into(),
                    name: "child".into(),
                    parent_id: Some("parent".into()),
                    updated_at: "2026-01-01T00:00:00Z".into(),
                },
            ],
        );
        let file_deletes = server.delete_file_hits.clone();
        let folder_deletes = server.delete_folder_hits.clone();
        let files = server.files.clone();
        let folders = server.folders.clone();
        let rig = rig(server).await;
        let fs = Arc::new(FaultyFs::default());
        let engine = build_engine_with_fs(&rig, fs.clone()).await;
        if exclude_before_sync {
            engine
                .set_folder_local_strategy("child", Some(SyncStrategy::DoNotSync))
                .await
                .unwrap();
        }
        engine.incremental_sync().await.unwrap();
        std::fs::remove_dir_all(rig.root.join("parent")).unwrap();
        if !exclude_before_sync {
            engine.incremental_sync().await.unwrap();
            assert!(journal_status(&rig, "parent", "folder").await.1.is_some());
            engine
                .set_folder_local_strategy("child", Some(SyncStrategy::DoNotSync))
                .await
                .unwrap();
        }

        fs.fail("is_dir", &rig.root.join("parent/child"));
        fs.fail("is_file", &rig.root.join("parent/child/protected.txt"));

        for _ in 0..3 {
            let report = engine.incremental_sync().await.unwrap();
            assert!(report.errors.is_empty(), "{:?}", report.errors);
            assert!(
                !rig.root.join("parent").exists(),
                "excluded children must not resurrect the parent"
            );
            assert_eq!(folder_deletes.load(Ordering::SeqCst), 0);
            assert_eq!(journal_status(&rig, "parent", "folder").await.1, None);
        }
        assert!(files.lock().await.contains_key("protected"));
        assert_eq!(
            file_deletes.load(Ordering::SeqCst),
            1,
            "eligible file should still be deleted"
        );
        assert_eq!(folders.lock().await.len(), 2);
    }
}

#[tokio::test]
async fn parent_delete_respects_non_deleting_descendant_strategies() {
    for strategy in [SyncStrategy::UploadOnly, SyncStrategy::ServerToClient] {
        let server = FakeServer::with_folders(
            vec![ServerFile {
                id: "protected".into(),
                name: "protected.txt".into(),
                parent_id: Some("child".into()),
                bytes: b"keep me".to_vec(),
                updated_at: "2026-01-01T00:00:00Z".into(),
            }],
            vec![
                ServerFolder::root("parent"),
                ServerFolder {
                    id: "child".into(),
                    name: "child".into(),
                    parent_id: Some("parent".into()),
                    updated_at: "2026-01-01T00:00:00Z".into(),
                },
            ],
        );
        let files = server.files.clone();
        let file_deletes = server.delete_file_hits.clone();
        let folder_deletes = server.delete_folder_hits.clone();
        let rig = rig(server).await;
        let fs = Arc::new(FaultyFs::default());
        let engine = build_engine_with_fs(&rig, fs.clone()).await;
        engine.incremental_sync().await.unwrap();
        engine
            .set_folder_local_strategy("child", Some(strategy))
            .await
            .unwrap();
        std::fs::remove_dir_all(rig.root.join("parent")).unwrap();
        // A policy barrier must hold even if materializing its directory fails.
        fs.fail("create_dir_all", &rig.root.join("parent/child"));
        for _ in 0..3 {
            let report = engine.incremental_sync().await.unwrap();
            assert!(!report.errors.is_empty());
            assert_eq!(file_deletes.load(Ordering::SeqCst), 0);
            assert_eq!(folder_deletes.load(Ordering::SeqCst), 0);
            assert_eq!(journal_status(&rig, "parent", "folder").await.1, None);
        }
        assert!(files.lock().await.contains_key("protected"));
    }
}

#[tokio::test]
async fn server_to_client_brings_back_a_locally_deleted_file() {
    let server = FakeServer::with_tree(
        vec![server_file(
            "f-inside",
            "inside.txt",
            Some("documents"),
            b"contents",
        )],
        vec!["documents"],
    );
    let delete_file_hits = server.delete_file_hits.clone();
    let delete_folder_hits = server.delete_folder_hits.clone();
    let rig = rig(server).await;
    let engine = build_engine(&rig).await;
    engine
        .set_folder_local_strategy("documents", Some(SyncStrategy::ServerToClient))
        .await
        .unwrap();
    engine.incremental_sync().await.unwrap();

    std::fs::remove_dir_all(rig.root.join("documents")).unwrap();
    for _ in 0..2 {
        let report = engine.incremental_sync().await.unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
    }
    assert_eq!(
        std::fs::read(rig.root.join("documents/inside.txt")).unwrap(),
        b"contents"
    );
    assert_eq!(delete_file_hits.load(Ordering::SeqCst), 0);
    assert_eq!(delete_folder_hits.load(Ordering::SeqCst), 0);
}

// A symlink whose target went away (an unmounted volume, say) looks absent
// to an ordinary probe. It is still something at that path, so the server
// copy stays.
#[cfg(unix)]
#[tokio::test]
async fn dangling_symlink_is_not_pushed_as_a_deletion() {
    let server = FakeServer::with_tree(
        vec![server_file(
            "f-inside",
            "inside.txt",
            Some("documents"),
            b"contents",
        )],
        vec!["documents"],
    );
    let files = server.files.clone();
    let delete_file_hits = server.delete_file_hits.clone();
    let delete_folder_hits = server.delete_folder_hits.clone();
    let rig = rig(server).await;
    let engine = build_engine(&rig).await;
    engine.incremental_sync().await.unwrap();

    let documents = rig.root.join("documents");
    std::fs::remove_dir_all(&documents).unwrap();
    std::os::unix::fs::symlink(rig.root.join("unmounted-volume/documents"), &documents).unwrap();

    engine.incremental_sync().await.unwrap();
    for _ in 0..2 {
        let report = engine.incremental_sync().await.unwrap();
        assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
    }
    assert_eq!(delete_file_hits.load(Ordering::SeqCst), 0);
    assert_eq!(delete_folder_hits.load(Ordering::SeqCst), 0);
    assert!(files.lock().await.contains_key("f-inside"));
}

// The desktop app runs sync inside spawned tasks, so the future has to stay
// `Send` whatever is held across its awaits.
#[tokio::test]
async fn sync_future_is_send() {
    fn assert_send<T: Send>(_: &T) {}
    let rig = rig(FakeServer::with(Vec::new())).await;
    let engine = build_engine(&rig).await;
    let run = engine.incremental_sync();
    assert_send(&run);
    run.await.unwrap();
}
