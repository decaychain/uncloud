//! Right-side playlist panel rendered alongside the Music browse routes on
//! wide screens. Shows the currently pinned playlist's tracks so users can
//! see additions in real time while browsing the library.

use dioxus::prelude::*;
use uncloud_common::{ServerEvent, TrackResponse};

use crate::components::icons::{IconExternalLink, IconMusic, IconPause, IconPlay, IconX};
use crate::hooks::{use_player, use_playlists};
use crate::router::Route;
use crate::state::{PinnedPlaylistState, PlayerState, PlaylistDirtyTick};

#[component]
pub fn PlaylistSidePanel() -> Element {
    let mut pinned = use_context::<Signal<PinnedPlaylistState>>();
    let mut player = use_context::<Signal<PlayerState>>();
    let sse_event = use_context::<Signal<Option<ServerEvent>>>();
    let dirty = use_context::<Signal<PlaylistDirtyTick>>();

    let pid = match pinned().0 {
        Some(id) => id,
        None => return rsx! {},
    };

    let mut tracks: Signal<Vec<TrackResponse>> = use_signal(Vec::new);
    let mut name: Signal<String> = use_signal(|| String::new());
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    // Bumped on relevant SSE events so the panel reloads when tracks are
    // added elsewhere (e.g. the user added a track from the album view).
    let mut refresh = use_signal(|| 0u32);

    use_effect(use_reactive!(|(pid)| {
        let _ = refresh();
        let _ = dirty().0;
        spawn(async move {
            error.set(None);
            match use_playlists::get_playlist(&pid).await {
                Ok(resp) => {
                    name.set(resp.name);
                    tracks.set(resp.tracks);
                    loading.set(false);
                }
                Err(e) => {
                    // 404 → the pinned playlist was deleted from elsewhere; clear pin.
                    if e == "Playlist not found" {
                        pinned.set(PinnedPlaylistState(None));
                    } else {
                        error.set(Some(e));
                    }
                    loading.set(false);
                }
            }
        });
    }));

    // Reload when files are created/updated/deleted — picks up adds/removes
    // made from the album/artist views without polling.
    use_effect(move || {
        if let Some(event) = sse_event() {
            match event {
                ServerEvent::FileCreated { .. }
                | ServerEvent::FileUpdated { .. }
                | ServerEvent::FileDeleted { .. } => {
                    let next = *refresh.peek() + 1;
                    refresh.set(next);
                }
                _ => {}
            }
        }
    });

    let current_playing_id = player().current_track().map(|t| t.file.id.clone());
    let is_playing = player().playing;
    let track_list = tracks();
    let track_count = track_list.len();
    let tracks_for_play_all = track_list.clone();
    let panel_pid = pid.clone();

    rsx! {
        div { class: "card bg-base-100 border border-base-300 sticky top-4",
            // Header
            div { class: "p-3 border-b border-base-300 flex items-center justify-between gap-2",
                div { class: "min-w-0 flex-1",
                    div { class: "text-xs uppercase tracking-wide text-base-content/50", "Pinned playlist" }
                    div { class: "font-semibold truncate", title: "{name}", "{name}" }
                }
                div { class: "flex items-center gap-1 shrink-0",
                    Link {
                        to: Route::MusicPlaylist { id: panel_pid.clone() },
                        class: "btn btn-ghost btn-xs btn-circle",
                        title: "Open playlist",
                        IconExternalLink { class: "w-4 h-4".to_string() }
                    }
                    button {
                        class: "btn btn-ghost btn-xs btn-circle",
                        title: "Unpin",
                        onclick: move |_| pinned.set(PinnedPlaylistState(None)),
                        IconX { class: "w-4 h-4".to_string() }
                    }
                }
            }

            // Body
            div { class: "p-2 max-h-[60vh] overflow-y-auto",
                if loading() {
                    div { class: "flex items-center justify-center py-8",
                        span { class: "loading loading-spinner loading-md" }
                    }
                } else if let Some(err) = error() {
                    div { class: "text-sm text-error p-3", "{err}" }
                } else if track_list.is_empty() {
                    div { class: "flex flex-col items-center justify-center py-8 gap-2 text-center",
                        IconMusic { class: "w-8 h-8 text-base-content/30".to_string() }
                        p { class: "text-xs text-base-content/60",
                            "No tracks yet. Add some from the music library."
                        }
                    }
                } else {
                    ul { class: "menu menu-sm w-full p-0 gap-0.5",
                        for (idx, track) in track_list.iter().enumerate() {
                            {
                                let title = track.audio.title.as_deref()
                                    .unwrap_or(&track.file.name).to_string();
                                let artist = track.audio.artist.as_deref()
                                    .unwrap_or("Unknown").to_string();
                                let file_id = track.file.id.clone();
                                let is_current = current_playing_id.as_deref() == Some(&track.file.id);
                                let tracks_for_play = track_list.clone();
                                let pid_remove = panel_pid.clone();

                                let row_class = if is_current && is_playing {
                                    "flex items-center gap-2 px-2 py-1 rounded bg-primary/10 group"
                                } else {
                                    "flex items-center gap-2 px-2 py-1 rounded hover:bg-base-200 group"
                                };

                                rsx! {
                                    li { key: "{file_id}",
                                        div { class: "{row_class}",
                                            // Play / pause
                                            button {
                                                class: "btn btn-ghost btn-xs btn-circle shrink-0",
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
                                            // Title + artist
                                            div { class: "flex-1 min-w-0",
                                                div {
                                                    class: if is_current && is_playing { "text-sm font-medium truncate text-primary" } else { "text-sm font-medium truncate" },
                                                    title: "{title}",
                                                    "{title}"
                                                }
                                                div { class: "text-xs text-base-content/60 truncate", title: "{artist}", "{artist}" }
                                            }
                                            // Remove
                                            button {
                                                class: "btn btn-ghost btn-xs btn-circle shrink-0 text-error opacity-0 group-hover:opacity-100 transition-opacity",
                                                title: "Remove from playlist",
                                                onclick: move |_| {
                                                    let fid = file_id.clone();
                                                    let pid = pid_remove.clone();
                                                    tracks.write().retain(|t| t.file.id != fid);
                                                    spawn(async move {
                                                        let _ = use_playlists::remove_from_playlist(&pid, &[&fid]).await;
                                                    });
                                                },
                                                IconX { class: "w-3 h-3".to_string() }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Footer with Play All
            if !track_list.is_empty() {
                div { class: "p-2 border-t border-base-300",
                    button {
                        class: "btn btn-primary btn-sm btn-block",
                        onclick: move |_| use_player::play_queue(player, tracks_for_play_all.clone(), 0),
                        IconPlay { class: "w-4 h-4".to_string() }
                        "Play all ({track_count})"
                    }
                }
            }
        }
    }
}
