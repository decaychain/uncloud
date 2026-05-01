use std::path::PathBuf;
#[cfg(desktop)]
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
#[cfg(desktop)]
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri::async_runtime;
#[cfg(desktop)]
use tauri_plugin_dialog::DialogExt;
#[cfg(desktop)]
use tauri_plugin_autostart::ManagerExt;
#[cfg(mobile)]
use tauri_plugin_android_fs::AndroidFsExt;
use tokio::sync::{Mutex, RwLock};
use tracing::{error, info, warn};
use uncloud_client::{Client, ClientIdentity};
use uncloud_sync::{
    BaseSource, SyncEngine, SyncEngineHooks, SyncLogRow, SyncReport,
    SyncState as EngineState,
};

#[cfg(mobile)]
mod android_fs;

#[cfg(desktop)]
mod file_watcher;

mod secret_store;

// ── App state ─────────────────────────────────────────────────────────────────

pub struct DesktopState {
    pub engine: Arc<RwLock<Option<Arc<SyncEngine>>>>,
    pub phase: Arc<Mutex<SyncPhase>>,
    pub stats: Arc<Mutex<SyncStats>>,
    pub client: Arc<RwLock<Option<Arc<Client>>>>,
    /// Desktop-level single-flight for `run_sync_once`. The engine has its
    /// own mutex around the actual sync work, but the desktop wrapper
    /// also resets `last_run_*` to 0 at the start and records
    /// `last_sync_at` at the end. Without this lock, two overlapping
    /// `run_sync_once` calls would clobber each other's bookkeeping.
    pub run_lock: Arc<Mutex<()>>,
    /// Tray icon handle. Stored so the activity listener can swap the
    /// per-state icons on [`EngineState`] transitions. Desktop-only.
    /// `std::sync::Mutex` keeps the call sites lock-free of `.await`.
    #[cfg(desktop)]
    pub tray: Arc<std::sync::Mutex<Option<TrayIcon>>>,
    /// Active filesystem watcher (kept alive for its `Drop` to detach
    /// inotify hooks on logout). Replaced when the configured root
    /// changes via login → engine rebuild.
    #[cfg(desktop)]
    pub watcher: Arc<std::sync::Mutex<Option<notify::RecommendedWatcher>>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum SyncPhase {
    NotConfigured,
    Idle,
    Syncing { started_at: String },
    Error { message: String },
}

impl Default for SyncPhase {
    fn default() -> Self {
        SyncPhase::NotConfigured
    }
}

/// Counters exposed to the UI. `session_*` accumulate across every sync run
/// since the app started; `last_run_*` reflect only the most recent completed
/// run.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SyncStats {
    pub session_uploaded: u32,
    pub session_downloaded: u32,
    pub session_deleted: u32,
    pub session_errors: u32,
    pub last_run_uploaded: u32,
    pub last_run_downloaded: u32,
    pub last_run_deleted: u32,
    pub last_run_errors: u32,
    pub last_sync_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncState {
    #[serde(flatten)]
    pub phase: SyncPhase,
    pub stats: SyncStats,
}

// ── DTOs (serialized for the frontend) ───────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfigDto {
    pub server_url: String,
    pub root_local_path: String,
    pub poll_interval_secs: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncErrorDto {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncReportDto {
    pub uploaded: Vec<String>,
    pub downloaded: Vec<String>,
    pub deleted_local: Vec<String>,
    pub conflict_count: usize,
    pub error_count: usize,
    pub errors: Vec<SyncErrorDto>,
}

impl From<SyncReport> for SyncReportDto {
    fn from(r: SyncReport) -> Self {
        let errors = r
            .errors
            .iter()
            .map(|e| SyncErrorDto {
                path: e.path.clone(),
                reason: e.reason.clone(),
            })
            .collect();
        SyncReportDto {
            conflict_count: r.conflicts.len(),
            error_count: r.errors.len(),
            uploaded: r.uploaded,
            downloaded: r.downloaded,
            deleted_local: r.deleted_local,
            errors,
        }
    }
}

// ── Persisted config ─────────────────────────────────────────────────────────

/// Non-secret state persisted to disk as JSON. The password lives separately
/// in the OS keyring (or an AES-GCM-encrypted file fallback) — see
/// `secret_store.rs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedConfig {
    server_url: String,
    username: String,
    root_path: String,
}

/// Struct returned to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigDto {
    pub server_url: String,
    pub username: String,
    pub root_path: String,
}

/// Subdirectory name for our config / data / fallback-credentials files.
/// Debug builds use `uncloud-dev` so a locally-built binary can never read
/// or overwrite a release install's state.
fn app_namespace() -> &'static str {
    if cfg!(debug_assertions) {
        "uncloud-dev"
    } else {
        "uncloud"
    }
}

fn config_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok()
        .or_else(|| dirs::config_dir())
        .map(|d| d.join(app_namespace()).join("desktop.json"))
}

