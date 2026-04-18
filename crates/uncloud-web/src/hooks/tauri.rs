use std::cell::Cell;

use js_sys::{Function, Object, Promise, Reflect};
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;

// ── Setup flag ────────────────────────────────────────────────────────────────

thread_local! {
    static NEEDS_SETUP: Cell<bool> = const { Cell::new(false) };
}

pub fn needs_setup() -> bool {
    NEEDS_SETUP.with(|c| c.get())
}

pub fn mark_needs_setup() {
    NEEDS_SETUP.with(|c| c.set(true));
}

pub fn mark_setup_complete() {
    NEEDS_SETUP.with(|c| c.set(false));
}

// ── Tauri detection ───────────────────────────────────────────────────────────

pub fn is_tauri() -> bool {
    web_sys::window()
        .and_then(|w| Reflect::get(&w, &JsValue::from_str("__TAURI__")).ok())
        .map(|v| !v.is_undefined() && !v.is_null())
        .unwrap_or(false)
}

/// Returns true when running inside the Android Tauri shell.
pub fn is_android() -> bool {
    web_sys::window()
        .and_then(|w| w.navigator().user_agent().ok())
        .map(|ua| ua.contains("Android"))
        .unwrap_or(false)
}

/// Push the current DaisyUI theme to the Android shell so the system bar
/// inlets (status bar, navigation bar) match the app background instead of
/// showing the OEM default colour. No-op on desktop / non-Android platforms.
/// Mirrors `MainActivity.AndroidBridge.setTheme` exposed as `window.UncloudAndroid`.
pub fn set_android_theme(dark: bool) {
    let Some(window) = web_sys::window() else { return };
    let Ok(bridge) = Reflect::get(&window, &JsValue::from_str("UncloudAndroid")) else { return };
    if bridge.is_undefined() || bridge.is_null() {
        return;
    }
    let Ok(method) = Reflect::get(&bridge, &JsValue::from_str("setTheme")) else { return };
    let Ok(func) = method.dyn_into::<Function>() else { return };
    let _ = func.call1(&bridge, &JsValue::from_bool(dark));
}

// ── DTOs ──────────────────────────────────────────────────────────────────────

pub struct DesktopConfig {
    pub server_url: String,
    pub username: String,
    pub root_path: String,
}

#[derive(Debug, Clone)]
pub enum SyncPhase {
    NotConfigured,
    Idle,
    Syncing,
    Error { message: String },
}

#[derive(Debug, Clone, Default)]
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

#[derive(Debug, Clone)]
pub struct SyncState {
    pub phase: SyncPhase,
    pub stats: SyncStats,
}

// ── Low-level invoke ──────────────────────────────────────────────────────────

async fn invoke_raw(cmd: &str, args: &JsValue) -> Result<JsValue, String> {
    let window = web_sys::window().ok_or("no window")?;
    let tauri = Reflect::get(&window, &JsValue::from_str("__TAURI__"))
        .map_err(|e| format!("{e:?}"))?;
    let core = Reflect::get(&tauri, &JsValue::from_str("core"))
        .map_err(|e| format!("{e:?}"))?;
    let invoke_fn: Function = Reflect::get(&core, &JsValue::from_str("invoke"))
        .map_err(|e| format!("{e:?}"))?
        .dyn_into()
        .map_err(|_| "invoke is not a function".to_string())?;

    let promise = Promise::from(
        invoke_fn
            .call2(&core, &JsValue::from_str(cmd), args)
            .map_err(|e| format!("{e:?}"))?,
    );

    JsFuture::from(promise)
        .await
        .map_err(|e| e.as_string().unwrap_or_else(|| format!("{e:?}")))
}

// ── Commands ──────────────────────────────────────────────────────────────────

