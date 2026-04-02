use dioxus::prelude::*;
use crate::components::{
    auth::{Login, Register},
    file_browser::FileBrowser,
    layout::Layout,
    gallery::{Gallery, GalleryAlbum},
    music::{Music, MusicArtistView, MusicAlbumView as MusicAlbumViewComp, MusicFolderView, MusicPlaylistView},
    settings::Settings,
    setup::Setup,
    shopping,
    trash::Trash,
};

#[derive(Debug, Clone, Routable, PartialEq)]
#[rustfmt::skip]
pub enum Route {
    #[route("/login")]
    Login {},

    #[route("/register")]
    Register {},

    // First-run onboarding (Tauri desktop only, shown when no config saved).
    #[route("/setup")]
    Setup {},

    #[layout(Layout)]
        #[route("/")]
        Home {},

        #[route("/folder/:id")]
        Folder { id: String },

        #[route("/shares")]
        Shares {},

        #[route("/gallery")]
        Gallery {},

        #[route("/gallery/album/:id")]
        GalleryAlbum { id: String },

        #[route("/music")]
        Music {},

        #[route("/music/artist/:name")]
        MusicArtist { name: String },

        #[route("/music/album/:artist/:album")]
        MusicAlbum { artist: String, album: String },

        #[route("/music/folder/:id")]
        MusicFolder { id: String },

        #[route("/music/playlist/:id")]
        MusicPlaylist { id: String },

        #[route("/trash")]
        Trash {},

        #[route("/shopping")]
        Shopping {},

        #[route("/shopping/list/:id")]
        ShoppingList { id: String },

        #[route("/settings")]
        Settings {},
    #[end_layout]

    #[route("/share/:token")]
    PublicShare { token: String },
}

#[component]
fn Home() -> Element {
    rsx! {
        FileBrowser { parent_id: None }
    }
}

#[component]
fn Folder(id: String) -> Element {
    rsx! {
        FileBrowser { key: "{id}", parent_id: Some(id) }
    }
}

#[component]
fn Shares() -> Element {
    rsx! {
        div { class: "card bg-base-100 shadow",
            div { class: "card-body",
                h2 { class: "card-title", "My Shares" }
                p { class: "text-base-content/70", "Shared files and folders will appear here." }
            }
        }
    }
}

#[component]
fn MusicArtist(name: String) -> Element {
    let nav = use_navigator();
    rsx! {
        div { class: "p-4",
            MusicArtistView {
                name,
                on_back: move |_| { let _ = nav.push(Route::Music {}); },
                on_album_select: move |album: uncloud_common::MusicAlbumResponse| {
                    let _ = nav.push(Route::MusicAlbum {
                        artist: album.artist,
                        album: album.name,
                    });
                },
            }
        }
    }
}

#[component]
fn MusicAlbum(artist: String, album: String) -> Element {
    let nav = use_navigator();
    rsx! {
        div { class: "p-4",
            MusicAlbumViewComp {
                artist,
                album,
                on_back: move |_| { let _ = nav.push(Route::Music {}); },
            }
        }
    }
}

#[component]
fn MusicFolder(id: String) -> Element {
    rsx! {
        div { class: "p-4",
            MusicFolderView { key: "{id}", folder_id: id }
        }
    }
}

#[component]
fn MusicPlaylist(id: String) -> Element {
    rsx! {
        div { class: "p-4",
            MusicPlaylistView { key: "{id}", playlist_id: id }
        }
    }
}

#[component]
fn Shopping() -> Element {
    rsx! {
        shopping::ShoppingPage {}
    }
}

#[component]
fn ShoppingList(id: String) -> Element {
    rsx! {
        shopping::ShoppingListView { key: "{id}", list_id: id }
    }
}

#[component]
fn PublicShare(token: String) -> Element {
    rsx! {
        div { class: "flex items-center justify-center min-h-screen bg-base-200",
            div { class: "card bg-base-100 shadow-xl w-full max-w-sm",
                div { class: "card-body",
                    h1 { class: "card-title", "Shared File" }
                    p { class: "text-base-content/70", "Token: {token}" }
                }
            }
        }
    }
}