/// Path to the sync journal database — stored in the user data dir, not inside
/// the sync root, so it is never picked up by the sync engine itself.
fn sync_db_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok()
        .or_else(|| dirs::data_local_dir())
        .map(|d| d.join(app_namespace()).join("sync.db"))
}

/// Directory the encrypted-file credential fallback lives in. Sits next to
/// `sync.db` in the data dir.
fn secrets_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok()
        .or_else(|| dirs::data_local_dir())
        .map(|d| d.join(app_namespace()).join("secrets"))
}

fn load_config_from(app: &AppHandle) -> Option<PersistedConfig> {
    let path = config_path(app)?;
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

fn save_config(app: &AppHandle, cfg: &PersistedConfig) {
    let Some(path) = config_path(app) else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(cfg) {
        let _ = std::fs::write(path, json);
    }
}

fn clear_config(app: &AppHandle) {
    if let Some(path) = config_path(app) {
        let _ = std::fs::remove_file(path);
    }
}

/// Identity advertised on every HTTP request to the server. Picked up by
/// the server's `request_meta_middleware` and attributed to the audit log.
fn sync_identity() -> ClientIdentity {
    let client_id = hostname::get()
        .ok()
        .and_then(|s| s.into_string().ok())
        .unwrap_or_else(|| "desktop".to_owned());
    ClientIdentity::sync(client_id, std::env::consts::OS)
}

/// Construct a [`Client`] that always sends the `X-Uncloud-*` headers.
fn make_client(base_url: &str) -> Client {
    Client::with_identity(base_url, sync_identity())
}

/// Snapshot of the current sync state shipped over the `sync-stats-changed`
/// Tauri event. Identical shape to [`SyncState`] so the frontend can deserialize
/// either `get_status` responses or event payloads interchangeably.
fn emit_stats(app: &AppHandle, phase: SyncPhase, stats: SyncStats) {
    let payload = SyncState { phase, stats };
    if let Err(e) = app.emit("sync-stats-changed", payload) {
        error!("emit sync-stats-changed: {}", e);
    }
}

/// Wire the engine's `on_log_appended` hook so each per-op row drives two
/// pieces of UI:
///   1. The activity log gets the row verbatim via `sync-log-appended`.
///   2. The relevant `session_*` / `last_run_*` counter ticks up and a
///      fresh `sync-stats-changed` event fires so the Sync tab's tiles
///      animate as the sync progresses, not just at the end.
///
/// Called each time an engine is freshly constructed (auto-login,
/// manual login, lazy init).
fn wire_engine_hooks(
    engine: &Arc<SyncEngine>,
    app: AppHandle,
    phase: Arc<Mutex<SyncPhase>>,
    stats: Arc<Mutex<SyncStats>>,
) {
    let on_log_appended: uncloud_sync::LogAppendedHook =
        Arc::new(move |row: &SyncLogRow| {
            // Always emit the raw row for the activity log.
            if let Err(e) = app.emit("sync-log-appended", row.clone()) {
                error!("emit sync-log-appended: {}", e);
            }

            // Bump the counter that matches this op. Tokio mutexes can't
            // be locked from a sync `Fn`, so spawn a short task per row;
            // the `app` / `phase` / `stats` clones are cheap (Arcs).
            let row = row.clone();
            let app = app.clone();
            let phase = phase.clone();
            let stats = stats.clone();
            async_runtime::spawn(async move {
                let updated = {
                    let mut s = stats.lock().await;
                    let bumped = match row.operation.as_str() {
                        "Uploaded" | "Updated on server" => {
                            s.session_uploaded = s.session_uploaded.saturating_add(1);
                            s.last_run_uploaded = s.last_run_uploaded.saturating_add(1);
                            true
                        }
                        "Downloaded" | "Updated from server" => {
                            s.session_downloaded = s.session_downloaded.saturating_add(1);
                            s.last_run_downloaded = s.last_run_downloaded.saturating_add(1);
                            true
                        }
                        "Deleted" => {
                            s.session_deleted = s.session_deleted.saturating_add(1);
                            s.last_run_deleted = s.last_run_deleted.saturating_add(1);
                            true
                        }
                        // SyncStart / SyncEnd are bracketing markers, no
                        // counter to bump.
                        _ => false,
                    };
                    if !bumped {
                        return;
                    }
                    s.clone()
                };
                let phase_snap = phase.lock().await.clone();
                emit_stats(&app, phase_snap, updated);
            });
        });
    engine.set_hooks(SyncEngineHooks {
        on_log_appended: Some(on_log_appended),
    });
}

/// (Re)start the filesystem watcher rooted at `root_path`. The previous
/// watcher (if any) is dropped, releasing its inotify/kqueue/etc.
/// handles. Called on every engine-creation path so a logout → login
/// cycle that targets a different folder reattaches cleanly.
#[cfg(desktop)]
fn restart_file_watcher(
    app: &AppHandle,
    state: &DesktopState,
    root_path: &str,
) {
    if root_path.is_empty() {
        // Mobile path-without-root never reaches here (cfg(desktop)),
        // but be defensive: nothing to watch.
        return;
    }
    let new_watcher = file_watcher::start_or_log(
        app.clone(),
        Path::new(root_path),
        state.engine.clone(),
        state.phase.clone(),
        state.stats.clone(),
        state.run_lock.clone(),
    );
    if let Ok(mut g) = state.watcher.lock() {
        // Drop the old watcher *after* the new one is in place so any
        // events that arrive in the gap aren't lost — the new watcher
        // catches them via inotify, the old one's queue drains harmlessly.
        *g = new_watcher;
    }
}

/// Subscribe to the engine's [`EngineState`] broadcast and swap the tray
/// icon on every transition. One icon per state — `Transferring` is sticky
/// for the run, so a sync session shows the syncing icon for its whole
/// duration instead of flickering between transfers.
///
/// Spawned each time a fresh engine is wired so a logout/login cycle ends
/// up with one listener per live engine. The previous task naturally ends
/// when the engine it captured is dropped — at which point its
/// `watch::Receiver` returns `Err` from `changed()` and we break.
#[cfg(desktop)]
fn spawn_activity_listener(
    engine: Arc<SyncEngine>,
    tray: Arc<std::sync::Mutex<Option<TrayIcon>>>,
) {
    let mut rx = engine.state();
    async_runtime::spawn(async move {
        let mut last = *rx.borrow();
        apply_tray_state(&tray, last);
        loop {
            if rx.changed().await.is_err() {
                // Sender dropped — engine is gone.
                break;
            }
            let next = *rx.borrow();
            if next != last {
                apply_tray_state(&tray, next);
                last = next;
            }
        }
    });
}

#[cfg(desktop)]
fn apply_tray_state(tray: &Arc<std::sync::Mutex<Option<TrayIcon>>>, state: EngineState) {
    let image = match state {
        EngineState::Transferring => tauri::include_image!("icons/tray-syncing.png"),
        EngineState::Connected => tauri::include_image!("icons/tray-idle.png"),
        EngineState::NotConnected => tauri::include_image!("icons/tray-disconnected.png"),
        EngineState::Error => tauri::include_image!("icons/tray-error.png"),
    };
    if let Ok(g) = tray.lock() {
        if let Some(t) = g.as_ref() {
            if let Err(e) = t.set_icon(Some(image)) {
                error!("set tray icon: {}", e);
            }
        }
    }
}

/// Build a [`SyncEngine`] with the platform-appropriate [`LocalFs`] backend:
/// [`uncloud_sync::NativeFs`] on desktop, `AndroidSafFs` on mobile.
async fn build_engine(
    app: &AppHandle,
    db_path: &std::path::Path,
    client: Arc<Client>,
    effective_root: Option<String>,
) -> Result<SyncEngine, Box<dyn std::error::Error>> {
    #[cfg(mobile)]
    {
        let fs = Arc::new(android_fs::AndroidSafFs::new(app.clone()));
        SyncEngine::with_fs(db_path, client, fs, effective_root).await
    }
    #[cfg(desktop)]
    {
        let _ = app;
        SyncEngine::new(db_path, client, effective_root).await
    }
}

// ── Autostart (desktop only) ──────────────────────────────────────────────────

/// Path of the sentinel that marks "we have already made the first-run
/// autostart decision for this user". Sits next to `desktop.json` so a
/// fresh data dir produces a clean first-run experience.
#[cfg(desktop)]
fn autostart_sentinel_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok()
        .or_else(|| dirs::config_dir())
        .map(|d| d.join(app_namespace()).join("autostart_decided"))
}

