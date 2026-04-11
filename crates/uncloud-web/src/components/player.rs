use dioxus::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use crate::components::icons::{IconMusic, IconPause, IconPlay, IconSkipBack, IconSkipForward, IconVolume2};
use crate::hooks::{api, use_files::download_url};
use crate::state::PlayerState;

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
    // Track which src we last set so we only reload when the track actually changes.
    let mut last_src: Signal<Option<String>> = use_signal(|| None);

    let state = player();

    // Sync audio element with player state.
    use_effect(move || {
        let state = player();
        let Some(audio) = get_audio_element() else {
            return;
        };

        let desired = state.current_track().map(|t| download_url(&t.file.id));

        // If the track changed, update src and load.
        if desired != *last_src.peek() {
            if let Some(ref src) = desired {
                audio.set_src(src);
                let _ = audio.load();
                current_secs.set(0.0);
                duration.set(0.0);
            }
            last_src.set(desired.clone());
        }

        // Sync play/pause.
        if state.playing && desired.is_some() {
            let _ = audio.play();
        } else {
            let _ = audio.pause();
        }
    });

    // Set up audio event listeners once.
    use_effect(move || {
        let Some(audio) = get_audio_element() else {
            return;
        };

        // timeupdate
        let timeupdate_cb = Closure::<dyn FnMut()>::new(move || {
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
        });
        let _ = audio.add_event_listener_with_callback(
            "timeupdate",
            timeupdate_cb.as_ref().unchecked_ref(),
        );
        timeupdate_cb.forget();

        // loadedmetadata
        let loadedmeta_cb = Closure::<dyn FnMut()>::new(move || {
            if let Some(a) = get_audio_element() {
                let d = a.duration();
                if !d.is_nan() && d > 0.0 {
                    duration.set(d);
                }
            }
        });
        let _ = audio.add_event_listener_with_callback(
            "loadedmetadata",
            loadedmeta_cb.as_ref().unchecked_ref(),
        );
        loadedmeta_cb.forget();

        // ended → advance to next track
        let ended_cb = Closure::<dyn FnMut()>::new(move || {
            let state = player();
            if state.has_next() {
                player.write().current_index += 1;
                // playing remains true; the sync effect above will load+play the new track
            } else {
                player.write().playing = false;
            }
        });
        let _ = audio.add_event_listener_with_callback(
            "ended",
            ended_cb.as_ref().unchecked_ref(),
        );
        ended_cb.forget();
    });

    // Don't render anything when queue is empty.
    if state.queue.is_empty() {
        // Still render the hidden audio element so listeners stay attached.
        return rsx! {
            audio { id: AUDIO_ELEMENT_ID, class: "hidden", preload: "auto" }
        };
    }

    let track = state.current_track().cloned();
    let Some(track) = track else {
        return rsx! {
            audio { id: AUDIO_ELEMENT_ID, class: "hidden", preload: "auto" }
        };
    };

    let title = track
        .audio
        .title
        .as_deref()
        .unwrap_or(&track.file.name)
        .to_string();
    let artist = track
        .audio
        .artist
        .as_deref()
        .unwrap_or("Unknown")
        .to_string();

    let cover_src = if track.audio.has_cover_art {
        Some(api::authenticated_media_url(&format!("/files/{}/thumb", track.file.id)))
    } else {
        None
    };

    let is_playing = state.playing;
    let has_prev = state.has_prev();
    let has_next = state.has_next();
    let cur = current_secs();
    let dur = duration();
    let vol = volume();

    let cur_fmt = format_time(cur);
    let dur_fmt = format_time(dur);

    // Seek percentage for the range input (0..1000 for smoother seeking).
    let seek_val = if dur > 0.0 {
        ((cur / dur) * 1000.0) as i64
    } else {
        0
    };

    let vol_val = (vol * 100.0) as i64;

    rsx! {
        // Hidden audio element — always mounted
        audio { id: AUDIO_ELEMENT_ID, class: "hidden", preload: "auto" }

        // Player bar
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
                    // Previous
                    button {
                        class: "btn btn-ghost btn-sm btn-circle",
                        disabled: !has_prev,
                        onclick: move |_| {
                            let mut state = player.write();
                            if state.current_index > 0 {
                                state.current_index -= 1;
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
                            let state = player();
                            if state.has_next() {
                                player.write().current_index += 1;
                            }
                        },
                        IconSkipForward { class: "w-5 h-5".to_string() }
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
            }
        }
    }
}
