//! Bridge to `tauri-plugin-native-audio` for Android/iOS playback.
//!
//! On mobile the HTML `<audio>` element can't keep playing when the screen
//! turns off or the app is backgrounded: the WebView gets suspended and
//! `onended` never fires, so the queue stops advancing. This module routes
//! playback through the native plugin instead, which runs a foreground
//! service + MediaSessionService on Android (Media3 ExoPlayer) and shows
//! proper lockscreen / notification controls.
//!
//! Only the commands we actually need are exposed. Events are not subscribed
//! through the plugin's Channel mechanism; the player polls `get_state()`
//! while a track is playing, which is enough to mirror current_time into
//! the UI and detect the `ended` status to auto-advance.

use js_sys::{Function, Object, Promise, Reflect};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use super::tauri::{is_android, is_tauri};

/// True when the native plugin is available (Tauri shell + Android/iOS).
pub fn is_available() -> bool {
    is_tauri() && is_android()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeStatus {
    Idle,
    Loading,
    Playing,
    Ended,
    Error,
}

impl NativeStatus {
    fn parse(s: &str) -> Self {
        match s {
            "loading" => NativeStatus::Loading,
            "playing" => NativeStatus::Playing,
            "ended" => NativeStatus::Ended,
            "error" => NativeStatus::Error,
            _ => NativeStatus::Idle,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NativeAudioState {
    pub status: NativeStatus,
    pub current_time: f64,
    pub duration: f64,
    pub is_playing: bool,
    pub buffering: bool,
    pub rate: f64,
    /// Index of the currently-playing item in the native ExoPlayer playlist.
    /// `-1` if no queue has been set. When the player auto-advances under
    /// Doze (WebView suspended), this is how the WASM side discovers that
    /// the current track has changed — compare against the last-seen value
    /// and update Rust-side `PlayerState.current_index` on polling resume.
    pub current_index: i32,
    pub queue_length: i32,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct QueueItem {
    pub src: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub artwork_url: Option<String>,
}

impl NativeAudioState {
    fn from_js(v: &JsValue) -> Option<Self> {
        if v.is_null() || v.is_undefined() {
            return None;
        }
        let status = Reflect::get(v, &"status".into())
            .ok()
            .and_then(|s| s.as_string())
            .map(|s| NativeStatus::parse(&s))
            .unwrap_or(NativeStatus::Idle);
        let num = |k: &str| -> f64 {
            Reflect::get(v, &JsValue::from_str(k))
                .ok()
                .and_then(|n| n.as_f64())
                .unwrap_or(0.0)
        };
        let boolean = |k: &str| -> bool {
            Reflect::get(v, &JsValue::from_str(k))
                .ok()
                .and_then(|b| b.as_bool())
                .unwrap_or(false)
        };
        let error = Reflect::get(v, &"error".into())
            .ok()
            .and_then(|e| e.as_string());
        let int = |k: &str| -> i32 {
            Reflect::get(v, &JsValue::from_str(k))
                .ok()
                .and_then(|n| n.as_f64())
                .map(|n| n as i32)
                .unwrap_or(-1)
        };
        let queue_len = Reflect::get(v, &"queueLength".into())
            .ok()
            .and_then(|n| n.as_f64())
            .map(|n| n as i32)
            .unwrap_or(0);
        Some(NativeAudioState {
            status,
            current_time: num("currentTime"),
            duration: num("duration"),
            is_playing: boolean("isPlaying"),
            buffering: boolean("buffering"),
            rate: num("rate").max(1.0),
            current_index: int("currentIndex"),
            queue_length: queue_len,
            error,
        })
    }
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

    let full_cmd = format!("plugin:native-audio|{cmd}");
    let promise = Promise::from(
        invoke_fn
            .call2(&core, &JsValue::from_str(&full_cmd), payload)
            .map_err(|e| format!("{e:?}"))?,
    );
    JsFuture::from(promise)
        .await
        .map_err(|e| e.as_string().unwrap_or_else(|| format!("{e:?}")))
}

pub async fn initialize() -> Result<Option<NativeAudioState>, String> {
    let v = call("initialize", &Object::new().into()).await?;
    Ok(NativeAudioState::from_js(&v))
}

pub async fn set_source(
    src: &str,
    title: Option<&str>,
    artist: Option<&str>,
    artwork_url: Option<&str>,
) -> Result<Option<NativeAudioState>, String> {
    let args = Object::new();
    let _ = Reflect::set(&args, &"src".into(), &JsValue::from_str(src));
    if let Some(t) = title {
        let _ = Reflect::set(&args, &"title".into(), &JsValue::from_str(t));
    }
    if let Some(a) = artist {
        let _ = Reflect::set(&args, &"artist".into(), &JsValue::from_str(a));
    }
    if let Some(u) = artwork_url {
        let _ = Reflect::set(&args, &"artworkUrl".into(), &JsValue::from_str(u));
    }
    let v = call("set_source", &args.into()).await?;
    Ok(NativeAudioState::from_js(&v))
}

/// Replace the native ExoPlayer playlist with `items` and start at `start_index`.
/// After this call, ExoPlayer advances through the queue natively — no WASM
/// polling is required for track transitions, which is what makes the player
/// survive Doze / screen-off suspension of the WebView.
pub async fn set_queue(
    items: &[QueueItem],
    start_index: usize,
    start_position: f64,
) -> Result<Option<NativeAudioState>, String> {
    let arr = js_sys::Array::new();
    for item in items {
        let obj = Object::new();
        let _ = Reflect::set(&obj, &"src".into(), &JsValue::from_str(&item.src));
        if let Some(t) = &item.title {
            let _ = Reflect::set(&obj, &"title".into(), &JsValue::from_str(t));
        }
        if let Some(a) = &item.artist {
            let _ = Reflect::set(&obj, &"artist".into(), &JsValue::from_str(a));
        }
        if let Some(u) = &item.artwork_url {
            let _ = Reflect::set(&obj, &"artworkUrl".into(), &JsValue::from_str(u));
        }
        arr.push(&obj);
    }
    let args = Object::new();
    let _ = Reflect::set(&args, &"items".into(), &arr);
    let _ = Reflect::set(
        &args,
        &"startIndex".into(),
        &JsValue::from_f64(start_index as f64),
    );
    let _ = Reflect::set(
        &args,
        &"startPosition".into(),
        &JsValue::from_f64(start_position),
    );
    let v = call("set_queue", &args.into()).await?;
    Ok(NativeAudioState::from_js(&v))
}

pub async fn seek_to_item(index: usize) -> Result<Option<NativeAudioState>, String> {
    let args = Object::new();
    let _ = Reflect::set(&args, &"index".into(), &JsValue::from_f64(index as f64));
    let v = call("seek_to_item", &args.into()).await?;
    Ok(NativeAudioState::from_js(&v))
}

/// `mode` is `"off"`, `"one"`, or `"all"` — maps to Media3's ExoPlayer repeat
/// modes so auto-advance under Doze honours the user's repeat choice without
/// requiring the WASM polling loop to intervene.
pub async fn set_repeat_mode(mode: &str) -> Result<Option<NativeAudioState>, String> {
    let args = Object::new();
    let _ = Reflect::set(&args, &"mode".into(), &JsValue::from_str(mode));
    let v = call("set_repeat_mode", &args.into()).await?;
    Ok(NativeAudioState::from_js(&v))
}

pub async fn play() -> Result<Option<NativeAudioState>, String> {
    let v = call("play", &Object::new().into()).await?;
    Ok(NativeAudioState::from_js(&v))
}

pub async fn pause() -> Result<Option<NativeAudioState>, String> {
    let v = call("pause", &Object::new().into()).await?;
    Ok(NativeAudioState::from_js(&v))
}

pub async fn seek_to(position: f64) -> Result<Option<NativeAudioState>, String> {
    let args = Object::new();
    let _ = Reflect::set(&args, &"position".into(), &JsValue::from_f64(position));
    let v = call("seek_to", &args.into()).await?;
    Ok(NativeAudioState::from_js(&v))
}

pub async fn get_state() -> Result<Option<NativeAudioState>, String> {
    let v = call("get_state", &Object::new().into()).await?;
    Ok(NativeAudioState::from_js(&v))
}

pub async fn dispose() -> Result<(), String> {
    call("dispose", &Object::new().into()).await.map(|_| ())
}
