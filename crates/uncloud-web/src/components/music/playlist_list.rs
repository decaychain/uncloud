use dioxus::prelude::*;
use uncloud_common::PlaylistSummary;

use crate::components::icons::{IconListMusic, IconPin, IconPinOff};
use crate::hooks::api;
use crate::router::Route;
use crate::state::PinnedPlaylistState;

#[component]
pub fn PlaylistList(playlists: Vec<PlaylistSummary>) -> Element {
    if playlists.is_empty() {
        return rsx! {};
    }

    let mut pinned = use_context::<Signal<PinnedPlaylistState>>();
    let pinned_id = pinned().0.clone();

    rsx! {
        div { class: "space-y-2",
            h2 { class: "text-lg font-semibold", "Playlists" }
            div { class: "grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 gap-4",
                for pl in &playlists {
                    {
                        let id = pl.id.clone();
                        let id_for_pin = pl.id.clone();
                        let name = pl.name.clone();
                        let track_count = pl.track_count;
                        let cover_src = pl.cover_file_id.as_ref()
                            .map(|fid| api::authenticated_media_url(&format!("/files/{}/thumb", fid)));
                        let track_word = if track_count == 1 { "track" } else { "tracks" };
                        let is_pinned = pinned_id.as_deref() == Some(pl.id.as_str());
                        rsx! {
                            div { class: "relative group",
                                Link {
                                    to: Route::MusicPlaylist { id: id.clone() },
                                    class: "card bg-base-100 shadow-sm border border-base-300 cursor-pointer hover:shadow-md hover:ring-2 hover:ring-primary transition-all block",
                                    div { class: "card-body p-0 gap-0",
                                        if let Some(src) = cover_src {
                                            img {
                                                class: "w-full h-32 object-cover rounded-t-xl",
                                                src: "{src}",
                                                loading: "lazy",
                                            }
                                        } else {
                                            div { class: "flex items-center justify-center h-32 bg-base-200 rounded-t-xl",
                                                IconListMusic { class: "w-10 h-10 text-base-content/30".to_string() }
                                            }
                                        }
                                        div { class: "p-3 text-center",
                                            div { class: "text-sm font-medium truncate", title: "{name}", "{name}" }
                                            div { class: "text-xs text-base-content/50",
                                                "{track_count} {track_word}"
                                            }
                                        }
                                    }
                                }
                                // Pin button overlays the card top-right corner. Hidden
                                // on mobile (no panel anyway) and revealed on hover on
                                // desktop unless already pinned, in which case it stays
                                // visible to communicate state.
                                button {
                                    class: if is_pinned {
                                        "hidden xl:flex absolute top-2 right-2 btn btn-xs btn-circle btn-primary shadow"
                                    } else {
                                        "hidden xl:flex absolute top-2 right-2 btn btn-xs btn-circle bg-base-100/80 hover:bg-base-100 border-base-300 opacity-0 group-hover:opacity-100 transition-opacity"
                                    },
                                    title: if is_pinned { "Unpin from side panel" } else { "Pin to side panel" },
                                    onclick: move |e: Event<MouseData>| {
                                        e.stop_propagation();
                                        e.prevent_default();
                                        if is_pinned {
                                            pinned.set(PinnedPlaylistState(None));
                                        } else {
                                            pinned.set(PinnedPlaylistState(Some(id_for_pin.clone())));
                                        }
                                    },
                                    if is_pinned {
                                        IconPinOff { class: "w-3 h-3".to_string() }
                                    } else {
                                        IconPin { class: "w-3 h-3".to_string() }
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
