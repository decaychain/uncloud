//! Abstraction over the local filesystem used by the sync engine.
//!
//! Desktop builds use [`NativeFs`] (wraps `tokio::fs` + `walkdir`). The Android
//! Tauri build wires a SAF-backed implementation from `uncloud-desktop` using
//! `tauri-plugin-android-fs`. The engine holds an `Arc<dyn LocalFs>` so the
//! picker can be swapped at runtime per build target.
//!
//! Paths are plain `String`s — native filesystem paths on desktop, opaque SAF
//! tree/document URIs on Android.

use std::io::Read;
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use futures::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// File content as a sequence of chunks, so a transfer costs one chunk of
/// memory whatever the file size.
pub type ByteStream<'a> = BoxStream<'a, Result<Bytes, LocalFsError>>;

const STREAM_CHUNK: usize = 256 * 1024;

/// Suffix of uniquely named internal download staging files.
pub const PARTIAL_SUFFIX: &str = ".uncloud-part";
const PARTIAL_PREFIX: &str = ".uncloud-download-";

/// Error type returned by [`LocalFs`] operations.
#[derive(Debug, thiserror::Error)]
pub enum LocalFsError {
    #[error("io error at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    /// The backend cannot perform this move as a single rename (another
    /// directory on SAF, another volume on desktop). Nothing was changed;
    /// the caller may fall back to copying.
    #[error("{0}")]
    MoveUnsupported(String),
    #[error("{0}")]
    Other(String),
}

impl LocalFsError {
    pub fn io(path: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }
}

/// Lowercase hex SHA-256 of everything `reader` yields, read in fixed-size
/// chunks so the cost in memory does not depend on the file size.
pub fn sha256_hex_reader(mut reader: impl Read) -> std::io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let read = reader.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// The content of an open file as a [`ByteStream`].
pub fn file_chunks(file: std::fs::File) -> ByteStream<'static> {
    futures::stream::unfold(Some(tokio::fs::File::from_std(file)), |state| async move {
        let mut file = state?;
        let mut buf = vec![0u8; STREAM_CHUNK];
        match file.read(&mut buf).await {
            Ok(0) => None,
            Ok(read) => {
                buf.truncate(read);
                Some((Ok(Bytes::from(buf)), Some(file)))
            }
            Err(e) => Some((Err(LocalFsError::other(format!("read failed: {e}"))), None)),
        }
    })
    .boxed()
}

