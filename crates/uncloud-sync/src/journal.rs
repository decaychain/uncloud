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
        let row: Option<(String, String, String, String, Option<i64>, Option<String>, String, Option<i64>, String, String)> =
            sqlx::query_as(
                "SELECT server_id, item_type, server_path, local_path, size_bytes, checksum, \
                 server_updated_at, local_mtime, last_synced_at, sync_status \
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
        }))
    }

    pub async fn all(&self) -> sqlx::Result<Vec<SyncStateRow>> {
        let rows: Vec<(String, String, String, String, Option<i64>, Option<String>, String, Option<i64>, String, String)> =
            sqlx::query_as(
                "SELECT server_id, item_type, server_path, local_path, size_bytes, checksum, \
                 server_updated_at, local_mtime, last_synced_at, sync_status FROM sync_state",
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
}
