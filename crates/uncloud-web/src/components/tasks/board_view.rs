use dioxus::prelude::*;
use std::collections::HashSet;
use uncloud_common::{
    CreateTaskRequest, ServerEvent, TaskLabelResponse, TaskProjectResponse, TaskResponse,
    TaskStatus, UpdateTaskStatusRequest,
};

use crate::hooks::use_events::use_events;
use crate::hooks::use_tasks;

use super::board_card::BoardCard;
use super::task_detail::TaskDetail;
use super::{task_matches_label_filter, LabelFilterBar};

/// Column definition for the Kanban board.
struct Column {
    status: TaskStatus,
    label: &'static str,
}

const COLUMNS: &[Column] = &[
    Column { status: TaskStatus::Todo, label: "Backlog" },
    Column { status: TaskStatus::InProgress, label: "In Progress" },
    Column { status: TaskStatus::Blocked, label: "Blocked" },
    Column { status: TaskStatus::Done, label: "Done" },
];

fn status_to_attr(status: &TaskStatus) -> &'static str {
    match status {
        TaskStatus::Todo => "todo",
        TaskStatus::InProgress => "in_progress",
        TaskStatus::Blocked => "blocked",
        TaskStatus::Done => "done",
        TaskStatus::Cancelled => "cancelled",
    }
}

fn attr_to_status(s: &str) -> Option<TaskStatus> {
    match s {
        "todo" => Some(TaskStatus::Todo),
        "in_progress" => Some(TaskStatus::InProgress),
        "blocked" => Some(TaskStatus::Blocked),
        "done" => Some(TaskStatus::Done),
        "cancelled" => Some(TaskStatus::Cancelled),
        _ => None,
    }
}

/// Walk up the DOM from `(x, y)` looking for an element with a
/// `data-column-status` attribute and return the matching `TaskStatus`.
fn column_status_at_point(x: f64, y: f64) -> Option<TaskStatus> {
    let doc = web_sys::window()?.document()?;
    let mut current = doc.element_from_point(x as f32, y as f32);
    while let Some(el) = current {
        if let Some(attr) = el.get_attribute("data-column-status") {
            if let Some(s) = attr_to_status(&attr) {
                return Some(s);
            }
        }
        current = el.parent_element();
    }
    None
}

