use mongodb::bson::oid::ObjectId;
use sha2::{Digest, Sha256};
use std::sync::Arc;

use super::StorageBackend;
use crate::error::{AppError, Result};

/// Request bodies arrive in frames of a few KB, and every `append_temp` is an
/// open/write/close on the backend (several network round trips on SFTP), so
/// frames are coalesced up to this size first. An in-flight upload holds at
/// most this much, plus the body frame being processed.
const APPEND_BUFFER_BYTES: usize = 4 * 1024 * 1024;

/// Owns a backend temp file until it is finalized. Dropping it while still
/// armed removes the temp, which is what cleans up after an early return or
/// after the handler future is cancelled because the client went away.
struct TempFile {
    backend: Arc<dyn StorageBackend>,
    path: Option<String>,
}

impl TempFile {
    async fn abort(&mut self) {
        if let Some(path) = self.path.take() {
            if let Err(e) = self.backend.abort_temp(&path).await {
                tracing::warn!("Failed to remove upload temp `{path}`: {e}");
            }
        }
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let Some(path) = self.path.take() else { return };
        let backend = self.backend.clone();
        // Cleanup is async and Drop is not. Outside a runtime there is nothing
        // to run it on, and the temp is left for the operator.
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            tracing::warn!("Upload temp `{path}` dropped outside a runtime; not removed");
            return;
        };
        handle.spawn(async move {
            if let Err(e) = backend.abort_temp(&path).await {
                tracing::warn!("Failed to remove upload temp `{path}`: {e}");
            }
        });
    }
}

/// An upload being streamed into a backend temp file, hashed as it goes.
pub struct UploadStaging {
    temp: TempFile,
    storage_id: ObjectId,
    hasher: Sha256,
    size: u64,
    pending: Vec<u8>,
}

impl UploadStaging {
    pub async fn create(storage_id: ObjectId, backend: Arc<dyn StorageBackend>) -> Result<Self> {
        let path = backend.create_temp().await?;
        Ok(Self {
            temp: TempFile {
                backend,
                path: Some(path),
            },
            storage_id,
            hasher: Sha256::new(),
            size: 0,
            pending: Vec::new(),
        })
    }

    pub async fn write(&mut self, data: &[u8]) -> Result<()> {
        self.hasher.update(data);
        self.size += data.len() as u64;
        if self.pending.len() + data.len() > APPEND_BUFFER_BYTES {
            self.flush().await?;
        }
        if data.len() >= APPEND_BUFFER_BYTES {
            return self.append(data).await;
        }
        self.pending.extend_from_slice(data);
        Ok(())
    }

    async fn append(&self, data: &[u8]) -> Result<()> {
        let path = self
            .temp
            .path
            .as_deref()
            .ok_or_else(|| AppError::Internal("Upload temp already released".to_string()))?;
        self.temp.backend.append_temp(path, data).await
    }

    async fn flush(&mut self) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        self.append(&self.pending).await?;
        self.pending.clear();
        Ok(())
    }

    pub async fn finish(mut self) -> Result<StagedUpload> {
        self.flush().await?;
        Ok(StagedUpload {
            temp: self.temp,
            storage_id: self.storage_id,
            size: self.size,
            checksum: hex::encode(self.hasher.finalize()),
        })
    }
}

/// A fully received upload sitting in a backend temp file, waiting for its
/// final path.
pub struct StagedUpload {
    temp: TempFile,
    storage_id: ObjectId,
    size: u64,
    checksum: String,
}

impl StagedUpload {
    pub fn size_bytes(&self) -> i64 {
        self.size as i64
    }

    pub fn checksum_sha256(&self) -> &str {
        &self.checksum
    }