pub async fn get_config() -> Option<DesktopConfig> {
    let args = Object::new();
    let result = invoke_raw("get_config", &args).await.ok()?;
    if result.is_null() || result.is_undefined() {
        return None;
    }
    let server_url = Reflect::get(&result, &JsValue::from_str("server_url"))
        .ok()?.as_string()?;
    let username = Reflect::get(&result, &JsValue::from_str("username"))
        .ok()?.as_string()?;
    let root_path = Reflect::get(&result, &JsValue::from_str("root_path"))
        .ok()?.as_string()?;
    Some(DesktopConfig { server_url, username, root_path })
}

/// Set up the sync engine via Tauri. Uses camelCase keys as Tauri 2 expects.
pub async fn login(server: &str, username: &str, password: &str, root_path: &str) -> Result<(), String> {
    let args = Object::new();
    let _ = Reflect::set(&args, &JsValue::from_str("server"), &JsValue::from_str(server));
    let _ = Reflect::set(&args, &JsValue::from_str("username"), &JsValue::from_str(username));
    let _ = Reflect::set(&args, &JsValue::from_str("password"), &JsValue::from_str(password));
    let _ = Reflect::set(&args, &JsValue::from_str("rootPath"), &JsValue::from_str(root_path));
    invoke_raw("login", &args).await.map(|_| ())
}

pub async fn disconnect() -> Result<(), String> {
    let args = Object::new();
    invoke_raw("disconnect", &args).await.map(|_| ())
}

pub async fn get_status() -> Result<SyncState, String> {
    let args = Object::new();
    let result = invoke_raw("get_status", &args).await?;
    let phase_tag = Reflect::get(&result, &JsValue::from_str("phase"))
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default();
    let phase = match phase_tag.as_str() {
        "idle" => SyncPhase::Idle,
        "syncing" => SyncPhase::Syncing,
        "error" => {
            let message = Reflect::get(&result, &JsValue::from_str("message"))
                .ok()
                .and_then(|v| v.as_string())
                .unwrap_or_default();
            SyncPhase::Error { message }
        }
        _ => SyncPhase::NotConfigured,
    };

    let stats_js = Reflect::get(&result, &JsValue::from_str("stats")).ok();
    let stats = stats_js
        .map(|s| {
            let num = |k: &str| -> u32 {
                Reflect::get(&s, &JsValue::from_str(k))
                    .ok()
                    .and_then(|v| v.as_f64())
                    .map(|f| f as u32)
                    .unwrap_or(0)
            };
            SyncStats {
                session_uploaded: num("session_uploaded"),
                session_downloaded: num("session_downloaded"),
                session_deleted: num("session_deleted"),
                session_errors: num("session_errors"),
                last_run_uploaded: num("last_run_uploaded"),
                last_run_downloaded: num("last_run_downloaded"),
                last_run_deleted: num("last_run_deleted"),
                last_run_errors: num("last_run_errors"),
                last_sync_at: Reflect::get(&s, &JsValue::from_str("last_sync_at"))
                    .ok()
                    .and_then(|v| v.as_string()),
            }
        })
        .unwrap_or_default();

    Ok(SyncState { phase, stats })
}

pub async fn sync_now() -> Result<(), String> {
    let args = Object::new();
    invoke_raw("sync_now", &args).await.map(|_| ())
}

/// Opens a native folder picker dialog. Returns the selected path or None if cancelled.
pub async fn pick_folder() -> Option<String> {
    let args = Object::new();
    let result = invoke_raw("pick_folder", &args).await.ok()?;
    if result.is_null() || result.is_undefined() {
        return None;
    }
    result.as_string()
}

/// Returns the platform-appropriate default sync folder (e.g. ~/Uncloud).
pub async fn default_sync_folder() -> Option<String> {
    let args = Object::new();
    let result = invoke_raw("default_sync_folder", &args).await.ok()?;
    result.as_string()
}

/// Per-folder effective sync config, resolved across all layers:
/// per-device override, server effective strategy, inherited local base path.
#[derive(Debug, Clone)]
pub struct FolderEffectiveConfig {
    pub client_strategy: Option<String>,
    pub effective_strategy: String,
    pub base_path: Option<String>,
    pub base_source: String,
    pub base_source_folder_id: Option<String>,
}