#[component]
pub fn BoardView(
    project_id: String,
    available_labels: Signal<Vec<TaskLabelResponse>>,
) -> Element {
    let mut project: Signal<Option<TaskProjectResponse>> = use_signal(|| None);
    let mut tasks: Signal<Vec<TaskResponse>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    // Which column is showing the quick-add input
    let mut adding_to: Signal<Option<TaskStatus>> = use_signal(|| None);
    let mut add_title = use_signal(String::new);

    // Task detail slide-over
    let mut detail_task_id: Signal<Option<String>> = use_signal(|| None);
    let mut detail_refresh: Signal<u32> = use_signal(|| 0);

    // Drag state
    let mut drag_task_id: Signal<Option<String>> = use_signal(|| None);
    let mut drop_column: Signal<Option<TaskStatus>> = use_signal(|| None);

    // Label filter (OR semantics — empty = no filter)
    let label_filter: Signal<HashSet<String>> = use_signal(HashSet::new);

    // Store project_id in a signal so closures can read it without moving a String,
    // and so the fetch effect re-runs when the route prop changes.
    let mut pid_sig = use_signal(|| project_id.clone());
    if *pid_sig.peek() != project_id {
        pid_sig.set(project_id.clone());
    }

    // Live updates: refetch tasks when a TaskChanged event for the current
    // project arrives (covers both same-tab actions on shared docs and
    // changes from other devices). Bumps detail_refresh too when the open
    // task is the one that changed, so the slide-over re-fetches.
    use_events(move |evt| {
        if let ServerEvent::TaskChanged { project_id: ev_pid, task_id } = evt {
            if ev_pid == *pid_sig.peek() {
                let pid = ev_pid.clone();
                spawn(async move {
                    if let Ok(t) = use_tasks::list_tasks(&pid, None, None).await {
                        tasks.set(t);
                    }
                });
                if let Some(tid) = task_id {
                    if detail_task_id.peek().as_ref() == Some(&tid) {
                        let next = detail_refresh.peek().wrapping_add(1);
                        detail_refresh.set(next);
                    }
                }
            }
        }
    });

    // Initial fetch (re-runs when pid_sig changes, i.e. user navigated to a different project)
    use_effect(move || {
        let pid = pid_sig.read().clone();
        spawn(async move {
            loading.set(true);
            error.set(None);

            let (proj_res, tasks_res) = futures::join!(
                use_tasks::get_project(&pid),
                use_tasks::list_tasks(&pid, None, None),
            );

            match proj_res {
                Ok(p) => project.set(Some(p)),
                Err(e) => error.set(Some(e)),
            }
            match tasks_res {
                Ok(t) => tasks.set(t),
                Err(e) => {
                    if error.peek().is_none() {
                        error.set(Some(e));
                    }
                }
            }

            loading.set(false);
        });
    });

    if *loading.read() {
        return rsx! {
            div { class: "flex items-center justify-center h-64",
                span { class: "loading loading-spinner loading-lg" }
            }
        };
    }

    if let Some(err) = error.read().as_ref() {
        return rsx! {
            div { class: "alert alert-error",
                span { "{err}" }
            }
        };
    }

    let container_class = if drag_task_id.read().is_some() {
        "flex gap-4 overflow-x-auto lg:overflow-x-visible pb-4 cursor-grabbing"
    } else {
        "flex gap-4 overflow-x-auto lg:overflow-x-visible pb-4"
    };

    rsx! {
        // Label filter strip
        LabelFilterBar {
            available_labels: available_labels.read().clone(),
            selected: label_filter,
        }

        // Board columns
        div {
            class: "{container_class}",
            onpointerup: move |_| {
                let task_id = drag_task_id.peek().clone();
                let target = drop_column.peek().clone();
                drag_task_id.set(None);
                drop_column.set(None);

                if let (Some(tid), Some(new_status)) = (task_id, target) {
                    let current = tasks.peek().iter().find(|t| t.id == tid).map(|t| t.status.clone());
                    if current.as_ref() == Some(&new_status) {
                        return;
                    }
                    spawn(async move {
                        let req = UpdateTaskStatusRequest {
                            status: new_status,
                            status_note: None,
                        };
                        if let Ok(updated) = use_tasks::update_task_status(&tid, &req).await {
                            let mut tw = tasks.write();
                            if let Some(t) = tw.iter_mut().find(|t| t.id == updated.id) {
                                *t = updated;
                            }
                            let next = detail_refresh.peek().wrapping_add(1);
                            detail_refresh.set(next);
                        }
                    });
                }
            },
            // Touch pointers are implicitly captured to the card that received
            // pointerdown, so onpointerenter on sibling columns never fires on
            // mobile. Walk the DOM at the current coordinate instead.
            onpointermove: move |e: Event<PointerData>| {
                if drag_task_id.peek().is_none() {
                    return;
                }
                let pt = e.pointer_type();
                if pt != "touch" && pt != "pen" {
                    return;
                }
                let p = e.client_coordinates();
                if let Some(status) = column_status_at_point(p.x, p.y) {
                    if drop_column.peek().as_ref() != Some(&status) {
                        drop_column.set(Some(status));
                    }
                }
            },
            onpointerleave: move |_| {
                drag_task_id.set(None);
                drop_column.set(None);
            },
            onpointercancel: move |_| {
                drag_task_id.set(None);
                drop_column.set(None);
            },

            for col in COLUMNS.iter() {
                {
                    let col_status = col.status.clone();
                    let col_status_enter = col.status.clone();
                    let col_status_add = col.status.clone();
                    let col_status_submit = col.status.clone();
                    let col_status_attr = status_to_attr(&col.status);
                    let col_label = col.label;

                    let col_tasks: Vec<TaskResponse> = {
                        let filter = label_filter.read();
                        tasks.read()
                            .iter()
                            .filter(|t| t.status == col_status)
                            .filter(|t| task_matches_label_filter(&t.labels, &filter))
                            .cloned()
                            .collect()
                    };
                    let count = col_tasks.len();

                    let is_drop_target = drop_column.read().as_ref() == Some(&col_status);
                    let has_drag = drag_task_id.read().is_some();
                    let col_class = if is_drop_target && has_drag {
                        "flex-shrink-0 w-72 lg:flex-1 lg:min-w-0 lg:w-auto bg-base-200 rounded-box p-3 flex flex-col gap-2 max-h-[calc(100vh-12rem)] overflow-y-auto ring-2 ring-primary"
                    } else {
                        "flex-shrink-0 w-72 lg:flex-1 lg:min-w-0 lg:w-auto bg-base-200 rounded-box p-3 flex flex-col gap-2 max-h-[calc(100vh-12rem)] overflow-y-auto"
                    };

                    let is_adding = adding_to.read().as_ref() == Some(&col_status);

                    rsx! {
                        div {
                            class: "{col_class}",
                            "data-column-status": "{col_status_attr}",
                            // Mouse drag: onpointerenter works because mouse
                            // pointers are not captured. Touch uses the
                            // container-level onpointermove + elementFromPoint.
                            onpointerenter: move |_| {
                                if drag_task_id.read().is_some() {
                                    drop_column.set(Some(col_status_enter.clone()));
                                }
                            },

                            // Column header
                            div { class: "flex items-center justify-between mb-1",
                                div { class: "flex items-center gap-2",
                                    span { class: "font-semibold text-sm", "{col_label}" }
                                    span { class: "badge badge-sm badge-ghost", "{count}" }
                                }
                                div { class: "flex items-center gap-1",
                                    // Clear completed (Done/Cancelled columns only)
                                    if col_status == TaskStatus::Done && count > 0 {
                                        {
                                            let done_task_ids: Vec<String> = col_tasks.iter().map(|t| t.id.clone()).collect();
                                            rsx! {
                                                button {
                                                    class: "btn btn-ghost btn-xs text-error/60 hover:text-error",
                                                    title: "Clear all done tasks",
                                                    onclick: move |_| {
                                                        let ids = done_task_ids.clone();
                                                        spawn(async move {
                                                            for id in &ids {
                                                                let _ = use_tasks::delete_task(id).await;
                                                            }
                                                            tasks.write().retain(|t| !ids.contains(&t.id));
                                                        });
                                                    },
                                                    "Clear"
                                                }
                                            }
                                        }
                                    }
                                    button {
                                        class: "btn btn-ghost btn-xs btn-circle",
                                        onclick: move |_| {
                                            adding_to.set(Some(col_status_add.clone()));
                                            add_title.set(String::new());
                                        },
                                        "+"
                                    }
                                }
                            }

                            // Quick-add input
                            if is_adding {
                                {
                                    let col_status_s = col_status_submit.clone();
                                    rsx! {
                                        div { class: "card bg-base-100 shadow-sm",
                                            div { class: "card-body p-2",
                                                input {
                                                    class: "input input-bordered input-sm w-full",
                                                    r#type: "text",
                                                    placeholder: "Task title...",
                                                    autofocus: true,
                                                    value: "{add_title}",
                                                    oninput: move |e| add_title.set(e.value()),
                                                    onkeydown: move |e: KeyboardEvent| {
                                                        if e.key() == Key::Enter {
                                                            let title = add_title.peek().trim().to_string();
                                                            if title.is_empty() {
                                                                adding_to.set(None);
                                                                return;
                                                            }
                                                            let pid = pid_sig.peek().clone();
                                                            let status = col_status_s.clone();
                                                            spawn(async move {
                                                                let req = CreateTaskRequest {
                                                                    title,
                                                                    status: Some(status),
                                                                    section_id: None,
                                                                    parent_task_id: None,
                                                                    description: None,
                                                                    priority: None,
                                                                    assignee_id: None,
                                                                    labels: None,
                                                                    due_date: None,
                                                                    recurrence_rule: None,
                                                                    position: None,
                                                                };
                                                                if let Ok(task) = use_tasks::create_task(&pid, &req).await {
                                                                    tasks.write().push(task);
                                                                }
                                                                adding_to.set(None);
                                                                add_title.set(String::new());
                                                            });
                                                        } else if e.key() == Key::Escape {
                                                            adding_to.set(None);
                                                            add_title.set(String::new());
                                                        }
                                                    },
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            // Task cards
                            for task in col_tasks.iter() {
                                {
                                    let task_id_click = task.id.clone();
                                    let task_id_drag = task.id.clone();
                                    let is_dragging = drag_task_id.read().as_ref() == Some(&task.id);

                                    rsx! {
                                        BoardCard {
                                            key: "{task.id}",
                                            task: task.clone(),
                                            available_labels: available_labels.read().clone(),
                                            dragging: is_dragging,
                                            on_click: move |_: String| {
                                                detail_task_id.set(Some(task_id_click.clone()));
                                            },
                                            on_drag_start: move |_: String| {
                                                drag_task_id.set(Some(task_id_drag.clone()));
                                            },
                                        }
                                    }
                                }
                            }

                            // Empty state
                            if col_tasks.is_empty() && !is_adding {
                                div { class: "text-base-content/40 text-center text-sm py-8",
                                    "No tasks"
                                }
                            }
                        }
                    }
                }
            }
        }

        // Task detail slide-over
        if let Some(tid) = detail_task_id.read().clone() {
            TaskDetail {
                task_id: tid,
                refresh_key: *detail_refresh.read(),
                available_labels,
                on_close: move |_| { detail_task_id.set(None); },
                on_updated: move |_| {
                    let pid = pid_sig.peek().clone();
                    spawn(async move {
                        if let Ok(t) = use_tasks::list_tasks(&pid, None, None).await {
                            tasks.set(t);
                        }
                    });
                },
                on_deleted: move |id: String| {
                    tasks.write().retain(|t| t.id != id);
                    detail_task_id.set(None);
                },
            }
        }
    }
}
