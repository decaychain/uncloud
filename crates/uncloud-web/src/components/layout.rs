use dioxus::prelude::*;
use uncloud_common::ServerEvent;
use crate::hooks::use_events::use_events;
use crate::components::icons::IconMenu;
use crate::hooks::tauri;
use crate::hooks::use_storages::{RescanConflict, RescanJob, RescanStatus};

fn parse_rescan_status(status: &str) -> RescanStatus {
    match status {
        "completed" => RescanStatus::Completed,
        "failed" => RescanStatus::Failed,
        "cancelled" => RescanStatus::Cancelled,
        _ => RescanStatus::Running,
    }
}
use crate::state::{AuthState, PinnedPlaylistState, PlayerState, RescanState, ThemeState};
use crate::router::Route;

#[component]
pub fn Layout() -> Element {
    let auth_state = use_context::<Signal<AuthState>>();
    let nav = use_navigator();

    let mut sse_event: Signal<Option<ServerEvent>> = use_context_provider(|| Signal::new(None));
    let mut rescan_state = use_context::<Signal<RescanState>>();
    use_events(move |event| {
        // Update the app-level rescan state directly — the panel in Settings
        // reads this signal, and we want the state to persist even when the
        // Settings component is unmounted.
        match &event {
            ServerEvent::RescanProgress {
                job_id,
                storage_id,
                status,
                processed_entries,
                total_entries,
                imported_folders,
                imported_files,
                skipped_existing,
                conflicts_count: _,
            } => {
                let parsed_status = parse_rescan_status(status);
                let mut current = rescan_state.write();
                // Progress events carry a count, not the conflicts themselves —
                // the full list arrives in RescanFinished. Preserve whatever
                // we already had so a late-mounted UI doesn't flash empty.
                let conflicts = current
                    .job
                    .as_ref()
                    .map(|j| j.conflicts.clone())
                    .unwrap_or_default();
                current.job = Some(RescanJob {
                    id: job_id.clone(),
                    storage_id: storage_id.clone(),
                    status: parsed_status,
                    total_entries: *total_entries,
                    processed_entries: *processed_entries,
                    imported_folders: *imported_folders,
                    imported_files: *imported_files,
                    skipped_existing: *skipped_existing,
                    conflicts,
                    error: None,
                });
                current.error = None;
                current.starting = false;
            }
            ServerEvent::RescanFinished {
                job_id,
                storage_id,
                status,
                processed_entries,
                total_entries,
                imported_folders,
                imported_files,
                skipped_existing,
                conflicts,
                error,
            } => {
                let parsed_status = parse_rescan_status(status);
                let conflicts = conflicts
                    .iter()
                    .map(|c| RescanConflict {
                        path: c.path.clone(),
                        reason: c.reason.clone(),
                    })
                    .collect();
                let mut current = rescan_state.write();
                current.job = Some(RescanJob {
                    id: job_id.clone(),
                    storage_id: storage_id.clone(),
                    status: parsed_status,
                    total_entries: *total_entries,
                    processed_entries: *processed_entries,
                    imported_folders: *imported_folders,
                    imported_files: *imported_files,
                    skipped_existing: *skipped_existing,
                    conflicts,
                    error: error.clone(),
                });
                current.error = None;
                current.starting = false;
            }
            _ => {}
        }
        sse_event.set(Some(event));
    });

    use_effect(move || {
        if auth_state().loading {
            return; // wait for session check before redirecting
        }
        if tauri::needs_setup() {
            nav.push("/setup");
        } else if !auth_state().is_authenticated() {
            nav.push("/login");
        }
    });

    if auth_state().loading || tauri::needs_setup() || !auth_state().is_authenticated() {
        return rsx! {
            div { class: "flex items-center justify-center min-h-screen",
                span { class: "loading loading-spinner loading-lg" }
            }
        };
    }

    let theme_state = use_context::<Signal<ThemeState>>();
    let theme = if theme_state().dark { "dark" } else { "light" };
    let player_state = use_context::<Signal<PlayerState>>();
    let player_visible = !player_state().queue.is_empty();
    // When the player is hidden, the main content's bottom flush is the
    // page edge — on Android that's under the gesture/nav bar, so we pad
    // with the safe-area inset. When the player is visible, it handles its
    // own safe-area and main just needs space for the fixed bar (`pb-20`).
    // `pb-player` reserves player-content (rem, scales with font) + safe-area
    // inset. Defined in `input.css`; taller on mobile for the two-row bar.
    let main_class = if player_visible {
        "flex-1 p-4 md:p-6 pb-player"
    } else {
        "flex-1 p-4 md:p-6 pb-safe"
    };

    let route = use_route::<Route>();

    // The right-side playlist panel is shown on music browse routes (not on
    // the dedicated playlist view, which already shows the playlist) when a
    // playlist is pinned and the viewport is wide enough. The wrapping flex
    // is kept across pin/unpin transitions so the Outlet doesn't remount and
    // discard browse state. The panel itself hides on small screens via
    // `hidden xl:block`.
    let pinned = use_context::<Signal<PinnedPlaylistState>>();
    let on_music_route = matches!(
        route,
        Route::Music {} | Route::MusicArtist { .. } | Route::MusicAlbum { .. } | Route::MusicFolder { .. }
    );
    let show_playlist_panel = on_music_route && pinned().0.is_some();

    // NOTE: when adding a new top-level Route, add it here AND in
    // `section_title` below AND in the sidebar section-matcher. Missing one
    // results in the wrong caption/title — a recurring regression.
    let page_title = match route {
        Route::Dashboard {} => "Uncloud - Dashboard",
        Route::Gallery {} | Route::GalleryAlbum { .. } => "Uncloud - Gallery",
        Route::Music {} | Route::MusicArtist { .. } | Route::MusicAlbum { .. }
            | Route::MusicFolder { .. } | Route::MusicPlaylist { .. } => "Uncloud - Music",
        Route::Shopping {} | Route::ShoppingList { .. } => "Uncloud - Shopping",
        Route::Tasks {} | Route::TasksProject { .. } => "Uncloud - Tasks",
        Route::Passwords {} => "Uncloud - Passwords",
        Route::Settings {} | Route::SettingsTab { .. } => "Uncloud - Settings",
        Route::Trash {} => "Uncloud - Trash",
        Route::Shares {} => "Uncloud - Shares",
        _ => "Uncloud - Files",
    };

    rsx! {
        document::Title { "{page_title}" }
        div { "data-theme": theme, class: "min-h-screen bg-base-100",
            div { class: "drawer lg:drawer-open",
                input { id: "main-sidebar", r#type: "checkbox", class: "drawer-toggle" }

                div { class: "drawer-content flex flex-col",
                    Navbar {}
                    if on_music_route {
                        main { class: "{main_class}",
                            div { class: "flex gap-4 min-w-0",
                                div { class: "flex-1 min-w-0",
                                    Outlet::<Route> {}
                                }
                                if show_playlist_panel {
                                    div { class: "hidden xl:block w-80 shrink-0",
                                        crate::components::music::PlaylistSidePanel {}
                                    }
                                }
                            }
                        }
                    } else {
                        main { class: main_class,
                            Outlet::<Route> {}
                        }
                    }
                }

                div { class: "drawer-side z-40",
                    label {
                        r#for: "main-sidebar",
                        "aria-label": "close sidebar",
                        class: "drawer-overlay",
                    }
                    crate::components::sidebar::Sidebar {}
                }
            }
            crate::components::player::Player {}
        }
    }
}

#[component]
fn Navbar() -> Element {
    let auth_state = use_context::<Signal<AuthState>>();
    let nav = use_navigator();
    let route = use_route::<Route>();

    let username = auth_state()
        .user
        .as_ref()
        .map(|u| u.username.clone())
        .unwrap_or_default();

    let initial = username.chars().next().unwrap_or('?').to_uppercase().to_string();

    let on_logout = move |_| {
        spawn(async move {
            let _ = crate::hooks::use_auth::logout().await;
            nav.push("/login");
        });
    };

    let section_title = if matches!(route, Route::Dashboard {}) {
        "Dashboard"
    } else if matches!(route, Route::Gallery {} | Route::GalleryAlbum { .. }) {
        "Gallery"
    } else if matches!(route, Route::Music {} | Route::MusicArtist { .. } | Route::MusicAlbum { .. } | Route::MusicFolder { .. } | Route::MusicPlaylist { .. }) {
        "Music"
    } else if matches!(route, Route::Shopping {} | Route::ShoppingList { .. }) {
        "Shopping"
    } else if matches!(route, Route::Tasks {} | Route::TasksProject { .. }) {
        "Tasks"
    } else if matches!(route, Route::Passwords {}) {
        "Passwords"
    } else if matches!(route, Route::Settings {} | Route::SettingsTab { .. }) {
        "Settings"
    } else if matches!(route, Route::Trash {}) {
        "Trash"
    } else if matches!(route, Route::Shares {}) {
        "Shares"
    } else {
        "Files"
    };

    let search_enabled = use_context::<Signal<bool>>()();

    rsx! {
        div { class: "navbar bg-base-200 shadow-sm sticky top-0 z-30 gap-2 pt-safe",
            // Left: hamburger + section title (mobile only)
            div { class: "flex-shrink-0 flex items-center gap-1",
                label {
                    r#for: "main-sidebar",
                    class: "btn btn-ghost btn-circle lg:hidden",
                    IconMenu { class: "w-5 h-5".to_string() }
                }
                span { class: "text-lg font-semibold lg:hidden", "{section_title}" }
            }

            // Search — fills all available space between left and right
            div { class: "hidden sm:flex flex-1 min-w-0 justify-center px-4",
                if search_enabled {
                    div { class: "w-full max-w-2xl",
                        crate::components::search::SearchBar {}
                    }
                }
            }

            div { class: "flex-shrink-0 flex items-center gap-0 ml-auto",
                // Mobile search icon (visible on sm and below)
                if search_enabled {
                    crate::components::search::SearchIconMobile {}
                }

                // User avatar dropdown
                div { class: "dropdown dropdown-end",
                    div {
                        tabindex: "0",
                        role: "button",
                        class: "btn btn-ghost btn-circle avatar placeholder",
                        div { class: "bg-neutral text-neutral-content w-8 rounded-full",
                            span { class: "text-sm", "{initial}" }
                        }
                    }
                    ul {
                        tabindex: "0",
                        class: "menu menu-sm dropdown-content bg-base-100 rounded-box z-50 mt-3 w-52 p-2 shadow",
                        li {
                            span { class: "font-semibold text-sm px-2 py-1 opacity-70", "{username}" }
                        }
                        li { div { class: "divider my-0" } }
                        li {
                            a { onclick: on_logout, "Sign out" }
                        }
                    }
                }
            }
        }
    }
}
