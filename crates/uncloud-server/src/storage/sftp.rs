use async_trait::async_trait;
use mongodb::{bson::doc, bson::oid::ObjectId, Database};
use russh::client::{self, Handler, Msg};
use russh::keys::{decode_secret_key, HashAlg, PrivateKey, PrivateKeyWithHashAlg};
use russh::keys::PublicKey;
use russh::Channel;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use sha2::{Digest, Sha256};
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::Mutex;
use uuid::Uuid;

use super::{BoxedAsyncRead, ScanEntry, StorageBackend};
use crate::error::{AppError, Result};
use crate::models::{SftpHostKey, StorageBackendConfig};

/// SFTP-backed storage. Holds a single long-lived SSH/SFTP session that is
/// recreated on demand if it dies. Concurrent operations share the session —
/// the SFTP protocol multiplexes requests over one channel.
pub struct SftpStorage {
    /// Connection pool. Each in-flight op checks out an SSH session;
    /// connections come back to the pool on `Drop` unless the op called
    /// `discard()` (which the retry layer does on transient errors so
    /// busted sessions don't go back into rotation).
    pool: Arc<ConnectionPool>,
    base_path: String,
    /// Retry policy for idempotent ops (read / read_range / exists / scan
    /// / write / delete / rename). Each retry attempt checks out a fresh
    /// connection from the pool, so a single broken session no longer
    /// kills concurrent ops the way `invalidate_session()` used to.
    retry: super::retry::RetryConfig,
}

#[derive(Clone)]
struct ConnConfig {
    host: String,
    port: u16,
    username: String,
    auth: AuthMode,
    expected_host_key: Option<HostKeyMatcher>,
    /// Skip-mode logs a warning once at startup and never compares keys.
    skip_host_key_check: bool,
}

#[derive(Clone)]
enum AuthMode {
    Password(String),
    PrivateKey {
        key: Arc<PrivateKey>,
    },
}

#[derive(Clone)]
struct HostKeyMatcher {
    /// SHA-256 fingerprint of the SSH wire-format public key blob.
    fingerprint_sha256_hex: String,
}

struct Conn {
    sftp: SftpSession,
    /// Holding the SSH handle keeps the session alive — its drop tears down
    /// the underlying TCP connection.
    _ssh: client::Handle<TofuHandler>,
}

impl std::ops::Deref for Conn {
    type Target = SftpSession;
    fn deref(&self) -> &Self::Target {
        &self.sftp
    }
}

/// SSH client handler. Captures the seen host key so the outer code can pin
/// it after handshake (TOFU) or compare against a pre-set fingerprint.
struct TofuHandler {
    expected: Option<HostKeyMatcher>,
    /// `true` when no comparison should happen at all.
    skip: bool,
    /// Filled in by `check_server_key` so the caller can read the seen key.
    seen: Arc<Mutex<Option<SeenKey>>>,
}

#[derive(Clone)]
struct SeenKey {
    key_type: String,
    blob_base64: String,
    fingerprint_sha256_hex: String,
}

impl Handler for TofuHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKey,
    ) -> std::result::Result<bool, Self::Error> {
        let key_type = server_public_key.algorithm().as_str().to_string();
        let blob = match server_public_key.to_bytes() {
            Ok(b) => b,
            Err(e) => {
                tracing::error!("Failed to serialise server key: {e}");
                return Ok(false);
            }
        };
        let fp_hex = hex::encode(Sha256::digest(&blob));
        let blob_base64 = base64::engine::general_purpose::STANDARD.encode(&blob);
        let seen = SeenKey {
            key_type,
            blob_base64,
            fingerprint_sha256_hex: fp_hex.clone(),
        };
        *self.seen.lock().await = Some(seen);

        if self.skip {
            return Ok(true);
        }
        if let Some(expected) = &self.expected {
            return Ok(expected.fingerprint_sha256_hex == fp_hex);
        }
        // No expectation pre-set → first connect (TOFU); accept and let the
        // caller pin the seen key afterwards.
        Ok(true)
    }
}

