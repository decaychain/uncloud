mod track_list;
mod artist_list;
mod album_grid;
mod artist_view;
mod album_view;
mod folder_view;
mod playlist_list;
mod playlist_panel;
pub mod playlist_view;
pub mod manage_categories;

use dioxus::prelude::*;
use uncloud_common::{
    ArtistResponse, MusicAlbumResponse, MusicCategory, PlaylistSummary, ServerEvent,
};

use crate::components::icons::{IconAlertTriangle, IconSearch, IconX};
use crate::hooks::{use_music, use_music_categories, use_playlists};
use crate::hooks::use_music::LibraryScope;
use crate::router::Route;

pub use album_view::AlbumView as MusicAlbumView;
pub use artist_view::ArtistView as MusicArtistView;
pub use folder_view::FolderView as MusicFolderView;
pub use playlist_panel::PlaylistSidePanel;
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
fn MetadataView(scope: LibraryScope, scope_label: Option<String>) -> Element {
    let nav = use_navigator();
    let mut nav_state: Signal<MetadataNav> = use_signal(|| MetadataNav::Artists);
    let mut artists: Signal<Vec<ArtistResponse>> = use_signal(Vec::new);
    let mut playlists: Signal<Vec<PlaylistSummary>> = use_signal(Vec::new);
    let mut categories: Signal<Vec<MusicCategory>> = use_signal(Vec::new);
    let mut filter: Signal<String> = use_signal(String::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut refresh = use_signal(|| 0u32);

    let sse_event = use_context::<Signal<Option<ServerEvent>>>();
    // Debounce CRUD-driven refreshes: coalesce bursts into one relist.
    let mut refresh_epoch = use_signal(|| 0u32);
    use_effect(move || {
        if let Some(event) = sse_event() {
            match event {
                ServerEvent::FileCreated { .. }
                | ServerEvent::FileDeleted { .. }
                | ServerEvent::FileUpdated { .. } => {
                    let epoch = *refresh_epoch.peek() + 1;
                    refresh_epoch.set(epoch);
                    spawn(async move {
                        gloo_timers::future::TimeoutFuture::new(150).await;
                        if *refresh_epoch.peek() == epoch {
                            let next = *refresh.peek() + 1;
                            refresh.set(next);
                        }
                    });
                }
                _ => {}
            }
        }
    });

    // Reset internal nav when the scope changes (e.g. user picks a new category
    // mid-browse) so we don't show a stale Artist sub-view.
    use_effect(use_reactive!(|scope| {
        let _ = scope;
        nav_state.set(MetadataNav::Artists);
        filter.set(String::new());
    }));

    let scope_for_effect = scope.clone();
    use_effect(use_reactive!(|(scope_for_effect, refresh)| {
        let _ = refresh;
        let scope = scope_for_effect;
        spawn(async move {
            // Don't flip `loading` on refresh — only the initial mount shows
            // the spinner (via use_signal(|| true)). Otherwise an SSE event
            // would cover any Artist/Album sub-view rendered below.
            error.set(None);
            let (artists_res, playlists_res, categories_res) = futures::join!(
                use_music::list_artists_scoped(&scope),
                use_playlists::list_playlists(),
                use_music_categories::list_categories(),
            );
            match artists_res {
                Ok(a) => artists.set(a),
                Err(e) => error.set(Some(e)),
            }
            if let Ok(p) = playlists_res {
                playlists.set(p);
            }
            if let Ok(c) = categories_res {
                categories.set(c);
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
                h3 { class: "text-lg font-semibold", "Error loading artists" }
                p { class: "text-base-content/60", "{err}" }
            }
        };
    }

    // Selected category id (when scope is Category(id)).
    let current_category_id = match &scope {
        LibraryScope::Category(id) => Some(id.clone()),
        _ => None,
    };
    let folder_scope_id = match &scope {
        LibraryScope::Folder(id) => Some(id.clone()),
        _ => None,
    };

    let scope_for_artist = scope.clone();
    let scope_for_album_back = scope.clone();
    let scope_for_album = scope.clone();

    rsx! {
        // Search + categories control row, only shown on the Artists view.
        if matches!(nav_state(), MetadataNav::Artists) {
            div { class: "flex flex-wrap items-center gap-3 mb-4",
                label { class: "input input-bordered input-sm flex items-center gap-2 flex-1 min-w-48 max-w-sm",
                    IconSearch { class: "w-4 h-4 opacity-60".to_string() }
                    input {
                        r#type: "search",
                        class: "grow",
                        placeholder: "Filter artists or albums…",
                        value: "{filter}",
                        oninput: move |e| filter.set(e.value()),
                    }
                }
                select {
                    class: "select select-bordered select-sm",
                    value: current_category_id.clone().unwrap_or_default(),
                    onchange: move |e| {
                        let v = e.value();
                        if v.is_empty() {
                            let _ = nav.push(Route::Music {});
                        } else {
                            let _ = nav.push(Route::MusicScopeCategory { id: v });
                        }
                    },
                    option { value: "", "All categories" }
                    for cat in categories() {
                        option {
                            value: "{cat.id}",
                            selected: current_category_id.as_deref() == Some(&cat.id),
                            "{cat.name}"
                        }
                    }
                }
                if let Some(label) = scope_label.clone() {
                    div { class: "badge badge-primary badge-outline gap-1 cursor-pointer",
                        onclick: move |_| { let _ = nav.push(Route::Music {}); },
                        span { "Scope: {label}" }
                        IconX { class: "w-3 h-3".to_string() }
                    }
                }
                if let Some(fid) = folder_scope_id.clone() {
                    Link {
                        to: Route::MusicFolder { id: fid },
                        class: "btn btn-ghost btn-sm",
                        "Browse files"
                    }
                }
            }
        }

        match nav_state() {
            MetadataNav::Artists => {
                let q = filter().to_lowercase();
                let filtered: Vec<ArtistResponse> = if q.is_empty() {
                    artists()
                } else {
                    artists().into_iter()
                        .filter(|a| a.name.to_lowercase().contains(&q))
                        .collect()
                };
                rsx! {
                    div { class: "space-y-6",
                        if matches!(scope, LibraryScope::All) {
                            playlist_list::PlaylistList { playlists: playlists() }
                        }
                        artist_list::ArtistList {
                            artists: filtered,
                            on_select: move |name: String| nav_state.set(MetadataNav::Artist(name)),
                        }
                    }
                }
            }
            MetadataNav::Artist(name) => {
                let name_clone = name.clone();
                rsx! {
                    MusicArtistView {
                        name,
                        scope: scope_for_artist.clone(),
                        on_back: move |_| nav_state.set(MetadataNav::Artists),
                        on_album_select: move |album: MusicAlbumResponse| {
                            nav_state.set(MetadataNav::Album(name_clone.clone(), album.name));
                        },
                    }
                }
            }
            MetadataNav::Album(artist, album) => {
                let scope_back = scope_for_album_back.clone();
                let scope_inner = scope_for_album.clone();
                rsx! {
                    MusicAlbumView {
                        artist,
                        album,
                        scope: scope_inner,
                        on_back: move |_| {
                            let _ = scope_back;
                            nav_state.set(MetadataNav::Artists);
                        },
                    }
                }
            }
        }
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
            MetadataView { scope: LibraryScope::All, scope_label: None }
        }
    }
}

#[component]
pub fn MusicScopeCategoryView(id: String) -> Element {
    let mut name: Signal<Option<String>> = use_signal(|| None);
    let id_for_effect = id.clone();
    use_effect(use_reactive!(|id_for_effect| {
        let i = id_for_effect;
        spawn(async move {
            if let Ok(cats) = use_music_categories::list_categories().await {
                if let Some(c) = cats.into_iter().find(|c| c.id == i) {
                    name.set(Some(c.name));
                }
            }
        });
    }));

    let label = name();
    rsx! {
        div { class: "p-4 space-y-4",
            div { class: "flex items-center justify-between",
                h1 { class: "text-2xl font-bold",
                    "Music"
                    if let Some(n) = &label {
                        span { class: "text-base-content/50 font-normal text-lg ml-2", " — {n}" }
                    }
                }
            }
            MetadataView { scope: LibraryScope::Category(id), scope_label: label }
        }
    }
}

#[component]
pub fn MusicScopeFolderView(id: String) -> Element {
    let mut folder_name: Signal<Option<String>> = use_signal(|| None);
    let id_for_effect = id.clone();
    use_effect(use_reactive!(|id_for_effect| {
        let i = id_for_effect;
        spawn(async move {
            if let Ok(folders) = use_music::list_music_folders().await {
                if let Some(f) = folders.into_iter().find(|f| f.folder_id == i) {
                    folder_name.set(Some(f.name));
                }
            }
        });
    }));

    let label = folder_name();
    rsx! {
        div { class: "p-4 space-y-4",
            div { class: "flex items-center justify-between",
                h1 { class: "text-2xl font-bold",
                    "Music"
                    if let Some(n) = &label {
                        span { class: "text-base-content/50 font-normal text-lg ml-2", " — {n}" }
                    }
                }
            }
            MetadataView { scope: LibraryScope::Folder(id), scope_label: label }
        }
    }
}
