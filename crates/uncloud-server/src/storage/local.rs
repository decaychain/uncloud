use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tokio::fs::{self, File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader};
use uuid::Uuid;

use super::{BoxedAsyncRead, ScanEntry, StorageBackend};
use crate::error::{AppError, Result};

pub struct LocalStorage {
    base_path: PathBuf,
    temp_path: PathBuf,
}

impl LocalStorage {
    pub async fn new(base_path: impl AsRef<Path>) -> Result<Self> {
        let base_path = base_path.as_ref().to_path_buf();
        let temp_path = base_path.join(".tmp");

        // Ensure directories exist
        fs::create_dir_all(&base_path)
            .await
            .map_err(|e| AppError::Storage(format!("Failed to create storage directory: {}", e)))?;
        fs::create_dir_all(&temp_path)
            .await
            .map_err(|e| AppError::Storage(format!("Failed to create temp directory: {}", e)))?;

        Ok(Self {
            base_path,
            temp_path,
        })
    }

    fn resolve_path(&self, path: &str) -> PathBuf {
        self.base_path.join(path.trim_start_matches('/'))
    }

    fn resolve_temp_path(&self, path: &str) -> PathBuf {
        self.temp_path.join(path.trim_start_matches('/'))
    }

    async fn ensure_parent_dir(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| AppError::Storage(format!("Failed to create directory: {}", e)))?;
        }
        Ok(())
    }
}

#[async_trait]
impl StorageBackend for LocalStorage {
    async fn read(&self, path: &str) -> Result<BoxedAsyncRead> {
        let full_path = self.resolve_path(path);
        let file = File::open(&full_path)
            .await
            .map_err(|e| AppError::Storage(format!("Failed to open file: {}", e)))?;
        Ok(Box::pin(BufReader::new(file)))
    }

