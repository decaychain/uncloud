use dioxus::prelude::*;
use wasm_bindgen::JsCast;
use uncloud_common::{AlbumResponse, MusicFolderResponse, PlaylistSummary};
use crate::hooks::{use_apps, use_files, use_music, use_playlists};
use crate::hooks::use_apps::AppEntry;
use crate::router::Route;
use crate::state::AuthState;

const LOGO: Asset = asset!("/assets/favicon-32.png");

/// Close the drawer on mobile by unchecking the toggle checkbox.
fn close_drawer() {
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        if let Ok(Some(el)) = doc.query_selector("#main-sidebar") {
            if let Some(input) = el.dyn_ref::<web_sys::HtmlInputElement>() {
                input.set_checked(false);
            }
        }
    }
}

#[component]
pub fn Sidebar() -> Element {
    let route = use_route::<Route>();

    let section = if matches!(route, Route::Gallery {} | Route::GalleryAlbum { .. }) {
        "gallery"
    } else if matches!(route, Route::Music {} | Route::MusicArtist { .. } | Route::MusicAlbum { .. } | Route::MusicFolder { .. } | Route::MusicPlaylist { .. }) {
        "music"
    } else if matches!(route, Route::Shopping {} | Route::ShoppingList { .. }) {
        "shopping"
    } else if matches!(route, Route::Passwords {}) {
        "passwords"
    } else if matches!(route, Route::Settings {} | Route::SettingsTab { .. }) {
        "settings"
    } else {
        "files"
    };

    let auth_state = use_context::<Signal<AuthState>>();
    let shopping_enabled = auth_state()
        .user
        .as_ref()
        .map(|u| u.features_enabled.contains(&"shopping".to_string()))
        .unwrap_or(false);

    rsx! {
        aside { class: "min-h-full w-64 bg-base-200 flex flex-col",
            // Logo
            div { class: "flex items-center gap-2 px-4 py-4 border-b border-base-300",
                img { src: LOGO, alt: "Uncloud", class: "w-7 h-7" }
                span { class: "text-xl font-bold", "Uncloud" }
            }

            // Top-level section nav
            ul { class: "menu menu-sm px-2 pt-2 pb-0 w-full",
                li {
                    Link {
                        to: Route::Home {},
                        class: if section == "files" { "active" } else { "" },
                        onclick: move |_| close_drawer(),
                        "📁 Files"
                    }
                }
                li {
                    Link {
                        to: Route::Gallery {},
                        class: if section == "gallery" { "active" } else { "" },
                        onclick: move |_| close_drawer(),
                        "🖼 Gallery"
                    }
                }
                li {
                    Link {
                        to: Route::Music {},
                        class: if section == "music" { "active" } else { "" },
                        onclick: move |_| close_drawer(),
                        "🎵 Music"
                    }
                }
                if shopping_enabled {
                    li {
                        Link {
                            to: Route::Shopping {},
                            class: if section == "shopping" { "active" } else { "" },
                            onclick: move |_| close_drawer(),
                            "🛒 Shopping"
                        }
                    }
                }
                li {
                    Link {
                        to: Route::Passwords {},
                        class: if section == "passwords" { "active" } else { "" },
                        onclick: move |_| close_drawer(),
                        "🔑 Passwords"
                    }
                }
                li {
                    Link {
                        to: Route::Settings {},
                        class: if section == "settings" { "active" } else { "" },
                        onclick: move |_| close_drawer(),
                        "⚙ Settings"
                    }
                }
            }

            div { class: "divider my-0 mx-3" }

            div { class: "flex-1 overflow-y-auto",
                ul { class: "menu menu-md p-2 w-full",
                    match section {
                        "gallery" => rsx! {
                            li { class: "menu-title", span { "Gallery" } }
                            li {
                                Link {
                                    to: Route::Gallery {},
                                    class: if matches!(route, Route::Gallery {}) { "active" } else { "" },
                                    "🖼 Timeline"
                                }
                            }
                            GallerySidebarAlbums {}
                        },
                        "music" => rsx! {
                            li { class: "menu-title", span { "Music" } }
                            li {
                                Link {
                                    to: Route::Music {},
                                    class: if matches!(route, Route::Music {}) { "active" } else { "" },
                                    "🎵 Library"
                                }
                            }
                            MusicSidebarPlaylists {}
                            MusicSidebarFolders {}
                        },
                        "shopping" => rsx! {
                            li { class: "menu-title", span { "Shopping" } }
                            li {
                                Link {
                                    to: Route::Shopping {},
                                    class: if matches!(route, Route::Shopping {}) { "active" } else { "" },
                                    "🛒 All Lists"
                                }
                            }
                        },
                        "passwords" => rsx! {
                            li { class: "menu-title", span { "Passwords" } }
                            li {
                                Link {
                                    to: Route::Passwords {},
                                    class: "active",
                                    "🔑 Vault"
                                }
                            }
                        },
                        "settings" => {
                            let active_tab = if let Route::SettingsTab { ref tab } = route {
                                tab.as_str().to_string()
                            } else {
                                "account".to_string()
                            };
                            let is_admin = auth_state().is_admin();
                            rsx! {
                                li { class: "menu-title", span { "Settings" } }
                                li {
                                    Link {
                                        to: Route::SettingsTab { tab: "account".to_string() },
                                        class: if active_tab == "account" { "active" } else { "" },
                                        onclick: move |_| close_drawer(),
                                        "👤 Account"
                                    }
                                }
                                li {
                                    Link {
                                        to: Route::SettingsTab { tab: "preferences".to_string() },
                                        class: if active_tab == "preferences" { "active" } else { "" },
                                        onclick: move |_| close_drawer(),
                                        "🎨 Preferences"
                                    }
                                }
                                if is_admin {
                                    li {
                                        Link {
                                            to: Route::SettingsTab { tab: "users".to_string() },
                                            class: if active_tab == "users" { "active" } else { "" },
                                            onclick: move |_| close_drawer(),
                                            "👥 Users"
                                        }
                                    }
                                    li {
                                        Link {
                                            to: Route::SettingsTab { tab: "admin".to_string() },
                                            class: if active_tab == "admin" { "active" } else { "" },
                                            onclick: move |_| close_drawer(),
                                            "🛡 Admin"
                                        }
                                    }
                                }
                            }
                        },
                        _ => rsx! {
                            li { class: "menu-title", span { "Files" } }
                            li {
                                Link {
                                    to: Route::Home {},
                                    class: if matches!(route, Route::Home {} | Route::Folder { .. }) { "active" } else { "" },
                                    "📁 All Files"
                                }
                            }
                            li {
                                Link {
                                    to: Route::Shares {},
                                    class: if matches!(route, Route::Shares {}) { "active" } else { "" },
                                    "🔗 Shares"
                                }
                            }
                            li {
                                Link {
                                    to: Route::Trash {},
                                    class: if matches!(route, Route::Trash {}) { "active" } else { "" },
                                    "🗑️ Trash"
                                }
                            }
                            SidebarApps {}
                        },
                    }
                }
            }

            // Storage usage at the bottom
            div { class: "border-t border-base-300 p-3",
                StorageUsage {}
            }
        }
    }
}

