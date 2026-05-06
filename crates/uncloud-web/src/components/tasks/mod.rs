pub mod assigned_view;
pub mod board_view;
pub mod board_card;
pub mod list_view;
pub mod project_settings;
pub mod task_detail;
pub mod schedule_view;

use dioxus::prelude::*;
use gloo_storage::{LocalStorage, Storage};
use std::collections::HashSet;
use uncloud_common::{ProjectView, ServerEvent, TaskLabelResponse};

pub use schedule_view::ScheduleView;

use crate::hooks::use_events::use_events;
use crate::hooks::use_tasks;
use project_settings::ProjectSettings;

/// Fixed palette for task labels. New labels are constrained to these so the
/// visual language stays consistent across the app.
pub const LABEL_PALETTE: &[&str] = &[
    "#ef4444", // red
    "#f97316", // orange
    "#eab308", // yellow
    "#22c55e", // green
    "#14b8a6", // teal
    "#3b82f6", // blue
    "#a855f7", // purple
    "#ec4899", // pink
];

/// Default fallback colour for labels that no longer have a matching TaskLabel
/// document (e.g. just-deleted server-side, brief race window).
pub const LABEL_FALLBACK_COLOR: &str = "#6b7280"; // gray-500

/// Look up the colour for a label name in the project's label catalogue,
/// returning a stable fallback when the name is not found.
pub fn label_color_for<'a>(labels: &'a [TaskLabelResponse], name: &str) -> &'a str {
    labels
        .iter()
        .find(|l| l.name == name)
        .map(|l| l.color.as_str())
        .unwrap_or(LABEL_FALLBACK_COLOR)
}

