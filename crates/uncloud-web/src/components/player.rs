use dioxus::prelude::*;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::spawn_local;
use crate::components::icons::{
    IconMusic, IconPause, IconPlay, IconRepeat, IconRepeat1, IconShuffle, IconSkipBack,
    IconSkipForward, IconVolume2, IconX,
};
use crate::hooks::{api, media_session, native_audio, use_files::download_url};
use crate::state::{PlayerState, RepeatMode};

const AUDIO_ELEMENT_ID: &str = "uc-audio-player";

/// Call `audio.play()` and attach a `.catch()` handler that forwards any
/// rejection to `console.error`. Useful when debugging autoplay / AbortError
/// issues on Android WebView — visible via `adb logcat | grep Console`.
fn try_play(a: &web_sys::HtmlAudioElement) {
    let Ok(promise) = a.play() else { return };
    let err_cb = Closure::<dyn FnMut(JsValue)>::new(|err: JsValue| {
        web_sys::console::error_2(&"uncloud-player: play() rejected:".into(), &err);
    });
    let _ = promise.catch(&err_cb);
    err_cb.forget();
}

fn format_time(secs: f64) -> String {
    if secs.is_nan() || secs.is_infinite() {
        return "0:00".to_string();
    }
    let total = secs as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

fn get_audio_element() -> Option<web_sys::HtmlAudioElement> {
    web_sys::window()?
        .document()?
        .get_element_by_id(AUDIO_ELEMENT_ID)?
        .dyn_into::<web_sys::HtmlAudioElement>()
        .ok()
}

/// Switch to track at `idx` and start playback. In native mode we just update
/// the `PlayerState` signal and let the driving effect dispatch the native
/// `set_source`/`play` calls. In browser mode we additionally poke the audio
/// element inline so the user-gesture chain from the originating event
/// (click, onended, media-session action) is preserved for autoplay policy.
fn advance_to(
    idx: usize,
    mut player: Signal<PlayerState>,
    mut last_src: Signal<Option<String>>,
    mut current_secs: Signal<f64>,
    mut duration: Signal<f64>,
    native_mode: bool,
) {
    let s = player.peek().clone();
    let Some(track) = s.queue.get(idx).cloned() else { return; };
    if !native_mode {
        let next_src = download_url(&track.file.id);
        if let Some(a) = get_audio_element() {
            let _ = a.pause();
            a.set_src(&next_src);
            try_play(&a);
        }
        last_src.set(Some(next_src));
        current_secs.set(0.0);
        duration.set(0.0);
    }
    let mut w = player.write();
    w.current_index = idx;
    w.playing = true;
}

fn media_session_metadata_from(state: &PlayerState) {
    let Some(track) = state.current_track() else {
        media_session::clear_metadata();
        return;
    };
    let title = track
        .audio
        .title
        .clone()
        .unwrap_or_else(|| track.file.name.clone());
    let artist = track
        .audio
        .artist
        .clone()
        .unwrap_or_else(|| "Unknown".into());
    let album = track.audio.album.clone().unwrap_or_default();
    let artwork = if track.audio.has_cover_art {
        Some(api::authenticated_media_url(&format!("/files/{}/thumb", track.file.id)))
    } else {
        None
    };
    media_session::set_metadata(&title, &artist, &album, artwork.as_deref());
}

/// Hash the queue + current index as a simple signature so the native-mode
/// driver can tell "queue changed" (needs `set_queue`) apart from "user
/// clicked prev/next inside the same queue" (only needs `seek_to_item`).
fn queue_signature(state: &PlayerState) -> String {
    let mut s = String::with_capacity(state.queue.len() * 25);
    for track in &state.queue {
        s.push_str(&track.file.id);
        s.push('\u{1f}');
    }
    s
}

#[component]
pub fn Player() -> Element {
    let mut player = use_context::<Signal<PlayerState>>();
    let mut current_secs = use_signal(|| 0.0_f64);
    let mut duration = use_signal(|| 0.0_f64);
    let mut volume = use_signal(|| 1.0_f64);
    let mut last_src: Signal<Option<String>> = use_signal(|| None);
    // Native-mode only: signature of the queue currently loaded into
    // ExoPlayer. Compared against `queue_signature(player())` to decide
    // whether to dispatch `set_queue` vs `seek_to_item`.
    let mut last_queue_sig: Signal<Option<String>> = use_signal(|| None);
    let mut last_native_index: Signal<i32> = use_signal(|| -1);

    // True on Android (Tauri shell). Drives the native-audio plugin instead
    // of the HTML `<audio>` element so playback survives screen-off and the
    // system MediaSession notification appears.
    let native_mode = native_audio::is_available();

    let state = player();

    // ── One-time setup ──────────────────────────────────────────────────────
    use_hook(move || {
        if native_mode {
            // Initialize the plugin (creates ExoPlayer + MediaSession on Android).
            spawn_local(async move {
                let _ = native_audio::initialize().await;
            });
            return;
        }

        // Browser/desktop path: install JS MediaSession action handlers so
        // the native OS media keys / notifications still drive playback.
        let play_cb = Closure::<dyn FnMut()>::new(move || {
            if let Some(a) = get_audio_element() {
                try_play(&a);
            }
            player.write().playing = true;
        });
        media_session::set_action_handler("play", &play_cb);
        play_cb.forget();

        let pause_cb = Closure::<dyn FnMut()>::new(move || {
            if let Some(a) = get_audio_element() {
                let _ = a.pause();
            }
            player.write().playing = false;
        });
        media_session::set_action_handler("pause", &pause_cb);
        pause_cb.forget();

        let next_cb = Closure::<dyn FnMut()>::new(move || {
            let s = player.peek().clone();
            if let Some(idx) = s.next_index(false) {
                advance_to(idx, player, last_src, current_secs, duration, false);
            }
        });
        media_session::set_action_handler("nexttrack", &next_cb);
        next_cb.forget();

        let prev_cb = Closure::<dyn FnMut()>::new(move || {
            let s = player.peek().clone();
            if let Some(idx) = s.prev_index() {
                advance_to(idx, player, last_src, current_secs, duration, false);
            }
        });
        media_session::set_action_handler("previoustrack", &prev_cb);
        prev_cb.forget();
    });

    // ── Native-mode driver ───────────────────────────────────────────────────
    // Push the full queue to ExoPlayer as a native playlist so auto-advance
    // runs inside the foreground service even when the WebView is suspended
    // under Doze. A pure `current_index` change (user tapped prev/next while
    // the queue itself didn't change) is handled with `seek_to_item` so we
    // don't rebuild the whole playlist on navigation.
    use_effect(move || {
        if !native_mode {
            return;
        }
        let state = player();
        let sig = queue_signature(&state);

        if state.queue.is_empty() {
            if last_queue_sig.peek().is_some() {
                last_queue_sig.set(None);
                last_src.set(None);
                last_native_index.set(-1);
                spawn_local(async move {
                    let _ = native_audio::pause().await;
                });
            }
            return;
        }

        let queue_changed = last_queue_sig.peek().as_deref() != Some(sig.as_str());
        let start_index = state.current_index;
        let should_play = state.playing;

        if queue_changed {
            last_queue_sig.set(Some(sig.clone()));
            last_native_index.set(start_index as i32);
            current_secs.set(0.0);
            duration.set(0.0);
            last_src.set(
                state
                    .current_track()
                    .map(|t| download_url(&t.file.id)),
            );
            let items: Vec<native_audio::QueueItem> = state
                .queue
                .iter()
                .map(|t| {
                    let title = t
                        .audio
                        .title
                        .clone()
                        .unwrap_or_else(|| t.file.name.clone());
                    let artist = t
                        .audio
                        .artist
                        .clone()
                        .unwrap_or_else(|| "Unknown".into());
                    let artwork_url = if t.audio.has_cover_art {
                        Some(api::authenticated_media_url(&format!(
                            "/files/{}/thumb",
                            t.file.id
                        )))
                    } else {
                        None
                    };
                    native_audio::QueueItem {
                        src: download_url(&t.file.id),
                        title: Some(title),
                        artist: Some(artist),
                        artwork_url,
                    }
                })
                .collect();
            spawn_local(async move {
                let _ = native_audio::set_queue(&items, start_index, 0.0).await;
                if should_play {
                    let _ = native_audio::play().await;
                } else {
                    let _ = native_audio::pause().await;
                }
            });
            return;
        }

        // Same queue — detect index change (user clicked prev/next).
        let last_idx = *last_native_index.peek();
        if last_idx != start_index as i32 {
            last_native_index.set(start_index as i32);
            current_secs.set(0.0);
            duration.set(0.0);
            last_src.set(
                state
                    .current_track()
                    .map(|t| download_url(&t.file.id)),
            );
            spawn_local(async move {
                let _ = native_audio::seek_to_item(start_index).await;
                if should_play {
                    let _ = native_audio::play().await;
                }
            });
            return;
        }

        // Same queue, same item — just sync playing state.
        spawn_local(async move {
            if should_play {
                let _ = native_audio::play().await;
            } else {
                let _ = native_audio::pause().await;
            }
        });
    });

    // Mirror repeat-mode into ExoPlayer so auto-advance honours it during Doze.
    use_effect(move || {
        if !native_mode {
            return;
        }
        let mode = match player().repeat {
            RepeatMode::Off => "off",
            RepeatMode::One => "one",
            RepeatMode::All => "all",
        };
        spawn_local(async move {
            let _ = native_audio::set_repeat_mode(mode).await;
        });
    });

    // ── Browser-mode driver: sync the HTML audio element with state ──────────
    use_effect(move || {
        if native_mode {
            return;
        }
        let state = player();
        let Some(audio) = get_audio_element() else {
            return;
        };
        let desired = state.current_track().map(|t| download_url(&t.file.id));
        if desired != *last_src.peek() {
            if let Some(ref src) = desired {
                let _ = audio.pause();
                audio.set_src(src);
                current_secs.set(0.0);
                duration.set(0.0);
            }
            last_src.set(desired.clone());
        }
        if state.playing && desired.is_some() {
            try_play(&audio);
        } else {
            let _ = audio.pause();
        }
    });

    // ── Native-mode polling: mirror progress + index into UI ────────────────
    // Track advancement happens inside ExoPlayer now (see `set_queue` +
    // `set_repeat_mode`), so this loop only needs to reflect the native
    // player's state back into Dioxus. Crucially, under Doze the WebView
    // is suspended and this loop stops ticking — ExoPlayer keeps playing
    // through the queue regardless, and on screen-on we pick up whatever
    // index it ended up on.
    use_hook(move || {
        if !native_mode {
            return;
        }
        spawn_local(async move {
            loop {
                gloo_timers::future::sleep(std::time::Duration::from_millis(500)).await;
                let s = player.peek().clone();
                if s.queue.is_empty() {
                    continue;
                }
                let Ok(Some(st)) = native_audio::get_state().await else {
                    continue;
                };
                if st.current_time >= 0.0 {
                    current_secs.set(st.current_time);
                }
                if st.duration > 0.0 {
                    duration.set(st.duration);
                }
                // Mirror ExoPlayer's current index back into PlayerState when
                // it has drifted — typically because ExoPlayer auto-advanced
                // while we weren't polling (screen off / Doze). `last_native_index`
                // is updated *before* `player.current_index` so the driver
                // effect's change detection sees matching values and doesn't
                // redundantly dispatch `seek_to_item`.
                if st.current_index >= 0
                    && st.current_index != *last_native_index.peek()
                    && (st.current_index as usize) < s.queue.len()
                {
                    last_native_index.set(st.current_index);
                    let mut w = player.write();
                    w.current_index = st.current_index as usize;
                }
                // End-of-playlist with REPEAT_MODE_OFF: ExoPlayer stops after
                // the last track. One/All repeat modes are handled natively
                // by ExoPlayer so we never observe Ended in those cases.
                if st.status == native_audio::NativeStatus::Ended && s.playing {
                    player.write().playing = false;
                }
            }
        });
    });

    // ── Browser-mode MediaSession metadata mirroring ─────────────────────────
    use_effect(move || {
        if native_mode {
            return;
        }
        let state = player();
        if state.current_track().is_none() {
            media_session::clear_metadata();
            media_session::set_playback_state("none");
            return;
        }
        media_session_metadata_from(&state);
        media_session::set_playback_state(if state.playing { "playing" } else { "paused" });
    });

    let player_visible = !state.queue.is_empty();
    let track = state.current_track().cloned();

    let is_playing = state.playing;
    let has_prev = state.has_prev();
    let has_next = state.has_next();
    let shuffle_on = state.shuffle;
    let repeat_mode = state.repeat;

    let cur = current_secs();
    let dur = duration();
    let vol = volume();
    let cur_fmt = format_time(cur);
    let dur_fmt = format_time(dur);
    let seek_val = if dur > 0.0 { ((cur / dur) * 1000.0) as i64 } else { 0 };
    let vol_val = (vol * 100.0) as i64;

    let title = track
        .as_ref()
        .map(|t| {
            t.audio
                .title
                .clone()
                .unwrap_or_else(|| t.file.name.clone())
        })
        .unwrap_or_default();
    let artist = track
        .as_ref()
        .map(|t| t.audio.artist.clone().unwrap_or_else(|| "Unknown".into()))
        .unwrap_or_default();

    let cover_src = track.as_ref().and_then(|t| {
        if t.audio.has_cover_art {
            Some(api::authenticated_media_url(&format!(
                "/files/{}/thumb",
                t.file.id
            )))
        } else {
            None
        }
    });

    let shuffle_class = if shuffle_on {
        "btn btn-ghost btn-sm btn-circle text-primary"
    } else {
        "btn btn-ghost btn-sm btn-circle"
    };
    let repeat_class = if repeat_mode != RepeatMode::Off {
        "btn btn-ghost btn-sm btn-circle text-primary"
    } else {
        "btn btn-ghost btn-sm btn-circle"
    };

    rsx! {
        // Audio element — used only in browser/desktop mode. Kept at a stable
        // tree position so Dioxus never recreates it (which would detach
        // listeners and break auto-advance).
        audio {
            id: AUDIO_ELEMENT_ID,
            class: "hidden",
            preload: "auto",
            onloadedmetadata: move |_| {
                if let Some(a) = get_audio_element() {
                    let d = a.duration();
                    if !d.is_nan() && d > 0.0 {
                        duration.set(d);
                    }
                }
            },
            oncanplay: move |_| {
                if player.peek().playing {
                    if let Some(a) = get_audio_element() {
                        if a.paused() {
                            try_play(&a);
                        }
                    }
                }
            },
            onerror: move |_| {
                web_sys::console::error_1(&"uncloud-player: audio element error event".into());
            },
            onended: move |_| {
                let s = player.peek().clone();
                if s.repeat == RepeatMode::One {
                    if let Some(a) = get_audio_element() {
                        a.set_current_time(0.0);
                        try_play(&a);
                    }
                    return;
                }
                match s.next_index(true) {
                    Some(idx) => advance_to(idx, player, last_src, current_secs, duration, false),
                    None => {
                        player.write().playing = false;
                    }
                }
            },
            ontimeupdate: move |_| {
                if let Some(a) = get_audio_element() {
                    let t = a.current_time();
                    if !t.is_nan() {
                        current_secs.set(t);
                    }
                    let d = a.duration();
                    if !d.is_nan() && d > 0.0 {
                        duration.set(d);
                        media_session::set_position_state(d, t);
                    }
                }
            },
        }

        if player_visible {
            if let Some(track) = track {
                div { class: "fixed bottom-0 left-0 right-0 z-50 bg-base-200 border-t border-base-300 px-4 py-2 pb-safe",
                    div { class: "flex items-center gap-3 max-w-screen-xl mx-auto",
                        // Cover art
                        div { class: "flex-shrink-0 w-10 h-10",
                            if let Some(src) = &cover_src {
                                img {
                                    class: "w-10 h-10 rounded object-cover",
                                    src: "{src}",
                                }
                            } else {
                                div { class: "w-10 h-10 rounded bg-base-300 flex items-center justify-center",
                                    IconMusic { class: "w-5 h-5 text-base-content/50".to_string() }
                                }
                            }
                        }

                        // Track info — fills remaining space on mobile so the
                        // controls group and close button sit flush right.
                        div { class: "flex flex-col min-w-0 flex-1 sm:flex-none sm:w-40",
                            div { class: "text-sm font-medium truncate", "{title}" }
                            div { class: "text-xs text-base-content/60 truncate", "{artist}" }
                        }

                        // Controls
                        div { class: "flex items-center gap-1",
                            button {
                                class: "{shuffle_class} hidden sm:inline-flex",
                                title: if shuffle_on { "Shuffle: on" } else { "Shuffle: off" },
                                onclick: move |_| {
                                    player.write().toggle_shuffle();
                                },
                                IconShuffle { class: "w-4 h-4".to_string() }
                            }
                            button {
                                class: "btn btn-ghost btn-sm btn-circle",
                                disabled: !has_prev && cur <= 3.0,
                                onclick: move |_| {
                                    if !native_mode {
                                        if let Some(a) = get_audio_element() {
                                            if a.current_time() > 3.0 {
                                                a.set_current_time(0.0);
                                                return;
                                            }
                                        }
                                    } else if cur > 3.0 {
                                        spawn_local(async move {
                                            let _ = native_audio::seek_to(0.0).await;
                                        });
                                        current_secs.set(0.0);
                                        return;
                                    }
                                    let s = player.peek().clone();
                                    if let Some(idx) = s.prev_index() {
                                        advance_to(idx, player, last_src, current_secs, duration, native_mode);
                                    }
                                },
                                IconSkipBack { class: "w-5 h-5".to_string() }
                            }
                            button {
                                class: "btn btn-ghost btn-sm btn-circle",
                                onclick: move |_| {
                                    player.write().playing = !is_playing;
                                },
                                if is_playing {
                                    IconPause { class: "w-6 h-6".to_string() }
                                } else {
                                    IconPlay { class: "w-6 h-6".to_string() }
                                }
                            }
                            button {
                                class: "btn btn-ghost btn-sm btn-circle",
                                disabled: !has_next,
                                onclick: move |_| {
                                    let s = player.peek().clone();
                                    if let Some(idx) = s.next_index(false) {
                                        advance_to(idx, player, last_src, current_secs, duration, native_mode);
                                    }
                                },
                                IconSkipForward { class: "w-5 h-5".to_string() }
                            }
                            button {
                                class: "{repeat_class} hidden sm:inline-flex",
                                title: match repeat_mode {
                                    RepeatMode::Off => "Repeat: off",
                                    RepeatMode::All => "Repeat: all",
                                    RepeatMode::One => "Repeat: one",
                                },
                                onclick: move |_| {
                                    let next = player.peek().repeat.cycle();
                                    player.write().repeat = next;
                                },
                                if repeat_mode == RepeatMode::One {
                                    IconRepeat1 { class: "w-4 h-4".to_string() }
                                } else {
                                    IconRepeat { class: "w-4 h-4".to_string() }
                                }
                            }
                        }

                        // Seek bar + times
                        div { class: "hidden sm:flex items-center gap-2 flex-1 min-w-0",
                            span { class: "text-xs text-base-content/50 tabular-nums w-10 text-right", "{cur_fmt}" }
                            input {
                                r#type: "range",
                                class: "range range-primary range-xs flex-1",
                                min: "0",
                                max: "1000",
                                value: "{seek_val}",
                                oninput: move |evt: Event<FormData>| {
                                    if let Ok(v) = evt.value().parse::<f64>() {
                                        let target_time = (v / 1000.0) * dur;
                                        if native_mode {
                                            spawn_local(async move {
                                                let _ = native_audio::seek_to(target_time).await;
                                            });
                                        } else if let Some(audio) = get_audio_element() {
                                            audio.set_current_time(target_time);
                                        }
                                        current_secs.set(target_time);
                                    }
                                },
                            }
                            span { class: "text-xs text-base-content/50 tabular-nums w-10", "{dur_fmt}" }
                        }

                        // Volume (desktop only)
                        div { class: "hidden md:flex items-center gap-1 w-28",
                            IconVolume2 { class: "w-4 h-4 text-base-content/60".to_string() }
                            input {
                                r#type: "range",
                                class: "range range-xs flex-1",
                                min: "0",
                                max: "100",
                                value: "{vol_val}",
                                oninput: move |evt: Event<FormData>| {
                                    if let Ok(v) = evt.value().parse::<f64>() {
                                        let new_vol = v / 100.0;
                                        volume.set(new_vol);
                                        if let Some(audio) = get_audio_element() {
                                            audio.set_volume(new_vol);
                                        }
                                    }
                                },
                            }
                        }

                        // Close button
                        button {
                            class: "btn btn-ghost btn-sm btn-circle flex-shrink-0",
                            title: "Close player",
                            onclick: move |_| {
                                if native_mode {
                                    spawn_local(async move {
                                        let _ = native_audio::pause().await;
                                    });
                                } else if let Some(audio) = get_audio_element() {
                                    let _ = audio.pause();
                                    audio.set_src("");
                                }
                                let mut w = player.write();
                                w.queue.clear();
                                w.current_index = 0;
                                w.playing = false;
                                w.original_queue = None;
                            },
                            IconX { class: "w-4 h-4".to_string() }
                        }
                    }
                    // Mobile-only secondary row: shuffle + scrub bar + repeat.
                    div { class: "flex sm:hidden items-center gap-2 max-w-screen-xl mx-auto mt-2",
                        button {
                            class: "{shuffle_class}",
                            title: if shuffle_on { "Shuffle: on" } else { "Shuffle: off" },
                            onclick: move |_| {
                                player.write().toggle_shuffle();
                            },
                            IconShuffle { class: "w-4 h-4".to_string() }
                        }
                        span { class: "text-[10px] text-base-content/50 tabular-nums w-9 text-right", "{cur_fmt}" }
                        input {
                            r#type: "range",
                            class: "range range-primary range-xs flex-1",
                            min: "0",
                            max: "1000",
                            value: "{seek_val}",
                            oninput: move |evt: Event<FormData>| {
                                if let Ok(v) = evt.value().parse::<f64>() {
                                    let target_time = (v / 1000.0) * dur;
                                    if native_mode {
                                        spawn_local(async move {
                                            let _ = native_audio::seek_to(target_time).await;
                                        });
                                    } else if let Some(audio) = get_audio_element() {
                                        audio.set_current_time(target_time);
                                    }
                                    current_secs.set(target_time);
                                }
                            },
                        }
                        span { class: "text-[10px] text-base-content/50 tabular-nums w-9", "{dur_fmt}" }
                        button {
                            class: "{repeat_class}",
                            title: match repeat_mode {
                                RepeatMode::Off => "Repeat: off",
                                RepeatMode::All => "Repeat: all",
                                RepeatMode::One => "Repeat: one",
                            },
                            onclick: move |_| {
                                let next = player.peek().repeat.cycle();
                                player.write().repeat = next;
                            },
                            if repeat_mode == RepeatMode::One {
                                IconRepeat1 { class: "w-4 h-4".to_string() }
                            } else {
                                IconRepeat { class: "w-4 h-4".to_string() }
                            }
                        }
                    }
                }
            }
        }
    }
}