impl SftpStorage {
    pub async fn new(
        config: &StorageBackendConfig,
        retry: super::retry::RetryConfig,
        db: Database,
        storage_id: ObjectId,
    ) -> Result<Self> {
        let StorageBackendConfig::Sftp {
            host,
            port,
            username,
            password,
            private_key,
            private_key_passphrase,
            base_path,
            host_key,
            host_key_check,
            connection_pool_size,
            max_concurrent_ops,
        } = config
        else {
            return Err(AppError::Internal(
                "SftpStorage::new called with non-Sftp config".into(),
            ));
        };
        let pool_size = connection_pool_size.unwrap_or(2).max(1);
        let op_cap = max_concurrent_ops.unwrap_or(4).max(1);
        if pool_size > 4 {
            tracing::warn!(
                "SFTP storage `{host}:{port}`: connection_pool_size={pool_size} — \
                 some shared-tenant SFTP servers (Hetzner Storage Box ≈ 5) \
                 cap concurrent SSH connections; raise with care",
            );
        }

        let auth = match (password.as_ref(), private_key.as_ref()) {
            (Some(p), None) => AuthMode::Password(p.clone()),
            (None, Some(pem)) => {
                let key = decode_secret_key(pem, private_key_passphrase.as_deref())
                    .map_err(|e| AppError::Storage(format!("Invalid SFTP private key: {e}")))?;
                AuthMode::PrivateKey { key: Arc::new(key) }
            }
            (Some(_), Some(_)) => {
                return Err(AppError::Internal(
                    "SFTP storage: set either password OR private_key, not both".into(),
                ));
            }
            (None, None) => {
                return Err(AppError::Internal(
                    "SFTP storage: one of password / private_key must be set".into(),
                ));
            }
        };

        // Resolve host-key strategy.
        let mode = host_key_check.as_deref().unwrap_or("tofu").to_lowercase();
        let skip = mode == "skip";
        if skip {
            tracing::warn!(
                "SFTP storage `{}@{}:{}` running with host_key_check=skip — vulnerable to MITM",
                username,
                host,
                port
            );
        }
        let expected = if let Some(pinned) = host_key {
            Some(parse_host_key(pinned)?)
        } else if !skip {
            // TOFU: load any previously pinned key for this storage_id.
            load_pinned_host_key(&db, storage_id).await?
        } else {
            None
        };

        let cfg = ConnConfig {
            host: host.clone(),
            port: *port,
            username: username.clone(),
            auth,
            expected_host_key: expected,
            skip_host_key_check: skip,
        };

        let pool = ConnectionPool::new(cfg, db, storage_id, pool_size, op_cap);
        let storage = Self {
            pool,
            base_path: base_path.trim_end_matches('/').to_string(),
            retry,
        };

        // Eagerly check out (and immediately return) a connection so
        // misconfig fails fast at startup, and so the first TOFU pin is
        // recorded immediately.
        let _ = storage.pool.checkout().await?;
        Ok(storage)
    }

    fn resolve(&self, path: &str) -> String {
        let p = path.trim_start_matches('/');
        if p.is_empty() {
            self.base_path.clone()
        } else if self.base_path.is_empty() {
            p.to_string()
        } else {
            format!("{}/{}", self.base_path, p)
        }
    }

    async fn ensure_parent_dir(&self, sftp: &SftpSession, full_path: &str) -> Result<()> {
        let Some(parent) = parent_of(full_path) else {
            return Ok(());
        };
        ensure_dir(sftp, parent).await
    }
}

/// Reader that owns a `PooledConn` for the lifetime of the read so
/// concurrent ops on the same SftpStorage don't pick the same connection
/// out of the pool while bytes are still streaming over its SFTP channel.
/// The wrapped reader (an `SftpFile`) holds an internal `Arc<SftpSession>`
/// which is what actually drives the byte transfer; we just keep the
/// pool slot occupied via `_pooled` until the reader is dropped.
struct PooledReader<R> {
    _pooled: PooledConn,
    inner: R,
}

impl<R: AsyncRead + Unpin> AsyncRead for PooledReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl SftpStorage {
    async fn read_once(&self, path: &str) -> Result<BoxedAsyncRead> {
        let pooled = self.pool.checkout().await?;
        let full = self.resolve(path);
        match pooled.sftp.open(full).await {
            Ok(file) => Ok(Box::pin(PooledReader {
                _pooled: pooled,
                inner: file,
            })),
            Err(e) => {
                pooled.discard();
                Err(AppError::Storage(format!("SFTP open failed: {e}")))
            }
        }
    }