/// Write every chunk of `chunks` to an open file. Stops at the first chunk
/// that is an error, with whatever was written before it left in the file.
pub async fn write_chunks(
    file: std::fs::File,
    mut chunks: ByteStream<'_>,
) -> Result<(), LocalFsError> {
    let mut file = tokio::fs::File::from_std(file);
    while let Some(chunk) = chunks.next().await {
        file.write_all(&chunk?)
            .await
            .map_err(|e| LocalFsError::other(format!("write failed: {e}")))?;
    }
    file.flush()
        .await
        .map_err(|e| LocalFsError::other(format!("write failed: {e}")))
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

    /// The content of the file at `path` in chunks. Backends override this
    /// to read as they go; the default holds the whole file in memory.
    async fn read_stream(&self, path: &str) -> Result<ByteStream<'static>, LocalFsError> {
        let data = self.read(path).await?;
        Ok(futures::stream::once(async move { Ok(Bytes::from(data)) }).boxed())
    }

    /// Write `chunks` to `path`, creating or overwriting it. A stream that
    /// fails part-way must leave whatever was at `path` as it was. Backends
    /// override this to write as they go; the default collects the stream
    /// in memory first.
    async fn write_stream(
        &self,
        path: &str,
        mut chunks: ByteStream<'_>,
    ) -> Result<(), LocalFsError> {
        let mut data = Vec::new();
        while let Some(chunk) = chunks.next().await {
            data.extend_from_slice(&chunk?);
        }
        self.write(path, &data).await
    }

    /// Delete the file at `path`. Returns `Ok(())` even if the file does not
    /// exist (idempotent).
    async fn remove_file(&self, path: &str) -> Result<(), LocalFsError>;

    /// Delete an EMPTY directory at `path`. Returns an error if the directory
    /// is non-empty, so the caller can fall back to a path that preserves
    /// unsynced local contents. Idempotent for the missing case: returns
    /// `Ok(())` if the directory does not exist.
    async fn remove_dir(&self, path: &str) -> Result<(), LocalFsError>;

    /// Called once at the start of every sync run. Backends that cache
    /// observations for the length of a run drop them here.
    fn begin_run(&self) {}

    /// Move an entry without replacing an existing destination. The entry
    /// must end up at exactly `to`: a backend that would store it under a
    /// different name has to undo the move and fail. A move the backend
    /// cannot do as one rename fails with [`LocalFsError::MoveUnsupported`]
    /// and leaves everything in place.
    async fn rename(&self, _from: &str, _to: &str) -> Result<(), LocalFsError> {
        Err(LocalFsError::MoveUnsupported(
            "local filesystem does not support renames".to_owned(),
        ))
    }

    /// Whether two different path strings name the same entry, as they do
    /// for a case-only rename on a case-insensitive filesystem. Only called
    /// for paths the backend reported as existing.
    async fn same_entry(&self, _a: &str, _b: &str) -> Result<bool, LocalFsError> {
        Ok(false)
    }

    /// Copy the file at `from` to `to`, creating or overwriting it. Parent
    /// directories are assumed to exist. Backends override this to stream
    /// and to carry the modification time across where they can.
    async fn copy_file(&self, from: &str, to: &str) -> Result<(), LocalFsError> {
        let data = self.read(from).await?;
        self.write(to, &data).await
    }

    /// Lowercase hex SHA-256 of the file at `path`. Backends override this
    /// to hash in chunks; the default holds the whole file in memory.
    async fn sha256(&self, path: &str) -> Result<String, LocalFsError> {
        let data = self.read(path).await?;
        Ok(format!("{:x}", Sha256::digest(&data)))
    }

    /// Return the mtime of `path` in unix seconds, or `None` if unavailable.
    async fn mtime(&self, path: &str) -> Result<Option<i64>, LocalFsError>;

    /// Return `true` if a regular file exists at `path`.
    async fn is_file(&self, path: &str) -> Result<bool, LocalFsError>;

    /// Return `true` if a directory exists at `path`. Phase 6a uses this
    /// to distinguish "single file deleted" from "user removed the whole
    /// folder," so the latter can be collapsed into one server-side
    /// folder-delete instead of N file-deletes.
    async fn is_dir(&self, path: &str) -> Result<bool, LocalFsError>;

    /// Authoritative absence check, called right before a local absence is
    /// pushed to the server as a deletion. Must return `Ok(true)` only when
    /// the backend positively observed that nothing exists at `path`; a
    /// backend that cannot tell must return an error instead. The default
    /// is only correct for backends whose `is_file` / `is_dir` already
    /// separate "not found" from every other failure.
    async fn confirm_absent(&self, path: &str) -> Result<bool, LocalFsError> {
        Ok(!self.is_file(path).await? && !self.is_dir(path).await?)
    }

    /// Join `parent` and `child` using the backend's path separator.
    fn join(&self, parent: &str, child: &str) -> String;
}

