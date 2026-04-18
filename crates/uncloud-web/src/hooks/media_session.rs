//! Thin wrapper around `navigator.mediaSession` using `js_sys::Reflect`.
//!
//! Exposes enough of the Media Session API to drive the Android media
//! notification / lockscreen controls: title/artist/album/artwork metadata,
//! playback state ("playing" / "paused" / "none"), and per-action handlers
//! (play, pause, previoustrack, nexttrack). All functions are no-ops if the
//! browser doesn't expose `navigator.mediaSession`.

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

fn session() -> Option<JsValue> {
    let nav = web_sys::window()?.navigator();
    let s = js_sys::Reflect::get(&nav, &"mediaSession".into()).ok()?;
    if s.is_undefined() || s.is_null() { None } else { Some(s) }
}

pub fn set_playback_state(state: &str) {
    if let Some(s) = session() {
        let _ = js_sys::Reflect::set(&s, &"playbackState".into(), &JsValue::from_str(state));
    }
}

pub fn clear_metadata() {
    if let Some(s) = session() {
        let _ = js_sys::Reflect::set(&s, &"metadata".into(), &JsValue::NULL);
    }
}

pub fn set_metadata(title: &str, artist: &str, album: &str, artwork_url: Option<&str>) {
    let Some(s) = session() else { return; };
    let Some(win) = web_sys::window() else { return; };
    let Ok(ctor) = js_sys::Reflect::get(&win, &"MediaMetadata".into()) else { return; };
    if ctor.is_undefined() || ctor.is_null() { return; }
    let ctor: js_sys::Function = ctor.unchecked_into();

    let init = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&init, &"title".into(), &JsValue::from_str(title));
    let _ = js_sys::Reflect::set(&init, &"artist".into(), &JsValue::from_str(artist));
    if !album.is_empty() {
        let _ = js_sys::Reflect::set(&init, &"album".into(), &JsValue::from_str(album));
    }
    if let Some(url) = artwork_url {
        let art = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&art, &"src".into(), &JsValue::from_str(url));
        let _ = js_sys::Reflect::set(&art, &"sizes".into(), &JsValue::from_str("320x320"));
        let _ = js_sys::Reflect::set(&art, &"type".into(), &JsValue::from_str("image/jpeg"));
        let arr = js_sys::Array::new();
        arr.push(&art);
        let _ = js_sys::Reflect::set(&init, &"artwork".into(), &arr);
    }

    let args = js_sys::Array::new();
    args.push(&init);
    let Ok(meta) = js_sys::Reflect::construct(&ctor, &args) else { return; };
    let _ = js_sys::Reflect::set(&s, &"metadata".into(), &meta);
}

pub fn set_action_handler(action: &str, handler: &Closure<dyn FnMut()>) {
    let Some(s) = session() else { return; };
    let Ok(set_fn) = js_sys::Reflect::get(&s, &"setActionHandler".into()) else { return; };
    if set_fn.is_undefined() || set_fn.is_null() { return; }
    let set_fn: js_sys::Function = set_fn.unchecked_into();
    let _ = set_fn.call2(&s, &JsValue::from_str(action), handler.as_ref());
}

pub fn set_position_state(duration: f64, position: f64) {
    let Some(s) = session() else { return; };
    let Ok(set_fn) = js_sys::Reflect::get(&s, &"setPositionState".into()) else { return; };
    if set_fn.is_undefined() || set_fn.is_null() { return; }
    let set_fn: js_sys::Function = set_fn.unchecked_into();
    if !duration.is_finite() || duration <= 0.0 {
        return;
    }
    let state = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&state, &"duration".into(), &JsValue::from_f64(duration));
    let _ = js_sys::Reflect::set(&state, &"playbackRate".into(), &JsValue::from_f64(1.0));
    let _ = js_sys::Reflect::set(
        &state,
        &"position".into(),
        &JsValue::from_f64(position.clamp(0.0, duration)),
    );
    let _ = set_fn.call1(&s, &state);
}