/// Reusable filter strip for narrowing tasks by label.
/// `selected` holds the set of active label names (OR semantics: a task matches
/// when at least one of its labels is in `selected`). When empty, nothing is
/// filtered.
#[component]
pub fn LabelFilterBar(
    available_labels: Vec<TaskLabelResponse>,
    selected: Signal<HashSet<String>>,
) -> Element {
    let mut open = use_signal(|| false);
    let mut selected = selected;

    if available_labels.is_empty() {
        return rsx! { div {} };
    }

    let active_count = selected.read().len();
    let labels_for_pills = available_labels.clone();

    rsx! {
        div { class: "flex items-center flex-wrap gap-1.5 mb-3",
            button {
                class: if active_count > 0 {
                    "btn btn-sm btn-primary gap-1"
                } else {
                    "btn btn-sm btn-ghost gap-1"
                },
                onclick: move |_| {
                    let next = !*open.peek();
                    open.set(next);
                },
                // Filter icon
                svg {
                    class: "w-3.5 h-3.5",
                    xmlns: "http://www.w3.org/2000/svg",
                    view_box: "0 0 24 24",
                    fill: "none",
                    stroke: "currentColor",
                    stroke_width: "2",
                    stroke_linecap: "round",
                    stroke_linejoin: "round",
                    polygon { points: "22 3 2 3 10 12.46 10 19 14 21 14 12.46 22 3" }
                }
                "Labels"
                if active_count > 0 {
                    span { class: "badge badge-xs badge-neutral", "{active_count}" }
                }
            }

            // Active filter pills
            for name in selected.read().iter().cloned().collect::<Vec<_>>() {
                {
                    let color = label_color_for(&labels_for_pills, &name).to_string();
                    let name_remove = name.clone();
                    rsx! {
                        span {
                            key: "{name}",
                            class: "inline-flex items-center gap-1 px-2 py-0.5 rounded text-xs font-medium text-white",
                            style: "background: {color};",
                            "{name}"
                            button {
                                class: "opacity-70 hover:opacity-100 leading-none",
                                title: "Clear",
                                onclick: move |_| {
                                    selected.write().remove(&name_remove);
                                },
                                "×"
                            }
                        }
                    }
                }
            }

            if active_count > 0 {
                button {
                    class: "btn btn-ghost btn-xs",
                    onclick: move |_| selected.write().clear(),
                    "Clear all"
                }
            }
        }

        // Picker dropdown
        if *open.read() {
            div { class: "mb-3 p-2 bg-base-200 rounded-box max-w-md",
                div { class: "max-h-48 overflow-y-auto space-y-1",
                    for label in available_labels.iter() {
                        {
                            let l_name = label.name.clone();
                            let l_color = label.color.clone();
                            let l_id = label.id.clone();
                            let is_selected = selected.read().contains(&l_name);
                            let l_name_toggle = l_name.clone();

                            rsx! {
                                button {
                                    key: "{l_id}",
                                    class: "w-full flex items-center gap-2 px-2 py-1 rounded hover:bg-base-300 text-left",
                                    onclick: move |_| {
                                        let mut s = selected.write();
                                        if s.contains(&l_name_toggle) {
                                            s.remove(&l_name_toggle);
                                        } else {
                                            s.insert(l_name_toggle.clone());
                                        }
                                    },
                                    input {
                                        class: "checkbox checkbox-xs pointer-events-none",
                                        r#type: "checkbox",
                                        checked: is_selected,
                                    }
                                    span {
                                        class: "px-2 py-0.5 rounded text-xs font-medium text-white",
                                        style: "background: {l_color};",
                                        "{l_name}"
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

/// True when the task should be visible given the current label filter.
/// Empty filter = no narrowing.
pub fn task_matches_label_filter(task_labels: &[String], filter: &HashSet<String>) -> bool {
    filter.is_empty() || task_labels.iter().any(|l| filter.contains(l))
}

const LS_VIEW_MODE: &str = "tasks-view-mode";

fn load_view_mode() -> Option<ProjectView> {
    let s: String = LocalStorage::get(LS_VIEW_MODE).ok()?;
    match s.as_str() {
        "board" => Some(ProjectView::Board),
        "list" => Some(ProjectView::List),
        _ => None,
    }
}

fn save_view_mode(v: &ProjectView) {
    let s = match v {
        ProjectView::Board => "board",
        ProjectView::List => "list",
    };
    let _ = LocalStorage::set(LS_VIEW_MODE, s);
}

#[component]
pub fn TasksSchedulePage() -> Element {
    rsx! {
        div { class: "p-4",
            ScheduleView {}
        }
    }
}

#[component]
pub fn TasksAssignedPage() -> Element {
    rsx! {
        div { class: "p-4",
            assigned_view::AssignedView {}
        }
    }
}

#[component]
pub fn TasksProjectPage(project_id: String) -> Element {
    let nav = use_navigator();
    let mut view_mode: Signal<ProjectView> =
        use_signal(|| load_view_mode().unwrap_or(ProjectView::Board));
    let mut project_name = use_signal(|| String::new());
    let mut project_color = use_signal(|| "#3B82F6".to_string());
    let mut project_owner_id = use_signal(String::new);
    let mut project_members: Signal<Vec<uncloud_common::ProjectMemberResponse>> =
        use_signal(Vec::new);
    let mut show_settings = use_signal(|| false);

    // Project label catalogue, lifted here so BoardView, ListView, ProjectSettings,
    // and the TaskDetail mounted under board/list views all share one source of
    // truth — edits in one surface appear immediately in the others.
    let mut available_labels: Signal<Vec<TaskLabelResponse>> = use_signal(Vec::new);

    // Sync prop into a Signal so use_effect re-runs when the route changes
    // (clicking a different project in the sidebar swaps the prop in place).
    let mut pid_sig = use_signal(|| project_id.clone());
    if *pid_sig.peek() != project_id {
        pid_sig.set(project_id.clone());
    }

    // Fetch project to get name + default_view, and the project's label catalogue
    use_effect(move || {
        let pid = pid_sig.read().clone();
        spawn(async move {
            if let Ok(p) = use_tasks::get_project(&pid).await {
                project_name.set(p.name);
                project_color.set(p.color.unwrap_or_else(|| "#3B82F6".to_string()));
                project_owner_id.set(p.owner_id);
                project_members.set(p.members);
                if load_view_mode().is_none() {
                    view_mode.set(p.default_view);
                }
            }
            if let Ok(ls) = use_tasks::list_labels(&pid).await {
                available_labels.set(ls);
            } else {
                available_labels.set(Vec::new());
            }
        });
    });

    // Live updates: refetch the label catalogue whenever a TaskChanged event
    // for the current project arrives. Sections / labels / member changes
    // emit `task_id: None`, so we can't narrow further without losing
    // events; refetching the small label list on any task change is cheap
    // and keeps the UI consistent across tabs and devices.
    use_events(move |evt| {
        if let ServerEvent::TaskChanged { project_id: ev_pid, .. } = evt {
            if ev_pid == *pid_sig.peek() {
                let pid = ev_pid;
                spawn(async move {
                    if let Ok(ls) = use_tasks::list_labels(&pid).await {
                        available_labels.set(ls);
                    }
                });
            }
        }
    });

    rsx! {
        div { class: "p-4",
            // View toggle header
            div { class: "flex items-center justify-between mb-4",
                div { class: "flex items-center gap-2",
                    h1 { class: "text-2xl font-bold", "{project_name}" }
                    button {
                        class: "btn btn-ghost btn-sm btn-circle",
                        onclick: move |_| show_settings.set(true),
                        // Gear icon (Lucide settings)
                        svg {
                            class: "w-4 h-4",
                            xmlns: "http://www.w3.org/2000/svg",
                            width: "24",
                            height: "24",
                            view_box: "0 0 24 24",
                            fill: "none",
                            stroke: "currentColor",
                            stroke_width: "2",
                            stroke_linecap: "round",
                            stroke_linejoin: "round",
                            path { d: "M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z" }
                            circle { cx: "12", cy: "12", r: "3" }
                        }
                    }
                }
                div { class: "join",
                    button {
                        class: if *view_mode.read() == ProjectView::Board {
                            "btn btn-sm join-item btn-active"
                        } else {
                            "btn btn-sm join-item"
                        },
                        onclick: move |_| {
                            view_mode.set(ProjectView::Board);
                            save_view_mode(&ProjectView::Board);
                        },
                        // Kanban icon
                        svg {
                            class: "w-4 h-4 mr-1",
                            xmlns: "http://www.w3.org/2000/svg",
                            width: "24",
                            height: "24",
                            view_box: "0 0 24 24",
                            fill: "none",
                            stroke: "currentColor",
                            stroke_width: "2",
                            stroke_linecap: "round",
                            stroke_linejoin: "round",
                            rect { x: "3", y: "3", width: "7", height: "18", rx: "1" }
                            rect { x: "14", y: "3", width: "7", height: "10", rx: "1" }
                        }
                        "Board"
                    }
                    button {
                        class: if *view_mode.read() == ProjectView::List {
                            "btn btn-sm join-item btn-active"
                        } else {
                            "btn btn-sm join-item"
                        },
                        onclick: move |_| {
                            view_mode.set(ProjectView::List);
                            save_view_mode(&ProjectView::List);
                        },
                        // List icon
                        svg {
                            class: "w-4 h-4 mr-1",
                            xmlns: "http://www.w3.org/2000/svg",
                            width: "24",
                            height: "24",
                            view_box: "0 0 24 24",
                            fill: "none",
                            stroke: "currentColor",
                            stroke_width: "2",
                            stroke_linecap: "round",
                            stroke_linejoin: "round",
                            path { d: "M3 12h18" }
                            path { d: "M3 6h18" }
                            path { d: "M3 18h18" }
                        }
                        "List"
                    }
                }
            }
            // Render the appropriate view
            match *view_mode.read() {
                ProjectView::List => rsx! {
                    list_view::ListView {
                        project_id: project_id.clone(),
                        available_labels,
                    }
                },
                _ => rsx! {
                    board_view::BoardView {
                        project_id: project_id.clone(),
                        available_labels,
                    }
                },
            }
        }

        // Project settings modal
        if *show_settings.read() {
            ProjectSettings {
                project_id: project_id.clone(),
                project_name: project_name.read().clone(),
                project_color: project_color.read().clone(),
                owner_id: project_owner_id.read().clone(),
                members: project_members.read().clone(),
                available_labels,
                on_close: move |_| show_settings.set(false),
                on_updated: move |new_name: String| {
                    project_name.set(new_name);
                },
                on_deleted: move |_| {
                    nav.push(crate::router::Route::Tasks {});
                },
            }
        }
    }
}
