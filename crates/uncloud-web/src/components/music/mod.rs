mod album_grid;
mod album_view;
mod artist_list;
mod artist_view;
mod folder_tree;
pub mod manage_categories;
mod playlist_list;
mod playlist_panel;
pub mod playlist_view;
mod track_list;

use dioxus::prelude::*;
use uncloud_common::{
    ArtistResponse, MusicAlbumResponse, MusicCategory, PlaylistSummary, ServerEvent,
};

use crate::components::icons::{IconAlertTriangle, IconFolder, IconMusic, IconSearch};
use crate::hooks::use_music::LibraryScope;
use crate::hooks::{use_music, use_music_categories, use_playlists};
use crate::router::Route;

pub use album_view::AlbumView as MusicAlbumView;
pub use artist_view::ArtistView as MusicArtistView;
pub use folder_tree::FolderTreeView;
pub use playlist_panel::PlaylistSidePanel;
pub use playlist_view::PlaylistView as MusicPlaylistView;

// ── Navigation state for "By Metadata" tab ─────────────────────────────────

#[derive(Clone, PartialEq)]
enum MetadataNav {
    Artists,
    Artist(String),
    Album(String, String),
}

#[derive(Clone, Copy, PartialEq)]
enum MusicMainView {
    Library,
    Folders,
}

// ── MetadataView ────────────────────────────────────────────────────────────