/// Default-on on first run: enable autostart and drop a sentinel so we
/// never override the user's later choice. Subsequent launches see the
/// sentinel and respect whatever the OS reports.
#[cfg(desktop)]
fn ensure_autostart_first_run(app: &AppHandle) {
    let Some(sentinel) = autostart_sentinel_path(app) else {
        warn!("autostart: cannot determine config directory");
        return;
    };
    if sentinel.exists() {
        return;
    }
    match app.autolaunch().enable() {
        Ok(()) => {
            if let Some(parent) = sentinel.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&sentinel, b"1");
            info!("autostart: enabled by default on first run");
        }
        Err(e) => warn!("autostart: enable failed: {}", e),
    }
}

#[cfg(desktop)]
#[tauri::command]
fn get_autostart(app: AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[cfg(mobile)]
#[tauri::command]
fn get_autostart(_app: AppHandle) -> Result<bool, String> {
    // Android handles autostart via system settings, not from inside the
    // app — surface "off" so the toggle hides on mobile builds.
    Ok(false)
}

#[cfg(desktop)]
#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<(), String> {
    let res = if enabled {
        app.autolaunch().enable()
    } else {
        app.autolaunch().disable()
    };
    res.map_err(|e| e.to_string())?;
    // Mark the first-run decision as made — even if the user toggles
    // before any sync runs, we record their choice so the default-on
    // logic doesn't fire on the next launch.
    if let Some(sentinel) = autostart_sentinel_path(&app) {
        if let Some(parent) = sentinel.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&sentinel, b"1");
    }
    Ok(())
}

