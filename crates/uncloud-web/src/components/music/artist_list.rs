use dioxus::prelude::*;
use uncloud_common::ArtistResponse;

use crate::components::icons::IconMusic;

#[component]
pub fn ArtistList(artists: Vec<ArtistResponse>, on_select: EventHandler<String>) -> Element {
    if artists.is_empty() {
        return rsx! {
            div { class: "flex flex-col items-center justify-center py-20 gap-3",
                IconMusic { class: "w-12 h-12 text-base-content/30".to_string() }
                h3 { class: "text-lg font-semibold", "No artists found" }
                p { class: "text-base-content/60 text-center max-w-md",
                    "Right-click a folder in Files and select \"Music settings\" to include it in your library."
                }
            }
        };
    }

    rsx! {
        div { class: "grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 gap-4",
            for artist in &artists {
                {
                    let name = artist.name.clone();
                    let name_click = artist.name.clone();
                    let album_count = artist.album_count;
                    let track_count = artist.track_count;
                    let first_letter = name.chars().next().unwrap_or('?').to_uppercase().to_string();
                    rsx! {
                        div {
                            class: "card bg-base-100 shadow-sm border border-base-300 cursor-pointer hover:shadow-md hover:ring-2 hover:ring-primary transition-all",
                            onclick: move |_| on_select.call(name_click.clone()),
                            div { class: "card-body items-center text-center p-4 gap-2",
                                div { class: "avatar placeholder",
                                    div { class: "bg-primary text-primary-content rounded-full w-16 h-16",
                                        span { class: "text-2xl font-bold", "{first_letter}" }
                                    }
                                }
                                div { class: "text-sm font-medium truncate w-full", title: "{name}", "{name}" }
                                div { class: "text-xs text-base-content/50",
                                    "{album_count} albums · {track_count} tracks"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
