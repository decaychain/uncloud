use dioxus::prelude::*;
use uncloud_common::PlaylistSummary;

use crate::components::icons::IconListMusic;
use crate::hooks::api;
use crate::router::Route;

#[component]
pub fn PlaylistList(playlists: Vec<PlaylistSummary>) -> Element {
    if playlists.is_empty() {
        return rsx! {};
    }

    rsx! {
        div { class: "space-y-2",
            h2 { class: "text-lg font-semibold", "Playlists" }
            div { class: "grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 gap-4",
                for pl in &playlists {
                    {
                        let id = pl.id.clone();
                        let name = pl.name.clone();
                        let track_count = pl.track_count;
                        let cover_src = pl.cover_file_id.as_ref()
                            .map(|fid| api::authenticated_media_url(&format!("/files/{}/thumb", fid)));
                        let track_word = if track_count == 1 { "track" } else { "tracks" };
                        rsx! {
                            Link {
                                to: Route::MusicPlaylist { id: id.clone() },
                                class: "card bg-base-100 shadow-sm border border-base-300 cursor-pointer hover:shadow-md hover:ring-2 hover:ring-primary transition-all",
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
                        }
                    }
                }
            }
        }
    }
}
