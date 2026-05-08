use dioxus::prelude::*;
use uncloud_common::{ServerEvent, TrackResponse};
use crate::hooks::{use_music, use_player};
use crate::state::PlayerState;
use super::track_list::TrackList;
use crate::components::icons::{IconAlertTriangle, IconMoreVertical, IconPlay};
use super::manage_categories::ManageCategoriesModal;

#[component]
pub fn FolderView(folder_id: String) -> Element {
    let player = use_context::<Signal<PlayerState>>();
    let mut tracks: Signal<Vec<TrackResponse>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut refresh = use_signal(|| 0u32);
    let mut folder_name: Signal<String> = use_signal(String::new);
    let mut show_categories: Signal<bool> = use_signal(|| false);

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

    let folder_id_for_name = folder_id.clone();
    use_effect(use_reactive!(|folder_id_for_name| {
        let fid = folder_id_for_name;
        spawn(async move {
            if let Ok(folders) = use_music::list_music_folders().await {
                if let Some(f) = folders.into_iter().find(|f| f.folder_id == fid) {
                    folder_name.set(f.name);
                }
            }
        });
    }));

    let folder_id_for_tracks = folder_id.clone();
    use_effect(use_reactive!(|(folder_id_for_tracks, refresh)| {
        let _ = refresh;
        let fid = folder_id_for_tracks;
        spawn(async move {
            // Don't flip `loading` on refresh — the router keys this view on
            // folder_id, so navigation remounts it and the initial `use_signal(|| true)`
            // covers that case. SSE-driven refreshes update silently.
            match use_music::list_music_tracks(Some(&fid), None).await {
                Ok(resp) => tracks.set(resp.tracks),
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
                h3 { class: "text-lg font-semibold", "Error loading tracks" }
                p { class: "text-base-content/60", "{err}" }
            }
        };
    }

    let track_list = tracks();
    let tracks_for_play_all = track_list.clone();
    let has_tracks = !track_list.is_empty();
    let folder_id_for_modal = folder_id.clone();
    let folder_name_for_modal = folder_name();
    let folder_name_display = folder_name();

    rsx! {
        div { class: "flex items-center gap-2 mb-3",
            if !folder_name_display.is_empty() {
                h2 { class: "text-lg font-semibold flex-1 truncate", "{folder_name_display}" }
            } else {
                div { class: "flex-1" }
            }
            if has_tracks {
                button {
                    class: "btn btn-primary btn-sm",
                    onclick: move |_| {
                        use_player::play_queue(player, tracks_for_play_all.clone(), 0);
                    },
                    IconPlay { class: "w-4 h-4".to_string() }
                    "Play All"
                }
            }
            div { class: "dropdown dropdown-end",
                button {
                    class: "btn btn-ghost btn-sm btn-circle",
                    tabindex: "0",
                    IconMoreVertical { class: "w-4 h-4".to_string() }
                }
                ul {
                    class: "dropdown-content menu menu-sm bg-base-200 rounded-box shadow z-10 w-52",
                    tabindex: "0",
                    li {
                        a {
                            onclick: move |_| show_categories.set(true),
                            "Manage categories…"
                        }
                    }
                }
            }
        }
        TrackList {
            tracks: track_list,
            show_artist: true,
            show_album: true,
            on_play: move |idx: usize| {
                use_player::play_queue(player, tracks().clone(), idx);
            },
        }
        if show_categories() {
            ManageCategoriesModal {
                folder_id: folder_id_for_modal,
                folder_name: folder_name_for_modal,
                on_close: move |_| show_categories.set(false),
                on_changed: move |_| {},
            }
        }
    }
}