    async fn read_range_once(
        &self,
        path: &str,
        offset: u64,
        length: u64,
    ) -> Result<BoxedAsyncRead> {
        let pooled = self.pool.checkout().await?;
        let full = self.resolve(path);
        let body = async {
            let mut file = pooled
                .sftp
                .open(full)
                .await
                .map_err(|e| AppError::Storage(format!("SFTP open failed: {e}")))?;
            file.seek(std::io::SeekFrom::Start(offset))
                .await
                .map_err(|e| AppError::Storage(format!("SFTP seek failed: {e}")))?;
            Ok::<_, AppError>(file)
        }
        .await;
        match body {
            Ok(file) => Ok(Box::pin(PooledReader {
                _pooled: pooled,
                inner: file.take(length),
            })),
            Err(e) => {
                pooled.discard();
                Err(e)
            }
        }
    }

    async fn write_once(&self, path: &str, data: &[u8]) -> Result<()> {
        let pooled = self.pool.checkout().await?;
        let full = self.resolve(path);
        let result = async {
            self.ensure_parent_dir(&pooled.sftp, &full).await?;
            let mut file = pooled
                .sftp
                .create(full)
                .await
                .map_err(|e| AppError::Storage(format!("SFTP create failed: {e}")))?;
            file.write_all(data)
                .await
                .map_err(|e| AppError::Storage(format!("SFTP write failed: {e}")))?;
            file.shutdown()
                .await
                .map_err(|e| AppError::Storage(format!("SFTP close failed: {e}")))?;
            Ok::<(), AppError>(())
        }
        .await;
        if result.is_err() {
            pooled.discard();
        }
        result
    }

    async fn rename_once(&self, from: &str, to: &str) -> Result<()> {
        let pooled = self.pool.checkout().await?;
        let from_full = self.resolve(from);
        let to_full = self.resolve(to);
        let result = async {
            self.ensure_parent_dir(&pooled.sftp, &to_full).await?;
            pooled
                .sftp
                .rename(from_full, to_full)
                .await
                .map_err(|e| AppError::Storage(format!("SFTP rename failed: {e}")))?;
            Ok::<(), AppError>(())
        }
        .await;
        if result.is_err() {
            pooled.discard();
        }
        result
    }

    async fn delete_once(&self, path: &str) -> Result<()> {
        let pooled = self.pool.checkout().await?;
        let full = self.resolve(path);
        match pooled.sftp.remove_file(full).await {
            Ok(()) => Ok(()),
            // Treat "not found" as success — matches LocalStorage's behaviour
            // where unlink-on-missing silently no-ops.
            Err(e) if is_no_such_file(&e) => Ok(()),
            Err(e) => {
                pooled.discard();
                Err(AppError::Storage(format!("SFTP delete failed: {e}")))
            }
        }
    }

    async fn exists_once(&self, path: &str) -> Result<bool> {
        let pooled = self.pool.checkout().await?;
        let full = self.resolve(path);
        match pooled.sftp.metadata(full).await {
            Ok(_) => Ok(true),
            Err(e) if is_no_such_file(&e) => Ok(false),
            Err(e) => {
                pooled.discard();
                Err(AppError::Storage(format!("SFTP stat failed: {e}")))
            }
        }
    }

    async fn scan_once(&self, prefix: &str) -> Result<Vec<ScanEntry>> {
        let pooled = self.pool.checkout().await?;
        let root = self.resolve(prefix);
        let mut out = Vec::new();
        let mut stack: Vec<String> = vec![root.clone()];
        let result = async {
            while let Some(dir) = stack.pop() {
                let entries = match pooled.sftp.read_dir(&dir).await {
                    Ok(e) => e,
                    Err(e) if is_no_such_file(&e) => continue,
                    Err(e) => {
                        return Err(AppError::Storage(format!("SFTP read_dir failed: {e}")));
                    }
                };
                for entry in entries {
                    let name = entry.file_name();
                    if name == "." || name == ".." {
                        continue;
                    }
                    let full = if dir.ends_with('/') {
                        format!("{}{}", dir, name)
                    } else {
                        format!("{}/{}", dir, name)
                    };
                    let trimmed = full.trim_start_matches('/');
                    let rel = trimmed
                        .strip_prefix(&format!("{}/", self.base_path.trim_start_matches('/')))
                        .unwrap_or(trimmed)
                        .to_string();
                    let attrs = entry.metadata();
                    if attrs.is_dir() {
                        out.push(ScanEntry {
                            path: rel,
                            is_dir: true,
                            size_bytes: 0,
                        });
                        stack.push(full);
                    } else {
                        out.push(ScanEntry {
                            path: rel,
                            is_dir: false,
                            size_bytes: attrs.size.unwrap_or(0),
                        });
                    }
                }
            }
            Ok::<_, AppError>(out)
        }
        .await;
        match result {
            Ok(v) => Ok(v),
            Err(e) => {
                pooled.discard();
                Err(e)
            }
        }
    }
}