/// A path is missing when it does not exist or when one of its parents is
/// not a directory; both are positive observations, unlike permission or
/// I/O failures.
fn is_missing(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

// ── NativeFs: desktop backing ────────────────────────────────────────────────

/// `tokio::fs` + `walkdir` backed implementation of [`LocalFs`].
///
/// Used by the desktop Tauri app and by server integration tests.
pub struct NativeFs;

impl NativeFs {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NativeFs {
    fn default() -> Self {
        Self::new()
    }
}

/// Filenames any [`LocalFs`] walker must never yield. These are never
/// uploaded, never compared against the journal, never treated as
/// local-only files. Re-exported from the crate root so out-of-tree
/// `LocalFs` impls (e.g. the Android SAF walker in `uncloud-desktop`)
/// can apply the same filter and stay in lock-step. Keep in sync with
/// [`crate::sentinel::SENTINEL_FILENAME`].
pub const EXCLUDED_NAMES: &[&str] = &[".uncloud-sync.db", ".uncloud-root.json"];

/// Whether a walker must skip a file with this name: one of
/// [`EXCLUDED_NAMES`], or the leftover of an interrupted download.
pub fn is_excluded_name(name: &str) -> bool {
    EXCLUDED_NAMES.contains(&name)
        || name
            .strip_prefix(PARTIAL_PREFIX)
            .and_then(|name| name.strip_suffix(PARTIAL_SUFFIX))
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
}

#[async_trait]
impl LocalFs for NativeFs {
    async fn rename(&self, from: &str, to: &str) -> Result<(), LocalFsError> {
        if !self.confirm_absent(to).await? && !self.same_entry(from, to).await? {
            return Err(LocalFsError::other(format!(
                "rename destination exists: {to}"
            )));
        }
        match tokio::fs::rename(from, to).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => Err(
                LocalFsError::MoveUnsupported(format!("{from} and {to} are on different volumes")),
            ),
            Err(e) => Err(LocalFsError::io(from, e)),
        }
    }

    // Two spellings of one directory resolve to the same canonical path. A
    // symlink pointing at the other path would too, and is not the same
    // entry, so links are ruled out first.
    async fn same_entry(&self, a: &str, b: &str) -> Result<bool, LocalFsError> {
        let (a, b) = (a.to_owned(), b.to_owned());
        tokio::task::spawn_blocking(move || {
            for path in [&a, &b] {
                match std::fs::symlink_metadata(path) {
                    Ok(meta) if meta.file_type().is_symlink() => return Ok(false),
                    Ok(_) => {}
                    Err(e) if is_missing(&e) => return Ok(false),
                    Err(e) => return Err(LocalFsError::io(path.clone(), e)),
                }
            }
            let canonical = |path: &String| {
                std::fs::canonicalize(path).map_err(|e| LocalFsError::io(path.clone(), e))
            };
            Ok(canonical(&a)? == canonical(&b)?)
        })
        .await
        .map_err(|e| LocalFsError::other(format!("same_entry task panicked: {e}")))?
    }

    // The engine compares local mtimes with the journal to spot local
    // edits, so a copy that reset them would make every file look changed.
    async fn copy_file(&self, from: &str, to: &str) -> Result<(), LocalFsError> {
        let (from, to) = (from.to_owned(), to.to_owned());
        tokio::task::spawn_blocking(move || {
            let modified = std::fs::metadata(&from)
                .and_then(|meta| meta.modified())
                .map_err(|e| LocalFsError::io(from.clone(), e))?;
            std::fs::copy(&from, &to).map_err(|e| LocalFsError::io(from.clone(), e))?;
            std::fs::File::options()
                .write(true)
                .open(&to)
                .and_then(|file| file.set_modified(modified))
                .map_err(|e| LocalFsError::io(to.clone(), e))
        })
        .await
        .map_err(|e| LocalFsError::other(format!("copy_file task panicked: {e}")))?
    }

    async fn sha256(&self, path: &str) -> Result<String, LocalFsError> {
        let path = path.to_owned();
        tokio::task::spawn_blocking(move || {
            std::fs::File::open(&path)
                .and_then(sha256_hex_reader)
                .map_err(|e| LocalFsError::io(path.clone(), e))
        })
        .await
        .map_err(|e| LocalFsError::other(format!("sha256 task panicked: {e}")))?
    }

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
                if is_excluded_name(&entry.file_name().to_string_lossy()) {
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
                out.push(WalkEntry {
                    rel_path: rel,
                    mtime,
                });
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

    async fn read_stream(&self, path: &str) -> Result<ByteStream<'static>, LocalFsError> {
        let file = tokio::fs::File::open(path)
            .await
            .map_err(|e| LocalFsError::io(path, e))?;
        Ok(file_chunks(file.into_std().await))
    }

    // The chunks go to a temporary file beside the destination, which then
    // replaces it in one rename. A download that dies half-way therefore
    // never leaves a truncated file where a complete one was, to be taken
    // for a local edit on the next scan.
    async fn write_stream(&self, path: &str, chunks: ByteStream<'_>) -> Result<(), LocalFsError> {
        let parent = std::path::Path::new(path)
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        let (partial, file) = loop {
            let partial = parent.join(format!(
                "{PARTIAL_PREFIX}{}{PARTIAL_SUFFIX}",
                uuid::Uuid::new_v4()
            ));
            match tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&partial)
                .await
            {
                Ok(file) => break (partial, file),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(LocalFsError::io(partial.to_string_lossy(), e)),
            }
        };
        let written = async {
            write_chunks(file.into_std().await, chunks).await?;
            tokio::fs::rename(&partial, path)
                .await
                .map_err(|e| LocalFsError::io(path, e))
        }
        .await;
        if written.is_err() {
            let _ = tokio::fs::remove_file(&partial).await;
        }
        written
    }

    async fn remove_file(&self, path: &str) -> Result<(), LocalFsError> {
        match tokio::fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(LocalFsError::io(path, e)),
        }
    }

    async fn remove_dir(&self, path: &str) -> Result<(), LocalFsError> {
        match tokio::fs::remove_dir(path).await {
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
        tokio::task::spawn_blocking(move || match std::fs::metadata(&path_owned) {
            Ok(m) => Ok(m.is_file()),
            Err(e) if is_missing(&e) => Ok(false),
            Err(e) => Err(LocalFsError::io(path_owned, e)),
        })
        .await
        .map_err(|e| LocalFsError::other(format!("is_file task panicked: {e}")))?
    }

    async fn is_dir(&self, path: &str) -> Result<bool, LocalFsError> {
        let path_owned = path.to_owned();
        tokio::task::spawn_blocking(move || match std::fs::metadata(&path_owned) {
            Ok(m) => Ok(m.is_dir()),
            Err(e) if is_missing(&e) => Ok(false),
            Err(e) => Err(LocalFsError::io(path_owned, e)),
        })
        .await
        .map_err(|e| LocalFsError::other(format!("is_dir task panicked: {e}")))?
    }

    // `symlink_metadata` does not follow links, so a dangling symlink (its
    // target on an unmounted volume, say) still counts as something being
    // there instead of reading as a deletion.
    async fn confirm_absent(&self, path: &str) -> Result<bool, LocalFsError> {
        let path_owned = path.to_owned();
        tokio::task::spawn_blocking(move || match std::fs::symlink_metadata(&path_owned) {
            Ok(_) => Ok(false),
            Err(e) if is_missing(&e) => Ok(true),
            Err(e) => Err(LocalFsError::io(path_owned, e)),
        })
        .await
        .map_err(|e| LocalFsError::other(format!("confirm_absent task panicked: {e}")))?
    }

    fn join(&self, parent: &str, child: &str) -> String {
        std::path::Path::new(parent)
            .join(child)
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn sha256_hex_reader_hashes_across_chunk_boundaries() {
        assert_eq!(sha256_hex_reader(&b"abc"[..]).unwrap(), ABC_SHA256);
        let large = vec![7u8; (1 << 20) * 2 + 13];
        assert_eq!(
            sha256_hex_reader(&large[..]).unwrap(),
            format!("{:x}", Sha256::digest(&large))
        );
    }

    #[tokio::test]
    async fn native_sha256_matches_the_whole_file_digest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("abc.txt");
        std::fs::write(&path, b"abc").unwrap();
        assert_eq!(
            NativeFs.sha256(path.to_str().unwrap()).await.unwrap(),
            ABC_SHA256
        );
    }

    fn chunks_of(parts: Vec<Result<&'static [u8], &'static str>>) -> ByteStream<'static> {
        futures::stream::iter(parts.into_iter().map(|part| {
            part.map(Bytes::from_static)
                .map_err(|message| LocalFsError::other(message))
        }))
        .boxed()
    }

    #[tokio::test]
    async fn native_read_stream_yields_the_file_in_bounded_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.bin");
        let content: Vec<u8> = (0..STREAM_CHUNK * 2 + 123)
            .map(|i| (i % 251) as u8)
            .collect();
        std::fs::write(&path, &content).unwrap();

        let mut stream = NativeFs.read_stream(path.to_str().unwrap()).await.unwrap();
        let mut read = Vec::new();
        let mut chunks = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.unwrap();
            assert!(chunk.len() <= STREAM_CHUNK);
            read.extend_from_slice(&chunk);
            chunks += 1;
        }
        assert!(chunks >= 3);
        assert_eq!(read, content);
    }

    #[tokio::test]
    async fn native_write_stream_replaces_the_file_only_when_complete() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.txt");
        std::fs::write(&path, b"original").unwrap();
        let fs = NativeFs;
        let target = path.to_str().unwrap();
        let names = || {
            let mut names: Vec<String> = std::fs::read_dir(dir.path())
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        };

        let cut_off = chunks_of(vec![Ok(b"new con"), Err("connection lost")]);
        assert!(fs.write_stream(target, cut_off).await.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert_eq!(names(), ["note.txt"]);

        let complete = chunks_of(vec![Ok(b"new con"), Ok(b"tent")]);
        fs.write_stream(target, complete).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new content");
        assert_eq!(names(), ["note.txt"]);
    }

    #[tokio::test]
    async fn downloads_preserve_user_files_with_the_old_staging_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("note.txt");
        let neighbor = dir.path().join("note.txt.uncloud-part");
        std::fs::write(&neighbor, b"unrelated local data").unwrap();
        for fail in [true, false] {
            let chunks = if fail {
                chunks_of(vec![Ok(b"partial"), Err("connection lost")])
            } else {
                chunks_of(vec![Ok(b"download")])
            };
            assert_eq!(
                NativeFs
                    .write_stream(target.to_str().unwrap(), chunks)
                    .await
                    .is_err(),
                fail
            );
            assert_eq!(std::fs::read(&neighbor).unwrap(), b"unrelated local data");
        }
        assert_eq!(std::fs::read(&target).unwrap(), b"download");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn download_staging_does_not_follow_a_neighboring_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("note.txt");
        let other = dir.path().join("other.txt");
        std::fs::write(&other, b"keep me").unwrap();
        let link = dir.path().join("note.txt.uncloud-part");
        std::os::unix::fs::symlink(&other, &link).unwrap();
        NativeFs
            .write_stream(target.to_str().unwrap(), chunks_of(vec![Ok(b"download")]))
            .await
            .unwrap();
        assert_eq!(std::fs::read(&other).unwrap(), b"keep me");
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
    }

    #[tokio::test]
    async fn walk_skips_the_leftover_of_an_interrupted_download() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("note.txt"), b"content").unwrap();
        std::fs::write(
            dir.path().join(format!(
                "{PARTIAL_PREFIX}{}{PARTIAL_SUFFIX}",
                uuid::Uuid::new_v4()
            )),
            b"half",
        )
        .unwrap();
        std::fs::write(dir.path().join("user.uncloud-part"), b"user data").unwrap();
        std::fs::write(
            dir.path().join(".uncloud-download-not-a-uuid.uncloud-part"),
            b"also user data",
        )
        .unwrap();
        let mut listed: Vec<String> = NativeFs
            .walk(dir.path().to_str().unwrap())
            .await
            .unwrap()
            .into_iter()
            .map(|entry| entry.rel_path)
            .collect();
        listed.sort();
        assert_eq!(
            listed,
            [
                ".uncloud-download-not-a-uuid.uncloud-part",
                "note.txt",
                "user.uncloud-part"
            ]
        );
    }

    #[tokio::test]
    async fn native_copy_file_carries_content_and_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("from.txt");
        let to = dir.path().join("to.txt");
        std::fs::write(&from, b"content").unwrap();
        let old = SystemTime::now() - Duration::from_secs(86_400);
        std::fs::File::options()
            .write(true)
            .open(&from)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let fs = NativeFs;
        fs.copy_file(from.to_str().unwrap(), to.to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"content");
        assert_eq!(
            fs.mtime(to.to_str().unwrap()).await.unwrap(),
            fs.mtime(from.to_str().unwrap()).await.unwrap()
        );
    }

    #[tokio::test]
    async fn native_rename_refuses_an_existing_destination() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("from");
        let to = dir.path().join("to");
        std::fs::create_dir(&from).unwrap();
        std::fs::create_dir(&to).unwrap();
        let fs = NativeFs;
        let (from, to) = (from.to_str().unwrap(), to.to_str().unwrap());
        assert!(!fs.same_entry(from, to).await.unwrap());
        assert!(fs.rename(from, to).await.is_err());
        assert!(fs.is_dir(from).await.unwrap());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlink_to_a_directory_is_not_the_same_entry() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        let link = dir.path().join("link");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(!NativeFs
            .same_entry(real.to_str().unwrap(), link.to_str().unwrap())
            .await
            .unwrap());
    }
}
