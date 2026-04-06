use dioxus::prelude::*;
use crate::hooks::api;
use crate::router::Route;
use crate::state::ViewMode;

/// Clamp menu coordinates so the menu stays inside the viewport.
/// Uses conservative fixed dimensions: 180 × 260 px.
fn clamp_menu_pos(x: f64, y: f64) -> (f64, f64) {
    let (vw, vh) = web_sys::window()
        .map(|w| {
            let vw = w.inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(1024.0);
            let vh = w.inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(768.0);
            (vw, vh)
        })
        .unwrap_or((1024.0, 768.0));

    let menu_w = 180.0_f64;
    let menu_h = 260.0_f64;

    let cx = if x + menu_w > vw { vw - menu_w } else { x };
    // If not enough space below, flip above the cursor.
    let cy = if y + menu_h > vh { y - menu_h } else { y };

    (cx.max(0.0), cy.max(0.0))
}

#[component]
pub fn FileItem(
    id: String,
    name: String,
    is_folder: bool,
    size: Option<i64>,
    mime_type: Option<String>,
    view_mode: ViewMode,
    selected: bool,
    thumbnail_ver: u32,
    on_delete_request: EventHandler<()>,
    on_toggle_select: EventHandler<()>,
    on_rename_request: EventHandler<()>,
    on_move_request: EventHandler<()>,
    on_copy_request: EventHandler<()>,
    on_open_request: EventHandler<()>,
    on_edit_request: EventHandler<()>,
    on_version_history_request: EventHandler<()>,
    on_folder_settings_request: EventHandler<()>,
    #[props(default)]
    on_share_folder_request: EventHandler<()>,
    #[props(default)]
    shared_by: Option<String>,
) -> Element {
    let nav = use_navigator();
    // None = hidden; Some((x, y)) = visible at clamped viewport coordinates.
    let mut menu_pos: Signal<Option<(f64, f64)>> = use_signal(|| None);

    // Tracks which thumbnail version last failed to load.
    // If ver_when_failed == Some(thumbnail_ver), we show the fallback icon.
    // When thumbnail_ver increments (SSE event), the mismatch triggers a retry.
    let mut ver_when_failed: Signal<Option<u32>> = use_signal(|| None);

    let icon = if is_folder {
        "📁"
    } else {
        match mime_type.as_deref() {
            Some(t) if t.starts_with("image/") => "🖼️",
            Some(t) if t.starts_with("video/") => "🎬",
            Some(t) if t.starts_with("audio/") => "🎵",
            Some(t) if t.starts_with("text/") => "📝",
            Some("application/pdf") => "📕",
            Some(t) if t.contains("zip") || t.contains("tar") || t.contains("rar") => "📦",
            _ => "📄",
        }
    };

    let size_str = size
        .map(|s| uncloud_common::validation::format_bytes(s))
        .unwrap_or_else(|| "—".to_string());

    let type_str = if is_folder {
        "Folder".to_string()
    } else {
        match mime_type.as_deref() {
            Some(t) if t.starts_with("image/") => "Image",
            Some(t) if t.starts_with("video/") => "Video",
            Some(t) if t.starts_with("audio/") => "Audio",
            Some(t) if t.starts_with("text/") => "Text",
            Some("application/pdf") => "PDF",
            Some(t) if t.contains("zip") || t.contains("tar") || t.contains("rar") => "Archive",
            _ => "File",
        }
        .to_string()
    };

    let on_click = {
        let id = id.clone();
        move |_| {
            if is_folder {
                let _ = nav.push(Route::Folder { id: id.clone() });
            }
        }
    };

    let on_context_menu = move |evt: Event<MouseData>| {
        evt.prevent_default();
        let c = evt.client_coordinates();
        menu_pos.set(Some(clamp_menu_pos(c.x, c.y)));
    };

    let dom_id = format!("file-{}", id);

    match view_mode {
        ViewMode::Grid => rsx! {
            div {
                id: "{dom_id}",
                class: if selected {
                    "card bg-base-100 shadow-sm border border-base-300 ring-2 ring-primary cursor-pointer hover:shadow-md transition-all relative group"
                } else {
                    "card bg-base-100 shadow-sm border border-base-300 cursor-pointer hover:shadow-md hover:ring-2 hover:ring-primary transition-all relative group"
                },
                onclick: on_click,
                ondblclick: move |e| {
                    e.stop_propagation();
                    if !is_folder { on_open_request.call(()); }
                },
                oncontextmenu: on_context_menu,

                // Checkbox — always visible on mobile, hover-reveal on desktop
                div {
                    class: if selected {
                        "absolute top-2 left-2 z-10"
                    } else {
                        "absolute top-2 left-2 z-10 opacity-0 group-hover:opacity-100 transition-opacity"
                    },
                    onclick: move |e| {
                        e.stop_propagation();
                        on_toggle_select.call(());
                    },
                    input {
                        r#type: "checkbox",
                        class: "checkbox checkbox-sm",
                        checked: selected,
                        onchange: move |_| {},
                    }
                }

                // ⋮ button — always visible on mobile, hover-reveal on lg+
                button {
                    class: "absolute top-1 right-1 z-10 btn btn-ghost btn-xs opacity-100 lg:opacity-0 lg:group-hover:opacity-100 transition-opacity",
                    title: "More options",
                    onclick: move |e| {
                        e.stop_propagation();
                        let c = e.client_coordinates();
                        menu_pos.set(Some(clamp_menu_pos(c.x, c.y)));
                    },
                    "⋮"
                }

                div { class: "card-body p-0 gap-0",
                    // Thumbnail area
                    {
                        let is_image = mime_type.as_deref().map(|m| m.starts_with("image/")).unwrap_or(false);
                        let show_thumb = is_image && ver_when_failed() != Some(thumbnail_ver);
                        if show_thumb {
                            let src = api::authenticated_media_url(&format!("/files/{}/thumb?v={}", id, thumbnail_ver));
                            rsx! {
                                img {
                                    class: "w-full h-32 object-cover rounded-t-xl",
                                    src,
                                    onerror: move |_| ver_when_failed.set(Some(thumbnail_ver)),
                                }
                            }
                        } else {
                            rsx! {
                                div { class: "flex items-center justify-center h-32 text-4xl bg-base-200 rounded-t-xl",
                                    "{icon}"
                                }
                            }
                        }
                    }
                    div { class: "p-3 items-center text-center gap-1 flex flex-col",
                        div { class: "text-sm font-medium truncate w-full", title: "{name}",
                            if shared_by.is_some() && is_folder {
                                span { class: "mr-1 opacity-60", "👥" }
                            }
                            "{name}"
                        }
                        if !is_folder {
                            div { class: "text-xs text-base-content/50", "{size_str}" }
                        }
                        if let Some(ref owner) = shared_by {
                            div { class: "text-xs text-base-content/40", "Shared by {owner}" }
                        }
                    }
                }

                if let Some((x, y)) = menu_pos() {
                    FileContextMenu {
                        x, y,
                        id: id.clone(),
                        name: name.clone(),
                        is_folder,
                        mime_type: mime_type.clone(),
                        shared_by: shared_by.clone(),
                        on_close: move |_| menu_pos.set(None),
                        on_open_request: move |_| on_open_request.call(()),
                        on_edit_request: move |_| on_edit_request.call(()),
                        on_delete_request,
                        on_rename: move |_| on_rename_request.call(()),
                        on_move: move |_| on_move_request.call(()),
                        on_copy: move |_| on_copy_request.call(()),
                        on_version_history: move |_| on_version_history_request.call(()),
                        on_folder_settings: move |_| on_folder_settings_request.call(()),
                        on_share_folder: move |_| on_share_folder_request.call(()),
                    }
                }
            }
        },

        ViewMode::List => rsx! {
            tr {
                id: "{dom_id}",
                class: if selected {
                    "cursor-pointer bg-primary/5"
                } else {
                    "hover cursor-pointer"
                },
                onclick: on_click,
                ondblclick: move |e| {
                    e.stop_propagation();
                    if !is_folder { on_open_request.call(()); }
                },
                oncontextmenu: on_context_menu,

                // Checkbox column
                td { class: "w-8 py-2",
                    div {
                        onclick: move |e| {
                            e.stop_propagation();
                            on_toggle_select.call(());
                        },
                        input {
                            r#type: "checkbox",
                            class: "checkbox checkbox-sm",
                            checked: selected,
                            onchange: move |_| {},
                        }
                    }
                }
                td { class: "w-8 text-lg py-2", "{icon}" }
                td { class: "font-medium",
                    span { title: "{name}",
                        if shared_by.is_some() && is_folder {
                            span { class: "mr-1 opacity-60", "👥" }
                        }
                        "{name}"
                    }
                    if let Some(ref owner) = shared_by {
                        span { class: "text-xs text-base-content/40 ml-2",
                            "Shared by {owner}"
                        }
                    }
                }
                td { class: "text-base-content/50 text-sm hidden sm:table-cell", "{type_str}" }
                td { class: "text-right text-sm text-base-content/50 hidden sm:table-cell tabular-nums",
                    "{size_str}"
                }
                td { class: "w-8 text-right",
                    button {
                        class: "btn btn-ghost btn-xs",
                        onclick: move |e| {
                            e.stop_propagation();
                            let c = e.client_coordinates();
                            menu_pos.set(Some(clamp_menu_pos(c.x, c.y)));
                        },
                        "⋮"
                    }
                }
            }

            if let Some((x, y)) = menu_pos() {
                FileContextMenu {
                    x, y,
                    id: id.clone(),
                    name: name.clone(),
                    is_folder,
                    mime_type: mime_type.clone(),
                    shared_by: shared_by.clone(),
                    on_close: move |_| menu_pos.set(None),
                    on_open_request: move |_| on_open_request.call(()),
                    on_edit_request: move |_| on_edit_request.call(()),
                    on_delete_request,
                    on_rename: move |_| on_rename_request.call(()),
                    on_move: move |_| on_move_request.call(()),
                    on_copy: move |_| on_copy_request.call(()),
                    on_version_history: move |_| on_version_history_request.call(()),
                    on_folder_settings: move |_| on_folder_settings_request.call(()),
                    on_share_folder: move |_| on_share_folder_request.call(()),
                }
            }
        },
    }
}

