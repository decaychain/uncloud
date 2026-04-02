use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use chrono::{NaiveDate, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use tracing::{info, warn};
use uncloud_client::Client;
use uncloud_common::SyncStrategy;

use crate::journal::Journal;

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SyncConflict {
    pub server_path: String,
    pub local_path: String,
    pub conflict_copy: String,
}

#[derive(Debug, Clone)]
pub struct SyncError {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct SyncReport {
    pub uploaded: Vec<String>,
    pub downloaded: Vec<String>,
    pub deleted_local: Vec<String>,
    pub conflicts: Vec<SyncConflict>,
    pub errors: Vec<SyncError>,
}

// ── Engine ────────────────────────────────────────────────────────────────────

pub struct SyncEngine {
    journal: Journal,
    client: Arc<Client>,
    root_local_path: PathBuf,
}

impl SyncEngine {
    pub async fn new(
        db_path: &Path,
        client: Arc<Client>,
        root_local_path: PathBuf,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let opts = SqliteConnectOptions::new()
            .filename(db_path)
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await?;

        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(Self {
            journal: Journal::new(pool),
            client,
            root_local_path,
        })
    }

    /// Full sync: rebuild journal from server tree + local walk, apply all diffs.
    pub async fn full_sync(&self) -> Result<SyncReport, Box<dyn std::error::Error>> {
        info!("Starting full sync");
        self.incremental_sync().await
    }

    /// Incremental sync: re-fetch server tree, compare with journal and local mtimes.
    pub async fn incremental_sync(&self) -> Result<SyncReport, Box<dyn std::error::Error>> {
        info!("Starting incremental sync");
        let mut report = SyncReport::default();

        // 1. Fetch server tree
        let tree = self.client.sync_tree(None).await?;

        // 2. Build local map: relative_path → mtime
        let local_map = walk_local(&self.root_local_path);

        // 3. Load journal
        let journal_rows = self.journal.all().await?;
        let journal_map: HashMap<(String, String), crate::journal::SyncStateRow> = journal_rows
            .into_iter()
            .map(|r| ((r.server_id.clone(), r.item_type.clone()), r))
            .collect();

        let today = Utc::now().date_naive();

        // 4. Process server folders first (create local dirs)
        for folder in &tree.folders {
            let eff = folder.effective_strategy;
            if eff == SyncStrategy::DoNotSync {
                continue;
            }

            let local_dir = self.root_local_path.join(&folder.name);
            let key = (folder.id.clone(), "folder".to_string());

            if !local_dir.exists() {
                if let Err(e) = tokio::fs::create_dir_all(&local_dir).await {
                    report.errors.push(SyncError {
                        path: folder.name.clone(),
                        reason: e.to_string(),
                    });
                    continue;
                }
            }

            let local_path_str = local_dir.to_string_lossy().into_owned();
            let _ = self.journal.upsert(
                &folder.id,
                "folder",
                &folder.name,
                &local_path_str,
                None,
                None,
                &folder.updated_at,
                None,
                "synced",
            ).await;

            let _ = key;
        }

        // 5. Process server files
        for file in &tree.files {
            let key = (file.id.clone(), "file".to_string());
            let server_rel_path = &file.name;
            let local_path = self.root_local_path.join(server_rel_path);
            let local_path_str = local_path.to_string_lossy().into_owned();

            // Determine effective strategy for this file (use parent folder's)
            let strategy = self.file_strategy(&file.parent_id, &tree.folders);
            if strategy == SyncStrategy::DoNotSync {
                continue;
            }

            let journal_entry = journal_map.get(&key);

            match journal_entry {
                None => {
                    // New on server → download if strategy allows
                    if strategy != SyncStrategy::ClientToServer {
                        match self.client.download_file(&file.id, &local_path).await {
                            Ok(()) => {
                                let mtime = file_mtime(&local_path);
                                let _ = self.journal.upsert(
                                    &file.id, "file",
                                    server_rel_path, &local_path_str,
                                    Some(file.size_bytes), None,
                                    &file.updated_at, mtime, "synced",
                                ).await;
                                report.downloaded.push(server_rel_path.clone());
                            }
                            Err(e) => report.errors.push(SyncError {
                                path: server_rel_path.clone(),
                                reason: e.to_string(),
                            }),
                        }
                    }
                }
                Some(j) => {
                    let server_newer = file.updated_at > j.server_updated_at;
                    let local_mtime = file_mtime(&local_path);
                    let local_newer = local_mtime
                        .zip(j.local_mtime)
                        .map(|(lm, jm)| lm > jm)
                        .unwrap_or(false);

                    if server_newer && local_newer {
                        // CONFLICT: keep both
                        let conflict_name = conflict_name(server_rel_path, today);
                        let conflict_path = self.root_local_path.join(&conflict_name);
                        if let Err(e) = tokio::fs::copy(&local_path, &conflict_path).await {
                            warn!("Could not create conflict copy: {}", e);
                        }
                        match self.client.download_file(&file.id, &local_path).await {
                            Ok(()) => {
                                let new_mtime = file_mtime(&local_path);
                                let _ = self.journal.upsert(
                                    &file.id, "file",
                                    server_rel_path, &local_path_str,
                                    Some(file.size_bytes), None,
                                    &file.updated_at, new_mtime, "synced",
                                ).await;
                                report.conflicts.push(SyncConflict {
                                    server_path: server_rel_path.clone(),
                                    local_path: local_path_str.clone(),
                                    conflict_copy: conflict_path.to_string_lossy().into_owned(),
                                });
                            }
                            Err(e) => report.errors.push(SyncError {
                                path: server_rel_path.clone(),
                                reason: e.to_string(),
                            }),
                        }
                    } else if server_newer {
                        // Server changed only → download
                        if strategy != SyncStrategy::ClientToServer {
                            match self.client.download_file(&file.id, &local_path).await {
                                Ok(()) => {
                                    let new_mtime = file_mtime(&local_path);
                                    let _ = self.journal.upsert(
                                        &file.id, "file",
                                        server_rel_path, &local_path_str,
                                        Some(file.size_bytes), None,
                                        &file.updated_at, new_mtime, "synced",
                                    ).await;
                                    report.downloaded.push(server_rel_path.clone());
                                }
                                Err(e) => report.errors.push(SyncError {
                                    path: server_rel_path.clone(),
                                    reason: e.to_string(),
                                }),
                            }
                        }
                    } else if local_newer {
                        // Local changed only → update existing server file if strategy allows.
                        // We use update_file_content (not upload_file) so the server ID stays
                        // the same and the old blob is archived as a version.
                        let can_upload = matches!(
                            strategy,
                            SyncStrategy::TwoWay
                                | SyncStrategy::ClientToServer
                                | SyncStrategy::UploadOnly
                        );
                        if can_upload {
                            match self.client.update_file_content(&file.id, &local_path).await {
                                Ok(updated) => {
                                    let new_mtime = file_mtime(&local_path);
                                    let _ = self.journal.upsert(
                                        &updated.id, "file",
                                        server_rel_path, &local_path_str,
                                        Some(updated.size_bytes), None,
                                        &updated.updated_at, new_mtime, "synced",
                                    ).await;
                                    report.uploaded.push(server_rel_path.clone());
                                }
                                Err(e) => report.errors.push(SyncError {
                                    path: server_rel_path.clone(),
                                    reason: e.to_string(),
                                }),
                            }
                        }
                    }
                    // else: nothing changed — already synced
                }
            }
        }

        // 6. Handle server deletions: items in journal but NOT in server tree
        let server_file_ids: std::collections::HashSet<&str> =
            tree.files.iter().map(|f| f.id.as_str()).collect();

        for (key, j) in &journal_map {
            if key.1 != "file" {
                continue;
            }
            if !server_file_ids.contains(j.server_id.as_str()) {
                // Server deleted this file
                let local = Path::new(&j.local_path);
                let strategy = SyncStrategy::TwoWay; // default; ideally look up parent folder
                if matches!(strategy, SyncStrategy::TwoWay | SyncStrategy::ServerToClient) {
                    if local.exists() {
                        if let Err(e) = tokio::fs::remove_file(local).await {
                            report.errors.push(SyncError {
                                path: j.server_path.clone(),
                                reason: e.to_string(),
                            });
                        } else {
                            report.deleted_local.push(j.server_path.clone());
                        }
                    }
                }
                let _ = self.journal.delete(&j.server_id, "file").await;
            }
        }

        // 7. Handle new local files not in journal
        for (rel_path, mtime) in &local_map {
            let full_path = self.root_local_path.join(rel_path);
            if !full_path.is_file() {
                continue;
            }
            // Check if any journal entry matches this local_path
            let already_tracked = journal_map.values().any(|j| {
                j.item_type == "file" && j.local_path == full_path.to_string_lossy().as_ref()
            });
            if !already_tracked {
                // New local file → upload if strategy allows
                let strategy = SyncStrategy::TwoWay;
                let can_upload = matches!(
                    strategy,
                    SyncStrategy::TwoWay
                        | SyncStrategy::ClientToServer
                        | SyncStrategy::UploadOnly
                );
                if can_upload {
                    match self.client.upload_file(&full_path, None).await {
                        Ok(new_file) => {
                            let _ = self.journal.upsert(
                                &new_file.id, "file",
                                rel_path,
                                &full_path.to_string_lossy(),
                                Some(new_file.size_bytes),
                                None,
                                &new_file.updated_at,
                                Some(*mtime),
                                "synced",
                            ).await;
                            report.uploaded.push(rel_path.clone());
                        }
                        Err(e) => report.errors.push(SyncError {
                            path: rel_path.clone(),
                            reason: e.to_string(),
                        }),
                    }
                }
            }
        }

        self.journal.set_config("last_full_sync_at", &Utc::now().to_rfc3339()).await?;

        info!(
            "Sync complete: {} uploaded, {} downloaded, {} deleted, {} conflicts, {} errors",
            report.uploaded.len(),
            report.downloaded.len(),
            report.deleted_local.len(),
            report.conflicts.len(),
            report.errors.len(),
        );
        Ok(report)
    }

    /// Resolve effective strategy for a file using its parent folder's info.
    fn file_strategy(
        &self,
        parent_id: &Option<String>,
        folders: &[uncloud_common::FolderResponse],
    ) -> SyncStrategy {
        let Some(pid) = parent_id else {
            return SyncStrategy::TwoWay;
        };
        folders
            .iter()
            .find(|f| &f.id == pid)
            .map(|f| f.effective_strategy)
            .unwrap_or(SyncStrategy::TwoWay)
    }

    /// Override the sync strategy for a folder on this client.
    pub async fn set_folder_strategy(
        &self,
        folder_id: &str,
        strategy: SyncStrategy,
        local_path: Option<&Path>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let strategy_str = serde_json::to_string(&strategy)?
            .trim_matches('"')
            .to_owned();
        self.journal
            .set_folder_strategy(
                folder_id,
                &strategy_str,
                local_path.map(|p| p.to_string_lossy()).as_deref(),
            )
            .await?;
        Ok(())
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Walk the local root and return a map of relative_path → mtime (Unix seconds).
/// Filenames that must never be synced regardless of location.
const EXCLUDED_NAMES: &[&str] = &[".uncloud-sync.db"];

fn walk_local(root: &Path) -> HashMap<String, i64> {
    let mut map = HashMap::new();
    for entry in walkdir::WalkDir::new(root)
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
            .strip_prefix(root)
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
        map.insert(rel, mtime);
    }
    map
}

/// Return the mtime of a local file as Unix seconds, or `None` if unavailable.
fn file_mtime(path: &Path) -> Option<i64> {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
}

/// Generate the conflict-renamed copy filename.
///
/// `"cat.jpg"` → `"cat (conflict 2024-01-15).jpg"`
/// `"report"`  → `"report (conflict 2024-01-15)"`
pub fn conflict_name(original: &str, date: NaiveDate) -> String {
    match original.rsplit_once('.') {
        Some((stem, ext)) => format!("{} (conflict {}).{}", stem, date, ext),
        None => format!("{} (conflict {})", original, date),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn conflict_name_with_extension() {
        let d = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        assert_eq!(conflict_name("cat.jpg", d), "cat (conflict 2024-01-15).jpg");
    }

    #[test]
    fn conflict_name_no_extension() {
        let d = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        assert_eq!(conflict_name("report", d), "report (conflict 2024-01-15)");
    }
}
