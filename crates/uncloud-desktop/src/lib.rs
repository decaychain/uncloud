use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
#[cfg(desktop)]
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use tauri::{AppHandle, Manager, State};
use tauri::async_runtime;
#[cfg(desktop)]
use tauri_plugin_dialog::DialogExt;
#[cfg(mobile)]
use tauri_plugin_android_fs::AndroidFsExt;
use tokio::sync::{Mutex, RwLock};
use tracing::{error, info};
use uncloud_client::Client;
use uncloud_sync::{BaseSource, SyncEngine, SyncReport};

#[cfg(mobile)]
mod android_fs;

mod secret_store;

// ── App state ─────────────────────────────────────────────────────────────────

pub struct DesktopState {
    pub engine: Arc<RwLock<Option<Arc<SyncEngine>>>>,
    pub phase: Arc<Mutex<SyncPhase>>,
    pub stats: Arc<Mutex<SyncStats>>,
    pub client: Arc<RwLock<Option<Arc<Client>>>>,
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
    let client = Arc::new(Client::new(&cfg.server_url));
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
    *state.client.write().await = Some(client);
    *state.engine.write().await = Some(engine.clone());
    *state.phase.lock().await = SyncPhase::Idle;
    eprintln!("[uncloud-desktop] Engine initialized lazily");
    Ok(engine)
}

#[tauri::command]
async fn get_status(state: State<'_, DesktopState>) -> Result<SyncState, String> {
    let phase = state.phase.lock().await.clone();
    let stats = state.stats.lock().await.clone();
    Ok(SyncState { phase, stats })
}