#[cfg(mobile)]
#[tauri::command]
fn set_autostart(_app: AppHandle, _enabled: bool) -> Result<(), String> {
    Err("Autostart on mobile is configured via system settings".to_string())
}

// ── Tauri commands ────────────────────────────────────────────────────────────

/// Ensure the sync engine is initialized. If not, try to load persisted config
/// and spin it up synchronously. This self-heals from a failed auto-login at
/// startup so commands like `sync_now` can still work once the server is up.
async fn ensure_engine(
    app: &AppHandle,
    state: &State<'_, DesktopState>,
) -> Result<Arc<SyncEngine>, String> {
    if let Some(eng) = state.engine.read().await.as_ref().map(Arc::clone) {
        return Ok(eng);
    }
    let cfg = load_config_from(app).ok_or("Not configured (no saved config)")?;
    let secrets = secrets_dir(app).ok_or("Cannot determine data directory")?;
    let password = secret_store::load_password(&secrets, &cfg.server_url, &cfg.username)
        .ok_or("No saved credentials")?;
    let client = Arc::new(make_client(&cfg.server_url));
    client.login(&cfg.username, &password).await.map_err(|e| {
        let msg = format!("Login failed during lazy init: {e}");
        eprintln!("[uncloud-desktop] {msg}");
        msg
    })?;
    let db_path = sync_db_path(app).ok_or("Cannot determine data directory")?;
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let effective_root: Option<String> = if cfg.root_path.is_empty() {
        None
    } else {
        let p = PathBuf::from(&cfg.root_path);
        let _ = std::fs::create_dir_all(&p);
        Some(cfg.root_path.clone())
    };
    let engine = build_engine(app, &db_path, client.clone(), effective_root)
        .await
        .map_err(|e| {
            let msg = format!("Sync engine init failed: {e}");
            eprintln!("[uncloud-desktop] {msg}");
            msg
        })?;
    let engine = Arc::new(engine);
    wire_engine_hooks(&engine, app.clone(), state.phase.clone(), state.stats.clone());
    #[cfg(desktop)]
    spawn_activity_listener(engine.clone(), state.tray.clone());
    *state.client.write().await = Some(client);
    *state.engine.write().await = Some(engine.clone());
    *state.phase.lock().await = SyncPhase::Idle;
    emit_stats(app, SyncPhase::Idle, state.stats.lock().await.clone());
    eprintln!("[uncloud-desktop] Engine initialized lazily");

    #[cfg(desktop)]
    if !cfg.root_path.is_empty() {
        restart_file_watcher(app, &state, &cfg.root_path);
    }

    // Sync-on-start (lazy-init path: server came back online and a
    // command triggered re-init).
    spawn_sync(
        app.clone(),
        state.engine.clone(),
        state.phase.clone(),
        state.stats.clone(),
        state.run_lock.clone(),
    );
    Ok(engine)
}

#[tauri::command]
async fn get_status(state: State<'_, DesktopState>) -> Result<SyncState, String> {
    let phase = state.phase.lock().await.clone();
    let stats = state.stats.lock().await.clone();
    Ok(SyncState { phase, stats })
}

