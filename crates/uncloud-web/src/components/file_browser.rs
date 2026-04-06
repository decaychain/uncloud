use std::collections::{HashMap, HashSet};
use dioxus::prelude::*;
use gloo_storage::{LocalStorage, Storage};
use uncloud_common::{AudioMeta, EffectiveStrategyResponse, FileResponse, FolderResponse, GalleryInclude, MusicInclude, ServerEvent, SyncStrategy, TrackResponse};
use crate::components::file_item::FileItem;
use crate::components::upload::{UploadZone, FILE_INPUT_ID};
use web_sys::wasm_bindgen::JsCast;
use crate::hooks::{use_files, use_player};
use crate::router::Route;
use crate::state::{HighlightTarget, PlayerState, VaultOpenTarget, ViewMode};

// ── Selection state ───────────────────────────────────────────────────────────

#[derive(Clone, Default)]
struct Selection {
    files: HashSet<String>,
    folders: HashSet<String>,
}

impl Selection {
    fn total(&self) -> usize {
        self.files.len() + self.folders.len()
    }

    fn contains_file(&self, id: &str) -> bool {
        self.files.contains(id)
    }

    fn contains_folder(&self, id: &str) -> bool {
        self.folders.contains(id)
    }

    fn toggle_file(&mut self, id: String) {
        if !self.files.remove(&id) {
            self.files.insert(id);
        }
    }

    fn toggle_folder(&mut self, id: String) {
        if !self.folders.remove(&id) {
            self.folders.insert(id);
        }
    }

    fn clear(&mut self) {
        self.files.clear();
        self.folders.clear();
    }

    fn items_with_names(
        &self,
        files: &[FileResponse],
        folders: &[FolderResponse],
    ) -> Vec<(String, bool, String)> {
        let mut out = Vec::new();
        for f in files {
            if self.files.contains(&f.id) {
                out.push((f.id.clone(), false, f.name.clone()));
            }
        }
        for f in folders {
            if self.folders.contains(&f.id) {
                out.push((f.id.clone(), true, f.name.clone()));
            }
        }
        out
    }
}

// ── ViewerTarget ─────────────────────────────────────────────────────────────

#[derive(Clone)]
enum ViewerTarget {
    Image { files: Vec<FileResponse>, index: usize },
    Text(FileResponse),
    TextEdit(FileResponse),
}

// ── FileBrowser ───────────────────────────────────────────────────────────────

