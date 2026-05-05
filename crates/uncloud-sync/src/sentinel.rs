//! Sync-root sentinel: a `.uncloud-root.json` file written at the top of every
//! sync base. Two jobs:
//!
//! 1. **Liveness check.** If the file is missing the volume is unmounted (USB
//!    drive disconnected, network share down, OS race during boot). Without
//!    this guard a single scan would treat every previously-synced file as
//!    "deleted locally" and push a tidal wave of deletes to the server.
//! 2. **Identity check.** The sentinel carries a UUID that pairs it with a
//!    row in the `sync_bases` table. If the IDs don't match, the user has
//!    pointed sync at a different folder — we abort rather than guess what
//!    they meant.
//!
//! On the first ever successful sync of a base the sentinel doesn't exist
//! yet, so a blank state is *not* an error there — [`verify_or_mint`] writes
//! the file and inserts the matching `sync_bases` row in the same call.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::fs::{LocalFs, LocalFsError};
use crate::journal::{Journal, SyncBaseRow};

/// Filename written at the top of every sync base.
pub const SENTINEL_FILENAME: &str = ".uncloud-root.json";

/// JSON payload of [`SENTINEL_FILENAME`]. Stable wire format — adding fields
/// is fine, renaming or removing them is a migration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sentinel {
    /// UUID pairing this sentinel with a `sync_bases` row.
    pub base_id: String,
    /// UUID identifying this client install. Shared across every sync base
    /// the client manages — useful as a forensic breadcrumb when a user
    /// runs multiple installs against the same physical folder.
    pub instance_id: String,
    /// Absolute path the base was created at, recorded at mint time. Purely
    /// informational; we re-resolve the path through the OS on every sync.
    pub local_path: String,
    /// ISO 8601 timestamp the sentinel was minted. Informational.
    pub created_at: String,
}

/// Outcome of [`verify_or_mint`]. [`SentinelStatus::Verified`] is the steady
/// state; [`SentinelStatus::Minted`] only fires on first sync of a fresh base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SentinelStatus {
    /// The on-disk sentinel matched the journal row.
    Verified,
    /// The journal had no row yet; we wrote a fresh sentinel and inserted
    /// the row in the same call.
    Minted,
}

