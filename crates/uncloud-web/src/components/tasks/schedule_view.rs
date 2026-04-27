use dioxus::prelude::*;
use uncloud_common::{
    ServerEvent, TaskLabelResponse, TaskPriority, TaskResponse, TaskScheduleResponse, TaskStatus,
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

struct ScheduleGroup {
    label: &'static str,
    badge_class: &'static str,
}

const GROUPS: &[ScheduleGroup] = &[
    ScheduleGroup { label: "Overdue", badge_class: "badge badge-error badge-sm" },
    ScheduleGroup { label: "Today", badge_class: "badge badge-warning badge-sm" },
    ScheduleGroup { label: "Tomorrow", badge_class: "badge badge-info badge-sm" },
    ScheduleGroup { label: "Next 7 Days", badge_class: "badge badge-sm" },
    ScheduleGroup { label: "Later", badge_class: "badge badge-ghost badge-sm" },
];

#[component]
pub fn ScheduleView() -> Element {
    let mut schedule: Signal<Option<TaskScheduleResponse>> = use_signal(|| None);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut detail_task_id: Signal<Option<String>> = use_signal(|| None);
    // Per-task local label catalogue. ScheduleView spans projects, so we don't
    // pre-populate this — TaskDetail's effect fetches based on the opened task.
    let detail_labels: Signal<Vec<TaskLabelResponse>> = use_signal(Vec::new);

    // Collapsed state per group index
    let mut collapsed: Signal<[bool; 5]> = use_signal(|| [false; 5]);

    // Fetch schedule
    use_effect(move || {
        spawn(async move {
            loading.set(true);
            error.set(None);
            match use_tasks::get_schedule().await {
                Ok(s) => schedule.set(Some(s)),
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    });

    // Live updates: ScheduleView spans projects, so any TaskChanged is
    // potentially relevant. Refetch the schedule on every event.
    use_events(move |evt| {
        if matches!(evt, ServerEvent::TaskChanged { .. }) {
            spawn(async move {
                if let Ok(s) = use_tasks::get_schedule().await {
                    schedule.set(Some(s));
                }
            });
        }
    });

    let refresh = move || {
        spawn(async move {
            match use_tasks::get_schedule().await {
                Ok(s) => schedule.set(Some(s)),
                Err(_) => {}
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

    let sched = match schedule.read().as_ref() {
        Some(s) => s.clone(),
        None => return rsx! {},
    };

    let group_tasks: [&Vec<TaskResponse>; 5] = [
        &sched.overdue,
        &sched.today,
        &sched.tomorrow,
        &sched.next_7_days,
        &sched.later,
    ];

    rsx! {
        div { class: "max-w-3xl mx-auto",
            h2 { class: "text-2xl font-bold mb-4", "Schedule" }

            div { class: "flex flex-col gap-2",
                for (i, group) in GROUPS.iter().enumerate() {
                    {
                        let tasks = group_tasks[i];
                        let count = tasks.len();
                        let is_collapsed = collapsed.read()[i];

                        if count == 0 {
                            // Skip empty groups
                            rsx! {}
                        } else {
                            rsx! {
                                // Group header
                                div {
                                    class: "flex items-center gap-2 cursor-pointer select-none py-2",
                                    onclick: move |_| {
                                        let mut c = *collapsed.read();
                                        c[i] = !c[i];
                                        collapsed.set(c);
                                    },
                                    // Chevron
                                    svg {
                                        class: if is_collapsed { "w-4 h-4 shrink-0 transition-transform -rotate-90" } else { "w-4 h-4 shrink-0 transition-transform" },
                                        xmlns: "http://www.w3.org/2000/svg",
                                        width: "24",
                                        height: "24",
                                        view_box: "0 0 24 24",
                                        fill: "none",
                                        stroke: "currentColor",
                                        stroke_width: "2",
                                        stroke_linecap: "round",
                                        stroke_linejoin: "round",
                                        path { d: "m6 9 6 6 6-6" }
                                    }
                                    span { class: "font-semibold text-sm", "{group.label}" }
                                    span { class: "{group.badge_class}", "{count}" }
                                }

                                // Task rows
                                if !is_collapsed {
                                    div { class: "flex flex-col gap-1 ml-6 mb-2",
                                        for task in tasks.iter() {
                                            {
                                                let task_id_click = task.id.clone();
                                                let task_id_check = task.id.clone();
                                                let is_done = task.status == TaskStatus::Done;
                                                let due_display = task.due_date.as_ref()
                                                    .map(|d| d.get(..10).unwrap_or(d).to_string())
                                                    .unwrap_or_default();

                                                rsx! {
                                                    div {
                                                        class: "flex items-center gap-3 py-1.5 px-2 rounded hover:bg-base-200 cursor-pointer group",
                                                        onclick: move |_| {
                                                            detail_task_id.set(Some(task_id_click.clone()));
                                                        },

                                                        // Quick-complete checkbox
                                                        input {
                                                            class: "checkbox checkbox-sm",
                                                            r#type: "checkbox",
                                                            checked: is_done,
                                                            onclick: move |e| {
                                                                e.stop_propagation();
                                                                let tid = task_id_check.clone();
                                                                let new_s = if is_done { TaskStatus::Todo } else { TaskStatus::Done };
                                                                spawn(async move {
                                                                    let req = UpdateTaskStatusRequest {
                                                                        status: new_s,
                                                                        status_note: None,
                                                                    };
                                                                    let _ = use_tasks::update_task_status(&tid, &req).await;
                                                                    // Refresh schedule
                                                                    if let Ok(s) = use_tasks::get_schedule().await {
                                                                        schedule.set(Some(s));
                                                                    }
                                                                });
                                                            },
                                                        }

                                                        // Priority dot
                                                        span { class: priority_dot_class(&task.priority) }

                                                        // Title
                                                        span {
                                                            class: if is_done {
                                                                "text-sm flex-1 line-through text-base-content/50"
                                                            } else {
                                                                "text-sm flex-1"
                                                            },
                                                            "{task.title}"
                                                        }

                                                        // Project ID as small chip (project name not available in TaskResponse directly)
                                                        span { class: "badge badge-sm badge-ghost text-xs",
                                                            {task.project_id.get(..6).unwrap_or(&task.project_id)}
                                                        }

                                                        // Due date
                                                        if !due_display.is_empty() {
                                                            span { class: "text-xs text-base-content/60",
                                                                "{due_display}"
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

                // Empty state
                {
                    let total: usize = group_tasks.iter().map(|g| g.len()).sum();
                    if total == 0 {
                        rsx! {
                            div { class: "text-center py-16 text-base-content/40",
                                p { class: "text-lg font-medium mb-1", "All clear" }
                                p { class: "text-sm", "No scheduled tasks. Create tasks in a project to see them here." }
                            }
                        }
                    } else {
                        rsx! {}
                    }
                }
            }
        }

        // Task detail slide-over
        if let Some(tid) = detail_task_id.read().clone() {
            TaskDetail {
                task_id: tid,
                available_labels: detail_labels,
                on_close: move |_| { detail_task_id.set(None); },
                on_updated: move |_| { refresh(); },
            }
        }
    }
}
