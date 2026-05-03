pub mod local;
pub mod retry;
pub mod s3;
pub mod sftp;

use async_trait::async_trait;
use std::pin::Pin;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::error::Result;

pub type BoxedAsyncRead = Pin<Box<dyn AsyncRead + Send + Unpin>>;
pub type BoxedAsyncWrite = Pin<Box<dyn AsyncWrite + Send + Unpin>>;

/// One on-disk entry surfaced by `StorageBackend::scan`.
#[derive(Debug, Clone)]
pub struct ScanEntry {
    /// Path relative to the backend root, using `/` separators.
    pub path: String,
    pub is_dir: bool,
    pub size_bytes: u64,
}

#[async_trait]
pub trait StorageBackend: Send + Sync {
    /// Read a file, returning an async reader
    async fn read(&self, path: &str) -> Result<BoxedAsyncRead>;

    /// Read a byte range from a file, returning an async reader that yields
    /// exactly `length` bytes starting at `offset`.
    async fn read_range(&self, path: &str, offset: u64, length: u64) -> Result<BoxedAsyncRead>;

    /// Write to a file from bytes
    async fn write(&self, path: &str, data: &[u8]) -> Result<()>;

    /// Write to a file from an async reader
    async fn write_stream(&self, path: &str, reader: BoxedAsyncRead, size: u64) -> Result<()>;

    /// Delete a file
    async fn delete(&self, path: &str) -> Result<()>;

    /// Check if path exists
    async fn exists(&self, path: &str) -> Result<bool>;

    /// Get available space in bytes (None if unknown/unlimited)
    async fn available_space(&self) -> Result<Option<u64>>;

    /// Create a temporary file for chunked uploads, returns temp path
    async fn create_temp(&self) -> Result<String>;

    /// Append to temporary file
    async fn append_temp(&self, temp_path: &str, data: &[u8]) -> Result<()>;

    /// Finalize temp file to permanent location
    async fn finalize_temp(&self, temp_path: &str, final_path: &str) -> Result<()>;

    /// Abort and cleanup temp file
    async fn abort_temp(&self, temp_path: &str) -> Result<()>;

    /// Rename / move a file within the same backend. Creates parent dirs as needed.
    async fn rename(&self, from: &str, to: &str) -> Result<()>;

    /// Copy the current blob to a version archive path, leaving the original in place.
    async fn archive_version(&self, current: &str, version: &str) -> Result<()>;

    /// Move a file blob into the trash directory.
    async fn move_to_trash(&self, current: &str, trash: &str) -> Result<()>;

    /// Restore a file blob from the trash directory.
    async fn restore_from_trash(&self, trash: &str, restore: &str) -> Result<()>;

    /// Recursively enumerate files and directories under `prefix` (relative to
    /// backend root). Used by the admin rescan/import endpoint. `prefix` of
    /// `""` means the entire backend. Returned paths are relative to the
    /// backend root.
    async fn scan(&self, prefix: &str) -> Result<Vec<ScanEntry>>;
}

pub use local::LocalStorage;
pub use s3::S3Storage;
pub use sftp::SftpStorage;
