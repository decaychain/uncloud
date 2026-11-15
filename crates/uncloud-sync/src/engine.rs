use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use tokio::sync::watch;
use tracing::{info, warn};
use uncloud_client::Client;
use uncloud_common::SyncStrategy;

use futures::StreamExt;

use crate::fs::{ByteStream, LocalFs, LocalFsError, NativeFs, WalkEntry};
use crate::journal::{Journal, SyncLogRow, SyncStateRow, SyncStatus};
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
    /// Server folders renamed or moved to follow a local rename, as
    /// `(old local path, new local path)`.
    pub renamed_remote_folders: Vec<(String, String)>,
}

/// How long a local absence must persist before it is pushed to the server
/// as a deletion. The filesystem watcher can fire two scans a couple of
/// seconds apart, so "seen missing on two scans" alone does not separate a
/// deliberate delete from a directory that a build tool, a branch switch or
/// a slow move is still in the middle of replacing.
pub const DEFAULT_DELETE_CONFIRM_DELAY: Duration = Duration::from_secs(30);

/// What a local probe observed. `Unknown` means the backend could not tell
/// (permission denied, I/O error); it is never treated as absence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Presence {
    Present,
    Absent,
    Unknown,
}

/// Failures reported to the activity log, so a path that stays unreadable
/// produces one row when it starts failing instead of one per poll.
#[derive(Default)]
struct FailureMemo {
    previous_run: HashSet<String>,
    this_run: HashSet<String>,
}

/// A tracked folder whose whole subtree is missing locally, described by
/// the files the journal last saw in it: path relative to the folder (with
/// `/` separators), local mtime and checksum of the last transferred bytes.
struct VanishedFolder {
    folder_id: String,
    files: BTreeMap<String, (i64, String)>,
}

/// A local directory with no server counterpart that sits directly inside
/// a known folder or a sync root.
struct LocalOnlyDir {
    path: String,
    name: String,
    parent_id: Option<String>,
}

/// What handling one folder in the incoming-rename pass came to.
enum Relocation {
    /// Journal rows changed: the folder moved, was adopted or was forgotten.
    Progress,
    /// Nothing to do, or the folder is skipped for this run.
    Settled,
    /// The destination is still the location of the tracked folder named
    /// here, which has to move first. A folder names itself when the backend
    /// cannot change only the case of its name in one step, so that it goes
    /// through a temporary name like one side of a swap.
    Deferred(String),
}

/// A file carried across by [`SyncEngine::move_by_copy`].
struct MovedFile {
    /// `/`-separated path relative to the moved folder.
    rel: String,
    /// Its mtime at the new location.
    mtime: i64,
    content: MovedContent,
}

/// How a moved file's content relates to what the journal last synced.
enum MovedContent {
    Unchanged,
    Edited,
    /// The journal holds no checksum to compare with; the file had this
    /// mtime before the move.
    Unverified {
        before: i64,
    },
}

/// Journal key prefix of a folder move recorded before the filesystem is
/// touched, as `(from, to, copying, in_flight)`. `in_flight` is the file a
/// copying move was writing when the record was made.
const MOVE_INTENT_PREFIX: &str = "pending_folder_move:";

/// Appended to a folder's path while it is parked so that two folders can
/// swap names.
const STAGING_SUFFIX: &str = ".uncloud-moving";

fn staging_path(path: &str) -> String {
    format!("{path}{STAGING_SUFFIX}")
}

fn parse_move_intent(value: &str) -> Option<(String, String, bool, Option<String>)> {
    serde_json::from_str::<(String, String, bool, Option<String>)>(value)
        .ok()
        .or_else(|| {
            serde_json::from_str::<(String, String, bool)>(value)
                .ok()
                .map(|(from, to, copying)| (from, to, copying, None))
        })
        .or_else(|| {
            serde_json::from_str::<(String, String)>(value)
                .ok()
                .map(|(from, to)| (from, to, false, None))
        })
}

/// Whether two paths differ in nothing but letter case.
fn differs_only_in_case(a: &str, b: &str) -> bool {
    a != b && a.to_lowercase() == b.to_lowercase()
}

/// Strategies under which this client applies what the server holds to the
/// local tree.
fn takes_server_changes(strategy: SyncStrategy) -> bool {
    !matches!(
        strategy,
        SyncStrategy::DoNotSync | SyncStrategy::ClientToServer | SyncStrategy::UploadOnly
    )
}

/// A copy-move that stopped on a file only the user can settle, remembered
/// so that the next runs do not read the folder again to reach the same
/// verdict. It holds for as long as the file's mtimes on both sides stay
/// what they were, and for [`STUCK_MOVE_RECHECK`] at most. It only ever
/// repeats an error; it never lets anything be copied or removed.
struct StuckMove {
    rel: String,
    source_mtime: Option<i64>,
    dest_mtime: Option<i64>,
    reason: String,
    checked: std::time::Instant,
}

/// How long a [`StuckMove`] stands in for reading the files again. An edit
/// can keep an mtime, so the verdict is rechecked now and then regardless.
const STUCK_MOVE_RECHECK: Duration = Duration::from_secs(600);

/// Local SHA-256 results remembered across runs, keyed by path and valid
/// for one mtime. Entries a run did not ask for are dropped at the start of
/// the next one.
#[derive(Default)]
struct HashCache {
    entries: HashMap<String, (i64, String)>,
    used: HashSet<String>,
}

fn strategy_name(strategy: SyncStrategy) -> String {
    serde_json::to_string(&strategy)
        .map(|s| s.trim_matches('"').to_owned())
        .unwrap_or_default()
}

fn strategy_from_name(name: &str) -> Option<SyncStrategy> {
    serde_json::from_str(&format!("\"{name}\"")).ok()
}

/// Path of `path` relative to the directory `prefix`, with `/` separators.
/// `None` when `path` is not strictly inside `prefix`.
fn relative_under(path: &str, prefix: &str) -> Option<String> {
    let rest = path.strip_prefix(prefix)?;
    let rest = rest.strip_prefix(['/', '\\'])?;
    (!rest.is_empty()).then(|| rest.replace('\\', "/"))
}

/// How long ago a pending-delete marker was set. `None` for a marker that
/// cannot be parsed or lies in the future.
fn pending_age(since: &str, now: DateTime<Utc>) -> Option<Duration> {
    let since = DateTime::parse_from_rfc3339(since).ok()?;
    now.signed_duration_since(since.with_timezone(&Utc))
        .to_std()
        .ok()
}