/// Run an idempotent SFTP op through the configured retry policy. The
/// retry helper itself doesn't touch connection state — each `_once`
/// closure checks out its own pooled connection and (via `discard()`)
/// removes a busted session from rotation on its way out, so the next
/// attempt naturally grabs a healthy connection from the pool.
async fn retry_idempotent<F, Fut, T>(
    storage: &SftpStorage,
    op_name: &str,
    mut f: F,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let max = storage.retry.effective_max_attempts();
    let mut delay = storage.retry.base_delay();
    let max_delay = storage.retry.max_delay();
    for attempt in 1..=max {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) if attempt < max => {
                tracing::warn!(
                    "{op_name}: attempt {attempt}/{max} failed: {e}; \
                     retrying in {delay:?}",
                );
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(max_delay);
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!("loop returns from every iteration")
}

#[async_trait]
impl StorageBackend for SftpStorage {
    async fn read(&self, path: &str) -> Result<BoxedAsyncRead> {
        retry_idempotent(self, "sftp.read", || self.read_once(path)).await
    }

    async fn read_range(&self, path: &str, offset: u64, length: u64) -> Result<BoxedAsyncRead> {
        retry_idempotent(self, "sftp.read_range", || {
            self.read_range_once(path, offset, length)
        })
        .await
    }

    async fn write(&self, path: &str, data: &[u8]) -> Result<()> {
        // Idempotent at the path level — re-issuing produces the same
        // file contents whether the server saw zero, partial, or full
        // data on the failed attempt. Caller owns the bytes; no stream
        // consumption to worry about.
        retry_idempotent(self, "sftp.write", || self.write_once(path, data)).await
    }

    /// **Not retried.** The input `reader` is consumed by the first attempt;
    /// re-issuing would either need a rewindable reader (`AsyncSeek`) or an
    /// in-memory buffer (which kills the streaming win for large uploads),
    /// or a callsite refactor to provide a `Fn() -> BoxedAsyncRead` that
    /// produces a fresh reader per attempt. None of those are worth the
    /// cost until upload reliability becomes a real problem in practice.
    async fn write_stream(
        &self,
        path: &str,
        mut reader: BoxedAsyncRead,
        _size: u64,
    ) -> Result<()> {
        let pooled = self.pool.checkout().await?;
        let full = self.resolve(path);
        let result = async {
            self.ensure_parent_dir(&pooled.sftp, &full).await?;
            let mut file = pooled
                .sftp
                .create(full)
                .await
                .map_err(|e| AppError::Storage(format!("SFTP create failed: {e}")))?;
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = reader
                    .read(&mut buf)
                    .await
                    .map_err(|e| AppError::Storage(format!("read stream: {e}")))?;
                if n == 0 {
                    break;
                }
                file.write_all(&buf[..n])
                    .await
                    .map_err(|e| AppError::Storage(format!("SFTP write failed: {e}")))?;
            }
            file.shutdown()
                .await
                .map_err(|e| AppError::Storage(format!("SFTP close failed: {e}")))?;
            Ok::<(), AppError>(())
        }
        .await;
        if result.is_err() {
            pooled.discard();
        }
        result
    }

    async fn delete(&self, path: &str) -> Result<()> {
        // Already idempotent — `is_no_such_file` is mapped to success, so a
        // retried delete after a successful-but-lost-response first attempt
        // sees "missing" and reports Ok.
        retry_idempotent(self, "sftp.delete", || self.delete_once(path)).await
    }

    async fn exists(&self, path: &str) -> Result<bool> {
        retry_idempotent(self, "sftp.exists", || self.exists_once(path)).await
    }

    async fn available_space(&self) -> Result<Option<u64>> {
        Ok(None)
    }

    /// **Not retried** — `create_temp` / `append_temp` / `finalize_temp` /
    /// `abort_temp` together implement the resumable-upload pattern. The
    /// caller orchestrates them and already knows which step to repeat on
    /// failure (typically `append_temp` continues from the last byte
    /// successfully written). Adding implicit retry inside each step
    /// would create double-retry semantics with the caller's own logic.
    async fn create_temp(&self) -> Result<String> {
        let pooled = self.pool.checkout().await?;
        let name = format!("{}.tmp", Uuid::new_v4());
        let full = self.resolve(&format!(".tmp/{}", name));
        let result = async {
            ensure_dir(&pooled.sftp, &self.resolve(".tmp")).await?;
            let mut file = pooled
                .sftp
                .create(full)
                .await
                .map_err(|e| AppError::Storage(format!("SFTP temp create failed: {e}")))?;
            file.shutdown().await.ok();
            Ok::<_, AppError>(name.clone())
        }
        .await;
        match result {
            Ok(v) => Ok(v),
            Err(e) => {
                pooled.discard();
                Err(e)
            }
        }
    }

    async fn append_temp(&self, temp_path: &str, data: &[u8]) -> Result<()> {
        let pooled = self.pool.checkout().await?;
        let full = self.resolve(&format!(".tmp/{}", temp_path.trim_start_matches('/')));
        let result = async {
            let mut file = pooled
                .sftp
                .open_with_flags(full, OpenFlags::WRITE | OpenFlags::APPEND)
                .await
                .map_err(|e| AppError::Storage(format!("SFTP temp open failed: {e}")))?;
            file.write_all(data)
                .await
                .map_err(|e| AppError::Storage(format!("SFTP append failed: {e}")))?;
            file.shutdown()
                .await
                .map_err(|e| AppError::Storage(format!("SFTP close failed: {e}")))?;
            Ok::<(), AppError>(())
        }
        .await;
        if result.is_err() {
            pooled.discard();
        }
        result
    }

    async fn finalize_temp(&self, temp_path: &str, final_path: &str) -> Result<()> {
        let pooled = self.pool.checkout().await?;
        let from = self.resolve(&format!(".tmp/{}", temp_path.trim_start_matches('/')));
        let to = self.resolve(final_path);
        let result = async {
            self.ensure_parent_dir(&pooled.sftp, &to).await?;
            pooled
                .sftp
                .rename(from, to)
                .await
                .map_err(|e| AppError::Storage(format!("SFTP rename failed: {e}")))?;
            Ok::<(), AppError>(())
        }
        .await;
        if result.is_err() {
            pooled.discard();
        }
        result
    }

    async fn abort_temp(&self, temp_path: &str) -> Result<()> {
        let pooled = self.pool.checkout().await?;
        let full = self.resolve(&format!(".tmp/{}", temp_path.trim_start_matches('/')));
        match pooled.sftp.remove_file(full).await {
            Ok(()) => Ok(()),
            Err(_) => {
                // abort_temp is best-effort; even on error we report Ok so
                // callers don't blow up on cleanup. But the connection's
                // probably toast — discard so the pool stays healthy.
                pooled.discard();
                Ok(())
            }
        }
    }

    async fn rename(&self, from: &str, to: &str) -> Result<()> {
        // Renames are *almost* idempotent — the catch is the second attempt
        // sees `from` missing if the first attempt actually landed but the
        // server's response was lost. Disambiguate by checking whether `to`
        // exists: if it does and `from` doesn't, the rename succeeded last
        // time and we should report Ok rather than chase a phantom failure.
        let max = self.retry.effective_max_attempts();
        let mut delay = self.retry.base_delay();
        let max_delay = self.retry.max_delay();

        for attempt in 1..=max {
            match self.rename_once(from, to).await {
                Ok(()) => return Ok(()),
                Err(e) if attempt < max => {
                    // Possibly a successful-but-lost-response: check the
                    // destination before assuming the rename failed.
                    let to_exists = self.exists_once(to).await.unwrap_or(false);
                    let from_exists = self.exists_once(from).await.unwrap_or(true);
                    if to_exists && !from_exists {
                        tracing::warn!(
                            "sftp.rename: first attempt errored ({e}) but `{to}` exists \
                             and `{from}` is gone — treating as successful previous attempt"
                        );
                        return Ok(());
                    }
                    tracing::warn!(
                        "sftp.rename: attempt {attempt}/{max} failed: {e}; \
                         retrying in {delay:?}"
                    );
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(max_delay);
                }
                Err(e) => return Err(e),
            }
        }
        unreachable!("loop returns from every iteration")
    }

    async fn archive_version(&self, current: &str, version: &str) -> Result<()> {
        // SFTP has no native copy — read source, write destination on the
        // same connection (cheaper than two checkouts since we'd serialize
        // them anyway via the underlying SFTP channel).
        let pooled = self.pool.checkout().await?;
        let src = self.resolve(current);
        let dst = self.resolve(version);
        let result = async {
            self.ensure_parent_dir(&pooled.sftp, &dst).await?;
            let mut input = pooled
                .sftp
                .open(src)
                .await
                .map_err(|e| AppError::Storage(format!("SFTP open source: {e}")))?;
            let mut output = pooled
                .sftp
                .create(dst)
                .await
                .map_err(|e| AppError::Storage(format!("SFTP create dest: {e}")))?;
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = input
                    .read(&mut buf)
                    .await
                    .map_err(|e| AppError::Storage(format!("read source: {e}")))?;
                if n == 0 {
                    break;
                }
                output
                    .write_all(&buf[..n])
                    .await
                    .map_err(|e| AppError::Storage(format!("write dest: {e}")))?;
            }
            output
                .shutdown()
                .await
                .map_err(|e| AppError::Storage(format!("SFTP close: {e}")))?;
            Ok::<(), AppError>(())
        }
        .await;
        if result.is_err() {
            pooled.discard();
        }
        result
    }

    async fn move_to_trash(&self, current: &str, trash: &str) -> Result<()> {
        self.rename(current, trash).await
    }

    async fn restore_from_trash(&self, trash: &str, restore: &str) -> Result<()> {
        self.rename(trash, restore).await
    }

    async fn scan(&self, prefix: &str) -> Result<Vec<ScanEntry>> {
        retry_idempotent(self, "sftp.scan", || self.scan_once(prefix)).await
    }
}

