//! Biometric vault unlock — thin Tauri-invoke wrapper for the
//! `uncloud-biometric` plugin.
//!
//! On non-Tauri builds (browser PWA) and on Tauri-desktop builds, every
//! command short-circuits: `status()` returns `available=false` and the
//! mutating commands return `Err("not_supported")`. Real implementation
//! is provided by `vendor/tauri-plugin-uncloud-biometric` on Android.
//!
//! Argument keys are camelCase (Tauri 2 convention); return values use
//! the same shape produced by `BiometricPlugin.kt`.
//! See `docs/biometric-unlock.md`.
//!
//! Runtime detection (Tauri + Android) is delegated to
//! `super::tauri::{is_tauri, is_android}` so the gating logic lives in
//! one place.

use js_sys::{Function, Object, Promise, Reflect};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use super::tauri::{is_android, is_tauri};

/// Reported by `status()`. `available` is the only field the UI reacts to;
/// `reason` exists for diagnostics ("none_enrolled" → user hasn't set up
/// any fingerprint, "no_hardware" → device has no sensor, etc.).
#[derive(Debug, Clone, Default)]
pub struct BiometricStatus {
    pub available: bool,
    pub reason: Option<String>,
}

fn supported() -> bool {
    is_tauri() && is_android()
}

async fn call(cmd: &str, payload: &JsValue) -> Result<JsValue, String> {
    let window = web_sys::window().ok_or("no window")?;
    let tauri = Reflect::get(&window, &"__TAURI__".into()).map_err(|e| format!("{e:?}"))?;
    if tauri.is_undefined() || tauri.is_null() {
        return Err("tauri runtime not present".into());
    }
    let core = Reflect::get(&tauri, &"core".into()).map_err(|e| format!("{e:?}"))?;
    let invoke_fn: Function = Reflect::get(&core, &"invoke".into())
        .map_err(|e| format!("{e:?}"))?
        .dyn_into()
        .map_err(|_| "invoke is not a function".to_string())?;

    let full_cmd = format!("plugin:uncloud-biometric|{cmd}");
    let promise = Promise::from(
        invoke_fn
            .call2(&core, &JsValue::from_str(&full_cmd), payload)
            .map_err(|e| format!("{e:?}"))?,
    );
    JsFuture::from(promise)
        .await
        .map_err(|e| e.as_string().unwrap_or_else(|| format!("{e:?}")))
}

fn vault_args(user_id: &str, vault_id: &str) -> Object {
    let args = Object::new();
    let _ = Reflect::set(&args, &"userId".into(), &JsValue::from_str(user_id));
    let _ = Reflect::set(&args, &"vaultId".into(), &JsValue::from_str(vault_id));
    args
}

pub async fn status() -> BiometricStatus {
    if !supported() {
        return BiometricStatus { available: false, reason: Some("not_supported".into()) };
    }
    match call("status", &Object::new().into()).await {
        Ok(v) => {
            let available = Reflect::get(&v, &"available".into())
                .ok()
                .and_then(|x| x.as_bool())
                .unwrap_or(false);
            let reason = Reflect::get(&v, &"reason".into())
                .ok()
                .and_then(|x| x.as_string());
            BiometricStatus { available, reason }
        }
        Err(e) => BiometricStatus { available: false, reason: Some(e) },
    }
}

pub async fn is_enrolled(user_id: &str, vault_id: &str) -> bool {
    if !supported() {
        return false;
    }
    match call("is_enrolled", &vault_args(user_id, vault_id).into()).await {
        Ok(v) => Reflect::get(&v, &"enrolled".into())
            .ok()
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
        Err(_) => false,
    }
}

pub async fn enroll(user_id: &str, vault_id: &str, secret: &str) -> Result<(), String> {
    if !supported() {
        return Err("not_supported".into());
    }
    let args = vault_args(user_id, vault_id);
    let _ = Reflect::set(&args, &"secret".into(), &JsValue::from_str(secret));
    let _ = Reflect::set(
        &args,
        &"reason".into(),
        &JsValue::from_str("Enable biometric unlock for this vault"),
    );
    call("enroll", &args.into()).await.map(|_| ())
}

pub async fn unlock(user_id: &str, vault_id: &str) -> Result<String, String> {
    if !supported() {
        return Err("not_supported".into());
    }
    let args = vault_args(user_id, vault_id);
    let _ = Reflect::set(
        &args,
        &"reason".into(),
        &JsValue::from_str("Unlock vault with fingerprint"),
    );
    let v = call("unlock", &args.into()).await?;
    Reflect::get(&v, &"secret".into())
        .ok()
        .and_then(|x| x.as_string())
        .ok_or_else(|| "missing secret in unlock response".to_string())
}

pub async fn clear(user_id: &str, vault_id: &str) -> Result<(), String> {
    if !supported() {
        return Ok(());
    }
    call("clear", &vault_args(user_id, vault_id).into())
        .await
        .map(|_| ())
}
