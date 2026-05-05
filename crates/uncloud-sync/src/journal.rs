use chrono::Utc;
use sqlx::SqlitePool;

/// Status of a single item in the sync journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncStatus {
    Synced,
    PendingUpload,
    PendingDownload,
    Conflict,
}

impl SyncStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            SyncStatus::Synced => "synced",
            SyncStatus::PendingUpload => "pending_upload",
            SyncStatus::PendingDownload => "pending_download",
            SyncStatus::Conflict => "conflict",
        }
    }
}

impl std::fmt::Display for SyncStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A row from the `sync_state` table.
#[derive(Debug, Clone)]
pub struct SyncStateRow {
    pub server_id: String,
    pub item_type: String,
    pub server_path: String,
    pub local_path: String,
    pub size_bytes: Option<i64>,
    pub checksum: Option<String>,
    pub server_updated_at: String,
    pub local_mtime: Option<i64>,
    pub last_synced_at: String,
    pub sync_status: String,
    /// `Some(<ISO 8601>)` once Phase 6.5 has noticed the local file is gone
    /// but is waiting one more scan before pushing the delete. Cleared the
    /// moment the file reappears, the watcher fires, the server changes, or
    /// the effective strategy stops permitting deletes.
    pub delete_pending_since: Option<String>,
}

/// A row from the `sync_bases` table — one per directory the user has pointed
/// sync at. The `base_id` is mirrored into a `.uncloud-root.json` sentinel at
/// the directory's root so the engine can detect "wrong volume mounted" or
/// "user pointed sync at a fresh folder" before treating any local absence as
/// a deletion.
#[derive(Debug, Clone)]
pub struct SyncBaseRow {
    pub base_id: String,
    pub local_path: String,
    pub instance_id: String,
    pub created_at: String,
}

/// A row from the `sync_log` table. Mirrors the schema in
/// `migrations/003_sync_log.sql`.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, serde::Deserialize)]
pub struct SyncLogRow {
    pub id: i64,
    pub timestamp: String,
    pub operation: String,
    pub direction: Option<String>,
    pub resource_type: Option<String>,
    pub path: String,
    pub new_path: Option<String>,
    pub reason: String,
    pub note: Option<String>,
}

impl SyncLogRow {
    /// Build a row with `id=0`; the journal assigns the real id on insert.
    pub fn new(
        timestamp: impl Into<String>,
        operation: impl Into<String>,
        reason: impl Into<String>,
        path: impl Into<String>,
    ) -> Self {
        Self {
            id: 0,
            timestamp: timestamp.into(),
            operation: operation.into(),
            direction: None,
            resource_type: None,
            path: path.into(),
            new_path: None,
            reason: reason.into(),
            note: None,
        }
    }
}

pub struct Journal {
    pool: SqlitePool,
}