/// Identity and version of every item in the subtree rooted at `root_id`,
/// the root included. `None` when the root is not in the tree.
fn subtree_fingerprint<'a>(
    tree: &'a uncloud_common::SyncTreeResponse,
    root_id: &str,
) -> Option<std::collections::BTreeSet<(&'static str, &'a str, &'a str)>> {
    let parents: HashMap<&str, Option<&str>> = tree
        .folders
        .iter()
        .map(|folder| (folder.id.as_str(), folder.parent_id.as_deref()))
        .collect();
    if !parents.contains_key(root_id) {
        return None;
    }
    let within = |start: Option<&str>| {
        let mut current = start;
        for _ in 0..=parents.len() {
            match current {
                Some(id) if id == root_id => return true,
                Some(id) => current = parents.get(id).copied().flatten(),
                None => return false,
            }
        }
        false
    };
    let mut items = std::collections::BTreeSet::new();
    for folder in &tree.folders {
        if within(Some(folder.id.as_str())) {
            items.insert(("folder", folder.id.as_str(), folder.updated_at.as_str()));
        }
    }
    for file in &tree.files {
        if within(file.parent_id.as_deref()) {
            items.insert(("file", file.id.as_str(), file.updated_at.as_str()));
        }
    }
    Some(items)
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
    delete_confirm_delay: Duration,
    failure_memo: Mutex<FailureMemo>,
    hash_cache: Mutex<HashCache>,
    /// Stuck copy-moves by move intent key.
    stuck_moves: Mutex<HashMap<String, StuckMove>>,
    /// Whether a download or copy may have left a staging file behind: true
    /// until the first sweep, since a previous process may have died in the
    /// middle of one, and again from the moment any of them starts.
    staging_dirty: AtomicBool,
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
    blocked_paths: HashSet<String>,
    /// Where a folder would live by default when its contents are mapped
    /// to another local path. Whatever sits there is not a new folder.
    shadowed_paths: HashSet<String>,
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
            delete_confirm_delay: DEFAULT_DELETE_CONFIRM_DELAY,
            failure_memo: Mutex::new(FailureMemo::default()),
            hash_cache: Mutex::new(HashCache::default()),
            stuck_moves: Mutex::new(HashMap::new()),
            staging_dirty: AtomicBool::new(true),
        })
    }

    /// Override [`DEFAULT_DELETE_CONFIRM_DELAY`]. Tests pass zero so two
    /// back-to-back scans are enough to confirm a deletion.
    pub fn with_delete_confirm_delay(mut self, delay: Duration) -> Self {
        self.delete_confirm_delay = delay;
        self
    }

    /// Subscribe to engine state updates. The receiver fires once with the
    /// current value on subscription and then on every transition.
    pub fn state(&self) -> watch::Receiver<SyncState> {
        self.state_tx.subscribe()
    }

    /// Borrow the underlying HTTP client. The desktop layer uses this to
    /// re-login with stored credentials when a sync run fails with 401 —
    /// the new session cookie lands in the same cookie jar the engine is
    /// already using, so the retry transparently succeeds.
    pub fn client(&self) -> Arc<Client> {
        Arc::clone(&self.client)
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
                    ClientError::Api { status, .. } if *status == 401 || *status == 403 => {
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
    pub async fn cancel_pending_delete_for_path(&self, local_path: &str) -> sqlx::Result<u64> {
        self.journal
            .cancel_pending_delete_by_local_path(local_path)
            .await
    }

    /// Drop rows older than `retention_days` and cap the table at `max_rows`.
    /// Called once at the end of every successful sync.
    pub async fn prune_sync_log(&self, retention_days: i64, max_rows: i64) -> sqlx::Result<u64> {
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
        let op = if is_update {
            "Updated from server"
        } else {
            "Downloaded"
        };
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
        let op = if is_update {
            "Updated on server"
        } else {
            "Uploaded"
        };
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

    /// A server change overrode a local deletion and the item was brought
    /// back. Files reuse the `Downloaded` operation so transfer counters
    /// keep working; the note carries the reason.
    async fn log_restore(&self, local_path: &str, resource_type: &str) {
        self.ensure_start_emitted().await;
        let operation = if resource_type == "File" {
            "Downloaded"
        } else {
            "Restored"
        };
        self.log_row(SyncLogRow {
            id: 0,
            timestamp: Utc::now().to_rfc3339(),
            operation: operation.to_owned(),
            direction: Some("Down".to_owned()),
            resource_type: Some(resource_type.to_owned()),
            path: self.relative_display_path(local_path),
            new_path: None,
            reason: "Sync".to_owned(),
            note: Some("Local deletion cancelled: changed on the server".to_owned()),
        })
        .await;
    }

    async fn log_rename_remote(&self, old_local_path: &str, new_local_path: &str) {
        self.ensure_start_emitted().await;
        self.log_row(SyncLogRow {
            id: 0,
            timestamp: Utc::now().to_rfc3339(),
            operation: "Renamed on server".to_owned(),
            direction: Some("Up".to_owned()),
            resource_type: Some("Folder".to_owned()),
            path: self.relative_display_path(old_local_path),
            new_path: Some(self.relative_display_path(new_local_path)),
            reason: "Sync".to_owned(),
            note: None,
        })
        .await;
    }

    /// Register a failure for this run. Returns `(first_in_run, log_it)`:
    /// the caller adds it to the report once per run, and writes an
    /// activity-log row only when the previous run did not already report
    /// the same failure.
    fn note_failure(&self, key: &str) -> (bool, bool) {
        let mut memo = self.failure_memo.lock().unwrap();
        if !memo.this_run.insert(key.to_owned()) {
            return (false, false);
        }
        (true, !memo.previous_run.contains(key))
    }

    /// A path whose state could not be observed is left alone for this run:
    /// nothing under it is downloaded over, uploaded or deleted, and the
    /// rest of the sync carries on.
    async fn report_unreadable(&self, path: &str, err: &LocalFsError, report: &mut SyncReport) {
        self.report_skipped(
            path,
            format!("skipped, local state unreadable: {err}"),
            report,
        )
        .await;
    }

    /// Leave `path` and everything under it alone for the rest of the run,
    /// for the stated reason.
    async fn report_skipped(&self, path: &str, reason: String, report: &mut SyncReport) {
        self.block_paths([path]);
        let (first_in_run, log_it) = self.note_failure(path);
        if !first_in_run {
            return;
        }
        warn!("{path}: {reason}");
        report.errors.push(SyncError {
            path: self.relative_display_path(path),
            reason: reason.clone(),
        });
        if log_it {
            self.log_sync_error_row(&self.relative_display_path(path), &reason)
                .await;
        }
    }

    async fn probe_file(&self, path: &str, report: &mut SyncReport) -> Presence {
        if self.path_blocked(path) {
            return Presence::Unknown;
        }
        match self.fs.is_file(path).await {
            Ok(true) => Presence::Present,
            Ok(false) => Presence::Absent,
            Err(e) => {
                self.report_unreadable(path, &e, report).await;
                Presence::Unknown
            }
        }
    }

    async fn probe_dir(&self, path: &str, report: &mut SyncReport) -> Presence {
        if self.path_blocked(path) {
            return Presence::Unknown;
        }
        match self.fs.is_dir(path).await {
            Ok(true) => Presence::Present,
            Ok(false) => Presence::Absent,
            Err(e) => {
                self.report_unreadable(path, &e, report).await;
                Presence::Unknown
            }
        }
    }

    /// Authoritative check made right before a deletion is pushed. Anything
    /// short of a positive "nothing is there" keeps the server copy; the
    /// quick probe and this check disagreeing (a dangling symlink, say) is
    /// reported so the item does not sit in limbo unnoticed.
    async fn confirm_gone(&self, path: &str, report: &mut SyncReport) -> bool {
        if self.subtree_blocked(path) {
            return false;
        }
        match self.fs.confirm_absent(path).await {
            Ok(true) => true,
            Ok(false) => {
                let (first_in_run, log_it) = self.note_failure(path);
                if first_in_run {
                    let reason =
                        "not deleted on the server: something still exists at this path".to_owned();
                    report.errors.push(SyncError {
                        path: self.relative_display_path(path),
                        reason: reason.clone(),
                    });
                    if log_it {
                        self.log_sync_error_row(&self.relative_display_path(path), &reason)
                            .await;
                    }
                }
                false
            }
            Err(e) => {
                self.report_unreadable(path, &e, report).await;
                false
            }
        }
    }

    fn block_paths<'p>(&self, paths: impl IntoIterator<Item = &'p str>) {
        if let Some(state) = self.run_state.write().unwrap().as_mut() {
            state
                .blocked_paths
                .extend(paths.into_iter().map(str::to_owned));
        }
    }

    fn path_blocked(&self, path: &str) -> bool {
        self.run_state
            .read()
            .unwrap()
            .as_ref()
            .is_some_and(|state| {
                state
                    .blocked_paths
                    .iter()
                    .any(|blocked| path == blocked || relative_under(path, blocked).is_some())
            })
    }

    fn subtree_blocked(&self, path: &str) -> bool {
        self.path_blocked(path)
            || self
                .run_state
                .read()
                .unwrap()
                .as_ref()
                .is_some_and(|state| {
                    state
                        .blocked_paths
                        .iter()
                        .any(|blocked| relative_under(blocked, path).is_some())
                })
    }

    fn path_shadowed(&self, path: &str) -> bool {
        self.run_state
            .read()
            .unwrap()
            .as_ref()
            .is_some_and(|state| state.shadowed_paths.contains(path))
    }

    /// Join a relative path, with either separator, onto `base`.
    fn join_rel(&self, base: &str, rel: &str) -> String {
        rel.split(['/', '\\'])
            .filter(|part| !part.is_empty())
            .fold(base.to_owned(), |path, part| self.fs.join(&path, part))
    }

    /// SHA-256 of a local file, remembered per path and mtime so that a
    /// rename which stays pending does not re-read the folder on every poll.
    async fn local_checksum(&self, path: &str, mtime: i64) -> Result<String, LocalFsError> {
        let cached = {
            let mut cache = self.hash_cache.lock().unwrap();
            cache.used.insert(path.to_owned());
            cache.entries.get(path).cloned()
        };
        if let Some((seen, checksum)) = cached {
            if seen == mtime {
                return Ok(checksum);
            }
        }
        let checksum = self.fs.sha256(path).await?;
        self.hash_cache
            .lock()
            .unwrap()
            .entries
            .insert(path.to_owned(), (mtime, checksum.clone()));
        Ok(checksum)
    }

    /// Drop remembered checksums that an overwrite, or an edit that kept the
    /// mtime, may have made wrong.
    fn forget_checksums<'p>(&self, paths: impl IntoIterator<Item = &'p str>) {
        let mut cache = self.hash_cache.lock().unwrap();
        for path in paths {
            cache.entries.remove(path);
        }
    }

    /// The remembered verdict on a stuck copy-move, if the file it stopped
    /// on still has the mtimes it had then. `mtimes` gives a file's current
    /// mtime at the source and at the destination.
    fn stuck_reason(
        &self,
        move_key: &str,
        mtimes: impl Fn(&str) -> (Option<i64>, Option<i64>),
    ) -> Option<String> {
        let mut stuck = self.stuck_moves.lock().unwrap();
        let memo = stuck.get(move_key)?;
        if memo.checked.elapsed() < STUCK_MOVE_RECHECK
            && mtimes(&memo.rel) == (memo.source_mtime, memo.dest_mtime)
        {
            return Some(memo.reason.clone());
        }
        stuck.remove(move_key);
        None
    }

    /// Remember that a copy-move cannot go on until the user settles `rel`,
    /// and return the error to report.
    fn stuck_on(
        &self,
        move_key: &str,
        rel: &str,
        (source_mtime, dest_mtime): (Option<i64>, Option<i64>),
        reason: String,
    ) -> LocalFsError {
        self.stuck_moves.lock().unwrap().insert(
            move_key.to_owned(),
            StuckMove {
                rel: rel.to_owned(),
                source_mtime,
                dest_mtime,
                reason: reason.clone(),
                checked: std::time::Instant::now(),
            },
        );
        LocalFsError::other(reason)
    }

    /// Whether `short` holds the beginning of `long` and nothing else, and
    /// is the shorter of the two. That is what a copy cut off part-way
    /// leaves behind, since copies are written front to back.
    async fn is_strict_prefix(&self, short: &str, long: &str) -> Result<bool, LocalFsError> {
        let mut short = self.fs.read_stream(short).await?;
        let mut long = self.fs.read_stream(long).await?;
        let mut ahead = bytes::Bytes::new();
        while let Some(chunk) = short.next().await {
            let mut chunk = chunk?;
            while !chunk.is_empty() {
                if ahead.is_empty() {
                    match long.next().await {
                        Some(next) => ahead = next?,
                        None => return Ok(false),
                    }
                    continue;
                }
                let shared = chunk.len().min(ahead.len());
                if chunk.split_to(shared) != ahead.split_to(shared) {
                    return Ok(false);
                }
            }
        }
        while ahead.is_empty() {
            match long.next().await {
                Some(next) => ahead = next?,
                None => return Ok(false),
            }
        }
        Ok(true)
    }

    /// Journal rows of the folder at `from` that have nothing at the
    /// matching place under `to`: files and subfolders, and the folder's own
    /// row when `to` itself is gone.
    async fn rows_missing_at(
        &self,
        from: &str,
        to: &str,
        rows: &HashMap<(String, String), SyncStateRow>,
    ) -> Result<Vec<(String, String)>, LocalFsError> {
        let listing = |entries: Vec<String>| -> HashSet<String> {
            entries
                .into_iter()
                .map(|rel| rel.replace('\\', "/"))
                .collect()
        };
        let exists = self.fs.is_dir(to).await?;
        let (files, dirs) = if exists {
            let files = self.fs.walk(to).await?;
            (
                listing(files.into_iter().map(|file| file.rel_path).collect()),
                listing(self.fs.walk_dirs(to).await?),
            )
        } else {
            (HashSet::new(), HashSet::new())
        };
        Ok(rows
            .values()
            .filter_map(|row| {
                let present = if row.local_path == from {
                    exists
                } else {
                    let rel = relative_under(&row.local_path, from)?;
                    match row.item_type.as_str() {
                        "file" => files.contains(&rel),
                        _ => dirs.contains(&rel),
                    }
                };
                (!present).then(|| (row.server_id.clone(), row.item_type.clone()))
            })
            .collect())
    }

    /// Whether `dir` holds every file in `files` at the same relative path,
    /// with the same mtime and the same content. `listing` is the walk of
    /// `dir`, keyed by `/`-separated relative path.
    async fn holds_tracked_files(
        &self,
        dir: &str,
        listing: &HashMap<String, i64>,
        files: &BTreeMap<String, (i64, String)>,
        report: &mut SyncReport,
    ) -> bool {
        if files.is_empty()
            || self.subtree_blocked(dir)
            || !files
                .iter()
                .all(|(rel, (mtime, _))| listing.get(rel) == Some(mtime))
        {
            return false;
        }
        for (rel, (mtime, checksum)) in files {
            let path = self.join_rel(dir, rel);
            match self.local_checksum(&path, *mtime).await {
                Ok(found) if found == *checksum => {}
                Ok(_) => return false,
                Err(e) => {
                    self.report_unreadable(&path, &e, report).await;
                    return false;
                }
            }
        }
        true
    }

    /// Whether the directory at `to` is the tracked folder last seen at
    /// `from`: it holds every file the journal tracked there, unchanged. A
    /// file whose content was never recorded cannot be vouched for.
    async fn is_same_folder(
        &self,
        from: &str,
        to: &str,
        rows: &HashMap<(String, String), SyncStateRow>,
        report: &mut SyncReport,
    ) -> bool {
        let mut files = BTreeMap::new();
        for row in rows.values().filter(|row| row.item_type == "file") {
            let Some(rel) = relative_under(&row.local_path, from) else {
                continue;
            };
            let (Some(mtime), Some(checksum)) = (row.local_mtime, row.checksum.clone()) else {
                return false;
            };
            files.insert(rel, (mtime, checksum));
        }
        let listing: HashMap<String, i64> = match self.fs.walk(to).await {
            Ok(entries) => entries
                .into_iter()
                .map(|entry| (entry.rel_path.replace('\\', "/"), entry.mtime))
                .collect(),
            Err(e) => {
                self.report_unreadable(to, &e, report).await;
                return false;
            }
        };
        self.holds_tracked_files(to, &listing, &files, report).await
    }

    /// Point every row at or under `from` to the same place under `to`, in
    /// one transaction that also clears the folder's recorded move.
    async fn rebase_rows(
        &self,
        folder_id: &str,
        from: &str,
        to: &str,
        rows: &mut HashMap<(String, String), SyncStateRow>,
        copied: &[MovedFile],
    ) -> sqlx::Result<()> {
        // Copies can reset mtimes. Save a baseline below verified edits in
        // the path transaction, or interruption could hide those edits.
        let copied: HashMap<&str, &MovedFile> = copied
            .iter()
            .map(|file| (file.rel.as_str(), file))
            .collect();
        let local_mtimes: Vec<(String, i64)> = rows
            .values()
            .filter(|row| row.item_type == "file")
            .filter_map(|row| {
                let rel = relative_under(&row.local_path, from)?;
                let file = copied.get(rel.as_str())?;
                let mtime = match file.content {
                    MovedContent::Unchanged => file.mtime,
                    MovedContent::Edited => file.mtime.saturating_sub(1),
                    MovedContent::Unverified { before } if row.local_mtime == Some(before) => {
                        file.mtime
                    }
                    MovedContent::Unverified { .. } => return None,
                };
                (row.local_mtime != Some(mtime)).then(|| (row.server_id.clone(), mtime))
            })
            .collect();
        let paths: Vec<_> = rows
            .values()
            .filter_map(|row| {
                let path = if row.local_path == from {
                    to.to_owned()
                } else {
                    self.join_rel(to, &relative_under(&row.local_path, from)?)
                };
                Some((row.server_id.clone(), row.item_type.clone(), path))
            })
            .collect();
        self.journal
            .rebase_folder_paths(folder_id, &paths, &local_mtimes)
            .await?;
        for (id, item_type, path) in paths {
            if let Some(row) = rows.get_mut(&(id, item_type)) {
                row.local_path = path;
                row.delete_pending_since = None;
            }
        }
        for (id, mtime) in local_mtimes {
            if let Some(row) = rows.get_mut(&(id, "file".to_owned())) {
                row.local_mtime = Some(mtime);
            }
        }
        Ok(())
    }

    /// The files a copy of `from` has to carry, or an error when the copy
    /// would leave something behind: an entry that is not a regular file or
    /// directory, or a tracked file the walk did not list. Changes nothing.
    async fn copyable_files(
        &self,
        from: &str,
        tracked: &HashMap<String, Option<String>>,
    ) -> Result<Vec<WalkEntry>, LocalFsError> {
        if let Some(entry) = self.fs.uncopyable(from).await?.first() {
            return Err(LocalFsError::other(format!(
                "{} cannot be copied to the new location (links, special files and sync \
                 state are not carried)",
                self.relative_display_path(&self.join_rel(from, entry))
            )));
        }
        let files = self.fs.walk(from).await?;
        let listed: HashSet<String> = files
            .iter()
            .map(|file| file.rel_path.replace('\\', "/"))
            .collect();
        // A walker that passes over what it cannot read must not make a
        // tracked file look moved: its row would follow the folder and the
        // file would then read as deleted.
        for rel in tracked.keys().filter(|rel| !listed.contains(*rel)) {
            let source = self.join_rel(from, rel);
            if self.fs.is_file(&source).await? {
                return Err(LocalFsError::other(format!(
                    "{} exists but could not be listed",
                    self.relative_display_path(&source)
                )));
            }
        }
        Ok(files)
    }

    /// Move a directory tree by copying it and then removing the original,
    /// for moves the backend cannot do as one rename. `files` is what
    /// [`copyable_files`](Self::copyable_files) listed. The original is
    /// only touched once every file has been copied.
    ///
    /// `tracked` maps each file the journal tracks in the folder to the
    /// checksum it last synced. A destination file that differs from its
    /// source is settled by that checksum: whichever side still matches it
    /// is the stale one. `in_flight` names the destination an interrupted run
    /// was writing. If it differs from its source it is copied again only
    /// when it is a cut-off copy, the beginning of the source and nothing
    /// more; anything else may be an edit and is kept for manual resolution.
    async fn move_by_copy(
        &self,
        from: &str,
        to: &str,
        files: Vec<WalkEntry>,
        move_key: &str,
        tracked: &HashMap<String, Option<String>>,
        in_flight: Option<String>,
    ) -> Result<Vec<MovedFile>, LocalFsError> {
        let display = |path: &str| self.relative_display_path(path);
        let listed: HashSet<String> = files
            .iter()
            .map(|file| file.rel_path.replace('\\', "/"))
            .collect();
        let mut dirs = self.fs.walk_dirs(from).await?;
        dirs.sort_by_key(String::len);
        self.fs.create_dir_all(to).await?;
        for dir in &dirs {
            self.fs.create_dir_all(&self.join_rel(to, dir)).await?;
        }
        // One listing of the destination instead of a probe per file, which
        // on SAF would list every directory from the tree root each time.
        let existing: HashMap<String, i64> = self
            .fs
            .walk(to)
            .await?
            .into_iter()
            .map(|entry| (entry.rel_path.replace('\\', "/"), entry.mtime))
            .collect();
        let strangers: HashSet<String> = self
            .fs
            .uncopyable(to)
            .await?
            .into_iter()
            .map(|rel| rel.replace('\\', "/"))
            .collect();
        let record = |in_flight: Option<&str>| {
            let value = serde_json::to_string(&(from, to, true, in_flight)).unwrap();
            async move {
                self.journal
                    .set_config(move_key, &value)
                    .await
                    .map_err(|e| LocalFsError::other(e.to_string()))
            }
        };

        let source_mtimes: HashMap<String, i64> = files
            .iter()
            .map(|file| (file.rel_path.replace('\\', "/"), file.mtime))
            .collect();
        let mtimes = |rel: &str| (source_mtimes.get(rel).copied(), existing.get(rel).copied());
        if let Some(reason) = self.stuck_reason(move_key, mtimes) {
            return Err(LocalFsError::other(reason));
        }

        // Resolve the old marker before another copy can replace it: a
        // later failure must not make this ambiguous destination look edited.
        let mut redo_in_flight = false;
        if let Some(rel) = in_flight
            .as_deref()
            .filter(|rel| existing.contains_key(*rel))
        {
            let source = self.join_rel(from, rel);
            let dest = self.join_rel(to, rel);
            let found = self.fs.sha256(&dest).await?;
            if !listed.contains(rel) {
                // With its original gone, the copy can only be vouched for
                // by what was last synced.
                if tracked.get(rel).and_then(Option::as_deref) != Some(found.as_str()) {
                    return Err(self.stuck_on(
                        move_key,
                        rel,
                        mtimes(rel),
                        format!(
                            "interrupted copy at {} needs manual resolution: its original \
                             is gone, so it cannot be told whether the copy is complete. \
                             Remove it to have the file downloaded again, after saving \
                             any changes it holds somewhere else",
                            display(&dest)
                        ),
                    ));
                }
            } else if found != self.fs.sha256(&source).await? {
                if !self.is_strict_prefix(&dest, &source).await? {
                    return Err(self.stuck_on(
                        move_key,
                        rel,
                        mtimes(rel),
                        format!(
                            "move destination differs from its source: {dest}; the \
                             interrupted copy was edited since, so it needs manual \
                             resolution. Remove it to have {source} copied again, or \
                             make the two identical; nothing was overwritten",
                            dest = display(&dest),
                            source = display(&source)
                        ),
                    ));
                }
                redo_in_flight = true;
            }
        }

        let mut dest_is_newer: HashSet<&str> = HashSet::new();
        let mut marked = in_flight.is_some();
        for file in &files {
            let rel = file.rel_path.replace('\\', "/");
            let source = self.join_rel(from, &file.rel_path);
            let dest = self.join_rel(to, &file.rel_path);
            if strangers.contains(&rel) {
                return Err(LocalFsError::other(format!(
                    "something that is not a file is in the way at {}",
                    display(&dest)
                )));
            }
            let copy = match existing.get(&rel) {
                None => true,
                // Found above to be a cut-off copy of this very file.
                Some(_) if redo_in_flight && in_flight.as_deref() == Some(rel.as_str()) => true,
                Some(_) => {
                    // These hashes authorize overwrites, so a cached mtime
                    // is not sufficient evidence that either copy is stale.
                    let found = self.fs.sha256(&dest).await?;
                    let wanted = self.fs.sha256(&source).await?;
                    let unsettled = |why: &str| {
                        self.stuck_on(
                            move_key,
                            &rel,
                            mtimes(&rel),
                            format!(
                                "move destination differs from its source: {dest}; {why}, \
                                 so it needs manual resolution. Remove the one to discard \
                                 ({dest} or {source}), or make the two identical; both \
                                 copies were kept",
                                dest = display(&dest),
                                source = display(&source)
                            ),
                        )
                    };
                    if found != wanted && in_flight.as_deref() == Some(rel.as_str()) {
                        return Err(unsettled(
                            "the interrupted copy changed while being checked",
                        ));
                    }
                    match tracked.get(&rel).and_then(Option::as_deref) {
                        _ if found == wanted => false,
                        Some(synced) if synced == found => true,
                        Some(synced) if synced == wanted => {
                            dest_is_newer.insert(file.rel_path.as_str());
                            false
                        }
                        Some(_) => return Err(unsettled("both were edited")),
                        None => {
                            return Err(unsettled(
                                "there is no synced version on record to compare them with",
                            ))
                        }
                    }
                }
            };
            if copy {
                // The marker names a destination that may be incomplete. It
                // is not by itself permission to overwrite that file later.
                record(Some(&rel)).await?;
                marked = true;
                self.forget_checksums([dest.as_str()]);
                self.staging_dirty.store(true, Ordering::SeqCst);
                if let Err(e) = self.fs.copy_file(&source, &dest).await {
                    // Whatever is at the destination now is what this call
                    // wrote over a file that was absent or already vouched
                    // stale. Only here, before anyone could have edited it,
                    // is it certain that removing it loses nothing.
                    if let Err(left) = self.fs.remove_file(&dest).await {
                        warn!("could not clear an incomplete copy: {left}");
                    }
                    return Err(e);
                }
            }
        }
        if marked {
            record(None).await?;
        }

        let mut moved = Vec::with_capacity(tracked.len().max(files.len()));
        for file in &files {
            let rel = file.rel_path.replace('\\', "/");
            let source = self.join_rel(from, &file.rel_path);
            let dest = self.join_rel(to, &file.rel_path);
            let synced = tracked.get(&rel).and_then(Option::as_deref);
            let before = self.fs.mtime(&source).await?.unwrap_or(file.mtime);
            // Read both sides again instead of trusting an mtime: an edit
            // can keep the timestamp or land within the same second while a
            // large move is running.
            let content = self.fs.sha256(&source).await?;
            let kept_edit = dest_is_newer.contains(file.rel_path.as_str());
            let intact = if kept_edit {
                synced == Some(content.as_str())
            } else {
                self.fs.sha256(&dest).await? == content
            };
            if !intact {
                self.forget_checksums([source.as_str(), dest.as_str()]);
                return Err(LocalFsError::other(format!(
                    "file changed during move: {}; both copies were kept",
                    display(&source)
                )));
            }
            let mtime = self.fs.mtime(&dest).await?.unwrap_or(before);
            self.fs.remove_file(&source).await?;
            let content = match synced {
                _ if kept_edit => MovedContent::Edited,
                Some(synced) if synced == content => MovedContent::Unchanged,
                Some(_) => MovedContent::Edited,
                None => MovedContent::Unverified { before },
            };
            moved.push(MovedFile {
                rel,
                mtime,
                content,
            });
        }
        // Files an earlier, interrupted run already carried across.
        for (rel, synced) in tracked {
            let (Some(synced), Some(mtime)) = (synced, existing.get(rel)) else {
                continue;
            };
            if listed.contains(rel) {
                continue;
            }
            let found = self.fs.sha256(&self.join_rel(to, rel)).await?;
            moved.push(MovedFile {
                rel: rel.clone(),
                mtime: *mtime,
                content: if found == *synced {
                    MovedContent::Unchanged
                } else {
                    MovedContent::Edited
                },
            });
        }
        // A file that appeared in the original while it was being moved
        // keeps its directory alive. The move itself is complete, so that
        // is left behind instead of failing the relocation on every run.
        for dir in dirs.iter().rev() {
            if let Err(e) = self.fs.remove_dir(&self.join_rel(from, dir)).await {
                warn!("left behind after move: {e}");
            }
        }
        if let Err(e) = self.fs.remove_dir(from).await {
            warn!("left behind after move: {e}");
        }
        Ok(moved)
    }

    /// Bring local directories in line with folders the server renamed or
    /// moved, before any later phase interprets the new layout.
    ///
    /// A folder is handled when its journal row and its resolved local path
    /// disagree. Folders are taken parents first; one whose destination is
    /// still another tracked folder's location waits for that folder, and a
    /// swap is broken by parking one side under a temporary name.
    async fn reconcile_remote_folder_paths(
        &self,
        tree: &uncloud_common::SyncTreeResponse,
        folder_info: &mut HashMap<String, ResolvedFolder>,
        ancestors: &HashMap<String, HashSet<String>>,
        rows: &mut HashMap<(String, String), SyncStateRow>,
        bases: &std::collections::BTreeSet<String>,
        report: &mut SyncReport,
    ) -> sqlx::Result<()> {
        // A recorded move matters only while the journal still shows the
        // folder at its origin and the destination is still where it is
        // headed. Anything else describes a situation that has passed.
        for (key, value) in self.journal.configs_with_prefix(MOVE_INTENT_PREFIX).await? {
            let folder_id = &key[MOVE_INTENT_PREFIX.len()..];
            let folder_key = (folder_id.to_owned(), "folder".to_owned());
            let intent = parse_move_intent(&value).filter(|(from, to, ..)| {
                from != to
                    && rows
                        .get(&folder_key)
                        .is_some_and(|row| row.local_path == *from)
                    && (*to == staging_path(from)
                        || folder_info
                            .get(folder_id)
                            .and_then(|info| info.base_path.as_deref())
                            == Some(to.as_str()))
            });
            let Some((from, to, ..)) = intent else {
                self.journal.delete_config(&key).await?;
                continue;
            };
            // Interrupted after a folder was parked for a swap. This has to
            // be settled before anything moves, while "absent at its origin"
            // can only mean the folder itself went away.
            if to == staging_path(&from) {
                if self.probe_dir(&from, report).await == Presence::Absent
                    && self.probe_dir(&to, report).await == Presence::Present
                {
                    self.rebase_rows(folder_id, &from, &to, rows, &[]).await?;
                } else {
                    self.journal.delete_config(&key).await?;
                }
            }
        }

        // Find unreadable directories under any folder about to move before
        // the first move is made, whatever order the folders come in.
        for row in rows.values().filter(|row| {
            row.item_type == "folder"
                && folder_info.get(&row.server_id).is_some_and(|info| {
                    info.strategy != SyncStrategy::DoNotSync
                        && info.base_path.as_deref() != Some(row.local_path.as_str())
                })
        }) {
            self.probe_dir(&row.local_path, report).await;
        }

        let mut queue: Vec<&uncloud_common::FolderResponse> = tree.folders.iter().collect();
        queue.sort_by_key(|folder| ancestors.get(&folder.id).map_or(0, HashSet::len));
        let mut parked: HashSet<String> = HashSet::new();
        loop {
            let mut deferred: Vec<(&uncloud_common::FolderResponse, String)> = Vec::new();
            let mut progressed = false;
            for folder in queue {
                match self
                    .relocate_folder(folder, folder_info, ancestors, rows, bases, report)
                    .await?
                {
                    Relocation::Progress => progressed = true,
                    Relocation::Settled => {}
                    Relocation::Deferred(holder) => deferred.push((folder, holder)),
                }
            }
            if deferred.is_empty() {
                break;
            }
            if !progressed {
                // Nothing can move. Folders waiting on each other in a ring
                // (a swap) are freed by parking one of them; anything else
                // is waiting on a folder that is staying where it is.
                let waits_on: HashMap<&str, &str> = deferred
                    .iter()
                    .map(|(folder, holder)| (folder.id.as_str(), holder.as_str()))
                    .collect();
                let in_ring = |start: &str| {
                    let mut current = start;
                    for _ in 0..waits_on.len() {
                        match waits_on.get(current) {
                            Some(next) if *next == start => return true,
                            Some(next) => current = next,
                            None => return false,
                        }
                    }
                    false
                };
                let mut freed = false;
                for (folder, _) in &deferred {
                    if in_ring(&folder.id)
                        && parked.insert(folder.id.clone())
                        && self.park_folder(folder, rows, report).await?
                    {
                        freed = true;
                        break;
                    }
                }
                if !freed {
                    for (folder, holder) in deferred {
                        // Covered by the report on the ancestor it waits for.
                        if ancestors
                            .get(&folder.id)
                            .is_some_and(|above| above.contains(&holder))
                        {
                            continue;
                        }
                        let Some(from) = rows
                            .get(&(folder.id.clone(), "folder".to_owned()))
                            .map(|row| row.local_path.clone())
                        else {
                            continue;
                        };
                        self.report_skipped(
                            &from,
                            "could not follow the server rename: another folder still \
                             occupies the new location"
                                .to_owned(),
                            report,
                        )
                        .await;
                        Self::unresolve_subtree(&folder.id, folder_info, ancestors, bases);
                    }
                    break;
                }
            }
            queue = deferred.into_iter().map(|(folder, _)| folder).collect();
        }
        Ok(())
    }

    /// Take a folder and the folders beneath it out of this run without
    /// blocking the place it is headed, which someone else still occupies:
    /// with no resolved local path, no later phase touches them. Folders
    /// with a local path of their own are unaffected.
    fn unresolve_subtree(
        folder_id: &str,
        folder_info: &mut HashMap<String, ResolvedFolder>,
        ancestors: &HashMap<String, HashSet<String>>,
        bases: &std::collections::BTreeSet<String>,
    ) {
        for (id, info) in folder_info.iter_mut() {
            let inside = id == folder_id
                || ancestors
                    .get(id)
                    .is_some_and(|above| above.contains(folder_id));
            if inside
                && !info
                    .base_path
                    .as_ref()
                    .is_some_and(|base| bases.contains(base))
            {
                info.base_path = None;
            }
        }
    }

    /// Give a folder a temporary name so that the folder waiting for its
    /// current one can move. The journal follows, so an interruption leaves
    /// the folder tracked at the temporary name and the next run finishes
    /// the job.
    async fn park_folder(
        &self,
        folder: &uncloud_common::FolderResponse,
        rows: &mut HashMap<(String, String), SyncStateRow>,
        report: &mut SyncReport,
    ) -> sqlx::Result<bool> {
        let Some(from) = rows
            .get(&(folder.id.clone(), "folder".to_owned()))
            .map(|row| row.local_path.clone())
        else {
            return Ok(false);
        };
        if self.probe_dir(&from, report).await != Presence::Present {
            return Ok(false);
        }
        let parked = staging_path(&from);
        let move_key = format!("{MOVE_INTENT_PREFIX}{}", folder.id);
        let result = async {
            if !self.fs.confirm_absent(&parked).await? {
                return Err(LocalFsError::other(format!("{parked} already exists")));
            }
            self.journal
                .set_config(
                    &move_key,
                    &serde_json::to_string(&(&from, &parked, false)).unwrap(),
                )
                .await
                .map_err(|e| LocalFsError::other(e.to_string()))?;
            self.fs.rename(&from, &parked).await
        }
        .await;
        if let Err(e) = result {
            self.journal.delete_config(&move_key).await?;
            self.report_skipped(
                &from,
                format!("could not follow the server rename: {e}"),
                report,
            )
            .await;
            return Ok(false);
        }
        self.rebase_rows(&folder.id, &from, &parked, rows, &[])
            .await?;
        Ok(true)
    }

    async fn relocate_folder(
        &self,
        folder: &uncloud_common::FolderResponse,
        folder_info: &mut HashMap<String, ResolvedFolder>,
        ancestors: &HashMap<String, HashSet<String>>,
        rows: &mut HashMap<(String, String), SyncStateRow>,
        bases: &std::collections::BTreeSet<String>,
        report: &mut SyncReport,
    ) -> sqlx::Result<Relocation> {
        let Some(info) = folder_info.get(&folder.id) else {
            return Ok(Relocation::Settled);
        };
        let strategy = info.strategy;
        if strategy == SyncStrategy::DoNotSync {
            return Ok(Relocation::Settled);
        }
        let Some(to) = info.base_path.clone() else {
            return Ok(Relocation::Settled);
        };
        let Some(from) = rows
            .get(&(folder.id.clone(), "folder".to_owned()))
            .map(|row| row.local_path.clone())
        else {
            return Ok(Relocation::Settled);
        };
        if from == to {
            return Ok(Relocation::Settled);
        }

        // The folder was given its own local path, or lost it: the mapping
        // changed, not the server. Nothing is moved. What was tracked at the
        // old location is forgotten and the folder syncs at the new one as
        // if seen for the first time; rows belonging to another base nested
        // in the old location are not this folder's to forget.
        if bases.contains(&to) || bases.contains(&from) {
            let nested: Vec<&String> = bases
                .iter()
                .filter(|base| relative_under(base, &from).is_some())
                .collect();
            let keys: Vec<(String, String)> = rows
                .values()
                .filter(|row| {
                    row.local_path == from || relative_under(&row.local_path, &from).is_some()
                })
                .filter(|row| {
                    !nested.iter().any(|base| {
                        row.local_path == **base || relative_under(&row.local_path, base).is_some()
                    })
                })
                .map(|row| (row.server_id.clone(), row.item_type.clone()))
                .collect();
            self.journal.retire_folder_path(&from, &keys).await?;
            for key in &keys {
                rows.remove(key);
            }
            return Ok(Relocation::Progress);
        }

        let failed = |reason: String| format!("could not follow the server rename: {reason}");

        // A picked sync base has its own sentinel and permissions; carrying
        // it along inside a moved directory would invalidate that mapping.
        if bases.iter().any(|base| {
            relative_under(base, &from).is_some() || relative_under(base, &to).is_some()
        }) {
            self.report_skipped(
                &from,
                failed("a folder inside it has its own local path".to_owned()),
                report,
            )
            .await;
            self.block_paths([to.as_str()]);
            return Ok(Relocation::Settled);
        }
        if self.subtree_blocked(&from) || self.subtree_blocked(&to) {
            self.block_paths([from.as_str(), to.as_str()]);
            return Ok(Relocation::Settled);
        }
        // Both of this folder's paths are derived from its ancestors'. While
        // one of them is still waiting to move, the folder travels with it
        // and has nothing of its own to do yet.
        let waiting_above = ancestors.get(&folder.id).and_then(|above| {
            above.iter().find(|id| {
                let Some(info) = folder_info.get(*id) else {
                    return false;
                };
                info.strategy != SyncStrategy::DoNotSync
                    && info.base_path.as_ref().is_some_and(|base| {
                        rows.get(&((*id).clone(), "folder".to_owned()))
                            .is_some_and(|row| row.local_path != *base)
                    })
            })
        });
        if let Some(ancestor) = waiting_above {
            return Ok(Relocation::Deferred(ancestor.clone()));
        }
        if let Some(holder) = rows.values().find(|row| {
            row.item_type == "folder" && row.server_id != folder.id && row.local_path == to
        }) {
            if folder_info.contains_key(&holder.server_id) {
                return Ok(Relocation::Deferred(holder.server_id.clone()));
            }
            // The server deleted the folder that holds this name. Its local
            // copy is cleared later in this run, so the move waits for the
            // next one, without blocking the path that has to be cleared.
            self.block_paths([from.as_str()]);
            Self::unresolve_subtree(&folder.id, folder_info, ancestors, bases);
            return Ok(Relocation::Settled);
        }
        let source = self.probe_dir(&from, report).await;
        let target = self.probe_dir(&to, report).await;
        if source == Presence::Unknown || target == Presence::Unknown {
            self.block_paths([from.as_str(), to.as_str()]);
            return Ok(Relocation::Settled);
        }

        // Under a strategy that never applies server changes locally, the
        // directory keeps its name. The folder stays linked by id: for this
        // run it, and everything resolved beneath it, lives where the
        // journal says.
        if !takes_server_changes(strategy) && source == Presence::Present {
            for other in folder_info.values_mut() {
                let Some(base) = other.base_path.as_deref() else {
                    continue;
                };
                if base == to {
                    other.base_path = Some(from.clone());
                } else if let Some(rel) = relative_under(base, &to) {
                    other.base_path = Some(self.join_rel(&from, &rel));
                }
            }
            return Ok(Relocation::Settled);
        }

        let move_key = format!("{MOVE_INTENT_PREFIX}{}", folder.id);
        let intent = self
            .journal
            .get_config(&move_key)
            .await?
            .and_then(|value| parse_move_intent(&value))
            .filter(|(old, new, ..)| *old == from && *new == to);
        let recovering = intent.is_some();
        let (copying, in_flight) = intent.map_or((false, None), |(_, _, copying, in_flight)| {
            (copying, in_flight)
        });
        let tracked: HashMap<String, Option<String>> = rows
            .values()
            .filter(|row| row.item_type == "file")
            .filter_map(|row| {
                Some((
                    relative_under(&row.local_path, &from)?,
                    row.checksum.clone(),
                ))
            })
            .collect();
        let record = |copying: bool| {
            let value = serde_json::to_string(&(&from, &to, copying)).unwrap();
            let move_key = &move_key;
            async move {
                self.journal
                    .set_config(move_key, &value)
                    .await
                    .map_err(|e| LocalFsError::other(e.to_string()))
            }
        };

        let mut copied: Vec<MovedFile> = Vec::new();
        let mut in_two_steps = false;
        let result: Result<(), LocalFsError> = async {
            match (source, target) {
                (Presence::Present, _) => {
                    // A case-only rename on a case-insensitive filesystem
                    // finds the folder itself at the destination.
                    let same_entry =
                        target == Presence::Present && self.fs.same_entry(&from, &to).await?;
                    if target == Presence::Present && !same_entry && !copying {
                        return Err(LocalFsError::other(format!(
                            "a different folder already exists at {}",
                            self.relative_display_path(&to)
                        )));
                    }
                    if !copying {
                        if !same_entry && !self.fs.confirm_absent(&to).await? {
                            return Err(LocalFsError::other(format!(
                                "something already exists at {}",
                                self.relative_display_path(&to)
                            )));
                        }
                        let parent = folder
                            .parent_id
                            .as_ref()
                            .and_then(|id| folder_info.get(id))
                            .and_then(|info| info.base_path.as_deref())
                            .or(self.root_local_path.as_deref())
                            .ok_or_else(|| {
                                LocalFsError::other("the new location has no local parent")
                            })?;
                        if self.path_blocked(parent) {
                            return Err(LocalFsError::other(
                                "the new parent directory is unreadable",
                            ));
                        }
                        self.fs.create_dir_all(parent).await?;
                        // The destination is known absent before the intent
                        // is recorded, so an unrelated directory that shows
                        // up there later is never taken for the result of an
                        // interrupted move.
                        record(false).await?;
                        let renamed = self.fs.rename(&from, &to).await;
                        // Copying a folder onto another spelling of its own
                        // name would have it overwrite itself, so a backend
                        // that cannot change only the case in one step gets
                        // a rename through a temporary name instead.
                        let case_only = same_entry || differs_only_in_case(&from, &to);
                        match renamed {
                            Ok(()) => return Ok(()),
                            Err(LocalFsError::MoveUnsupported(_)) if !case_only => {}
                            Err(e) => {
                                // A failed rename moved nothing.
                                self.journal
                                    .delete_config(&move_key)
                                    .await
                                    .map_err(|e| LocalFsError::other(e.to_string()))?;
                                if matches!(e, LocalFsError::MoveUnsupported(_)) {
                                    in_two_steps = true;
                                    return Ok(());
                                }
                                return Err(e);
                            }
                        }
                    }
                    let files = match self.copyable_files(&from, &tracked).await {
                        Ok(files) => files,
                        Err(e) => {
                            // Refused before anything was copied; a move
                            // already under way keeps its record.
                            if !copying {
                                self.journal
                                    .delete_config(&move_key)
                                    .await
                                    .map_err(|e| LocalFsError::other(e.to_string()))?;
                            }
                            return Err(e);
                        }
                    };
                    if !copying {
                        record(true).await?;
                    }
                    copied = self
                        .move_by_copy(&from, &to, files, &move_key, &tracked, in_flight)
                        .await?;
                    Ok(())
                }
                (_, Presence::Present) => {
                    // The folder is gone from its old path and a directory
                    // sits at the new one: an interrupted move of ours, the
                    // same rename already made by hand, or a stranger.
                    if copying {
                        // The filesystem move may have completed before its
                        // journal transaction. Rebuild content evidence rather
                        // than completing it with stale pre-copy mtimes.
                        let mut arrived = self.fs.walk(&to).await?;
                        for file in &mut arrived {
                            file.rel_path = file.rel_path.replace('\\', "/");
                        }
                        // The file an interrupted run was writing comes
                        // first: if it cannot be vouched for, nothing else
                        // needs to be read.
                        arrived.sort_by_key(|file| in_flight.as_deref() != Some(&file.rel_path));
                        let mtimes = |rel: &str| {
                            let at_dest = arrived.iter().find(|file| file.rel_path == rel);
                            (None, at_dest.map(|file| file.mtime))
                        };
                        if let Some(reason) = self.stuck_reason(&move_key, mtimes) {
                            return Err(LocalFsError::other(reason));
                        }
                        for file in &arrived {
                            let rel = &file.rel_path;
                            let interrupted = in_flight.as_deref() == Some(rel.as_str());
                            let synced = match tracked.get(rel) {
                                Some(Some(synced)) => synced,
                                // Without a synced checksum the content says
                                // nothing; the mtime comparison decides, as
                                // it does for any other file.
                                Some(None) if !interrupted => continue,
                                Some(None) => "",
                                None => continue,
                            };
                            let dest = self.join_rel(&to, rel);
                            let found = self.fs.sha256(&dest).await?;
                            if interrupted && found != synced {
                                return Err(self.stuck_on(
                                    &move_key,
                                    rel,
                                    mtimes(rel),
                                    format!(
                                        "interrupted copy at {} needs manual resolution: its \
                                         original is gone, so it cannot be told whether the \
                                         copy is complete. Remove it to have the file \
                                         downloaded again, after saving any changes it holds \
                                         somewhere else",
                                        self.relative_display_path(&dest)
                                    ),
                                ));
                            }
                            copied.push(MovedFile {
                                rel: rel.clone(),
                                mtime: file.mtime,
                                content: if found == synced {
                                    MovedContent::Unchanged
                                } else {
                                    MovedContent::Edited
                                },
                            });
                        }
                        Ok(())
                    } else if recovering || self.is_same_folder(&from, &to, rows, report).await {
                        Ok(())
                    } else {
                        Err(LocalFsError::other(format!(
                            "a different folder already exists at {}",
                            self.relative_display_path(&to)
                        )))
                    }
                }
                _ => {
                    if self.fs.confirm_absent(&from).await? {
                        Ok(())
                    } else {
                        Err(LocalFsError::other(format!(
                            "{} is not a directory",
                            self.relative_display_path(&from)
                        )))
                    }
                }
            }
        }
        .await;
        // A copy that was interrupted may never have carried some files
        // across, and the user may since have cleared the old directory as
        // a leftover. Their rows must not follow the folder: they would read
        // as local deletions and take the server copies with them. Forgotten
        // instead, the items are downloaded again like any first sight.
        let mut never_arrived: Vec<(String, String)> = Vec::new();
        let result = match result {
            Ok(()) if copying => self
                .rows_missing_at(&from, &to, rows)
                .await
                .map(|keys| never_arrived = keys),
            other => other,
        };
        if let Err(e) = result {
            self.report_skipped(&from, failed(e.to_string()), report)
                .await;
            self.block_paths([to.as_str()]);
            return Ok(Relocation::Settled);
        }
        if in_two_steps {
            return Ok(Relocation::Deferred(folder.id.clone()));
        }
        for key in never_arrived {
            self.journal.delete(&key.0, &key.1).await?;
            rows.remove(&key);
        }

        // Also completes a move interrupted between the filesystem and the
        // journal: the old directory is gone and the new one exists.
        self.rebase_rows(&folder.id, &from, &to, rows, &copied)
            .await?;
        self.stuck_moves.lock().unwrap().remove(&move_key);
        Ok(Relocation::Progress)
    }

    /// Per-folder override bases that are walked as sync roots of their
    /// own, as `(base_path, folder_id, strategy)`. Bases under the global
    /// root are covered by the root walk; a base without an explicit
    /// override was derived from root + names and is covered the same way.
    async fn override_walk_roots(
        &self,
        folder_info: &HashMap<String, ResolvedFolder>,
    ) -> Vec<(String, String, SyncStrategy)> {
        let mut roots = Vec::new();
        for (id, info) in folder_info {
            let Some(base) = info.base_path.as_ref() else {
                continue;
            };
            if let Some(root) = self.root_local_path.as_ref() {
                if base.starts_with(root.as_str()) {
                    continue;
                }
            }
            let has_override = self
                .journal
                .get_folder_sync_config(id)
                .await
                .ok()
                .flatten()
                .map(|(_, p)| p.is_some())
                .unwrap_or(false);
            if !has_override && self.root_local_path.is_some() {
                continue;
            }
            roots.push((base.clone(), id.clone(), info.strategy));
        }
        roots
    }

    /// Local directories that have no server folder yet and sit directly
    /// inside a known folder or a sync root: the places a renamed or moved
    /// folder can have landed. Directories nested inside another local-only
    /// directory are left to the ordinary create-and-upload path.
    async fn local_only_top_dirs(
        &self,
        folder_info: &HashMap<String, ResolvedFolder>,
    ) -> Vec<LocalOnlyDir> {
        let known: HashMap<&str, (&str, SyncStrategy)> = folder_info
            .iter()
            .filter_map(|(id, info)| {
                info.base_path
                    .as_deref()
                    .map(|base| (base, (id.as_str(), info.strategy)))
            })
            .collect();
        let mut walk_roots: Vec<(String, Option<(String, SyncStrategy)>)> = Vec::new();
        if let Some(root) = self.root_local_path.clone() {
            walk_roots.push((root, None));
        }
        for (base, folder_id, strategy) in self.override_walk_roots(folder_info).await {
            walk_roots.push((base, Some((folder_id, strategy))));
        }

        let mut out = Vec::new();
        for (walk_root, owner) in walk_roots {
            let rel_dirs = match self.fs.walk_dirs(&walk_root).await {
                Ok(dirs) => dirs,
                Err(e) => {
                    warn!("walk_dirs({walk_root}) failed: {e}");
                    continue;
                }
            };
            for rel in rel_dirs {
                let path = self.fs.join(&walk_root, &rel);
                if self.subtree_blocked(&path) || self.path_shadowed(&path) {
                    continue;
                }
                if known.contains_key(path.as_str()) {
                    continue;
                }
                let (parent_path, name) = match rel.rfind(['/', '\\']) {
                    Some(idx) => (Some(self.fs.join(&walk_root, &rel[..idx])), &rel[idx + 1..]),
                    None => (None, rel.as_str()),
                };
                let (parent_id, parent_strategy) = match &parent_path {
                    Some(parent) => match known.get(parent.as_str()) {
                        Some((id, strategy)) => (Some((*id).to_owned()), *strategy),
                        None => continue,
                    },
                    None => match &owner {
                        Some((id, strategy)) => (Some(id.clone()), *strategy),
                        None => (None, SyncStrategy::TwoWay),
                    },
                };
                let accepts_uploads = matches!(
                    parent_strategy,
                    SyncStrategy::TwoWay | SyncStrategy::ClientToServer | SyncStrategy::UploadOnly
                );
                if name.is_empty() || !accepts_uploads {
                    continue;
                }
                out.push(LocalOnlyDir {
                    path,
                    name: name.to_owned(),
                    parent_id,
                });
            }
        }
        out
    }

    /// Pair each vanished folder with the local-only directory it was
    /// renamed or moved to. A directory matches when it holds every file
    /// the journal tracked in the folder, at the same relative path and
    /// with the same mtime and the same content hash; extra files upload as
    /// new. Only unambiguous pairs are returned, so two identical copies or two
    /// candidate targets fall back to delete-and-create.
    async fn match_folder_renames<'a>(
        &self,
        vanished: &'a [VanishedFolder],
        landing: &'a [LocalOnlyDir],
        report: &mut SyncReport,
    ) -> Vec<(&'a VanishedFolder, &'a LocalOnlyDir)> {
        let mut listings: Vec<HashMap<String, i64>> = Vec::with_capacity(landing.len());
        for dir in landing {
            let listing = match self.fs.walk(&dir.path).await {
                Ok(entries) => entries
                    .into_iter()
                    .map(|entry| (entry.rel_path.replace('\\', "/"), entry.mtime))
                    .collect(),
                Err(e) => {
                    warn!("walk({}) failed: {e}", dir.path);
                    HashMap::new()
                }
            };
            listings.push(listing);
        }
        let mut targets_of = Vec::new();
        for folder in vanished {
            let mut targets = Vec::new();
            for (idx, listing) in listings.iter().enumerate() {
                if self
                    .holds_tracked_files(&landing[idx].path, listing, &folder.files, report)
                    .await
                {
                    targets.push(idx);
                }
            }
            targets_of.push(targets);
        }
        let mut claims: HashMap<usize, usize> = HashMap::new();
        for targets in &targets_of {
            if let [only] = targets.as_slice() {
                *claims.entry(*only).or_default() += 1;
            }
        }
        vanished
            .iter()
            .zip(&targets_of)
            .filter_map(|(folder, targets)| match targets.as_slice() {
                [only] if claims.get(only) == Some(&1) => Some((folder, &landing[*only])),
                _ => None,
            })
            .collect()
    }

    /// Record a run-level error in the local audit log so the desktop
    /// activity view shows *why* the run failed. The message lands in
    /// `note` because that's the field the renderer reads as the row's
    /// details; `reason` is reserved for the start/end-marker run tag
    /// ("Sync" / "ManualSyncStart").
    async fn log_sync_error_row(&self, path: &str, message: &str) {
        self.ensure_start_emitted().await;
        self.log_row(SyncLogRow {
            id: 0,
            timestamp: Utc::now().to_rfc3339(),
            operation: "Error".to_owned(),
            direction: None,
            resource_type: None,
            path: path.to_owned(),
            new_path: None,
            reason: "Sync".to_owned(),
            note: Some(message.to_owned()),
        })
        .await;
    }

    async fn log_sync_marker(&self, operation: &str, reason: &str, note: Option<String>) {
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
            blocked_paths: HashSet::new(),
            shadowed_paths: HashSet::new(),
        });
        self.fs.begin_run();
        {
            let mut cache = self.hash_cache.lock().unwrap();
            let HashCache { entries, used } = &mut *cache;
            entries.retain(|path, _| used.contains(path));
            used.clear();
        }
        // No state transition here — the previous outcome (`Connected` /
        // `NotConnected` / `Error`) holds while we check for updates. The
        // first transfer (if any) sticky-flips us to `Transferring`; the
        // post-run classifier below publishes the final state.
        let started = std::time::Instant::now();

        // Body runs inside an async block so a `?`-driven early return
        // from any phase still reaches the post-run state classifier
        // below. The block keeps the existing indentation — Rust doesn't
        // care, and reindenting hundreds of lines would obscure the diff.
        //
        // The error is `Send + Sync` so it can be held across the logging
        // awaits below without making this future `!Send`.
        let result: Result<SyncReport, Box<dyn std::error::Error + Send + Sync>> = async {
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
                    Err(
                        e @ (SentinelError::Missing { .. }
                        | SentinelError::Mismatch { .. }
                        | SentinelError::Corrupt { .. }
                        | SentinelError::Fs(_)
                        | SentinelError::Journal(_)),
                    ) => {
                        let reason = e.to_string();
                        report.errors.push(SyncError {
                            path: base.clone(),
                            reason: reason.clone(),
                        });
                        warn!("Aborting sync: {e}");
                        self.log_sync_error_row(base, &reason).await;
                        let elapsed = started.elapsed();
                        let note = format!(
                            "0 up, 0 down, 0 deleted, 0 folders created, 0 conflicts, \
                         {} errors, {:.1}s",
                            report.errors.len(),
                            elapsed.as_secs_f32(),
                        );
                        self.log_sync_marker("SyncEnd", end_reason, Some(note))
                            .await;
                        return Ok(report);
                    }
                }
            }

            // What an interrupted download left behind is invisible to the
            // walkers, so it would keep a folder deleted on the server from
            // being removed here and have it created there again.
            if self.staging_dirty.swap(false, Ordering::SeqCst) {
                for base in &bases_to_verify {
                    match self.fs.sweep_staging(base).await {
                        Ok(0) => {}
                        Ok(removed) => {
                            info!("Removed {removed} leftover download file(s) in {base}")
                        }
                        Err(e) => {
                            warn!("Could not clear leftover download files in {base}: {e}");
                            self.staging_dirty.store(true, Ordering::SeqCst);
                        }
                    }
                }
            }

            // Discover unreadable directories before any descendant can be
            // changed, regardless of the order returned by the server.
            let mut local_dirs: Vec<&str> = folder_info
                .values()
                .filter(|info| info.strategy != SyncStrategy::DoNotSync)
                .filter_map(|info| info.base_path.as_deref())
                .collect();
            local_dirs.sort_by_key(|path| path.len());
            for path in local_dirs {
                self.probe_dir(path, &mut report).await;
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
                    let presence = if row.item_type == "file" {
                        self.probe_file(&row.local_path, &mut report).await
                    } else {
                        self.probe_dir(&row.local_path, &mut report).await
                    };
                    if presence == Presence::Absent {
                        let _ = self.journal.delete(&row.server_id, &row.item_type).await;
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
            let mut journal_map: HashMap<(String, String), SyncStateRow> = journal_rows
                .into_iter()
                .map(|r| ((r.server_id.clone(), r.item_type.clone()), r))
                .collect();

            let server_files_by_id: HashMap<&str, &uncloud_common::FileResponse> =
                tree.files.iter().map(|f| (f.id.as_str(), f)).collect();
            let server_folders_by_id: HashMap<&str, &uncloud_common::FolderResponse> =
                tree.folders.iter().map(|f| (f.id.as_str(), f)).collect();

            // Cache every folder's ancestors. Folder deletion and remote-change
            // checks need subtree membership, and repeatedly walking parent
            // pointers made those checks both noisy and quadratic in depth.
            let mut folder_ancestors: HashMap<String, HashSet<String>> = HashMap::new();
            for folder in &tree.folders {
                let mut ancestors = HashSet::new();
                let mut current = folder.parent_id.as_deref();
                while let Some(parent_id) = current {
                    if !ancestors.insert(parent_id.to_owned()) {
                        break;
                    }
                    current = server_folders_by_id
                        .get(parent_id)
                        .and_then(|parent| parent.parent_id.as_deref());
                }
                folder_ancestors.insert(folder.id.clone(), ancestors);
            }

            let folder_in_subtree = |folder_id: &str, root_id: &str| {
                folder_id == root_id
                    || folder_ancestors
                        .get(folder_id)
                        .is_some_and(|ancestors| ancestors.contains(root_id))
            };
            let file_in_subtree = |file: &uncloud_common::FileResponse, root_id: &str| {
                file.parent_id
                    .as_deref()
                    .is_some_and(|parent_id| folder_in_subtree(parent_id, root_id))
            };
            // Rows written before checksums were recorded get theirs from the
            // server: while the row still names the server's current version,
            // the bytes last transferred are the bytes the server holds.
            let backfill: Vec<(String, String)> = tree
                .files
                .iter()
                .filter_map(|file| {
                    let checksum = file.checksum_sha256.as_ref()?;
                    let row = journal_map.get(&(file.id.clone(), "file".to_owned()))?;
                    (row.checksum.is_none()
                        && row.server_updated_at == file.updated_at
                        && row.sync_status == SyncStatus::Synced.as_str())
                    .then(|| (file.id.clone(), checksum.clone()))
                })
                .collect();
            if !backfill.is_empty() {
                self.journal.fill_missing_checksums(&backfill).await?;
                for (id, checksum) in backfill {
                    if let Some(row) = journal_map.get_mut(&(id, "file".to_owned())) {
                        row.checksum = Some(checksum);
                    }
                }
            }

            self.reconcile_remote_folder_paths(
                &tree,
                &mut folder_info,
                &folder_ancestors,
                &mut journal_map,
                &bases_to_verify,
                &mut report,
            )
            .await?;

            // A folder mapped to its own local path leaves its default
            // location under the root behind. That directory is the old copy,
            // not a new folder to create on the server.
            let actual: HashSet<&str> = folder_info
                .values()
                .filter_map(|info| info.base_path.as_deref())
                .collect();
            // A retired path stops being one when a folder syncs there
            // again or the old copy is gone; a directory created at it
            // later is new and syncs like any other.
            let mut shadowed: HashSet<String> = HashSet::new();
            for path in self.journal.retired_local_paths().await? {
                if actual.contains(path.as_str())
                    || matches!(self.fs.confirm_absent(&path).await, Ok(true))
                {
                    self.journal.forget_retired_path(&path).await?;
                } else {
                    shadowed.insert(path);
                }
            }
            if let Some(root) = self.root_local_path.as_deref() {
                for folder in &tree.folders {
                    let mut names = vec![folder.name.as_str()];
                    let mut current = folder.parent_id.as_deref();
                    while let Some(parent) = current.and_then(|id| server_folders_by_id.get(id)) {
                        if names.len() > tree.folders.len() {
                            break;
                        }
                        names.push(parent.name.as_str());
                        current = parent.parent_id.as_deref();
                    }
                    let default = names
                        .iter()
                        .rev()
                        .fold(root.to_owned(), |path, name| self.fs.join(&path, name));
                    if !actual.contains(default.as_str()) {
                        shadowed.insert(default);
                    }
                }
            }
            if let Some(state) = self.run_state.write().unwrap().as_mut() {
                state.shadowed_paths = shadowed;
            }

            let allows_delete_push =
                |s: SyncStrategy| matches!(s, SyncStrategy::TwoWay | SyncStrategy::ClientToServer);
            let restore_in_progress =
                |row: &SyncStateRow| row.sync_status == SyncStatus::PendingDownload.as_str();
            let downloads = |s: SyncStrategy| {
                !matches!(
                    s,
                    SyncStrategy::DoNotSync
                        | SyncStrategy::ClientToServer
                        | SyncStrategy::UploadOnly
                )
            };

            // 3.1. Strategy transitions. While a folder's strategy never
            //      pushes deletes, removing its local copy is routine (freeing
            //      space under `DoNotSync` / `UploadOnly`) and the journal
            //      rows stay behind. Once the strategy starts pushing deletes
            //      again those rows would read as "tracked, now missing" and
            //      trash the server copy. Forget them instead, so the items
            //      are treated as first-sight, exactly like a fresh install.
            let strategies_seen = self.journal.effective_strategies_seen().await?;
            let mut strategies_now: HashMap<String, String> = HashMap::new();
            for folder in &tree.folders {
                let Some(info) = folder_info.get(&folder.id) else {
                    continue;
                };
                let previous = strategies_seen.get(&folder.id);
                let regained_delete_push = allows_delete_push(info.strategy)
                    && previous
                        .and_then(|name| strategy_from_name(name))
                        .is_some_and(|prev| !allows_delete_push(prev));
                let mut settled = true;
                if regained_delete_push {
                    let mut forget: Vec<(String, String)> = Vec::new();
                    let folder_key = (folder.id.clone(), "folder".to_owned());
                    if let Some(row) = journal_map.get(&folder_key) {
                        match self.probe_dir(&row.local_path, &mut report).await {
                            Presence::Absent => forget.push(folder_key),
                            Presence::Present => {}
                            Presence::Unknown => settled = false,
                        }
                    }
                    for file in tree
                        .files
                        .iter()
                        .filter(|file| file.parent_id.as_deref() == Some(folder.id.as_str()))
                    {
                        let key = (file.id.clone(), "file".to_owned());
                        if let Some(row) = journal_map.get(&key) {
                            match self.probe_file(&row.local_path, &mut report).await {
                                Presence::Absent => forget.push(key),
                                Presence::Present => {}
                                Presence::Unknown => settled = false,
                            }
                        }
                    }
                    for key in forget {
                        self.journal.delete(&key.0, &key.1).await?;
                        journal_map.remove(&key);
                    }
                }
                // An item that could not be probed keeps the old strategy on
                // record, so the transition is replayed once it is readable.
                let recorded = match previous {
                    Some(name) if !settled => name.clone(),
                    _ => strategy_name(info.strategy),
                };
                strategies_now.insert(folder.id.clone(), recorded);
            }
            if strategies_now != strategies_seen {
                self.journal
                    .replace_effective_strategies_seen(&strategies_now)
                    .await?;
            }

            // 3.2. What the server holds that the journal has not caught up
            //      with, resolved per item. A file or folder is "changed"
            //      when it has no journal row, its server timestamp moved
            //      past the row, or an earlier restore of it has not finished.
            //      Two consequences, tracked separately:
            //
            //       - `needs_local_dir`: a change this client will download
            //         needs every folder on the way to it to exist locally,
            //         even one the user just deleted. Only those folders come
            //         back; their unchanged contents are still local
            //         deletions.
            //       - `collapse_blocked`: a folder holding anything this
            //         client has not seen in its current form, or anything
            //         whose strategy forbids pushing deletes, is never
            //         removed with a single cascading delete.
            let mut needs_local_dir: HashSet<String> = HashSet::new();
            let mut collapse_blocked: HashSet<String> = HashSet::new();
            let with_ancestors = |set: &mut HashSet<String>, folder_id: &str| {
                set.insert(folder_id.to_owned());
                if let Some(ancestors) = folder_ancestors.get(folder_id) {
                    set.extend(ancestors.iter().cloned());
                }
            };
            for folder in &tree.folders {
                let Some(info) = folder_info.get(&folder.id) else {
                    continue;
                };
                let changed = journal_map
                    .get(&(folder.id.clone(), "folder".to_owned()))
                    .is_none_or(|row| folder.updated_at > row.server_updated_at);
                if changed || !allows_delete_push(info.strategy) {
                    with_ancestors(&mut collapse_blocked, &folder.id);
                }
                if changed && info.strategy != SyncStrategy::DoNotSync && info.base_path.is_some() {
                    with_ancestors(&mut needs_local_dir, &folder.id);
                }
            }
            for file in &tree.files {
                let Some(parent_id) = file.parent_id.as_deref() else {
                    continue;
                };
                let changed = journal_map
                    .get(&(file.id.clone(), "file".to_owned()))
                    .is_none_or(|row| {
                        file.updated_at > row.server_updated_at || restore_in_progress(row)
                    });
                if !changed {
                    continue;
                }
                with_ancestors(&mut collapse_blocked, parent_id);
                if folder_info
                    .get(parent_id)
                    .is_some_and(|info| downloads(info.strategy) && info.base_path.is_some())
                {
                    with_ancestors(&mut needs_local_dir, parent_id);
                }
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

                let key = (folder.id.clone(), "folder".to_owned());
                let existing = journal_map.get(&key);
                let local_exists = match self.probe_dir(base, &mut report).await {
                    Presence::Present => true,
                    Presence::Absent => false,
                    Presence::Unknown => continue,
                };

                // A journaled folder that disappeared locally is a possible
                // delete or rename. Do not recreate it before Phase 6a gets a
                // chance to confirm that absence, unless a server change has
                // to land inside it.
                let locally_deleted = !local_exists
                    && allows_delete_push(info.strategy)
                    && existing.is_some_and(|row| row.local_path == *base);
                if locally_deleted && !needs_local_dir.contains(&folder.id) {
                    continue;
                }

                if existing.is_some_and(|row| row.delete_pending_since.is_some()) {
                    self.journal
                        .clear_delete_pending(&folder.id, "folder")
                        .await?;
                }

                if let Err(e) = self.fs.create_dir_all(base).await {
                    report.errors.push(SyncError {
                        path: folder.name.clone(),
                        reason: e.to_string(),
                    });
                    continue;
                }

                self.journal
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
                    .await?;
                if locally_deleted {
                    self.log_restore(base, "Folder").await;
                }
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

                let local_exists = match self.probe_file(local_path, &mut report).await {
                    Presence::Present => true,
                    Presence::Absent => false,
                    Presence::Unknown => continue,
                };

                // A tracked file that is gone locally belongs to Phase 6a's
                // two-phase deletion; re-downloading it here would silently
                // undo the user's `rm`. Two exceptions bring it back instead:
                // the server holds a version this client never had (the
                // change wins over the delete), or the strategy never pushes
                // deletes, in which case the server copy is the truth.
                //
                // A restore is recorded before it starts and stays recorded
                // until it succeeds, so whatever a failed attempt left on
                // disk is overwritten by the retry rather than mistaken for
                // a local edit. Under a strategy that cannot download, an
                // unfinished restore is left untouched in both directions.
                //
                // If the journal has no row at all (genuine first-sight) we
                // still fall through to the `None` arm below and download.
                let tracked = journal_map.get(&key);
                let resuming = tracked.is_some_and(restore_in_progress);
                let restoring = tracked.is_some() && (resuming || !local_exists);
                if restoring {
                    let server_wins = downloads(strategy)
                        && tracked.is_some_and(|row| {
                            resuming
                                || file.updated_at > row.server_updated_at
                                || !allows_delete_push(strategy)
                        });
                    if !server_wins {
                        continue;
                    }
                    if !resuming {
                        self.journal.set_pending_download(&file.id, "file").await?;
                    }
                }
                let journal_entry = tracked.filter(|_| !restoring);

                match journal_entry {
                    None => {
                        // New on server → download if strategy allows
                        let can_download = !matches!(
                            strategy,
                            SyncStrategy::ClientToServer | SyncStrategy::UploadOnly
                        );
                        if can_download {
                            match self.download_to(&file.id, local_path).await {
                                Ok(checksum) => {
                                    let mtime = self.fs.mtime(local_path).await.ok().flatten();
                                    self.journal
                                        .upsert(
                                            &file.id,
                                            "file",
                                            server_rel_path,
                                            &local_path_str,
                                            Some(file.size_bytes),
                                            Some(&checksum),
                                            &file.updated_at,
                                            mtime,
                                            "synced",
                                        )
                                        .await?;
                                    touched_paths.insert(local_path_str.clone());
                                    report.downloaded.push(server_rel_path.clone());
                                    if restoring {
                                        self.journal.clear_delete_pending(&file.id, "file").await?;
                                    }
                                    if restoring && allows_delete_push(strategy) {
                                        self.log_restore(local_path, "File").await;
                                    } else {
                                        self.log_download(local_path, false).await;
                                    }
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
                                match self
                                    .upload_update(&file.id, server_rel_path, local_path)
                                    .await
                                {
                                    Ok((updated, checksum, new_mtime)) => {
                                        let _ = self
                                            .journal
                                            .upsert(
                                                &updated.id,
                                                "file",
                                                server_rel_path,
                                                &local_path_str,
                                                Some(updated.size_bytes),
                                                Some(&checksum),
                                                &updated.updated_at,
                                                new_mtime,
                                                "synced",
                                            )
                                            .await;
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
                                // Don't let Phase 7 push the conflict copy
                                // back up in this run, complete or not.
                                touched_paths.insert(conflict_path.clone());
                                // The copy is the only place the local
                                // version survives the download, so without
                                // it the local file is left as it is.
                                self.staging_dirty.store(true, Ordering::SeqCst);
                                let downloaded =
                                    match self.fs.copy_file(local_path, &conflict_path).await {
                                        Ok(()) => self.download_to(&file.id, local_path).await,
                                        Err(e) => Err(format!(
                                            "kept the local version: its conflict copy could \
                                             not be written: {e}"
                                        )),
                                    };
                                match downloaded {
                                    Ok(checksum) => {
                                        let new_mtime =
                                            self.fs.mtime(local_path).await.ok().flatten();
                                        let _ = self
                                            .journal
                                            .upsert(
                                                &file.id,
                                                "file",
                                                server_rel_path,
                                                &local_path_str,
                                                Some(file.size_bytes),
                                                Some(&checksum),
                                                &file.updated_at,
                                                new_mtime,
                                                "synced",
                                            )
                                            .await;
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
                                    Ok(checksum) => {
                                        let new_mtime =
                                            self.fs.mtime(local_path).await.ok().flatten();
                                        let _ = self
                                            .journal
                                            .upsert(
                                                &file.id,
                                                "file",
                                                server_rel_path,
                                                &local_path_str,
                                                Some(file.size_bytes),
                                                Some(&checksum),
                                                &file.updated_at,
                                                new_mtime,
                                                "synced",
                                            )
                                            .await;
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
                                match self
                                    .upload_update(&file.id, server_rel_path, local_path)
                                    .await
                                {
                                    Ok((updated, checksum, new_mtime)) => {
                                        let _ = self
                                            .journal
                                            .upsert(
                                                &updated.id,
                                                "file",
                                                server_rel_path,
                                                &local_path_str,
                                                Some(updated.size_bytes),
                                                Some(&checksum),
                                                &updated.updated_at,
                                                new_mtime,
                                                "synced",
                                            )
                                            .await;
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
                    if self.path_blocked(&j.local_path) {
                        continue;
                    }
                    // Server deleted this file
                    let strategy = SyncStrategy::TwoWay; // default; ideally look up parent folder
                    if matches!(
                        strategy,
                        SyncStrategy::TwoWay | SyncStrategy::ServerToClient
                    ) {
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

            // 6.1. Server folder deletions: folder rows in journal whose
            //      server_id is no longer in `tree.folders`. Symmetric with
            //      Phase 6 above for files. Without this phase, Phase 6.5
            //      walks the still-present local directory, fails to find a
            //      matching server folder in `bases`, and re-creates the
            //      folder on the server — an infinite zombie loop after the
            //      user deletes a folder via the web UI.
            //
            //      Two behaviours, both preserve data:
            //       - Local dir empty (or already gone): `remove_dir`
            //         succeeds, we mark the path as touched so Phase 6.5
            //         skips it, and we log a local delete. This mirrors the
            //         file case.
            //       - Local dir non-empty (user has unsynced files inside):
            //         `remove_dir` fails with NotEmpty. We still drop the
            //         journal row to break the loop, but DON'T mark touched
            //         — Phase 6.5 will re-create the folder so those
            //         unsynced files have somewhere to upload to. The user
            //         can re-delete on the server once the inner files are
            //         dealt with.
            //
            //      Process deepest paths first so nested empty-dir chains
            //      collapse: removing `Test/Sub` first leaves `Test` empty,
            //      then `Test` can also `remove_dir` cleanly.
            let server_folder_ids: std::collections::HashSet<&str> =
                tree.folders.iter().map(|f| f.id.as_str()).collect();
            let mut folder_deletion_echoes: Vec<&crate::journal::SyncStateRow> = journal_map
                .iter()
                .filter(|((_, item_type), _)| item_type == "folder")
                .filter(|((server_id, _), _)| !server_folder_ids.contains(server_id.as_str()))
                .map(|(_, row)| row)
                .collect();
            folder_deletion_echoes.sort_by_key(|r| std::cmp::Reverse(r.local_path.len()));

            for j in folder_deletion_echoes {
                let dir_exists = match self.probe_dir(&j.local_path, &mut report).await {
                    Presence::Present => true,
                    Presence::Absent => false,
                    Presence::Unknown => continue,
                };
                let path_clear = if dir_exists {
                    match self.fs.remove_dir(&j.local_path).await {
                        Ok(()) => {
                            report.deleted_local.push(j.server_path.clone());
                            self.log_delete_local(&j.local_path).await;
                            true
                        }
                        // Non-empty (or other error): leave the dir for Phase
                        // 6.5 to re-attach to the server tree.
                        Err(_) => false,
                    }
                } else {
                    true
                };
                if path_clear {
                    touched_paths.insert(j.local_path.clone());
                }
                let _ = self.journal.delete(&j.server_id, "folder").await;
            }

            // 6a. Handle local deletions: rows in journal whose local file or
            //     folder is gone but the server still has them, unchanged.
            //     Two-phase with a minimum dwell: the first scan that sees
            //     the absence sets `delete_pending_since`, and the delete is
            //     only pushed by a later scan that still finds the item
            //     missing once the confirmation delay has passed.
            //
            //     A folder whose whole subtree is gone is handled as one
            //     item: either it was renamed or moved locally, in which case
            //     the server folder follows it, or it is deleted with a
            //     single `delete_folder` that the server cascades.
            let now_dt = Utc::now();
            let now = now_dt.to_rfc3339();
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
            let journal_folder =
                |folder_id: &str| journal_map.get(&(folder_id.to_owned(), "folder".to_owned()));
            let journal_file =
                |file_id: &str| journal_map.get(&(file_id.to_owned(), "file".to_owned()));

            // Tracked files that are gone locally and that this client is
            // allowed to delete on the server. A file the server changed or
            // whose restore is unfinished is excluded (Phase 5 brings it
            // back), as is anything whose strategy does not push deletes.
            let mut missing_files: Vec<&SyncStateRow> = Vec::new();
            let mut missing_file_ids: HashSet<&str> = HashSet::new();
            for ((server_id, item_type), j) in &journal_map {
                if item_type != "file" {
                    continue;
                }
                let Some(server_file) = server_files_by_id.get(server_id.as_str()) else {
                    continue; // Phase 6 covers server-side delete echoes.
                };
                let strategy = strategy_for_file(server_id);
                let server_wins =
                    downloads(strategy) && server_file.updated_at > j.server_updated_at;
                let absent = allows_delete_push(strategy)
                    && !server_wins
                    && !restore_in_progress(j)
                    && self.probe_file(&j.local_path, &mut report).await == Presence::Absent;
                if !absent {
                    if j.delete_pending_since.is_some() {
                        self.journal.clear_delete_pending(server_id, "file").await?;
                    }
                    continue;
                }
                missing_file_ids.insert(server_id.as_str());
                missing_files.push(j);
            }

            // A folder's subtree is gone when its own directory is absent and
            // so is everything tracked beneath it. Resolved deepest-first so
            // every item is probed at most once; `collapse_blocked` already
            // rules out subtrees holding anything untracked, changed on the
            // server, or protected by strategy.
            let mut child_folders: HashMap<&str, Vec<&str>> = HashMap::new();
            for folder in &tree.folders {
                if let Some(parent_id) = folder.parent_id.as_deref() {
                    child_folders
                        .entry(parent_id)
                        .or_default()
                        .push(folder.id.as_str());
                }
            }
            let mut files_by_parent: HashMap<&str, Vec<&str>> = HashMap::new();
            for file in &tree.files {
                if let Some(parent_id) = file.parent_id.as_deref() {
                    files_by_parent
                        .entry(parent_id)
                        .or_default()
                        .push(file.id.as_str());
                }
            }
            let depth_of =
                |folder_id: &str| folder_ancestors.get(folder_id).map_or(0, HashSet::len);
            let mut deepest_first: Vec<&uncloud_common::FolderResponse> =
                tree.folders.iter().collect();
            deepest_first.sort_by_key(|folder| std::cmp::Reverse(depth_of(&folder.id)));
            let mut subtree_gone: HashSet<&str> = HashSet::new();
            for folder in deepest_first {
                let Some(row) = journal_folder(&folder.id) else {
                    continue;
                };
                if collapse_blocked.contains(&folder.id) {
                    continue;
                }
                let contents_gone = child_folders
                    .get(folder.id.as_str())
                    .is_none_or(|children| children.iter().all(|id| subtree_gone.contains(id)))
                    && files_by_parent
                        .get(folder.id.as_str())
                        .is_none_or(|files| files.iter().all(|id| missing_file_ids.contains(id)));
                if contents_gone
                    && self.probe_dir(&row.local_path, &mut report).await == Presence::Absent
                {
                    subtree_gone.insert(folder.id.as_str());
                }
            }

            // Only the topmost vanished folder of each subtree is acted on;
            // whatever happens to it carries its descendants along. Server
            // ancestry decides "topmost", not local path length: a per-folder
            // override can place a child at a shorter path.
            let mut top_vanished: Vec<&str> = Vec::new();
            for folder in &tree.folders {
                let Some(row) = journal_folder(&folder.id) else {
                    continue;
                };
                let tracked_here = folder_info
                    .get(&folder.id)
                    .and_then(|info| info.base_path.as_deref())
                    == Some(row.local_path.as_str());
                let vanished = tracked_here && subtree_gone.contains(folder.id.as_str());
                if !vanished {
                    if row.delete_pending_since.is_some() {
                        self.journal
                            .clear_delete_pending(&folder.id, "folder")
                            .await?;
                    }
                    continue;
                }
                top_vanished.push(folder.id.as_str());
            }
            let vanished_ids: HashSet<&str> = top_vanished.iter().copied().collect();
            top_vanished.retain(|folder_id| {
                !folder_ancestors.get(*folder_id).is_some_and(|ancestors| {
                    ancestors
                        .iter()
                        .any(|ancestor| vanished_ids.contains(ancestor.as_str()))
                })
            });

            // Files under a vanished folder are settled together with it.
            let mut settled_file_ids: HashSet<&str> = HashSet::new();
            for folder_id in &top_vanished {
                for file in tree
                    .files
                    .iter()
                    .filter(|file| file_in_subtree(file, folder_id))
                {
                    settled_file_ids.insert(file.id.as_str());
                }
            }

            // Local rename or move: a vanished folder whose tracked files all
            // sit, unmodified, inside one new local directory was not deleted.
            // Mirror the move on the server so ids, shares and version
            // history survive, instead of deleting and re-uploading it.
            let mut renamed_folder_ids: HashSet<&str> = HashSet::new();
            let mut folder_base_updates: Vec<(String, String)> = Vec::new();
            let mut vanished_folders: Vec<VanishedFolder> = Vec::new();
            for folder_id in &top_vanished {
                let Some(row) = journal_folder(folder_id) else {
                    continue;
                };
                let mut files = BTreeMap::new();
                let mut relocatable = true;
                for file in tree
                    .files
                    .iter()
                    .filter(|file| file_in_subtree(file, folder_id))
                {
                    let located = journal_file(&file.id).and_then(|file_row| {
                        Some((
                            relative_under(&file_row.local_path, &row.local_path)?,
                            file_row.local_mtime?,
                            file_row.checksum.clone()?,
                        ))
                    });
                    match located {
                        Some((rel, mtime, checksum)) => {
                            files.insert(rel, (mtime, checksum));
                        }
                        None => relocatable = false,
                    }
                }
                for descendant in tree
                    .folders
                    .iter()
                    .filter(|f| f.id != *folder_id && folder_in_subtree(&f.id, folder_id))
                {
                    relocatable &= journal_folder(&descendant.id).is_some_and(|descendant_row| {
                        relative_under(&descendant_row.local_path, &row.local_path).is_some()
                    });
                }
                if relocatable && !files.is_empty() {
                    vanished_folders.push(VanishedFolder {
                        folder_id: (*folder_id).to_owned(),
                        files,
                    });
                }
            }
            if !vanished_folders.is_empty() {
                let landing = self.local_only_top_dirs(&folder_info).await;
                for (vanished, target) in self
                    .match_folder_renames(&vanished_folders, &landing, &mut report)
                    .await
                {
                    let folder_id = vanished.folder_id.as_str();
                    let (Some(row), Some(server_folder)) = (
                        journal_folder(folder_id),
                        server_folders_by_id.get(folder_id),
                    ) else {
                        continue;
                    };
                    let request = uncloud_common::UpdateFolderRequest {
                        name: (target.name != server_folder.name).then(|| target.name.clone()),
                        parent_id: (target.parent_id != server_folder.parent_id)
                            .then(|| target.parent_id.clone().unwrap_or_default()),
                        sync_strategy: None,
                        gallery_include: None,
                        music_include: None,
                    };
                    if request.name.is_none() && request.parent_id.is_none() {
                        continue;
                    }
                    let updated = {
                        self.journal
                            .set_config(
                                &format!("{MOVE_INTENT_PREFIX}{folder_id}"),
                                &serde_json::to_string(&(&row.local_path, &target.path, false))
                                    .unwrap(),
                            )
                            .await?;
                        let _g = self.transfer_guard();
                        self.client.update_folder(folder_id, &request).await
                    };
                    let updated = match updated {
                        Ok(updated) => updated,
                        Err(e) => {
                            // Fall through to the ordinary two-phase path.
                            report.errors.push(SyncError {
                                path: row.server_path.clone(),
                                reason: format!("rename on server: {e}"),
                            });
                            continue;
                        }
                    };
                    let rebase = |old_local_path: &str| {
                        relative_under(old_local_path, &row.local_path).map(|rel| {
                            rel.split('/')
                                .fold(target.path.clone(), |path, part| self.fs.join(&path, part))
                        })
                    };
                    let mut paths = vec![(
                        folder_id.to_owned(),
                        "folder".to_owned(),
                        target.path.clone(),
                    )];
                    folder_base_updates.push((folder_id.to_owned(), target.path.clone()));
                    for descendant in tree
                        .folders
                        .iter()
                        .filter(|f| f.id != folder_id && folder_in_subtree(&f.id, folder_id))
                    {
                        let Some(new_path) = journal_folder(&descendant.id)
                            .and_then(|descendant_row| rebase(&descendant_row.local_path))
                        else {
                            continue;
                        };
                        paths.push((descendant.id.clone(), "folder".to_owned(), new_path.clone()));
                        folder_base_updates.push((descendant.id.clone(), new_path));
                    }
                    for file in tree
                        .files
                        .iter()
                        .filter(|file| file_in_subtree(file, folder_id))
                    {
                        let Some(new_path) = journal_file(&file.id)
                            .and_then(|file_row| rebase(&file_row.local_path))
                        else {
                            continue;
                        };
                        paths.push((file.id.clone(), "file".to_owned(), new_path));
                    }
                    self.journal
                        .rebase_folder_paths(folder_id, &paths, &[])
                        .await?;
                    self.journal
                        .upsert(
                            folder_id,
                            "folder",
                            &updated.name,
                            &target.path,
                            None,
                            None,
                            &updated.updated_at,
                            None,
                            "synced",
                        )
                        .await?;
                    self.log_rename_remote(&row.local_path, &target.path).await;
                    report
                        .renamed_remote_folders
                        .push((row.local_path.clone(), target.path.clone()));
                    if let Some(id) = top_vanished.iter().find(|id| **id == folder_id) {
                        renamed_folder_ids.insert(*id);
                    }
                }
            }

            // The tree was fetched at the start of the run and transfers may
            // have taken minutes since. Before pushing any delete, fetch it
            // again and require the doomed items to be exactly as they were,
            // so an upload that landed in the meantime is not trashed along
            // with them.
            let confirmed_age = |row: &SyncStateRow| {
                row.delete_pending_since
                    .as_deref()
                    .and_then(|since| pending_age(since, now_dt))
                    .is_some_and(|age| age >= self.delete_confirm_delay)
            };
            let any_delete_due = top_vanished
                .iter()
                .filter(|folder_id| !renamed_folder_ids.contains(*folder_id))
                .filter_map(|folder_id| journal_folder(folder_id))
                .any(confirmed_age)
                || missing_files
                    .iter()
                    .filter(|row| !settled_file_ids.contains(row.server_id.as_str()))
                    .any(|row| confirmed_age(row));
            let fresh_tree = if any_delete_due {
                match self.client.sync_tree(None).await {
                    Ok(fresh) => Some(fresh),
                    Err(e) => {
                        report.errors.push(SyncError {
                            path: "deletions".to_owned(),
                            reason: format!("postponed, could not re-check the server: {e}"),
                        });
                        None
                    }
                }
            } else {
                None
            };

            for folder_id in top_vanished
                .iter()
                .copied()
                .filter(|folder_id| !renamed_folder_ids.contains(folder_id))
            {
                let Some(folder_journal) = journal_folder(folder_id) else {
                    continue;
                };
                let pending = folder_journal
                    .delete_pending_since
                    .as_deref()
                    .and_then(|since| pending_age(since, now_dt));
                match pending {
                    // Not marked yet, or the marker is unusable (unparseable,
                    // or in the future after a clock change): start over.
                    None => {
                        self.journal
                            .set_delete_pending(folder_id, "folder", &now)
                            .await?;
                        continue;
                    }
                    Some(age) if age < self.delete_confirm_delay => continue,
                    Some(_) => {}
                }

                let descendant_rows: Vec<&SyncStateRow> = tree
                    .folders
                    .iter()
                    .filter(|f| f.id != folder_id && folder_in_subtree(&f.id, folder_id))
                    .filter_map(|f| journal_folder(&f.id))
                    .collect();
                let subtree_file_rows: Vec<&SyncStateRow> = tree
                    .files
                    .iter()
                    .filter(|file| file_in_subtree(file, folder_id))
                    .filter_map(|file| journal_file(&file.id))
                    .collect();

                // Rows mapped outside the folder's own directory (per-folder
                // overrides) are not implied absent by it, so each is
                // confirmed on its own.
                let mut confirm_paths = vec![folder_journal.local_path.as_str()];
                confirm_paths.extend(
                    descendant_rows
                        .iter()
                        .chain(subtree_file_rows.iter())
                        .map(|row| row.local_path.as_str())
                        .filter(|path| relative_under(path, &folder_journal.local_path).is_none()),
                );
                let mut gone = true;
                for path in confirm_paths {
                    if !self.confirm_gone(path, &mut report).await {
                        gone = false;
                        break;
                    }
                }
                if !gone {
                    continue;
                }
                let unchanged_on_server = fresh_tree.as_ref().is_some_and(|fresh| {
                    let before = subtree_fingerprint(&tree, folder_id);
                    before.is_some() && before == subtree_fingerprint(fresh, folder_id)
                });
                if !unchanged_on_server {
                    continue;
                }

                match self.client.delete_folder(folder_id).await {
                    Ok(()) => {
                        self.log_delete_remote(&folder_journal.local_path, "Folder")
                            .await;
                        report
                            .deleted_local
                            .push(folder_journal.server_path.clone());
                        touched_paths.insert(folder_journal.local_path.clone());
                        self.journal.delete(folder_id, "folder").await?;
                        for row in descendant_rows {
                            touched_paths.insert(row.local_path.clone());
                            self.journal.delete(&row.server_id, "folder").await?;
                        }
                        for row in subtree_file_rows {
                            touched_paths.insert(row.local_path.clone());
                            self.journal.delete(&row.server_id, "file").await?;
                        }
                    }
                    Err(e) => report.errors.push(SyncError {
                        path: folder_journal.server_path.clone(),
                        reason: e.to_string(),
                    }),
                }
            }

            // File-by-file pass for missing files whose folder is still
            // around (or that sit at the server root).
            let fresh_files: HashMap<&str, &uncloud_common::FileResponse> = fresh_tree
                .iter()
                .flat_map(|fresh| fresh.files.iter())
                .map(|file| (file.id.as_str(), file))
                .collect();
            for j in missing_files {
                if settled_file_ids.contains(j.server_id.as_str()) {
                    continue;
                }
                let pending = j
                    .delete_pending_since
                    .as_deref()
                    .and_then(|since| pending_age(since, now_dt));
                match pending {
                    None => {
                        self.journal
                            .set_delete_pending(&j.server_id, "file", &now)
                            .await?;
                        continue;
                    }
                    Some(age) if age < self.delete_confirm_delay => continue,
                    Some(_) => {}
                }
                if !self.confirm_gone(&j.local_path, &mut report).await {
                    continue;
                }
                let unchanged_on_server = server_files_by_id
                    .get(j.server_id.as_str())
                    .zip(fresh_files.get(j.server_id.as_str()))
                    .is_some_and(|(before, fresh)| {
                        before.updated_at == fresh.updated_at && before.parent_id == fresh.parent_id
                    });
                if !unchanged_on_server {
                    continue;
                }
                match self.client.delete_file(&j.server_id).await {
                    Ok(()) => {
                        self.log_delete_remote(&j.local_path, "File").await;
                        report.deleted_local.push(j.server_path.clone());
                        self.journal.delete(&j.server_id, "file").await?;
                        touched_paths.insert(j.local_path.clone());
                    }
                    Err(e) => report.errors.push(SyncError {
                        path: j.server_path.clone(),
                        reason: e.to_string(),
                    }),
                }
            }

            // Renamed folders now live at their new local paths; later phases
            // must see those as known server folders, not as new local ones.
            for (folder_id, base_path) in folder_base_updates {
                if let Some(info) = folder_info.get_mut(&folder_id) {
                    info.base_path = Some(base_path);
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
                touched_paths: &std::collections::HashSet<String>,
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
                local_dirs.sort_by_key(|d| d.chars().filter(|&c| c == '/' || c == '\\').count());

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
                    if this.path_blocked(&full_path) || this.path_shadowed(&full_path) {
                        continue;
                    }
                    // Already a registered base → known server folder, skip.
                    if bases.iter().any(|(p, _, _)| p == &full_path) {
                        continue;
                    }
                    // Server-side delete echo handled by Phase 6.1 — don't
                    // resurrect the folder we just acknowledged the server
                    // deleted.
                    if touched_paths.contains(&full_path) {
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
                create_remote_dirs(
                    self,
                    &root,
                    None,
                    &mut folder_info,
                    &mut report,
                    &touched_paths,
                )
                .await;
            }

            // Pass (b): walk per-folder override bases.
            for (base, fid, strat) in self.override_walk_roots(&folder_info).await {
                create_remote_dirs(
                    self,
                    &base,
                    Some((&base, &fid, strat)),
                    &mut folder_info,
                    &mut report,
                    &touched_paths,
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
            let journal_map: HashMap<(String, String), crate::journal::SyncStateRow> = journal_rows
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
                    if self.probe_file(&full_path, &mut report).await != Presence::Present {
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
                    SyncStrategy::TwoWay | SyncStrategy::ClientToServer | SyncStrategy::UploadOnly
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
                    if self.probe_file(&full_path, &mut report).await != Presence::Present {
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

            self.journal
                .set_config("last_full_sync_at", &Utc::now().to_rfc3339())
                .await?;

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
            if !report.errors.is_empty() {}

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

        // Publish the final SyncState. `set_state` only emits on real
        // transition, so a successful run always lands on `Connected`
        // (clearing any prior `Error` / `NotConnected`); a failed run
        // classifies the error chain into transport vs logical.
        let final_state = match &result {
            Ok(_) => SyncState::Connected,
            Err(e) => Self::classify_run_error(e.as_ref()),
        };

        // A run that bailed out through `?` never reached the reporting at
        // the bottom of the body. Say why in the activity log (once per
        // distinct failure, not once per poll; transport failures already
        // show as the tray's not-connected state) and close the bracket if
        // a `SyncStart` went out.
        if let Err(e) = &result {
            let message = e.to_string();
            if final_state == SyncState::Error {
                let (first_in_run, log_it) = self.note_failure(&format!("run: {message}"));
                if first_in_run && log_it {
                    self.log_sync_error_row("run", &message).await;
                }
            }
            let emitted_start = self
                .run_state
                .read()
                .unwrap()
                .as_ref()
                .is_some_and(|state| state.emitted_start);
            if emitted_start {
                self.log_sync_marker("SyncEnd", end_reason, Some(format!("aborted: {message}")))
                    .await;
            }
        }
        {
            let mut memo = self.failure_memo.lock().unwrap();
            memo.previous_run = std::mem::take(&mut memo.this_run);
        }

        // Always clear the run-state slot. The body may have early-returned
        // via `?` from a phase failure, in which case the on-success take
        // never ran.
        let _ = self.run_state.write().unwrap().take();

        self.set_state(final_state);

        result.map_err(|e| e as Box<dyn std::error::Error>)
    }

    /// Download file `id` from the server and write it to `path` via the
    /// configured [`LocalFs`] backend.
    async fn download_to(&self, id: &str, path: &str) -> Result<String, String> {
        let _g = self.transfer_guard();
        let download = self
            .client
            .download_file_stream(id)
            .await
            .map_err(|e| e.to_string())?;
        // Set before the first byte is written and never cleared here: a
        // download that is cancelled gets no chance to clean up after itself.
        self.staging_dirty.store(true, Ordering::SeqCst);
        let mut hasher = Sha256::new();
        let chunks = download
            .map(|chunk| {
                let chunk = chunk.map_err(|e| LocalFsError::other(e.to_string()))?;
                hasher.update(&chunk);
                Ok(chunk)
            })
            .boxed();
        self.fs
            .write_stream(path, chunks)
            .await
            .map_err(|e| e.to_string())?;
        Ok(format!("{:x}", hasher.finalize()))
    }

    /// The content of a local file as an upload body, hashed as it is read,
    /// with its length where the backend can state it. The checksum is
    /// complete once the stream has been consumed.
    ///
    /// A stated length is binding: the stream stops there even if the file
    /// has grown since, so that the bytes hashed are the bytes sent. A file
    /// that shrank falls short of it and fails the request.
    async fn hashed_read_stream(
        &self,
        path: &str,
    ) -> Result<(ByteStream<'static>, Arc<Mutex<Sha256>>, Option<u64>), LocalFsError> {
        let length = self.fs.size(path).await?;
        let hasher = Arc::new(Mutex::new(Sha256::new()));
        let tee = Arc::clone(&hasher);
        let chunks = self
            .fs
            .read_stream(path)
            .await?
            .scan(length.unwrap_or(u64::MAX), |left, chunk| {
                if *left == 0 {
                    return futures::future::ready(None);
                }
                futures::future::ready(Some(chunk.map(|mut chunk| {
                    if chunk.len() as u64 > *left {
                        chunk.truncate(*left as usize);
                    }
                    *left -= chunk.len() as u64;
                    chunk
                })))
            })
            .inspect(move |chunk| {
                if let Ok(chunk) = chunk {
                    tee.lock().unwrap().update(chunk);
                }
            })
            .boxed();
        Ok((chunks, hasher, length))
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

        match self.hashed_read_stream(full_path).await {
            Ok((chunks, hasher, length)) => {
                match self
                    .client
                    .upload_stream(&file_name, chunks, length, parent_id)
                    .await
                {
                    Ok(new_file) => {
                        let checksum = format!("{:x}", hasher.lock().unwrap().clone().finalize());
                        let _ = self
                            .journal
                            .upsert(
                                &new_file.id,
                                "file",
                                &file_name,
                                full_path,
                                Some(new_file.size_bytes),
                                Some(&checksum),
                                &new_file.updated_at,
                                Some(mtime),
                                "synced",
                            )
                            .await;
                        self.log_upload(full_path, false).await;
                        report.uploaded.push(file_name);
                    }
                    Err(e) => report.errors.push(SyncError {
                        path: rel_path.to_owned(),
                        reason: e.to_string(),
                    }),
                }
            }
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
    ) -> Result<(uncloud_common::FileResponse, String, Option<i64>), String> {
        let _g = self.transfer_guard();
        // Taken before the first byte is read: a file that changes while it
        // is being sent then still reads as edited on the next scan.
        let mtime = self.fs.mtime(path).await.ok().flatten();
        let (chunks, hasher, length) = self
            .hashed_read_stream(path)
            .await
            .map_err(|e| e.to_string())?;
        let file_name = server_rel_path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(server_rel_path);
        self.client
            .update_file_content_stream(id, file_name, chunks, length)
            .await
            .map(|file| {
                let checksum = format!("{:x}", hasher.lock().unwrap().clone().finalize());
                (file, checksum, mtime)
            })
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
    ) -> Result<HashMap<String, ResolvedFolder>, Box<dyn std::error::Error + Send + Sync>> {
        // Pull all journal overrides in one pass.
        let mut overrides: HashMap<String, (Option<SyncStrategy>, Option<String>)> = HashMap::new();
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
        let Some((strategy_opt, _)) = row else {
            return Ok(None);
        };
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

    #[tokio::test]
    async fn local_checksum_is_reused_until_the_mtime_changes() {
        let dir = tempfile::tempdir().unwrap();
        let engine = SyncEngine::new(
            &dir.path().join("sync.db"),
            Arc::new(Client::new("http://127.0.0.1:9")),
            None,
        )
        .await
        .unwrap();
        let file = dir.path().join("file.txt");
        std::fs::write(&file, b"abc").unwrap();
        let path = file.to_str().unwrap();

        let first = engine.local_checksum(path, 10).await.unwrap();
        // With the file gone, only a remembered result can answer.
        std::fs::remove_file(&file).unwrap();
        assert_eq!(engine.local_checksum(path, 10).await.unwrap(), first);
        assert!(engine.local_checksum(path, 11).await.is_err());
    }

    #[tokio::test]
    async fn only_the_beginning_of_a_file_counts_as_a_cut_off_copy() {
        let dir = tempfile::tempdir().unwrap();
        let engine = SyncEngine::new(
            &dir.path().join("sync.db"),
            Arc::new(Client::new("http://127.0.0.1:9")),
            None,
        )
        .await
        .unwrap();
        // Longer than two read chunks, so the comparison crosses them.
        let whole: Vec<u8> = (0..700_000u32).map(|i| (i % 251) as u8).collect();
        let source = dir.path().join("source.bin");
        std::fs::write(&source, &whole).unwrap();
        let mut edited = whole[..300_000].to_vec();
        edited[299_999] ^= 1;
        let mut longer = whole.clone();
        longer.push(0);
        for (case, content, expected) in [
            ("nothing written yet", &[][..], true),
            ("stopped inside a chunk", &whole[..300_000], true),
            ("stopped one byte short", &whole[..699_999], true),
            ("complete", &whole[..], false),
            ("changed in its last byte", &edited[..], false),
            ("longer than the source", &longer[..], false),
        ] {
            let dest = dir.path().join("dest.bin");
            std::fs::write(&dest, content).unwrap();
            let found = engine
                .is_strict_prefix(dest.to_str().unwrap(), source.to_str().unwrap())
                .await
                .unwrap();
            assert_eq!(found, expected, "{case}");
        }
        let empty = dir.path().join("empty.bin");
        std::fs::write(&empty, b"").unwrap();
        let path = empty.to_str().unwrap();
        assert!(!engine.is_strict_prefix(path, path).await.unwrap());
    }

    #[test]
    fn move_intent_reads_every_recorded_form() {
        assert_eq!(
            parse_move_intent(r#"["/a","/b"]"#),
            Some(("/a".to_owned(), "/b".to_owned(), false, None))
        );
        assert_eq!(
            parse_move_intent(r#"["/a","/b",true]"#),
            Some(("/a".to_owned(), "/b".to_owned(), true, None))
        );
        assert_eq!(
            parse_move_intent(r#"["/a","/b",true,"sub/note.txt"]"#),
            Some((
                "/a".to_owned(),
                "/b".to_owned(),
                true,
                Some("sub/note.txt".to_owned())
            ))
        );
        assert_eq!(
            parse_move_intent(r#"["/a","/b",true,null]"#),
            Some(("/a".to_owned(), "/b".to_owned(), true, None))
        );
        assert_eq!(parse_move_intent("intent"), None);
    }

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
