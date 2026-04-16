use dioxus::prelude::*;
use wasm_bindgen::JsCast;
use crate::components::icons::{
    IconMusic, IconPause, IconPlay, IconRepeat, IconRepeat1, IconShuffle, IconSkipBack,
    IconSkipForward, IconVolume2, IconX,
};
use crate::hooks::{api, use_files::download_url};
use crate::state::{PlayerState, RepeatMode};

const AUDIO_ELEMENT_ID: &str = "uc-audio-player";

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

#[component]
pub fn Player() -> Element {
    let mut player = use_context::<Signal<PlayerState>>();
    let mut current_secs = use_signal(|| 0.0_f64);
    let mut duration = use_signal(|| 0.0_f64);
    let mut volume = use_signal(|| 1.0_f64);
    let mut last_src: Signal<Option<String>> = use_signal(|| None);

    let state = player();

    // Sync audio element src + play/pause with player state.
    use_effect(move || {
        let state = player();
        let Some(audio) = get_audio_element() else {
            return;
        };

        let desired = state.current_track().map(|t| download_url(&t.file.id));

        if desired != *last_src.peek() {
            if let Some(ref src) = desired {
                audio.set_src(src);
                let _ = audio.load();
                current_secs.set(0.0);
                duration.set(0.0);
            }
            last_src.set(desired.clone());
        }

        if state.playing && desired.is_some() {
            let _ = audio.play();
        } else {
            let _ = audio.pause();
        }
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
        // Audio element — kept at a stable tree position so Dioxus never
        // recreates it (which would detach listeners and break auto-advance).
        audio {
            id: AUDIO_ELEMENT_ID,
            class: "hidden",
            preload: "auto",
            ontimeupdate: move |_| {
                if let Some(a) = get_audio_element() {
                    let t = a.current_time();
                    if !t.is_nan() {
                        current_secs.set(t);
                    }
                    let d = a.duration();
                    if !d.is_nan() && d > 0.0 {
                        duration.set(d);
                    }
                }
            },
            onloadedmetadata: move |_| {
                if let Some(a) = get_audio_element() {
                    let d = a.duration();
                    if !d.is_nan() && d > 0.0 {
                        duration.set(d);
                    }
                }
            },
            onended: move |_| {
                let s = player.peek().clone();
                if s.repeat == RepeatMode::One {
                    if let Some(a) = get_audio_element() {
                        a.set_current_time(0.0);
                        let _ = a.play();
                    }
                    return;
                }
                match s.next_index(true) {
                    Some(idx) => {
                        let mut w = player.write();
                        w.current_index = idx;
                        w.playing = true;
                    }
                    None => {
                        player.write().playing = false;
                    }
                }
            },
        }

        if player_visible {
            if let Some(track) = track {
                div { class: "fixed bottom-0 left-0 right-0 z-50 bg-base-200 border-t border-base-300 px-4 py-2",
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

                        // Track info
                        div { class: "flex flex-col min-w-0 w-28 sm:w-40",
                            div { class: "text-sm font-medium truncate", "{title}" }
                            div { class: "text-xs text-base-content/60 truncate", "{artist}" }
                        }

                        // Controls
                        div { class: "flex items-center gap-1",
                            // Shuffle
                            button {
                                class: "{shuffle_class} hidden sm:inline-flex",
                                title: if shuffle_on { "Shuffle: on" } else { "Shuffle: off" },
                                onclick: move |_| {
                                    player.write().toggle_shuffle();
                                },
                                IconShuffle { class: "w-4 h-4".to_string() }
                            }
                            // Previous
                            button {
                                class: "btn btn-ghost btn-sm btn-circle",
                                disabled: !has_prev && cur <= 3.0,
                                onclick: move |_| {
                                    // If more than 3s into the track, restart current song.
                                    if let Some(a) = get_audio_element() {
                                        if a.current_time() > 3.0 {
                                            a.set_current_time(0.0);
                                            return;
                                        }
                                    }
                                    let s = player.peek().clone();
                                    if let Some(idx) = s.prev_index() {
                                        let mut w = player.write();
                                        w.current_index = idx;
                                        w.playing = true;
                                    }
                                },
                                IconSkipBack { class: "w-5 h-5".to_string() }
                            }
                            // Play/Pause
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
                            // Next
                            button {
                                class: "btn btn-ghost btn-sm btn-circle",
                                disabled: !has_next,
                                onclick: move |_| {
                                    let s = player.peek().clone();
                                    if let Some(idx) = s.next_index(false) {
                                        let mut w = player.write();
                                        w.current_index = idx;
                                        w.playing = true;
                                    }
                                },
                                IconSkipForward { class: "w-5 h-5".to_string() }
                            }
                            // Repeat
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
                                        if let Some(audio) = get_audio_element() {
                                            audio.set_current_time(target_time);
                                        }
                                        current_secs.set(target_time);
                                    }
                                },
                            }
                            span { class: "text-xs text-base-content/50 tabular-nums w-10", "{dur_fmt}" }
                        }

                        // Volume
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
                                if let Some(audio) = get_audio_element() {
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
                }
            }
        }
    }
}
