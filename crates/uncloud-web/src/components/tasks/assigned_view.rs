use dioxus::prelude::*;
use uncloud_common::{
    ServerEvent, TaskLabelResponse, TaskPriority, TaskResponse, TaskStatus,
    UpdateTaskStatusRequest,
};

use crate::hooks::use_events::use_events;
use crate::hooks::use_tasks;

use super::task_detail::TaskDetail;

fn priority_dot_class(priority: &TaskPriority) -> &'static str {
    match priority {
        TaskPriority::High => "w-2 h-2 rounded-full bg-error shrink-0",
        TaskPriority::Medium => "w-2 h-2 rounded-full bg-warning shrink-0",
        TaskPriority::Low => "w-2 h-2 rounded-full bg-info shrink-0",
    }
}

fn priority_rank(p: &TaskPriority) -> u8 {
    match p {
        TaskPriority::High => 0,
        TaskPriority::Medium => 1,
        TaskPriority::Low => 2,
    }
}

#[component]
pub fn AssignedView() -> Element {
    let mut tasks: Signal<Vec<TaskResponse>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut detail_task_id: Signal<Option<String>> = use_signal(|| None);
    let detail_labels: Signal<Vec<TaskLabelResponse>> = use_signal(Vec::new);

    use_effect(move || {
        spawn(async move {
            loading.set(true);
            error.set(None);
            match use_tasks::get_assigned_to_me().await {
                Ok(t) => tasks.set(t),
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    });

    use_events(move |evt| {
        if matches!(evt, ServerEvent::TaskChanged { .. }) {
            spawn(async move {
                if let Ok(t) = use_tasks::get_assigned_to_me().await {
                    tasks.set(t);
                }
            });
        }
    });

    let refresh = move || {
        spawn(async move {
            if let Ok(t) = use_tasks::get_assigned_to_me().await {
                tasks.set(t);
            }
        });
    };

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

    // Sort: overdue/dated tasks first by due_date asc, then undated by priority.
    let mut ordered: Vec<TaskResponse> = tasks.read().clone();
    ordered.sort_by(|a, b| {
        match (&a.due_date, &b.due_date) {
            (Some(x), Some(y)) => x.cmp(y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => priority_rank(&a.priority).cmp(&priority_rank(&b.priority)),
        }
    });

    rsx! {
        div { class: "max-w-3xl mx-auto",
            h2 { class: "text-2xl font-bold mb-4", "Assigned to me" }

            if ordered.is_empty() {
                div { class: "text-center py-16 text-base-content/40",
                    p { class: "text-lg font-medium mb-1", "Nothing assigned" }
                    p { class: "text-sm",
                        "Open tasks assigned to you will show up here."
                    }
                }
            } else {
                div { class: "flex flex-col gap-1",
                    for task in ordered.iter() {
                        {
                            let task_id_click = task.id.clone();
                            let task_id_check = task.id.clone();
                            let is_done = task.status == TaskStatus::Done;
                            let due_display = task
                                .due_date
                                .as_ref()
                                .map(|d| d.get(..10).unwrap_or(d).to_string())
                                .unwrap_or_default();
                            rsx! {
                                div {
                                    key: "{task.id}",
                                    class: "flex items-center gap-3 py-1.5 px-2 rounded hover:bg-base-200 cursor-pointer group",
                                    onclick: move |_| {
                                        detail_task_id.set(Some(task_id_click.clone()));
                                    },
                                    input {
                                        class: "checkbox checkbox-sm",
                                        r#type: "checkbox",
                                        checked: is_done,
                                        onclick: move |e| {
                                            e.stop_propagation();
                                            let tid = task_id_check.clone();
                                            let new_s = if is_done {
                                                TaskStatus::Todo
                                            } else {
                                                TaskStatus::Done
                                            };
                                            spawn(async move {
                                                let req = UpdateTaskStatusRequest {
                                                    status: new_s,
                                                    status_note: None,
                                                };
                                                let _ =
                                                    use_tasks::update_task_status(&tid, &req).await;
                                                if let Ok(t) =
                                                    use_tasks::get_assigned_to_me().await
                                                {
                                                    tasks.set(t);
                                                }
                                            });
                                        },
                                    }
                                    span { class: priority_dot_class(&task.priority) }
                                    span {
                                        class: if is_done {
                                            "text-sm flex-1 line-through text-base-content/50"
                                        } else {
                                            "text-sm flex-1"
                                        },
                                        "{task.title}"
                                    }
                                    span { class: "badge badge-sm badge-ghost text-xs",
                                        {task.project_id.get(..6).unwrap_or(&task.project_id)}
                                    }
                                    if !due_display.is_empty() {
                                        span { class: "text-xs text-base-content/60", "{due_display}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Some(tid) = detail_task_id.read().clone() {
            TaskDetail {
                task_id: tid,
                available_labels: detail_labels,
                on_close: move |_| {
                    detail_task_id.set(None);
                },
                on_updated: move |_| {
                    refresh();
                },
            }
        }
    }
}
