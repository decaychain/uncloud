use dioxus::prelude::*;
use uncloud_common::{PlaylistSummary, TrackResponse};
use crate::components::icons::{IconListMusic, IconMusic, IconPause, IconPlay};
use crate::hooks::use_playlists;
use crate::state::{PlayerState, PlaylistDirtyTick};

fn format_duration(secs: f64) -> String {
    let total = secs as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

#[component]
pub fn TrackList(
    tracks: Vec<TrackResponse>,
    show_artist: bool,
    show_album: bool,
    on_play: Option<EventHandler<usize>>,
) -> Element {
    let mut player = use_context::<Signal<PlayerState>>();
    let mut playlist_dirty = use_context::<Signal<PlaylistDirtyTick>>();

    // "Add to playlist" modal state
    let mut add_to_playlist_file_id: Signal<Option<String>> = use_signal(|| None);
    let mut available_playlists: Signal<Vec<PlaylistSummary>> = use_signal(Vec::new);
    let mut add_loading = use_signal(|| false);
    let mut add_success: Signal<Option<String>> = use_signal(|| None);

    // Inline create state
    let mut show_inline_create = use_signal(|| false);
    let mut inline_name: Signal<String> = use_signal(|| String::new());
    let mut inline_error: Signal<Option<String>> = use_signal(|| None);

    if tracks.is_empty() {
        return rsx! {
            div { class: "flex flex-col items-center justify-center py-12 gap-3",
                IconMusic { class: "w-10 h-10 text-base-content/30".to_string() }
                p { class: "text-base-content/60", "No tracks found" }
            }
        };
    }

    let current_playing_id = player()
        .current_track()
        .map(|t| t.file.id.clone());
    let is_playing = player().playing;

    rsx! {
        div { class: "overflow-hidden rounded-box border border-base-300",
            table { class: "table table-sm w-full",
                thead {
                    tr {
                        if on_play.is_some() {
                            th { class: "w-10" }
                        }
                        th { class: "w-10 text-center", "#" }
                        th { "Title" }
                        if show_artist {
                            th { class: "hidden sm:table-cell", "Artist" }
                        }
                        if show_album {
                            th { class: "hidden md:table-cell", "Album" }
                        }
                        th { class: "w-16 text-right", "Duration" }
                        th { class: "w-10", "" }
                    }
                }
                tbody {
                    for (idx, track) in tracks.iter().enumerate() {
                        {
                            let title = track.audio.title.as_deref()
                                .unwrap_or(&track.file.name);
                            let artist = track.audio.artist.as_deref()
                                .unwrap_or("Unknown");
                            let album = track.audio.album.as_deref()
                                .unwrap_or("Unknown");
                            let track_num = track.audio.track_number
                                .map(|n| n.to_string())
                                .unwrap_or_default();
                            let duration = track.audio.duration_secs
                                .map(format_duration)
                                .unwrap_or_else(|| "--:--".to_string());
                            let is_current = current_playing_id.as_deref() == Some(&track.file.id);
                            let row_class = if is_current && is_playing {
                                "hover:bg-base-200 transition-colors bg-primary/10 group"
                            } else {
                                "hover:bg-base-200 transition-colors group"
                            };
                            let file_id = track.file.id.clone();
                            let on_play_clone = on_play.clone();
                            rsx! {
                                tr { class: row_class,
                                    if on_play_clone.is_some() {
                                        td { class: "text-center",
                                            button {
                                                class: "btn btn-ghost btn-xs btn-circle",
                                                onclick: move |_| {
                                                    if is_current {
                                                        player.write().playing = !is_playing;
                                                    } else if let Some(ref handler) = on_play_clone {
                                                        handler.call(idx);
                                                    }
                                                },
                                                if is_current && is_playing {
                                                    IconPause { class: "w-4 h-4".to_string() }
                                                } else {
                                                    IconPlay { class: "w-4 h-4".to_string() }
                                                }
                                            }
                                        }
                                    }
                                    td { class: "text-center text-base-content/50 tabular-nums", "{track_num}" }
                                    td { class: "font-medium truncate max-w-xs",
                                        span { title: "{title}",
                                            if is_current && is_playing {
                                                span { class: "text-primary", "{title}" }
                                            } else {
                                                "{title}"
                                            }
                                        }
                                    }
                                    if show_artist {
                                        td { class: "hidden sm:table-cell text-base-content/70 truncate max-w-xs", "{artist}" }
                                    }
                                    if show_album {
                                        td { class: "hidden md:table-cell text-base-content/70 truncate max-w-xs", "{album}" }
                                    }
                                    td { class: "text-right text-base-content/50 tabular-nums", "{duration}" }
                                    td {
                                        button {
                                            class: "btn btn-ghost btn-xs opacity-0 group-hover:opacity-100 transition-opacity",
                                            title: "Add to playlist",
                                            onclick: move |_| {
                                                add_to_playlist_file_id.set(Some(file_id.clone()));
                                                add_success.set(None);
                                                add_loading.set(false);
                                                show_inline_create.set(false);
                                                inline_name.set(String::new());
                                                inline_error.set(None);
                                                // Fetch playlists
                                                spawn(async move {
                                                    if let Ok(pls) = use_playlists::list_playlists().await {
                                                        available_playlists.set(pls);
                                                    }
                                                });
                                            },
                                            "+"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Add to playlist modal
        if let Some(file_id) = add_to_playlist_file_id() {
            div { class: "modal modal-open",
                div { class: "modal-box",
                    h3 { class: "font-bold text-lg mb-4", "Add to Playlist" }

                    if let Some(msg) = add_success() {
                        div { class: "alert alert-success mb-3 text-sm", "{msg}" }
                    }

                    if available_playlists().is_empty() && !show_inline_create() {
                        p { class: "text-base-content/60 mb-4", "No playlists yet. Create one to get started." }
                    }

                    if !show_inline_create() {
                        ul { class: "menu menu-sm bg-base-200 rounded-box w-full mb-3",
                            for pl in available_playlists() {
                                {
                                    let pl_id = pl.id.clone();
                                    let pl_name = pl.name.clone();
                                    let fid = file_id.clone();
                                    rsx! {
                                        li {
                                            a {
                                                onclick: move |_| {
                                                    let pid = pl_id.clone();
                                                    let fid = fid.clone();
                                                    let pname = pl_name.clone();
                                                    spawn(async move {
                                                        add_loading.set(true);
                                                        match use_playlists::add_to_playlist(&pid, &[fid.as_str()]).await {
                                                            Ok(()) => {
                                                                add_success.set(Some(format!("Added to \"{}\"", pname)));
                                                                let next = playlist_dirty.peek().0.wrapping_add(1);
                                                                playlist_dirty.set(PlaylistDirtyTick(next));
                                                            }
                                                            Err(e) => {
                                                                add_success.set(Some(format!("Error: {}", e)));
                                                            }
                                                        }
                                                        add_loading.set(false);
                                                    });
                                                },
                                                IconListMusic { class: "w-4 h-4 opacity-60".to_string() }
                                                span { "{pl.name}" }
                                                span { class: "text-base-content/50 text-xs ml-auto",
                                                    "{pl.track_count} tracks"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Inline create
                    if show_inline_create() {
                        div { class: "mb-3",
                            if let Some(err) = inline_error() {
                                div { class: "alert alert-error mb-2 text-sm", "{err}" }
                            }
                            div { class: "flex gap-2",
                                input {
                                    class: "input input-bordered input-sm flex-1",
                                    r#type: "text",
                                    placeholder: "New playlist name",
                                    value: "{inline_name}",
                                    oninput: move |e| inline_name.set(e.value()),
                                }
                                button {
                                    class: "btn btn-primary btn-sm",
                                    disabled: inline_name().trim().is_empty(),
                                    onclick: move |_| {
                                        let name = inline_name().trim().to_string();
                                        let fid = file_id.clone();
                                        spawn(async move {
                                            match use_playlists::create_playlist(&name, None).await {
                                                Ok(summary) => {
                                                    // Add track to the new playlist
                                                    let _ = use_playlists::add_to_playlist(&summary.id, &[fid.as_str()]).await;
                                                    add_success.set(Some(format!("Created \"{}\" and added track", name)));
                                                    show_inline_create.set(false);
                                                    let next = playlist_dirty.peek().0.wrapping_add(1);
                                                    playlist_dirty.set(PlaylistDirtyTick(next));
                                                    // Refresh playlist list
                                                    if let Ok(pls) = use_playlists::list_playlists().await {
                                                        available_playlists.set(pls);
                                                    }
                                                }
                                                Err(e) => {
                                                    if e == "CONFLICT" {
                                                        inline_error.set(Some(format!("\"{}\" already exists", name)));
                                                    } else {
                                                        inline_error.set(Some(e));
                                                    }
                                                }
                                            }
                                        });
                                    },
                                    "Create"
                                }
                            }
                        }
                    } else {
                        button {
                            class: "btn btn-ghost btn-sm w-full mb-2",
                            onclick: move |_| {
                                show_inline_create.set(true);
                                inline_name.set(String::new());
                                inline_error.set(None);
                            },
                            "+ New playlist"
                        }
                    }

                    div { class: "modal-action",
                        button {
                            class: "btn",
                            onclick: move |_| add_to_playlist_file_id.set(None),
                            "Close"
                        }
                    }
                }
                div { class: "modal-backdrop",
                    onclick: move |_| add_to_playlist_file_id.set(None),
                }
            }
        }
    }
}
