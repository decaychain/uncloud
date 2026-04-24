use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};

use chrono::{NaiveDate, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use tracing::{info, warn};
use uncloud_client::Client;
use uncloud_common::SyncStrategy;

use crate::fs::{LocalFs, NativeFs};
use crate::journal::{Journal, SyncLogRow};

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

// ── Internal resolved per-folder info ─────────────────────────────────────────

/// Internal scratch struct produced by [`SyncEngine::resolve_folders`]. Each
/// server folder is paired with the strategy that applies to it on *this*
/// client and the local directory where its contents should live.
#[derive(Debug, Clone)]
struct ResolvedFolder {
    strategy: SyncStrategy,
    base_path: Option<String>,
}

// ── Hooks ─────────────────────────────────────────────────────────────────────

/// Why a sync run is happening — drives the `reason` field of the bracketing
/// `SyncStart` / `SyncEnd` meta rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncTrigger {
    Auto,
    Manual,
}

pub type LogAppendedHook = Arc<dyn Fn(&SyncLogRow) + Send + Sync>;

/// Callbacks fired by the engine so embedding apps (Tauri desktop, future
/// mobile daemon) can push state to their UI without polling. Stored via
/// interior mutability so hooks can be wired after construction.
#[derive(Default, Clone)]
pub struct SyncEngineHooks {
    pub on_log_appended: Option<LogAppendedHook>,
}

pub struct SyncEngine {
    journal: Journal,
    client: Arc<Client>,
    fs: Arc<dyn LocalFs>,
    /// Client-wide root path. `None` on mobile where there is no global sync
    /// root — each picked folder carries its own `local_path` instead.
    root_local_path: Option<String>,
    hooks: RwLock<SyncEngineHooks>,
}

impl SyncEngine {
    /// Shorthand for desktop callers: opens the journal and wires a
    /// [`NativeFs`] backend. Android callers use [`SyncEngine::with_fs`].
    pub async fn new(
        db_path: &Path,
        client: Arc<Client>,
        root_local_path: Option<String>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_fs(db_path, client, Arc::new(NativeFs::new()), root_local_path).await
    }

