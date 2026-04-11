use dioxus::prelude::*;
use uncloud_common::MusicAlbumResponse;
use crate::hooks::{use_music, use_player};
use crate::state::PlayerState;
use super::album_grid::AlbumGrid;
use crate::components::icons::IconAlertTriangle;

#[component]
pub fn ArtistView(
    name: String,
    on_back: EventHandler<()>,
    on_album_select: EventHandler<MusicAlbumResponse>,
) -> Element {
    let player = use_context::<Signal<PlayerState>>();
    let mut albums: Signal<Vec<MusicAlbumResponse>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let name_effect = name.clone();
    use_effect(use_reactive!(|name_effect| {
        let artist_name = name_effect;
        spawn(async move {
            loading.set(true);
            error.set(None);
            match use_music::list_artist_albums(&artist_name).await {
                Ok(a) => albums.set(a),
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
                h3 { class: "text-lg font-semibold", "Error loading albums" }
                p { class: "text-base-content/60", "{err}" }
            }
        };
    }

    rsx! {
        div {
            div { class: "flex items-center gap-3 mb-4",
                button {
                    class: "btn btn-ghost btn-sm",
                    onclick: move |_| on_back.call(()),
                    "← Back"
                }
                h2 { class: "text-xl font-bold", "{name}" }
            }
            AlbumGrid {
                albums: albums(),
                on_select: on_album_select,
                on_play: move |album: MusicAlbumResponse| {
                    let player_sig = player;
                    spawn(async move {
                        if let Ok(tracks) = use_music::list_album_tracks(&album.artist, &album.name).await {
                            use_player::play_queue(player_sig, tracks, 0);
                        }
                    });
                },
            }
        }
    }
}
