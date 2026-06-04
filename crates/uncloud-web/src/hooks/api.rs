use std::cell::RefCell;

use gloo_net::http::{Request, RequestBuilder};
use gloo_storage::{LocalStorage, Storage};
use web_sys::RequestCredentials;

thread_local! {
    static CACHED_BASE: RefCell<Option<String>> = const { RefCell::new(None) };
    static AUTH_TOKEN: RefCell<Option<String>> = const { RefCell::new(None) };
    /// The query string the SPA was loaded with, captured before
    /// `dioxus::launch` because the router strips it during normalisation.
    /// Read with `initial_search()`.
    static INITIAL_SEARCH: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Capture `window.location.search` before the Dioxus router takes over.
/// Must be called once at the top of `main` (before `dioxus::launch`).
pub fn snapshot_initial_url() {
    let search = web_sys::window()
        .and_then(|w| w.location().search().ok())
        .unwrap_or_default();
    INITIAL_SEARCH.with(|cell| *cell.borrow_mut() = search);
}

/// The query string the SPA was loaded with (including the leading `?`,
/// or empty). Survives router normalisation.
pub fn initial_search() -> String {
    INITIAL_SEARCH.with(|cell| cell.borrow().clone())
}

const LS_API_BASE: &str = "uncloud_api_base";
const LS_AUTH_TOKEN: &str = "uncloud_auth_token";

/// Seed the API base URL before Dioxus launches.
///
/// In Tauri mode, `main()` calls this with the server URL from `get_config`.
/// In browser mode, it is never called; `api_base()` returns `""` (relative URLs).
pub fn seed_api_base(url: String) {
    let url = url.trim_end_matches('/').to_string();
    let url = if url.starts_with("http") {
        url
    } else {
        String::new()
    };
    let _ = LocalStorage::set(LS_API_BASE, &url);
    CACHED_BASE.with(|cell| *cell.borrow_mut() = Some(url));
}

/// Returns the cached API base URL. Empty string means same-origin (browser mode).
pub fn api_base() -> String {
    CACHED_BASE.with(|cell| cell.borrow().clone().unwrap_or_default())
}

pub fn api_url(path: &str) -> String {
    format!("{}/api{}", api_base().trim_end_matches('/'), path)
}

pub fn api_v1_url(path: &str) -> String {
    format!("{}/api/v1{}", api_base().trim_end_matches('/'), path)
}

/// Store a Bearer token obtained at login time.
///
/// When set, all request helpers (`get`, `post`, etc.) will attach an
/// `Authorization: Bearer <token>` header in addition to `credentials: include`.
pub fn seed_auth_token(token: String) {
    let _ = LocalStorage::set(LS_AUTH_TOKEN, &token);
    AUTH_TOKEN.with(|cell| *cell.borrow_mut() = Some(token));
}

/// Returns the stored auth token, if any.
pub fn auth_token() -> Option<String> {
    AUTH_TOKEN.with(|cell| cell.borrow().clone())
}

/// Clear the stored auth token (e.g. on logout).
pub fn clear_auth_token() {
    LocalStorage::delete(LS_AUTH_TOKEN);
    AUTH_TOKEN.with(|cell| *cell.borrow_mut() = None);
}

/// Clear all persisted session data (server URL + auth token).
/// Used on disconnect / full logout to return to the setup screen.
#[allow(dead_code)]
pub fn clear_stored_session() {
    LocalStorage::delete(LS_API_BASE);
    LocalStorage::delete(LS_AUTH_TOKEN);
    CACHED_BASE.with(|cell| *cell.borrow_mut() = None);
    AUTH_TOKEN.with(|cell| *cell.borrow_mut() = None);
}

/// Restore API base and auth token from localStorage into thread-local cache.
/// Returns true if both were restored (app can skip setup).
pub fn restore_from_storage() -> bool {
    let base: Option<String> = LocalStorage::get(LS_API_BASE).ok();
    let token: Option<String> = LocalStorage::get(LS_AUTH_TOKEN).ok();
    if let Some(url) = &base {
        if !url.is_empty() {
            CACHED_BASE.with(|cell| *cell.borrow_mut() = Some(url.clone()));
        }
    }
    if let Some(t) = &token {
        AUTH_TOKEN.with(|cell| *cell.borrow_mut() = Some(t.clone()));
    }
    base.filter(|u| !u.is_empty()).is_some() && token.is_some()
}

/// Build an API URL with the auth token as a `?token=` query parameter.
///
/// Use for URLs set as `src` on `<img>`, `<audio>`, or `href` on `<a>` — these
/// elements cannot send `Authorization` headers. The server's auth middleware
/// already accepts `?token=` for this purpose (also used by SSE/EventSource).
///
/// When no token is stored (browser/cookie mode), returns the plain URL.
pub fn authenticated_media_url(path: &str) -> String {
    let base = api_url(path);
    match auth_token() {
        Some(token) => {
            let sep = if base.contains('?') { '&' } else { '?' };
            format!("{}{sep}token={}", base, token)
        }
        None => base,
    }
}

pub fn open_external_file(path: &str, filename: &str, mime_type: &str) {
    let url = authenticated_media_url(path);
    if crate::hooks::tauri::open_android_file(&url, filename, mime_type) {
        return;
    }
    if crate::hooks::tauri::open_desktop_file(path, filename) {
        return;
    }
    let _ = web_sys::window()
        .and_then(|w| w.open_with_url(&url).ok())
        .flatten();
}

pub fn download_external_file_native(path: &str, filename: &str, mime_type: &str) -> bool {
    let url = authenticated_media_url(path);
    if crate::hooks::tauri::download_android_file(&url, filename, mime_type) {
        return true;
    }
    crate::hooks::tauri::download_desktop_file(path, filename)
}

pub fn download_external_file(path: &str, filename: &str, mime_type: &str) {
    if download_external_file_native(path, filename, mime_type) {
        return;
    }
    let url = authenticated_media_url(path);
    let _ = web_sys::window()
        .and_then(|w| w.open_with_url(&url).ok())
        .flatten();
}

// ---------------------------------------------------------------------------
// Request builder helpers
//
// Each helper creates a gloo_net Request with `credentials: include` (for
// cookie auth in browser mode) and, when an auth token is stored, also sets
// the `Authorization: Bearer <token>` header (for Tauri / Android mode).
// ---------------------------------------------------------------------------

fn apply_auth(req: RequestBuilder) -> RequestBuilder {
    if let Some(token) = auth_token() {
        req.header("Authorization", &format!("Bearer {}", token))
    } else {
        req
    }
}

pub fn get(path: &str) -> RequestBuilder {
    apply_auth(Request::get(&api_url(path)).credentials(RequestCredentials::Include))
}

pub fn post(path: &str) -> RequestBuilder {
    apply_auth(Request::post(&api_url(path)).credentials(RequestCredentials::Include))
}

pub fn put(path: &str) -> RequestBuilder {
    apply_auth(Request::put(&api_url(path)).credentials(RequestCredentials::Include))
}

pub fn delete(path: &str) -> RequestBuilder {
    apply_auth(Request::delete(&api_url(path)).credentials(RequestCredentials::Include))
}

pub fn patch(path: &str) -> RequestBuilder {
    apply_auth(Request::patch(&api_url(path)).credentials(RequestCredentials::Include))
}

// v1 variants

pub fn get_v1(path: &str) -> RequestBuilder {
    apply_auth(Request::get(&api_v1_url(path)).credentials(RequestCredentials::Include))
}

#[allow(dead_code)]
pub fn post_v1(path: &str) -> RequestBuilder {
    apply_auth(Request::post(&api_v1_url(path)).credentials(RequestCredentials::Include))
}

pub fn put_v1(path: &str) -> RequestBuilder {
    apply_auth(Request::put(&api_v1_url(path)).credentials(RequestCredentials::Include))
}

#[allow(dead_code)]
pub fn delete_v1(path: &str) -> RequestBuilder {
    apply_auth(Request::delete(&api_v1_url(path)).credentials(RequestCredentials::Include))
}

// ---------------------------------------------------------------------------
// Raw-URL variant — for cases where the caller has already built the full URL
// (e.g. with query parameters appended to `api_url(…)`).
// ---------------------------------------------------------------------------

pub fn get_raw(url: &str) -> RequestBuilder {
    apply_auth(Request::get(url).credentials(RequestCredentials::Include))
}

pub fn post_raw(url: &str) -> RequestBuilder {
    apply_auth(Request::post(url).credentials(RequestCredentials::Include))
}

#[allow(dead_code)]
pub fn put_raw(url: &str) -> RequestBuilder {
    apply_auth(Request::put(url).credentials(RequestCredentials::Include))
}

#[allow(dead_code)]
pub fn delete_raw(url: &str) -> RequestBuilder {
    apply_auth(Request::delete(url).credentials(RequestCredentials::Include))
}

#[allow(dead_code)]
pub fn patch_raw(url: &str) -> RequestBuilder {
    apply_auth(Request::patch(url).credentials(RequestCredentials::Include))
}
