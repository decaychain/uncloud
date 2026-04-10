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

// ── DTOs ──────────────────────────────────────────────────────────────────────

pub struct DesktopConfig {
    pub server_url: String,
    pub username: String,
    pub root_path: String,
}

#[derive(Debug, Clone)]
pub enum SyncStatus {
    NotConfigured,
    Idle { last_sync: String },
    Syncing,
    Error { message: String },
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

pub async fn get_status() -> Result<SyncStatus, String> {
    let args = Object::new();
    let result = invoke_raw("get_status", &args).await?;
    let status_type = Reflect::get(&result, &JsValue::from_str("type"))
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default();
    Ok(match status_type.as_str() {
        "idle" => {
            let last_sync = Reflect::get(&result, &JsValue::from_str("last_sync"))
                .ok()
                .and_then(|v| v.as_string())
                .unwrap_or_default();
            SyncStatus::Idle { last_sync }
        }
        "syncing" => SyncStatus::Syncing,
        "error" => {
            let message = Reflect::get(&result, &JsValue::from_str("message"))
                .ok()
                .and_then(|v| v.as_string())
                .unwrap_or_default();
            SyncStatus::Error { message }
        }
        _ => SyncStatus::NotConfigured,
    })
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