// ── connection pool ─────────────────────────────────────────────────────────

/// Bounded pool of SSH/SFTP sessions. Each `checkout` returns a `PooledConn`
/// guard that releases the connection back to the pool on `Drop` — unless
/// `discard()` was called first, in which case the connection is dropped
/// (and the next checkout will open a fresh one).
///
/// Sizing semantics:
///
/// * `pool_size` caps how many connections the pool will *hold*. The pool
///   is lazy — connections are opened on demand and reused thereafter, so
///   the steady-state count tracks the number of concurrent ops.
/// * `op_cap` caps how many ops can be in flight at once across all
///   callers, independent of pool size. With `pool_size=2, op_cap=4` you
///   get up to 4 ops queued against 2 connections at any moment, which is
///   the sweet spot for shared-tenant SFTP servers (Hetzner, etc.) where
///   the connection cap is tight but the request rate isn't.
///
/// On error, callers are expected to invoke `PooledConn::discard()` so a
/// busted SSH session doesn't go back into rotation. The next checkout
/// opens a fresh connection in its slot.
struct ConnectionPool {
    cfg: Arc<ConnConfig>,
    db: Database,
    storage_id: ObjectId,
    available: std::sync::Mutex<std::collections::VecDeque<Arc<Conn>>>,
    /// Caps total in-flight checkouts. Doubles as the upper bound on
    /// concurrent ops at the SFTP-storage layer.
    op_semaphore: Arc<tokio::sync::Semaphore>,
    pool_size: usize,
}