/// Run a single incremental sync and keep the desktop's `phase` / `stats`
/// in sync with the engine. Per-op counter increments happen inside the
/// engine's `on_log_appended` hook (see `wire_engine_hooks`), so this
/// function only handles the run-boundary bookkeeping:
///   • reset `last_run_*` to 0 before starting,
///   • record `last_sync_at` and `last_run_errors` at the end.
///
/// Concurrent invocations (poll loop tick + tray "Sync Now" + UI button +
/// mobile resume) serialize on `state.run_lock` so neither side clobbers
/// the other's counter resets, and the engine's own mutex serializes the
/// actual sync work behind that.
async fn run_sync_once(
    app: AppHandle,
    engine: Arc<SyncEngine>,
    phase: Arc<Mutex<SyncPhase>>,
    stats: Arc<Mutex<SyncStats>>,
    run_lock: Arc<Mutex<()>>,
) -> Result<SyncReport, String> {
    let _guard = run_lock.lock().await;

    let start_phase = SyncPhase::Syncing {
        started_at: Utc::now().to_rfc3339(),
    };
    let start_snapshot = {
        let mut s = stats.lock().await;
        s.last_run_uploaded = 0;
        s.last_run_downloaded = 0;
        s.last_run_deleted = 0;
        s.last_run_errors = 0;
        s.clone()
    };
    *phase.lock().await = start_phase.clone();
    emit_stats(&app, start_phase, start_snapshot);

    let result = engine.incremental_sync().await.map_err(|e| e.to_string());
    match result {
        Ok(report) => {
            // Hook already bumped uploaded/downloaded/deleted incrementally
            // — only errors still need to land here, since the engine
            // doesn't fire a log row for them.
            let stats_snapshot = {
                let mut s = stats.lock().await;
                let errors = report.errors.len() as u32;
                s.session_errors = s.session_errors.saturating_add(errors);
                s.last_run_errors = errors;
                s.last_sync_at = Some(Utc::now().to_rfc3339());
                s.clone()
            };
            *phase.lock().await = SyncPhase::Idle;
            emit_stats(&app, SyncPhase::Idle, stats_snapshot);
            Ok(report)
        }
        Err(msg) => {
            let err_phase = SyncPhase::Error {
                message: msg.clone(),
            };
            *phase.lock().await = err_phase.clone();
            emit_stats(&app, err_phase, stats.lock().await.clone());
            Err(msg)
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FolderEffectiveConfigDto {
    /// Per-device client strategy override, snake_case (e.g. "two_way"), or None.
    pub client_strategy: Option<String>,
    /// Server-resolved effective strategy (shared across clients), snake_case.
    pub effective_strategy: String,
    /// Resolved local base path for this folder's contents, or None.
    pub base_path: Option<String>,
    /// Where `base_path` came from: "self" / "ancestor" / "client_root" / "none".
    pub base_source: String,
    /// Ancestor folder id when `base_source == "ancestor"`; otherwise None.
    pub base_source_folder_id: Option<String>,
}

fn strategy_to_snake(s: uncloud_common::SyncStrategy) -> String {
    serde_json::to_string(&s)
        .unwrap_or_else(|_| "\"inherit\"".to_string())
        .trim_matches('"')
        .to_owned()
}

fn strategy_from_snake(s: &str) -> Result<uncloud_common::SyncStrategy, String> {
    serde_json::from_str(&format!("\"{}\"", s)).map_err(|e| format!("Invalid strategy: {}", e))
}

#[tauri::command]
async fn get_folder_effective_config(
    app: AppHandle,
    state: State<'_, DesktopState>,
    folder_id: String,
) -> Result<FolderEffectiveConfigDto, String> {
    let engine = ensure_engine(&app, &state).await?;
    let cfg = engine
        .get_folder_effective_config(&folder_id)
        .await
        .map_err(|e| e.to_string())?;

    let base_source_folder_id = match &cfg.base_source {
        BaseSource::Ancestor(id) => Some(id.clone()),
        _ => None,
    };

    Ok(FolderEffectiveConfigDto {
        client_strategy: cfg.client_strategy.map(strategy_to_snake),
        effective_strategy: strategy_to_snake(cfg.effective_strategy),
        base_path: cfg.base_path,
        base_source: cfg.base_source.as_str().to_string(),
        base_source_folder_id,
    })
}

#[tauri::command]
async fn login(
    app: AppHandle,
    state: State<'_, DesktopState>,
    server: String,
    username: String,
    password: String,
    root_path: String,
) -> Result<(), String> {
    let client = Arc::new(make_client(&server));
    client
        .login(&username, &password)
        .await
        .map_err(|e| e.to_string())?;

    let db_path = sync_db_path(&app).ok_or("Cannot determine data directory")?;
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    // Desktop requires a root path (enforced at the setup UI). Mobile passes
    // an empty string — there is no global sync root on Android, only
    // per-folder picks.
    #[cfg(desktop)]
    if root_path.is_empty() {
        return Err("A sync folder is required on desktop".to_string());
    }

    let effective_root: Option<String> = if root_path.is_empty() {
        None
    } else {
        Some(root_path.clone())
    };

    let engine = build_engine(&app, &db_path, client.clone(), effective_root)
        .await
        .map_err(|e| e.to_string())?;

    let engine = Arc::new(engine);
    wire_engine_hooks(&engine, app.clone(), state.phase.clone(), state.stats.clone());
    #[cfg(desktop)]
    spawn_activity_listener(engine.clone(), state.tray.clone());
    *state.client.write().await = Some(client);
    *state.engine.write().await = Some(engine);
    *state.phase.lock().await = SyncPhase::Idle;
    let stats_snapshot = {
        let mut s = state.stats.lock().await;
        s.last_sync_at = Some(Utc::now().to_rfc3339());
        s.clone()
    };
    emit_stats(&app, SyncPhase::Idle, stats_snapshot);

    let secrets = secrets_dir(&app).ok_or("Cannot determine data directory")?;
    secret_store::store_password(&secrets, &server, &username, &password)?;
    save_config(&app, &PersistedConfig { server_url: server, username, root_path: root_path.clone() });
    info!("Logged in and sync engine initialised");

    #[cfg(desktop)]
    if !root_path.is_empty() {
        restart_file_watcher(&app, &state, &root_path);
    }

    // Sync-on-start: kick off an immediate sync so the user sees the
    // server state without having to wait for the first poll tick.
    spawn_sync(
        app.clone(),
        state.engine.clone(),
        state.phase.clone(),
        state.stats.clone(),
        state.run_lock.clone(),
    );
    Ok(())
}

#[tauri::command]
fn get_config(app: AppHandle) -> Option<ConfigDto> {
    load_config_from(&app).map(|c| ConfigDto {
        server_url: c.server_url,
        username: c.username,
        root_path: c.root_path,
    })
}

#[tauri::command]
async fn disconnect(app: AppHandle, state: State<'_, DesktopState>) -> Result<(), String> {
    // Capture identity before we wipe the file so we know which keyring/file
    // entry to remove.
    let prev = load_config_from(&app);

    *state.engine.write().await = None;
    *state.client.write().await = None;
    *state.phase.lock().await = SyncPhase::NotConfigured;
    *state.stats.lock().await = SyncStats::default();
    #[cfg(desktop)]
    if let Ok(mut g) = state.watcher.lock() {
        *g = None;
    }
    clear_config(&app);

    if let (Some(cfg), Some(secrets)) = (prev, secrets_dir(&app)) {
        secret_store::delete_password(&secrets, &cfg.server_url, &cfg.username);
    }

    emit_stats(&app, SyncPhase::NotConfigured, SyncStats::default());
    Ok(())
}

#[tauri::command]
async fn sync_now(app: AppHandle, state: State<'_, DesktopState>) -> Result<SyncReportDto, String> {
    let engine = ensure_engine(&app, &state).await?;
    let report = run_sync_once(
        app.clone(),
        engine,
        state.phase.clone(),
        state.stats.clone(),
        state.run_lock.clone(),
    )
    .await?;
    eprintln!(
        "[uncloud-desktop] sync report: uploaded={} downloaded={} deleted_local={} conflicts={} errors={}",
        report.uploaded.len(),
        report.downloaded.len(),
        report.deleted_local.len(),
        report.conflicts.len(),
        report.errors.len(),
    );
    for err in &report.errors {
        eprintln!("[uncloud-desktop] sync error: {} — {}", err.path, err.reason);
    }
    Ok(SyncReportDto::from(report))
}

/// Read the last `limit` rows from the local sync audit log. The desktop
/// "This Device" tab calls this once on mount and thereafter relies on
/// `sync-log-appended` Tauri events to stay fresh.
#[tauri::command]
async fn get_local_sync_log(
    app: AppHandle,
    state: State<'_, DesktopState>,
    limit: Option<i64>,
) -> Result<Vec<SyncLogRow>, String> {
    let engine = ensure_engine(&app, &state).await?;
    let cap = limit.unwrap_or(200).clamp(1, 1000);
    engine
        .recent_sync_log(cap)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn set_folder_local_strategy(
    app: AppHandle,
    state: State<'_, DesktopState>,
    folder_id: String,
    strategy: Option<String>,
) -> Result<(), String> {
    let strategy = match strategy {
        Some(s) => Some(strategy_from_snake(&s)?),
        None => None,
    };
    let engine = ensure_engine(&app, &state).await?;
    engine
        .set_folder_local_strategy(&folder_id, strategy)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn set_folder_local_path(
    app: AppHandle,
    state: State<'_, DesktopState>,
    folder_id: String,
    local_path: Option<String>,
) -> Result<(), String> {
    let engine = ensure_engine(&app, &state).await?;
    engine
        .set_folder_local_path(&folder_id, local_path.as_deref())
        .await
        .map_err(|e| e.to_string())
}

#[cfg(desktop)]
#[tauri::command]
async fn pick_folder(app: AppHandle) -> Result<Option<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |folder_path| {
        let _ = tx.send(folder_path.map(|p| p.to_string()));
    });
    rx.await.map_err(|e| e.to_string())
}

#[cfg(mobile)]
#[tauri::command]
async fn pick_folder(app: AppHandle) -> Result<Option<String>, String> {
    let api = app.android_fs_async();
    let selected = api
        .file_picker()
        .pick_dir(None, false)
        .await
        .map_err(|e| e.to_string())?;
    match selected {
        Some(dir_uri) => {
            // Persist access so it survives app/device restarts.
            api.file_picker()
                .persist_uri_permission(&dir_uri)
                .await
                .map_err(|e| e.to_string())?;
            Ok(Some(dir_uri.uri))
        }
        None => Ok(None),
    }
}

#[cfg(desktop)]
#[tauri::command]
fn default_sync_folder() -> Option<String> {
    dirs::home_dir().map(|h| h.join("Uncloud").to_string_lossy().to_string())
}

#[cfg(mobile)]
#[tauri::command]
fn default_sync_folder() -> Option<String> {
    // On Android, sync is per-folder — no global default.
    None
}

// ── Polling scheduler ─────────────────────────────────────────────────────────

fn start_poll_loop(
    app: AppHandle,
    engine: Arc<RwLock<Option<Arc<SyncEngine>>>>,
    phase: Arc<Mutex<SyncPhase>>,
    stats: Arc<Mutex<SyncStats>>,
    run_lock: Arc<Mutex<()>>,
    poll_interval_secs: u64,
) {
    async_runtime::spawn(async move {
        let mut interval =
            tokio::time::interval(Duration::from_secs(poll_interval_secs));
        // Don't burst-fire missed ticks after a long OS suspend — just resume
        // cadence from "now".
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let maybe_engine = engine.read().await.as_ref().map(Arc::clone);
            if let Some(eng) = maybe_engine {
                if let Err(msg) =
                    run_sync_once(app.clone(), eng, phase.clone(), stats.clone(), run_lock.clone()).await
                {
                    error!("Sync error: {}", msg);
                }
            }
        }
    });
}

/// Spawn an immediate sync (best-effort; no-op if no engine yet). Used by the
/// mobile resume handler and the tray menu.
fn spawn_sync(
    app: AppHandle,
    engine: Arc<RwLock<Option<Arc<SyncEngine>>>>,
    phase: Arc<Mutex<SyncPhase>>,
    stats: Arc<Mutex<SyncStats>>,
    run_lock: Arc<Mutex<()>>,
) {
    async_runtime::spawn(async move {
        let maybe_engine = engine.read().await.as_ref().map(Arc::clone);
        if let Some(eng) = maybe_engine {
            if let Err(msg) = run_sync_once(app, eng, phase, stats, run_lock).await {
                error!("Sync error: {}", msg);
            }
        }
    });
}

// ── Desktop-only: window management ──────────────────────────────────────────

#[cfg(desktop)]
fn open_browser(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("browser") {
        let _ = w.show();
        return;
    }
    // The Dioxus app fetches the server URL via invoke("get_config") in main().
    let _ = tauri::WebviewWindowBuilder::new(
        app,
        "browser",
        tauri::WebviewUrl::App("".into()),
    )
    .title("Uncloud")
    .inner_size(1280.0, 800.0)
    .resizable(true)
    .build();
}

// ── App entry point ───────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let state = DesktopState {
        engine: Arc::new(RwLock::new(None)),
        phase: Arc::new(Mutex::new(SyncPhase::default())),
        stats: Arc::new(Mutex::new(SyncStats::default())),
        client: Arc::new(RwLock::new(None)),
        run_lock: Arc::new(Mutex::new(())),
        #[cfg(desktop)]
        tray: Arc::new(std::sync::Mutex::new(None)),
        #[cfg(desktop)]
        watcher: Arc::new(std::sync::Mutex::new(None)),
    };

    // Cloned handles for the desktop tray menu handler.
    #[cfg(desktop)]
    let engine_arc = state.engine.clone();
    #[cfg(desktop)]
    let phase_arc = state.phase.clone();
    #[cfg(desktop)]
    let stats_arc = state.stats.clone();
    #[cfg(desktop)]
    let run_lock_arc = state.run_lock.clone();
    // Separate clones owned by the run-event handler (mobile resume).
    #[cfg(mobile)]
    let run_engine = state.engine.clone();
    #[cfg(mobile)]
    let run_phase = state.phase.clone();
    #[cfg(mobile)]
    let run_stats = state.stats.clone();
    #[cfg(mobile)]
    let run_run_lock = state.run_lock.clone();

    let mut builder = tauri::Builder::default();
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_dialog::init());
        // Autostart plugin: registers the binary with the OS launch
        // mechanism. The plugin is configured here in code so the
        // first-run "default on" decision lives next to the rest of
        // the desktop wiring instead of in tauri.conf.json.
        builder = builder.plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None, // no extra args; the binary handles its own state
        ));
    }
    #[cfg(mobile)]
    {
        builder = builder.plugin(tauri_plugin_android_fs::init());
        builder = builder.plugin(tauri_plugin_native_audio::init());
    }

    builder
        .manage(state)
        .setup(move |app| {
            // First-run default: enable autostart unless the user has
            // already made a choice (sentinel present).
            #[cfg(desktop)]
            ensure_autostart_first_run(app.handle());

            // Auto-login from persisted config + stored credentials if both exist.
            if let Some(cfg) = load_config_from(app.handle()) {
                let ds = app.state::<DesktopState>();
                let engine_arc = ds.engine.clone();
                let phase_arc = ds.phase.clone();
                let stats_arc = ds.stats.clone();
                let client_arc = ds.client.clone();
                let run_lock_arc_auto = ds.run_lock.clone();
                #[cfg(desktop)]
                let tray_arc = ds.tray.clone();
                let app_handle = app.handle().clone();
                async_runtime::spawn(async move {
                    let Some(secrets) = secrets_dir(&app_handle) else {
                        error!("Auto-login: cannot determine data directory");
                        return;
                    };
                    let Some(password) = secret_store::load_password(&secrets, &cfg.server_url, &cfg.username) else {
                        eprintln!("[uncloud-desktop] Auto-login skipped: no stored credentials");
                        return;
                    };
                    let client = Arc::new(make_client(&cfg.server_url));
                    let login_result = client.login(&cfg.username, &password).await.map_err(|e| e.to_string());
                    match login_result {
                        Ok(_) => {
                            let db = match sync_db_path(&app_handle) {
                                Some(p) => { let _ = p.parent().map(std::fs::create_dir_all); p }
                                None => { error!("Auto-login: cannot determine data directory"); return; }
                            };
                            let effective_root: Option<String> = if cfg.root_path.is_empty() {
                                None
                            } else {
                                Some(cfg.root_path.clone())
                            };
                            let engine_result = build_engine(&app_handle, &db, client.clone(), effective_root).await.map_err(|e| e.to_string());
                            match engine_result {
                                Ok(engine) => {
                                    let engine = Arc::new(engine);
                                    wire_engine_hooks(&engine, app_handle.clone(), phase_arc.clone(), stats_arc.clone());
                                    #[cfg(desktop)]
                                    spawn_activity_listener(engine.clone(), tray_arc.clone());
                                    *client_arc.write().await = Some(client);
                                    *engine_arc.write().await = Some(engine);
                                    *phase_arc.lock().await = SyncPhase::Idle;
                                    let stats_snapshot = {
                                        let ds = app_handle.state::<DesktopState>();
                                        let guard = ds.stats.lock().await;
                                        guard.clone()
                                    };
                                    emit_stats(&app_handle, SyncPhase::Idle, stats_snapshot);
                                    eprintln!("[uncloud-desktop] Auto-login successful");
                                    info!("Auto-login successful");

                                    #[cfg(desktop)]
                                    if !cfg.root_path.is_empty() {
                                        let st = app_handle.state::<DesktopState>();
                                        restart_file_watcher(&app_handle, &st, &cfg.root_path);
                                    }

                                    // Sync-on-start (auto-login path).
                                    spawn_sync(
                                        app_handle.clone(),
                                        engine_arc.clone(),
                                        phase_arc.clone(),
                                        stats_arc.clone(),
                                        run_lock_arc_auto.clone(),
                                    );
                                }
                                Err(e) => {
                                    eprintln!("[uncloud-desktop] Auto-login engine init failed: {e}");
                                    error!("Auto-login: engine init failed: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("[uncloud-desktop] Auto-login login failed: {e}");
                            error!("Auto-login: login failed: {}", e);
                        },
                    }
                });
            }

            // Desktop: system tray + menu
            #[cfg(desktop)]
            {
                let tray_engine = engine_arc.clone();
                let tray_phase = phase_arc.clone();
                let tray_stats = stats_arc.clone();
                let tray_run_lock = run_lock_arc.clone();
                let quit = MenuItemBuilder::new("Quit").id("quit").build(app)?;
                let open = MenuItemBuilder::new("Open Uncloud").id("open").build(app)?;
                let sync_now_item = MenuItemBuilder::new("Sync Now").id("sync_now").build(app)?;
                let sep = PredefinedMenuItem::separator(app)?;

                let menu = MenuBuilder::new(app)
                    .items(&[&open, &sep, &sync_now_item, &sep, &quit])
                    .build()?;

                let tray_icon = TrayIconBuilder::new()
                    .icon(tauri::include_image!("icons/tray-idle.png"))
                    .menu(&menu)
                    .on_tray_icon_event(|tray, event| {
                        if let TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        } = event
                        {
                            open_browser(tray.app_handle());
                        }
                    })
                    .on_menu_event(move |app, event| match event.id().as_ref() {
                        "quit" => app.exit(0),
                        "open" => open_browser(app),
                        "sync_now" => {
                            spawn_sync(
                                app.clone(),
                                tray_engine.clone(),
                                tray_phase.clone(),
                                tray_stats.clone(),
                                tray_run_lock.clone(),
                            );
                        }
                        _ => {}
                    })
                    .build(app)?;

                // Hand the tray to DesktopState so the activity listener
                // can swap icons later.
                let tray_slot: Arc<std::sync::Mutex<Option<TrayIcon>>> = {
                    let st = app.state::<DesktopState>();
                    st.tray.clone()
                };
                if let Ok(mut g) = tray_slot.lock() {
                    *g = Some(tray_icon);
                };
            }

            // Poll loop runs on both desktop and mobile.
            start_poll_loop(
                app.handle().clone(),
                app.state::<DesktopState>().engine.clone(),
                app.state::<DesktopState>().phase.clone(),
                app.state::<DesktopState>().stats.clone(),
                app.state::<DesktopState>().run_lock.clone(),
                60,
            );

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            get_config,
            login,
            disconnect,
            sync_now,
            get_local_sync_log,
            set_folder_local_strategy,
            set_folder_local_path,
            get_folder_effective_config,
            pick_folder,
            default_sync_folder,
            get_autostart,
            set_autostart,
        ])
        .build(tauri::generate_context!())
        .expect("error building Tauri application")
        .run(move |_app, event| {
            // Keep alive when the last window closes (tray-only mode on desktop).
            // On mobile this event never fires in the same way.
            #[cfg(desktop)]
            if let tauri::RunEvent::ExitRequested { api, code, .. } = &event {
                if code.is_none() {
                    api.prevent_exit();
                }
            }
            // On Android the OS suspends the process when the screen is off and
            // the tokio interval loop may not tick on resume. Kick an immediate
            // sync when the app returns to the foreground so the UI reflects
            // current state and sync actually resumes.
            #[cfg(mobile)]
            if let tauri::RunEvent::Resumed = &event {
                spawn_sync(
                    _app.clone(),
                    run_engine.clone(),
                    run_phase.clone(),
                    run_stats.clone(),
                    run_run_lock.clone(),
                );
            }
        });
}