#[component]
pub fn FileBrowser(parent_id: Option<String>) -> Element {
    let mut files = use_signal(Vec::<FileResponse>::new);
    let mut folders = use_signal(Vec::<FolderResponse>::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| None::<String>);
    let mut show_new_folder = use_signal(|| false);
    let mut show_new_file = use_signal(|| false);
    let mut refresh = use_signal(|| 0u32);
    let mut view_mode = use_context::<Signal<ViewMode>>();

    // Selection
    let mut selection = use_signal(Selection::default);

    // Modal targets
    // (id, is_folder, current_name)
    let mut rename_target: Signal<Option<(String, bool, String)>> = use_signal(|| None);
    // (items: Vec<(id, is_folder, name)>, is_copy)
    let mut move_target: Signal<Option<(Vec<(String, bool, String)>, bool)>> = use_signal(|| None);
    // Bulk delete confirmation: true = show modal
    let mut bulk_delete_confirm = use_signal(|| false);
    // Single-item delete confirmation: Some((id, is_folder, name))
    let mut delete_target: Signal<Option<(String, bool, String)>> = use_signal(|| None);
    // Folder settings modal target: Some((folder_id, folder_name, gallery_include, music_include))
    let mut folder_settings_target: Signal<Option<(String, String, GalleryInclude, MusicInclude)>> = use_signal(|| None);
    // File viewer target
    let mut viewer_target: Signal<Option<ViewerTarget>> = use_signal(|| None);
    // Version history modal target: Some((file_id, file_name))
    let mut version_history_target: Signal<Option<(String, String)>> = use_signal(|| None);

    let player = use_context::<Signal<PlayerState>>();
    let mut vault_open_target = use_context::<Signal<VaultOpenTarget>>();
    let nav = use_navigator();

    // Thumbnail version counters — incremented when ProcessingCompleted arrives for a file.
    let mut thumb_vers: Signal<HashMap<String, u32>> = use_signal(HashMap::new);

    let sse_event = use_context::<Signal<Option<ServerEvent>>>();
    use_effect(move || {
        if let Some(event) = sse_event() {
            match event {
                ServerEvent::ProcessingCompleted { file_id, task_type, success } => {
                    if task_type == "thumbnail" && success {
                        *thumb_vers.write().entry(file_id).or_insert(0) += 1;
                    }
                }
                ServerEvent::FileCreated { .. } | ServerEvent::FileUpdated { .. }
                | ServerEvent::FileDeleted { .. } | ServerEvent::FolderCreated { .. }
                | ServerEvent::FolderUpdated { .. } | ServerEvent::FolderDeleted { .. }
                | ServerEvent::FileRestored { .. } => {
                    let next = *refresh.peek() + 1;
                    refresh.set(next);
                }
                _ => {}
            }
        }
    });

    // Sync parent_id prop into a signal so use_effect reacts to navigation changes.
    let mut parent_sig = use_signal(|| parent_id.clone());
    if *parent_sig.peek() != parent_id {
        parent_sig.set(parent_id.clone());
        selection.write().clear();
        loading.set(true); // show spinner on folder navigation
    }

    let mut highlight = use_context::<Signal<HighlightTarget>>();

    use_effect(move || {
        let _ = refresh();
        let parent = parent_sig();
        // Only show the loading spinner for initial load and folder navigation.
        // SSE-triggered refreshes (refresh signal incremented while loading=false)
        // run silently so that open modals are not unmounted mid-operation.
        let show_spinner = *loading.peek();
        spawn(async move {
            error.set(None);
            match use_files::list_contents(parent.as_deref()).await {
                Ok((f, d)) => {
                    files.set(f);
                    folders.set(d);
                }
                Err(e) => error.set(Some(e)),
            }
            if show_spinner {
                loading.set(false);
            }
        });
    });

    // Highlight a file/folder after navigation (e.g. trash restore or search result click).
    // Runs whenever the file list or highlight target changes.
    use_effect(move || {
        let _ = files();
        let _ = folders();
        let target = highlight();
        if let Some(ref fid) = target.file_id {
            let dom_id = format!("file-{}", fid);
            if let Some(el) = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.get_element_by_id(&dom_id))
            {
                // Scroll into view
                el.scroll_into_view();
                // Apply highlight animation
                let _ = el.class_list().add_1("uc-highlight");
                // Clear the target so it does not re-trigger
                highlight.set(HighlightTarget::default());
                // Remove the class after the animation completes (~1.5s = 3 × 0.5s)
                let el_clone = el.clone();
                spawn(async move {
                    gloo_timers::future::TimeoutFuture::new(1_600).await;
                    let _ = el_clone.class_list().remove_1("uc-highlight");
                });
            }
        }
    });

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
                h3 { class: "text-lg font-semibold", "Error loading files" }
                p { class: "text-base-content/60", "{err}" }
            }
        };
    }

    let is_empty = files().is_empty() && folders().is_empty();
    let sel_count = selection().total();

    rsx! {
        // ── Selection toolbar (shown when items are selected) ──────────────────
        if sel_count > 0 {
            div { class: "flex items-center gap-2 mb-3 px-3 py-2 bg-primary/10 rounded-box border border-primary/20",
                span { class: "text-sm font-medium flex-1",
                    "{sel_count} selected"
                }
                button {
                    class: "btn btn-sm btn-ghost gap-1",
                    title: "Move selected",
                    onclick: move |_| {
                        let items = selection().items_with_names(&files(), &folders());
                        move_target.set(Some((items, false)));
                    },
                    span { "📂" }
                    span { class: "hidden sm:inline", "Move" }
                }
                button {
                    class: "btn btn-sm btn-ghost gap-1",
                    title: "Copy selected",
                    onclick: move |_| {
                        let items = selection().items_with_names(&files(), &folders());
                        move_target.set(Some((items, true)));
                    },
                    span { "📋" }
                    span { class: "hidden sm:inline", "Copy" }
                }
                button {
                    class: "btn btn-sm btn-ghost gap-1 text-error",
                    title: "Delete selected",
                    onclick: move |_| bulk_delete_confirm.set(true),
                    span { "🗑️" }
                    span { class: "hidden sm:inline", "Delete" }
                }
                button {
                    class: "btn btn-sm btn-ghost btn-circle",
                    title: "Deselect all",
                    onclick: move |_| selection.write().clear(),
                    "✕"
                }
            }
        }

        // ── Toolbar: breadcrumb + view toggle + new folder ─────────────────────
        div { class: "flex items-center gap-2 mb-4 min-w-0",
            div { class: "flex-1 min-w-0",
                if let Some(ref pid) = parent_id {
                    Breadcrumb { folder_id: pid.clone() }
                } else {
                    div { class: "text-sm breadcrumbs px-1",
                        ul { li { "Files" } }
                    }
                }
            }
            div { class: "join shrink-0",
                button {
                    class: if view_mode() == ViewMode::Grid { "join-item btn btn-sm btn-primary" } else { "join-item btn btn-sm btn-ghost" },
                    title: "Grid view",
                    onclick: move |_| {
                        view_mode.set(ViewMode::Grid);
                        let _ = LocalStorage::set("uncloud_view_mode", "grid");
                    },
                    svg {
                        xmlns: "http://www.w3.org/2000/svg",
                        width: "16", height: "16",
                        fill: "currentColor",
                        view_box: "0 0 16 16",
                        path { d: "M1 2.5A1.5 1.5 0 0 1 2.5 1h3A1.5 1.5 0 0 1 7 2.5v3A1.5 1.5 0 0 1 5.5 7h-3A1.5 1.5 0 0 1 1 5.5zm8 0A1.5 1.5 0 0 1 10.5 1h3A1.5 1.5 0 0 1 15 2.5v3A1.5 1.5 0 0 1 13.5 7h-3A1.5 1.5 0 0 1 9 5.5zm-8 8A1.5 1.5 0 0 1 2.5 9h3A1.5 1.5 0 0 1 7 10.5v3A1.5 1.5 0 0 1 5.5 15h-3A1.5 1.5 0 0 1 1 13.5zm8 0A1.5 1.5 0 0 1 10.5 9h3A1.5 1.5 0 0 1 15 10.5v3A1.5 1.5 0 0 1 13.5 15h-3A1.5 1.5 0 0 1 9 13.5z" }
                    }
                }
                button {
                    class: if view_mode() == ViewMode::List { "join-item btn btn-sm btn-primary" } else { "join-item btn btn-sm btn-ghost" },
                    title: "List view",
                    onclick: move |_| {
                        view_mode.set(ViewMode::List);
                        let _ = LocalStorage::set("uncloud_view_mode", "list");
                    },
                    svg {
                        xmlns: "http://www.w3.org/2000/svg",
                        width: "16", height: "16",
                        fill: "currentColor",
                        view_box: "0 0 16 16",
                        path {
                            fill_rule: "evenodd",
                            d: "M2.5 12a.5.5 0 0 1 .5-.5h10a.5.5 0 0 1 0 1H3a.5.5 0 0 1-.5-.5m0-4a.5.5 0 0 1 .5-.5h10a.5.5 0 0 1 0 1H3a.5.5 0 0 1-.5-.5m0-4a.5.5 0 0 1 .5-.5h10a.5.5 0 0 1 0 1H3a.5.5 0 0 1-.5-.5"
                        }
                    }
                }
            }
            button {
                class: "btn btn-sm btn-ghost gap-1 shrink-0",
                onclick: move |_| {
                    if let Some(input) = web_sys::window()
                        .and_then(|w| w.document())
                        .and_then(|d| d.get_element_by_id(FILE_INPUT_ID))
                        .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
                    {
                        input.click();
                    }
                },
                span { "📤" }
                span { class: "hidden sm:inline", "Upload" }
            }
            button {
                class: "btn btn-sm btn-ghost gap-1 shrink-0",
                onclick: move |_| show_new_folder.set(true),
                span { "📁" }
                span { class: "hidden sm:inline", "New Folder" }
            }
            button {
                class: "btn btn-sm btn-ghost gap-1 shrink-0",
                onclick: move |_| show_new_file.set(true),
                span { "📝" }
                span { class: "hidden sm:inline", "New File" }
            }
        }

        UploadZone {
            parent_id: parent_id.clone(),
            on_complete: move |_| refresh.set(refresh() + 1),
            show_zone: is_empty,
        }

        if !is_empty && view_mode() == ViewMode::Grid {
            div { class: "grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-6 gap-3 mt-6",
                for folder in folders() {
                    {
                        let (id_t, id_r, name_r, id_m, name_m, id_c, name_c, id_d, name_d, id_fs, name_fs) = (
                            folder.id.clone(), folder.id.clone(), folder.name.clone(),
                            folder.id.clone(), folder.name.clone(),
                            folder.id.clone(), folder.name.clone(),
                            folder.id.clone(), folder.name.clone(),
                            folder.id.clone(), folder.name.clone(),
                        );
                        let gi = folder.gallery_include;
                        let mi = folder.music_include;
                        rsx! {
                            FileItem {
                                key: "{folder.id}",
                                id: folder.id.clone(),
                                name: folder.name.clone(),
                                is_folder: true,
                                size: None,
                                mime_type: None,
                                view_mode: ViewMode::Grid,
                                selected: selection().contains_folder(&folder.id),
                                thumbnail_ver: 0,
                                on_delete_request: move |_| delete_target.set(Some((id_d.clone(), true, name_d.clone()))),
                                on_toggle_select: move |_| selection.write().toggle_folder(id_t.clone()),
                                on_rename_request: move |_| rename_target.set(Some((id_r.clone(), true, name_r.clone()))),
                                on_move_request: move |_| move_target.set(Some((vec![(id_m.clone(), true, name_m.clone())], false))),
                                on_copy_request: move |_| move_target.set(Some((vec![(id_c.clone(), true, name_c.clone())], true))),
                                on_open_request: move |_| {},
                                on_edit_request: move |_| {},
                                on_version_history_request: move |_| {},
                                on_folder_settings_request: move |_| {
                                    folder_settings_target.set(Some((id_fs.clone(), name_fs.clone(), gi, mi)));
                                },
                            }
                        }
                    }
                }
                for file in files() {
                    {
                        let (id_t, id_r, name_r, id_m, name_m, id_c, name_c, id_d, name_d, id_v, name_v) = (
                            file.id.clone(), file.id.clone(), file.name.clone(),
                            file.id.clone(), file.name.clone(),
                            file.id.clone(), file.name.clone(),
                            file.id.clone(), file.name.clone(),
                            file.id.clone(), file.name.clone(),
                        );
                        let file_for_open = file.clone();
                        let file_for_edit = file.clone();
                        rsx! {
                            FileItem {
                                key: "{file.id}",
                                id: file.id.clone(),
                                name: file.name.clone(),
                                is_folder: false,
                                size: Some(file.size_bytes),
                                mime_type: Some(file.mime_type.clone()),
                                view_mode: ViewMode::Grid,
                                selected: selection().contains_file(&file.id),
                                thumbnail_ver: *thumb_vers.read().get(&file.id).unwrap_or(&0),
                                on_delete_request: move |_| delete_target.set(Some((id_d.clone(), false, name_d.clone()))),
                                on_toggle_select: move |_| selection.write().toggle_file(id_t.clone()),
                                on_rename_request: move |_| rename_target.set(Some((id_r.clone(), false, name_r.clone()))),
                                on_move_request: move |_| move_target.set(Some((vec![(id_m.clone(), false, name_m.clone())], false))),
                                on_copy_request: move |_| move_target.set(Some((vec![(id_c.clone(), false, name_c.clone())], true))),
                                on_open_request: {
                                    let f = file_for_open.clone();
                                    move |_| {
                                        let f = f.clone();
                                        if f.name.ends_with(".kdbx") {
                                            vault_open_target.set(VaultOpenTarget {
                                                file_id: Some(f.id.clone()),
                                                file_name: Some(f.name.clone()),
                                            });
                                            let _ = nav.push(Route::Passwords {});
                                            return;
                                        }
                                        let mime = f.mime_type.as_str();
                                        if mime.starts_with("audio/") {
                                            let audio: AudioMeta = f.metadata.get("audio")
                                                .and_then(|v| serde_json::from_value(v.clone()).ok())
                                                .unwrap_or_default();
                                            let track = TrackResponse { file: f, audio };
                                            use_player::play_queue(player, vec![track], 0);
                                        } else if mime.starts_with("image/") {
                                            let images: Vec<FileResponse> = files().into_iter()
                                                .filter(|fi| fi.mime_type.starts_with("image/"))
                                                .collect();
                                            let idx = images.iter().position(|fi| fi.id == f.id).unwrap_or(0);
                                            viewer_target.set(Some(ViewerTarget::Image { files: images, index: idx }));
                                        } else if mime == "application/pdf" {
                                            let url = crate::hooks::api::authenticated_media_url(&format!("/files/{}/download", f.id));
                                            let _ = web_sys::window().and_then(|w| w.open_with_url_and_target(&url, "_blank").ok());
                                        } else if mime.starts_with("text/") || mime == "application/json" || mime == "application/xml" {
                                            viewer_target.set(Some(ViewerTarget::Text(f)));
                                        }
                                    }
                                },
                                on_edit_request: {
                                    let f = file_for_edit.clone();
                                    move |_| {
                                        viewer_target.set(Some(ViewerTarget::TextEdit(f.clone())));
                                    }
                                },
                                on_version_history_request: move |_| version_history_target.set(Some((id_v.clone(), name_v.clone()))),
                                on_folder_settings_request: move |_| {},
                            }
                        }
                    }
                }
            }
        } else if !is_empty {
            div { class: "overflow-hidden rounded-box border border-base-300 mt-6",
                table { class: "table table-sm w-full",
                    thead {
                        tr {
                            th { class: "w-8" } // checkbox
                            th { class: "w-8" } // icon
                            th { "Name" }
                            th { class: "hidden sm:table-cell", "Type" }
                            th { class: "hidden sm:table-cell text-right", "Size" }
                            th { class: "w-8" } // menu
                        }
                    }
                    tbody {
                        for folder in folders() {
                            {
                                let (id_t, id_r, name_r, id_m, name_m, id_c, name_c, id_d, name_d, id_fs, name_fs) = (
                                    folder.id.clone(), folder.id.clone(), folder.name.clone(),
                                    folder.id.clone(), folder.name.clone(),
                                    folder.id.clone(), folder.name.clone(),
                                    folder.id.clone(), folder.name.clone(),
                                    folder.id.clone(), folder.name.clone(),
                                );
                                let gi = folder.gallery_include;
                                let mi = folder.music_include;
                                rsx! {
                                    FileItem {
                                        key: "{folder.id}",
                                        id: folder.id.clone(),
                                        name: folder.name.clone(),
                                        is_folder: true,
                                        size: None,
                                        mime_type: None,
                                        view_mode: ViewMode::List,
                                        selected: selection().contains_folder(&folder.id),
                                        thumbnail_ver: 0,
                                        on_delete_request: move |_| delete_target.set(Some((id_d.clone(), true, name_d.clone()))),
                                        on_toggle_select: move |_| selection.write().toggle_folder(id_t.clone()),
                                        on_rename_request: move |_| rename_target.set(Some((id_r.clone(), true, name_r.clone()))),
                                        on_move_request: move |_| move_target.set(Some((vec![(id_m.clone(), true, name_m.clone())], false))),
                                        on_copy_request: move |_| move_target.set(Some((vec![(id_c.clone(), true, name_c.clone())], true))),
                                        on_open_request: move |_| {},
                                        on_edit_request: move |_| {},
                                        on_version_history_request: move |_| {},
                                        on_folder_settings_request: move |_| {
                                            folder_settings_target.set(Some((id_fs.clone(), name_fs.clone(), gi, mi)));
                                        },
                                    }
                                }
                            }
                        }
                        for file in files() {
                            {
                                let (id_t, id_r, name_r, id_m, name_m, id_c, name_c, id_d, name_d, id_v, name_v) = (
                                    file.id.clone(), file.id.clone(), file.name.clone(),
                                    file.id.clone(), file.name.clone(),
                                    file.id.clone(), file.name.clone(),
                                    file.id.clone(), file.name.clone(),
                                    file.id.clone(), file.name.clone(),
                                );
                                let file_for_open = file.clone();
                                let file_for_edit = file.clone();
                                rsx! {
                                    FileItem {
                                        key: "{file.id}",
                                        id: file.id.clone(),
                                        name: file.name.clone(),
                                        is_folder: false,
                                        size: Some(file.size_bytes),
                                        mime_type: Some(file.mime_type.clone()),
                                        view_mode: ViewMode::List,
                                        selected: selection().contains_file(&file.id),
                                        thumbnail_ver: *thumb_vers.read().get(&file.id).unwrap_or(&0),
                                        on_delete_request: move |_| delete_target.set(Some((id_d.clone(), false, name_d.clone()))),
                                        on_toggle_select: move |_| selection.write().toggle_file(id_t.clone()),
                                        on_rename_request: move |_| rename_target.set(Some((id_r.clone(), false, name_r.clone()))),
                                        on_move_request: move |_| move_target.set(Some((vec![(id_m.clone(), false, name_m.clone())], false))),
                                        on_copy_request: move |_| move_target.set(Some((vec![(id_c.clone(), false, name_c.clone())], true))),
                                        on_open_request: {
                                            let f = file_for_open.clone();
                                            move |_| {
                                                let f = f.clone();
                                                if f.name.ends_with(".kdbx") {
                                                    vault_open_target.set(VaultOpenTarget {
                                                        file_id: Some(f.id.clone()),
                                                        file_name: Some(f.name.clone()),
                                                    });
                                                    let _ = nav.push(Route::Passwords {});
                                                    return;
                                                }
                                                let mime = f.mime_type.as_str();
                                                if mime.starts_with("audio/") {
                                                    let audio: AudioMeta = f.metadata.get("audio")
                                                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                                                        .unwrap_or_default();
                                                    let track = TrackResponse { file: f, audio };
                                                    use_player::play_queue(player, vec![track], 0);
                                                } else if mime.starts_with("image/") {
                                                    let images: Vec<FileResponse> = files().into_iter()
                                                        .filter(|fi| fi.mime_type.starts_with("image/"))
                                                        .collect();
                                                    let idx = images.iter().position(|fi| fi.id == f.id).unwrap_or(0);
                                                    viewer_target.set(Some(ViewerTarget::Image { files: images, index: idx }));
                                                } else if mime == "application/pdf" {
                                                    let url = crate::hooks::api::authenticated_media_url(&format!("/files/{}/download", f.id));
                                                    let _ = web_sys::window().and_then(|w| w.open_with_url_and_target(&url, "_blank").ok());
                                                } else if mime.starts_with("text/") || mime == "application/json" || mime == "application/xml" {
                                                    viewer_target.set(Some(ViewerTarget::Text(f)));
                                                }
                                            }
                                        },
                                        on_edit_request: {
                                            let f = file_for_edit.clone();
                                            move |_| {
                                                viewer_target.set(Some(ViewerTarget::TextEdit(f.clone())));
                                            }
                                        },
                                        on_version_history_request: move |_| version_history_target.set(Some((id_v.clone(), name_v.clone()))),
                                        on_folder_settings_request: move |_| {},
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if show_new_folder() {
            NewFolderModal {
                parent_id: parent_id.clone(),
                on_cancel: move |_| show_new_folder.set(false),
                on_created: move |_| {
                    show_new_folder.set(false);
                    refresh.set(refresh() + 1);
                },
            }
        }

        if show_new_file() {
            NewFileModal {
                parent_id: parent_id.clone(),
                on_cancel: move |_| show_new_file.set(false),
                on_created: move |file: FileResponse| {
                    show_new_file.set(false);
                    refresh.set(refresh() + 1);
                    viewer_target.set(Some(ViewerTarget::TextEdit(file)));
                },
            }
        }

        if let Some((id, is_folder, name)) = delete_target() {
            DeleteConfirmModal {
                id,
                is_folder,
                name,
                on_cancel: move |_| delete_target.set(None),
                on_deleted: move |_| {
                    delete_target.set(None);
                    refresh.set(refresh() + 1);
                },
            }
        }

        if let Some((id, is_folder, current_name)) = rename_target() {
            RenameModal {
                id,
                is_folder,
                current_name,
                on_cancel: move |_| rename_target.set(None),
                on_renamed: move |_| {
                    rename_target.set(None);
                    refresh.set(refresh() + 1);
                },
            }
        }

        if let Some((items, is_copy)) = move_target() {
            MoveDialog {
                items,
                is_copy,
                on_cancel: move |_| move_target.set(None),
                on_success: move |_| {
                    move_target.set(None);
                    selection.write().clear();
                    refresh.set(refresh() + 1);
                },
            }
        }

        if let Some((folder_id, folder_name, gallery_include, music_include)) = folder_settings_target() {
            FolderSettingsModal {
                folder_id,
                folder_name,
                gallery_include,
                music_include,
                on_close: move |_| folder_settings_target.set(None),
                on_saved: move |_| {
                    folder_settings_target.set(None);
                    refresh.set(refresh() + 1);
                },
            }
        }

        if let Some((file_id, file_name)) = version_history_target() {
            crate::components::version_history::VersionHistoryModal {
                file_id,
                file_name,
                on_close: move |_| version_history_target.set(None),
                on_restored: move |_| {
                    version_history_target.set(None);
                    refresh.set(refresh() + 1);
                },
            }
        }

        if bulk_delete_confirm() {
            {
                let has_folders = !selection().folders.is_empty();
                let count = sel_count;
                let item_label = if count == 1 { "item" } else { "items" };
                rsx! {
                    div { class: "modal modal-open",
                        div { class: "modal-box max-w-sm",
                            h3 { class: "font-bold text-lg mb-2",
                                "Delete {count} {item_label}?"
                            }
                            if has_folders {
                                p { class: "text-sm text-base-content/70 mb-4",
                                    "Folders and all their contents will be permanently deleted."
                                }
                            }
                            div { class: "modal-action",
                                button {
                                    class: "btn btn-ghost",
                                    onclick: move |_| bulk_delete_confirm.set(false),
                                    "Cancel"
                                }
                                button {
                                    class: "btn btn-error",
                                    onclick: move |_| {
                                        bulk_delete_confirm.set(false);
                                        let sel = selection();
                                        let file_ids: Vec<String> = sel.files.iter().cloned().collect();
                                        let folder_ids: Vec<String> = sel.folders.iter().cloned().collect();
                                        selection.write().clear();
                                        spawn(async move {
                                            for id in &file_ids {
                                                let _ = use_files::delete_file(id).await;
                                            }
                                            for id in &folder_ids {
                                                let _ = use_files::delete_folder(id).await;
                                            }
                                            refresh.set(refresh() + 1);
                                        });
                                    },
                                    "Delete"
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Some(target) = viewer_target() {
            match target {
                ViewerTarget::Image { files: imgs, index } => rsx! {
                    crate::components::lightbox::Lightbox {
                        images: imgs,
                        initial_index: index,
                        on_close: move |_| viewer_target.set(None),
                    }
                },
                ViewerTarget::Text(file) => rsx! {
                    crate::components::file_viewer::TextViewer {
                        file,
                        start_editing: false,
                        on_close: move |_| viewer_target.set(None),
                    }
                },
                ViewerTarget::TextEdit(file) => rsx! {
                    crate::components::file_viewer::TextViewer {
                        file,
                        start_editing: true,
                        on_close: move |_| viewer_target.set(None),
                    }
                },
            }
        }
    }
}

// ── Delete Confirm Modal ───────────────────────────────────────────────────────

#[component]
fn DeleteConfirmModal(
    id: String,
    is_folder: bool,
    name: String,
    on_cancel: EventHandler<()>,
    on_deleted: EventHandler<()>,
) -> Element {
    let mut deleting = use_signal(|| false);

    rsx! {
        div { class: "modal modal-open",
            div { class: "modal-box max-w-sm",
                h3 { class: "font-bold text-lg mb-2", "Delete \"{name}\"?" }
                if is_folder {
                    p { class: "text-sm text-base-content/70 mb-4",
                        "This folder and all its contents will be permanently deleted."
                    }
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
                            let id = id.clone();
                            spawn(async move {
                                if is_folder {
                                    let _ = use_files::delete_folder(&id).await;
                                } else {
                                    let _ = use_files::delete_file(&id).await;
                                }
                                on_deleted.call(());
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

// ── Breadcrumb ────────────────────────────────────────────────────────────────

#[component]
fn Breadcrumb(folder_id: String) -> Element {
    let mut chain = use_signal(Vec::<FolderResponse>::new);
    let nav = use_navigator();

    use_effect(move || {
        let id = folder_id.clone();
        spawn(async move {
            if let Ok(c) = use_files::get_breadcrumb(&id).await {
                chain.set(c);
            }
        });
    });

    rsx! {
        div { class: "text-sm breadcrumbs px-1",
            ul {
                li {
                    a {
                        class: "cursor-pointer",
                        onclick: move |_| { let _ = nav.push(Route::Home {}); },
                        "Files"
                    }
                }
                for folder in chain() {
                    li {
                        a {
                            class: "cursor-pointer",
                            onclick: {
                                let id = folder.id.clone();
                                move |_| { let _ = nav.push(Route::Folder { id: id.clone() }); }
                            },
                            "{folder.name}"
                        }
                    }
                }
            }
        }
    }
}

// ── New Folder Modal ──────────────────────────────────────────────────────────

#[component]
fn NewFolderModal(
    parent_id: Option<String>,
    on_cancel: EventHandler<()>,
    on_created: EventHandler<()>,
) -> Element {
    let mut name = use_signal(String::new);
    let mut creating = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    let on_submit = move |e: Event<FormData>| {
        e.prevent_default();
        let n = name().trim().to_string();
        if n.is_empty() {
            return;
        }
        let parent = parent_id.clone();
        creating.set(true);
        error.set(None);
        spawn(async move {
            match use_files::create_folder(&n, parent.as_deref()).await {
                Ok(_) => on_created.call(()),
                Err(e) => {
                    error.set(Some(e));
                    creating.set(false);
                }
            }
        });
    };

    rsx! {
        div { class: "modal modal-open",
            div { class: "modal-box max-w-sm",
                h3 { class: "font-bold text-lg mb-4", "New Folder" }

                form { onsubmit: on_submit,
                    div { class: "form-control",
                        input {
                            class: "input input-bordered w-full",
                            placeholder: "Folder name",
                            autofocus: true,
                            value: "{name}",
                            oninput: move |e| name.set(e.value()),
                        }
                    }

                    if let Some(err) = error() {
                        div { class: "alert alert-error mt-3 py-2 text-sm", "{err}" }
                    }

                    div { class: "modal-action",
                        button {
                            class: "btn btn-ghost",
                            r#type: "button",
                            onclick: move |_| on_cancel.call(()),
                            "Cancel"
                        }
                        button {
                            class: "btn btn-primary",
                            r#type: "submit",
                            disabled: creating(),
                            if creating() {
                                span { class: "loading loading-spinner loading-sm" }
                            }
                            "Create"
                        }
                    }
                }
            }
        }
    }
}

// ── New File Modal ───────────────────────────────────────────────────────────

#[component]
fn NewFileModal(
    parent_id: Option<String>,
    on_cancel: EventHandler<()>,
    on_created: EventHandler<FileResponse>,
) -> Element {
    let mut name = use_signal(|| "untitled.md".to_string());
    let mut creating = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    let on_submit = move |e: Event<FormData>| {
        e.prevent_default();
        let n = name().trim().to_string();
        if n.is_empty() {
            return;
        }
        let parent = parent_id.clone();
        creating.set(true);
        error.set(None);
        spawn(async move {
            let result = async {
                // Create a Blob with empty content
                let blob_parts = js_sys::Array::new();
                blob_parts.push(&wasm_bindgen::JsValue::from_str(""));
                let opts = web_sys::BlobPropertyBag::new();
                opts.set_type("text/markdown");
                let blob = web_sys::Blob::new_with_str_sequence_and_options(&blob_parts, &opts)
                    .map_err(|_| "Failed to create Blob".to_string())?;

                let form = web_sys::FormData::new()
                    .map_err(|_| "Failed to create FormData".to_string())?;
                form.append_with_blob_and_filename("file", &blob, &n)
                    .map_err(|_| "Failed to append file".to_string())?;
                if let Some(pid) = &parent {
                    form.append_with_str("parent_id", pid)
                        .map_err(|_| "Failed to append parent_id".to_string())?;
                }

                let resp = crate::hooks::api::post("/uploads/simple")
                    .body(wasm_bindgen::JsValue::from(form))
                    .map_err(|e| format!("{:?}", e))?
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;

                if resp.ok() {
                    resp.json::<FileResponse>().await.map_err(|e| e.to_string())
                } else {
                    Err(format!("Failed to create file (HTTP {})", resp.status()))
                }
            }
            .await;

            match result {
                Ok(file) => on_created.call(file),
                Err(e) => {
                    error.set(Some(e));
                    creating.set(false);
                }
            }
        });
    };

    rsx! {
        div { class: "modal modal-open",
            div { class: "modal-box max-w-sm",
                h3 { class: "font-bold text-lg mb-4", "New File" }

                form { onsubmit: on_submit,
                    div { class: "form-control",
                        input {
                            class: "input input-bordered w-full",
                            placeholder: "File name",
                            autofocus: true,
                            value: "{name}",
                            oninput: move |e| name.set(e.value()),
                        }
                    }

                    if let Some(err) = error() {
                        div { class: "alert alert-error mt-3 py-2 text-sm", "{err}" }
                    }

                    div { class: "modal-action",
                        button {
                            class: "btn btn-ghost",
                            r#type: "button",
                            onclick: move |_| on_cancel.call(()),
                            "Cancel"
                        }
                        button {
                            class: "btn btn-primary",
                            r#type: "submit",
                            disabled: creating(),
                            if creating() {
                                span { class: "loading loading-spinner loading-sm" }
                            }
                            "Create"
                        }
                    }
                }
            }
        }
    }
}

// ── Rename Modal ──────────────────────────────────────────────────────────────

#[component]
fn RenameModal(
    id: String,
    is_folder: bool,
    current_name: String,
    on_cancel: EventHandler<()>,
    on_renamed: EventHandler<()>,
) -> Element {
    let mut name = use_signal(|| current_name.clone());
    let mut saving = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    let on_submit = move |e: Event<FormData>| {
        e.prevent_default();
        let n = name().trim().to_string();
        if n.is_empty() || n == current_name {
            on_cancel.call(());
            return;
        }
        let id = id.clone();
        saving.set(true);
        error.set(None);
        spawn(async move {
            let result = if is_folder {
                use_files::rename_folder(&id, &n).await.map(|_| ())
            } else {
                use_files::rename_file(&id, &n).await.map(|_| ())
            };
            match result {
                Ok(_) => on_renamed.call(()),
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
                h3 { class: "font-bold text-lg mb-4", "Rename" }

                form { onsubmit: on_submit,
                    div { class: "form-control",
                        input {
                            class: "input input-bordered w-full",
                            autofocus: true,
                            value: "{name}",
                            oninput: move |e| name.set(e.value()),
                        }
                    }

                    if let Some(err) = error() {
                        div { class: "alert alert-error mt-3 py-2 text-sm", "{err}" }
                    }

                    div { class: "modal-action",
                        button {
                            class: "btn btn-ghost",
                            r#type: "button",
                            onclick: move |_| on_cancel.call(()),
                            "Cancel"
                        }
                        button {
                            class: "btn btn-primary",
                            r#type: "submit",
                            disabled: saving(),
                            if saving() {
                                span { class: "loading loading-spinner loading-sm" }
                            }
                            "Rename"
                        }
                    }
                }
            }
        }
    }
}

// ── Move / Copy Dialog ────────────────────────────────────────────────────────

/// Generate a conflict-resolution name: "foo.txt" → "foo (1).txt", "foo (1).txt" → "foo (2).txt".
fn suggest_name(name: &str) -> String {
    let (base, ext) = match name.rfind('.') {
        Some(dot) => (&name[..dot], &name[dot..]),
        None => (name, ""),
    };
    if let Some(open) = base.rfind(" (") {
        let inner = &base[open + 2..];
        if inner.ends_with(')') {
            if let Ok(n) = inner[..inner.len() - 1].parse::<u32>() {
                return format!("{} ({}){}", &base[..open], n + 1, ext);
            }
        }
    }
    format!("{} (1){}", base, ext)
}

#[component]
fn MoveDialog(
    /// Items being moved/copied: (id, is_folder, name).
    items: Vec<(String, bool, String)>,
    is_copy: bool,
    on_cancel: EventHandler<()>,
    /// Called after all items have been successfully moved/copied.
    on_success: EventHandler<()>,
) -> Element {
    // IDs of folders being moved — excluded from picker (can't move into self).
    let moved_folder_ids: HashSet<String> = items.iter()
        .filter(|(_, is_f, _)| *is_f)
        .map(|(id, _, _)| id.clone())
        .collect();

    let mut picker_parent: Signal<Option<String>> = use_signal(|| None);
    let mut picker_folders: Signal<Vec<FolderResponse>> = use_signal(Vec::new);
    let mut picker_breadcrumb: Signal<Vec<FolderResponse>> = use_signal(Vec::new);
    let mut picker_loading = use_signal(|| false);
    let mut working = use_signal(|| false);
    let mut op_error: Signal<Option<String>> = use_signal(|| None);

    // Remaining items to process: (id, is_folder, name_to_use)
    let mut queue: Signal<Vec<(String, bool, String)>> = use_signal(|| items.clone());
    // Set when a 409 conflict is encountered: (id, is_folder) of the blocked item
    let mut conflict: Signal<Option<(String, bool)>> = use_signal(|| None);
    let mut conflict_new_name: Signal<String> = use_signal(String::new);

    use_effect(move || {
        let parent = picker_parent();
        spawn(async move {
            picker_loading.set(true);
            if let Ok(flds) = use_files::list_folders(parent.as_deref()).await {
                picker_folders.set(flds);
            }
            match &parent {
                Some(pid) => {
                    if let Ok(crumbs) = use_files::get_breadcrumb(pid).await {
                        picker_breadcrumb.set(crumbs);
                    }
                }
                None => picker_breadcrumb.set(Vec::new()),
            }
            picker_loading.set(false);
        });
    });

    let title = if is_copy { "Copy to…" } else { "Move to…" };
    let confirm_label = if is_copy { "Copy Here" } else { "Move Here" };

    let visible_folders: Vec<FolderResponse> = picker_folders()
        .into_iter()
        .filter(|f| !moved_folder_ids.contains(&f.id))
        .collect();

    let in_conflict = conflict().is_some();

    rsx! {
        div { class: "modal modal-open",
            div { class: "modal-box max-w-md",
                h3 { class: "font-bold text-lg mb-3", "{title}" }

                if !in_conflict {
                    // Destination picker
                    div { class: "text-sm breadcrumbs px-0 mb-1",
                        ul {
                            li {
                                a {
                                    class: "cursor-pointer",
                                    onclick: move |_| picker_parent.set(None),
                                    "Files"
                                }
                            }
                            for folder in picker_breadcrumb() {
                                li {
                                    a {
                                        class: "cursor-pointer",
                                        onclick: {
                                            let id = folder.id.clone();
                                            move |_| picker_parent.set(Some(id.clone()))
                                        },
                                        "{folder.name}"
                                    }
                                }
                            }
                        }
                    }

                    div { class: "min-h-28 border border-base-300 rounded-box overflow-y-auto max-h-60",
                        if picker_loading() {
                            div { class: "flex justify-center items-center h-28",
                                span { class: "loading loading-spinner loading-md" }
                            }
                        } else if visible_folders.is_empty() {
                            div { class: "flex justify-center items-center h-28 text-base-content/40 text-sm",
                                "No subfolders here"
                            }
                        } else {
                            ul { class: "menu menu-sm p-1",
                                for folder in visible_folders {
                                    li {
                                        a {
                                            onclick: {
                                                let id = folder.id.clone();
                                                move |_| picker_parent.set(Some(id.clone()))
                                            },
                                            span { "📁" }
                                            span { "{folder.name}" }
                                            span { class: "ml-auto text-base-content/40", "›" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else {
                    // Conflict resolution: ask for a new name
                    div { class: "alert alert-warning mb-3",
                        div {
                            p { class: "font-semibold text-sm", "Name conflict" }
                            p { class: "text-sm mt-1",
                                "An item with this name already exists at the destination. Enter a new name to continue:"
                            }
                        }
                    }
                    div { class: "form-control",
                        input {
                            class: "input input-bordered w-full",
                            autofocus: true,
                            value: "{conflict_new_name}",
                            oninput: move |e| conflict_new_name.set(e.value()),
                        }
                    }
                }

                if let Some(err) = op_error() {
                    div { class: "alert alert-error mt-3 py-2 text-sm", "{err}" }
                }

                div { class: "modal-action mt-4",
                    button {
                        class: "btn btn-ghost",
                        r#type: "button",
                        disabled: working(),
                        onclick: move |_| on_cancel.call(()),
                        "Cancel"
                    }
                    button {
                        class: "btn btn-primary",
                        disabled: working() || (in_conflict && conflict_new_name().trim().is_empty()),
                        onclick: move |_| {
                            // If we're resolving a conflict, update the queued name first.
                            if conflict().is_some() {
                                let new_name = conflict_new_name().trim().to_string();
                                if new_name.is_empty() { return; }
                                if let Some(first) = queue.write().first_mut() {
                                    first.2 = new_name;
                                }
                                conflict.set(None);
                            }
                            op_error.set(None);
                            let dest = picker_parent();
                            working.set(true);
                            spawn(async move {
                                loop {
                                    let q = queue().clone();
                                    if q.is_empty() {
                                        working.set(false);
                                        on_success.call(());
                                        return;
                                    }
                                    let (ref id, is_folder, ref name) = q[0];
                                    let result: Result<(), String> = if is_copy && is_folder {
                                        use_files::copy_folder(id, dest.as_deref(), Some(name))
                                            .await.map(|_| ())
                                    } else if is_copy {
                                        use_files::copy_file(id, dest.as_deref(), Some(name))
                                            .await.map(|_| ())
                                    } else if is_folder {
                                        use_files::move_folder(id, dest.as_deref(), Some(name))
                                            .await.map(|_| ())
                                    } else {
                                        use_files::move_file(id, dest.as_deref(), Some(name))
                                            .await.map(|_| ())
                                    };
                                    match result {
                                        Ok(()) => { queue.write().remove(0); }
                                        Err(e) if e == "CONFLICT" => {
                                            conflict_new_name.set(suggest_name(name));
                                            conflict.set(Some((id.clone(), is_folder)));
                                            working.set(false);
                                            return;
                                        }
                                        Err(e) => {
                                            op_error.set(Some(e));
                                            working.set(false);
                                            return;
                                        }
                                    }
                                }
                            });
                        },
                        if working() { span { class: "loading loading-spinner loading-sm" } }
                        if in_conflict { "Retry" } else { "{confirm_label}" }
                    }
                }
            }
        }
    }
}

// ── Folder Sync Settings Modal ─────────────────────────────────────────────────

fn strategy_label(s: SyncStrategy) -> &'static str {
    match s {
        SyncStrategy::Inherit => "Inherit (use parent's setting)",
        SyncStrategy::TwoWay => "Two-way",
        SyncStrategy::ClientToServer => "Client to server (upload + deletions)",
        SyncStrategy::ServerToClient => "Server to client (read-only local)",
        SyncStrategy::UploadOnly => "Upload only (phone gallery mode)",
        SyncStrategy::DoNotSync => "Do not sync",
    }
}

#[component]
fn FolderSettingsModal(
    folder_id: String,
    folder_name: String,
    gallery_include: GalleryInclude,
    music_include: MusicInclude,
    on_close: EventHandler<()>,
    on_saved: EventHandler<()>,
) -> Element {
    let mut tab: Signal<&'static str> = use_signal(|| "sync");
    let mut sync_selected: Signal<SyncStrategy> = use_signal(|| SyncStrategy::Inherit);
    let mut gallery_selected: Signal<GalleryInclude> = use_signal(|| gallery_include);
    let mut music_selected: Signal<MusicInclude> = use_signal(|| music_include);
    let mut effective_info: Signal<Option<EffectiveStrategyResponse>> = use_signal(|| None);
    let mut loading = use_signal(|| true);
    let mut saving = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let folder_id_for_save = folder_id.clone();

    use_effect(move || {
        let id = folder_id.clone();
        spawn(async move {
            match use_files::get_effective_strategy(&id).await {
                Ok(resp) => effective_info.set(Some(resp)),
                Err(_) => {}
            }
            loading.set(false);
        });
    });

    rsx! {
        div { class: "modal modal-open",
            div { class: "modal-box max-w-md",
                h3 { class: "font-bold text-lg mb-4", "Folder settings \u{2014} {folder_name}" }

                div { role: "tablist", class: "tabs tabs-bordered mb-4",
                    a { role: "tab", class: if tab() == "sync" { "tab tab-active" } else { "tab" },
                        onclick: move |_| tab.set("sync"), "Sync" }
                    a { role: "tab", class: if tab() == "gallery" { "tab tab-active" } else { "tab" },
                        onclick: move |_| tab.set("gallery"), "Gallery" }
                    a { role: "tab", class: if tab() == "music" { "tab tab-active" } else { "tab" },
                        onclick: move |_| tab.set("music"), "Music" }
                }

                // ── Sync tab ────────────────────────────────────────────
                if tab() == "sync" {
                    if loading() {
                        div { class: "flex justify-center py-8",
                            span { class: "loading loading-spinner loading-md" }
                        }
                    } else {
                        div { class: "form-control mb-2",
                            label { class: "label",
                                span { class: "label-text font-medium", "Sync strategy" }
                            }
                            select {
                                class: "select select-bordered w-full",
                                onchange: move |e| {
                                    let s = match e.value().as_str() {
                                        "two_way"          => SyncStrategy::TwoWay,
                                        "client_to_server" => SyncStrategy::ClientToServer,
                                        "server_to_client" => SyncStrategy::ServerToClient,
                                        "upload_only"      => SyncStrategy::UploadOnly,
                                        "do_not_sync"      => SyncStrategy::DoNotSync,
                                        _                  => SyncStrategy::Inherit,
                                    };
                                    sync_selected.set(s);
                                },
                                option { value: "inherit",          selected: sync_selected() == SyncStrategy::Inherit,          "Inherit (use parent's setting)" }
                                option { value: "two_way",          selected: sync_selected() == SyncStrategy::TwoWay,           "Two-way" }
                                option { value: "client_to_server", selected: sync_selected() == SyncStrategy::ClientToServer,   "Client to server" }
                                option { value: "server_to_client", selected: sync_selected() == SyncStrategy::ServerToClient,   "Server to client (read-only)" }
                                option { value: "upload_only",      selected: sync_selected() == SyncStrategy::UploadOnly,       "Upload only (phone gallery mode)" }
                                option { value: "do_not_sync",      selected: sync_selected() == SyncStrategy::DoNotSync,        "Do not sync" }
                            }
                        }

                        if let Some(info) = effective_info() {
                            p { class: "text-sm text-base-content/60 mb-2",
                                "Effective: "
                                span { class: "font-medium", "{strategy_label(info.strategy)}" }
                                if info.source_folder_id.is_some() {
                                    " (inherited from parent)"
                                }
                            }
                        }
                    }
                }

                // ── Gallery tab ─────────────────────────────────────────
                if tab() == "gallery" {
                    div { class: "form-control mb-3",
                        label { class: "label",
                            span { class: "label-text font-medium", "Include in Gallery" }
                        }
                        select {
                            class: "select select-bordered w-full",
                            onchange: move |e| {
                                gallery_selected.set(match e.value().as_str() {
                                    "include" => GalleryInclude::Include,
                                    "exclude" => GalleryInclude::Exclude,
                                    _ => GalleryInclude::Inherit,
                                });
                            },
                            option { value: "inherit", selected: gallery_selected() == GalleryInclude::Inherit, "Inherit (use parent folder's setting)" }
                            option { value: "include", selected: gallery_selected() == GalleryInclude::Include, "Include \u{2014} show images in Gallery" }
                            option { value: "exclude", selected: gallery_selected() == GalleryInclude::Exclude, "Exclude \u{2014} hide from Gallery" }
                        }
                    }
                    p { class: "text-sm text-base-content/60 mb-4",
                        "When set to Include, images in this folder and its subfolders appear in the Gallery timeline."
                    }
                }

                // ── Music tab ───────────────────────────────────────────
                if tab() == "music" {
                    div { class: "form-control mb-3",
                        label { class: "label",
                            span { class: "label-text font-medium", "Include in Music library" }
                        }
                        select {
                            class: "select select-bordered w-full",
                            onchange: move |e| {
                                music_selected.set(match e.value().as_str() {
                                    "include" => MusicInclude::Include,
                                    "exclude" => MusicInclude::Exclude,
                                    _ => MusicInclude::Inherit,
                                });
                            },
                            option { value: "inherit", selected: music_selected() == MusicInclude::Inherit, "Inherit (use parent folder's setting)" }
                            option { value: "include", selected: music_selected() == MusicInclude::Include, "Include \u{2014} add to Music library" }
                            option { value: "exclude", selected: music_selected() == MusicInclude::Exclude, "Exclude \u{2014} hide from Music library" }
                        }
                    }
                    p { class: "text-sm text-base-content/60 mb-4",
                        "When set to Include, audio files in this folder and its subfolders appear in the Music library."
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
                        onclick: move |_| on_close.call(()),
                        "Cancel"
                    }
                    button {
                        class: "btn btn-primary",
                        r#type: "button",
                        disabled: saving() || (tab() == "sync" && loading()),
                        onclick: move |_| {
                            let fid = folder_id_for_save.clone();
                            let current_tab = tab();
                            let sync_val = sync_selected();
                            let gallery_val = gallery_selected();
                            let music_val = music_selected();
                            spawn(async move {
                                saving.set(true);
                                error.set(None);
                                let result = match current_tab {
                                    "gallery" => use_files::update_folder_gallery_include(&fid, gallery_val).await,
                                    "music"   => use_files::update_folder_music_include(&fid, music_val).await,
                                    _         => use_files::update_folder_strategy(&fid, sync_val).await,
                                };
                                match result {
                                    Ok(_) => on_saved.call(()),
                                    Err(e) => {
                                        error.set(Some(e));
                                        saving.set(false);
                                    }
                                }
                            });
                        },
                        if saving() { span { class: "loading loading-spinner loading-sm" } }
                        "Save"
                    }
                }
            }
            div { class: "modal-backdrop", onclick: move |_| on_close.call(()) }
        }
    }
}
