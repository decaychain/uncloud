use dioxus::prelude::*;
use uncloud_common::TrackResponse;
use wasm_bindgen::JsCast;
use crate::components::icons::{IconAlertTriangle, IconGripVertical, IconMoreVertical, IconMusic, IconPause, IconPencil, IconPin, IconPinOff, IconPlay, IconTrash, IconX};
use crate::hooks::{use_playlists, use_player};
use crate::router::Route;
use crate::state::{PinnedPlaylistState, PlayerState};

/// Milliseconds the user must hold a touch on a row before it enters drag
/// mode. Matches Android's typical long-press threshold.
const LONG_PRESS_MS: u32 = 450;

/// Finger movement (in CSS px) that cancels a pending long-press. Also used
/// as the jitter tolerance before "press" becomes "drag".
const MOVE_THRESHOLD_PX: f64 = 10.0;

fn format_duration(secs: f64) -> String {
    let total = secs as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

/// Walk up the DOM from `start` looking for an element with a `data-row-idx`
/// attribute and return its parsed value.
fn row_idx_at_point(x: f64, y: f64) -> Option<usize> {
    let doc = web_sys::window()?.document()?;
    let mut current = doc.element_from_point(x as f32, y as f32);
    while let Some(el) = current {
        if let Some(attr) = el.get_attribute("data-row-idx") {
            if let Ok(n) = attr.parse::<usize>() {
                return Some(n);
            }
        }
        current = el.parent_element();
    }
    None
}

fn haptic_blip() {
    if let Some(nav) = web_sys::window().map(|w| w.navigator()) {
        let _ = nav.vibrate_with_duration(15);
    }
}

#[component]
pub fn PlaylistView(playlist_id: String) -> Element {
    let mut player = use_context::<Signal<PlayerState>>();
    let mut pinned = use_context::<Signal<PinnedPlaylistState>>();
    let nav = use_navigator();
    let is_pinned = pinned().0.as_deref() == Some(playlist_id.as_str());
    let pid_for_pin = playlist_id.clone();
    let mut tracks: Signal<Vec<TrackResponse>> = use_signal(Vec::new);
    let mut playlist_name: Signal<String> = use_signal(|| String::new());
    let mut playlist_desc: Signal<Option<String>> = use_signal(|| None);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut rename_open: Signal<bool> = use_signal(|| false);
    let mut delete_open: Signal<bool> = use_signal(|| false);

    // Drag state.
    //   drag_idx: index being dragged (source row)
    //   drop_idx: index currently under the pointer (destination)
    //   dragging: true once drag mode has actually engaged (handle-press or
    //             long-press fired). Distinct from "drag_idx.is_some()" so we
    //             can distinguish the pending long-press phase from active drag.
    //   press_seq: monotonic token. Incremented on every pointerup/cancel; a
    //             spawned long-press task only fires if its captured seq still
    //             matches, which lets us cancel safely without managing a
    //             setTimeout handle.
    //   press_origin: pointerdown client xy. Used to cancel long-press if the
    //             user moves more than MOVE_THRESHOLD_PX before it fires.
    let mut drag_idx: Signal<Option<usize>> = use_signal(|| None);
    let mut drop_idx: Signal<Option<usize>> = use_signal(|| None);
    let mut dragging: Signal<bool> = use_signal(|| false);
    let mut press_seq: Signal<u32> = use_signal(|| 0);
    let mut press_origin: Signal<Option<(f64, f64)>> = use_signal(|| None);

    let pid_for_remove = playlist_id.clone();
    let pid_for_reorder = playlist_id.clone();
    let pid_for_rename = playlist_id.clone();
    let pid_for_delete = playlist_id.clone();

    use_effect(use_reactive!(|(playlist_id)| {
        spawn(async move {
            error.set(None);
            match use_playlists::get_playlist(&playlist_id).await {
                Ok(resp) => {
                    playlist_name.set(resp.name);
                    playlist_desc.set(resp.description);
                    tracks.set(resp.tracks);
                }
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
                h3 { class: "text-lg font-semibold", "Error loading playlist" }
                p { class: "text-base-content/60", "{err}" }
            }
        };
    }

    let track_list = tracks();
    let total_tracks = track_list.len();
    let total_duration: f64 = track_list.iter().filter_map(|t| t.audio.duration_secs).sum();
    let total_dur_str = if total_duration > 0.0 {
        let total_mins = (total_duration / 60.0).round() as u64;
        if total_mins >= 60 {
            format!("{} hr {} min", total_mins / 60, total_mins % 60)
        } else {
            format!("{} min", total_mins)
        }
    } else {
        String::new()
    };

    let current_playing_id = player().current_track().map(|t| t.file.id.clone());
    let is_playing = player().playing;

    let tracks_for_play_all = track_list.clone();
    let dragging_now = dragging();
    let is_dragging = dragging_now || drag_idx().is_some();

    // Cancel any pending long-press and clear press origin in one place.
    let mut cancel_pending_press = move || {
        press_origin.set(None);
        press_seq += 1;
    };

    // Commit the reorder to the server and local state, then clear drag state.
    let mut finish_drag = move || {
        if let (Some(from), Some(to)) = (drag_idx.peek().clone(), drop_idx.peek().clone()) {
            if from != to && from < tracks.peek().len() {
                let mut t = tracks.write();
                let item = t.remove(from);
                let to_clamped = to.min(t.len());
                t.insert(to_clamped, item);
                let ids: Vec<String> = t.iter().map(|tr| tr.file.id.clone()).collect();
                drop(t);
                let pid = pid_for_reorder.clone();
                spawn(async move {
                    let id_refs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
                    let _ = use_playlists::reorder_playlist(&pid, &id_refs).await;
                });
            }
        }
        drag_idx.set(None);
        drop_idx.set(None);
        dragging.set(false);
    };

    // Container classes — when dragging, disable text selection and (importantly
    // for Android) touch-action: none so the browser won't intercept the drag
    // as a scroll.
    let container_class = if is_dragging {
        "overflow-hidden rounded-box border border-base-300 select-none"
    } else {
        "overflow-hidden rounded-box border border-base-300"
    };
    let container_style = if is_dragging {
        "touch-action: none;"
    } else {
        ""
    };

    rsx! {
        div { class: "space-y-4",
            // Header
            div { class: "flex items-start justify-between gap-2",
                div { class: "min-w-0",
                    h2 { class: "text-2xl font-bold truncate", "{playlist_name}" }
                    if let Some(desc) = playlist_desc() {
                        p { class: "text-base-content/60 mt-1", "{desc}" }
                    }
                    p { class: "text-sm text-base-content/50 mt-1",
                        "{total_tracks} tracks"
                        if !total_dur_str.is_empty() {
                            " \u{00B7} {total_dur_str}"
                        }
                    }
                }
                div { class: "flex items-center gap-2 shrink-0",
                    if !track_list.is_empty() {
                        button {
                            class: "btn btn-primary btn-sm",
                            onclick: move |_| use_player::play_queue(player, tracks_for_play_all.clone(), 0),
                            IconPlay { class: "w-4 h-4".to_string() }
                            "Play All"
                        }
                    }
                    // Pin/unpin to right side panel — visible only on xl+
                    // viewports where the panel can render.
                    button {
                        class: if is_pinned {
                            "hidden xl:inline-flex btn btn-sm btn-primary"
                        } else {
                            "hidden xl:inline-flex btn btn-sm btn-ghost"
                        },
                        title: if is_pinned { "Unpin from side panel" } else { "Pin to side panel" },
                        onclick: move |_| {
                            if is_pinned {
                                pinned.set(PinnedPlaylistState(None));
                            } else {
                                pinned.set(PinnedPlaylistState(Some(pid_for_pin.clone())));
                            }
                        },
                        if is_pinned {
                            IconPinOff { class: "w-4 h-4".to_string() }
                            "Unpin"
                        } else {
                            IconPin { class: "w-4 h-4".to_string() }
                            "Pin"
                        }
                    }
                    div { class: "dropdown dropdown-end",
                        div {
                            tabindex: "0",
                            role: "button",
                            class: "btn btn-ghost btn-sm btn-circle",
                            title: "Playlist actions",
                            IconMoreVertical { class: "w-4 h-4".to_string() }
                        }
                        ul {
                            tabindex: "0",
                            class: "menu menu-sm dropdown-content bg-base-100 rounded-box z-50 mt-2 w-44 p-2 shadow border border-base-300",
                            li {
                                a {
                                    onclick: move |_| {
                                        rename_open.set(true);
                                        // Drop focus so the dropdown closes.
                                        if let Some(active) = web_sys::window()
                                            .and_then(|w| w.document())
                                            .and_then(|d| d.active_element())
                                        {
                                            let _ = active.dyn_ref::<web_sys::HtmlElement>().map(|el| el.blur());
                                        }
                                    },
                                    IconPencil { class: "w-4 h-4".to_string() }
                                    "Rename"
                                }
                            }
                            li {
                                a {
                                    class: "text-error",
                                    onclick: move |_| {
                                        delete_open.set(true);
                                        if let Some(active) = web_sys::window()
                                            .and_then(|w| w.document())
                                            .and_then(|d| d.active_element())
                                        {
                                            let _ = active.dyn_ref::<web_sys::HtmlElement>().map(|el| el.blur());
                                        }
                                    },
                                    IconTrash { class: "w-4 h-4".to_string() }
                                    "Delete\u{2026}"
                                }
                            }
                        }
                    }
                }
            }

            if track_list.is_empty() {
                div { class: "flex flex-col items-center justify-center py-12 gap-3",
                    IconMusic { class: "w-10 h-10 text-base-content/30".to_string() }
                    p { class: "text-base-content/60", "This playlist is empty. Add tracks from the music library." }
                }
            } else {
                div {
                    class: "{container_class}",
                    style: "{container_style}",
                    // Commit the drop on release anywhere inside the table.
                    onpointerup: move |_| {
                        cancel_pending_press();
                        if drag_idx.peek().is_some() {
                            finish_drag();
                        }
                    },
                    // Cancel on pointer leaving the container (mouse drag drifts off).
                    onpointerleave: move |_| {
                        if dragging() {
                            drag_idx.set(None);
                            drop_idx.set(None);
                            dragging.set(false);
                        }
                        cancel_pending_press();
                    },
                    // Cancel if the system interrupts the gesture (e.g. Android
                    // back-swipe, notification).
                    onpointercancel: move |_| {
                        drag_idx.set(None);
                        drop_idx.set(None);
                        dragging.set(false);
                        cancel_pending_press();
                    },
                    table { class: "table table-sm w-full",
                        thead {
                            tr {
                                th { class: "w-8 px-1" }  // drag handle
                                th { class: "w-8" }  // play button
                                th { class: "w-10 text-center", "#" }
                                th { "Title" }
                                th { class: "hidden sm:table-cell", "Artist" }
                                th { class: "hidden md:table-cell", "Album" }
                                th { class: "w-16 text-right", "Duration" }
                                th { class: "w-10 text-center", "" }  // remove
                            }
                        }
                        tbody {
                            for (idx, track) in track_list.iter().enumerate() {
                                {
                                    let title = track.audio.title.as_deref()
                                        .unwrap_or(&track.file.name).to_string();
                                    let artist = track.audio.artist.as_deref()
                                        .unwrap_or("Unknown").to_string();
                                    let album = track.audio.album.as_deref()
                                        .unwrap_or("Unknown").to_string();
                                    let duration = track.audio.duration_secs
                                        .map(format_duration)
                                        .unwrap_or_else(|| "--:--".to_string());
                                    let file_id = track.file.id.clone();
                                    let is_current = current_playing_id.as_deref() == Some(&track.file.id);
                                    let tracks_for_play = track_list.clone();
                                    let pid_rm = pid_for_remove.clone();

                                    let is_drag_source = drag_idx() == Some(idx);
                                    let is_drop_target = is_dragging && drop_idx() == Some(idx) && !is_drag_source;

                                    let row_class = if is_drag_source {
                                        "opacity-30"
                                    } else if is_drop_target {
                                        if drag_idx().unwrap_or(0) > idx {
                                            "border-t-2 border-t-primary bg-primary/5"
                                        } else {
                                            "border-b-2 border-b-primary bg-primary/5"
                                        }
                                    } else if is_current && is_playing {
                                        "hover:bg-base-200 bg-primary/10"
                                    } else {
                                        "hover:bg-base-200"
                                    };

                                    rsx! {
                                        tr {
                                            class: "{row_class} group transition-colors",
                                            "data-row-idx": "{idx}",
                                            // Any pointer pressing on the row starts a possible long-press.
                                            // (Mouse presses are ignored — desktop uses the grip handle for
                                            // instant drag.) Buttons and the grip cell call stop_propagation
                                            // so they never reach this handler.
                                            onpointerdown: move |e: Event<PointerData>| {
                                                let pt = e.pointer_type();
                                                if pt != "touch" && pt != "pen" {
                                                    return;
                                                }
                                                let p = e.client_coordinates();
                                                let (x, y) = (p.x, p.y);
                                                press_origin.set(Some((x, y)));
                                                let my_seq = *press_seq.peek() + 1;
                                                press_seq.set(my_seq);
                                                let my_idx = idx;
                                                spawn(async move {
                                                    gloo_timers::future::TimeoutFuture::new(LONG_PRESS_MS).await;
                                                    // Still the most recent press, and finger hasn't lifted or
                                                    // moved out of tolerance?
                                                    if *press_seq.peek() == my_seq
                                                        && press_origin.peek().is_some()
                                                    {
                                                        drag_idx.set(Some(my_idx));
                                                        drop_idx.set(Some(my_idx));
                                                        dragging.set(true);
                                                        haptic_blip();
                                                    }
                                                });
                                            },
                                            // Every move updates drop target during drag, and cancels a
                                            // pending long-press if the finger wandered.
                                            onpointermove: move |e: Event<PointerData>| {
                                                let p = e.client_coordinates();
                                                let (x, y) = (p.x, p.y);

                                                if !dragging() {
                                                    let origin = *press_origin.peek();
                                                    if let Some((ox, oy)) = origin {
                                                        let dx = x - ox;
                                                        let dy = y - oy;
                                                        if (dx * dx + dy * dy).sqrt() > MOVE_THRESHOLD_PX {
                                                            press_origin.set(None);
                                                            press_seq += 1;
                                                        }
                                                    }
                                                }

                                                if !drag_idx.peek().is_some() {
                                                    return;
                                                }
                                                let pt = e.pointer_type();
                                                if pt == "touch" || pt == "pen" {
                                                    // Touch pointer is implicitly captured to the origin row,
                                                    // so pointerenter on sibling rows never fires on mobile
                                                    // — walk the DOM at the current coordinate instead.
                                                    if let Some(n) = row_idx_at_point(x, y) {
                                                        if *drop_idx.peek() != Some(n) {
                                                            drop_idx.set(Some(n));
                                                        }
                                                    }
                                                } else if *drop_idx.peek() != Some(idx) {
                                                    // Mouse: we're hovering this row by virtue of the event
                                                    // firing on it.
                                                    drop_idx.set(Some(idx));
                                                }
                                            },
                                            // Drag handle
                                            td {
                                                class: "px-1 cursor-grab active:cursor-grabbing",
                                                style: "touch-action: none;",
                                                onpointerdown: move |e: Event<PointerData>| {
                                                    // Starting from the handle means "drag right now",
                                                    // regardless of pointer type. Stop propagation so the
                                                    // row doesn't also arm its long-press timer.
                                                    e.stop_propagation();
                                                    drag_idx.set(Some(idx));
                                                    drop_idx.set(Some(idx));
                                                    dragging.set(true);
                                                },
                                                IconGripVertical { class: "w-4 h-4 text-base-content/30".to_string() }
                                            }
                                            // Play / pause button
                                            td { class: "text-center",
                                                button {
                                                    class: "btn btn-ghost btn-xs btn-circle",
                                                    // Interactive buttons must not arm long-press.
                                                    onpointerdown: move |e: Event<PointerData>| {
                                                        e.stop_propagation();
                                                    },
                                                    onclick: move |_| {
                                                        if is_current {
                                                            player.write().playing = !is_playing;
                                                        } else {
                                                            use_player::play_queue(player, tracks_for_play.clone(), idx);
                                                        }
                                                    },
                                                    if is_current && is_playing {
                                                        IconPause { class: "w-3 h-3".to_string() }
                                                    } else {
                                                        IconPlay { class: "w-3 h-3".to_string() }
                                                    }
                                                }
                                            }
                                            td { class: "text-center text-base-content/50 tabular-nums", "{idx + 1}" }
                                            td { class: "font-medium truncate max-w-xs",
                                                if is_current && is_playing {
                                                    span { class: "text-primary", title: "{title}", "{title}" }
                                                } else {
                                                    span { title: "{title}", "{title}" }
                                                }
                                            }
                                            td { class: "hidden sm:table-cell text-base-content/70 truncate max-w-xs", "{artist}" }
                                            td { class: "hidden md:table-cell text-base-content/70 truncate max-w-xs", "{album}" }
                                            td { class: "text-right text-base-content/50 tabular-nums", "{duration}" }
                                            td { class: "text-center",
                                                button {
                                                    class: "btn btn-ghost btn-xs btn-circle text-error opacity-0 group-hover:opacity-100 transition-opacity",
                                                    title: "Remove from playlist",
                                                    onpointerdown: move |e: Event<PointerData>| {
                                                        e.stop_propagation();
                                                    },
                                                    onclick: move |_| {
                                                        let fid = file_id.clone();
                                                        let pid = pid_rm.clone();
                                                        // Optimistic update — drop the row from the local
                                                        // tracks signal so the table re-renders immediately.
                                                        tracks.write().retain(|t| t.file.id != fid);
                                                        spawn(async move {
                                                            let _ = use_playlists::remove_from_playlist(&pid, &[&fid]).await;
                                                        });
                                                    },
                                                    IconX { class: "w-3 h-3".to_string() }
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

            if rename_open() {
                RenamePlaylistModal {
                    playlist_id: pid_for_rename.clone(),
                    current_name: playlist_name(),
                    current_description: playlist_desc(),
                    on_cancel: move |_| rename_open.set(false),
                    on_renamed: move |(new_name, new_desc): (String, Option<String>)| {
                        playlist_name.set(new_name);
                        playlist_desc.set(new_desc);
                        rename_open.set(false);
                    },
                }
            }

            if delete_open() {
                DeletePlaylistModal {
                    playlist_id: pid_for_delete.clone(),
                    name: playlist_name(),
                    on_cancel: move |_| delete_open.set(false),
                    on_deleted: move |_| {
                        delete_open.set(false);
                        if is_pinned {
                            pinned.set(PinnedPlaylistState(None));
                        }
                        let _ = nav.replace(Route::Music {});
                    },
                }
            }
        }
    }
}

#[component]
fn RenamePlaylistModal(
    playlist_id: String,
    current_name: String,
    current_description: Option<String>,
    on_cancel: EventHandler<()>,
    on_renamed: EventHandler<(String, Option<String>)>,
) -> Element {
    let mut name = use_signal(|| current_name.clone());
    let initial_desc = current_description.clone().unwrap_or_default();
    let mut desc = use_signal(|| initial_desc.clone());
    let mut saving = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let cn = current_name.clone();
    let cd = current_description.clone();

    let on_submit = move |e: Event<FormData>| {
        e.prevent_default();
        let new_name = name().trim().to_string();
        let new_desc_raw = desc().trim().to_string();
        let new_desc = if new_desc_raw.is_empty() { None } else { Some(new_desc_raw) };

        if new_name.is_empty() {
            error.set(Some("Name cannot be empty".to_string()));
            return;
        }
        if new_name == cn && new_desc == cd {
            on_cancel.call(());
            return;
        }

        let id = playlist_id.clone();
        let name_to_send = if new_name == cn { None } else { Some(new_name.clone()) };
        let desc_to_send = if new_desc == cd { None } else { new_desc.clone() };
        saving.set(true);
        error.set(None);

        spawn(async move {
            // The API treats `Some("")` as "set to empty", which is what we want
            // when the user clears the field. We pass empty string for None
            // because the request body uses Option<String> and our hook signature
            // takes Option<&str>.
            let desc_param: Option<String> = if desc_to_send.is_none() && new_desc.is_none() {
                // user wants no description; backend default treats omission as no-change,
                // so explicitly send empty to clear it.
                Some(String::new())
            } else {
                desc_to_send
            };
            let res = use_playlists::update_playlist(
                &id,
                name_to_send.as_deref(),
                desc_param.as_deref(),
            ).await;
            match res {
                Ok(_) => on_renamed.call((new_name.clone(), new_desc.clone())),
                Err(e) if e == "CONFLICT" => {
                    error.set(Some("A playlist with this name already exists".to_string()));
                    saving.set(false);
                }
                Err(e) => {
                    error.set(Some(e));
                    saving.set(false);
                }
            }
        });
    };

    rsx! {
        div { class: "modal modal-open",
            div { class: "modal-box max-w-sm",
                h3 { class: "font-bold text-lg mb-4", "Rename playlist" }

                form { onsubmit: on_submit,
                    div { class: "form-control",
                        label { class: "label", span { class: "label-text", "Name" } }
                        input {
                            class: "input input-bordered w-full",
                            autofocus: true,
                            value: "{name}",
                            oninput: move |e| name.set(e.value()),
                        }
                    }
                    div { class: "form-control mt-2",
                        label { class: "label", span { class: "label-text", "Description" } }
                        input {
                            class: "input input-bordered w-full",
                            placeholder: "Optional",
                            value: "{desc}",
                            oninput: move |e| desc.set(e.value()),
                        }
                    }

                    if let Some(err) = error() {
                        div { class: "alert alert-error mt-3 py-2 text-sm", "{err}" }
                    }

                    div { class: "modal-action",
                        button {
                            class: "btn btn-ghost",
                            r#type: "button",
                            disabled: saving(),
                            onclick: move |_| on_cancel.call(()),
                            "Cancel"
                        }
                        button {
                            class: "btn btn-primary",
                            r#type: "submit",
                            disabled: saving(),
                            if saving() { span { class: "loading loading-spinner loading-sm" } }
                            "Save"
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn DeletePlaylistModal(
    playlist_id: String,
    name: String,
    on_cancel: EventHandler<()>,
    on_deleted: EventHandler<()>,
) -> Element {
    let mut deleting = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    rsx! {
        div { class: "modal modal-open",
            div { class: "modal-box max-w-sm",
                h3 { class: "font-bold text-lg mb-2", "Delete \"{name}\"?" }
                p { class: "text-sm text-base-content/70 mb-4",
                    "The playlist will be permanently deleted. The tracks themselves are not affected."
                }
                if let Some(err) = error() {
                    div { class: "alert alert-error mb-3 py-2 text-sm", "{err}" }
                }
                div { class: "modal-action",
                    button {
                        class: "btn btn-ghost",
                        disabled: deleting(),
                        onclick: move |_| on_cancel.call(()),
                        "Cancel"
                    }
                    button {
                        class: "btn btn-error",
                        disabled: deleting(),
                        onclick: move |_| {
                            deleting.set(true);
                            error.set(None);
                            let id = playlist_id.clone();
                            spawn(async move {
                                match use_playlists::delete_playlist(&id).await {
                                    Ok(()) => on_deleted.call(()),
                                    Err(e) => {
                                        error.set(Some(e));
                                        deleting.set(false);
                                    }
                                }
                            });
                        },
                        if deleting() { span { class: "loading loading-spinner loading-sm" } }
                        "Delete"
                    }
                }
            }
        }
    }
}