/// Fetch the resolved per-folder sync config from the desktop journal + server.
pub async fn get_folder_effective_config(folder_id: &str) -> Option<FolderEffectiveConfig> {
    let args = Object::new();
    let _ = Reflect::set(&args, &JsValue::from_str("folderId"), &JsValue::from_str(folder_id));
    let result = invoke_raw("get_folder_effective_config", &args).await.ok()?;
    if result.is_null() || result.is_undefined() {
        return None;
    }
    let client_strategy = Reflect::get(&result, &JsValue::from_str("client_strategy"))
        .ok()
        .and_then(|v| v.as_string());
    let effective_strategy = Reflect::get(&result, &JsValue::from_str("effective_strategy"))
        .ok()?
        .as_string()?;
    let base_path = Reflect::get(&result, &JsValue::from_str("base_path"))
        .ok()
        .and_then(|v| v.as_string());
    let base_source = Reflect::get(&result, &JsValue::from_str("base_source"))
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_else(|| "none".to_string());
    let base_source_folder_id = Reflect::get(&result, &JsValue::from_str("base_source_folder_id"))
        .ok()
        .and_then(|v| v.as_string());
    Some(FolderEffectiveConfig {
        client_strategy,
        effective_strategy,
        base_path,
        base_source,
        base_source_folder_id,
    })
}

/// Convert a stored local path (native filesystem path or Android SAF
/// `content://` URI) to a human-readable label for display.
pub fn display_local_path(raw: &str) -> String {
    // Native path (desktop) — show as-is.
    if !raw.starts_with("content://") {
        return raw.to_string();
    }
    // Android SAF tree URI:
    //   content://com.android.externalstorage.documents/tree/primary%3ADownload%2FFoo
    // Extract the last path segment after "/tree/" and URL-decode it.
    let Some(idx) = raw.find("/tree/") else { return raw.to_string() };
    let encoded = &raw[idx + 6..];
    // Strip anything after the next '/' — some URIs append a document part.
    let encoded = encoded.split('/').next().unwrap_or(encoded);
    let decoded = percent_decode(encoded);
    // "primary:Download/Foo" → "Internal storage/Download/Foo"
    if let Some((volume, rel)) = decoded.split_once(':') {
        let prefix = if volume == "primary" { "Internal storage" } else { volume };
        if rel.is_empty() {
            prefix.to_string()
        } else {
            format!("{prefix}/{rel}")
        }
    } else {
        decoded
    }
}

/// Minimal percent-decoder for ASCII. Handles `%XX` sequences, leaves
/// everything else untouched.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(b) = u8::from_str_radix(hex, 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// Write (or clear) the per-device strategy override for a folder.
/// Pass `None` to clear the override (use server default).
pub async fn set_folder_local_strategy(folder_id: &str, strategy: Option<&str>) -> Result<(), String> {
    let args = Object::new();
    let _ = Reflect::set(&args, &JsValue::from_str("folderId"), &JsValue::from_str(folder_id));
    match strategy {
        Some(s) => { let _ = Reflect::set(&args, &JsValue::from_str("strategy"), &JsValue::from_str(s)); }
        None => { let _ = Reflect::set(&args, &JsValue::from_str("strategy"), &JsValue::NULL); }
    }
    invoke_raw("set_folder_local_strategy", &args).await.map(|_| ())
}

/// Write (or clear) the per-device local path override for a folder.
/// Pass `None` to clear the override (inherit from ancestor or client root).
pub async fn set_folder_local_path(folder_id: &str, local_path: Option<&str>) -> Result<(), String> {
    let args = Object::new();
    let _ = Reflect::set(&args, &JsValue::from_str("folderId"), &JsValue::from_str(folder_id));
    match local_path {
        Some(p) => { let _ = Reflect::set(&args, &JsValue::from_str("localPath"), &JsValue::from_str(p)); }
        None => { let _ = Reflect::set(&args, &JsValue::from_str("localPath"), &JsValue::NULL); }
    }
    invoke_raw("set_folder_local_path", &args).await.map(|_| ())
}
