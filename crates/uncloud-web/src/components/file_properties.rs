use dioxus::prelude::*;
use uncloud_common::{FileResponse, ProcessingTaskInfo, ServerEvent};

use crate::components::icons::{IconAlertTriangle, IconCheck};
use crate::components::right_drawer::RightDrawer;
use crate::hooks::use_files;

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Info,
    Processing,
}

/// Right-side drawer showing file metadata + post-processing status. Subscribes
/// to SSE `ProcessingCompleted` events and refetches when the open file's tasks
/// transition.
#[component]
pub fn FilePropertiesDrawer(
    /// Target file id; when `None` the drawer is closed.
    file_id: Option<String>,
    on_close: EventHandler<()>,
) -> Element {
    let mut file: Signal<Option<FileResponse>> = use_signal(|| None);
    let mut loading = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut tab = use_signal(|| Tab::Info);
    let mut refresh = use_signal(|| 0u32);

    // Stash the current id in a signal so it can be observed by use_effect.
    let fid_prop = file_id.clone();
    let mut fid_signal: Signal<Option<String>> = use_signal(|| fid_prop.clone());
    if *fid_signal.peek() != fid_prop {
        fid_signal.set(fid_prop);
        tab.set(Tab::Info);
    }

    // Fetch when the file id or refresh counter changes.
    use_effect(move || {
        let _ = refresh();
        let Some(id) = fid_signal() else {
            file.set(None);
            error.set(None);
            return;
        };
        spawn(async move {
            loading.set(true);
            error.set(None);
            match use_files::get_file(&id).await {
                Ok(f) => file.set(Some(f)),
                Err(e) => {
                    file.set(None);
                    error.set(Some(e));
                }
            }
            loading.set(false);
        });
    });

    // Refetch on processing events for this file.
    let sse_event = use_context::<Signal<Option<ServerEvent>>>();
    use_effect(move || {
        let Some(ServerEvent::ProcessingCompleted { file_id: evt_id, .. }) = sse_event() else {
            return;
        };
        let Some(current) = fid_signal.peek().clone() else {
            return;
        };
        if evt_id == current {
            let next = *refresh.peek() + 1;
            refresh.set(next);
        }
    });

    let open = file_id.is_some();
    let title = file
        .read()
        .as_ref()
        .map(|f| f.name.clone())
        .unwrap_or_else(|| "File properties".to_string());

    rsx! {
        RightDrawer { open, title, on_close: move |_| on_close.call(()),
            if loading() && file.read().is_none() {
                div { class: "flex items-center justify-center py-12",
                    span { class: "loading loading-spinner loading-lg" }
                }
            } else if let Some(err) = error() {
                div { class: "alert alert-error text-sm",
                    IconAlertTriangle { class: "w-4 h-4 shrink-0".to_string() }
                    span { "{err}" }
                }
            } else if let Some(f) = file.read().clone() {
                // Tab strip
                div { role: "tablist", class: "tabs tabs-bordered mb-4",
                    a {
                        role: "tab",
                        class: if tab() == Tab::Info { "tab tab-active" } else { "tab" },
                        onclick: move |_| tab.set(Tab::Info),
                        "Info"
                    }
                    a {
                        role: "tab",
                        class: if tab() == Tab::Processing { "tab tab-active" } else { "tab" },
                        onclick: move |_| tab.set(Tab::Processing),
                        "Processing "
                        ProcessingBadge { tasks: f.processing_tasks.clone() }
                    }
                }

                if tab() == Tab::Info {
                    InfoTab { file: f.clone() }
                } else {
                    ProcessingTab { tasks: f.processing_tasks.clone() }
                }
            }
        }
    }
}

#[component]
fn ProcessingBadge(tasks: Vec<ProcessingTaskInfo>) -> Element {
    let has_error = tasks.iter().any(|t| t.status == "error");
    let has_pending = tasks.iter().any(|t| t.status == "pending");
    if has_error {
        return rsx! { span { class: "badge badge-error badge-xs ml-1" } };
    }
    if has_pending {
        return rsx! { span { class: "badge badge-warning badge-xs ml-1" } };
    }
    rsx! {}
}

#[component]
fn InfoTab(file: FileResponse) -> Element {
    let size = file.formatted_size();
    rsx! {
        dl { class: "grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm",
            InfoRow { label: "Name", value: file.name.clone() }
            InfoRow { label: "Type", value: file.mime_type.clone() }
            InfoRow { label: "Size", value: size }
            InfoRow { label: "Created", value: format_dt(&file.created_at) }
            InfoRow { label: "Modified", value: format_dt(&file.updated_at) }
            if let Some(captured) = file.captured_at.as_ref() {
                InfoRow { label: "Captured", value: format_dt(captured) }
            }
            InfoRow { label: "ID", value: file.id.clone() }
        }
    }
}

#[component]
fn InfoRow(label: String, value: String) -> Element {
    rsx! {
        dt { class: "text-base-content/60 font-medium", "{label}" }
        dd { class: "break-all", "{value}" }
    }
}

#[component]
fn ProcessingTab(tasks: Vec<ProcessingTaskInfo>) -> Element {
    if tasks.is_empty() {
        return rsx! {
            p { class: "text-sm text-base-content/60",
                "No post-processing has been applied to this file yet. If this file was uploaded before processing was introduced, it will be picked up on the next server start."
            }
        };
    }

    rsx! {
        div { class: "space-y-3",
            for task in tasks {
                TaskCard { task: task }
            }
        }
    }
}

#[component]
fn TaskCard(task: ProcessingTaskInfo) -> Element {
    let (status_class, status_label, status_icon) = match task.status.as_str() {
        "done" => ("badge-success", "Done", rsx! { IconCheck { class: "w-3 h-3".to_string() } }),
        "pending" => ("badge-warning", "Pending", rsx! { span { class: "loading loading-spinner loading-xs" } }),
        "error" => ("badge-error", "Error", rsx! { IconAlertTriangle { class: "w-3 h-3".to_string() } }),
        _ => ("badge-ghost", task.status.as_str(), rsx! {}),
    };

    let task_name = task_type_label(&task.task_type);

    rsx! {
        div { class: "card card-compact bg-base-200 border border-base-300",
            div { class: "card-body gap-2",
                div { class: "flex items-center justify-between gap-2",
                    span { class: "font-medium text-sm", "{task_name}" }
                    span { class: "badge badge-sm gap-1 {status_class}",
                        {status_icon}
                        "{status_label}"
                    }
                }

                div { class: "grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs",
                    span { class: "text-base-content/60", "Attempts" }
                    span { "{task.attempts}" }
                    span { class: "text-base-content/60", "Queued" }
                    span { {format_dt(&task.queued_at)} }
                    if let Some(done) = task.completed_at.as_ref() {
                        span { class: "text-base-content/60", "Completed" }
                        span { {format_dt(done)} }
                    }
                }

                if let Some(err) = task.error.as_ref() {
                    div { class: "alert alert-error py-2 text-xs",
                        span { class: "break-words", "{err}" }
                    }
                }
            }
        }
    }
}

fn task_type_label(raw: &str) -> &str {
    match raw {
        "thumbnail" => "Thumbnail",
        "audio_metadata" => "Audio metadata",
        "text_extract" => "Text extraction",
        "search_index" => "Search index",
        other => other,
    }
}

/// Render an ISO-8601 timestamp as a locale-friendly short string. Falls back
/// to the raw string on parse failure so the user sees something either way.
fn format_dt(raw: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(raw) {
        Ok(dt) => dt.format("%Y-%m-%d %H:%M:%S").to_string(),
        Err(_) => raw.to_string(),
    }
}
