//! Bridge from Uncloud's async storage backends to rustic's sync `ReadSource`.
//!
//! `UncloudSource` implements `rustic_core::ReadSource`, presenting a virtual
//! tree of:
//!
//! - **Static entries** — DB jsonl dumps and the snapshot manifest, already
//!   on local disk in a staging directory. Read via `std::fs::File`.
//! - **File entries** — one per `File` document, opened by streaming bytes
//!   from the file's `StorageBackend` through an async-to-sync adapter.
//!
//! No full-dataset local staging: each blob streams straight from its
//! backend (Local / S3 / SFTP) into rustic's chunker. Peak local-disk usage
//! is the DB dump plus whatever rustic itself buffers in `staging_dir`.
//!
//! The async-to-sync adapter (`AsyncReadBridge`) calls `Handle::block_on` per
//! `Read::read` invocation. Safe because the entire `repo.backup_with_source`
//! call runs inside `tokio::task::spawn_blocking` — the worker threads
//! rustic spawns are plain OS threads outside the runtime, where
//! `Handle::block_on` is permitted.

use std::io::Read;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use rustic_core::{
    ErrorKind, Metadata, Node, NodeType, ReadSource, ReadSourceEntry, ReadSourceOpen, RusticError,
    RusticResult,
};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::runtime::Handle;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::storage::StorageBackend;

/// Static entry — DB jsonl, manifest, anything already on local disk.
#[derive(Clone, Debug)]
pub struct StaticEntry {
    /// On-disk path the bytes live at right now.
    pub local_path: PathBuf,
    /// Path the snapshot should record. Should be absolute (starts with `/`).
    pub snapshot_path: PathBuf,
    pub size: u64,
}

/// Blob entry — content streamed from a `StorageBackend` at run time.
#[derive(Clone)]
pub struct FileEntry {
    pub backend: Arc<dyn StorageBackend>,
    pub storage_path: String,
    pub snapshot_path: PathBuf,
    pub size: u64,
}

impl std::fmt::Debug for FileEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileEntry")
            .field("storage_path", &self.storage_path)
            .field("snapshot_path", &self.snapshot_path)
            .field("size", &self.size)
            .finish()
    }
}

/// All entries to back up, plus the runtime handle the async-to-sync bridge
/// uses to drive backend reads from rustic's worker threads.
///
/// `failures` counts entries whose `open()` errored out — typically a
/// FileVersion whose archive blob is no longer on the storage backend
/// (left over from a partial migration, a deleted-source workflow, or a
/// decommissioned backend). Read after the rustic backup returns to
/// surface the snapshot as partial.
#[derive(Clone)]
pub struct UncloudSource {
    handle: Handle,
    entries: Arc<Vec<SourceEntry>>,
    total_bytes: u64,
    failures: Arc<AtomicUsize>,
    /// Caps simultaneous backend `read()` calls so SFTP servers with tight
    /// per-session handle limits (Hetzner Storage Box etc.) don't reject
    /// opens once rayon's archiver reaches full parallelism. Each
    /// `UncloudOpen::open()` for a backend entry acquires one permit and
    /// holds it until the resulting `Read` is dropped.
    concurrency: Arc<Semaphore>,
}

#[derive(Clone, Debug)]
enum SourceEntry {
    Static(StaticEntry),
    File(FileEntry),
}

impl SourceEntry {
    fn snapshot_path(&self) -> &std::path::Path {
        match self {
            SourceEntry::Static(s) => &s.snapshot_path,
            SourceEntry::File(f) => &f.snapshot_path,
        }
    }
    fn size(&self) -> u64 {
        match self {
            SourceEntry::Static(s) => s.size,
            SourceEntry::File(f) => f.size,
        }
    }
}

impl UncloudSource {
    /// Build a new source. Entries are sorted by snapshot path so the tree
    /// iterator inside rustic's archiver can synthesise intermediate
    /// directories deterministically.
    pub fn new(
        handle: Handle,
        statics: Vec<StaticEntry>,
        files: Vec<FileEntry>,
        max_concurrent_reads: usize,
    ) -> Self {
        let mut entries: Vec<SourceEntry> = Vec::with_capacity(statics.len() + files.len());
        entries.extend(statics.into_iter().map(SourceEntry::Static));
        entries.extend(files.into_iter().map(SourceEntry::File));
        // Depth-first lexicographic order on paths.
        entries.sort_by(|a, b| a.snapshot_path().cmp(b.snapshot_path()));
        let total_bytes: u64 = entries.iter().map(SourceEntry::size).sum();
        Self {
            handle,
            entries: Arc::new(entries),
            total_bytes,
            failures: Arc::new(AtomicUsize::new(0)),
            concurrency: Arc::new(Semaphore::new(max_concurrent_reads.max(1))),
        }
    }

    /// Cheap clone of the failure counter — caller reads it after the
    /// rustic backup returns to detect partial snapshots.
    pub fn failures(&self) -> Arc<AtomicUsize> {
        self.failures.clone()
    }
}

impl ReadSource for UncloudSource {
    type Open = UncloudOpen;
    type Iter = UncloudIter;

    fn size(&self) -> RusticResult<Option<u64>> {
        Ok(Some(self.total_bytes))
    }

