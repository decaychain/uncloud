use dioxus::prelude::*;
use uncloud_common::TrackResponse;
use crate::components::icons::{IconAlertTriangle, IconMusic, IconPause, IconPlay};
use crate::hooks::{use_playlists, use_player};
use crate::state::PlayerState;

fn format_duration(secs: f64) -> String {
    let total = secs as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

#[component]
pub fn PlaylistView(playlist_id: String) -> Element {
    let mut player = use_context::<Signal<PlayerState>>();
    let mut tracks: Signal<Vec<TrackResponse>> = use_signal(Vec::new);
    let mut playlist_name: Signal<String> = use_signal(|| String::new());
    let mut playlist_desc: Signal<Option<String>> = use_signal(|| None);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut refresh = use_signal(|| 0u32);

    let pid_for_remove = playlist_id.clone();
    let pid_for_reorder = playlist_id.clone();

    use_effect(use_reactive!(|(playlist_id, refresh)| {
        spawn(async move {
            loading.set(true);
            error.set(None);
            match use_playlists::get_playlist(&playlist_id).await {
                Ok(resp) => {
                    playlist_name.set(resp.name);
                    playlist_desc.set(resp.description);
                    tracks.set(resp.tracks);
                }
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    }));

    if loading() {
        return rsx! {
            div { class: "flex items-center justify-center py-20",
                span { class: "loading loading-spinner loading-lg" }
            }
        };
    }

    if let Some(err) = error() {
        return rsx! {
            div { class: "flex flex-col items-center justify-center py-20 gap-3",
                IconAlertTriangle { class: "w-12 h-12 text-warning".to_string() }
                h3 { class: "text-lg font-semibold", "Error loading playlist" }
                p { class: "text-base-content/60", "{err}" }
            }
        };
    }

    let track_list = tracks();
    let total_tracks = track_list.len();
    let total_duration: f64 = track_list.iter().filter_map(|t| t.audio.duration_secs).sum();
    let total_dur_str = if total_duration > 0.0 {
        let total_mins = (total_duration / 60.0).round() as u64;
        if total_mins >= 60 {
            format!("{} hr {} min", total_mins / 60, total_mins % 60)
        } else {
            format!("{} min", total_mins)
        }
    } else {
        String::new()
    };

    let current_playing_id = player().current_track().map(|t| t.file.id.clone());
    let is_playing = player().playing;

    let tracks_for_play_all = track_list.clone();

    rsx! {
        div { class: "space-y-4",
            // Header
            div { class: "flex items-start justify-between",
                div {
                    h2 { class: "text-2xl font-bold", "{playlist_name}" }
                    if let Some(desc) = playlist_desc() {
                        p { class: "text-base-content/60 mt-1", "{desc}" }
                    }
                    p { class: "text-sm text-base-content/50 mt-1",
                        "{total_tracks} tracks"
                        if !total_dur_str.is_empty() {
                            " \u{00B7} {total_dur_str}"
                        }
                    }
                }
                if !track_list.is_empty() {
                    button {
                        class: "btn btn-primary btn-sm",
                        onclick: move |_| use_player::play_queue(player, tracks_for_play_all.clone(), 0),
                        IconPlay { class: "w-4 h-4".to_string() }
                        "Play All"
                    }
                }
            }

            if track_list.is_empty() {
                div { class: "flex flex-col items-center justify-center py-12 gap-3",
                    IconMusic { class: "w-10 h-10 text-base-content/30".to_string() }
                    p { class: "text-base-content/60", "This playlist is empty. Add tracks from the music library." }
                }
            } else {
                div { class: "overflow-hidden rounded-box border border-base-300",
                    table { class: "table table-sm w-full",
                        thead {
                            tr {
                                th { class: "w-8" }  // play button
                                th { class: "w-10 text-center", "#" }
                                th { "Title" }
                                th { class: "hidden sm:table-cell", "Artist" }
                                th { class: "hidden md:table-cell", "Album" }
                                th { class: "w-16 text-right", "Duration" }
                                th { class: "w-20 text-center", "" }  // reorder + remove
                            }
                        }
                        tbody {
                            for (idx, track) in track_list.iter().enumerate() {
                                {
                                    let title = track.audio.title.as_deref()
                                        .unwrap_or(&track.file.name).to_string();
                                    let artist = track.audio.artist.as_deref()
                                        .unwrap_or("Unknown").to_string();
                                    let album = track.audio.album.as_deref()
                                        .unwrap_or("Unknown").to_string();
                                    let duration = track.audio.duration_secs
                                        .map(format_duration)
                                        .unwrap_or_else(|| "--:--".to_string());
                                    let file_id = track.file.id.clone();
                                    let is_current = current_playing_id.as_deref() == Some(&track.file.id);
                                    let row_class = if is_current && is_playing {
                                        "hover:bg-base-200 transition-colors bg-primary/10 group"
                                    } else {
                                        "hover:bg-base-200 transition-colors group"
                                    };
                                    let pid_rm = pid_for_remove.clone();
                                    let pid_ro_up = pid_for_reorder.clone();
                                    let pid_ro_down = pid_for_reorder.clone();
                                    let tracks_for_play = track_list.clone();
                                    let is_first = idx == 0;
                                    let is_last = idx == total_tracks - 1;

                                    rsx! {
                                        tr { class: row_class,
                                            // Play / pause button
                                            td { class: "text-center",
                                                button {
                                                    class: "btn btn-ghost btn-xs btn-circle",
                                                    onclick: move |_| {
                                                        if is_current {
                                                            player.write().playing = !is_playing;
                                                        } else {
                                                            use_player::play_queue(player, tracks_for_play.clone(), idx);
                                                        }
                                                    },
                                                    if is_current && is_playing {
                                                        IconPause { class: "w-3 h-3".to_string() }
                                                    } else {
                                                        IconPlay { class: "w-3 h-3".to_string() }
                                                    }
                                                }
                                            }
                                            td { class: "text-center text-base-content/50 tabular-nums", "{idx + 1}" }
                                            td { class: "font-medium truncate max-w-xs",
                                                if is_current && is_playing {
                                                    span { class: "text-primary", title: "{title}", "{title}" }
                                                } else {
                                                    span { title: "{title}", "{title}" }
                                                }
                                            }
                                            td { class: "hidden sm:table-cell text-base-content/70 truncate max-w-xs", "{artist}" }
                                            td { class: "hidden md:table-cell text-base-content/70 truncate max-w-xs", "{album}" }
                                            td { class: "text-right text-base-content/50 tabular-nums", "{duration}" }
                                            td { class: "text-center",
                                                div { class: "flex items-center gap-0.5 opacity-0 group-hover:opacity-100 transition-opacity justify-center",
                                                    // Move up
                                                    button {
                                                        class: "btn btn-ghost btn-xs btn-circle",
                                                        disabled: is_first,
                                                        title: "Move up",
                                                        onclick: move |_| {
                                                            let mut t = tracks.write();
                                                            if idx > 0 {
                                                                t.swap(idx, idx - 1);
                                                                let ids: Vec<String> = t.iter().map(|tr| tr.file.id.clone()).collect();
                                                                let pid = pid_ro_up.clone();
                                                                drop(t);
                                                                spawn(async move {
                                                                    let id_refs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
                                                                    let _ = use_playlists::reorder_playlist(&pid, &id_refs).await;
                                                                });
                                                            }
                                                        },
                                                        "↑"
                                                    }
                                                    // Move down
                                                    button {
                                                        class: "btn btn-ghost btn-xs btn-circle",
                                                        disabled: is_last,
                                                        title: "Move down",
                                                        onclick: move |_| {
                                                            let mut t = tracks.write();
                                                            let len = t.len();
                                                            if idx + 1 < len {
                                                                t.swap(idx, idx + 1);
                                                                let ids: Vec<String> = t.iter().map(|tr| tr.file.id.clone()).collect();
                                                                let pid = pid_ro_down.clone();
                                                                drop(t);
                                                                spawn(async move {
                                                                    let id_refs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
                                                                    let _ = use_playlists::reorder_playlist(&pid, &id_refs).await;
                                                                });
                                                            }
                                                        },
                                                        "↓"
                                                    }
                                                    // Remove
                                                    button {
                                                        class: "btn btn-ghost btn-xs btn-circle text-error",
                                                        title: "Remove from playlist",
                                                        onclick: move |_| {
                                                            let fid = file_id.clone();
                                                            let pid = pid_rm.clone();
                                                            spawn(async move {
                                                                let _ = use_playlists::remove_from_playlist(&pid, &[&fid]).await;
                                                                let next = *refresh.peek() + 1;
                                                                refresh.set(next);
                                                            });
                                                        },
                                                        "\u{00D7}"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
