use std::cell::Cell;

use js_sys::{Array, Function, Object, Promise, Reflect};
use wasm_bindgen::closure::Closure;
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
///
/// Uses `js_sys::eval` rather than `Reflect::get` + `dyn_into::<Function>`
/// because Android WebView's `addJavascriptInterface` methods are host-bound
/// and don't satisfy wasm-bindgen's `Function` instanceof check, even though
/// they are callable from plain JS.
pub fn set_android_theme(dark: bool) {
    let code = format!(
        "try {{ if (window.UncloudAndroid && window.UncloudAndroid.setTheme) \
                  window.UncloudAndroid.setTheme({dark}); }} \
         catch (e) {{ console.error('UncloudAndroid.setTheme failed', e); }}"
    );
    let _ = js_sys::eval(&code);
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

/// A single row from the desktop's local `sync_log` SQLite table, pushed over
/// the `sync-log-appended` Tauri event and also returned by `get_local_sync_log`.
#[derive(Debug, Clone)]
pub struct SyncLogRow {
    pub id: i64,
    pub timestamp: String,
    pub operation: String,
    pub direction: Option<String>,
    pub resource_type: Option<String>,
    pub path: String,
    pub new_path: Option<String>,
    pub reason: String,
    pub note: Option<String>,
}

fn parse_sync_state(result: JsValue) -> SyncState {
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

    SyncState { phase, stats }
}

fn parse_sync_log_row(v: &JsValue) -> Option<SyncLogRow> {
    let get_str = |k: &str| -> Option<String> {
        Reflect::get(v, &JsValue::from_str(k))
            .ok()
            .and_then(|jv| jv.as_string())
    };
    let get_opt_str = |k: &str| -> Option<String> {
        Reflect::get(v, &JsValue::from_str(k))
            .ok()
            .and_then(|jv| if jv.is_null() || jv.is_undefined() { None } else { jv.as_string() })
    };
    Some(SyncLogRow {
        id: Reflect::get(v, &JsValue::from_str("id"))
            .ok()
            .and_then(|jv| jv.as_f64())
            .map(|f| f as i64)
            .unwrap_or(0),
        timestamp: get_str("timestamp")?,
        operation: get_str("operation")?,
        direction: get_opt_str("direction"),
        resource_type: get_opt_str("resource_type"),
        path: get_str("path")?,
        new_path: get_opt_str("new_path"),
        reason: get_str("reason")?,
        note: get_opt_str("note"),
    })
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

/// Result of [`get_auth_status`]. `pending=true` means the native auto-login is
/// still in flight; `pending=false` with `token=None` means it definitively
/// failed/skipped and the webview should clear its stored credentials.
pub struct AuthStatus {
    pub token: Option<String>,
    pub pending: bool,
}

/// Fetch the session token + auto-login status from the native side. Used at
/// app boot to seed the webview's bearer-token auth without re-prompting for
/// credentials — see `seed_auth_token` in `hooks::api`.
pub async fn get_auth_status() -> Option<AuthStatus> {
    let args = Object::new();
    let result = invoke_raw("get_auth_status", &args).await.ok()?;
    if result.is_null() || result.is_undefined() {
        return None;
    }
    let token = Reflect::get(&result, &JsValue::from_str("token"))
        .ok()
        .and_then(|v| if v.is_null() || v.is_undefined() { None } else { v.as_string() });
    let pending = Reflect::get(&result, &JsValue::from_str("pending"))
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Some(AuthStatus { token, pending })
}

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
    Ok(parse_sync_state(result))
}

pub async fn sync_now() -> Result<(), String> {
    let args = Object::new();
    invoke_raw("sync_now", &args).await.map(|_| ())
}

pub async fn get_autostart() -> Result<bool, String> {
    let args = Object::new();
    let result = invoke_raw("get_autostart", &args).await?;
    Ok(result.as_bool().unwrap_or(false))
}

pub async fn set_autostart(enabled: bool) -> Result<(), String> {
    let args = Object::new();
    let _ = Reflect::set(&args, &JsValue::from_str("enabled"), &JsValue::from_bool(enabled));
    invoke_raw("set_autostart", &args).await.map(|_| ())
}

/// Read the tail of the local sync_log from the desktop SQLite store. Called
/// once on mount; subsequent updates arrive via `sync-log-appended` events.
pub async fn get_local_sync_log(limit: i64) -> Result<Vec<SyncLogRow>, String> {
    let args = Object::new();
    let _ = Reflect::set(&args, &JsValue::from_str("limit"), &JsValue::from_f64(limit as f64));
    let result = invoke_raw("get_local_sync_log", &args).await?;
    let arr = Array::from(&result);
    let mut out = Vec::with_capacity(arr.length() as usize);
    for i in 0..arr.length() {
        if let Some(row) = parse_sync_log_row(&arr.get(i)) {
            out.push(row);
        }
    }
    Ok(out)
}

/// Subscribe to a Tauri event. Returns a handle that unsubscribes on drop
/// so the web frontend can wire these in `use_effect` without leaks. The
/// closure is kept alive inside the handle; dropping the handle awaits the
/// returned unlisten function and drops the closure.
pub struct TauriListener {
    _closure: Closure<dyn FnMut(JsValue)>,
    unlisten: Option<Function>,
}

impl Drop for TauriListener {
    fn drop(&mut self) {
        if let Some(f) = self.unlisten.take() {
            let _ = f.call0(&JsValue::NULL);
        }
    }
}

/// Attach a handler to a Tauri event. The returned [`TauriListener`] must be
/// kept alive for the duration of the subscription — when it drops, the
/// underlying JS unlisten is invoked.
pub async fn listen_event<F>(event: &str, mut handler: F) -> Option<TauriListener>
where
    F: FnMut(JsValue) + 'static,
{
    let window = web_sys::window()?;
    let tauri = Reflect::get(&window, &JsValue::from_str("__TAURI__")).ok()?;
    let event_mod = Reflect::get(&tauri, &JsValue::from_str("event")).ok()?;
    let listen: Function = Reflect::get(&event_mod, &JsValue::from_str("listen"))
        .ok()?
        .dyn_into()
        .ok()?;

    // Wrap the handler in a JS closure. Tauri's listen API invokes the
    // callback with a single arg shaped like `{ event, id, payload }`; we
    // unwrap `payload` before handing off to the caller so Rust code stays
    // simple.
    let closure = Closure::wrap(Box::new(move |e: JsValue| {
        let payload = Reflect::get(&e, &JsValue::from_str("payload"))
            .unwrap_or(JsValue::NULL);
        handler(payload);
    }) as Box<dyn FnMut(JsValue)>);

    let promise = Promise::from(
        listen
            .call2(
                &event_mod,
                &JsValue::from_str(event),
                closure.as_ref().unchecked_ref(),
            )
            .ok()?,
    );
    let unlisten = JsFuture::from(promise).await.ok()?;
    let unlisten: Function = unlisten.dyn_into().ok()?;

    Some(TauriListener {
        _closure: closure,
        unlisten: Some(unlisten),
    })
}

/// Convenience wrapper: listen for the desktop `sync-stats-changed` event and
/// invoke `handler` with the parsed [`SyncState`] each time.
pub async fn listen_sync_stats<F>(mut handler: F) -> Option<TauriListener>
where
    F: FnMut(SyncState) + 'static,
{
    listen_event("sync-stats-changed", move |payload| {
        handler(parse_sync_state(payload));
    })
    .await
}

/// Convenience wrapper: listen for the desktop `sync-log-appended` event and
/// invoke `handler` with the parsed [`SyncLogRow`] each time.
pub async fn listen_sync_log_appended<F>(mut handler: F) -> Option<TauriListener>
where
    F: FnMut(SyncLogRow) + 'static,
{
    listen_event("sync-log-appended", move |payload| {
        if let Some(row) = parse_sync_log_row(&payload) {
            handler(row);
        }
    })
    .await
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
