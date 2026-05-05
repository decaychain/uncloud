use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, RwLock};

use chrono::{NaiveDate, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use tokio::sync::watch;
use tracing::{info, warn};
use uncloud_client::Client;
use uncloud_common::SyncStrategy;

use crate::fs::{LocalFs, NativeFs};
use crate::journal::{Journal, SyncLogRow};
use crate::sentinel::{ensure_instance_id, verify_or_mint, SentinelError, SentinelStatus};

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
    /// Local-only folders that were created on the server during this run.
    pub created_remote_folders: Vec<String>,
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

/// Connection / activity state published by the engine and consumed by the
/// desktop tray. Single source of truth — the tray maps each variant to one
/// icon.
///
/// * `NotConnected` — no successful run yet, or the most recent run failed
///   for transport reasons (network down, server unreachable, auth lost).
/// * `Connected` — last run succeeded and we are ready to react to changes.
///   Includes the brief "checking for updates" interlude (what was
///   previously broadcast as `Polling`) — from the user's vantage that's
///   the same as "everything is fine, nothing to do".
/// * `Transferring` — at least one transfer has fired in the current run.
///   Sticky for the duration of the run: we enter this state on the first
///   transfer and stay there until the run ends, instead of flickering back
///   to `Connected` between sequential file ops.
/// * `Error` — most recent run failed for a non-transport reason (server
///   5xx, journal/data error). Cleared on the next successful run-start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncState {
    NotConnected,
    Connected,
    Transferring,
    Error,
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
    /// Per-run state that lets us defer the `SyncStart` audit row until we
    /// actually have something to log. Empty runs therefore leave the audit
    /// log untouched.
    run_state: RwLock<Option<RunState>>,
    /// Serializes `run_sync_inner` so concurrent callers (poll loop tick
    /// firing while a manual sync is mid-flight, mobile resume racing
    /// against a poll, etc.) queue up rather than racing on the journal,
    /// the local filesystem, and `touched_paths`. Block-until-done — the
    /// second caller waits, it is not silently dropped.
    sync_lock: tokio::sync::Mutex<()>,
    /// State broadcast: `NotConnected` until the first successful run,
    /// `Connected` between successful runs, `Transferring` once any
    /// transfer fires in the current run (sticky until run end), `Error`
    /// after a non-transport failure. Subscribed by the desktop app to
    /// drive the tray icon.
    state_tx: watch::Sender<SyncState>,
    inflight_transfers: AtomicI64,
}

struct TransferGuard<'a> {
    engine: &'a SyncEngine,
}

impl<'a> Drop for TransferGuard<'a> {
    fn drop(&mut self) {
        self.engine.leave_transfer();
    }
}

#[derive(Debug, Clone)]
struct RunState {
    /// Reason tag to use for the `SyncStart` marker — `"Sync"` for auto,
    /// `"ManualSyncStart"` when the user triggered the run. The matching
    /// `SyncEnd` reason is derived separately from [`SyncTrigger`] at the
    /// bottom of `run_sync_inner`.
    start_reason: String,
    /// Set to true the first time a real op row is logged in this run.
    emitted_start: bool,
    /// Sticky flag: once a transfer has fired in this run we stay in the
    /// `Transferring` state until the run ends, instead of flickering back
    /// to `Connected` between sequential file ops.
    any_transfer_seen: bool,
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