/// Why the sentinel check refused to continue. The desktop UI is expected to
/// surface these to the user verbatim — the engine deliberately does not
/// auto-recover, because every recovery path masks a real problem.
#[derive(Debug, thiserror::Error)]
pub enum SentinelError {
    /// We have a `sync_bases` row for this path but no sentinel file at it.
    /// The volume is unmounted, the user manually deleted the sentinel, or
    /// they pointed sync at a different folder that happens to share the
    /// path. None of those are safe to ignore.
    #[error("Sync root '{path}' has no `{filename}` sentinel — refusing to sync.")]
    Missing { path: String, filename: &'static str },

    /// We found a sentinel but its `base_id` is for a different base. Most
    /// likely the user replaced the contents of the sync folder with a
    /// snapshot from a different machine.
    #[error(
        "Sync root '{path}' is tagged as base '{found}' but the journal \
         expects '{expected}' — refusing to sync."
    )]
    Mismatch { path: String, expected: String, found: String },

    /// The sentinel file was unreadable or didn't deserialize.
    #[error("Sync root '{path}' has a corrupt sentinel: {reason}")]
    Corrupt { path: String, reason: String },

    /// Filesystem error reading or writing the sentinel.
    #[error(transparent)]
    Fs(#[from] LocalFsError),

    /// Journal lookup or insert failed.
    #[error(transparent)]
    Journal(#[from] sqlx::Error),
}

/// Verify the sentinel at `base_local_path` against the journal, or — if no
/// journal row exists yet — mint a fresh sentinel + base row atomically. The
/// caller is expected to do this once per base at the start of every sync
/// run, before any scan logic that might interpret a missing file as a
/// deletion.
pub async fn verify_or_mint(
    fs: &Arc<dyn LocalFs>,
    journal: &Journal,
    base_local_path: &str,
    instance_id: &str,
) -> Result<(SentinelStatus, SyncBaseRow), SentinelError> {
    let sentinel_path = fs.join(base_local_path, SENTINEL_FILENAME);
    let on_disk = read_sentinel(fs, &sentinel_path).await?;
    let in_journal = journal.get_base_by_path(base_local_path).await?;

    match (on_disk, in_journal) {
        // Both sides agree on the same base — happy path.
        (Some(s), Some(row)) if s.base_id == row.base_id => {
            Ok((SentinelStatus::Verified, row))
        }
        // Sentinel exists, journal disagrees — different base mounted at
        // this path. Don't auto-fix.
        (Some(s), Some(row)) => Err(SentinelError::Mismatch {
            path: base_local_path.to_owned(),
            expected: row.base_id,
            found: s.base_id,
        }),
        // Journal has a row but the sentinel is gone — volume unmounted or
        // user deleted the file. Bail; do not recreate.
        (None, Some(row)) => Err(SentinelError::Missing {
            path: row.local_path,
            filename: SENTINEL_FILENAME,
        }),
        // Stale sentinel left over from a previous install whose journal we
        // don't have. Refuse rather than adopt — the user might be pointing
        // at a folder restored from a backup that still has another client's
        // sentinel inside.
        (Some(s), None) => Err(SentinelError::Mismatch {
            path: base_local_path.to_owned(),
            expected: "<no journal row>".to_owned(),
            found: s.base_id,
        }),
        // First sync of a fresh base. Mint atomically: write the sentinel
        // first so an interrupted insert doesn't leave the journal with a
        // base it can't verify, then record the row.
        (None, None) => {
            let base_id = uuid::Uuid::new_v4().to_string();
            let now = chrono::Utc::now().to_rfc3339();
            let sentinel = Sentinel {
                base_id: base_id.clone(),
                instance_id: instance_id.to_owned(),
                local_path: base_local_path.to_owned(),
                created_at: now,
            };
            write_sentinel(fs, &sentinel_path, &sentinel).await?;
            let row = journal
                .insert_base(&base_id, base_local_path, instance_id)
                .await?;
            Ok((SentinelStatus::Minted, row))
        }
    }
}

/// Returns the per-install instance UUID, minting and persisting a fresh one
/// the first time it is requested. Stored in the journal's `sync_config` KV
/// table under the well-known key.
pub async fn ensure_instance_id(journal: &Journal) -> sqlx::Result<String> {
    const KEY: &str = "instance_id";
    if let Some(existing) = journal.get_config(KEY).await? {
        return Ok(existing);
    }
    let fresh = uuid::Uuid::new_v4().to_string();
    journal.set_config(KEY, &fresh).await?;
    Ok(fresh)
}

async fn read_sentinel(
    fs: &Arc<dyn LocalFs>,
    path: &str,
) -> Result<Option<Sentinel>, SentinelError> {
    match fs.read(path).await {
        Ok(bytes) => match serde_json::from_slice::<Sentinel>(&bytes) {
            Ok(s) => Ok(Some(s)),
            Err(e) => Err(SentinelError::Corrupt {
                path: path.to_owned(),
                reason: e.to_string(),
            }),
        },
        // Treat any read error as "missing" — the most common path is
        // ENOENT, but a permission error or unmounted volume should land
        // the caller on the same `Missing` branch where it can decide to
        // mint (no journal row yet) or abort (journal row exists).
        Err(_) => Ok(None),
    }
}

async fn write_sentinel(
    fs: &Arc<dyn LocalFs>,
    path: &str,
    sentinel: &Sentinel,
) -> Result<(), SentinelError> {
    let bytes = serde_json::to_vec_pretty(sentinel)
        .expect("Sentinel serialization is infallible");
    fs.write(path, &bytes).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::NativeFs;
    use sqlx::sqlite::SqlitePoolOptions;
    use tempfile::TempDir;

    /// Spin up a fresh journal on a temp DB plus a NativeFs anchored at
    /// a temp directory. Returns both, plus the temp guards so the
    /// caller's drop tears the whole rig down.
    async fn rig() -> (Arc<dyn LocalFs>, Journal, TempDir, TempDir) {
        let db_dir = TempDir::new().unwrap();
        let root_dir = TempDir::new().unwrap();
        let db_path = db_dir.path().join("journal.db");
        let url = format!("sqlite://{}?mode=rwc", db_path.display());
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        let fs: Arc<dyn LocalFs> = Arc::new(NativeFs::new());
        (fs, Journal::new(pool), db_dir, root_dir)
    }

    fn root_str(dir: &TempDir) -> String {
        dir.path().to_string_lossy().into_owned()
    }

    #[tokio::test]
    async fn first_sync_mints_sentinel_and_base_row() {
        let (fs, journal, _db, root) = rig().await;
        let path = root_str(&root);
        let (status, base) = verify_or_mint(&fs, &journal, &path, "instance-1")
            .await
            .unwrap();
        assert_eq!(status, SentinelStatus::Minted);
        assert_eq!(base.local_path, path);

        // Sentinel file exists on disk and matches the base row.
        let sentinel_bytes = fs
            .read(&fs.join(&path, SENTINEL_FILENAME))
            .await
            .unwrap();
        let s: Sentinel = serde_json::from_slice(&sentinel_bytes).unwrap();
        assert_eq!(s.base_id, base.base_id);
        assert_eq!(s.instance_id, "instance-1");
    }

    #[tokio::test]
    async fn second_call_verifies_against_existing_row() {
        let (fs, journal, _db, root) = rig().await;
        let path = root_str(&root);
        let (_, base) = verify_or_mint(&fs, &journal, &path, "instance-1")
            .await
            .unwrap();
        let (status, again) = verify_or_mint(&fs, &journal, &path, "instance-1")
            .await
            .unwrap();
        assert_eq!(status, SentinelStatus::Verified);
        assert_eq!(again.base_id, base.base_id);
    }

    #[tokio::test]
    async fn missing_sentinel_with_journal_row_is_an_error() {
        let (fs, journal, _db, root) = rig().await;
        let path = root_str(&root);
        verify_or_mint(&fs, &journal, &path, "instance-1")
            .await
            .unwrap();
        // Simulate user/OS removing the sentinel.
        fs.remove_file(&fs.join(&path, SENTINEL_FILENAME))
            .await
            .unwrap();
        let err = verify_or_mint(&fs, &journal, &path, "instance-1")
            .await
            .unwrap_err();
        assert!(matches!(err, SentinelError::Missing { .. }));
    }

    #[tokio::test]
    async fn mismatched_sentinel_is_an_error() {
        let (fs, journal, _db, root) = rig().await;
        let path = root_str(&root);
        verify_or_mint(&fs, &journal, &path, "instance-1")
            .await
            .unwrap();
        // Overwrite the sentinel with a different base_id, as if a
        // backup or a different install dropped its file in here.
        let imposter = Sentinel {
            base_id: "different-base".into(),
            instance_id: "instance-1".into(),
            local_path: path.clone(),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        fs.write(
            &fs.join(&path, SENTINEL_FILENAME),
            &serde_json::to_vec(&imposter).unwrap(),
        )
        .await
        .unwrap();
        let err = verify_or_mint(&fs, &journal, &path, "instance-1")
            .await
            .unwrap_err();
        assert!(matches!(err, SentinelError::Mismatch { .. }));
    }

    #[tokio::test]
    async fn stale_sentinel_without_journal_row_is_rejected() {
        let (fs, journal, _db, root) = rig().await;
        let path = root_str(&root);
        // Write a sentinel without ever calling verify_or_mint — as if a
        // user pointed sync at a folder that already contains another
        // install's file.
        let leftover = Sentinel {
            base_id: "ghost-base".into(),
            instance_id: "ghost-instance".into(),
            local_path: path.clone(),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        fs.write(
            &fs.join(&path, SENTINEL_FILENAME),
            &serde_json::to_vec(&leftover).unwrap(),
        )
        .await
        .unwrap();
        let err = verify_or_mint(&fs, &journal, &path, "instance-1")
            .await
            .unwrap_err();
        assert!(matches!(err, SentinelError::Mismatch { .. }));
    }

    #[tokio::test]
    async fn corrupt_sentinel_is_an_error() {
        let (fs, journal, _db, root) = rig().await;
        let path = root_str(&root);
        fs.write(&fs.join(&path, SENTINEL_FILENAME), b"not json")
            .await
            .unwrap();
        let err = verify_or_mint(&fs, &journal, &path, "instance-1")
            .await
            .unwrap_err();
        assert!(matches!(err, SentinelError::Corrupt { .. }));
    }

    #[tokio::test]
    async fn instance_id_is_minted_once_and_persists() {
        let (_fs, journal, _db, _root) = rig().await;
        let first = ensure_instance_id(&journal).await.unwrap();
        let second = ensure_instance_id(&journal).await.unwrap();
        assert_eq!(first, second);
        assert!(!first.is_empty());
    }
}
