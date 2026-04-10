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
use uncloud_sync::{SyncEngine, SyncReport};

// ── App state ─────────────────────────────────────────────────────────────────

pub struct DesktopState {
    pub engine: Arc<RwLock<Option<Arc<SyncEngine>>>>,
    pub status: Arc<Mutex<SyncStatus>>,
    pub client: Arc<RwLock<Option<Arc<Client>>>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SyncStatus {
    NotConfigured,
    Idle { last_sync: String },
    Syncing { started_at: String },
    Error { message: String },
}

impl Default for SyncStatus {
    fn default() -> Self {
        SyncStatus::NotConfigured
    }
}

// ── DTOs (serialized for the frontend) ───────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfigDto {
    pub server_url: String,
    pub root_local_path: String,
    pub poll_interval_secs: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncReportDto {
    pub uploaded: Vec<String>,
    pub downloaded: Vec<String>,
    pub deleted_local: Vec<String>,
    pub conflict_count: usize,
    pub error_count: usize,
}

impl From<SyncReport> for SyncReportDto {
    fn from(r: SyncReport) -> Self {
        SyncReportDto {
            conflict_count: r.conflicts.len(),
            error_count: r.errors.len(),
            uploaded: r.uploaded,
            downloaded: r.downloaded,
            deleted_local: r.deleted_local,
        }
    }
}

// ── Persisted config ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedConfig {
    server_url: String,
    username: String,
    password: String,
    root_path: String,
}

/// Struct returned to the frontend — password is intentionally omitted.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigDto {
    pub server_url: String,
    pub username: String,
    pub root_path: String,
}

fn config_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok()
        .or_else(|| dirs::config_dir())
        .map(|d| d.join("uncloud").join("desktop.json"))
}

/// Path to the sync journal database — stored in the user data dir, not inside
/// the sync root, so it is never picked up by the sync engine itself.
fn sync_db_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok()
        .or_else(|| dirs::data_local_dir())
        .map(|d| d.join("uncloud").join("sync.db"))
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

// ── Tauri commands ────────────────────────────────────────────────────────────

#[tauri::command]
async fn get_status(state: State<'_, DesktopState>) -> Result<SyncStatus, String> {
    Ok(state.status.lock().await.clone())
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
    let engine = SyncEngine::new(&db_path, client.clone(), PathBuf::from(&root_path))
        .await
        .map_err(|e| e.to_string())?;

    *state.client.write().await = Some(client);
    *state.engine.write().await = Some(Arc::new(engine));
    *state.status.lock().await = SyncStatus::Idle {
        last_sync: Utc::now().to_rfc3339(),
    };

    save_config(&app, &PersistedConfig { server_url: server, username, password, root_path });
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
    *state.engine.write().await = None;
    *state.client.write().await = None;
    *state.status.lock().await = SyncStatus::NotConfigured;
    clear_config(&app);
    Ok(())
}

#[tauri::command]
async fn sync_now(state: State<'_, DesktopState>) -> Result<SyncReportDto, String> {
    let engine = state.engine.read().await.as_ref().map(Arc::clone).ok_or("Not configured")?;

    *state.status.lock().await = SyncStatus::Syncing {
        started_at: Utc::now().to_rfc3339(),
    };

    // Convert Box<dyn Error> (!Send) to String (Send) before any subsequent .await.
    let result = engine.incremental_sync().await.map_err(|e| e.to_string());
    match result {
        Ok(report) => {
            *state.status.lock().await = SyncStatus::Idle {
                last_sync: Utc::now().to_rfc3339(),
            };
            Ok(SyncReportDto::from(report))
        }
        Err(msg) => {
            *state.status.lock().await = SyncStatus::Error {
                message: msg.clone(),
            };
            Err(msg)
        }
    }
}