#[component]
fn SidebarApps() -> Element {
    let mut apps: Signal<Vec<AppEntry>> = use_signal(Vec::new);

    use_effect(move || {
        spawn(async move {
            if let Ok(a) = use_apps::list_apps().await {
                apps.set(a);
            }
        });
    });

    let app_list = apps();
    if app_list.is_empty() {
        return rsx! {};
    }

    rsx! {
        li { class: "menu-title mt-2", span { "Apps" } }
        for app in app_list {
            {
                let app_name = app.name.clone();
                let app_label = app.nav_label.clone();
                let app_icon = app.icon.clone();
                rsx! {
                    li {
                        a {
                            href: "/apps/{app_name}/",
                            // Force a full browser navigation — Dioxus's router must not
                            // intercept this, as /apps/* is proxied content, not a Dioxus route.
                            onclick: move |evt| {
                                evt.prevent_default();
                                if let Some(window) = web_sys::window() {
                                    let _ = window.location().set_href(&format!("/apps/{}/", app_name));
                                }
                            },
                            "{app_icon} {app_label}"
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn StorageUsage() -> Element {
    let auth_state = use_context::<Signal<AuthState>>();

    let (used, quota) = auth_state()
        .user
        .as_ref()
        .map(|u| (u.used_bytes, u.quota_bytes))
        .unwrap_or((0, None));

    let used_str = uncloud_common::validation::format_bytes(used);

    rsx! {
        div { class: "text-xs",
            div { class: "font-semibold opacity-60 mb-1", "Storage" }
            if let Some(quota) = quota {
                {
                    let quota_str = uncloud_common::validation::format_bytes(quota);
                    let percentage = if quota > 0 {
                        (used as f64 / quota as f64 * 100.0).min(100.0)
                    } else {
                        0.0
                    };
                    rsx! {
                        progress {
                            class: "progress progress-primary w-full",
                            value: "{percentage}",
                            max: "100",
                        }
                        div { class: "mt-1 opacity-70", "{used_str} / {quota_str}" }
                    }
                }
            } else {
                div { class: "opacity-70", "{used_str} used" }
            }
        }
    }
}

/// Flatten `albums` into DFS order. Each entry is `(album, depth)`.
/// Top-level albums are those whose `parent_folder_id` is not itself an album.
fn flatten_album_tree(albums: &[AlbumResponse]) -> Vec<(AlbumResponse, usize)> {
    let album_ids: std::collections::HashSet<&str> =
        albums.iter().map(|a| a.folder_id.as_str()).collect();

    fn dfs(
        albums: &[AlbumResponse],
        parent: Option<&str>,
        album_ids: &std::collections::HashSet<&str>,
        depth: usize,
        out: &mut Vec<(AlbumResponse, usize)>,
    ) {
        let mut children: Vec<&AlbumResponse> = albums
            .iter()
            .filter(|a| {
                let effective_parent = a
                    .parent_folder_id
                    .as_deref()
                    .filter(|pid| album_ids.contains(pid));
                effective_parent == parent
            })
            .collect();
        children.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        for child in children {
            out.push((child.clone(), depth));
            dfs(albums, Some(&child.folder_id), album_ids, depth + 1, out);
        }
    }

    let mut result = Vec::new();
    dfs(albums, None, &album_ids, 0, &mut result);
    result
}

#[component]
fn GallerySidebarAlbums() -> Element {
    let mut albums: Signal<Vec<AlbumResponse>> = use_signal(Vec::new);
    let route = use_route::<Route>();

    use_effect(move || {
        spawn(async move {
            if let Ok(a) = use_files::list_gallery_albums().await {
                albums.set(a);
            }
        });
    });

    let album_list = albums();
    if album_list.is_empty() {
        return rsx! {};
    }

    let flattened = flatten_album_tree(&album_list);

    rsx! {
        li { class: "menu-title mt-2", span { "Albums" } }
        for (album, depth) in flattened {
            {
                let album_id = album.folder_id.clone();
                let is_active = matches!(&route, Route::GalleryAlbum { id } if *id == album_id);
                let indent_px = depth * 12;
                rsx! {
                    li {
                        Link {
                            to: Route::GalleryAlbum { id: album.folder_id.clone() },
                            class: if is_active { "active" } else { "" },
                            style: "padding-left: calc(0.75rem + {indent_px}px)",
                            if depth > 0 {
                                span { class: "opacity-30 mr-1 text-xs", "└" }
                            }
                            "📁 {album.name}"
                        }
                    }
                }
            }
        }
    }
}

/// Flatten `folders` into DFS order for the music sidebar.
fn flatten_music_folder_tree(folders: &[MusicFolderResponse]) -> Vec<(MusicFolderResponse, usize)> {
    let folder_ids: std::collections::HashSet<&str> =
        folders.iter().map(|f| f.folder_id.as_str()).collect();

    fn dfs(
        folders: &[MusicFolderResponse],
        parent: Option<&str>,
        folder_ids: &std::collections::HashSet<&str>,
        depth: usize,
        out: &mut Vec<(MusicFolderResponse, usize)>,
    ) {
        let mut children: Vec<&MusicFolderResponse> = folders
            .iter()
            .filter(|f| {
                let effective_parent = f
                    .parent_folder_id
                    .as_deref()
                    .filter(|pid| folder_ids.contains(pid));
                effective_parent == parent
            })
            .collect();
        children.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        for child in children {
            out.push((child.clone(), depth));
            dfs(folders, Some(&child.folder_id), folder_ids, depth + 1, out);
        }
    }

    let mut result = Vec::new();
    dfs(folders, None, &folder_ids, 0, &mut result);
    result
}

/// Returns the set of folder IDs that should start collapsed given an expand depth.
/// `depth=0` means all parent folders collapsed; `depth=1` expands only the top level, etc.
/// `depth=u32::MAX as usize` (or any very large value) means expand everything.
fn initial_collapsed(
    folders: &[MusicFolderResponse],
    expand_depth: usize,
) -> std::collections::HashSet<String> {
    let has_children: std::collections::HashSet<String> = folders
        .iter()
        .filter_map(|f| f.parent_folder_id.clone())
        .collect();
    let flattened = flatten_music_folder_tree(folders);
    flattened
        .into_iter()
        .filter(|(f, depth)| has_children.contains(&f.folder_id) && *depth >= expand_depth)
        .map(|(f, _)| f.folder_id)
        .collect()
}

#[component]
fn MusicSidebarFolders() -> Element {
    let mut folders_sig: Signal<Vec<MusicFolderResponse>> = use_signal(Vec::new);
    // Set of folder IDs that have been manually collapsed by the user.
    let mut collapsed: Signal<std::collections::HashSet<String>> =
        use_signal(std::collections::HashSet::new);
    let route = use_route::<Route>();
    let expand_depth = use_context::<Signal<u32>>();

    // Load folders; initialize collapsed from the depth preference.
    use_effect(move || {
        spawn(async move {
            if let Ok(f) = use_music::list_music_folders().await {
                let depth = *expand_depth.peek() as usize;
                let init = initial_collapsed(&f, depth);
                folders_sig.set(f);
                collapsed.set(init);
            }
        });
    });

    // When the depth preference changes, reset collapsed to match the new setting.
    // Manual toggles made during this session are discarded — that's intentional.
    use_effect(use_reactive!(|(expand_depth)| {
        let f = folders_sig.peek().clone();
        if !f.is_empty() {
            collapsed.set(initial_collapsed(&f, expand_depth() as usize));
        }
    }));

    let folders = folders_sig();
    if folders.is_empty() {
        return rsx! {};
    }

    let active_id: Option<String> = if let Route::MusicFolder { id } = &route {
        Some(id.clone())
    } else {
        None
    };

    // When the active folder changes, auto-expand all its ancestors.
    use_effect(use_reactive!(|(active_id)| {
        if let Some(aid) = active_id {
            let fs = folders_sig.peek().clone();
            let parent_of: std::collections::HashMap<String, String> = fs
                .iter()
                .filter_map(|f| f.parent_folder_id.clone().map(|p| (f.folder_id.clone(), p)))
                .collect();
            let mut cur = aid;
            let mut c = collapsed.write();
            while let Some(pid) = parent_of.get(&cur).cloned() {
                c.remove(&pid);
                cur = pid;
            }
        }
    }));

    // Folders that are the parent of at least one other folder.
    let has_children: std::collections::HashSet<String> = folders
        .iter()
        .filter_map(|f| f.parent_folder_id.clone())
        .collect();

    // child → parent lookup for visibility check.
    let parent_of: std::collections::HashMap<String, String> = folders
        .iter()
        .filter_map(|f| f.parent_folder_id.clone().map(|p| (f.folder_id.clone(), p)))
        .collect();

    let col = collapsed();
    let flattened = flatten_music_folder_tree(&folders);

    rsx! {
        li { class: "menu-title mt-2", span { "Folders" } }
        for (folder, depth) in flattened {
            {
                // Skip this item if any ancestor is collapsed.
                let mut visible = true;
                let mut cur = folder.folder_id.clone();
                while let Some(pid) = parent_of.get(&cur).cloned() {
                    if col.contains(&pid) { visible = false; break; }
                    cur = pid;
                }

                if !visible {
                    rsx! {}
                } else {
                    let fid = folder.folder_id.clone();
                    let fid_toggle = folder.folder_id.clone();
                    let is_active = active_id.as_deref() == Some(&fid);
                    let is_parent = has_children.contains(&fid);
                    let is_collapsed = col.contains(&fid);
                    let indent_px = depth * 12;

                    rsx! {
                        li {
                            div {
                                class: if is_active {
                                    "flex items-center gap-0.5 rounded-md bg-base-300"
                                } else {
                                    "flex items-center gap-0.5 rounded-md hover:bg-base-200"
                                },
                                style: "padding-left: calc(0.35rem + {indent_px}px); padding-right: 0.25rem; padding-top: 0.15rem; padding-bottom: 0.15rem; min-width: 0",
                                // Expand / collapse toggle for folders that have children.
                                if is_parent {
                                    button {
                                        class: "btn btn-ghost btn-xs btn-circle flex-shrink-0 opacity-40 hover:opacity-100",
                                        onclick: move |_| {
                                            let mut c = collapsed.write();
                                            if c.contains(&fid_toggle) {
                                                c.remove(&fid_toggle);
                                            } else {
                                                c.insert(fid_toggle.clone());
                                            }
                                        },
                                        if is_collapsed { "▶" } else { "▼" }
                                    }
                                } else {
                                    // Spacer so leaf folders align with siblings.
                                    span { class: "w-5 flex-shrink-0" }
                                }
                                Link {
                                    to: Route::MusicFolder { id: folder.folder_id.clone() },
                                    class: if is_active {
                                        "flex-1 text-sm font-medium break-words min-w-0"
                                    } else {
                                        "flex-1 text-sm break-words min-w-0"
                                    },
                                    "📁 {folder.name}"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn MusicSidebarPlaylists() -> Element {
    let mut playlists: Signal<Vec<PlaylistSummary>> = use_signal(Vec::new);
    let mut refresh = use_signal(|| 0u32);
    let route = use_route::<Route>();
    let nav = use_navigator();

    // Create modal state
    let mut show_create: Signal<bool> = use_signal(|| false);
    let mut new_name: Signal<String> = use_signal(|| String::new());
    let mut create_error: Signal<Option<String>> = use_signal(|| None);

    // Rename modal state: Some((id, current_name))
    let mut rename_target: Signal<Option<(String, String)>> = use_signal(|| None);
    let mut rename_name: Signal<String> = use_signal(|| String::new());
    let mut rename_error: Signal<Option<String>> = use_signal(|| None);

    // Delete confirm state: Some((id, name))
    let mut delete_target: Signal<Option<(String, String)>> = use_signal(|| None);

    use_effect(move || {
        let _ = refresh();
        spawn(async move {
            if let Ok(p) = use_playlists::list_playlists().await {
                playlists.set(p);
            }
        });
    });

    rsx! {
        li { class: "menu-title mt-2", span { "Playlists" } }
        for pl in playlists() {
            {
                let pl_id = pl.id.clone();
                let pl_name = pl.name.clone();
                let pl_id_rename = pl.id.clone();
                let pl_name_rename = pl.name.clone();
                let pl_id_delete = pl.id.clone();
                let pl_name_delete = pl.name.clone();
                let is_active = matches!(&route, Route::MusicPlaylist { id } if *id == pl_id);
                rsx! {
                    li { class: "group",
                        Link {
                            to: Route::MusicPlaylist { id: pl.id.clone() },
                            class: if is_active { "active" } else { "" },
                            span { class: "flex-1 truncate", "🎶 {pl_name}" }
                            // Rename + delete buttons — inside the <a> so they sit on the same line;
                            // stop_propagation prevents the link from firing on button click.
                            div { class: "flex gap-0 opacity-0 group-hover:opacity-100 transition-opacity ml-auto shrink-0",
                                button {
                                    class: "btn btn-ghost btn-xs btn-circle",
                                    title: "Rename",
                                    onclick: move |evt| {
                                        evt.stop_propagation();
                                        rename_name.set(pl_name_rename.clone());
                                        rename_error.set(None);
                                        rename_target.set(Some((pl_id_rename.clone(), pl_name_rename.clone())));
                                    },
                                    svg { class: "w-3 h-3", fill: "none", stroke: "currentColor", view_box: "0 0 24 24",
                                        path { stroke_linecap: "round", stroke_linejoin: "round", stroke_width: "2",
                                            d: "M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z"
                                        }
                                    }
                                }
                                button {
                                    class: "btn btn-ghost btn-xs btn-circle text-error",
                                    title: "Delete",
                                    onclick: move |evt| {
                                        evt.stop_propagation();
                                        delete_target.set(Some((pl_id_delete.clone(), pl_name_delete.clone())));
                                    },
                                    svg { class: "w-3 h-3", fill: "none", stroke: "currentColor", view_box: "0 0 24 24",
                                        path { stroke_linecap: "round", stroke_linejoin: "round", stroke_width: "2",
                                            d: "M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        li {
            a {
                class: "text-base-content/50 hover:text-base-content",
                onclick: move |_| {
                    new_name.set(String::new());
                    create_error.set(None);
                    show_create.set(true);
                },
                "+ New playlist"
            }
        }

        // Create modal
        if show_create() {
            div { class: "modal modal-open",
                div { class: "modal-box",
                    h3 { class: "font-bold text-lg mb-4", "New Playlist" }
                    if let Some(err) = create_error() {
                        div { class: "alert alert-error mb-3 text-sm", "{err}" }
                    }
                    input {
                        class: "input input-bordered w-full",
                        r#type: "text",
                        placeholder: "Playlist name",
                        value: "{new_name}",
                        oninput: move |e| new_name.set(e.value()),
                    }
                    div { class: "modal-action",
                        button {
                            class: "btn",
                            onclick: move |_| show_create.set(false),
                            "Cancel"
                        }
                        button {
                            class: "btn btn-primary",
                            disabled: new_name().trim().is_empty(),
                            onclick: move |_| {
                                let name = new_name().trim().to_string();
                                spawn(async move {
                                    match use_playlists::create_playlist(&name, None).await {
                                        Ok(summary) => {
                                            show_create.set(false);
                                            let next = *refresh.peek() + 1;
                                            refresh.set(next);
                                            nav.push(Route::MusicPlaylist { id: summary.id });
                                        }
                                        Err(e) => {
                                            if e == "CONFLICT" {
                                                create_error.set(Some(format!("A playlist named \"{}\" already exists", name)));
                                            } else {
                                                create_error.set(Some(e));
                                            }
                                        }
                                    }
                                });
                            },
                            "Create"
                        }
                    }
                }
                div { class: "modal-backdrop", onclick: move |_| show_create.set(false) }
            }
        }

        // Rename modal
        if let Some((ref target_id, _)) = rename_target() {
            {
                let tid = target_id.clone();
                rsx! {
                    div { class: "modal modal-open",
                        div { class: "modal-box",
                            h3 { class: "font-bold text-lg mb-4", "Rename Playlist" }
                            if let Some(err) = rename_error() {
                                div { class: "alert alert-error mb-3 text-sm", "{err}" }
                            }
                            input {
                                class: "input input-bordered w-full",
                                r#type: "text",
                                placeholder: "Playlist name",
                                value: "{rename_name}",
                                oninput: move |e| rename_name.set(e.value()),
                            }
                            div { class: "modal-action",
                                button {
                                    class: "btn",
                                    onclick: move |_| rename_target.set(None),
                                    "Cancel"
                                }
                                button {
                                    class: "btn btn-primary",
                                    disabled: rename_name().trim().is_empty(),
                                    onclick: move |_| {
                                        let name = rename_name().trim().to_string();
                                        let id = tid.clone();
                                        spawn(async move {
                                            match use_playlists::update_playlist(&id, Some(&name), None).await {
                                                Ok(_) => {
                                                    rename_target.set(None);
                                                    let next = *refresh.peek() + 1;
                                                    refresh.set(next);
                                                }
                                                Err(e) => {
                                                    if e == "CONFLICT" {
                                                        rename_error.set(Some(format!("A playlist named \"{}\" already exists", name)));
                                                    } else {
                                                        rename_error.set(Some(e));
                                                    }
                                                }
                                            }
                                        });
                                    },
                                    "Rename"
                                }
                            }
                        }
                        div { class: "modal-backdrop", onclick: move |_| rename_target.set(None) }
                    }
                }
            }
        }

        // Delete confirm modal
        if let Some((ref del_id, ref del_name)) = delete_target() {
            {
                let did = del_id.clone();
                let dname = del_name.clone();
                let viewing_deleted = matches!(&route, Route::MusicPlaylist { id } if id == del_id);
                rsx! {
                    div { class: "modal modal-open",
                        div { class: "modal-box",
                            h3 { class: "font-bold text-lg mb-2", "Delete Playlist" }
                            p { class: "text-base-content/70",
                                "Delete \"{dname}\"? This cannot be undone. Tracks are not deleted."
                            }
                            div { class: "modal-action",
                                button {
                                    class: "btn",
                                    onclick: move |_| delete_target.set(None),
                                    "Cancel"
                                }
                                button {
                                    class: "btn btn-error",
                                    onclick: move |_| {
                                        let id = did.clone();
                                        spawn(async move {
                                            let _ = use_playlists::delete_playlist(&id).await;
                                            delete_target.set(None);
                                            let next = *refresh.peek() + 1;
                                            refresh.set(next);
                                            if viewing_deleted {
                                                nav.push(Route::Music {});
                                            }
                                        });
                                    },
                                    "Delete"
                                }
                            }
                        }
                        div { class: "modal-backdrop", onclick: move |_| delete_target.set(None) }
                    }
                }
            }
        }
    }
}