#[component]
fn FileContextMenu(
    /// Viewport X coordinate (already clamped to stay on-screen).
    x: f64,
    /// Viewport Y coordinate (already clamped to stay on-screen).
    y: f64,
    id: String,
    name: String,
    is_folder: bool,
    mime_type: Option<String>,
    shared_by: Option<String>,
    on_close: EventHandler<()>,
    on_open_request: EventHandler<()>,
    on_edit_request: EventHandler<()>,
    on_delete_request: EventHandler<()>,
    on_rename: EventHandler<()>,
    on_move: EventHandler<()>,
    on_copy: EventHandler<()>,
    on_version_history: EventHandler<()>,
    on_folder_settings: EventHandler<()>,
    on_share_folder: EventHandler<()>,
) -> Element {
    let is_editable_text = !is_folder && mime_type.as_deref()
        .map(|m| m.starts_with("text/") || m == "application/json" || m == "application/xml")
        .unwrap_or(false);
    let on_download = {
        let id = id.clone();
        move |_| {
            if !is_folder {
                let url = api::authenticated_media_url(&format!("/files/{}/download", id));
                let _ = web_sys::window().and_then(|w| w.open_with_url(&url).ok());
            }
            on_close.call(());
        }
    };

    rsx! {
        div {
            class: "fixed inset-0 z-40",
            onclick: move |_| on_close.call(()),
        }
        ul {
            class: "menu menu-sm bg-base-100 rounded-box shadow-lg border border-base-300 w-44 p-1 z-50",
            style: "position: fixed; left: {x:.0}px; top: {y:.0}px;",
            onclick: move |evt| evt.stop_propagation(),

            if !is_folder {
                li {
                    a { onclick: move |_| { on_open_request.call(()); on_close.call(()); },
                        span { "\u{1F441}" }
                        span { "Open" }
                    }
                }
                if is_editable_text {
                    li {
                        a { onclick: move |_| { on_edit_request.call(()); on_close.call(()); },
                            span { "✏️" }
                            span { "Edit" }
                        }
                    }
                }
                li {
                    a { onclick: on_download,
                        span { "⬇️" }
                        span { "Download" }
                    }
                }
            }
            li {
                a { onclick: move |_| { on_rename.call(()); on_close.call(()); },
                    span { "✏️" }
                    span { "Rename" }
                }
            }
            li {
                a { onclick: move |_| { on_move.call(()); on_close.call(()); },
                    span { "📂" }
                    span { "Move to…" }
                }
            }
            // Hide Copy for shared folders (they are mounted references)
            if !(is_folder && shared_by.is_some()) {
                li {
                    a { onclick: move |_| { on_copy.call(()); on_close.call(()); },
                        span { "📋" }
                        span { "Copy" }
                    }
                }
            }
            if is_folder {
                li {
                    a { onclick: move |_| { on_share_folder.call(()); on_close.call(()); },
                        span { "👥" }
                        span { "Share folder\u{2026}" }
                    }
                }
            }
            li {
                a { onclick: move |_| on_close.call(()),
                    span { "🔗" }
                    span { "Share link" }
                }
            }
            if !is_folder {
                li {
                    a { onclick: move |_| { on_version_history.call(()); on_close.call(()); },
                        span { "🕓" }
                        span { "Version history" }
                    }
                }
            }
            if is_folder {
                li {
                    a { onclick: move |_| { on_folder_settings.call(()); on_close.call(()); },
                        span { "\u{2699}\u{FE0F}" }
                        span { "Folder settings\u{2026}" }
                    }
                }
            }
            li { div { class: "divider my-0" } }
            li {
                a {
                    class: "text-error",
                    onclick: move |_| { on_delete_request.call(()); on_close.call(()); },
                    span { "🗑️" }
                    span { "Delete" }
                }
            }
        }
    }
}