    /// Construct a [`SyncEngine`] with an explicit [`LocalFs`] backend. The
    /// Android Tauri build uses this to wire a SAF-backed implementation.
    pub async fn with_fs(
        db_path: &Path,
        client: Arc<Client>,
        fs: Arc<dyn LocalFs>,
        root_local_path: Option<String>,
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
            fs,
            root_local_path,
            hooks: RwLock::new(SyncEngineHooks::default()),
        })
    }

    /// Wire (or replace) the callbacks fired by the engine. The desktop/mobile
    /// apps point `on_log_appended` at a closure that emits a Tauri event so
    /// the UI sees new audit rows without polling.
    pub fn set_hooks(&self, hooks: SyncEngineHooks) {
        if let Ok(mut guard) = self.hooks.write() {
            *guard = hooks;
        }
    }

    fn fire_log_append(&self, row: &SyncLogRow) {
        let hook = self
            .hooks
            .read()
            .ok()
            .and_then(|h| h.on_log_appended.clone());
        if let Some(cb) = hook {
            (cb)(row);
        }
    }

    /// Insert an audit row, fire the `on_log_appended` hook with the assigned
    /// id. Errors are warn-logged — a sync_log failure must never break the
    /// surrounding sync operation.
    async fn log_row(&self, mut row: SyncLogRow) {
        match self.journal.insert_sync_log(&row).await {
            Ok(id) => {
                row.id = id;
                self.fire_log_append(&row);
            }
            Err(e) => warn!("sync_log insert failed: {}", e),
        }
    }

    /// Return the most recent `limit` rows from the local audit log, newest
    /// first. Used by the desktop `get_local_sync_log` Tauri command.
    pub async fn recent_sync_log(&self, limit: i64) -> sqlx::Result<Vec<SyncLogRow>> {
        self.journal.recent_sync_log(limit).await
    }

    /// Drop rows older than `retention_days` and cap the table at `max_rows`.
    /// Called once at the end of every successful sync.
    pub async fn prune_sync_log(
        &self,
        retention_days: i64,
        max_rows: i64,
    ) -> sqlx::Result<u64> {
        let cutoff = (Utc::now() - chrono::Duration::days(retention_days)).to_rfc3339();
        self.journal.prune_sync_log(&cutoff, max_rows).await
    }

    // ── Per-op instrumentation helpers ────────────────────────────────────
    //
    // Each helper just writes a row to the local audit log and fires the
    // on_log_appended hook. They are sprinkled next to the existing
    // `report.X.push(...)` calls in `incremental_sync`.

    async fn log_download(&self, path: &str, is_update: bool) {
        let op = if is_update { "ContentReplaced" } else { "Created" };
        self.log_row(SyncLogRow {
            id: 0,
            timestamp: Utc::now().to_rfc3339(),
            operation: op.to_owned(),
            direction: Some("Down".to_owned()),
            resource_type: Some("File".to_owned()),
            path: path.to_owned(),
            new_path: None,
            reason: "Sync".to_owned(),
            note: None,
        })
        .await;
    }

    async fn log_upload(&self, path: &str, is_update: bool) {
        let op = if is_update { "ContentReplaced" } else { "Created" };
        self.log_row(SyncLogRow {
            id: 0,
            timestamp: Utc::now().to_rfc3339(),
            operation: op.to_owned(),
            direction: Some("Up".to_owned()),
            resource_type: Some("File".to_owned()),
            path: path.to_owned(),
            new_path: None,
            reason: "Sync".to_owned(),
            note: None,
        })
        .await;
    }

    async fn log_delete_local(&self, path: &str) {
        self.log_row(SyncLogRow {
            id: 0,
            timestamp: Utc::now().to_rfc3339(),
            operation: "Deleted".to_owned(),
            direction: Some("Down".to_owned()),
            resource_type: Some("File".to_owned()),
            path: path.to_owned(),
            new_path: None,
            reason: "Sync".to_owned(),
            note: None,
        })
        .await;
    }

    async fn log_sync_marker(
        &self,
        operation: &str,
        reason: &str,
        note: Option<String>,
    ) {
        self.log_row(SyncLogRow {
            id: 0,
            timestamp: Utc::now().to_rfc3339(),
            operation: operation.to_owned(),
            direction: None,
            resource_type: None,
            path: "run".to_owned(),
            new_path: None,
            reason: reason.to_owned(),
            note,
        })
        .await;
    }

    /// Full sync: rebuild journal from server tree + local walk, apply all diffs.
    pub async fn full_sync(&self) -> Result<SyncReport, Box<dyn std::error::Error>> {
        info!("Starting full sync");
        self.incremental_sync().await
    }

    /// Incremental sync triggered automatically (poll timer, tray "Sync Now"
    /// ends up here too for now — see `run_sync_manual` if we want to
    /// distinguish). Brackets the run with `SyncStart` / `SyncEnd` meta rows
    /// in the local audit log.
    pub async fn incremental_sync(&self) -> Result<SyncReport, Box<dyn std::error::Error>> {
        self.run_sync_inner(SyncTrigger::Auto).await
    }

    /// Variant used by the tray's "Sync Now" entry — tags the bracketing
    /// meta rows with `ManualSyncStart` / `ManualSyncEnd` so the activity
    /// view reads as a human-initiated run.
    pub async fn run_sync_manual(&self) -> Result<SyncReport, Box<dyn std::error::Error>> {
        self.run_sync_inner(SyncTrigger::Manual).await
    }

    async fn run_sync_inner(
        &self,
        trigger: SyncTrigger,
    ) -> Result<SyncReport, Box<dyn std::error::Error>> {
        info!("Starting incremental sync");
        let (start_reason, end_reason) = match trigger {
            SyncTrigger::Auto => ("Sync", "Sync"),
            SyncTrigger::Manual => ("ManualSyncStart", "ManualSyncEnd"),
        };
        self.log_sync_marker("SyncStart", start_reason, None).await;
        let started = std::time::Instant::now();
        let mut report = SyncReport::default();

        // 1. Fetch server tree
        let tree = self.client.sync_tree(None).await?;

        // 2. Resolve (strategy, base_path) for every server folder. This layers
        //    client journal overrides on top of the server's effective strategy
        //    and walks up the parent chain to compute the local directory each
        //    folder's contents live in. Folders with no resolvable base_path
        //    (Android with no root and no ancestor override) are kept in the
        //    map with `base_path = None` so subtree lookups still succeed.
        let folder_info = self.resolve_folders(&tree.folders).await?;

        // 3. Load journal
        let journal_rows = self.journal.all().await?;
        let journal_map: HashMap<(String, String), crate::journal::SyncStateRow> = journal_rows
            .into_iter()
            .map(|r| ((r.server_id.clone(), r.item_type.clone()), r))
            .collect();

        let today = Utc::now().date_naive();

        // 4. Process server folders first (create local dirs)
        for folder in &tree.folders {
            let Some(info) = folder_info.get(&folder.id) else {
                continue;
            };
            if info.strategy == SyncStrategy::DoNotSync {
                continue;
            }
            let Some(base) = info.base_path.as_ref() else {
                continue;
            };

            if let Err(e) = self.fs.create_dir_all(base).await {
                report.errors.push(SyncError {
                    path: folder.name.clone(),
                    reason: e.to_string(),
                });
                continue;
            }

            let _ = self
                .journal
                .upsert(
                    &folder.id,
                    "folder",
                    &folder.name,
                    base,
                    None,
                    None,
                    &folder.updated_at,
                    None,
                    "synced",
                )
                .await;
        }

        // 5. Process server files
        for file in &tree.files {
            let key = (file.id.clone(), "file".to_string());
            let server_rel_path = &file.name;

            // Determine effective strategy and local base for this file's parent.
            let (strategy, parent_base) = match &file.parent_id {
                None => (SyncStrategy::TwoWay, self.root_local_path.clone()),
                Some(pid) => match folder_info.get(pid) {
                    Some(info) => (info.strategy, info.base_path.clone()),
                    None => continue,
                },
            };
            if strategy == SyncStrategy::DoNotSync {
                continue;
            }
            let Some(parent_base) = parent_base else {
                // No resolvable local base — skip (Android subtree with no override).
                continue;
            };

            let local_path_str = self.fs.join(&parent_base, server_rel_path);
            let local_path = &local_path_str;

            // If the journal thinks this file is synced but it no longer exists
            // at the resolved local path, treat it as new. This catches root-path
            // changes (stale journal rows pointing at an old root) and accidental
            // local deletes.
            let local_exists = self.fs.is_file(local_path).await.unwrap_or(false);
            let journal_entry = journal_map
                .get(&key)
                .filter(|_| local_exists);

            match journal_entry {
                None => {
                    // New on server → download if strategy allows
                    let can_download = !matches!(
                        strategy,
                        SyncStrategy::ClientToServer | SyncStrategy::UploadOnly
                    );
                    if can_download {
                        match self.download_to(&file.id, local_path).await {
                            Ok(()) => {
                                let mtime = self.fs.mtime(local_path).await.ok().flatten();
                                let _ = self.journal.upsert(
                                    &file.id, "file",
                                    server_rel_path, &local_path_str,
                                    Some(file.size_bytes), None,
                                    &file.updated_at, mtime, "synced",
                                ).await;
                                report.downloaded.push(server_rel_path.clone());
                                self.log_download(server_rel_path, false).await;
                            }
                            Err(e) => report.errors.push(SyncError {
                                path: server_rel_path.clone(),
                                reason: e,
                            }),
                        }
                    }
                }
                Some(j) => {
                    let server_newer = file.updated_at > j.server_updated_at;
                    let local_mtime = self.fs.mtime(local_path).await.ok().flatten();
                    let local_newer = local_mtime
                        .zip(j.local_mtime)
                        .map(|(lm, jm)| lm > jm)
                        .unwrap_or(false);

                    if server_newer && local_newer {
                        // Both sides changed. For upload-only strategies the
                        // local version wins (push to server). For bidirectional
                        // strategies, create a conflict copy and pull server.
                        if matches!(
                            strategy,
                            SyncStrategy::ClientToServer | SyncStrategy::UploadOnly
                        ) {
                            // Local wins → upload to server.
                            match self.upload_update(&file.id, server_rel_path, local_path).await {
                                Ok(updated) => {
                                    let new_mtime = self.fs.mtime(local_path).await.ok().flatten();
                                    let _ = self.journal.upsert(
                                        &updated.id, "file",
                                        server_rel_path, &local_path_str,
                                        Some(updated.size_bytes), None,
                                        &updated.updated_at, new_mtime, "synced",
                                    ).await;
                                    report.uploaded.push(server_rel_path.clone());
                                    self.log_upload(server_rel_path, true).await;
                                }
                                Err(e) => report.errors.push(SyncError {
                                    path: server_rel_path.clone(),
                                    reason: e,
                                }),
                            }
                        } else {
                            let conflict_rel = conflict_name(server_rel_path, today);
                            let conflict_path = self.fs.join(&parent_base, &conflict_rel);
                            match self.fs.read(local_path).await {
                                Ok(cur) => {
                                    if let Err(e) = self.fs.write(&conflict_path, &cur).await {
                                        warn!("Could not create conflict copy: {}", e);
                                    }
                                }
                                Err(e) => warn!("Could not read local for conflict copy: {}", e),
                            }
                            match self.download_to(&file.id, local_path).await {
                                Ok(()) => {
                                    let new_mtime = self.fs.mtime(local_path).await.ok().flatten();
                                    let _ = self.journal.upsert(
                                        &file.id, "file",
                                        server_rel_path, &local_path_str,
                                        Some(file.size_bytes), None,
                                        &file.updated_at, new_mtime, "synced",
                                    ).await;
                                    self.log_download(server_rel_path, true).await;
                                    report.conflicts.push(SyncConflict {
                                        server_path: server_rel_path.clone(),
                                        local_path: local_path_str.clone(),
                                        conflict_copy: conflict_path,
                                    });
                                }
                                Err(e) => report.errors.push(SyncError {
                                    path: server_rel_path.clone(),
                                    reason: e,
                                }),
                            }
                        }
                    } else if server_newer {
                        // Server changed only → download
                        let can_download = !matches!(
                            strategy,
                            SyncStrategy::ClientToServer | SyncStrategy::UploadOnly
                        );
                        if can_download {
                            match self.download_to(&file.id, local_path).await {
                                Ok(()) => {
                                    let new_mtime = self.fs.mtime(local_path).await.ok().flatten();
                                    let _ = self.journal.upsert(
                                        &file.id, "file",
                                        server_rel_path, &local_path_str,
                                        Some(file.size_bytes), None,
                                        &file.updated_at, new_mtime, "synced",
                                    ).await;
                                    report.downloaded.push(server_rel_path.clone());
                                    self.log_download(server_rel_path, true).await;
                                }
                                Err(e) => report.errors.push(SyncError {
                                    path: server_rel_path.clone(),
                                    reason: e,
                                }),
                            }
                        }
                    } else if local_newer {
                        // Local changed only → update existing server file if strategy allows.
                        // We use update_file_content_bytes (not upload_bytes) so the server ID
                        // stays the same and the old blob is archived as a version.
                        let can_upload = matches!(
                            strategy,
                            SyncStrategy::TwoWay
                                | SyncStrategy::ClientToServer
                                | SyncStrategy::UploadOnly
                        );
                        if can_upload {
                            match self.upload_update(&file.id, server_rel_path, local_path).await {
                                Ok(updated) => {
                                    let new_mtime = self.fs.mtime(local_path).await.ok().flatten();
                                    let _ = self.journal.upsert(
                                        &updated.id, "file",
                                        server_rel_path, &local_path_str,
                                        Some(updated.size_bytes), None,
                                        &updated.updated_at, new_mtime, "synced",
                                    ).await;
                                    report.uploaded.push(server_rel_path.clone());
                                    self.log_upload(server_rel_path, true).await;
                                }
                                Err(e) => report.errors.push(SyncError {
                                    path: server_rel_path.clone(),
                                    reason: e,
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
                let strategy = SyncStrategy::TwoWay; // default; ideally look up parent folder
                if matches!(strategy, SyncStrategy::TwoWay | SyncStrategy::ServerToClient) {
                    match self.fs.remove_file(&j.local_path).await {
                        Ok(()) => {
                            report.deleted_local.push(j.server_path.clone());
                            self.log_delete_local(&j.server_path).await;
                        }
                        Err(e) => report.errors.push(SyncError {
                            path: j.server_path.clone(),
                            reason: e.to_string(),
                        }),
                    }
                }
                let _ = self.journal.delete(&j.server_id, "file").await;
            }
        }

        // 7. Handle new local files not in journal.
        //
        // Two passes:
        //  (a) Walk `root_local_path` (desktop) — discovers files at the global
        //      root and matches them to server folders by longest-prefix.
        //  (b) Walk each folder that has a per-folder `base_path` override and
        //      an upload-compatible strategy — covers Android (no global root)
        //      and desktop folders with explicit local_path overrides.
        //
        // Pass (b) skips folders whose base_path is already a subtree of the
        // root (those are covered by pass (a)).

        // Build a descending-length index of (base_path, folder_id, strategy).
        let mut bases: Vec<(String, String, SyncStrategy)> = folder_info
            .iter()
            .filter_map(|(id, info)| {
                info.base_path
                    .as_ref()
                    .map(|p| (p.clone(), id.clone(), info.strategy))
            })
            .collect();
        bases.sort_by_key(|(p, _, _)| std::cmp::Reverse(p.len()));

        // Pass (a): walk the global root (desktop).
        if let Some(root) = self.root_local_path.as_ref() {
            let local_entries = self.fs.walk(root).await?;

            for entry in local_entries {
                let full_path = self.fs.join(root, &entry.rel_path);
                if !self.fs.is_file(&full_path).await.unwrap_or(false) {
                    continue;
                }
                let already_tracked = journal_map
                    .values()
                    .any(|j| j.item_type == "file" && j.local_path == full_path);
                if already_tracked {
                    continue;
                }

                // Longest-prefix match against folder bases.
                let mut matched_parent: Option<(String, SyncStrategy)> = None;
                for (base, fid, strat) in &bases {
                    if let Some(rest) = full_path.strip_prefix(base.as_str()) {
                        let rest = rest.strip_prefix('/').unwrap_or(rest);
                        if !rest.is_empty() && !rest.contains('/') && !rest.contains('\\') {
                            matched_parent = Some((fid.clone(), *strat));
                            break;
                        }
                    }
                }

                let (parent_id, strategy) = match matched_parent {
                    Some((fid, s)) => (Some(fid), s),
                    None => {
                        if entry.rel_path.contains('/') || entry.rel_path.contains('\\') {
                            continue;
                        }
                        (None, SyncStrategy::TwoWay)
                    }
                };

                let can_upload = matches!(
                    strategy,
                    SyncStrategy::TwoWay
                        | SyncStrategy::ClientToServer
                        | SyncStrategy::UploadOnly
                );
                if !can_upload {
                    continue;
                }

                self.upload_new_local_file(
                    &full_path,
                    &entry.rel_path,
                    entry.mtime,
                    parent_id.as_deref(),
                    &mut report,
                )
                .await;
            }
        }

        // Pass (b): walk per-folder base_path overrides.
        // This covers Android (no root_local_path) and desktop folders with
        // explicit local_path overrides that live outside the global root.
        for (base_path, folder_id, strategy) in &bases {
            let can_upload = matches!(
                strategy,
                SyncStrategy::TwoWay
                    | SyncStrategy::ClientToServer
                    | SyncStrategy::UploadOnly
            );
            if !can_upload {
                continue;
            }

            // Skip if already covered by the root walk (pass a).
            if let Some(root) = self.root_local_path.as_ref() {
                if base_path.starts_with(root.as_str()) {
                    continue;
                }
            }

            // Only walk folders that have an explicit journal local_path
            // override — otherwise base_path was derived from root + names
            // and is already covered by pass (a).
            let has_override = self
                .journal
                .get_folder_sync_config(folder_id)
                .await
                .ok()
                .flatten()
                .map(|(_, p)| p.is_some())
                .unwrap_or(false);
            if !has_override && self.root_local_path.is_some() {
                continue;
            }

            let local_entries = match self.fs.walk(base_path).await {
                Ok(entries) => entries,
                Err(e) => {
                    warn!("Cannot walk folder override {}: {}", base_path, e);
                    continue;
                }
            };

            for entry in local_entries {
                // Only pick up files directly in this folder (not in
                // subdirectories which map to child server folders).
                if entry.rel_path.contains('/') || entry.rel_path.contains('\\') {
                    continue;
                }

                let full_path = self.fs.join(base_path, &entry.rel_path);
                if !self.fs.is_file(&full_path).await.unwrap_or(false) {
                    continue;
                }
                let already_tracked = journal_map
                    .values()
                    .any(|j| j.item_type == "file" && j.local_path == full_path);
                if already_tracked {
                    continue;
                }

                self.upload_new_local_file(
                    &full_path,
                    &entry.rel_path,
                    entry.mtime,
                    Some(folder_id),
                    &mut report,
                )
                .await;
            }
        }

        self.journal.set_config("last_full_sync_at", &Utc::now().to_rfc3339()).await?;

        let elapsed = started.elapsed();
        let note = format!(
            "{} up, {} down, {} deleted, {} conflicts, {} errors, {:.1}s",
            report.uploaded.len(),
            report.downloaded.len(),
            report.deleted_local.len(),
            report.conflicts.len(),
            report.errors.len(),
            elapsed.as_secs_f32(),
        );
        info!("Sync complete: {}", note);
        self.log_sync_marker("SyncEnd", end_reason, Some(note)).await;

        // Cap retention so the log doesn't grow without bound. Defaults match
        // the server (7 days / 10k rows) and are fine without a config knob
        // yet — if either matters we'll lift them into the desktop config.
        if let Err(e) = self.prune_sync_log(7, 10_000).await {
            warn!("sync_log prune failed: {}", e);
        }

        Ok(report)
    }

    /// Download file `id` from the server and write it to `path` via the
    /// configured [`LocalFs`] backend.
    async fn download_to(&self, id: &str, path: &str) -> Result<(), String> {
        let bytes = self
            .client
            .download_file_bytes(id)
            .await
            .map_err(|e| e.to_string())?;
        self.fs
            .write(path, &bytes)
            .await
            .map_err(|e| e.to_string())
    }

    /// Upload a newly-discovered local file to the server and record it in
    /// the journal. Used by both pass (a) and pass (b) of step 7.
    async fn upload_new_local_file(
        &self,
        full_path: &str,
        rel_path: &str,
        mtime: i64,
        parent_id: Option<&str>,
        report: &mut SyncReport,
    ) {
        let file_name = rel_path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(rel_path)
            .to_owned();

        match self.fs.read(full_path).await {
            Ok(bytes) => match self
                .client
                .upload_bytes(&file_name, bytes, parent_id)
                .await
            {
                Ok(new_file) => {
                    let _ = self
                        .journal
                        .upsert(
                            &new_file.id,
                            "file",
                            &file_name,
                            full_path,
                            Some(new_file.size_bytes),
                            None,
                            &new_file.updated_at,
                            Some(mtime),
                            "synced",
                        )
                        .await;
                    self.log_upload(&file_name, false).await;
                    report.uploaded.push(file_name);
                }
                Err(e) => report.errors.push(SyncError {
                    path: rel_path.to_owned(),
                    reason: e.to_string(),
                }),
            },
            Err(e) => report.errors.push(SyncError {
                path: rel_path.to_owned(),
                reason: e.to_string(),
            }),
        }
    }

    /// Read `path` via the configured [`LocalFs`] and upload the bytes as a
    /// new version of the server-side file `id`.
    async fn upload_update(
        &self,
        id: &str,
        server_rel_path: &str,
        path: &str,
    ) -> Result<uncloud_common::FileResponse, String> {
        let bytes = self.fs.read(path).await.map_err(|e| e.to_string())?;
        let file_name = server_rel_path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(server_rel_path);
        self.client
            .update_file_content_bytes(id, file_name, bytes)
            .await
            .map_err(|e| e.to_string())
    }

    /// Resolve `(strategy, base_path)` for every folder in the server tree.
    ///
    /// **Strategy** layers client journal overrides on top of the server's
    /// effective strategy: if the folder itself or any ancestor has an
    /// explicit client-side strategy override, that wins; otherwise the
    /// server's precomputed `effective_strategy` is used.
    ///
    /// **Base path** walks the parent chain from the folder itself upwards.
    /// The nearest ancestor with a client-side `local_path` override anchors
    /// the subtree, and the walked subpath is joined onto it. If no ancestor
    /// has an override, `root_local_path` (if set) is used as the anchor. A
    /// folder with no resolvable anchor (Android fresh install, no root, no
    /// overrides anywhere in the chain) ends up with `base_path = None` and
    /// is skipped during sync.
    async fn resolve_folders(
        &self,
        folders: &[uncloud_common::FolderResponse],
    ) -> Result<HashMap<String, ResolvedFolder>, Box<dyn std::error::Error>> {
        // Pull all journal overrides in one pass.
        let mut overrides: HashMap<String, (Option<SyncStrategy>, Option<String>)> =
            HashMap::new();
        for f in folders {
            if let Some((s_opt, p_opt)) = self.journal.get_folder_sync_config(&f.id).await? {
                let strat = match s_opt {
                    Some(s) => serde_json::from_str::<SyncStrategy>(&format!("\"{}\"", s)).ok(),
                    None => None,
                };
                overrides.insert(f.id.clone(), (strat, p_opt));
            }
        }

        let by_id: HashMap<&str, &uncloud_common::FolderResponse> =
            folders.iter().map(|f| (f.id.as_str(), f)).collect();

        let mut result: HashMap<String, ResolvedFolder> = HashMap::new();

        for folder in folders {
            // Strategy: nearest client override on the chain, else server effective.
            let strategy = {
                let mut current: Option<&uncloud_common::FolderResponse> = Some(folder);
                let mut found: Option<SyncStrategy> = None;
                while let Some(f) = current {
                    if let Some((Some(s), _)) = overrides.get(&f.id) {
                        found = Some(*s);
                        break;
                    }
                    current = f
                        .parent_id
                        .as_deref()
                        .and_then(|pid| by_id.get(pid).copied());
                }
                found.unwrap_or(folder.effective_strategy)
            };

            // Base path: nearest ancestor (including self) with a local_path
            // override, joined with the relative subpath walked over from
            // there; else client root + full relative path; else None.
            let base_path = {
                let mut stack: Vec<&str> = Vec::new();
                let mut current: Option<&uncloud_common::FolderResponse> = Some(folder);
                let mut resolved: Option<String> = None;
                loop {
                    let Some(f) = current else { break };
                    if let Some((_, Some(p))) = overrides.get(&f.id) {
                        let mut base = p.clone();
                        for name in stack.iter().rev() {
                            base = self.fs.join(&base, name);
                        }
                        resolved = Some(base);
                        break;
                    }
                    stack.push(&f.name);
                    current = f
                        .parent_id
                        .as_deref()
                        .and_then(|pid| by_id.get(pid).copied());
                }
                resolved.or_else(|| {
                    let root = self.root_local_path.as_ref()?;
                    let mut base = root.clone();
                    for name in stack.iter().rev() {
                        base = self.fs.join(&base, name);
                    }
                    Some(base)
                })
            };

            result.insert(
                folder.id.clone(),
                ResolvedFolder {
                    strategy,
                    base_path,
                },
            );
        }

        Ok(result)
    }

    /// Client-side override of the sync strategy for a folder. `None` means
    /// "no override — use the server's effective strategy".
    pub async fn get_folder_local_strategy(
        &self,
        folder_id: &str,
    ) -> Result<Option<SyncStrategy>, Box<dyn std::error::Error>> {
        let row = self.journal.get_folder_sync_config(folder_id).await?;
        let Some((strategy_opt, _)) = row else { return Ok(None) };
        match strategy_opt {
            Some(s) => Ok(Some(serde_json::from_str::<SyncStrategy>(&format!(
                "\"{}\"",
                s
            ))?)),
            None => Ok(None),
        }
    }

    /// Client-side override of the local base path for a folder. `None` means
    /// "no override — inherit from ancestor or client root".
    pub async fn get_folder_local_path(
        &self,
        folder_id: &str,
    ) -> Result<Option<String>, Box<dyn std::error::Error>> {
        let row = self.journal.get_folder_sync_config(folder_id).await?;
        Ok(row.and_then(|(_, p)| p))
    }

    /// Write (or clear) the client-side strategy override for a folder without
    /// touching the stored local path.
    pub async fn set_folder_local_strategy(
        &self,
        folder_id: &str,
        strategy: Option<SyncStrategy>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let strategy_opt: Option<String> = match strategy {
            None => None,
            Some(s) => Some(serde_json::to_string(&s)?.trim_matches('"').to_owned()),
        };
        self.journal
            .set_folder_local_strategy(folder_id, strategy_opt.as_deref())
            .await?;
        Ok(())
    }

    /// Write (or clear) the client-side local path override for a folder
    /// without touching the stored strategy.
    pub async fn set_folder_local_path(
        &self,
        folder_id: &str,
        local_path: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.journal
            .set_folder_local_path(folder_id, local_path)
            .await?;
        Ok(())
    }

    /// Resolve the full effective config for a folder:
    /// - `client_strategy`: per-device override, if any
    /// - `effective_strategy`: server's resolved strategy (all clients)
    /// - `base_path` + `base_source`: where this folder's contents live locally
    ///
    /// Base path resolution walks the breadcrumb from the folder itself up
    /// through its ancestors, stopping at the nearest journal `local_path`.
    /// If no ancestor has an override, falls back to the client root (if set).
    pub async fn get_folder_effective_config(
        &self,
        folder_id: &str,
    ) -> Result<FolderEffectiveConfig, Box<dyn std::error::Error>> {
        let client_strategy = self.get_folder_local_strategy(folder_id).await?;

        let eff = self.client.get_effective_strategy(folder_id).await?;
        let effective_strategy = eff.strategy;

        // Walk breadcrumb from leaf → root, stopping at the first journal
        // override. Breadcrumb is ordered root → leaf, so the leaf is last.
        // Names passed AFTER the override (descendants we walked over) get
        // joined onto the anchor so the final path points at the folder's
        // own local directory, not the ancestor's.
        let breadcrumb = self.client.get_folder_breadcrumb(folder_id).await?;
        let mut base_path: Option<String> = None;
        let mut base_source: BaseSource = BaseSource::None;

        let mut descendant_names: Vec<&str> = Vec::new();
        for (i, f) in breadcrumb.iter().enumerate().rev() {
            if let Some(p) = self.get_folder_local_path(&f.id).await? {
                let mut base = p;
                for name in descendant_names.iter().rev() {
                    base = self.fs.join(&base, name);
                }
                base_path = Some(base);
                base_source = if i == breadcrumb.len() - 1 {
                    BaseSource::SelfOverride
                } else {
                    BaseSource::Ancestor(f.id.clone())
                };
                break;
            }
            descendant_names.push(&f.name);
        }

        if base_path.is_none() {
            if let Some(root) = self.root_local_path.as_ref() {
                let mut base = root.clone();
                for name in descendant_names.iter().rev() {
                    base = self.fs.join(&base, name);
                }
                base_path = Some(base);
                base_source = BaseSource::ClientRoot;
            }
        }

        Ok(FolderEffectiveConfig {
            client_strategy,
            effective_strategy,
            base_path,
            base_source,
        })
    }
}

/// Where a resolved `base_path` originated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaseSource {
    /// Override set directly on the folder itself.
    SelfOverride,
    /// Inherited from an ancestor folder (holds the ancestor's id).
    Ancestor(String),
    /// Falling back to the client-wide root path.
    ClientRoot,
    /// No path available — folder has no ancestor override and client has no root.
    None,
}

impl BaseSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            BaseSource::SelfOverride => "self",
            BaseSource::Ancestor(_) => "ancestor",
            BaseSource::ClientRoot => "client_root",
            BaseSource::None => "none",
        }
    }
}

/// Result of resolving all layers of per-folder sync config.
#[derive(Debug, Clone)]
pub struct FolderEffectiveConfig {
    pub client_strategy: Option<SyncStrategy>,
    pub effective_strategy: SyncStrategy,
    pub base_path: Option<String>,
    pub base_source: BaseSource,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

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