#[component]
fn MetadataView(scope: LibraryScope) -> Element {
    let nav = use_navigator();
    let mut nav_state: Signal<MetadataNav> = use_signal(|| MetadataNav::Artists);
    let mut artists: Signal<Vec<ArtistResponse>> = use_signal(Vec::new);
    let mut playlists: Signal<Vec<PlaylistSummary>> = use_signal(Vec::new);
    let mut categories: Signal<Vec<MusicCategory>> = use_signal(Vec::new);
    let mut filter: Signal<String> = use_signal(String::new);
    let mut search_results: Signal<Option<uncloud_common::MusicSearchResponse>> =
        use_signal(|| None);
    let mut search_loading = use_signal(|| false);
    let mut search_epoch: Signal<u32> = use_signal(|| 0);
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

    let cat_dirty = use_context::<Signal<crate::state::MusicCategoryDirtyTick>>();

    let scope_for_effect = scope.clone();
    use_effect(use_reactive!(|(scope_for_effect, refresh)| {
        let _ = refresh;
        let scope = scope_for_effect;
        spawn(async move {
            // Don't flip `loading` on refresh — only the initial mount shows
            // the spinner (via use_signal(|| true)). Otherwise an SSE event
            // would cover any Artist/Album sub-view rendered below.
            error.set(None);
            let (artists_res, playlists_res) = futures::join!(
                use_music::list_artists_scoped(&scope),
                use_playlists::list_playlists(),
            );
            match artists_res {
                Ok(a) => artists.set(a),
                Err(e) => error.set(Some(e)),
            }
            if let Ok(p) = playlists_res {
                playlists.set(p);
            }
            loading.set(false);
        });
    }));

    // Categories live on a separate dependency (cat_dirty) so creating /
    // renaming / removing a category from the folder-view modal updates the
    // dropdown without needing an SSE bump.
    use_effect(use_reactive!(|cat_dirty| {
        let _ = cat_dirty();
        spawn(async move {
            if let Ok(c) = use_music_categories::list_categories().await {
                categories.set(c);
            }
        });
    }));

    // Debounced cross-entity search. When the filter is non-empty, fetch
    // matching artists/albums/tracks from the server; clear when empty.
    let q_for_effect = filter();
    let scope_for_search = scope.clone();
    use_effect(use_reactive!(|(q_for_effect, scope_for_search)| {
        let q = q_for_effect.trim().to_string();
        if q.is_empty() {
            search_results.set(None);
            search_loading.set(false);
            return;
        }
        search_loading.set(true);
        let next_epoch = *search_epoch.peek() + 1;
        search_epoch.set(next_epoch);
        let scope = scope_for_search.clone();
        spawn(async move {
            gloo_timers::future::TimeoutFuture::new(200).await;
            // Stale-request guard: only the latest typed query wins.
            if *search_epoch.peek() != next_epoch {
                return;
            }
            match use_music::search_music(&q, &scope, None).await {
                Ok(r) => search_results.set(Some(r)),
                Err(_) => {}
            }
            search_loading.set(false);
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

    // Dropdown selection. For Category(id) scope it's the active category.
    // For Folder(id) scope, surface the (alphabetically) first category that
    // contains the folder so the dropdown reflects what the user is browsing
    // — most folders only ever belong to a single category, so this matches
    // the user's mental model in the common case.
    let current_category_id = match &scope {
        LibraryScope::Category(id) => Some(id.clone()),
        LibraryScope::Folder(fid) => {
            let mut matches: Vec<MusicCategory> = categories()
                .into_iter()
                .filter(|c| c.folder_ids.iter().any(|f| f == fid))
                .collect();
            matches.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            matches.into_iter().next().map(|c| c.id)
        }
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
                label { class: "input input-bordered input-sm flex items-center gap-2 flex-1 min-w-48",
                    if search_loading() {
                        span { class: "loading loading-spinner loading-xs opacity-60" }
                    } else {
                        IconSearch { class: "w-4 h-4 opacity-60".to_string() }
                    }
                    input {
                        r#type: "search",
                        class: "grow",
                        placeholder: "Search artists, albums, tracks…",
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
                // Only switch to the SearchResults layout once the first
                // response has arrived. While the user is mid-keystroke
                // (200 ms debounce + network), we keep the artist grid up so
                // the page doesn't collapse to a spinner and back.
                let has_results = !filter().trim().is_empty() && search_results().is_some();
                if has_results {
                    rsx! {
                        SearchResults {
                            results: search_results(),
                            on_artist_select: move |name: String| {
                                nav_state.set(MetadataNav::Artist(name));
                            },
                            on_album_select: move |a: MusicAlbumResponse| {
                                nav_state.set(MetadataNav::Album(a.artist, a.name));
                            },
                        }
                    }
                } else {
                    rsx! {
                        div { class: "space-y-6",
                            if matches!(scope, LibraryScope::All) {
                                playlist_list::PlaylistList { playlists: playlists() }
                            }
                            artist_list::ArtistList {
                                artists: artists(),
                                on_select: move |name: String| nav_state.set(MetadataNav::Artist(name)),
                            }
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
    let mut view = use_signal(|| MusicMainView::Library);
    let current_view = view();

    rsx! {
        div { class: "p-3 space-y-3 sm:p-4 sm:space-y-4",
            div { class: "flex flex-wrap items-center justify-between gap-3",
                h1 { class: "text-2xl font-bold", "Music" }
                div { class: "join",
                    button {
                        class: if current_view == MusicMainView::Library {
                            "btn btn-sm join-item btn-primary"
                        } else {
                            "btn btn-sm join-item"
                        },
                        onclick: move |_| view.set(MusicMainView::Library),
                        IconMusic { class: "w-4 h-4".to_string() }
                        "Library"
                    }
                    button {
                        class: if current_view == MusicMainView::Folders {
                            "btn btn-sm join-item btn-primary"
                        } else {
                            "btn btn-sm join-item"
                        },
                        onclick: move |_| view.set(MusicMainView::Folders),
                        IconFolder { class: "w-4 h-4".to_string() }
                        "Folders"
                    }
                }
            }
            if current_view == MusicMainView::Folders {
                FolderTreeView { root_folder_id: None }
            } else {
                MetadataView { scope: LibraryScope::All }
            }
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
            MetadataView { scope: LibraryScope::Category(id) }
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
            MetadataView { scope: LibraryScope::Folder(id) }
        }
    }
}

// ── SearchResults ───────────────────────────────────────────────────────────

#[component]
fn SearchResults(
    results: Option<uncloud_common::MusicSearchResponse>,
    on_artist_select: EventHandler<String>,
    on_album_select: EventHandler<MusicAlbumResponse>,
) -> Element {
    use crate::hooks::use_player;
    use crate::state::PlayerState;
    use album_grid::AlbumGrid;
    use track_list::TrackList;

    let player = use_context::<Signal<PlayerState>>();

    // Caller renders this only after the first response has arrived, so
    // `results` should always be Some here. We treat None defensively as
    // a no-op rather than a spinner — that keeps the layout shape stable.
    let Some(r) = results else {
        return rsx! { div {} };
    };

    let no_hits = r.artists.is_empty() && r.albums.is_empty() && r.tracks.is_empty();
    if no_hits {
        return rsx! {
            div { class: "flex flex-col items-center justify-center py-16 gap-2 text-base-content/60",
                p { class: "text-lg", "No matches" }
                p { class: "text-sm", "Try a different search term." }
            }
        };
    }

    let artists_extra = r.total_artists.saturating_sub(r.artists.len());
    let albums_extra = r.total_albums.saturating_sub(r.albums.len());
    let tracks_extra = r.total_tracks.saturating_sub(r.tracks.len());
    let tracks_for_play = r.tracks.clone();

    rsx! {
        div { class: "space-y-8",
            // ── Artists ─────────
            if !r.artists.is_empty() {
                div { class: "space-y-2",
                    h2 { class: "text-lg font-semibold", "Artists" }
                    div { class: "grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 gap-4",
                        for artist in r.artists.iter() {
                            {
                                let name = artist.name.clone();
                                let name_click = artist.name.clone();
                                let album_count = artist.album_count;
                                let track_count = artist.track_count;
                                let first_letter = name.chars().next().unwrap_or('?').to_uppercase().to_string();
                                rsx! {
                                    div {
                                        class: "card bg-base-100 shadow-sm border border-base-300 cursor-pointer hover:shadow-md hover:ring-2 hover:ring-primary transition-all",
                                        onclick: move |_| on_artist_select.call(name_click.clone()),
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
                    if artists_extra > 0 {
                        p { class: "text-xs text-base-content/50", "+{artists_extra} more artists — refine your search to narrow." }
                    }
                }
            }

            // ── Albums ─────────
            if !r.albums.is_empty() {
                div { class: "space-y-2",
                    h2 { class: "text-lg font-semibold", "Albums" }
                    AlbumGrid {
                        albums: r.albums.clone(),
                        on_select: on_album_select,
                        on_play: None,
                    }
                    if albums_extra > 0 {
                        p { class: "text-xs text-base-content/50", "+{albums_extra} more albums — refine your search to narrow." }
                    }
                }
            }

            // ── Tracks ─────────
            if !r.tracks.is_empty() {
                div { class: "space-y-2",
                    h2 { class: "text-lg font-semibold", "Tracks" }
                    TrackList {
                        tracks: r.tracks.clone(),
                        show_artist: true,
                        show_album: true,
                        on_play: move |idx: usize| {
                            use_player::play_queue(player, tracks_for_play.clone(), idx);
                        },
                    }
                    if tracks_extra > 0 {
                        p { class: "text-xs text-base-content/50", "+{tracks_extra} more tracks — refine your search to narrow." }
                    }
                }
            }
        }
    }
}