struct PooledConn {
    conn: Option<Arc<Conn>>,
    pool: std::sync::Weak<ConnectionPool>,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

impl ConnectionPool {
    fn new(
        cfg: ConnConfig,
        db: Database,
        storage_id: ObjectId,
        pool_size: u32,
        op_cap: u32,
    ) -> Arc<Self> {
        Arc::new(Self {
            cfg: Arc::new(cfg),
            db,
            storage_id,
            available: std::sync::Mutex::new(std::collections::VecDeque::new()),
            op_semaphore: Arc::new(tokio::sync::Semaphore::new(op_cap as usize)),
            pool_size: pool_size as usize,
        })
    }

    async fn checkout(self: &Arc<Self>) -> Result<PooledConn> {
        let permit = self
            .op_semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| AppError::Storage(format!("SFTP op semaphore closed: {e}")))?;
        // Try to grab a warm connection; if none, open a fresh one.
        let warm = {
            let mut available = self
                .available
                .lock()
                .map_err(|_| AppError::Storage("SFTP pool mutex poisoned".into()))?;
            available.pop_front()
        };
        let conn = match warm {
            Some(c) => c,
            None => Arc::new(self.connect().await?),
        };
        Ok(PooledConn {
            conn: Some(conn),
            pool: Arc::downgrade(self),
            _permit: permit,
        })
    }

