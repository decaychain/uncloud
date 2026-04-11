use dioxus::prelude::*;
use uncloud_common::TrackResponse;
use crate::hooks::{api, use_music, use_player};
use crate::state::PlayerState;
use super::track_list::TrackList;
use crate::components::icons::{IconAlertTriangle, IconMusic, IconPlay};

#[component]
pub fn AlbumView(artist: String, album: String, on_back: EventHandler<()>) -> Element {
    let player = use_context::<Signal<PlayerState>>();
    let mut tracks: Signal<Vec<TrackResponse>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let artist_effect = artist.clone();
    let album_effect = album.clone();
    use_effect(use_reactive!(|(artist_effect, album_effect)| {
        let a = artist_effect;
        let b = album_effect;
        spawn(async move {
            loading.set(true);
            error.set(None);
            match use_music::list_album_tracks(&a, &b).await {
                Ok(t) => tracks.set(t),
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
                h3 { class: "text-lg font-semibold", "Error loading album" }
                p { class: "text-base-content/60", "{err}" }
            }
        };
    }

    let track_list = tracks();
    let total_tracks = track_list.len();
    let cover_src = track_list.iter()
        .find(|t| t.audio.has_cover_art)
        .map(|t| api::authenticated_media_url(&format!("/files/{}/thumb", t.file.id)));
    let year_str = track_list.first()
        .and_then(|t| t.audio.year)
        .map(|y| format!("{}", y));

    let tracks_for_play_all = track_list.clone();
    let tracks_for_row = track_list.clone();

    rsx! {
        div {
            div { class: "flex items-center gap-3 mb-4",
                button {
                    class: "btn btn-ghost btn-sm",
                    onclick: move |_| on_back.call(()),
                    "← Back"
                }
            }

            // Album header
            div { class: "flex gap-4 mb-6",
                if let Some(src) = cover_src {
                    img {
                        class: "w-32 h-32 object-cover rounded-xl shadow",
                        src: "{src}",
                    }
                } else {
                    div { class: "w-32 h-32 flex items-center justify-center bg-base-200 rounded-xl shadow",
                        IconMusic { class: "w-12 h-12 text-base-content/40".to_string() }
                    }
                }
                div { class: "flex flex-col justify-center",
                    h2 { class: "text-xl font-bold", "{album}" }
                    p { class: "text-base-content/70", "{artist}" }
                    div { class: "flex gap-2 text-sm text-base-content/50 mt-1",
                        if let Some(y) = &year_str {
                            span { "{y}" }
                            span { "·" }
                        }
                        span { "{total_tracks} tracks" }
                    }
                    button {
                        class: "btn btn-primary btn-sm mt-2 w-fit",
                        onclick: move |_| {
                            use_player::play_queue(player, tracks_for_play_all.clone(), 0);
                        },
                        IconPlay { class: "w-4 h-4".to_string() }
                        "Play All"
                    }
                }
            }

            TrackList {
                tracks: tracks_for_row,
                show_artist: false,
                show_album: false,
                on_play: move |idx: usize| {
                    use_player::play_queue(player, tracks().clone(), idx);
                },
            }
        }
    }
}
