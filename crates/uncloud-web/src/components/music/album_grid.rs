use dioxus::prelude::*;
use uncloud_common::MusicAlbumResponse;
use crate::hooks::api;

#[component]
pub fn AlbumGrid(
    albums: Vec<MusicAlbumResponse>,
    on_select: EventHandler<MusicAlbumResponse>,
    on_play: Option<EventHandler<MusicAlbumResponse>>,
) -> Element {
    if albums.is_empty() {
        return rsx! {
            div { class: "flex flex-col items-center justify-center py-12 gap-3",
                div { class: "text-4xl", "🎵" }
                p { class: "text-base-content/60", "No albums found" }
            }
        };
    }

    rsx! {
        div { class: "grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 gap-4",
            for album in &albums {
                {
                    let album_select = album.clone();
                    let album_play = album.clone();
                    let cover_src = album.cover_file_id.as_ref()
                        .map(|id| api::authenticated_media_url(&format!("/files/{}/thumb", id)));
                    let year_str = album.year.map(|y| format!(" · {}", y)).unwrap_or_default();
                    let on_play_clone = on_play.clone();
                    rsx! {
                        div {
                            class: "card bg-base-100 shadow-sm border border-base-300 cursor-pointer hover:shadow-md hover:ring-2 hover:ring-primary transition-all group relative",
                            onclick: move |_| on_select.call(album_select.clone()),
                            div { class: "card-body p-0 gap-0",
                                div { class: "relative",
                                    if let Some(src) = cover_src {
                                        img {
                                            class: "w-full h-32 object-cover rounded-t-xl",
                                            src: "{src}",
                                            loading: "lazy",
                                        }
                                    } else {
                                        div { class: "flex items-center justify-center h-32 text-4xl bg-base-200 rounded-t-xl",
                                            "🎵"
                                        }
                                    }
                                    // Play button overlay
                                    if on_play_clone.is_some() {
                                        button {
                                            class: "absolute bottom-2 right-2 btn btn-circle btn-sm btn-primary opacity-0 group-hover:opacity-100 transition-opacity shadow-lg",
                                            onclick: move |evt: Event<MouseData>| {
                                                evt.stop_propagation();
                                                if let Some(ref handler) = on_play_clone {
                                                    handler.call(album_play.clone());
                                                }
                                            },
                                            svg { class: "w-4 h-4", fill: "currentColor", view_box: "0 0 24 24",
                                                path { d: "M8 5v14l11-7z" }
                                            }
                                        }
                                    }
                                }
                                div { class: "p-3 text-center",
                                    div { class: "text-sm font-medium truncate", "{album.name}" }
                                    div { class: "text-xs text-base-content/60 truncate", "{album.artist}" }
                                    div { class: "text-xs text-base-content/50",
                                        "{album.track_count} tracks{year_str}"
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