        let (state_tx, _) = watch::channel(SyncState::NotConnected);
        Ok(Self {
            journal: Journal::new(pool),
            client,
            fs,
            root_local_path,
            hooks: RwLock::new(SyncEngineHooks::default()),
            run_state: RwLock::new(None),
            sync_lock: tokio::sync::Mutex::new(()),
            state_tx,
            inflight_transfers: AtomicI64::new(0),
        })
    }

    /// Subscribe to engine state updates. The receiver fires once with the
    /// current value on subscription and then on every transition.
    pub fn state(&self) -> watch::Receiver<SyncState> {
        self.state_tx.subscribe()
    }

    fn set_state(&self, next: SyncState) {
        // `send_if_modified` skips the notification if the value is
        // unchanged, so subscribers see one event per real transition.
        self.state_tx.send_if_modified(|cur| {
            if *cur != next {
                *cur = next;
                true
            } else {
                false
            }
        });
    }

    fn enter_transfer(&self) {
        self.inflight_transfers.fetch_add(1, Ordering::SeqCst);
        // Mark the run sticky-Transferring on the first transfer. Subsequent
        // transfers in the same run are no-ops here; we only ever transition
        // out of `Transferring` at run end.
        let mut should_publish = false;
        if let Ok(mut g) = self.run_state.write() {
            if let Some(rs) = g.as_mut() {
                if !rs.any_transfer_seen {
                    rs.any_transfer_seen = true;
                    should_publish = true;
                }
            }
        }
        if should_publish {
            self.set_state(SyncState::Transferring);
        }
    }

    fn leave_transfer(&self) {
        // Counter bookkeeping only — the activity state is sticky for the
        // run and only flips back to `Connected` / `NotConnected` / `Error`
        // at run end (see `run_sync_inner`).
        self.inflight_transfers.fetch_sub(1, Ordering::SeqCst);
    }

    /// RAII guard around a single transfer (download / upload / remote
    /// folder create / local delete). Bumps the in-flight counter on
    /// construction and decrements on drop. The first transfer in a run
    /// publishes `SyncState::Transferring`; subsequent transfers leave the
    /// state alone. Panic-safe via `Drop`.
    fn transfer_guard(&self) -> TransferGuard<'_> {
        self.enter_transfer();
        TransferGuard { engine: self }
    }

    /// Walk an error chain from `run_sync_inner` and decide whether it is a
    /// transport-class failure (server unreachable / auth lost / network
    /// down) or a logical one. Drives the post-run state transition.
    fn classify_run_error(err: &(dyn std::error::Error + 'static)) -> SyncState {
        use uncloud_client::ClientError;
        let mut current: Option<&(dyn std::error::Error + 'static)> = Some(err);
        while let Some(e) = current {
            if let Some(ce) = e.downcast_ref::<ClientError>() {
                return match ce {
                    ClientError::Network(_) | ClientError::Unauthenticated => {
                        SyncState::NotConnected
                    }
                    ClientError::Api { status, .. }
                        if *status == 401 || *status == 403 =>
                    {
                        SyncState::NotConnected
                    }
                    _ => SyncState::Error,
                };
            }
            current = e.source();
        }
        SyncState::Error
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

    /// Filesystem-watcher hook: when the OS reports any event for
    /// `local_path`, clear a pending-delete flag on the matching journal
    /// row. This prevents a transient absence (atomic-replace editors,
    /// quick rename round-trips) from committing a delete on the next
    /// sync run. The watcher is *only* allowed to cancel — never to
    /// commit — because we don't trust it as authoritative.
    ///
    /// No-op for paths the journal doesn't know about and for rows that
    /// don't currently have a pending delete. Returns the number of rows
    /// cleared (0 or 1 in practice).
    pub async fn cancel_pending_delete_for_path(
        &self,
        local_path: &str,
    ) -> sqlx::Result<u64> {
        self.journal
            .cancel_pending_delete_by_local_path(local_path)
            .await
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
    // Each helper writes a row to the local audit log and fires the
    // on_log_appended hook. They also emit the deferred `SyncStart` marker
    // on first call so empty runs leave the log untouched. They are
    // sprinkled next to the existing `report.X.push(...)` calls in
    // `incremental_sync`.

    /// Strip `root_local_path` from an absolute local path so the log shows
    /// `photos/vacation/cat.jpg` rather than the full OS path. Falls back to
    /// the raw path on mobile (where there is no global root) or when the
    /// path lives outside the configured root.
    fn relative_display_path(&self, local_path: &str) -> String {
        if let Some(root) = &self.root_local_path {
            if let Some(rest) = local_path.strip_prefix(root.as_str()) {
                let rest = rest.trim_start_matches(['/', '\\']);
                if !rest.is_empty() {
                    return rest.to_owned();
                }
            }
        }
        local_path.to_owned()
    }

    /// Insert the deferred `SyncStart` marker if a run is active and it
    /// hasn't been emitted yet. Cheap to call repeatedly — noop once the
    /// flag is set.
    async fn ensure_start_emitted(&self) {
        let reason = {
            let mut guard = self.run_state.write().unwrap();
            match guard.as_mut() {
                Some(state) if !state.emitted_start => {
                    state.emitted_start = true;
                    state.start_reason.clone()
                }
                _ => return,
            }
        };
        self.log_sync_marker("SyncStart", &reason, None).await;
    }

    async fn log_download(&self, local_path: &str, is_update: bool) {
        self.ensure_start_emitted().await;
        let op = if is_update { "Updated from server" } else { "Downloaded" };
        self.log_row(SyncLogRow {
            id: 0,
            timestamp: Utc::now().to_rfc3339(),
            operation: op.to_owned(),
            direction: Some("Down".to_owned()),
            resource_type: Some("File".to_owned()),
            path: self.relative_display_path(local_path),
            new_path: None,
            reason: "Sync".to_owned(),
            note: None,
        })
        .await;
    }

    async fn log_upload(&self, local_path: &str, is_update: bool) {
        self.ensure_start_emitted().await;
        let op = if is_update { "Updated on server" } else { "Uploaded" };
        self.log_row(SyncLogRow {
            id: 0,
            timestamp: Utc::now().to_rfc3339(),
            operation: op.to_owned(),
            direction: Some("Up".to_owned()),
            resource_type: Some("File".to_owned()),
            path: self.relative_display_path(local_path),
            new_path: None,
            reason: "Sync".to_owned(),
            note: None,
        })
        .await;
    }

    async fn log_delete_local(&self, local_path: &str) {
        self.ensure_start_emitted().await;
        self.log_row(SyncLogRow {
            id: 0,
            timestamp: Utc::now().to_rfc3339(),
            operation: "Deleted".to_owned(),
            direction: Some("Down".to_owned()),
            resource_type: Some("File".to_owned()),
            path: self.relative_display_path(local_path),
            new_path: None,
            reason: "Sync".to_owned(),
            note: None,
        })
        .await;
    }

    /// Log a delete we pushed to the server (mirror of `log_delete_local`,
    /// just on the way up). Used by Phase 6a when the two-phase confirmation
    /// commits a local deletion.
    async fn log_delete_remote(&self, local_path: &str, resource_type: &str) {
        self.ensure_start_emitted().await;
        self.log_row(SyncLogRow {
            id: 0,
            timestamp: Utc::now().to_rfc3339(),
            operation: "Deleted on server".to_owned(),
            direction: Some("Up".to_owned()),
            resource_type: Some(resource_type.to_owned()),
            path: self.relative_display_path(local_path),
            new_path: None,
            reason: "Sync".to_owned(),
            note: None,
        })
        .await;
    }

    async fn log_create_remote_folder(&self, local_path: &str) {
        self.ensure_start_emitted().await;
        self.log_row(SyncLogRow {
            id: 0,
            timestamp: Utc::now().to_rfc3339(),
            operation: "Created on server".to_owned(),
            direction: Some("Up".to_owned()),
            resource_type: Some("Folder".to_owned()),
            path: self.relative_display_path(local_path),
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
        // Single-flight: any concurrent caller (poll loop tick + tray "Sync
        // Now", auto-login + mobile resume, etc.) queues here rather than
        // racing against another run on the journal, the local filesystem,
        // and `touched_paths`. Block-until-done — second caller waits and
        // gets its own SyncReport.
        let _guard = self.sync_lock.lock().await;
        info!("Starting incremental sync");
        let (start_reason, end_reason) = match trigger {
            SyncTrigger::Auto => ("Sync", "Sync"),
            SyncTrigger::Manual => ("ManualSyncStart", "ManualSyncEnd"),
        };
        // Arm the deferred `SyncStart` — it only lands in the log if a real
        // op fires below. Empty no-op runs produce zero rows.
        *self.run_state.write().unwrap() = Some(RunState {
            start_reason: start_reason.to_owned(),
            emitted_start: false,
            any_transfer_seen: false,
        });
        // No state transition here — the previous outcome (`Connected` /
        // `NotConnected` / `Error`) holds while we check for updates. The
        // first transfer (if any) sticky-flips us to `Transferring`; the
        // post-run classifier below publishes the final state.
        let started = std::time::Instant::now();

        // Body runs inside an async block so a `?`-driven early return
        // from any phase still reaches the post-run state classifier
        // below. The block keeps the existing indentation — Rust doesn't
        // care, and reindenting hundreds of lines would obscure the diff.
        let result: Result<SyncReport, Box<dyn std::error::Error>> = async {
        let mut report = SyncReport::default();
        // Set of local paths Phase 5 / Phase 6 has already acted on this
        // run — written by a download, pushed by an upload, removed by a
        // server-deletion echo. Phase 7 short-circuits any of these so a
        // file we just touched cannot loop back through the "new local
        // file" path. This is independent of the journal: if some future
        // bug lets the journal upsert lag or store a path string that
        // doesn't byte-equal what walkdir produces, this set still keeps
        // us honest.
        let mut touched_paths: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        // 1. Fetch server tree
        let tree = self.client.sync_tree(None).await?;

        // 2. Resolve (strategy, base_path) for every server folder. This layers
        //    client journal overrides on top of the server's effective strategy
        //    and walks up the parent chain to compute the local directory each
        //    folder's contents live in. Folders with no resolvable base_path
        //    (Android with no root and no ancestor override) are kept in the
        //    map with `base_path = None` so subtree lookups still succeed.
        let mut folder_info = self.resolve_folders(&tree.folders).await?;

        // 2.5. Verify (or mint) the `.uncloud-root.json` sentinel at every
        //      sync base before any phase that interprets file absence.
        //      Without this, an unmounted volume turns the entire next
        //      scan into "every previously-synced file is locally deleted
        //      → push deletes for all of them" — catastrophic. A failure
        //      here aborts the whole run with a structured error the
        //      desktop UI can surface to the user.
        let instance_id = ensure_instance_id(&self.journal).await?;
        let mut bases_to_verify: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        if let Some(root) = self.root_local_path.as_ref() {
            bases_to_verify.insert(root.clone());
        }
        // Each per-folder `local_path` override is its own physical sync
        // root (Android's SAF picks land here). Folders that inherit from
        // an ancestor or fall back to the client-wide root reuse a base
        // already in the set, so the SQL `DISTINCT local_path` query is
        // sufficient — no need to walk `folder_info`.
        for base in self.journal.all_local_path_overrides().await? {
            bases_to_verify.insert(base);
        }
        let mut freshly_minted_bases: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        for base in &bases_to_verify {
            match verify_or_mint(&self.fs, &self.journal, base, &instance_id).await {
                Ok((SentinelStatus::Minted, _)) => {
                    freshly_minted_bases.insert(base.clone());
                }
                Ok((SentinelStatus::Verified, _)) => {}
                Err(e @ (SentinelError::Missing { .. }
                | SentinelError::Mismatch { .. }
                | SentinelError::Corrupt { .. }
                | SentinelError::Fs(_)
                | SentinelError::Journal(_))) => {
                    report.errors.push(SyncError {
                        path: base.clone(),
                        reason: e.to_string(),
                    });
                    warn!("Aborting sync: {e}");
                    return Ok(report);
                }
            }
        }

        // 2.55. Reconcile freshly-minted bases. When `verify_or_mint`
        //       returns `Minted` for a base it means we never had a
        //       `sync_bases` row for this path — either a true first
        //       sync, or the first sync after upgrading from a
        //       pre-sentinel build. In the latter case the journal may
        //       contain rows for files that no longer exist locally
        //       (incoherent migration state). Treating those as Phase
        //       6a deletions would silently nuke server data, so
        //       instead we drop the rows and let Phase 4 re-evaluate
        //       from scratch — the same effect a fresh install would
        //       have. This branch is intentionally cheap to be wrong
        //       in: even if a row WAS coherent, dropping it just makes
        //       Phase 4 redo the compare-and-sync, not lose data.
        for base in &freshly_minted_bases {
            let rows = self.journal.all().await?;
            for row in rows {
                let inside = row.local_path == *base
                    || row
                        .local_path
                        .strip_prefix(base.as_str())
                        .map(|r| r.starts_with('/') || r.starts_with('\\'))
                        .unwrap_or(false);
                if !inside {
                    continue;
                }
                if row.item_type == "file" {
                    let exists = self.fs.is_file(&row.local_path).await.unwrap_or(false);
                    if !exists {
                        let _ = self.journal.delete(&row.server_id, "file").await;
                    }
                } else if row.item_type == "folder" {
                    let exists = self.fs.is_dir(&row.local_path).await.unwrap_or(false);
                    if !exists {
                        let _ = self.journal.delete(&row.server_id, "folder").await;
                    }
                }
            }
        }

        // 2.6. Prune journal rows pointing outside any verified base. This
        //      catches stale entries left over from a previous root path,
        //      a journal DB copied between machines, or a folder whose
        //      `local_path` override was cleared. Without this prune,
        //      Phase 4 would honour the stale row (skipping a needed
        //      download) and Phase 6a could interpret the always-missing
        //      file as a deletion to push. The sentinel above guarantees
        //      we still know which roots are real, so anything outside
        //      them is by definition not part of the current sync.
        if !bases_to_verify.is_empty() {
            let bases_ref: Vec<&str> = bases_to_verify.iter().map(String::as_str).collect();
            let pruned = self.journal.prune_rows_outside_bases(&bases_ref).await?;
            if pruned > 0 {
                info!("Pruned {pruned} stale journal row(s) outside any verified base");
            }
        }

        // 3. Load journal
        let journal_rows = self.journal.all().await?;
        let journal_map: HashMap<(String, String), crate::journal::SyncStateRow> = journal_rows
            .into_iter()
            .map(|r| ((r.server_id.clone(), r.item_type.clone()), r))
            .collect();
        for (key, j) in &journal_map {
        }

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

            // Defensive guard against server-side duplicate-name corruption:
            // if `tree.files` contains two distinct server documents that
            // resolve to the same `local_path` (a violation of the unique
            // `(owner_id, parent_id, name)` invariant the server is supposed
            // to enforce), every iteration after the first sees a freshly-
            // written file with a `mtime` newer than its own stale journal
            // row, trips `local_newer`, and uploads. We pick a winner —
            // whichever iteration touches the path first — and silently
            // skip the rest. The journal row for the duplicate stays stale
            // until the server cleans up its data.
            if touched_paths.contains(&local_path_str) {
                continue;
            }

            // Hand off the "missing locally + journal already knows about
            // this file" case to Phase 6a — that's where the two-phase
            // deletion logic lives. Re-downloading here would silently
            // undo the user's `rm`, which used to be the bug we're fixing.
            //
            // If the journal has no row at all (genuine first-sight) we
            // still fall through to the `None` arm below and download.
            let local_exists = self.fs.is_file(local_path).await.unwrap_or(false);
            let in_journal = journal_map.contains_key(&key);
            if !local_exists && in_journal {
                continue;
            }
            let journal_entry = journal_map.get(&key).filter(|_| local_exists);


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
                                touched_paths.insert(local_path_str.clone());
                                report.downloaded.push(server_rel_path.clone());
                                self.log_download(local_path, false).await;
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
                                    touched_paths.insert(local_path_str.clone());
                                    report.uploaded.push(server_rel_path.clone());
                                    self.log_upload(local_path, true).await;
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
                                    } else {
                                        // Don't let Phase 7 immediately push
                                        // the conflict copy back up.
                                        touched_paths.insert(conflict_path.clone());
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
                                    touched_paths.insert(local_path_str.clone());
                                    self.log_download(local_path, true).await;
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
                                    touched_paths.insert(local_path_str.clone());
                                    report.downloaded.push(server_rel_path.clone());
                                    self.log_download(local_path, true).await;
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
                                    touched_paths.insert(local_path_str.clone());
                                    report.uploaded.push(server_rel_path.clone());
                                    self.log_upload(local_path, true).await;
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
                            touched_paths.insert(j.local_path.clone());
                            report.deleted_local.push(j.server_path.clone());
                            self.log_delete_local(&j.local_path).await;
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

        // 6a. Handle local deletions: rows in journal whose local file is
        //     gone but the server still has them. Two-phase to absorb
        //     transient absences (e.g. a watcher missed an edit so a tool
        //     briefly write-then-replaces a file): the first scan that
        //     sees the absence sets `delete_pending_since`; only the
        //     *next* scan still finding it gone commits the delete.
        //
        //     Folder-level collapse: if a journal-known folder's local
        //     directory is missing AND every file the journal had under
        //     that folder is also missing, push a single `delete_folder`
        //     instead of N file-deletes. The server cascades the trash
        //     and leaves a single audit entry.
        let now = Utc::now().to_rfc3339();
        let server_files_by_id: std::collections::HashMap<&str, &uncloud_common::FileResponse> =
            tree.files.iter().map(|f| (f.id.as_str(), f)).collect();
        let server_folders_by_id: std::collections::HashMap<&str, &uncloud_common::FolderResponse> =
            tree.folders.iter().map(|f| (f.id.as_str(), f)).collect();

        // Resolve effective strategy for a journal row by looking up its
        // server record's parent folder. Falls back to TwoWay for root-
        // level items, which is consistent with the rest of the engine.
        let strategy_for_file = |file_id: &str| -> SyncStrategy {
            server_files_by_id
                .get(file_id)
                .and_then(|f| f.parent_id.as_deref())
                .and_then(|pid| folder_info.get(pid))
                .map(|info| info.strategy)
                .unwrap_or(SyncStrategy::TwoWay)
        };
        let strategy_for_folder = |folder_id: &str| -> SyncStrategy {
            // The folder itself either has its own info entry or is root.
            folder_info
                .get(folder_id)
                .map(|info| info.strategy)
                .unwrap_or(SyncStrategy::TwoWay)
        };
        let allows_delete_push =
            |s: SyncStrategy| matches!(s, SyncStrategy::TwoWay | SyncStrategy::ClientToServer);

        // First, identify journal-known folders whose local directory is
        // gone — those are candidates for collapse. Walk file rows
        // grouped by parent so we can count "how many of this folder's
        // children went missing in one go."
        let mut missing_files_by_parent: std::collections::HashMap<
            String,
            Vec<crate::journal::SyncStateRow>,
        > = std::collections::HashMap::new();
        let mut orphan_missing_files: Vec<crate::journal::SyncStateRow> = Vec::new();
        for ((server_id, item_type), j) in &journal_map {
            if item_type != "file" {
                continue;
            }
            if !server_files_by_id.contains_key(server_id.as_str()) {
                continue; // Phase 6 covers server-side delete echoes.
            }
            let local_exists = self.fs.is_file(&j.local_path).await.unwrap_or(false);
            if local_exists {
                continue;
            }
            let parent = server_files_by_id
                .get(server_id.as_str())
                .and_then(|f| f.parent_id.clone());
            match parent {
                Some(pid) => missing_files_by_parent
                    .entry(pid)
                    .or_default()
                    .push(j.clone()),
                None => orphan_missing_files.push(j.clone()),
            }
        }

        // Folder collapse: if a parent folder's local directory is gone
        // AND every file the server has under it is also missing, push
        // one folder-delete and mark the children touched so the
        // file-level pass below skips them.
        let mut collapsed_file_ids: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for (parent_id, missing) in &missing_files_by_parent {
            let folder_journal = match journal_map.get(&(parent_id.clone(), "folder".to_owned())) {
                Some(row) => row,
                None => continue, // Folder isn't journaled — bail to file-by-file.
            };
            let server_count = tree
                .files
                .iter()
                .filter(|f| f.parent_id.as_deref() == Some(parent_id.as_str()))
                .count();
            if missing.len() != server_count {
                continue; // Some children still present locally — partial delete.
            }
            // Every child gone — confirm the directory itself is gone.
            let dir_exists = self.fs.is_dir(&folder_journal.local_path).await.unwrap_or(false);
            if dir_exists {
                continue;
            }
            let strategy = strategy_for_folder(parent_id);
            if !allows_delete_push(strategy) {
                continue;
            }
            // Two-phase on the folder row.
            match folder_journal.delete_pending_since.as_deref() {
                None => {
                    let _ = self
                        .journal
                        .set_delete_pending(parent_id, "folder", &now)
                        .await;
                    // Also pre-mark the children so the file pass below
                    // doesn't double-mark and produce a stale pending
                    // delete after the folder is gone.
                    for j in missing {
                        let _ = self
                            .journal
                            .set_delete_pending(&j.server_id, "file", &now)
                            .await;
                        collapsed_file_ids.insert(j.server_id.clone());
                    }
                }
                Some(since) => {
                    let server_folder = server_folders_by_id.get(parent_id.as_str()).copied();
                    let folder_changed = server_folder
                        .map(|f| f.updated_at.as_str() > since)
                        .unwrap_or(false);
                    if folder_changed {
                        // Server moved/renamed the folder while we were
                        // pending — server wins, drop the pending state
                        // and let the next scan re-download.
                        let _ = self.journal.clear_delete_pending(parent_id, "folder").await;
                        for j in missing {
                            let _ = self
                                .journal
                                .clear_delete_pending(&j.server_id, "file")
                                .await;
                        }
                        continue;
                    }
                    match self.client.delete_folder(parent_id).await {
                        Ok(()) => {
                            self.log_delete_remote(&folder_journal.local_path, "Folder")
                                .await;
                            report
                                .deleted_local
                                .push(folder_journal.server_path.clone());
                            let _ = self.journal.delete(parent_id, "folder").await;
                            for j in missing {
                                let _ = self.journal.delete(&j.server_id, "file").await;
                                collapsed_file_ids.insert(j.server_id.clone());
                                touched_paths.insert(j.local_path.clone());
                            }
                            touched_paths.insert(folder_journal.local_path.clone());
                        }
                        Err(e) => report.errors.push(SyncError {
                            path: folder_journal.server_path.clone(),
                            reason: e.to_string(),
                        }),
                    }
                }
            }
        }

        // File-by-file pass for missing files that couldn't be collapsed
        // (parent folder still on disk, or parent isn't journaled, or
        // file is at the server root with no parent).
        let mut all_missing_files: Vec<crate::journal::SyncStateRow> = Vec::new();
        for (_pid, mut v) in missing_files_by_parent {
            all_missing_files.append(&mut v);
        }
        all_missing_files.append(&mut orphan_missing_files);
        for j in all_missing_files {
            if collapsed_file_ids.contains(&j.server_id) {
                continue;
            }
            let strategy = strategy_for_file(&j.server_id);
            if !allows_delete_push(strategy) {
                // Strategy doesn't push deletes (UploadOnly / DownloadOnly /
                // ServerToClient). Clear any stale pending flag and let
                // Phase 5 next scan re-download as it always has.
                if j.delete_pending_since.is_some() {
                    let _ = self.journal.clear_delete_pending(&j.server_id, "file").await;
                }
                continue;
            }
            match j.delete_pending_since.as_deref() {
                None => {
                    let _ = self
                        .journal
                        .set_delete_pending(&j.server_id, "file", &now)
                        .await;
                }
                Some(since) => {
                    let server_file = match server_files_by_id.get(j.server_id.as_str()) {
                        Some(f) => *f,
                        None => continue, // Phase 6 handled it already.
                    };
                    if server_file.updated_at.as_str() > since {
                        // Server-newer-than-pending: server wins, cancel
                        // the pending delete and let Phase 5 re-download
                        // next scan.
                        let _ = self.journal.clear_delete_pending(&j.server_id, "file").await;
                        continue;
                    }
                    match self.client.delete_file(&j.server_id).await {
                        Ok(()) => {
                            self.log_delete_remote(&j.local_path, "File").await;
                            report.deleted_local.push(j.server_path.clone());
                            let _ = self.journal.delete(&j.server_id, "file").await;
                            touched_paths.insert(j.local_path.clone());
                        }
                        Err(e) => report.errors.push(SyncError {
                            path: j.server_path.clone(),
                            reason: e.to_string(),
                        }),
                    }
                }
            }
        }

        // 6.5. Create server folders for local-only directories.
        //
        // Phase 4 only mirrors the *server* tree onto the local disk. Any
        // directory the user creates locally has no counterpart on the server,
        // so files inside it would be silently skipped by Phase 7 (they fail
        // the "parent must already be a known folder base" check). Walk the
        // local tree, find every directory that is not yet a registered base,
        // and POST `/api/folders` for it. The new folder inherits its
        // parent's effective strategy — top-level folders default to `TwoWay`
        // (matching Phase 5's root-file fallback), so a freshly created
        // local folder syncs out of the box without manual configuration.
        //
        // Two passes mirror Phase 7:
        //  (a) walk `root_local_path` (desktop)
        //  (b) walk each per-folder override base (Android, or desktop folders
        //      whose `local_path` was overridden to live outside the root).
        //
        // Newly-created folders are inserted into `folder_info` immediately
        // so subsequent dirs (deeper in the tree) and Phase 7's file walk
        // both see them as valid parents.
        async fn create_remote_dirs(
            this: &SyncEngine,
            walk_root: &str,
            attach_under: Option<(&str, &str, SyncStrategy)>,
            folder_info: &mut HashMap<String, ResolvedFolder>,
            report: &mut SyncReport,
        ) {
            let mut local_dirs = match this.fs.walk_dirs(walk_root).await {
                Ok(d) => d,
                Err(e) => {
                    warn!("walk_dirs({}) failed: {}", walk_root, e);
                    return;
                }
            };
            // Shallowest first — a child folder cannot be created before its
            // parent has been registered in `folder_info`.
            local_dirs.sort_by_key(|d| {
                d.chars().filter(|&c| c == '/' || c == '\\').count()
            });

            // Snapshot of currently-known bases as `(base_path, folder_id, strategy)`.
            // Rebuilt lazily — we push freshly-created folders directly so the
            // longest-prefix match below picks them up for any deeper dir we
            // process later in the same pass.
            let mut bases: Vec<(String, String, SyncStrategy)> = folder_info
                .iter()
                .filter_map(|(id, info)| {
                    info.base_path
                        .as_ref()
                        .map(|p| (p.clone(), id.clone(), info.strategy))
                })
                .collect();
            bases.sort_by_key(|(p, _, _)| std::cmp::Reverse(p.len()));

            for rel in local_dirs {
                let full_path = this.fs.join(walk_root, &rel);
                // Already a registered base → known server folder, skip.
                if bases.iter().any(|(p, _, _)| p == &full_path) {
                    continue;
                }

                // Determine parent: longest-prefix match against known bases,
                // falling back to `attach_under` (override-root case) or the
                // global root (desktop top-level).
                let parent_full = match rel.rfind(|c| c == '/' || c == '\\') {
                    Some(idx) => Some(this.fs.join(walk_root, &rel[..idx])),
                    None => None,
                };

                let (parent_id, parent_strategy) = match &parent_full {
                    Some(p) => match bases.iter().find(|(bp, _, _)| bp == p) {
                        Some((_, fid, s)) => (Some(fid.clone()), *s),
                        None => continue, // ancestor missing — corruption; skip
                    },
                    None => match attach_under {
                        Some((_, fid, s)) => (Some(fid.to_owned()), s),
                        None => (None, SyncStrategy::TwoWay),
                    },
                };

                let can_upload = matches!(
                    parent_strategy,
                    SyncStrategy::TwoWay
                        | SyncStrategy::ClientToServer
                        | SyncStrategy::UploadOnly
                );
                if !can_upload {
                    continue;
                }

                let name = rel
                    .rsplit(|c| c == '/' || c == '\\')
                    .next()
                    .unwrap_or(&rel)
                    .to_owned();
                if name.is_empty() {
                    continue;
                }

                let _g = this.transfer_guard();
                match this.client.create_folder(&name, parent_id.as_deref()).await {
                    Ok(folder) => {
                        let _ = this
                            .journal
                            .upsert(
                                &folder.id,
                                "folder",
                                &folder.name,
                                &full_path,
                                None,
                                None,
                                &folder.updated_at,
                                None,
                                "synced",
                            )
                            .await;
                        folder_info.insert(
                            folder.id.clone(),
                            ResolvedFolder {
                                strategy: parent_strategy,
                                base_path: Some(full_path.clone()),
                            },
                        );
                        bases.push((full_path.clone(), folder.id.clone(), parent_strategy));
                        bases.sort_by_key(|(p, _, _)| std::cmp::Reverse(p.len()));
                        this.log_create_remote_folder(&full_path).await;
                        report.created_remote_folders.push(rel.clone());
                    }
                    Err(e) => {
                        report.errors.push(SyncError {
                            path: rel.clone(),
                            reason: format!("create folder: {e}"),
                        });
                    }
                }
            }
        }

        // Pass (a): walk the global root.
        if let Some(root) = self.root_local_path.clone() {
            create_remote_dirs(self, &root, None, &mut folder_info, &mut report).await;
        }

        // Pass (b): walk per-folder override bases. Snapshot ids/paths so we
        // don't hold an immutable borrow on `folder_info` while passing it
        // mutably into `create_remote_dirs`.
        let override_walks: Vec<(String, String, SyncStrategy)> = folder_info
            .iter()
            .filter_map(|(id, info)| {
                let base = info.base_path.as_ref()?;
                if let Some(root) = self.root_local_path.as_ref() {
                    if base.starts_with(root.as_str()) {
                        return None;
                    }
                }
                Some((base.clone(), id.clone(), info.strategy))
            })
            .collect();
        for (base, fid, strat) in override_walks {
            // Only walk folders that have an explicit journal local_path
            // override — same gate as Phase 7 pass (b).
            let has_override = self
                .journal
                .get_folder_sync_config(&fid)
                .await
                .ok()
                .flatten()
                .map(|(_, p)| p.is_some())
                .unwrap_or(false);
            if !has_override && self.root_local_path.is_some() {
                continue;
            }
            create_remote_dirs(
                self,
                &base,
                Some((&base, &fid, strat)),
                &mut folder_info,
                &mut report,
            )
            .await;
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

        // The `journal_map` captured at the top of this function predates
        // Phase 5's downloads. If we use it here, freshly-downloaded files
        // would fail the `already_tracked` check and get re-uploaded on the
        // first sync. Re-read the journal so this pass sees the state Phase
        // 5 left behind. (Independently, `touched_paths` below catches the
        // same files even if the journal upsert somehow lagged or stored a
        // string that doesn't match what walkdir produces — a defence in
        // depth so a future bug in path resolution can't cause a download
        // to bounce straight back to the server.)
        let journal_rows = self.journal.all().await?;
        let journal_map: HashMap<(String, String), crate::journal::SyncStateRow> =
            journal_rows
                .into_iter()
                .map(|r| ((r.server_id.clone(), r.item_type.clone()), r))
                .collect();

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
                // We just touched this path in Phase 5/6 — never push it
                // back up in the same run, regardless of journal state.
                if touched_paths.contains(&full_path) {
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
                if touched_paths.contains(&full_path) {
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
            "{} up, {} down, {} deleted, {} folders created, {} conflicts, {} errors, {:.1}s",
            report.uploaded.len(),
            report.downloaded.len(),
            report.deleted_local.len(),
            report.created_remote_folders.len(),
            report.conflicts.len(),
            report.errors.len(),
            elapsed.as_secs_f32(),
        );
        info!("Sync complete: {}", note);
        if !report.errors.is_empty() {
        }

        // Only emit `SyncEnd` when we already emitted `SyncStart` — i.e. when
        // at least one real op landed. Empty runs leave the log untouched.
        // Read-only here; the outer scope owns the slot lifecycle so the
        // post-run classifier sees the same RunState on the error path too.
        let emitted_start = self
            .run_state
            .read()
            .unwrap()
            .as_ref()
            .map(|s| s.emitted_start)
            .unwrap_or(false);
        if emitted_start {
            self.log_sync_marker("SyncEnd", end_reason, Some(note))
                .await;
        }

        // Cap retention so the log doesn't grow without bound. Defaults match
        // the server (7 days / 10k rows) and are fine without a config knob
        // yet — if either matters we'll lift them into the desktop config.
        if let Err(e) = self.prune_sync_log(7, 10_000).await {
            warn!("sync_log prune failed: {}", e);
        }

        Ok(report)
        }
        .await;

        // Always clear the run-state slot. The body may have early-returned
        // via `?` from a phase failure, in which case the on-success take
        // never ran.
        let _ = self.run_state.write().unwrap().take();

        // Publish the final SyncState. `set_state` only emits on real
        // transition, so a successful run always lands on `Connected`
        // (clearing any prior `Error` / `NotConnected`); a failed run
        // classifies the error chain into transport vs logical.
        let final_state = match &result {
            Ok(_) => SyncState::Connected,
            Err(e) => Self::classify_run_error(e.as_ref()),
        };
        self.set_state(final_state);

        result
    }

    /// Download file `id` from the server and write it to `path` via the
    /// configured [`LocalFs`] backend.
    async fn download_to(&self, id: &str, path: &str) -> Result<(), String> {
        let _g = self.transfer_guard();
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
        let _g = self.transfer_guard();
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
                    self.log_upload(full_path, false).await;
                    report.uploaded.push(file_name);
                }
                Err(e) => {
                    report.errors.push(SyncError {
                        path: rel_path.to_owned(),
                        reason: e.to_string(),
                    })
                }
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
        let _g = self.transfer_guard();
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