    fn entries(&self) -> Self::Iter {
        UncloudIter {
            handle: self.handle.clone(),
            inner: self.entries.clone(),
            failures: self.failures.clone(),
            concurrency: self.concurrency.clone(),
            idx: 0,
        }
    }
}

pub struct UncloudIter {
    handle: Handle,
    inner: Arc<Vec<SourceEntry>>,
    failures: Arc<AtomicUsize>,
    concurrency: Arc<Semaphore>,
    idx: usize,
}

impl Iterator for UncloudIter {
    type Item = RusticResult<ReadSourceEntry<UncloudOpen>>;

    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.inner.get(self.idx)?;
        self.idx += 1;
        Some(make_entry(&self.handle, &self.failures, &self.concurrency, entry.clone()))
    }
}

fn make_entry(
    handle: &Handle,
    failures: &Arc<AtomicUsize>,
    concurrency: &Arc<Semaphore>,
    entry: SourceEntry,
) -> RusticResult<ReadSourceEntry<UncloudOpen>> {
    let path = match &entry {
        SourceEntry::Static(s) => s.snapshot_path.clone(),
        SourceEntry::File(f) => f.snapshot_path.clone(),
    };
    let name = path.file_name().ok_or_else(|| {
        RusticError::new(
            ErrorKind::Internal,
            "backup source entry has no filename component: `{path}`",
        )
        .attach_context("path", path.display().to_string())
    })?;
    let meta = Metadata {
        size: entry.size(),
        ..Metadata::default()
    };
    let node = Node::new_node(name, NodeType::File, meta);
    let open = match entry {
        SourceEntry::Static(s) => UncloudOpen {
            handle: handle.clone(),
            failures: failures.clone(),
            concurrency: concurrency.clone(),
            kind: OpenKind::Local(s.local_path),
        },
        SourceEntry::File(f) => UncloudOpen {
            handle: handle.clone(),
            failures: failures.clone(),
            concurrency: concurrency.clone(),
            kind: OpenKind::Backend {
                backend: f.backend,
                storage_path: f.storage_path,
            },
        },
    };
    Ok(ReadSourceEntry {
        path,
        node,
        open: Some(open),
    })
}

pub struct UncloudOpen {
    handle: Handle,
    failures: Arc<AtomicUsize>,
    concurrency: Arc<Semaphore>,
    kind: OpenKind,
}

enum OpenKind {
    Local(PathBuf),
    Backend {
        backend: Arc<dyn StorageBackend>,
        storage_path: String,
    },
}

impl ReadSourceOpen for UncloudOpen {
    type Reader = Box<dyn Read + Send + 'static>;

    fn open(self) -> RusticResult<Self::Reader> {
        match self.kind {
            OpenKind::Local(p) => {
                let f = std::fs::File::open(&p).map_err(|e| {
                    self.failures.fetch_add(1, Ordering::Relaxed);
                    RusticError::with_source(
                        ErrorKind::Backend,
                        "Failed to open backup staging file `{path}`",
                        e,
                    )
                    .attach_context("path", p.display().to_string())
                })?;
                Ok(Box::new(f))
            }
            OpenKind::Backend {
                backend,
                storage_path,
            } => {
                // Acquire a permit before the backend.read() so concurrent
                // open file handles against the source backend are capped.
                // Held for the lifetime of the resulting reader.
                let concurrency = self.concurrency.clone();
                let storage_path_for_err = storage_path.clone();
                let (permit, async_reader) = self
                    .handle
                    .block_on(async move {
                        let permit = concurrency
                            .acquire_owned()
                            .await
                            .map_err(|e| std::io::Error::other(format!("semaphore closed: {e}")))?;
                        let reader = backend
                            .read(&storage_path)
                            .await
                            .map_err(|e| std::io::Error::other(format!("{e}")))?;
                        Ok::<_, std::io::Error>((permit, reader))
                    })
                    .map_err(|e| {
                        self.failures.fetch_add(1, Ordering::Relaxed);
                        RusticError::with_source(
                            ErrorKind::Backend,
                            "Failed to open backup blob `{path}`",
                            e,
                        )
                        .attach_context("path", storage_path_for_err)
                    })?;
                Ok(Box::new(GatedReader {
                    _permit: permit,
                    inner: AsyncReadBridge {
                        handle: self.handle,
                        reader: async_reader,
                    },
                }))
            }
        }
    }
}

/// Sync `Read` adapter over an async `tokio::io::AsyncRead`.
///
/// Each `read()` call drives the underlying async reader on the captured
/// runtime handle. Only safe to use from threads that aren't themselves
/// executing the runtime — which is the case inside `spawn_blocking` and
/// the worker threads rustic spawns from there.
pub struct AsyncReadBridge {
    handle: Handle,
    reader: Pin<Box<dyn AsyncRead + Send + Unpin>>,
}

impl Read for AsyncReadBridge {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let reader = self.reader.as_mut();
        self.handle.block_on(async { reader.get_mut().read(buf).await })
    }
}

/// `AsyncReadBridge` plus a permit that's released when the reader is
/// dropped. Used for backend-source reads so the source-storage handle
/// cap is enforced for the entire read lifetime, not just the open call.
struct GatedReader {
    _permit: OwnedSemaphorePermit,
    inner: AsyncReadBridge,
}

impl Read for GatedReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}
