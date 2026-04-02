use dioxus::prelude::*;
use uncloud_common::TrashItemResponse;
use crate::hooks::use_files;
use crate::router::Route;
use crate::state::HighlightTarget;

#[component]
pub fn Trash() -> Element {
    let mut items: Signal<Vec<TrashItemResponse>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut refresh = use_signal(|| 0u32);

    // Delete confirm: Some(id)
    let mut delete_target: Signal<Option<String>> = use_signal(|| None);
    // Empty trash confirm
    let mut show_empty_confirm = use_signal(|| false);
    // Conflict resolution: Some((item_id, suggested_name))
    let mut conflict_target: Signal<Option<(String, String)>> = use_signal(|| None);
    let mut conflict_name: Signal<String> = use_signal(String::new);

    use_effect(move || {
        let _ = refresh();
        spawn(async move {
            loading.set(true);
            match use_files::list_trash().await {
                Ok(t) => {
                    items.set(t);
                    error.set(None);
                }
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    });

    let item_list = items();
    let is_empty = item_list.is_empty();

    rsx! {
        div { class: "p-4",
            // Header
            div { class: "flex items-center justify-between mb-4",
                h2 { class: "text-2xl font-bold", "Trash" }
                if !is_empty {
                    button {
                        class: "btn btn-error btn-sm",
                        onclick: move |_| show_empty_confirm.set(true),
                        "Empty Trash"
                    }
                }
            }

            if let Some(err) = error() {
                div { class: "alert alert-error mb-4", "{err}" }
            }

            if loading() {
                div { class: "flex justify-center py-12",
                    span { class: "loading loading-spinner loading-lg" }
                }
            } else if is_empty {
                div { class: "card bg-base-100 shadow",
                    div { class: "card-body items-center text-center py-12",
                        p { class: "text-5xl mb-4", "🗑" }
                        p { class: "text-base-content/70", "Trash is empty" }
                    }
                }
            } else {
                div { class: "overflow-x-auto",
                    table { class: "table table-zebra w-full",
                        thead {
                            tr {
                                th { "Name" }
                                th { "Type" }
                                th { "Size" }
                                th { "Deleted" }
                                th { class: "text-right", "Actions" }
                            }
                        }
                        tbody {
                            for item in &item_list {
                                {
                                    let id_restore = item.id.clone();
                                    let id_delete = item.id.clone();
                                    let id_conflict = item.id.clone();
                                    let id_conflict_submit = item.id.clone();
                                    let id_conflict_cancel = item.id.clone();
                                    let name = item.name.clone();
                                    let is_folder = item.is_folder;
                                    let mime = item.mime_type.clone().unwrap_or_default();
                                    let size = item.size_bytes
                                        .map(|s| uncloud_common::validation::format_bytes(s))
                                        .unwrap_or_else(|| "--".to_string());
                                    let deleted = item.deleted_at.clone();
                                    let parent_for_restore = item.parent_id.clone();
                                    let parent_for_conflict = item.parent_id.clone();

                                    // Check if this item has an active conflict
                                    let has_conflict = conflict_target().as_ref().map_or(false, |(cid, _)| *cid == id_conflict);

                                    let name_for_conflict = name.clone();
                                    rsx! {
                                        tr {
                                            td {
                                                span { class: "mr-2",
                                                    { if is_folder { "📁" } else { file_emoji(&mime) } }
                                                }
                                                "{name}"
                                            }
                                            td {
                                                if is_folder { "Folder" } else { "{mime}" }
                                            }
                                            td { "{size}" }
                                            td { class: "text-sm opacity-70", "{format_deleted_at(&deleted)}" }
                                            td { class: "text-right",
                                                button {
                                                    class: "btn btn-ghost btn-xs mr-1",
                                                    title: "Restore",
                                                    onclick: move |_| {
                                                        let id = id_restore.clone();
                                                        let parent = parent_for_restore.clone();
                                                        let item_name = name_for_conflict.clone();
                                                        spawn(async move {
                                                            match use_files::restore_from_trash(&id, None).await {
                                                                Ok(()) => {
                                                                    // Navigate to the parent folder and highlight the restored item
                                                                    let mut hl = consume_context::<Signal<HighlightTarget>>();
                                                                    hl.set(HighlightTarget { file_id: Some(id.clone()) });
                                                                    let nav = navigator();
                                                                    match parent {
                                                                        Some(pid) => nav.push(Route::Folder { id: pid }),
                                                                        None => nav.push(Route::Home {}),
                                                                    };
                                                                }
                                                                Err(e) if e.starts_with("CONFLICT:") => {
                                                                    let suggestion = e[9..].to_string();
                                                                    conflict_name.set(suggestion.clone());
                                                                    conflict_target.set(Some((id.clone(), suggestion)));
                                                                }
                                                                Err(e) => {
                                                                    error.set(Some(e));
                                                                }
                                                            }
                                                        });
                                                    },
                                                    "Restore"
                                                }
                                                button {
                                                    class: "btn btn-ghost btn-xs text-error",
                                                    title: "Delete permanently",
                                                    onclick: move |_| {
                                                        delete_target.set(Some(id_delete.clone()));
                                                    },
                                                    "Delete"
                                                }
                                            }
                                        }
                                        // Inline conflict rename row
                                        if has_conflict {
                                            tr { class: "bg-base-200",
                                                td { colspan: "5",
                                                    div { class: "flex items-center gap-2 py-2 px-1",
                                                        span { class: "text-sm text-base-content/70 shrink-0",
                                                            "Name taken — restore as:"
                                                        }
                                                        input {
                                                            class: "input input-bordered input-sm flex-1",
                                                            r#type: "text",
                                                            value: "{conflict_name}",
                                                            oninput: move |e: Event<FormData>| {
                                                                conflict_name.set(e.value());
                                                            },
                                                        }
                                                        button {
                                                            class: "btn btn-primary btn-sm",
                                                            onclick: move |_| {
                                                                let id = id_conflict_submit.clone();
                                                                let new_name = conflict_name.peek().clone();
                                                                let parent = parent_for_conflict.clone();
                                                                conflict_target.set(None);
                                                                spawn(async move {
                                                                    match use_files::restore_from_trash(&id, Some(&new_name)).await {
                                                                        Ok(()) => {
                                                                            let mut hl = consume_context::<Signal<HighlightTarget>>();
                                                                            hl.set(HighlightTarget { file_id: Some(id.clone()) });
                                                                            let nav = navigator();
                                                                            match parent {
                                                                                Some(pid) => nav.push(Route::Folder { id: pid }),
                                                                                None => nav.push(Route::Home {}),
                                                                            };
                                                                        }
                                                                        Err(e) if e == "CONFLICT" => {
                                                                            // Name still taken; leave dialog open with same name
                                                                            conflict_target.set(Some((id.clone(), new_name.clone())));
                                                                        }
                                                                        Err(e) => {
                                                                            error.set(Some(e));
                                                                        }
                                                                    }
                                                                });
                                                            },
                                                            "Restore as..."
                                                        }
                                                        button {
                                                            class: "btn btn-ghost btn-sm",
                                                            onclick: move |_| {
                                                                let _ = &id_conflict_cancel;
                                                                conflict_target.set(None);
                                                            },
                                                            "Cancel"
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
            }

            // Permanent delete confirm modal
            if let Some(ref del_id) = delete_target() {
                {
                    let did = del_id.clone();
                    rsx! {
                        div { class: "modal modal-open",
                            div { class: "modal-box",
                                h3 { class: "font-bold text-lg mb-2", "Permanently Delete" }
                                p { class: "text-base-content/70",
                                    "This item will be permanently deleted. This cannot be undone."
                                }
                                div { class: "modal-action",
                                    button {
                                        class: "btn",
                                        onclick: move |_| delete_target.set(None),
                                        "Cancel"
                                    }
                                    button {
                                        class: "btn btn-error",
                                        onclick: move |_| {
                                            let id = did.clone();
                                            spawn(async move {
                                                match use_files::permanently_delete_trash(&id).await {
                                                    Ok(()) => {
                                                        delete_target.set(None);
                                                        let next = *refresh.peek() + 1;
                                                        refresh.set(next);
                                                    }
                                                    Err(e) => {
                                                        delete_target.set(None);
                                                        error.set(Some(e));
                                                    }
                                                }
                                            });
                                        },
                                        "Delete Permanently"
                                    }
                                }
                            }
                            div { class: "modal-backdrop", onclick: move |_| delete_target.set(None) }
                        }
                    }
                }
            }

            // Empty trash confirm modal
            if show_empty_confirm() {
                div { class: "modal modal-open",
                    div { class: "modal-box",
                        h3 { class: "font-bold text-lg mb-2", "Empty Trash" }
                        p { class: "text-base-content/70",
                            "All items in the trash will be permanently deleted. This cannot be undone."
                        }
                        div { class: "modal-action",
                            button {
                                class: "btn",
                                onclick: move |_| show_empty_confirm.set(false),
                                "Cancel"
                            }
                            button {
                                class: "btn btn-error",
                                onclick: move |_| {
                                    spawn(async move {
                                        match use_files::empty_trash().await {
                                            Ok(()) => {
                                                show_empty_confirm.set(false);
                                                let next = *refresh.peek() + 1;
                                                refresh.set(next);
                                            }
                                            Err(e) => {
                                                show_empty_confirm.set(false);
                                                error.set(Some(e));
                                            }
                                        }
                                    });
                                },
                                "Empty Trash"
                            }
                        }
                    }
                    div { class: "modal-backdrop", onclick: move |_| show_empty_confirm.set(false) }
                }
            }
        }
    }
}

fn file_emoji(mime: &str) -> &'static str {
    if mime.starts_with("image/") {
        "🖼"
    } else if mime.starts_with("audio/") {
        "🎵"
    } else if mime.starts_with("video/") {
        "🎬"
    } else if mime == "application/pdf" {
        "📄"
    } else {
        "📎"
    }
}

fn format_deleted_at(rfc3339: &str) -> String {
    // Try to parse and display a human-friendly date
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(rfc3339) {
        dt.format("%Y-%m-%d %H:%M").to_string()
    } else {
        rfc3339.to_string()
    }
}