    /// Move the staged bytes to `final_path` on the storage `storage_id`.
    ///
    /// The destination storage can differ from the one the upload was staged
    /// on: a multipart upload names its parent folder after the file part, so
    /// the folder's pinned storage is not known while the body streams. In
    /// that case the temp is copied across and then removed.
    pub async fn commit(
        mut self,
        storage_id: ObjectId,
        backend: &Arc<dyn StorageBackend>,
        final_path: &str,
    ) -> Result<()> {
        let temp_path = self
            .temp
            .path
            .clone()
            .ok_or_else(|| AppError::Internal("Upload temp already released".to_string()))?;

        if storage_id == self.storage_id {
            let result = self
                .temp
                .backend
                .finalize_temp(&temp_path, final_path)
                .await;
            match result {
                Ok(()) => self.temp.path = None,
                Err(_) => self.temp.abort().await,
            }
            return result;
        }

        let result = match self.temp.backend.read_temp(&temp_path).await {
            Ok(reader) => backend.write_stream(final_path, reader, self.size).await,
            Err(e) => Err(e),
        };
        self.temp.abort().await;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::LocalStorage;

    async fn local() -> (Arc<dyn StorageBackend>, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend: Arc<dyn StorageBackend> =
            Arc::new(LocalStorage::new(dir.path()).await.expect("local storage"));
        (backend, dir)
    }

    fn temp_entries(dir: &tempfile::TempDir) -> usize {
        std::fs::read_dir(dir.path().join(".tmp"))
            .expect("read .tmp")
            .count()
    }

    async fn wait_for_no_temps(dir: &tempfile::TempDir) {
        for _ in 0..200 {
            if temp_entries(dir) == 0 {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("temp file still present: {} entries", temp_entries(dir));
    }

    /// Deterministic filler that does not repeat on the append-buffer
    /// boundary, so a dropped, duplicated or reordered buffer changes the hash.
    fn pattern(len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| (i as u32).wrapping_mul(2_654_435_761).to_le_bytes()[1])
            .collect()
    }

    #[tokio::test]
    async fn staged_bytes_match_input_across_buffer_flushes() {
        let (backend, dir) = local().await;
        let storage_id = ObjectId::new();
        let data = pattern(4 * APPEND_BUFFER_BYTES + 12_345);

        // Small frames that fill the buffer, then one frame larger than the
        // buffer arriving while it is part full, then small frames again.
        let (head, rest) = data.split_at(APPEND_BUFFER_BYTES + 100);
        let (oversized, tail) = rest.split_at(APPEND_BUFFER_BYTES + 1);
        let mut staging = UploadStaging::create(storage_id, backend.clone())
            .await
            .unwrap();
        for frame in head.chunks(7_001) {
            staging.write(frame).await.unwrap();
        }
        staging.write(oversized).await.unwrap();
        for frame in tail.chunks(7_001) {
            staging.write(frame).await.unwrap();
        }
        let staged = staging.finish().await.unwrap();

        assert_eq!(staged.size_bytes(), data.len() as i64);
        assert_eq!(staged.checksum_sha256(), hex::encode(Sha256::digest(&data)));

        staged
            .commit(storage_id, &backend, "alice/big.bin")
            .await
            .unwrap();
        assert_eq!(backend.read_all("alice/big.bin").await.unwrap(), data);
        assert_eq!(temp_entries(&dir), 0);
    }

    #[tokio::test]
    async fn empty_upload_commits_an_empty_file() {
        let (backend, dir) = local().await;
        let storage_id = ObjectId::new();

        let staged = UploadStaging::create(storage_id, backend.clone())
            .await
            .unwrap()
            .finish()
            .await
            .unwrap();
        assert_eq!(staged.size_bytes(), 0);
        assert_eq!(staged.checksum_sha256(), hex::encode(Sha256::digest(b"")));

        staged
            .commit(storage_id, &backend, "alice/empty.txt")
            .await
            .unwrap();
        assert!(backend
            .read_all("alice/empty.txt")
            .await
            .unwrap()
            .is_empty());
        assert_eq!(temp_entries(&dir), 0);
    }

    #[tokio::test]
    async fn commit_replaces_existing_file() {
        let (backend, dir) = local().await;
        let storage_id = ObjectId::new();
        backend
            .write("alice/doc.txt", b"old content")
            .await
            .unwrap();

        let mut staging = UploadStaging::create(storage_id, backend.clone())
            .await
            .unwrap();
        staging.write(b"new").await.unwrap();
        staging
            .finish()
            .await
            .unwrap()
            .commit(storage_id, &backend, "alice/doc.txt")
            .await
            .unwrap();

        assert_eq!(backend.read_all("alice/doc.txt").await.unwrap(), b"new");
        assert_eq!(temp_entries(&dir), 0);
    }

    #[tokio::test]
    async fn dropping_mid_stream_removes_temp() {
        let (backend, dir) = local().await;
        let mut staging = UploadStaging::create(ObjectId::new(), backend)
            .await
            .unwrap();
        staging
            .write(&pattern(APPEND_BUFFER_BYTES + 1))
            .await
            .unwrap();
        assert_eq!(temp_entries(&dir), 1);

        drop(staging);
        wait_for_no_temps(&dir).await;
    }

    #[tokio::test]
    async fn dropping_after_finish_removes_temp() {
        let (backend, dir) = local().await;
        let mut staging = UploadStaging::create(ObjectId::new(), backend)
            .await
            .unwrap();
        staging.write(b"rejected later").await.unwrap();
        let staged = staging.finish().await.unwrap();
        assert_eq!(temp_entries(&dir), 1);

        drop(staged);
        wait_for_no_temps(&dir).await;
    }

    #[tokio::test]
    async fn commit_to_another_storage_copies_and_removes_temp() {
        let (staging_backend, staging_dir) = local().await;
        let (target_backend, target_dir) = local().await;
        let data = pattern(APPEND_BUFFER_BYTES + 99);

        let mut staging = UploadStaging::create(ObjectId::new(), staging_backend.clone())
            .await
            .unwrap();
        staging.write(&data).await.unwrap();
        staging
            .finish()
            .await
            .unwrap()
            .commit(ObjectId::new(), &target_backend, "bob/pinned.bin")
            .await
            .unwrap();

        assert_eq!(
            target_backend.read_all("bob/pinned.bin").await.unwrap(),
            data
        );
        assert!(!staging_backend.exists("bob/pinned.bin").await.unwrap());
        assert_eq!(temp_entries(&staging_dir), 0);
        assert_eq!(temp_entries(&target_dir), 0);
    }
}