    /// Open a fresh SSH/SFTP session. Pinned host key (or TOFU on first
    /// connect) is read from / written to MongoDB via the configured
    /// `db` + `storage_id`. Used by `checkout` when no warm connection
    /// is available, and indirectly by retry paths after `discard()`.
    async fn connect(&self) -> Result<Conn> {
        let seen = Arc::new(Mutex::new(None));
        let handler = TofuHandler {
            expected: self.cfg.expected_host_key.clone(),
            skip: self.cfg.skip_host_key_check,
            seen: seen.clone(),
        };

        let ssh_config = Arc::new(client::Config::default());
        let mut handle = client::connect(
            ssh_config,
            (self.cfg.host.as_str(), self.cfg.port),
            handler,
        )
        .await
        .map_err(|e| {
            AppError::Storage(format!(
                "SSH connect to {}:{} failed: {e}",
                self.cfg.host, self.cfg.port
            ))
        })?;

        let auth_ok = match &self.cfg.auth {
            AuthMode::Password(p) => handle
                .authenticate_password(self.cfg.username.clone(), p.clone())
                .await
                .map_err(|e| AppError::Storage(format!("SSH auth (password) failed: {e}")))?
                .success(),
            AuthMode::PrivateKey { key } => handle
                .authenticate_publickey(
                    self.cfg.username.clone(),
                    PrivateKeyWithHashAlg::new(key.clone(), Some(HashAlg::Sha256)),
                )
                .await
                .map_err(|e| AppError::Storage(format!("SSH auth (publickey) failed: {e}")))?
                .success(),
        };
        if !auth_ok {
            return Err(AppError::Storage(
                "SSH authentication rejected by server".into(),
            ));
        }

        if self.cfg.expected_host_key.is_none() && !self.cfg.skip_host_key_check {
            if let Some(s) = seen.lock().await.clone() {
                save_pinned_host_key(&self.db, self.storage_id, &s).await?;
            }
        }

        let channel: Channel<Msg> = handle
            .channel_open_session()
            .await
            .map_err(|e| AppError::Storage(format!("SSH channel open failed: {e}")))?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|e| AppError::Storage(format!("SFTP subsystem request failed: {e}")))?;

        let sftp = SftpSession::new(channel.into_stream())
            .await
            .map_err(|e| AppError::Storage(format!("SFTP session init failed: {e}")))?;

        Ok(Conn { sftp, _ssh: handle })
    }
}

impl PooledConn {
    /// Drop this connection without returning it to the pool. Call after
    /// any error that might indicate a broken SSH session — the next
    /// checkout opens a fresh one.
    fn discard(mut self) {
        self.conn = None;
    }
}

impl std::ops::Deref for PooledConn {
    type Target = Conn;
    fn deref(&self) -> &Conn {
        self.conn.as_ref().expect("conn taken before drop").as_ref()
    }
}

