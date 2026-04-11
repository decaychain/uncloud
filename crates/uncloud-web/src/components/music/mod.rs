mod track_list;
mod artist_list;
mod album_grid;
mod artist_view;
mod album_view;
mod folder_view;
pub mod playlist_view;

use dioxus::prelude::*;
use uncloud_common::{ArtistResponse, MusicAlbumResponse, ServerEvent};

use crate::components::icons::IconAlertTriangle;
use crate::hooks::use_music;

pub use album_view::AlbumView as MusicAlbumView;
pub use artist_view::ArtistView as MusicArtistView;
pub use folder_view::FolderView as MusicFolderView;
pub use playlist_view::PlaylistView as MusicPlaylistView;

// ── Navigation state for "By Metadata" tab ─────────────────────────────────

#[derive(Clone, PartialEq)]
enum MetadataNav {
    Artists,
    Artist(String),
    Album(String, String),
}

// ── MetadataView ────────────────────────────────────────────────────────────

#[component]
fn MetadataView() -> Element {
    let mut nav_state: Signal<MetadataNav> = use_signal(|| MetadataNav::Artists);
    let mut artists: Signal<Vec<ArtistResponse>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut refresh = use_signal(|| 0u32);

    let sse_event = use_context::<Signal<Option<ServerEvent>>>();
    use_effect(move || {
        if let Some(event) = sse_event() {
            match event {
                ServerEvent::FileCreated { .. }
                | ServerEvent::FileDeleted { .. }
                | ServerEvent::FileUpdated { .. } => {
                    let next = *refresh.peek() + 1;
                    refresh.set(next);
                }
                _ => {}
            }
        }
    });

    use_effect(move || {
        let _ = refresh();
        spawn(async move {
            loading.set(true);
            error.set(None);
            match use_music::list_artists().await {
                Ok(a) => artists.set(a),
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    });

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
                h3 { class: "text-lg font-semibold", "Error loading artists" }
                p { class: "text-base-content/60", "{err}" }
            }
        };
    }

    match nav_state() {
        MetadataNav::Artists => rsx! {
            artist_list::ArtistList {
                artists: artists(),
                on_select: move |name: String| nav_state.set(MetadataNav::Artist(name)),
            }
        },
        MetadataNav::Artist(name) => {
            let name_clone = name.clone();
            rsx! {
                MusicArtistView {
                    name,
                    on_back: move |_| nav_state.set(MetadataNav::Artists),
                    on_album_select: move |album: MusicAlbumResponse| {
                        nav_state.set(MetadataNav::Album(name_clone.clone(), album.name));
                    },
                }
            }
        },
        MetadataNav::Album(artist, album) => rsx! {
            MusicAlbumView {
                artist,
                album,
                on_back: move |_| nav_state.set(MetadataNav::Artists),
            }
        },
    }
}

// ── Music (main component) ──────────────────────────────────────────────────

#[component]
pub fn Music() -> Element {
    rsx! {
        div { class: "p-4 space-y-4",
            div { class: "flex items-center justify-between",
                h1 { class: "text-2xl font-bold", "Music" }
            }
            MetadataView {}
        }
    }
}
