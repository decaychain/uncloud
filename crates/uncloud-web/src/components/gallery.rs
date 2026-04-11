use std::collections::HashMap;
use dioxus::prelude::*;
use uncloud_common::{AlbumResponse, FileResponse, ServerEvent};
use crate::components::icons::{IconAlertTriangle, IconImage};
use crate::components::lightbox::Lightbox;
use crate::hooks::{api, use_files};
use crate::router::Route;

// ── Date grouping ────────────────────────────────────────────────────────────

/// Groups files by date label. Returns `(label, indices_into_files)`.
fn group_by_date(files: &[FileResponse]) -> Vec<(String, Vec<usize>)> {
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    let mut current_label = String::new();

    for (i, file) in files.iter().enumerate() {
        let label = format_date_label(&file.created_at);
        if label != current_label {
            current_label = label.clone();
            groups.push((label, vec![i]));
        } else if let Some(last) = groups.last_mut() {
            last.1.push(i);
        }
    }
    groups
}

/// Format an ISO 8601 date string as "14 March 2026".
fn format_date_label(iso: &str) -> String {
    if iso.len() < 10 {
        return iso.to_string();
    }
    let date_part = &iso[..10];
    let parts: Vec<&str> = date_part.split('-').collect();
    if parts.len() != 3 {
        return date_part.to_string();
    }
    let year = parts[0];
    let month = match parts[1] {
        "01" => "January", "02" => "February", "03" => "March",
        "04" => "April", "05" => "May", "06" => "June",
        "07" => "July", "08" => "August", "09" => "September",
        "10" => "October", "11" => "November", "12" => "December",
        _ => parts[1],
    };
    let day = parts[2].trim_start_matches('0');
    format!("{} {} {}", day, month, year)
}

// ── GalleryThumbnail ─────────────────────────────────────────────────────────

#[component]
fn GalleryThumbnail(id: String, name: String, thumb_ver: u32, on_click: EventHandler<()>) -> Element {
    let src = api::authenticated_media_url(&format!("/files/{}/thumb?v={}", id, thumb_ver));
    rsx! {
        div {
            class: "aspect-square cursor-pointer overflow-hidden rounded bg-base-200 hover:ring-2 hover:ring-primary transition-all",
            title: "{name}",
            onclick: move |_| on_click.call(()),
            img { class: "w-full h-full object-cover", src: "{src}", loading: "lazy" }
        }
    }
}

// ── TimelineView ─────────────────────────────────────────────────────────────

