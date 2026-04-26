//! Abstraction over the local filesystem used by the sync engine.
//!
//! Desktop builds use [`NativeFs`] (wraps `tokio::fs` + `walkdir`). The Android
//! Tauri build wires a SAF-backed implementation from `uncloud-desktop` using
//! `tauri-plugin-android-fs`. The engine holds an `Arc<dyn LocalFs>` so the
//! picker can be swapped at runtime per build target.
//!
//! Paths are plain `String`s — native filesystem paths on desktop, opaque SAF
//! tree/document URIs on Android.

use std::time::UNIX_EPOCH;

use async_trait::async_trait;

/// Error type returned by [`LocalFs`] operations.
#[derive(Debug, thiserror::Error)]
pub enum LocalFsError {
    #[error("io error at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}")]
    Other(String),
}

impl LocalFsError {
    pub fn io(path: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io { path: path.into(), source }
    }
    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }
}

/// An entry returned by [`LocalFs::walk`].
#[derive(Debug, Clone)]
pub struct WalkEntry {
    /// Path relative to the walk root.
    pub rel_path: String,
    /// Modification time in unix seconds.
    pub mtime: i64,
}

/// Bytes-oriented filesystem backend used by [`SyncEngine`](crate::SyncEngine).
///
/// Implementations must be thread-safe (`Send + Sync`) so the engine can hold
/// them behind `Arc<dyn LocalFs>` and share them across async tasks.
#[async_trait]
pub trait LocalFs: Send + Sync {
    /// Recursively walk `root`, yielding every file (not directory) as a
    /// `(rel_path, mtime)` pair.
    async fn walk(&self, root: &str) -> Result<Vec<WalkEntry>, LocalFsError>;

    /// Recursively walk `root`, yielding every subdirectory as a relative
    /// path string. Used by the engine to discover client-side folders that
    /// do not yet exist on the server.
    async fn walk_dirs(&self, root: &str) -> Result<Vec<String>, LocalFsError>;

    /// Ensure the directory at `path` exists, creating parents if needed.
    async fn create_dir_all(&self, path: &str) -> Result<(), LocalFsError>;

    /// Read the entire file at `path` into memory.
    async fn read(&self, path: &str) -> Result<Vec<u8>, LocalFsError>;

    /// Write `data` to `path`, creating or overwriting as needed. Parent
    /// directories are assumed to already exist.
    async fn write(&self, path: &str, data: &[u8]) -> Result<(), LocalFsError>;

    /// Delete the file at `path`. Returns `Ok(())` even if the file does not
    /// exist (idempotent).
    async fn remove_file(&self, path: &str) -> Result<(), LocalFsError>;

    /// Return the mtime of `path` in unix seconds, or `None` if unavailable.
    async fn mtime(&self, path: &str) -> Result<Option<i64>, LocalFsError>;

    /// Return `true` if a regular file exists at `path`.
    async fn is_file(&self, path: &str) -> Result<bool, LocalFsError>;

    /// Join `parent` and `child` using the backend's path separator.
    fn join(&self, parent: &str, child: &str) -> String;
}

// ── NativeFs: desktop backing ────────────────────────────────────────────────

/// `tokio::fs` + `walkdir` backed implementation of [`LocalFs`].
///
/// Used by the desktop Tauri app and by server integration tests.
pub struct NativeFs;

impl NativeFs {
    pub fn new() -> Self { Self }
}

impl Default for NativeFs {
    fn default() -> Self { Self::new() }
}

/// Filenames the walker must never surface regardless of location.
const EXCLUDED_NAMES: &[&str] = &[".uncloud-sync.db"];

#[async_trait]
impl LocalFs for NativeFs {
    async fn walk(&self, root: &str) -> Result<Vec<WalkEntry>, LocalFsError> {
        let root = root.to_owned();
        tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
            for entry in walkdir::WalkDir::new(&root)
                .min_depth(1)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if !entry.file_type().is_file() {
                    continue;
                }
                let file_name = entry.file_name().to_string_lossy();
                if EXCLUDED_NAMES.iter().any(|&n| file_name == n) {
                    continue;
                }
                let rel = entry
                    .path()
                    .strip_prefix(&root)
                    .unwrap_or(entry.path())
                    .to_string_lossy()
                    .into_owned();
                let mtime = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                out.push(WalkEntry { rel_path: rel, mtime });
            }
            Ok(out)
        })
        .await
        .map_err(|e| LocalFsError::other(format!("walk task panicked: {e}")))?
    }

    async fn walk_dirs(&self, root: &str) -> Result<Vec<String>, LocalFsError> {
        let root = root.to_owned();
        tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
            for entry in walkdir::WalkDir::new(&root)
                .min_depth(1)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if !entry.file_type().is_dir() {
                    continue;
                }
                let rel = entry
                    .path()
                    .strip_prefix(&root)
                    .unwrap_or(entry.path())
                    .to_string_lossy()
                    .into_owned();
                out.push(rel);
            }
            Ok(out)
        })
        .await
        .map_err(|e| LocalFsError::other(format!("walk_dirs task panicked: {e}")))?
    }

    async fn create_dir_all(&self, path: &str) -> Result<(), LocalFsError> {
        tokio::fs::create_dir_all(path)
            .await
            .map_err(|e| LocalFsError::io(path, e))
    }

    async fn read(&self, path: &str) -> Result<Vec<u8>, LocalFsError> {
        tokio::fs::read(path)
            .await
            .map_err(|e| LocalFsError::io(path, e))
    }

    async fn write(&self, path: &str, data: &[u8]) -> Result<(), LocalFsError> {
        tokio::fs::write(path, data)
            .await
            .map_err(|e| LocalFsError::io(path, e))
    }

    async fn remove_file(&self, path: &str) -> Result<(), LocalFsError> {
        match tokio::fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(LocalFsError::io(path, e)),
        }
    }

    async fn mtime(&self, path: &str) -> Result<Option<i64>, LocalFsError> {
        let path_owned = path.to_owned();
        tokio::task::spawn_blocking(move || {
            let m = match std::fs::metadata(&path_owned) {
                Ok(m) => m,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(LocalFsError::io(path_owned, e)),
            };
            Ok(m.modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64))
        })
        .await
        .map_err(|e| LocalFsError::other(format!("mtime task panicked: {e}")))?
    }

    async fn is_file(&self, path: &str) -> Result<bool, LocalFsError> {
        let path_owned = path.to_owned();
        tokio::task::spawn_blocking(move || {
            match std::fs::metadata(&path_owned) {
                Ok(m) => Ok(m.is_file()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(e) => Err(LocalFsError::io(path_owned, e)),
            }
        })
        .await
        .map_err(|e| LocalFsError::other(format!("is_file task panicked: {e}")))?
    }

    fn join(&self, parent: &str, child: &str) -> String {
        std::path::Path::new(parent)
            .join(child)
            .to_string_lossy()
            .into_owned()
    }
}
