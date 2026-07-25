use gloo_net::http::Request;
use gloo_storage::{LocalStorage, Storage};
use js_sys::{Promise, Reflect};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::future_to_promise;
use web_sys::RequestCredentials;

use super::{api, tauri};
use uncloud_common::{
    DeviceInfo, DeviceTokenRequest, DeviceTokenResponse, LoginResponse, RefreshSessionRequest,
    RefreshSessionResponse,
};

const LAST_REFRESH_KEY: &str = "uncloud_auth_refreshed_at";
const PROACTIVE_REFRESH_INTERVAL_MS: f64 = 24.0 * 60.0 * 60.0 * 1000.0;

pub fn device_info() -> Option<DeviceInfo> {
    if !tauri::is_android() {
        return None;
    }
    let value = js_sys::eval(
        "(() => { try { return window.UncloudAndroid?.getDeviceInfo?.() || ''; } \
         catch (_) { return ''; } })()",
    )
    .ok()?
    .as_string()?;
    let (id, name) = value.split_once('\t')?;
    if id.is_empty() {
        return None;
    }
    Some(DeviceInfo {
        id: id.to_string(),
        name: if name.is_empty() {
            "Android device".to_string()
        } else {
            name.to_string()
        },
    })
}

pub fn store_refresh_token(token: &str) -> Result<(), String> {
    if !tauri::is_android() {
        return Ok(());
    }
    let server = serde_json::to_string(&api::api_base()).map_err(|error| error.to_string())?;
    let token = serde_json::to_string(token).map_err(|error| error.to_string())?;
    let stored = js_sys::eval(&format!(
        "(() => {{ try {{ return !!window.UncloudAndroid?.storeRefreshToken?.({server}, {token}); }} \
         catch (_) {{ return false; }} }})()"
    ))
    .ok()
    .and_then(|value| value.as_bool())
    .unwrap_or(false);
    if stored {
        Ok(())
    } else {
        Err("Android secure credential storage is unavailable".to_string())
    }
}

pub fn load_refresh_token() -> Option<String> {
    if !tauri::is_android() {
        return None;
    }
    let server = serde_json::to_string(&api::api_base()).ok()?;
    js_sys::eval(&format!(
        "(() => {{ try {{ return window.UncloudAndroid?.loadRefreshToken?.({server}) || ''; }} \
         catch (_) {{ return ''; }} }})()"
    ))
    .ok()
    .and_then(|value| value.as_string())
    .filter(|value| !value.is_empty())
}

pub fn clear_refresh_token() {
    if !tauri::is_android() {
        return;
    }
    if let Ok(server) = serde_json::to_string(&api::api_base()) {
        let _ = js_sys::eval(&format!(
            "try {{ window.UncloudAndroid?.clearRefreshToken?.({server}); }} catch (_) {{}}"
        ));
    }
    LocalStorage::delete(LAST_REFRESH_KEY);
}

pub fn persist_login_response(response: &LoginResponse) {
    if let Some(token) = response.refresh_token.as_deref() {
        if let Err(error) = store_refresh_token(token) {
            web_sys::console::warn_1(&format!("Could not store device credential: {error}").into());
        } else {
            mark_refreshed();
        }
    }
}

pub async fn prepare_android_auth() {
    if !tauri::is_android() {
        return;
    }
    if load_refresh_token().is_some() {
        if refresh_is_due() || api::auth_token().is_none() {
            let _ = refresh_session().await;
        }
    } else if api::auth_token().is_some() {
        let _ = create_device_token().await;
    }
}

pub async fn create_device_token() -> Result<(), String> {
    let device =
        device_info().ok_or_else(|| "Android device information unavailable".to_string())?;
    let response = api::post("/auth/device")
        .json(&DeviceTokenRequest { device })
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(response.text().await.unwrap_or_default());
    }
    let token = response
        .json::<DeviceTokenResponse>()
        .await
        .map_err(|error| error.to_string())?;
    store_refresh_token(&token.refresh_token)?;
    mark_refreshed();
    Ok(())
}

pub async fn refresh_session() -> Result<String, String> {
    let refresh_token =
        load_refresh_token().ok_or_else(|| "No Android device credential".to_string())?;
    let response = Request::post(&api::api_url("/auth/refresh"))
        .credentials(RequestCredentials::Include)
        .json(&RefreshSessionRequest { refresh_token })
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        if response.status() == 401 {
            clear_refresh_token();
        }
        return Err(response.text().await.unwrap_or_default());
    }
    let refreshed = response
        .json::<RefreshSessionResponse>()
        .await
        .map_err(|error| error.to_string())?;

    // Persist the rotated credential before replacing the access token. The
    // server accepts the previous value briefly, so an interrupted write can
    // recover on the next attempt instead of stranding the installation.
    store_refresh_token(&refreshed.refresh_token)?;
    api::seed_auth_token(refreshed.session_token.clone());
    mark_refreshed();
    Ok(refreshed.session_token)
}

pub fn install_refresh_bridge() {
    if !tauri::is_android() {
        return;
    }
    let callback = Closure::wrap(Box::new(move || {
        future_to_promise(async {
            refresh_session()
                .await
                .map(JsValue::from)
                .map_err(JsValue::from)
        })
    }) as Box<dyn Fn() -> Promise>);

    if let Some(window) = web_sys::window() {
        let _ = Reflect::set(
            &window,
            &JsValue::from_str("__uncloud_refresh_auth"),
            callback.as_ref(),
        );
    }
    callback.forget();
}

fn refresh_is_due() -> bool {
    let last = LocalStorage::get::<f64>(LAST_REFRESH_KEY).unwrap_or(0.0);
    js_sys::Date::now() - last >= PROACTIVE_REFRESH_INTERVAL_MS
}

fn mark_refreshed() {
    let _ = LocalStorage::set(LAST_REFRESH_KEY, js_sys::Date::now());
}
