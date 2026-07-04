use crate::components::icons::{
    IconCheckSquare, IconCopy, IconFileText, IconFolder, IconHistory, IconImage, IconKey,
    IconLayoutGrid, IconLink, IconList, IconListMusic, IconMail, IconMusic, IconPalette,
    IconRefreshCw, IconSettings, IconShield, IconShoppingCart, IconTrash, IconUser, IconUsers,
    IconWallet,
};
use crate::hooks::tauri as tauri_hook;
use crate::hooks::use_apps::AppEntry;
use crate::hooks::{use_apps, use_files, use_mail, use_music, use_playlists, use_tasks};
use crate::router::Route;
use crate::state::{AuthState, MailAccountDirtyTick, PlaylistDirtyTick};
use dioxus::prelude::*;
use uncloud_common::{
    AlbumResponse, MailAccountResponse, MusicFolderResponse, PlaylistSummary, TaskProjectResponse,
};
use wasm_bindgen::JsCast;

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

    let raw_section = if matches!(route, Route::Dashboard {}) {
        "dashboard"
    } else if matches!(route, Route::Gallery {} | Route::GalleryAlbum { .. }) {
        "gallery"
    } else if matches!(
        route,
        Route::Music {}
            | Route::MusicArtist { .. }
            | Route::MusicAlbum { .. }
            | Route::MusicFolder { .. }
            | Route::MusicScopeFolder { .. }
            | Route::MusicScopeCategory { .. }
            | Route::MusicPlaylist { .. },
    ) {
        "music"
    } else if matches!(
        route,
        Route::Tasks {} | Route::TasksAssigned {} | Route::TasksProject { .. }
    ) {
        "tasks"
    } else if matches!(route, Route::Shopping {} | Route::ShoppingList { .. }) {
        "shopping"
    } else if matches!(route, Route::Mail {} | Route::MailAccount { .. }) {
        "mail"
    } else if matches!(
        route,
        Route::Finance {}
            | Route::FinanceAccounts {}
            | Route::FinanceCategories {}
            | Route::FinanceSettlements {}
            | Route::FinanceSettlementDetail { .. }
            | Route::FinanceSchemas {}
            | Route::FinanceImports {}
            | Route::FinanceRules {},
    ) {
        "finance"
    } else if matches!(route, Route::Passwords {}) {
        "passwords"
    } else if matches!(route, Route::Settings {} | Route::SettingsTab { .. }) {
        "settings"
    } else {
        "files"
    };

    let auth_state = use_context::<Signal<AuthState>>();
    let tasks_enabled = auth_state().feature_enabled("tasks");
    let shopping_enabled = auth_state().feature_enabled("shopping");
    let finance_enabled = auth_state().feature_enabled("finance");
    let mail_enabled = auth_state().feature_enabled("mail");
    let music_enabled = auth_state().feature_enabled("music");
    let section = match raw_section {
        "music" if !music_enabled => "files",
        "tasks" if !tasks_enabled => "files",
        "shopping" if !shopping_enabled => "files",
        "finance" if !finance_enabled => "files",
        "mail" if !mail_enabled => "files",
        _ => raw_section,
    };

    rsx! {
        aside { class: "min-h-full w-64 bg-base-200 flex flex-col",
            // Logo — extra top padding so the bg extends up under the Android
            // status bar without crushing the logo against it
            div {
                class: "flex items-center gap-2 px-4 pb-4 border-b border-base-300",
                style: "padding-top: calc(1rem + env(safe-area-inset-top))",
                img { src: LOGO, alt: "Uncloud", class: "w-7 h-7" }
                span { class: "text-xl font-bold", "Uncloud" }
            }

            // Top-level section nav
            ul { class: "menu menu-sm px-2 pt-2 pb-0 w-full",
                li {
                    Link {
                        to: Route::Dashboard {},
                        class: if section == "dashboard" { "active" } else { "" },
                        onclick: move |_| close_drawer(),
                        IconLayoutGrid {}
                        span { "Dashboard" }
                    }
                }
                li {
                    Link {
                        to: Route::Home {},
                        class: if section == "files" { "active" } else { "" },
                        onclick: move |_| close_drawer(),
                        IconFolder {}
                        span { "Files" }
                    }
                }
                li {
                    Link {
                        to: Route::Gallery {},
                        class: if section == "gallery" { "active" } else { "" },
                        onclick: move |_| close_drawer(),
                        IconImage {}
                        span { "Gallery" }
                    }
                }
                if music_enabled {
                    li {
                        Link {
                            to: Route::Music {},
                            class: if section == "music" { "active" } else { "" },
                            onclick: move |_| close_drawer(),
                            IconMusic {}
                            span { "Music" }
                        }
                    }
                }
                if tasks_enabled {
                    li {
                        Link {
                            to: Route::Tasks {},
                            class: if section == "tasks" { "active" } else { "" },
                            onclick: move |_| close_drawer(),
                            IconCheckSquare {}
                            span { class: "min-w-0 flex-1", "Tasks" }
                            TasksOverdueBadge {}
                        }
                    }
                }
                if shopping_enabled {
                    li {
                        Link {
                            to: Route::Shopping {},
                            class: if section == "shopping" { "active" } else { "" },
                            onclick: move |_| close_drawer(),
                            IconShoppingCart {}
                            span { "Shopping" }
                        }
                    }
                }
                if finance_enabled {
                    li {
                        Link {
                            to: Route::Finance {},
                            class: if section == "finance" { "active" } else { "" },
                            onclick: move |_| close_drawer(),
                            IconWallet {}
                            span { "Finance" }
                        }
                    }
                }
                if mail_enabled {
                    li {
                        Link {
                            to: Route::Mail {},
                            class: if section == "mail" { "active" } else { "" },
                            onclick: move |_| close_drawer(),
                            IconMail {}
                            span { class: "min-w-0 flex-1", "Mail" }
                            MailTotalUnreadBadge {}
                        }
                    }
                }
                li {
                    Link {
                        to: Route::Passwords {},
                        class: if section == "passwords" { "active" } else { "" },
                        onclick: move |_| close_drawer(),
                        IconKey {}
                        span { "Passwords" }
                    }
                }
                li {
                    Link {
                        to: Route::Settings {},
                        class: if section == "settings" { "active" } else { "" },
                        onclick: move |_| close_drawer(),
                        IconSettings {}
                        span { "Settings" }
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
                                    onclick: move |_| close_drawer(),
                                    IconImage {}
                                    span { "Timeline" }
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
                                    onclick: move |_| close_drawer(),
                                    IconMusic {}
                                    span { "Library" }
                                }
                            }
                            MusicSidebarPlaylists {}
                            MusicSidebarFolders {}
                        },
                        "tasks" => rsx! {
                            li { class: "menu-title", span { "Tasks" } }
                            li {
                                Link {
                                    to: Route::Tasks {},
                                    class: if matches!(route, Route::Tasks {}) { "active" } else { "" },
                                    onclick: move |_| close_drawer(),
                                    IconCheckSquare {}
                                    span { "Schedule" }
                                }
                            }
                            li {
                                Link {
                                    to: Route::TasksAssigned {},
                                    class: if matches!(route, Route::TasksAssigned {}) { "active" } else { "" },
                                    onclick: move |_| close_drawer(),
                                    IconCheckSquare {}
                                    span { "Assigned to me" }
                                }
                            }
                            TasksSidebarProjects {}
                        },
                        "shopping" => rsx! {
                            li { class: "menu-title", span { "Shopping" } }
                            li {
                                Link {
                                    to: Route::Shopping {},
                                    class: if matches!(route, Route::Shopping {}) { "active" } else { "" },
                                    onclick: move |_| close_drawer(),
                                    IconShoppingCart {}
                                    span { "All Lists" }
                                }
                            }
                        },
                        "finance" => rsx! {
                            li { class: "menu-title", span { "Finance" } }
                            li {
                                Link {
                                    to: Route::Finance {},
                                    class: if matches!(route, Route::Finance {}) { "active" } else { "" },
                                    onclick: move |_| close_drawer(),
                                    IconList {}
                                    span { "Transactions" }
                                }
                            }
                            li {
                                Link {
                                    to: Route::FinanceAccounts {},
                                    class: if matches!(route, Route::FinanceAccounts {}) { "active" } else { "" },
                                    onclick: move |_| close_drawer(),
                                    IconWallet {}
                                    span { "Accounts" }
                                }
                            }
                            li {
                                Link {
                                    to: Route::FinanceCategories {},
                                    class: if matches!(route, Route::FinanceCategories {}) { "active" } else { "" },
                                    onclick: move |_| close_drawer(),
                                    IconLayoutGrid {}
                                    span { "Categories" }
                                }
                            }
                            li {
                                Link {
                                    to: Route::FinanceSettlements {},
                                    class: if matches!(route, Route::FinanceSettlements {} | Route::FinanceSettlementDetail { .. }) { "active" } else { "" },
                                    onclick: move |_| close_drawer(),
                                    IconUsers {}
                                    span { "Settlements" }
                                }
                            }
                            li {
                                Link {
                                    to: Route::FinanceImports {},
                                    class: if matches!(route, Route::FinanceImports {}) { "active" } else { "" },
                                    onclick: move |_| close_drawer(),
                                    IconHistory {}
                                    span { "Import" }
                                }
                                ul {
                                    li {
                                        Link {
                                            to: Route::FinanceSchemas {},
                                            class: if matches!(route, Route::FinanceSchemas {}) { "active" } else { "" },
                                            onclick: move |_| close_drawer(),
                                            IconFileText {}
                                            span { "Schemas" }
                                        }
                                    }
                                }
                            }
                            li {
                                Link {
                                    to: Route::FinanceRules {},
                                    class: if matches!(route, Route::FinanceRules {}) { "active" } else { "" },
                                    onclick: move |_| close_drawer(),
                                    IconShield {}
                                    span { "Rules" }
                                }
                            }
                        },
                        "mail" => rsx! {
                            li { class: "menu-title", span { "Accounts" } }
                            MailSidebarAccounts {}
                        },
                        "passwords" => rsx! {
                            li { class: "menu-title", span { "Passwords" } }
                            li {
                                Link {
                                    to: Route::Passwords {},
                                    class: "active",
                                    onclick: move |_| close_drawer(),
                                    IconKey {}
                                    span { "Vault" }
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
                                        IconUser {}
                                        span { "Account" }
                                    }
                                }
                                if tauri_hook::is_tauri() {
                                    li {
                                        Link {
                                            to: Route::SettingsTab { tab: "sync".to_string() },
                                            class: if active_tab == "sync" { "active" } else { "" },
                                            onclick: move |_| close_drawer(),
                                            IconRefreshCw {}
                                            span { "Sync" }
                                        }
                                    }
                                }
                                li {
                                    Link {
                                        to: Route::SettingsTab { tab: "preferences".to_string() },
                                        class: if active_tab == "preferences" { "active" } else { "" },
                                        onclick: move |_| close_drawer(),
                                        IconPalette {}
                                        span { "Preferences" }
                                    }
                                }
                                li {
                                    Link {
                                        to: Route::SettingsTab { tab: "activity".to_string() },
                                        class: if active_tab == "activity" { "active" } else { "" },
                                        onclick: move |_| close_drawer(),
                                        IconHistory {}
                                        span { "Activity" }
                                    }
                                }
                                if is_admin {
                                    li {
                                        Link {
                                            to: Route::SettingsTab { tab: "users".to_string() },
                                            class: if active_tab == "users" { "active" } else { "" },
                                            onclick: move |_| close_drawer(),
                                            IconUsers {}
                                            span { "Users" }
                                        }
                                    }
                                    li {
                                        Link {
                                            to: Route::SettingsTab { tab: "admin".to_string() },
                                            class: if active_tab == "admin" { "active" } else { "" },
                                            onclick: move |_| close_drawer(),
                                            IconShield {}
                                            span { "Admin" }
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
                                    onclick: move |_| close_drawer(),
                                    IconFolder {}
                                    span { "All Files" }
                                }
                            }
                            li {
                                Link {
                                    to: Route::Shares {},
                                    class: if matches!(route, Route::Shares {}) { "active" } else { "" },
                                    onclick: move |_| close_drawer(),
                                    IconLink {}
                                    span { "Shares" }
                                }
                            }
                            li {
                                Link {
                                    to: Route::Trash {},
                                    class: if matches!(route, Route::Trash {}) { "active" } else { "" },
                                    onclick: move |_| close_drawer(),
                                    IconTrash {}
                                    span { "Trash" }
                                }
                            }
                            li {
                                Link {
                                    to: Route::Duplicates {},
                                    class: if matches!(route, Route::Duplicates {}) { "active" } else { "" },
                                    onclick: move |_| close_drawer(),
                                    IconCopy {}
                                    span { "Duplicates" }
                                }
                            }
                            SidebarApps {}
                        },
                    }
                }
            }

            // Storage usage at the bottom — extra bottom padding so the bg
            // extends down under the Android nav bar
            div {
                class: "border-t border-base-300 px-3 pt-3",
                style: "padding-bottom: calc(0.75rem + env(safe-area-inset-bottom))",
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
fn MailUnreadBadge(count: u64) -> Element {
    if count == 0 {
        return rsx! {};
    }

    rsx! {
        span { class: "ml-auto inline-flex min-w-5 shrink-0 items-center justify-center rounded-full bg-primary px-2 py-0.5 text-[11px] font-semibold leading-none text-primary-content",
            "{count}"
        }
    }
}

#[component]
fn MailTotalUnreadBadge() -> Element {
    let mut unread = use_signal(|| 0u64);
    let dirty = use_context::<Signal<MailAccountDirtyTick>>();

    use_effect(use_reactive!(|(dirty)| {
        let _ = dirty().0;
        spawn(async move {
            if let Ok(rows) = use_mail::list_accounts().await {
                unread.set(rows.iter().map(|account| account.unread_count).sum());
            }
        });
    }));

    rsx! {
        MailUnreadBadge { count: unread() }
    }
}

#[component]
fn TasksOverdueBadge() -> Element {
    let mut overdue = use_signal(|| 0u64);
    // The sidebar is persistent, so re-read the current route to refetch the
    // count on every navigation — there is no task-mutation broadcast to
    // subscribe to the way Mail has `MailAccountDirtyTick`.
    let route = use_route::<Route>();

    use_effect(use_reactive!(|(route)| {
        let _ = route;
        spawn(async move {
            if let Ok(schedule) = use_tasks::get_schedule().await {
                overdue.set(schedule.overdue.len() as u64);
            }
        });
    }));

    if overdue() == 0 {
        return rsx! {};
    }

    rsx! {
        span { class: "ml-auto inline-flex min-w-5 shrink-0 items-center justify-center rounded-full bg-error px-2 py-0.5 text-[11px] font-semibold leading-none text-error-content",
            "{overdue}"
        }
    }
}

#[component]
fn MailSidebarAccounts() -> Element {
    let mut accounts: Signal<Vec<MailAccountResponse>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let dirty = use_context::<Signal<MailAccountDirtyTick>>();
    let route = use_route::<Route>();

    use_effect(use_reactive!(|(dirty)| {
        let _ = dirty().0;
        spawn(async move {
            loading.set(true);
            if let Ok(rows) = use_mail::list_accounts().await {
                accounts.set(rows);
            }
            loading.set(false);
        });
    }));

    let active_account_id = match &route {
        Route::MailAccount { account_id } => Some(account_id.as_str()),
        _ => None,
    };
    let account_list = accounts();

    rsx! {
        if loading() {
            li {
                div { class: "justify-start gap-2 text-base-content/60",
                    span { class: "loading loading-spinner loading-xs" }
                    span { "Loading accounts" }
                }
            }
        } else if account_list.is_empty() {
            li {
                div { class: "text-sm text-base-content/60", "No accounts" }
            }
        } else {
            for account in account_list {
                {
                    let id = account.id.clone();
                    let is_active = active_account_id == Some(id.as_str());
                    rsx! {
                        li {
                            Link {
                                to: Route::MailAccount { account_id: account.id.clone() },
                                class: if is_active {
                                    "active flex items-start gap-2 min-w-0"
                                } else {
                                    "flex items-start gap-2 min-w-0"
                                },
                                onclick: move |_| close_drawer(),
                                IconMail { class: "mt-0.5 h-4 w-4 shrink-0".to_string() }
                                span { class: "min-w-0 flex-1",
                                    span { class: "block truncate text-sm", "{account.display_name}" }
                                    span { class: "block truncate text-xs opacity-60", "{account.email_address}" }
                                    if account.sync_in_progress {
                                        span { class: "mt-1 inline-flex items-center gap-1 text-xs opacity-60",
                                            span { class: "loading loading-spinner loading-xs" }
                                            span { "Syncing" }
                                        }
                                    }
                                }
                                MailUnreadBadge { count: account.unread_count }
                            }
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
                            class: if is_active {
                                "flex items-center gap-2 min-w-0 active"
                            } else {
                                "flex items-center gap-2 min-w-0"
                            },
                            style: "padding-left: calc(0.75rem + {indent_px}px)",
                            onclick: move |_| close_drawer(),
                            if depth > 0 {
                                span { class: "opacity-30 text-xs flex-shrink-0", "└" }
                            }
                            IconFolder {}
                            span { class: "truncate min-w-0", "{album.name}" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn MusicSidebarFolders() -> Element {
    let mut folders_sig: Signal<Vec<MusicFolderResponse>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let route = use_route::<Route>();
    let cat_dirty = use_context::<Signal<crate::state::MusicCategoryDirtyTick>>();

    // Load the folders that belong to at least one category. The full folder
    // tree now lives in the main Music view, so the sidebar remains compact.
    use_effect(use_reactive!(|cat_dirty| {
        let _ = cat_dirty();
        spawn(async move {
            loading.set(true);
            let mut folder_ids: Vec<String> = Vec::new();
            if let Ok(cats) = crate::hooks::use_music_categories::list_categories().await {
                let mut seen = std::collections::HashSet::new();
                for category in cats {
                    for folder_id in category.folder_ids {
                        if seen.insert(folder_id.clone()) {
                            folder_ids.push(folder_id);
                        }
                    }
                }
            }

            if folder_ids.is_empty() {
                folders_sig.set(Vec::new());
            } else if let Ok(mut folders) = use_music::list_music_folders_by_ids(&folder_ids).await
            {
                folders.sort_by(|a, b| a.path.to_lowercase().cmp(&b.path.to_lowercase()));
                folders_sig.set(folders);
            }
            loading.set(false);
        });
    }));

    let folders = folders_sig();
    if folders.is_empty() && !loading() {
        return rsx! {};
    }

    let active_id: Option<String> = match &route {
        Route::MusicFolder { id } | Route::MusicScopeFolder { id } => Some(id.clone()),
        _ => None,
    };

    rsx! {
        li { class: "menu-title mt-2", span { "Categories" } }
        if loading() {
            li {
                div { class: "flex items-center gap-2 text-base-content/50",
                    span { class: "loading loading-spinner loading-xs" }
                    span { "Loading..." }
                }
            }
        }
        for folder in folders {
            {
                let fid = folder.folder_id.clone();
                let is_active = active_id.as_deref() == Some(&fid);
                rsx! {
                    li {
                        Link {
                            to: Route::MusicScopeFolder { id: folder.folder_id.clone() },
                            class: if is_active {
                                "active flex items-center gap-2 min-w-0"
                            } else {
                                "flex items-center gap-2 min-w-0"
                            },
                            title: "{folder.path}",
                            onclick: move |_| close_drawer(),
                            IconFolder {}
                            span { class: "truncate", "{folder.name}" }
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
    let dirty = use_context::<Signal<PlaylistDirtyTick>>();
    let route = use_route::<Route>();
    let nav = use_navigator();

    // Create modal state
    let mut show_create: Signal<bool> = use_signal(|| false);
    let mut new_name: Signal<String> = use_signal(|| String::new());
    let mut create_error: Signal<Option<String>> = use_signal(|| None);

    use_effect(move || {
        let _ = refresh();
        let _ = dirty().0;
        spawn(async move {
            if let Ok(p) = use_playlists::list_playlists_light().await {
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
                let is_active = matches!(&route, Route::MusicPlaylist { id } if *id == pl_id);
                rsx! {
                    li {
                        Link {
                            to: Route::MusicPlaylist { id: pl.id.clone() },
                            class: if is_active { "active" } else { "" },
                            onclick: move |_| close_drawer(),
                            IconListMusic {}
                            span { class: "flex-1 truncate", "{pl_name}" }
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

    }
}

#[component]
fn TasksSidebarProjects() -> Element {
    let mut projects: Signal<Vec<TaskProjectResponse>> = use_signal(Vec::new);
    let mut refresh = use_signal(|| 0u32);
    let route = use_route::<Route>();
    let nav = use_navigator();

    // Create modal state
    let mut show_create: Signal<bool> = use_signal(|| false);
    let mut new_name: Signal<String> = use_signal(|| String::new());
    let mut create_error: Signal<Option<String>> = use_signal(|| None);

    use_effect(move || {
        let _ = refresh();
        spawn(async move {
            if let Ok(p) = use_tasks::list_projects().await {
                projects.set(p);
            }
        });
    });

    rsx! {
        li { class: "menu-title mt-2", span { "Projects" } }
        for project in projects() {
            {
                let pid = project.id.clone();
                let is_active = matches!(&route, Route::TasksProject { id } if *id == pid);
                let color = project.color.clone().unwrap_or_else(|| "#3B82F6".to_string());
                rsx! {
                    li {
                        Link {
                            to: Route::TasksProject { id: project.id.clone() },
                            class: if is_active { "active" } else { "" },
                            onclick: move |_| close_drawer(),
                            span {
                                class: "w-3 h-3 rounded-full inline-block",
                                style: "background-color: {color}",
                            }
                            span { class: "truncate", "{project.name}" }
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
                "+ New project"
            }
        }

        // Create project modal
        if show_create() {
            div { class: "modal modal-open",
                div { class: "modal-box",
                    h3 { class: "font-bold text-lg mb-4", "New Project" }
                    if let Some(err) = create_error() {
                        div { class: "alert alert-error mb-3 text-sm", "{err}" }
                    }
                    input {
                        class: "input input-bordered w-full",
                        r#type: "text",
                        placeholder: "Project name",
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
                                    let req = uncloud_common::CreateTaskProjectRequest {
                                        name: name.clone(),
                                        description: None,
                                        color: None,
                                        icon: None,
                                        default_view: None,
                                    };
                                    match use_tasks::create_project(&req).await {
                                        Ok(project) => {
                                            show_create.set(false);
                                            let next = *refresh.peek() + 1;
                                            refresh.set(next);
                                            nav.push(Route::TasksProject { id: project.id });
                                        }
                                        Err(e) => {
                                            create_error.set(Some(e));
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
    }
}