    async fn read_range(&self, path: &str, offset: u64, length: u64) -> Result<BoxedAsyncRead> {
        let full_path = self.resolve_path(path);
        let mut file = File::open(&full_path)
            .await
            .map_err(|e| AppError::Storage(format!("Failed to open file: {}", e)))?;
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|e| AppError::Storage(format!("Failed to seek in file: {}", e)))?;
        Ok(Box::pin(file.take(length)))
    }

    async fn write(&self, path: &str, data: &[u8]) -> Result<()> {
        let full_path = self.resolve_path(path);
        self.ensure_parent_dir(&full_path).await?;

        let mut file = File::create(&full_path)
            .await
            .map_err(|e| AppError::Storage(format!("Failed to create file: {}", e)))?;

        file.write_all(data)
            .await
            .map_err(|e| AppError::Storage(format!("Failed to write file: {}", e)))?;

        file.sync_all()
            .await
            .map_err(|e| AppError::Storage(format!("Failed to sync file: {}", e)))?;

        Ok(())
    }

    async fn write_stream(&self, path: &str, mut reader: BoxedAsyncRead, _size: u64) -> Result<()> {
        let full_path = self.resolve_path(path);
        self.ensure_parent_dir(&full_path).await?;

        let mut file = File::create(&full_path)
            .await
            .map_err(|e| AppError::Storage(format!("Failed to create file: {}", e)))?;

        let mut buffer = vec![0u8; 64 * 1024]; // 64KB buffer
        loop {
            let n = reader
                .read(&mut buffer)
                .await
                .map_err(|e| AppError::Storage(format!("Failed to read stream: {}", e)))?;

            if n == 0 {
                break;
            }

            file.write_all(&buffer[..n])
                .await
                .map_err(|e| AppError::Storage(format!("Failed to write file: {}", e)))?;
        }

        file.sync_all()
            .await
            .map_err(|e| AppError::Storage(format!("Failed to sync file: {}", e)))?;

        Ok(())
    }

    async fn delete(&self, path: &str) -> Result<()> {
        let full_path = self.resolve_path(path);
        if full_path.exists() {
            fs::remove_file(&full_path)
                .await
                .map_err(|e| AppError::Storage(format!("Failed to delete file: {}", e)))?;
        }
        Ok(())
    }

    async fn exists(&self, path: &str) -> Result<bool> {
        let full_path = self.resolve_path(path);
        Ok(full_path.exists())
    }

    async fn available_space(&self) -> Result<Option<u64>> {
        // Use statvfs on Unix systems
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let path_bytes = self.base_path.as_os_str().as_bytes();
            let mut path_cstr = path_bytes.to_vec();
            path_cstr.push(0);

            unsafe {
                let mut stat: libc::statvfs = std::mem::zeroed();
                if libc::statvfs(path_cstr.as_ptr() as *const libc::c_char, &mut stat) == 0 {
                    return Ok(Some(stat.f_bavail as u64 * stat.f_frsize as u64));
                }
            }
        }
        Ok(None)
    }

    async fn create_temp(&self) -> Result<String> {
        let temp_name = format!("{}.tmp", Uuid::new_v4());
        let temp_path = self.temp_path.join(&temp_name);

        File::create(&temp_path)
            .await
            .map_err(|e| AppError::Storage(format!("Failed to create temp file: {}", e)))?;

        Ok(temp_name)
    }

    async fn append_temp(&self, temp_path: &str, data: &[u8]) -> Result<()> {
        let full_path = self.resolve_temp_path(temp_path);

        let mut file = OpenOptions::new()
            .append(true)
            .open(&full_path)
            .await
            .map_err(|e| AppError::Storage(format!("Failed to open temp file: {}", e)))?;

        file.write_all(data)
            .await
            .map_err(|e| AppError::Storage(format!("Failed to append to temp file: {}", e)))?;

        Ok(())
    }

    async fn finalize_temp(&self, temp_path: &str, final_path: &str) -> Result<()> {
        let temp_full = self.resolve_temp_path(temp_path);
        let final_full = self.resolve_path(final_path);

        self.ensure_parent_dir(&final_full).await?;

        fs::rename(&temp_full, &final_full)
            .await
            .map_err(|e| AppError::Storage(format!("Failed to finalize temp file: {}", e)))?;

        Ok(())
    }

    async fn abort_temp(&self, temp_path: &str) -> Result<()> {
        let full_path = self.resolve_temp_path(temp_path);
        if full_path.exists() {
            fs::remove_file(&full_path)
                .await
                .map_err(|e| AppError::Storage(format!("Failed to abort temp file: {}", e)))?;
        }
        Ok(())
    }

    async fn rename(&self, from: &str, to: &str) -> Result<()> {
        let from_full = self.resolve_path(from);
        let to_full = self.resolve_path(to);
        self.ensure_parent_dir(&to_full).await?;
        fs::rename(&from_full, &to_full)
            .await
            .map_err(|e| AppError::Storage(format!("Failed to rename file: {}", e)))?;
        Ok(())
    }

    async fn archive_version(&self, current: &str, version: &str) -> Result<()> {
        let from = self.resolve_path(current);
        let to = self.resolve_path(version);
        self.ensure_parent_dir(&to).await?;
        fs::copy(&from, &to)
            .await
            .map_err(|e| AppError::Storage(format!("Failed to archive version: {}", e)))?;
        Ok(())
    }

    async fn move_to_trash(&self, current: &str, trash: &str) -> Result<()> {
        self.rename(current, trash).await
    }

    async fn restore_from_trash(&self, trash: &str, restore: &str) -> Result<()> {
        self.rename(trash, restore).await
    }

    async fn scan(&self, prefix: &str) -> Result<Vec<ScanEntry>> {
        let root = self.resolve_path(prefix);
        if !root.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let mut stack: Vec<(PathBuf, String)> = vec![(root, prefix.trim_start_matches('/').to_string())];

        while let Some((dir, rel)) = stack.pop() {
            let mut entries = match fs::read_dir(&dir).await {
                Ok(e) => e,
                Err(e) => {
                    return Err(AppError::Storage(format!(
                        "Failed to read directory {:?}: {}",
                        dir, e
                    )));
                }
            };
            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|e| AppError::Storage(format!("Failed to iterate dir: {}", e)))?
            {
                let name = entry.file_name().to_string_lossy().to_string();
                let child_rel = if rel.is_empty() {
                    name.clone()
                } else {
                    format!("{}/{}", rel, name)
                };
                let meta = match entry.metadata().await {
                    Ok(m) => m,
                    Err(_) => continue, // skip unreadable entries
                };
                if meta.is_dir() {
                    out.push(ScanEntry {
                        path: child_rel.clone(),
                        is_dir: true,
                        size_bytes: 0,
                    });
                    stack.push((entry.path(), child_rel));
                } else if meta.is_file() {
                    out.push(ScanEntry {
                        path: child_rel,
                        is_dir: false,
                        size_bytes: meta.len(),
                    });
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    /// Helper: create a LocalStorage backed by a tempdir and write a test file.
    async fn setup_with_file(
        content: &[u8],
    ) -> (LocalStorage, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("failed to create tempdir");
        let storage = LocalStorage::new(tmp.path()).await.unwrap();
        storage.write("test.bin", content).await.unwrap();
        (storage, tmp)
    }

    #[tokio::test]
    async fn read_range_from_start() {
        let data = b"Hello, World!";
        let (storage, _tmp) = setup_with_file(data).await;

        let mut reader = storage.read_range("test.bin", 0, 5).await.unwrap();
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.unwrap();
        assert_eq!(&buf, b"Hello");
    }

    #[tokio::test]
    async fn read_range_from_middle() {
        let data = b"Hello, World!";
        let (storage, _tmp) = setup_with_file(data).await;

        let mut reader = storage.read_range("test.bin", 7, 5).await.unwrap();
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.unwrap();
        assert_eq!(&buf, b"World");
    }

    #[tokio::test]
    async fn read_range_single_byte() {
        let data = b"ABCDE";
        let (storage, _tmp) = setup_with_file(data).await;

        let mut reader = storage.read_range("test.bin", 2, 1).await.unwrap();
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.unwrap();
        assert_eq!(&buf, b"C");
    }

    #[tokio::test]
    async fn read_range_entire_file() {
        let data = b"full content";
        let (storage, _tmp) = setup_with_file(data).await;

        let mut reader = storage
            .read_range("test.bin", 0, data.len() as u64)
            .await
            .unwrap();
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.unwrap();
        assert_eq!(&buf, data.as_slice());
    }

    #[tokio::test]
    async fn read_range_length_clamped_by_eof() {
        // Requesting more bytes than remain should return only what's available
        let data = b"short";
        let (storage, _tmp) = setup_with_file(data).await;

        let mut reader = storage.read_range("test.bin", 3, 100).await.unwrap();
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.unwrap();
        // take(100) will stop at EOF, yielding only 2 bytes ("rt")
        assert_eq!(&buf, b"rt");
    }

    #[tokio::test]
    async fn read_range_nonexistent_file() {
        let tmp = tempfile::tempdir().unwrap();
        let storage = LocalStorage::new(tmp.path()).await.unwrap();

        let result = storage.read_range("no-such-file.bin", 0, 10).await;
        assert!(result.is_err(), "expected error for missing file");
    }
}