#[component]
fn TimelineView() -> Element {
    let mut images: Signal<Vec<FileResponse>> = use_signal(Vec::new);
    let mut next_cursor: Signal<Option<String>> = use_signal(|| None);
    let mut loading = use_signal(|| true);
    let mut loading_more = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut thumb_vers: Signal<HashMap<String, u32>> = use_signal(HashMap::new);
    let mut lightbox_index: Signal<Option<usize>> = use_signal(|| None);
    let mut refresh = use_signal(|| 0u32);

    let sse_event = use_context::<Signal<Option<ServerEvent>>>();
    use_effect(move || {
        if let Some(event) = sse_event() {
            match event {
                ServerEvent::ProcessingCompleted { file_id, task_type, success } => {
                    if task_type == "thumbnail" && success {
                        *thumb_vers.write().entry(file_id).or_insert(0) += 1;
                    }
                }
                ServerEvent::FileCreated { .. } | ServerEvent::FileDeleted { .. } => {
                    let next = *refresh.peek() + 1;
                    refresh.set(next);
                }
                _ => {}
            }
        }
    });

    use_effect(move || {
        let _ = refresh();
        spawn(async move {
            loading.set(true);
            error.set(None);
            match use_files::list_gallery(None, None, None).await {
                Ok(resp) => {
                    images.set(resp.files);
                    next_cursor.set(resp.next_cursor);
                }
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    });

    let load_more = move |_| {
        if let Some(cursor) = next_cursor() {
            spawn(async move {
                loading_more.set(true);
                match use_files::list_gallery(Some(&cursor), None, None).await {
                    Ok(resp) => {
                        images.write().extend(resp.files);
                        next_cursor.set(resp.next_cursor);
                    }
                    Err(e) => error.set(Some(e)),
                }
                loading_more.set(false);
            });
        }
    };

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
                h3 { class: "text-lg font-semibold", "Error loading gallery" }
                p { class: "text-base-content/60", "{err}" }
            }
        };
    }

    let imgs = images();
    if imgs.is_empty() {
        return rsx! {
            div { class: "flex flex-col items-center justify-center py-20 gap-3",
                IconImage { class: "w-12 h-12 text-base-content/30".to_string() }
                h3 { class: "text-lg font-semibold", "No images in your Gallery yet" }
                p { class: "text-base-content/60 text-center max-w-md",
                    "Right-click a folder in Files and select \"Gallery settings\" to include it."
                }
            }
        };
    }

    let groups = group_by_date(&imgs);

    rsx! {
        div { class: "space-y-1",
            for (date_label, indices) in groups {
                div {
                    div { class: "sticky top-16 z-10 bg-base-100/90 backdrop-blur-sm py-2 -mx-4 px-4",
                        h2 { class: "text-sm font-semibold text-base-content/70 uppercase tracking-wide", "{date_label}" }
                    }
                    div { class: "grid grid-cols-3 sm:grid-cols-4 md:grid-cols-5 lg:grid-cols-6 xl:grid-cols-8 gap-1 mt-2",
                        for idx in indices {
                            {
                                let file = &imgs[idx];
                                let id = file.id.clone();
                                let name = file.name.clone();
                                let ver = *thumb_vers.read().get(&file.id).unwrap_or(&0);
                                let lb_idx = idx;
                                rsx! {
                                    GalleryThumbnail {
                                        key: "{id}",
                                        id: id.clone(),
                                        name,
                                        thumb_ver: ver,
                                        on_click: move |_| lightbox_index.set(Some(lb_idx)),
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if next_cursor().is_some() {
                div { class: "flex justify-center py-6",
                    button {
                        class: "btn btn-ghost",
                        disabled: loading_more(),
                        onclick: load_more,
                        if loading_more() {
                            span { class: "loading loading-spinner loading-sm" }
                        }
                        "Load more"
                    }
                }
            }
        }

        if let Some(idx) = lightbox_index() {
            Lightbox {
                images: imgs.clone(),
                initial_index: idx,
                on_close: move |_| lightbox_index.set(None),
            }
        }
    }
}

// ── AlbumsGrid ───────────────────────────────────────────────────────────────

#[component]
fn AlbumsGrid(on_select: EventHandler<AlbumResponse>) -> Element {
    let mut albums: Signal<Vec<AlbumResponse>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    use_effect(move || {
        spawn(async move {
            loading.set(true);
            match use_files::list_gallery_albums().await {
                Ok(a) => albums.set(a),
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
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
                IconAlertTriangle { class: "w-12 h-12 text-warning".to_string() }
                h3 { class: "text-lg font-semibold", "Error loading albums" }
                p { class: "text-base-content/60", "{err}" }
            }
        };
    }

    let album_list = albums();
    if album_list.is_empty() {
        return rsx! {
            div { class: "flex flex-col items-center justify-center py-20 gap-3",
                IconImage { class: "w-12 h-12 text-base-content/30".to_string() }
                h3 { class: "text-lg font-semibold", "No albums yet" }
                p { class: "text-base-content/60 text-center max-w-md",
                    "Right-click a folder in Files and select \"Gallery settings\" to include it."
                }
            }
        };
    }

    rsx! {
        div { class: "grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 gap-4",
            for album in album_list {
                {
                    let album_clone = album.clone();
                    let cover_src = album.cover_image_id.as_ref()
                        .map(|id| api::authenticated_media_url(&format!("/files/{}/thumb", id)));
                    let count = album.image_count;
                    rsx! {
                        div {
                            class: "card bg-base-100 shadow-sm border border-base-300 cursor-pointer hover:shadow-md hover:ring-2 hover:ring-primary transition-all",
                            onclick: move |_| on_select.call(album_clone.clone()),
                            div { class: "card-body p-0 gap-0",
                                if let Some(src) = cover_src {
                                    img {
                                        class: "w-full h-32 object-cover rounded-t-xl",
                                        src: "{src}",
                                    }
                                } else {
                                    div { class: "flex items-center justify-center h-32 bg-base-200 rounded-t-xl",
                                        IconImage { class: "w-10 h-10 text-base-content/30".to_string() }
                                    }
                                }
                                div { class: "p-3 text-center",
                                    div { class: "text-sm font-medium truncate", "{album.name}" }
                                    div { class: "text-xs text-base-content/50", "{count} images" }
                                    div { class: "text-xs text-base-content/40 truncate", "{album.path}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ── AlbumView ────────────────────────────────────────────────────────────────

#[component]
fn AlbumView(album: AlbumResponse, on_back: EventHandler<()>) -> Element {
    let mut images: Signal<Vec<FileResponse>> = use_signal(Vec::new);
    let mut next_cursor: Signal<Option<String>> = use_signal(|| None);
    let mut loading = use_signal(|| true);
    let mut loading_more = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut thumb_vers: Signal<HashMap<String, u32>> = use_signal(HashMap::new);
    let mut lightbox_index: Signal<Option<usize>> = use_signal(|| None);

    let folder_id = album.folder_id.clone();
    let folder_id_more = album.folder_id.clone();

    let sse_event = use_context::<Signal<Option<ServerEvent>>>();
    use_effect(move || {
        if let Some(ServerEvent::ProcessingCompleted { file_id, task_type, success }) = sse_event() {
            if task_type == "thumbnail" && success {
                *thumb_vers.write().entry(file_id).or_insert(0) += 1;
            }
        }
    });

    use_effect(move || {
        let fid = folder_id.clone();
        spawn(async move {
            loading.set(true);
            match use_files::list_gallery(None, None, Some(&fid)).await {
                Ok(resp) => {
                    images.set(resp.files);
                    next_cursor.set(resp.next_cursor);
                }
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    });

    let load_more = move |_| {
        if let Some(cursor) = next_cursor() {
            let fid = folder_id_more.clone();
            spawn(async move {
                loading_more.set(true);
                match use_files::list_gallery(Some(&cursor), None, Some(&fid)).await {
                    Ok(resp) => {
                        images.write().extend(resp.files);
                        next_cursor.set(resp.next_cursor);
                    }
                    Err(e) => error.set(Some(e)),
                }
                loading_more.set(false);
            });
        }
    };

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
                h3 { class: "text-lg font-semibold", "Error loading album" }
                p { class: "text-base-content/60", "{err}" }
            }
        };
    }

    let imgs = images();

    rsx! {
        div {
            div { class: "flex items-center gap-3 mb-4",
                button {
                    class: "btn btn-ghost btn-sm",
                    onclick: move |_| on_back.call(()),
                    "← Back"
                }
                h2 { class: "text-xl font-bold", "{album.name}" }
                span { class: "text-sm text-base-content/50", "{album.path}" }
            }

            if imgs.is_empty() {
                div { class: "flex flex-col items-center justify-center py-20 gap-3",
                    IconImage { class: "w-12 h-12 text-base-content/30".to_string() }
                    h3 { class: "text-lg font-semibold", "No images in this album" }
                }
            } else {
                div { class: "grid grid-cols-3 sm:grid-cols-4 md:grid-cols-5 lg:grid-cols-6 xl:grid-cols-8 gap-1",
                    for (idx, file) in imgs.iter().enumerate() {
                        {
                            let id = file.id.clone();
                            let name = file.name.clone();
                            let ver = *thumb_vers.read().get(&file.id).unwrap_or(&0);
                            let lb_idx = idx;
                            rsx! {
                                GalleryThumbnail {
                                    key: "{id}",
                                    id: id.clone(),
                                    name,
                                    thumb_ver: ver,
                                    on_click: move |_| lightbox_index.set(Some(lb_idx)),
                                }
                            }
                        }
                    }
                }

                if next_cursor().is_some() {
                    div { class: "flex justify-center py-6",
                        button {
                            class: "btn btn-ghost",
                            disabled: loading_more(),
                            onclick: load_more,
                            if loading_more() {
                                span { class: "loading loading-spinner loading-sm" }
                            }
                            "Load more"
                        }
                    }
                }
            }
        }

        if let Some(idx) = lightbox_index() {
            Lightbox {
                images: imgs.clone(),
                initial_index: idx,
                on_close: move |_| lightbox_index.set(None),
            }
        }
    }
}

// ── Gallery views enum ───────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum GalleryView {
    Timeline,
    Albums,
}

// ── Gallery (main component) ─────────────────────────────────────────────────

#[component]
pub fn Gallery() -> Element {
    let mut view = use_signal(|| GalleryView::Timeline);
    let mut active_album: Signal<Option<AlbumResponse>> = use_signal(|| None);

    rsx! {
        div { class: "p-4 space-y-4",
            div { class: "flex items-center justify-between",
                h1 { class: "text-2xl font-bold", "Gallery" }
                div { class: "tabs tabs-boxed",
                    a {
                        class: if view() == GalleryView::Timeline && active_album().is_none() { "tab tab-active" } else { "tab" },
                        onclick: move |_| { view.set(GalleryView::Timeline); active_album.set(None); },
                        "Timeline"
                    }
                    a {
                        class: if view() == GalleryView::Albums || active_album().is_some() { "tab tab-active" } else { "tab" },
                        onclick: move |_| { view.set(GalleryView::Albums); active_album.set(None); },
                        "Albums"
                    }
                }
            }

            if let Some(album) = active_album() {
                AlbumView { album, on_back: move |_| active_album.set(None) }
            } else {
                match view() {
                    GalleryView::Timeline => rsx! { TimelineView {} },
                    GalleryView::Albums => rsx! { AlbumsGrid { on_select: move |a| active_album.set(Some(a)) } },
                }
            }
        }
    }
}

// ── GalleryAlbum (route component) ───────────────────────────────────────────

#[component]
pub fn GalleryAlbum(id: String) -> Element {
    let mut album: Signal<Option<AlbumResponse>> = use_signal(|| None);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let nav = use_navigator();

    use_effect(use_reactive!(|id| {
        let target_id = id;
        spawn(async move {
            loading.set(true);
            match use_files::list_gallery_albums().await {
                Ok(albums) => {
                    album.set(albums.into_iter().find(|a| a.folder_id == target_id));
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
            div { class: "p-4",
                div { class: "alert alert-error", "{err}" }
            }
        };
    }

    if let Some(a) = album() {
        rsx! {
            div { class: "p-4",
                AlbumView {
                    album: a,
                    on_back: move |_| { let _ = nav.push(Route::Gallery {}); },
                }
            }
        }
    } else {
        rsx! {
            div { class: "p-4",
                div { class: "alert alert-error", "Album not found" }
            }
        }
    }
}
