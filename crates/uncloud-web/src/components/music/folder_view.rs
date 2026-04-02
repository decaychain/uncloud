use dioxus::prelude::*;
use uncloud_common::{ServerEvent, TrackResponse};
use crate::hooks::{use_music, use_player};
use crate::state::PlayerState;
use super::track_list::TrackList;

#[component]
pub fn FolderView(folder_id: String) -> Element {
    let player = use_context::<Signal<PlayerState>>();
    let mut tracks: Signal<Vec<TrackResponse>> = use_signal(Vec::new);
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

    use_effect(use_reactive!(|(folder_id, refresh)| {
        let _ = refresh;
        spawn(async move {
            loading.set(true);
            match use_music::list_music_tracks(Some(&folder_id), None).await {
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
                div { class: "text-5xl", "⚠️" }
                h3 { class: "text-lg font-semibold", "Error loading tracks" }
                p { class: "text-base-content/60", "{err}" }
            }
        };
    }

    let track_list = tracks();
    let tracks_for_play_all = track_list.clone();
    let has_tracks = !track_list.is_empty();

    rsx! {
        if has_tracks {
            div { class: "flex items-center gap-2 mb-3",
                button {
                    class: "btn btn-primary btn-sm",
                    onclick: move |_| {
                        use_player::play_queue(player, tracks_for_play_all.clone(), 0);
                    },
                    svg { class: "w-4 h-4", fill: "currentColor", view_box: "0 0 24 24",
                        path { d: "M8 5v14l11-7z" }
                    }
                    "Play All"
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
    }
}