impl Drop for PooledConn {
    fn drop(&mut self) {
        let Some(conn) = self.conn.take() else { return };
        let Some(pool) = self.pool.upgrade() else { return };
        let pool_size = pool.pool_size;
        let mut available = match pool.available.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        if available.len() < pool_size {
            available.push_back(conn);
        }
        // else: pool is full — drop the connection. Happens only if we
        // transiently checked out more than pool_size, which the
        // op_semaphore is supposed to prevent; the bounded push is
        // defensive.
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn parent_of(path: &str) -> Option<&str> {
    let p = path.trim_end_matches('/');
    let idx = p.rfind('/')?;
    if idx == 0 {
        Some("/")
    } else {
        Some(&p[..idx])
    }
}

/// Recursive `mkdir -p` against an SFTP session. Walks downwards from the
/// shortest prefix, creating each missing directory.
async fn ensure_dir(sftp: &SftpSession, dir: &str) -> Result<()> {
    if dir.is_empty() || dir == "/" {
        return Ok(());
    }
    // If `metadata` succeeds, the path already exists. Don't trust
    // `is_dir()` — some servers (e.g. atmoz/sftp's chrooted parent dirs) omit
    // the file-type permission bits, making the helper falsely return false.
    // The subsequent file operation will fail on its own if it isn't a dir.
    if sftp.metadata(dir.to_string()).await.is_ok() {
        return Ok(());
    }
    if let Some(parent) = parent_of(dir) {
        Box::pin(ensure_dir(sftp, parent)).await?;
    }
    match sftp.create_dir(dir.to_string()).await {
        Ok(()) => Ok(()),
        // Race or stat-permission-denied-but-mkdir-saw-it: treat as ok.
        Err(e) if is_already_exists(&e) => Ok(()),
        Err(e) => Err(AppError::Storage(format!(
            "SFTP mkdir({dir}) failed: {e}"
        ))),
    }
}

fn is_no_such_file(err: &russh_sftp::client::error::Error) -> bool {
    let s = err.to_string().to_lowercase();
    s.contains("no such file") || s.contains("does not exist") || s.contains("not found")
}

fn is_already_exists(err: &russh_sftp::client::error::Error) -> bool {
    let s = err.to_string().to_lowercase();
    s.contains("file already exists") || s.contains("already exists")
}

fn parse_host_key(pinned: &str) -> Result<HostKeyMatcher> {
    // Accepts either a full "ssh-ed25519 AAAA... [comment]" line or a bare
    // base64 blob. SHA-256 fingerprints are also accepted as
    // "SHA256:<base64>" — common in `ssh-keygen -lf` output.
    let trimmed = pinned.trim();
    if let Some(rest) = trimmed.strip_prefix("SHA256:") {
        // A user-supplied fingerprint. Convert to lowercase hex of the same bytes.
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(rest.trim().trim_end_matches('='))
            .map_err(|e| AppError::Internal(format!("Invalid SHA256 fingerprint: {e}")))?;
        return Ok(HostKeyMatcher {
            fingerprint_sha256_hex: hex::encode(bytes),
        });
    }
    let parts: Vec<&str> = trimmed.split_ascii_whitespace().collect();
    let blob_b64 = match parts.as_slice() {
        [_algo, blob, ..] => blob.to_string(),
        [blob] => blob.to_string(),
        _ => {
            return Err(AppError::Internal(
                "host_key: expected `<algo> <base64>` or `SHA256:<...>`".into(),
            ))
        }
    };
    let blob = base64::engine::general_purpose::STANDARD
        .decode(blob_b64)
        .map_err(|e| AppError::Internal(format!("Invalid host_key base64: {e}")))?;
    Ok(HostKeyMatcher {
        fingerprint_sha256_hex: hex::encode(Sha256::digest(&blob)),
    })
}

async fn load_pinned_host_key(
    db: &Database,
    storage_id: ObjectId,
) -> Result<Option<HostKeyMatcher>> {
    let coll = db.collection::<SftpHostKey>("sftp_host_keys");
    let row = coll.find_one(doc! { "storage_id": storage_id }).await?;
    Ok(row.map(|r| HostKeyMatcher {
        fingerprint_sha256_hex: r.fingerprint_sha256,
    }))
}

async fn save_pinned_host_key(
    db: &Database,
    storage_id: ObjectId,
    seen: &SeenKey,
) -> Result<()> {
    let coll = db.collection::<SftpHostKey>("sftp_host_keys");
    let row = SftpHostKey {
        id: ObjectId::new(),
        storage_id,
        key_type: seen.key_type.clone(),
        key_blob_base64: seen.blob_base64.clone(),
        fingerprint_sha256: seen.fingerprint_sha256_hex.clone(),
        first_seen_at: chrono::Utc::now(),
    };
    coll.insert_one(&row).await?;
    tracing::info!(
        "SFTP storage {}: pinned host key fingerprint SHA256:{} (TOFU)",
        storage_id,
        seen.fingerprint_sha256_hex
    );
    Ok(())
}

use base64::Engine as _;