/// Run a single incremental sync, updating phase + stats as it progresses.
/// Shared by `sync_now`, the poll loop, the desktop tray menu, and the mobile
/// resume handler.
async fn run_sync_once(
    engine: Arc<SyncEngine>,
    phase: Arc<Mutex<SyncPhase>>,
    stats: Arc<Mutex<SyncStats>>,
) -> Result<SyncReport, String> {
    *phase.lock().await = SyncPhase::Syncing {
        started_at: Utc::now().to_rfc3339(),
    };
    let result = engine.incremental_sync().await.map_err(|e| e.to_string());
    match result {
        Ok(report) => {
            {
                let mut s = stats.lock().await;
                let uploaded = report.uploaded.len() as u32;
                let downloaded = report.downloaded.len() as u32;
                let deleted = report.deleted_local.len() as u32;
                let errors = report.errors.len() as u32;
                s.session_uploaded = s.session_uploaded.saturating_add(uploaded);
                s.session_downloaded = s.session_downloaded.saturating_add(downloaded);
                s.session_deleted = s.session_deleted.saturating_add(deleted);
                s.session_errors = s.session_errors.saturating_add(errors);
                s.last_run_uploaded = uploaded;
                s.last_run_downloaded = downloaded;
                s.last_run_deleted = deleted;
                s.last_run_errors = errors;
                s.last_sync_at = Some(Utc::now().to_rfc3339());
            }
            *phase.lock().await = SyncPhase::Idle;
            Ok(report)
        }
        Err(msg) => {
            *phase.lock().await = SyncPhase::Error {
                message: msg.clone(),
            };
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
    let client = Arc::new(Client::new(&server));
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

    *state.client.write().await = Some(client);
    *state.engine.write().await = Some(Arc::new(engine));
    *state.phase.lock().await = SyncPhase::Idle;
    {
        let mut s = state.stats.lock().await;
        s.last_sync_at = Some(Utc::now().to_rfc3339());
    }

    let secrets = secrets_dir(&app).ok_or("Cannot determine data directory")?;
    secret_store::store_password(&secrets, &server, &username, &password)?;
    save_config(&app, &PersistedConfig { server_url: server, username, root_path });
    info!("Logged in and sync engine initialised");
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
    clear_config(&app);

    if let (Some(cfg), Some(secrets)) = (prev, secrets_dir(&app)) {
        secret_store::delete_password(&secrets, &cfg.server_url, &cfg.username);
    }
    Ok(())
}

#[tauri::command]
async fn sync_now(app: AppHandle, state: State<'_, DesktopState>) -> Result<SyncReportDto, String> {
    let engine = ensure_engine(&app, &state).await?;
    let report = run_sync_once(engine, state.phase.clone(), state.stats.clone()).await?;
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
    engine: Arc<RwLock<Option<Arc<SyncEngine>>>>,
    phase: Arc<Mutex<SyncPhase>>,
    stats: Arc<Mutex<SyncStats>>,
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
                    run_sync_once(eng, phase.clone(), stats.clone()).await
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
    engine: Arc<RwLock<Option<Arc<SyncEngine>>>>,
    phase: Arc<Mutex<SyncPhase>>,
    stats: Arc<Mutex<SyncStats>>,
) {
    async_runtime::spawn(async move {
        let maybe_engine = engine.read().await.as_ref().map(Arc::clone);
        if let Some(eng) = maybe_engine {
            if let Err(msg) = run_sync_once(eng, phase, stats).await {
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
    };

    // Cloned handles for the desktop tray menu handler.
    #[cfg(desktop)]
    let engine_arc = state.engine.clone();
    #[cfg(desktop)]
    let phase_arc = state.phase.clone();
    #[cfg(desktop)]
    let stats_arc = state.stats.clone();
    // Separate clones owned by the run-event handler (mobile resume).
    #[cfg(mobile)]
    let run_engine = state.engine.clone();
    #[cfg(mobile)]
    let run_phase = state.phase.clone();
    #[cfg(mobile)]
    let run_stats = state.stats.clone();

    let mut builder = tauri::Builder::default();
    #[cfg(desktop)]
    { builder = builder.plugin(tauri_plugin_dialog::init()); }
    #[cfg(mobile)]
    {
        builder = builder.plugin(tauri_plugin_android_fs::init());
        builder = builder.plugin(tauri_plugin_native_audio::init());
    }

    builder
        .manage(state)
        .setup(move |app| {
            // Auto-login from persisted config + stored credentials if both exist.
            if let Some(cfg) = load_config_from(app.handle()) {
                let ds = app.state::<DesktopState>();
                let engine_arc = ds.engine.clone();
                let phase_arc = ds.phase.clone();
                let client_arc = ds.client.clone();
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
                    let client = Arc::new(Client::new(&cfg.server_url));
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
                                    *client_arc.write().await = Some(client);
                                    *engine_arc.write().await = Some(Arc::new(engine));
                                    *phase_arc.lock().await = SyncPhase::Idle;
                                    eprintln!("[uncloud-desktop] Auto-login successful");
                                    info!("Auto-login successful");
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
                let quit = MenuItemBuilder::new("Quit").id("quit").build(app)?;
                let open = MenuItemBuilder::new("Open Uncloud").id("open").build(app)?;
                let sync_now_item = MenuItemBuilder::new("Sync Now").id("sync_now").build(app)?;
                let sep = PredefinedMenuItem::separator(app)?;

                let menu = MenuBuilder::new(app)
                    .items(&[&open, &sep, &sync_now_item, &sep, &quit])
                    .build()?;

                let _tray = TrayIconBuilder::new()
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
                                tray_engine.clone(),
                                tray_phase.clone(),
                                tray_stats.clone(),
                            );
                        }
                        _ => {}
                    })
                    .build(app)?;
            }

            // Poll loop runs on both desktop and mobile.
            start_poll_loop(
                app.state::<DesktopState>().engine.clone(),
                app.state::<DesktopState>().phase.clone(),
                app.state::<DesktopState>().stats.clone(),
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
            set_folder_local_strategy,
            set_folder_local_path,
            get_folder_effective_config,
            pick_folder,
            default_sync_folder,
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
                spawn_sync(run_engine.clone(), run_phase.clone(), run_stats.clone());
            }
        });
}