#[tauri::command]
async fn set_folder_strategy(
    state: State<'_, DesktopState>,
    folder_id: String,
    strategy: String,
    local_path: Option<String>,
) -> Result<(), String> {
    let strategy: uncloud_common::SyncStrategy =
        serde_json::from_str(&format!("\"{}\"", strategy))
            .map_err(|e| format!("Invalid strategy: {}", e))?;

    let engine = state.engine.read().await.as_ref().map(Arc::clone).ok_or("Not configured")?;
    engine
        .set_folder_strategy(
            &folder_id,
            strategy,
            local_path.as_deref().map(std::path::Path::new),
        )
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
    status: Arc<Mutex<SyncStatus>>,
    poll_interval_secs: u64,
) {
    async_runtime::spawn(async move {
        let mut interval =
            tokio::time::interval(Duration::from_secs(poll_interval_secs));
        loop {
            interval.tick().await;
            // Clone the Arc out of the lock and drop the guard immediately so we
            // never hold a lock guard across an await point.
            let maybe_engine = engine.read().await.as_ref().map(Arc::clone);
            if let Some(eng) = maybe_engine {
                *status.lock().await = SyncStatus::Syncing {
                    started_at: Utc::now().to_rfc3339(),
                };
                // Convert Box<dyn Error> (!Send) to String (Send) before any .await.
                let result = eng.incremental_sync().await.map_err(|e| e.to_string());
                match result {
                    Ok(_) => {
                        *status.lock().await = SyncStatus::Idle {
                            last_sync: Utc::now().to_rfc3339(),
                        };
                    }
                    Err(msg) => {
                        error!("Sync error: {}", msg);
                        *status.lock().await = SyncStatus::Error { message: msg };
                    }
                }
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
        status: Arc::new(Mutex::new(SyncStatus::default())),
        client: Arc::new(RwLock::new(None)),
    };

    // engine_arc / status_arc are only needed by the desktop tray menu handler.
    #[cfg(desktop)]
    let engine_arc = state.engine.clone();
    #[cfg(desktop)]
    let status_arc = state.status.clone();

    let mut builder = tauri::Builder::default();
    #[cfg(desktop)]
    { builder = builder.plugin(tauri_plugin_dialog::init()); }
    #[cfg(mobile)]
    { builder = builder.plugin(tauri_plugin_android_fs::init()); }

    builder
        .manage(state)
        .setup(move |app| {
            // Auto-login from persisted config if one exists.
            if let Some(cfg) = load_config_from(app.handle()) {
                let ds = app.state::<DesktopState>();
                let engine_arc = ds.engine.clone();
                let status_arc = ds.status.clone();
                let client_arc = ds.client.clone();
                let app_handle = app.handle().clone();
                async_runtime::spawn(async move {
                    let client = Arc::new(Client::new(&cfg.server_url));
                    let login_result = client.login(&cfg.username, &cfg.password).await.map_err(|e| e.to_string());
                    match login_result {
                        Ok(_) => {
                            let db = match sync_db_path(&app_handle) {
                                Some(p) => { let _ = p.parent().map(std::fs::create_dir_all); p }
                                None => { error!("Auto-login: cannot determine data directory"); return; }
                            };
                            let engine_result = SyncEngine::new(&db, client.clone(), PathBuf::from(&cfg.root_path)).await.map_err(|e| e.to_string());
                            match engine_result {
                                Ok(engine) => {
                                    *client_arc.write().await = Some(client);
                                    *engine_arc.write().await = Some(Arc::new(engine));
                                    *status_arc.lock().await = SyncStatus::Idle {
                                        last_sync: "Never".to_string(),
                                    };
                                    info!("Auto-login successful");
                                }
                                Err(e) => error!("Auto-login: engine init failed: {}", e),
                            }
                        }
                        Err(e) => error!("Auto-login: login failed: {}", e),
                    }
                });
            }

            // Desktop: system tray + menu
            #[cfg(desktop)]
            {
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
                            let engine = engine_arc.clone();
                            let status = status_arc.clone();
                            async_runtime::spawn(async move {
                                let maybe_engine = engine.read().await.as_ref().map(Arc::clone);
                                if let Some(eng) = maybe_engine {
                                    *status.lock().await = SyncStatus::Syncing {
                                        started_at: Utc::now().to_rfc3339(),
                                    };
                                    let result = eng.incremental_sync().await.map_err(|e| e.to_string());
                                    match result {
                                        Ok(_) => {
                                            *status.lock().await = SyncStatus::Idle {
                                                last_sync: Utc::now().to_rfc3339(),
                                            };
                                        }
                                        Err(msg) => {
                                            *status.lock().await = SyncStatus::Error { message: msg };
                                        }
                                    }
                                }
                            });
                        }
                        _ => {}
                    })
                    .build(app)?;
            }

            // Poll loop runs on both desktop and mobile.
            start_poll_loop(
                app.state::<DesktopState>().engine.clone(),
                app.state::<DesktopState>().status.clone(),
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
            set_folder_strategy,
            pick_folder,
            default_sync_folder,
        ])
        .build(tauri::generate_context!())
        .expect("error building Tauri application")
        .run(|_app, event| {
            // Keep alive when the last window closes (tray-only mode on desktop).
            // On mobile this event never fires in the same way.
            #[cfg(desktop)]
            if let tauri::RunEvent::ExitRequested { api, code, .. } = event {
                if code.is_none() {
                    api.prevent_exit();
                }
            }
            #[cfg(mobile)]
            let _ = event;
        });
}