impl Journal {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upsert(
        &self,
        server_id: &str,
        item_type: &str,
        server_path: &str,
        local_path: &str,
        size_bytes: Option<i64>,
        checksum: Option<&str>,
        server_updated_at: &str,
        local_mtime: Option<i64>,
        status: &str,
    ) -> sqlx::Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO sync_state
                (server_id, item_type, server_path, local_path, size_bytes, checksum,
                 server_updated_at, local_mtime, last_synced_at, sync_status)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(server_id, item_type) DO UPDATE SET
                server_path       = excluded.server_path,
                local_path        = excluded.local_path,
                size_bytes        = excluded.size_bytes,
                checksum          = excluded.checksum,
                server_updated_at = excluded.server_updated_at,
                local_mtime       = excluded.local_mtime,
                last_synced_at    = excluded.last_synced_at,
                sync_status       = excluded.sync_status
            "#,
        )
        .bind(server_id)
        .bind(item_type)
        .bind(server_path)
        .bind(local_path)
        .bind(size_bytes)
        .bind(checksum)
        .bind(server_updated_at)
        .bind(local_mtime)
        .bind(&now)
        .bind(status)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get(
        &self,
        server_id: &str,
        item_type: &str,
    ) -> sqlx::Result<Option<SyncStateRow>> {
        let row: Option<(String, String, String, String, Option<i64>, Option<String>, String, Option<i64>, String, String, Option<String>)> =
            sqlx::query_as(
                "SELECT server_id, item_type, server_path, local_path, size_bytes, checksum, \
                 server_updated_at, local_mtime, last_synced_at, sync_status, delete_pending_since \
                 FROM sync_state WHERE server_id = ? AND item_type = ?",
            )
            .bind(server_id)
            .bind(item_type)
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.map(|r| SyncStateRow {
            server_id: r.0,
            item_type: r.1,
            server_path: r.2,
            local_path: r.3,
            size_bytes: r.4,
            checksum: r.5,
            server_updated_at: r.6,
            local_mtime: r.7,
            last_synced_at: r.8,
            sync_status: r.9,
            delete_pending_since: r.10,
        }))
    }

    pub async fn all(&self) -> sqlx::Result<Vec<SyncStateRow>> {
        let rows: Vec<(String, String, String, String, Option<i64>, Option<String>, String, Option<i64>, String, String, Option<String>)> =
            sqlx::query_as(
                "SELECT server_id, item_type, server_path, local_path, size_bytes, checksum, \
                 server_updated_at, local_mtime, last_synced_at, sync_status, delete_pending_since \
                 FROM sync_state",
            )
            .fetch_all(&self.pool)
            .await?;

        Ok(rows
            .into_iter()
            .map(|r| SyncStateRow {
                server_id: r.0,
                item_type: r.1,
                server_path: r.2,
                local_path: r.3,
                size_bytes: r.4,
                checksum: r.5,
                server_updated_at: r.6,
                local_mtime: r.7,
                last_synced_at: r.8,
                sync_status: r.9,
                delete_pending_since: r.10,
            })
            .collect())
    }

    pub async fn delete(&self, server_id: &str, item_type: &str) -> sqlx::Result<()> {
        sqlx::query("DELETE FROM sync_state WHERE server_id = ? AND item_type = ?")
            .bind(server_id)
            .bind(item_type)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Drop every `sync_state` row whose `local_path` doesn't fall under any
    /// of the supplied verified base paths. Used after sentinel verification
    /// to silently discard stale journal entries left over from a previous
    /// root path, a copied DB file, or a folder whose `local_path` override
    /// was cleared. Without this prune step those rows would either trigger
    /// a false local-deletion push (Phase 6a interpreting "missing locally"
    /// authoritatively) or just sit forever as ghosts.
    ///
    /// A row is considered "inside" a base when its local_path equals the
    /// base or starts with `base + separator`. Both `/` and `\` are
    /// treated as separators so the same logic works on Unix and Windows.
    /// Returns the number of rows pruned.
    pub async fn prune_rows_outside_bases(
        &self,
        bases: &[&str],
    ) -> sqlx::Result<u64> {
        if bases.is_empty() {
            // No bases to pin against — pruning would wipe everything,
            // which is never the right call. Caller should have ensured
            // at least one base before reaching here.
            return Ok(0);
        }
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT server_id, item_type, local_path FROM sync_state",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut pruned: u64 = 0;
        for (server_id, item_type, local_path) in rows {
            let inside_any = bases.iter().any(|base| {
                if local_path == *base {
                    return true;
                }
                if let Some(rest) = local_path.strip_prefix(*base) {
                    return rest.starts_with('/') || rest.starts_with('\\');
                }
                false
            });
            if !inside_any {
                sqlx::query(
                    "DELETE FROM sync_state WHERE server_id = ? AND item_type = ?",
                )
                .bind(&server_id)
                .bind(&item_type)
                .execute(&self.pool)
                .await?;
                pruned += 1;
            }
        }
        Ok(pruned)
    }

    pub async fn get_config(&self, key: &str) -> sqlx::Result<Option<String>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT value FROM sync_config WHERE key = ?")
                .bind(key)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|r| r.0))
    }

    pub async fn set_config(&self, key: &str, value: &str) -> sqlx::Result<()> {
        sqlx::query(
            "INSERT INTO sync_config (key, value) VALUES (?, ?) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// All distinct non-NULL `local_path` overrides across every folder.
    /// Used by sentinel verification to enumerate Android's per-folder SAF
    /// picks (each is its own physical sync base).
    pub async fn all_local_path_overrides(&self) -> sqlx::Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT DISTINCT local_path FROM folder_sync_config WHERE local_path IS NOT NULL",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    /// Returns the stored strategy and local path for a folder, if a row exists.
    /// Both fields are independently nullable: `None` in either position means
    /// "no client override" for that field.
    pub async fn get_folder_sync_config(
        &self,
        folder_id: &str,
    ) -> sqlx::Result<Option<(Option<String>, Option<String>)>> {
        let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT strategy, local_path FROM folder_sync_config WHERE folder_id = ?",
        )
        .bind(folder_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Set (or clear) the client-side strategy override for a folder without
    /// touching the stored local path.
    pub async fn set_folder_local_strategy(
        &self,
        folder_id: &str,
        strategy: Option<&str>,
    ) -> sqlx::Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO folder_sync_config (folder_id, strategy, local_path, updated_at)
            VALUES (?, ?, NULL, ?)
            ON CONFLICT(folder_id) DO UPDATE SET
                strategy   = excluded.strategy,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(folder_id)
        .bind(strategy)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Set (or clear) the client-side local path override for a folder without
    /// touching the stored strategy.
    pub async fn set_folder_local_path(
        &self,
        folder_id: &str,
        local_path: Option<&str>,
    ) -> sqlx::Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO folder_sync_config (folder_id, strategy, local_path, updated_at)
            VALUES (?, NULL, ?, ?)
            ON CONFLICT(folder_id) DO UPDATE SET
                local_path = excluded.local_path,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(folder_id)
        .bind(local_path)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ── sync_bases ────────────────────────────────────────────────────────

    /// Look up a base row by its absolute local path.
    pub async fn get_base_by_path(
        &self,
        local_path: &str,
    ) -> sqlx::Result<Option<SyncBaseRow>> {
        let row: Option<(String, String, String, String)> = sqlx::query_as(
            "SELECT base_id, local_path, instance_id, created_at \
             FROM sync_bases WHERE local_path = ?",
        )
        .bind(local_path)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| SyncBaseRow {
            base_id: r.0,
            local_path: r.1,
            instance_id: r.2,
            created_at: r.3,
        }))
    }

    /// Insert a new base row with a freshly minted UUID. Returns the inserted
    /// row. Caller is responsible for ensuring there is no existing row for
    /// `local_path` (a UNIQUE constraint enforces this at the schema layer).
    pub async fn insert_base(
        &self,
        base_id: &str,
        local_path: &str,
        instance_id: &str,
    ) -> sqlx::Result<SyncBaseRow> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO sync_bases (base_id, local_path, instance_id, created_at) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(base_id)
        .bind(local_path)
        .bind(instance_id)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(SyncBaseRow {
            base_id: base_id.to_owned(),
            local_path: local_path.to_owned(),
            instance_id: instance_id.to_owned(),
            created_at: now,
        })
    }

    // ── delete_pending_since ──────────────────────────────────────────────

    /// Mark a row as "tentatively deleted as of `since`". Phase 6.5 will only
    /// commit the delete on a subsequent scan that still finds the file
    /// missing. Pass an ISO 8601 timestamp.
    pub async fn set_delete_pending(
        &self,
        server_id: &str,
        item_type: &str,
        since: &str,
    ) -> sqlx::Result<()> {
        sqlx::query(
            "UPDATE sync_state SET delete_pending_since = ? \
             WHERE server_id = ? AND item_type = ?",
        )
        .bind(since)
        .bind(server_id)
        .bind(item_type)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Clear the pending-delete flag, e.g. when the file reappears, the
    /// server-side mtime advances past it, the watcher fires for the path,
    /// or the effective strategy changes to one that doesn't push deletes.
    pub async fn clear_delete_pending(
        &self,
        server_id: &str,
        item_type: &str,
    ) -> sqlx::Result<()> {
        sqlx::query(
            "UPDATE sync_state SET delete_pending_since = NULL \
             WHERE server_id = ? AND item_type = ?",
        )
        .bind(server_id)
        .bind(item_type)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Filesystem-watcher hook: any event for `local_path` cancels a pending
    /// delete on the row pointing at it. The watcher is *only* allowed to
    /// cancel — never to commit — because we don't trust it as authoritative.
    /// Returns the number of rows cleared (0 or 1 in practice).
    pub async fn cancel_pending_delete_by_local_path(
        &self,
        local_path: &str,
    ) -> sqlx::Result<u64> {
        let res = sqlx::query(
            "UPDATE sync_state SET delete_pending_since = NULL \
             WHERE local_path = ? AND delete_pending_since IS NOT NULL",
        )
        .bind(local_path)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }

    // ── sync_log ──────────────────────────────────────────────────────────

    /// Insert an audit row. Never bubbles up — callers warn-log on error so
    /// a log failure cannot break a real sync operation.
    pub async fn insert_sync_log(&self, row: &SyncLogRow) -> sqlx::Result<i64> {
        let res = sqlx::query(
            r#"
            INSERT INTO sync_log
                (timestamp, operation, direction, resource_type, path, new_path, reason, note)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&row.timestamp)
        .bind(&row.operation)
        .bind(row.direction.as_deref())
        .bind(row.resource_type.as_deref())
        .bind(&row.path)
        .bind(row.new_path.as_deref())
        .bind(&row.reason)
        .bind(row.note.as_deref())
        .execute(&self.pool)
        .await?;
        Ok(res.last_insert_rowid())
    }

    /// Most recent `limit` rows, newest first.
    pub async fn recent_sync_log(&self, limit: i64) -> sqlx::Result<Vec<SyncLogRow>> {
        let rows = sqlx::query_as::<_, SyncLogRow>(
            r#"
            SELECT id, timestamp, operation, direction, resource_type, path,
                   new_path, reason, note
            FROM sync_log
            ORDER BY id DESC
            LIMIT ?
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Delete rows older than `cutoff_iso` (ISO-8601 string comparison works
    /// because timestamps are always in UTC `…Z` form). Also caps the table
    /// at `max_rows` by deleting the oldest excess rows.
    pub async fn prune_sync_log(
        &self,
        cutoff_iso: &str,
        max_rows: i64,
    ) -> sqlx::Result<u64> {
        let mut total = 0u64;
        let time_deleted = sqlx::query("DELETE FROM sync_log WHERE timestamp < ?")
            .bind(cutoff_iso)
            .execute(&self.pool)
            .await?
            .rows_affected();
        total += time_deleted;

        if max_rows > 0 {
            let excess_deleted = sqlx::query(
                r#"
                DELETE FROM sync_log
                WHERE id IN (
                    SELECT id FROM sync_log
                    ORDER BY id DESC
                    LIMIT -1 OFFSET ?
                )
                "#,
            )
            .bind(max_rows)
            .execute(&self.pool)
            .await?
            .rows_affected();
            total += excess_deleted;
        }

        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;
    use tempfile::TempDir;

    async fn fresh_journal() -> (Journal, TempDir) {
        let dir = TempDir::new().unwrap();
        let url = format!("sqlite://{}?mode=rwc", dir.path().join("j.db").display());
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        (Journal::new(pool), dir)
    }

    async fn seed_row(journal: &Journal, server_id: &str, local_path: &str) {
        journal
            .upsert(
                server_id,
                "file",
                server_id,
                local_path,
                Some(1),
                None,
                "2026-01-01T00:00:00Z",
                Some(1_700_000_000),
                "synced",
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn delete_pending_lifecycle() {
        let (j, _d) = fresh_journal().await;
        seed_row(&j, "f-1", "/r/foo.txt").await;

        let row = j.get("f-1", "file").await.unwrap().unwrap();
        assert!(row.delete_pending_since.is_none());

        j.set_delete_pending("f-1", "file", "2026-05-06T00:00:00Z")
            .await
            .unwrap();
        let row = j.get("f-1", "file").await.unwrap().unwrap();
        assert_eq!(
            row.delete_pending_since.as_deref(),
            Some("2026-05-06T00:00:00Z")
        );

        j.clear_delete_pending("f-1", "file").await.unwrap();
        let row = j.get("f-1", "file").await.unwrap().unwrap();
        assert!(row.delete_pending_since.is_none());
    }

    #[tokio::test]
    async fn cancel_pending_by_local_path_only_touches_matching_row() {
        let (j, _d) = fresh_journal().await;
        seed_row(&j, "f-target", "/r/wanted.txt").await;
        seed_row(&j, "f-other", "/r/unrelated.txt").await;
        j.set_delete_pending("f-target", "file", "2026-05-06T00:00:00Z")
            .await
            .unwrap();
        j.set_delete_pending("f-other", "file", "2026-05-06T00:00:00Z")
            .await
            .unwrap();

        let cleared = j
            .cancel_pending_delete_by_local_path("/r/wanted.txt")
            .await
            .unwrap();
        assert_eq!(cleared, 1);

        let target = j.get("f-target", "file").await.unwrap().unwrap();
        let other = j.get("f-other", "file").await.unwrap().unwrap();
        assert!(target.delete_pending_since.is_none());
        assert!(other.delete_pending_since.is_some());
    }

    #[tokio::test]
    async fn cancel_pending_is_noop_when_already_cleared() {
        let (j, _d) = fresh_journal().await;
        seed_row(&j, "f-1", "/r/foo.txt").await;
        let cleared = j
            .cancel_pending_delete_by_local_path("/r/foo.txt")
            .await
            .unwrap();
        assert_eq!(cleared, 0);
    }

    #[tokio::test]
    async fn insert_and_lookup_base() {
        let (j, _d) = fresh_journal().await;
        let row = j
            .insert_base("base-uuid-1", "/sync/root", "instance-x")
            .await
            .unwrap();
        assert_eq!(row.base_id, "base-uuid-1");
        let found = j.get_base_by_path("/sync/root").await.unwrap().unwrap();
        assert_eq!(found.base_id, "base-uuid-1");
        assert_eq!(found.instance_id, "instance-x");
        assert!(j.get_base_by_path("/other").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn all_local_path_overrides_returns_only_set_paths() {
        let (j, _d) = fresh_journal().await;
        j.set_folder_local_path("f-1", Some("/saf/photos"))
            .await
            .unwrap();
        j.set_folder_local_path("f-2", Some("/saf/music")).await.unwrap();
        j.set_folder_local_strategy("f-3", Some("two_way")).await.unwrap();
        let mut paths = j.all_local_path_overrides().await.unwrap();
        paths.sort();
        assert_eq!(paths, vec!["/saf/music".to_string(), "/saf/photos".into()]);
    }
}
