use dioxus::prelude::*;
use uncloud_common::ServerEvent;
use crate::hooks::use_events::use_events;
use crate::components::icons::IconMenu;
use crate::hooks::tauri;
use crate::state::{AuthState, PlayerState, ThemeState};
use crate::router::Route;

#[component]
pub fn Layout() -> Element {
    let auth_state = use_context::<Signal<AuthState>>();
    let nav = use_navigator();

    let mut sse_event: Signal<Option<ServerEvent>> = use_context_provider(|| Signal::new(None));
    use_events(move |event| {
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
    let main_class = if player_visible {
        "flex-1 p-4 md:p-6 pb-20"
    } else {
        "flex-1 p-4 md:p-6"
    };

    let route = use_route::<Route>();
    let page_title = match route {
        Route::Gallery {} | Route::GalleryAlbum { .. } => "Uncloud - Gallery",
        Route::Music {} | Route::MusicArtist { .. } | Route::MusicAlbum { .. }
            | Route::MusicFolder { .. } | Route::MusicPlaylist { .. } => "Uncloud - Music",
        Route::Shopping {} | Route::ShoppingList { .. } => "Uncloud - Shopping",
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
                    main { class: main_class,
                        Outlet::<Route> {}
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

    let section_title = if matches!(route, Route::Gallery {} | Route::GalleryAlbum { .. }) {
        "Gallery"
    } else if matches!(route, Route::Music {} | Route::MusicArtist { .. } | Route::MusicAlbum { .. } | Route::MusicFolder { .. } | Route::MusicPlaylist { .. }) {
        "Music"
    } else if matches!(route, Route::Shopping {} | Route::ShoppingList { .. }) {
        "Shopping"
    } else if matches!(route, Route::Settings {}) {
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
        div { class: "navbar bg-base-200 shadow-sm sticky top-0 z-30 gap-2",
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
