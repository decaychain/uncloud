//! Mobile-friendly dashboard with configurable shortcut tiles.
//!
//! Each tile is a link into another section. Tile config lives on the server
//! in `UserPreferences.dashboard_tiles` and is seeded with `default_tile_ids()`
//! when the user hasn't customised it.

use dioxus::prelude::*;

use crate::components::icons::{
    IconCheckSquare, IconFolder, IconImage, IconKey, IconLink, IconMusic, IconShoppingCart,
    IconTrash,
};
use crate::hooks::{use_files, use_shares, use_shopping, use_tasks};
use crate::router::Route;
use crate::state::AuthState;

/// Static list of tiles the dashboard knows how to render. Order here is the
/// order tiles appear in the picker; enabled order is preserved from the user's
/// saved preference.
pub fn all_tile_ids() -> &'static [&'static str] {
    &[
        "files",
        "gallery",
        "music",
        "tasks",
        "shopping",
        "passwords",
        "shares",
        "trash",
    ]
}

/// Default tiles shown when the user has not customised their preference.
pub fn default_tile_ids() -> Vec<String> {
    vec![
        "files".to_string(),
        "gallery".to_string(),
        "tasks".to_string(),
        "passwords".to_string(),
    ]
}

pub fn tile_label(id: &str) -> &'static str {
    match id {
        "files" => "Files",
        "gallery" => "Gallery",
        "music" => "Music",
        "tasks" => "Tasks",
        "shopping" => "Shopping",
        "passwords" => "Passwords",
        "shares" => "Shares",
        "trash" => "Trash",
        _ => "Unknown",
    }
}

fn tile_route(id: &str) -> Option<Route> {
    match id {
        "files" => Some(Route::Home {}),
        "gallery" => Some(Route::Gallery {}),
        "music" => Some(Route::Music {}),
        "tasks" => Some(Route::Tasks {}),
        "shopping" => Some(Route::Shopping {}),
        "passwords" => Some(Route::Passwords {}),
        "shares" => Some(Route::Shares {}),
        "trash" => Some(Route::Trash {}),
        _ => None,
    }
}

#[component]
pub fn DashboardPage() -> Element {
    let auth_state = use_context::<Signal<AuthState>>();

    let shopping_enabled = auth_state()
        .user
        .as_ref()
        .map(|u| u.features_enabled.contains(&"shopping".to_string()))
        .unwrap_or(false);

    // Resolve enabled tiles: user's preference, or the default set.
    // Filter out tiles for features the user has disabled (e.g. shopping).
    let enabled: Vec<String> = {
        let configured = auth_state()
            .user
            .as_ref()
            .map(|u| u.preferences.dashboard_tiles.clone())
            .unwrap_or_default();
        let base = if configured.is_empty() {
            default_tile_ids()
        } else {
            configured
        };
        base.into_iter()
            .filter(|id| id != "shopping" || shopping_enabled)
            .collect()
    };

    rsx! {
        div { class: "p-4",
            h1 { class: "text-2xl font-bold mb-4", "Dashboard" }

            if enabled.is_empty() {
                div { class: "text-base-content/60 text-sm",
                    "No tiles enabled. Configure them in "
                    Link { to: Route::SettingsTab { tab: "preferences".to_string() }, class: "link link-primary", "Preferences" }
                    "."
                }
            } else {
                div { class: "grid grid-cols-2 md:grid-cols-3 lg:grid-cols-4 gap-3",
                    for id in enabled {
                        {
                            let key = id.clone();
                            rsx! {
                                DashboardTile { key: "{key}", tile_id: id }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn DashboardTile(tile_id: String) -> Element {
    let route = match tile_route(&tile_id) {
        Some(r) => r,
        None => return rsx! {},
    };
    let label = tile_label(&tile_id);
    let count = use_tile_count(&tile_id);

    rsx! {
        Link {
            to: route,
            class: "card bg-base-200 hover:bg-base-300 transition-colors shadow-sm",
            div { class: "card-body p-4 gap-1",
                div { class: "flex items-center gap-2",
                    TileIcon { tile_id: tile_id.clone() }
                    span { class: "font-semibold text-sm", "{label}" }
                }
                div { class: "text-xs text-base-content/60 h-4",
                    match count() {
                        TileCount::Loading => rsx! { span { class: "opacity-40", "…" } },
                        TileCount::None => rsx! { span { "" } },
                        TileCount::Value(v, suffix) => rsx! { span { "{v} {suffix}" } },
                    }
                }
            }
        }
    }
}

#[component]
fn TileIcon(tile_id: String) -> Element {
    let class = "w-5 h-5".to_string();
    match tile_id.as_str() {
        "files" => rsx! { IconFolder { class } },
        "gallery" => rsx! { IconImage { class } },
        "music" => rsx! { IconMusic { class } },
        "tasks" => rsx! { IconCheckSquare { class } },
        "shopping" => rsx! { IconShoppingCart { class } },
        "passwords" => rsx! { IconKey { class } },
        "shares" => rsx! { IconLink { class } },
        "trash" => rsx! { IconTrash { class } },
        _ => rsx! {},
    }
}

#[derive(Clone, PartialEq)]
enum TileCount {
    Loading,
    None,
    Value(usize, &'static str),
}

/// Fetches the tile's summary count in the background. Returns `None` for
/// tiles that don't have a cheap count endpoint.
fn use_tile_count(tile_id: &str) -> Signal<TileCount> {
    let mut state = use_signal(|| TileCount::Loading);
    let tid = tile_id.to_string();

    use_effect(move || {
        let tid = tid.clone();
        spawn(async move {
            let result = match tid.as_str() {
                "tasks" => use_tasks::list_projects()
                    .await
                    .map(|v| TileCount::Value(v.len(), "projects"))
                    .unwrap_or(TileCount::None),
                "shopping" => use_shopping::list_lists()
                    .await
                    .map(|v| TileCount::Value(v.len(), "lists"))
                    .unwrap_or(TileCount::None),
                "shares" => use_shares::list_shares()
                    .await
                    .map(|v| TileCount::Value(v.len(), "shares"))
                    .unwrap_or(TileCount::None),
                "trash" => use_files::list_trash()
                    .await
                    .map(|v| TileCount::Value(v.len(), "items"))
                    .unwrap_or(TileCount::None),
                "gallery" => use_files::list_gallery_albums()
                    .await
                    .map(|v| TileCount::Value(v.len(), "albums"))
                    .unwrap_or(TileCount::None),
                // Files/music/passwords have no cheap total-count API; we'd
                // have to page through. Leave blank rather than lie.
                _ => TileCount::None,
            };
            state.set(result);
        });
    });

    state
}
