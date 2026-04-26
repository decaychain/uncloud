use dioxus::prelude::*;
use uncloud_common::{TaskLabelResponse, TaskPriority, TaskResponse};

use super::label_color_for;
use crate::components::icons::IconGripVertical;

fn priority_dot_class(priority: &TaskPriority) -> &'static str {
    match priority {
        TaskPriority::High => "w-2 h-2 rounded-full bg-error shrink-0",
        TaskPriority::Medium => "w-2 h-2 rounded-full bg-warning shrink-0",
        TaskPriority::Low => "w-2 h-2 rounded-full bg-info shrink-0",
    }
}

/// Determine due-date styling. Returns (text, class).
fn due_date_display(due: &str) -> (&'static str, String) {
    // due is an ISO date string like "2026-04-12" or "2026-04-12T00:00:00Z"
    let date_part = &due[..due.len().min(10)];

    // Get today from js_sys
    let now = js_sys::Date::new_0();
    let today = format!(
        "{:04}-{:02}-{:02}",
        now.get_full_year(),
        now.get_month() + 1,
        now.get_date(),
    );

    if date_part < today.as_str() {
        ("overdue", format!("badge badge-sm badge-error gap-1"))
    } else if date_part == today.as_str() {
        ("today", format!("badge badge-sm badge-warning gap-1"))
    } else {
        ("", format!("badge badge-sm badge-ghost gap-1"))
    }
}

#[component]
pub fn BoardCard(
    task: TaskResponse,
    #[props(default)] available_labels: Vec<TaskLabelResponse>,
    on_click: EventHandler<String>,
    on_drag_start: EventHandler<String>,
    dragging: bool,
) -> Element {
    let card_class = if dragging {
        "card bg-base-100 shadow-sm cursor-pointer select-none opacity-30 flex-row"
    } else {
        "card bg-base-100 shadow-sm cursor-pointer select-none hover:shadow-md transition-shadow flex-row"
    };

    let task_id_click = task.id.clone();
    let task_id_drag = task.id.clone();

    rsx! {
        div {
            class: "{card_class}",
            onclick: move |_| on_click.call(task_id_click.clone()),

            // Grip handle — only this zone initiates drag. `touch-action: none`
            // means the browser won't interpret a touch here as a scroll. Rest
            // of the card keeps default touch-action so vertical column
            // scrolling still works when you pull on the card body.
            div {
                class: "flex items-center justify-center w-10 shrink-0 text-base-content/30 cursor-grab active:cursor-grabbing rounded-l-box hover:bg-base-200 hover:text-base-content/60",
                style: "touch-action: none;",
                title: "Drag to move",
                onpointerdown: move |e: Event<PointerData>| {
                    e.stop_propagation();
                    on_drag_start.call(task_id_drag.clone());
                },
                // Stop click from bubbling up to the card's onclick so
                // tapping the grip doesn't open the task detail.
                onclick: move |e: Event<MouseData>| {
                    e.stop_propagation();
                },
                IconGripVertical { class: "w-4 h-4".to_string() }
            }

            div { class: "card-body p-3 gap-1 flex-1",
                // Title row with priority dot
                div { class: "flex items-start gap-2",
                    span { class: priority_dot_class(&task.priority) }
                    span { class: "text-sm font-medium leading-tight line-clamp-2",
                        "{task.title}"
                    }
                }

                // Status note (if present)
                if let Some(note) = &task.status_note {
                    if !note.is_empty() {
                        p { class: "text-xs italic text-base-content/50 line-clamp-1",
                            "{note}"
                        }
                    }
                }

                // Labels as Trello-style coloured bars (compact for the Kanban board).
                // Each bar is 6px tall + 14px wide; the label name is in the title for hover.
                if !task.labels.is_empty() {
                    div { class: "flex flex-wrap gap-1 mt-1",
                        for label in task.labels.iter() {
                            {
                                let color = label_color_for(&available_labels, label).to_string();
                                rsx! {
                                    span {
                                        key: "{label}",
                                        class: "h-1.5 w-3.5 rounded-sm",
                                        style: "background: {color};",
                                        title: "{label}",
                                    }
                                }
                            }
                        }
                    }
                }

                // Bottom row: assignee, due date, subtasks, comments
                div { class: "flex items-center gap-2 mt-1 text-base-content/60",
                    // Assignee
                    if let Some(username) = &task.assignee_username {
                        div {
                            class: "avatar placeholder",
                            div { class: "bg-neutral text-neutral-content w-5 h-5 rounded-full",
                                span { class: "text-[10px]",
                                    {username.chars().next().unwrap_or('?').to_uppercase().to_string()}
                                }
                            }
                        }
                    }

                    // Due date
                    if let Some(due) = &task.due_date {
                        {
                            let (_label, badge_class) = due_date_display(due);
                            let display = &due[..due.len().min(10)];
                            rsx! {
                                span { class: "{badge_class}", "{display}" }
                            }
                        }
                    }

                    // Spacer
                    div { class: "flex-1" }

                    // Subtask progress
                    if task.subtask_count > 0 {
                        span { class: "text-xs flex items-center gap-0.5",
                            "✓ {task.subtask_done_count}/{task.subtask_count}"
                        }
                    }

                    // Comment count
                    if task.comment_count > 0 {
                        span { class: "text-xs flex items-center gap-0.5",
                            // Message icon placeholder
                            svg {
                                class: "w-3 h-3 shrink-0",
                                xmlns: "http://www.w3.org/2000/svg",
                                width: "24",
                                height: "24",
                                view_box: "0 0 24 24",
                                fill: "none",
                                stroke: "currentColor",
                                stroke_width: "2",
                                stroke_linecap: "round",
                                stroke_linejoin: "round",
                                path { d: "M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" }
                            }
                            "{task.comment_count}"
                        }
                    }
                }
            }
        }
    }
}
