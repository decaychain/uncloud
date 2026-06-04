use std::collections::{HashMap, HashSet};

use super::manage_categories::ManageCategoriesModal;
use crate::components::icons::{
    IconAlertTriangle, IconChevronDown, IconChevronRight, IconFolder, IconFolderOpen,
    IconMoreVertical, IconMusic, IconPause, IconPlay,
};
use crate::hooks::{use_music, use_player};
use crate::router::Route;
use crate::state::PlayerState;
use dioxus::prelude::*;
use uncloud_common::{MusicFolderResponse, TrackResponse};

#[component]
pub fn FolderTreeView(root_folder_id: Option<String>) -> Element {
    let route = use_route::<Route>();
    let player = use_context::<Signal<PlayerState>>();
    let mut folders: Signal<Vec<MusicFolderResponse>> = use_signal(Vec::new);
    let mut expanded: Signal<HashSet<String>> = use_signal(HashSet::new);
    let mut loaded_children: Signal<HashSet<String>> = use_signal(HashSet::new);
    let mut loading_children: Signal<HashSet<String>> = use_signal(HashSet::new);
    let mut open_tracks: Signal<HashSet<String>> = use_signal(HashSet::new);
    let mut tracks_by_folder: Signal<HashMap<String, Vec<TrackResponse>>> =
        use_signal(HashMap::new);
    let mut track_cursors: Signal<HashMap<String, Option<String>>> = use_signal(HashMap::new);
    let mut loading_tracks: Signal<HashSet<String>> = use_signal(HashSet::new);
    let mut track_errors: Signal<HashMap<String, String>> = use_signal(HashMap::new);
    let mut category_modal: Signal<Option<(String, String)>> = use_signal(|| None);
    let mut loading_root = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let root_folder_id_for_effect = root_folder_id.clone();
    use_effect(use_reactive!(|root_folder_id_for_effect| {
        let root_folder_id = root_folder_id_for_effect;
        spawn(async move {
            loading_root.set(true);
            error.set(None);
            expanded.set(HashSet::new());
            loaded_children.set(HashSet::new());
            loading_children.set(HashSet::new());
            open_tracks.set(HashSet::new());
            tracks_by_folder.set(HashMap::new());
            track_cursors.set(HashMap::new());
            loading_tracks.set(HashSet::new());
            track_errors.set(HashMap::new());

            let result = if let Some(root_id) = root_folder_id {
                use_music::list_music_folders_by_ids(&[root_id])
                    .await
                    .map(|mut roots| {
                        for root in &mut roots {
                            root.parent_folder_id = None;
                        }
                        roots
                    })
            } else {
                use_music::list_music_root_folders().await
            };

            match result {
                Ok(root_folders) => folders.set(root_folders),
                Err(err) => error.set(Some(err)),
            }
            loading_root.set(false);
        });
    }));

    if loading_root() {
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
                h3 { class: "text-lg font-semibold", "Error loading folders" }
                p { class: "text-base-content/60", "{err}" }
            }
        };
    }

    let rows = visible_folder_rows(&folders(), &expanded());
    if rows.is_empty() {
        return rsx! {
            div { class: "flex flex-col items-center justify-center py-20 gap-2 text-base-content/60",
                IconFolder { class: "w-12 h-12 opacity-30".to_string() }
                p { "No music folders" }
            }
        };
    }

    let active_folder_id = match &route {
        Route::MusicFolder { id } | Route::MusicScopeFolder { id } => Some(id.clone()),
        _ => None,
    };
    let expanded_now = expanded();
    let loading_children_now = loading_children();
    let open_tracks_now = open_tracks();
    let tracks_now = tracks_by_folder();
    let track_cursors_now = track_cursors();
    let loading_tracks_now = loading_tracks();
    let track_errors_now = track_errors();
    let player_now = player();
    let current_playing_id = player_now
        .current_track()
        .map(|track| track.file.id.clone());
    let player_is_playing = player_now.playing;

    rsx! {
        div { class: "overflow-hidden rounded-lg border border-base-300 bg-base-100",
            div { class: "hidden md:grid grid-cols-[minmax(0,1fr)_7rem] gap-3 border-b border-base-300 bg-base-200/60 px-3 py-2 text-xs font-medium uppercase tracking-wide text-base-content/60",
                span { "Folder" }
                span { class: "text-right", "Tracks" }
            }
            div { class: "divide-y divide-base-200",
                for (folder, depth) in rows {
                    {
                        let id = folder.folder_id.clone();
                        let id_for_toggle = id.clone();
                        let is_expanded = expanded_now.contains(&id);
                        let is_loading = loading_children_now.contains(&id);
                        let is_active = active_folder_id.as_deref() == Some(&id);
                        let tracks_open = open_tracks_now.contains(&id);
                        let tracks_loading = loading_tracks_now.contains(&id);
                        let has_loaded_tracks = tracks_now.contains_key(&id);
                        let folder_tracks = tracks_now.get(&id).cloned().unwrap_or_default();
                        let next_cursor = track_cursors_now.get(&id).cloned().flatten();
                        let track_error = track_errors_now.get(&id).cloned();
                        let indent_px = depth * 18;
                        let track_count = format_track_count(folder.track_count);
                        let id_for_tracks_toggle = id.clone();
                        let id_for_load_more = id.clone();
                        rsx! {
                            div {
                                key: "{id}",
                                class: if is_active || tracks_open {
                                    "grid grid-cols-[minmax(0,1fr)_auto] md:grid-cols-[minmax(0,1fr)_7rem] items-center gap-3 bg-primary/10 px-3 py-2"
                                } else {
                                    "grid grid-cols-[minmax(0,1fr)_auto] md:grid-cols-[minmax(0,1fr)_7rem] items-center gap-3 px-3 py-2 hover:bg-base-200/70"
                                },
                                div {
                                    class: "flex min-w-0 items-center gap-2",
                                    style: "padding-left: {indent_px}px",
                                    if folder.has_children {
                                        button {
                                            class: "btn btn-ghost btn-xs btn-circle flex-shrink-0",
                                            "aria-label": if is_expanded { "Collapse folder" } else { "Expand folder" },
                                            onclick: move |_| {
                                                let id = id_for_toggle.clone();
                                                if expanded.peek().contains(&id) {
                                                    expanded.write().remove(&id);
                                                    return;
                                                }

                                                expanded.write().insert(id.clone());
                                                if loaded_children.peek().contains(&id)
                                                    || loading_children.peek().contains(&id)
                                                {
                                                    return;
                                                }

                                                loading_children.write().insert(id.clone());
                                                let parent_id = id.clone();
                                                spawn(async move {
                                                    match use_music::list_music_child_folders(&parent_id).await {
                                                        Ok(children) => {
                                                            let mut current = folders.write();
                                                            let known: HashSet<String> = current
                                                                .iter()
                                                                .map(|folder| folder.folder_id.clone())
                                                                .collect();
                                                            current.extend(
                                                                children
                                                                    .into_iter()
                                                                    .filter(|child| !known.contains(&child.folder_id)),
                                                            );
                                                            loaded_children.write().insert(parent_id.clone());
                                                        }
                                                        Err(err) => error.set(Some(err)),
                                                    }
                                                    loading_children.write().remove(&parent_id);
                                                });
                                            },
                                            if is_loading {
                                                span { class: "loading loading-spinner loading-xs" }
                                            } else if is_expanded {
                                                IconChevronDown { class: "w-4 h-4".to_string() }
                                            } else {
                                                IconChevronRight { class: "w-4 h-4".to_string() }
                                            }
                                        }
                                    } else {
                                        span { class: "w-6 flex-shrink-0" }
                                    }
                                    button {
                                        class: if tracks_open {
                                            "flex min-w-0 flex-1 items-center gap-2 text-left text-primary"
                                        } else {
                                            "flex min-w-0 flex-1 items-center gap-2 text-left"
                                        },
                                        onclick: move |_| {
                                            let id = id_for_tracks_toggle.clone();
                                            if open_tracks.peek().contains(&id) {
                                                open_tracks.write().remove(&id);
                                                return;
                                            }

                                            open_tracks.write().insert(id.clone());
                                            if tracks_by_folder.peek().contains_key(&id)
                                                || loading_tracks.peek().contains(&id)
                                            {
                                                return;
                                            }

                                            loading_tracks.write().insert(id.clone());
                                            track_errors.write().remove(&id);
                                            let folder_id = id.clone();
                                            spawn(async move {
                                                match use_music::list_music_tracks(Some(&folder_id), None).await {
                                                    Ok(response) => {
                                                        tracks_by_folder.write().insert(
                                                            folder_id.clone(),
                                                            response.tracks,
                                                        );
                                                        track_cursors.write().insert(
                                                            folder_id.clone(),
                                                            response.next_cursor,
                                                        );
                                                    }
                                                    Err(err) => {
                                                        track_errors.write().insert(folder_id.clone(), err);
                                                    }
                                                }
                                                loading_tracks.write().remove(&folder_id);
                                            });
                                        },
                                        if tracks_open {
                                            IconChevronDown { class: "w-3.5 h-3.5 flex-shrink-0 opacity-70".to_string() }
                                            IconFolderOpen { class: "w-4 h-4 flex-shrink-0".to_string() }
                                        } else {
                                            IconChevronRight { class: "w-3.5 h-3.5 flex-shrink-0 opacity-40".to_string() }
                                            IconFolder { class: "w-4 h-4 flex-shrink-0 opacity-70".to_string() }
                                        }
                                        span { class: "truncate font-medium", "{folder.name}" }
                                        span { class: "hidden min-w-0 truncate text-xs text-base-content/50 sm:inline", "{folder.path}" }
                                    }
                                }
                                div { class: "flex items-center justify-end gap-2",
                                    span { class: "text-right text-sm text-base-content/60", "{track_count}" }
                                    div { class: "dropdown dropdown-end",
                                        button {
                                            class: "btn btn-ghost btn-xs btn-circle",
                                            tabindex: "0",
                                            IconMoreVertical { class: "w-4 h-4".to_string() }
                                        }
                                        ul {
                                            class: "dropdown-content menu menu-sm bg-base-200 rounded-box shadow z-10 w-52",
                                            tabindex: "0",
                                            li {
                                                a {
                                                    onclick: move |_| {
                                                        category_modal.set(Some((
                                                            folder.folder_id.clone(),
                                                            folder.name.clone(),
                                                        )));
                                                    },
                                                    "Manage categories..."
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            if tracks_open {
                                div {
                                    key: "{id}-tracks",
                                    class: "border-t border-base-200 bg-base-100/80 px-3 py-3",
                                    div {
                                        class: "space-y-2",
                                        style: "padding-left: {indent_px + 32}px",
                                        if let Some(err) = track_error {
                                            div { class: "alert alert-warning py-2 text-sm", "{err}" }
                                        }
                                        if tracks_loading && !has_loaded_tracks {
                                            div { class: "flex items-center gap-2 text-sm text-base-content/60",
                                                span { class: "loading loading-spinner loading-xs" }
                                                span { "Loading tracks..." }
                                            }
                                        } else if folder_tracks.is_empty() {
                                            div { class: "flex items-center gap-2 text-sm text-base-content/50",
                                                IconMusic { class: "w-4 h-4".to_string() }
                                                span { "No tracks directly in this folder" }
                                            }
                                        } else {
                                            InlineTrackRows {
                                                tracks: folder_tracks.clone(),
                                                current_playing_id: current_playing_id.clone(),
                                                player_is_playing,
                                            }
                                            if let Some(cursor) = next_cursor {
                                                button {
                                                    class: "btn btn-ghost btn-xs",
                                                    disabled: tracks_loading,
                                                    onclick: move |_| {
                                                        let folder_id = id_for_load_more.clone();
                                                        let cursor = cursor.clone();
                                                        spawn(async move {
                                                            loading_tracks.write().insert(folder_id.clone());
                                                            track_errors.write().remove(&folder_id);
                                                            match use_music::list_music_tracks(
                                                                Some(&folder_id),
                                                                Some(&cursor),
                                                            )
                                                            .await
                                                            {
                                                                Ok(response) => {
                                                                    tracks_by_folder
                                                                        .write()
                                                                        .entry(folder_id.clone())
                                                                        .or_default()
                                                                        .extend(response.tracks);
                                                                    track_cursors.write().insert(
                                                                        folder_id.clone(),
                                                                        response.next_cursor,
                                                                    );
                                                                }
                                                                Err(err) => {
                                                                    track_errors
                                                                        .write()
                                                                        .insert(folder_id.clone(), err);
                                                                }
                                                            }
                                                            loading_tracks.write().remove(&folder_id);
                                                        });
                                                    },
                                                    if tracks_loading {
                                                        span { class: "loading loading-spinner loading-xs" }
                                                    }
                                                    "Load more"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if let Some((folder_id, folder_name)) = category_modal() {
            ManageCategoriesModal {
                folder_id,
                folder_name,
                on_close: move |_| category_modal.set(None),
                on_changed: move |_| {},
            }
        }
    }
}

#[component]
fn InlineTrackRows(
    tracks: Vec<TrackResponse>,
    current_playing_id: Option<String>,
    player_is_playing: bool,
) -> Element {
    let mut player = use_context::<Signal<PlayerState>>();
    let tracks_for_queue = tracks.clone();

    rsx! {
        div { class: "overflow-hidden rounded-md border border-base-300 bg-base-100",
            for (idx, track) in tracks.iter().enumerate() {
                {
                    let title = track.audio.title.clone().unwrap_or_else(|| track.file.name.clone());
                    let artist = track
                        .audio
                        .artist
                        .clone()
                        .unwrap_or_else(|| "Unknown Artist".to_string());
                    let album = track
                        .audio
                        .album
                        .clone()
                        .unwrap_or_else(|| "Unknown Album".to_string());
                    let duration = track
                        .audio
                        .duration_secs
                        .map(format_duration)
                        .unwrap_or_else(|| "--:--".to_string());
                    let file_id = track.file.id.clone();
                    let is_current = current_playing_id.as_deref() == Some(&file_id);
                    let queue = tracks_for_queue.clone();

                    rsx! {
                        div {
                            key: "{file_id}",
                            class: if is_current && player_is_playing {
                                "grid grid-cols-[2rem_minmax(0,1fr)_auto] items-center gap-2 border-b border-base-200 px-2 py-2 last:border-b-0 bg-primary/10"
                            } else {
                                "grid grid-cols-[2rem_minmax(0,1fr)_auto] items-center gap-2 border-b border-base-200 px-2 py-2 last:border-b-0 hover:bg-base-200/70"
                            },
                            button {
                                class: "btn btn-ghost btn-xs btn-circle",
                                onclick: move |_| {
                                    if is_current {
                                        player.write().playing = !player_is_playing;
                                    } else {
                                        use_player::play_queue(player, queue.clone(), idx);
                                    }
                                },
                                if is_current && player_is_playing {
                                    IconPause { class: "w-4 h-4".to_string() }
                                } else {
                                    IconPlay { class: "w-4 h-4".to_string() }
                                }
                            }
                            div { class: "min-w-0",
                                div {
                                    class: if is_current && player_is_playing {
                                        "truncate text-sm font-medium text-primary"
                                    } else {
                                        "truncate text-sm font-medium"
                                    },
                                    "{title}"
                                }
                                div { class: "truncate text-xs text-base-content/50",
                                    "{artist} - {album}"
                                }
                            }
                            div { class: "text-xs tabular-nums text-base-content/50", "{duration}" }
                        }
                    }
                }
            }
        }
    }
}

fn visible_folder_rows(
    folders: &[MusicFolderResponse],
    expanded: &HashSet<String>,
) -> Vec<(MusicFolderResponse, usize)> {
    fn visit(
        folders: &[MusicFolderResponse],
        expanded: &HashSet<String>,
        parent_id: Option<&str>,
        depth: usize,
        rows: &mut Vec<(MusicFolderResponse, usize)>,
    ) {
        let mut children: Vec<&MusicFolderResponse> = folders
            .iter()
            .filter(|folder| folder.parent_folder_id.as_deref() == parent_id)
            .collect();
        children.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        for child in children {
            rows.push((child.clone(), depth));
            if expanded.contains(&child.folder_id) {
                visit(folders, expanded, Some(&child.folder_id), depth + 1, rows);
            }
        }
    }

    let mut rows = Vec::new();
    visit(folders, expanded, None, 0, &mut rows);
    rows
}

fn format_track_count(count: i64) -> String {
    match count {
        1 => "1 track".to_string(),
        n => format!("{n} tracks"),
    }
}

fn format_duration(secs: f64) -> String {
    let total = secs as u64;
    format!("{}:{:02}", total / 60, total % 60)
}
