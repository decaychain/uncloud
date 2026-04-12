pub mod board_view;
pub mod board_card;
pub mod list_view;
pub mod project_settings;
pub mod task_detail;
pub mod schedule_view;

use dioxus::prelude::*;
use uncloud_common::ProjectView;

pub use schedule_view::ScheduleView;

use crate::hooks::use_tasks;
use project_settings::ProjectSettings;

#[component]
pub fn TasksSchedulePage() -> Element {
    rsx! {
        div { class: "p-4",
            ScheduleView {}
        }
    }
}

#[component]
pub fn TasksProjectPage(project_id: String) -> Element {
    let nav = use_navigator();
    let mut view_mode: Signal<ProjectView> = use_signal(|| ProjectView::Board);
    let mut project_name = use_signal(|| String::new());
    let mut project_color = use_signal(|| "#3B82F6".to_string());
    let mut project_owner_id = use_signal(String::new);
    let mut project_members: Signal<Vec<uncloud_common::ProjectMemberResponse>> =
        use_signal(Vec::new);
    let mut show_settings = use_signal(|| false);

    // Fetch project to get name + default_view
    let pid = project_id.clone();
    use_effect(move || {
        let pid = pid.clone();
        spawn(async move {
            if let Ok(p) = use_tasks::get_project(&pid).await {
                project_name.set(p.name);
                project_color.set(p.color.unwrap_or_else(|| "#3B82F6".to_string()));
                project_owner_id.set(p.owner_id);
                project_members.set(p.members);
                view_mode.set(p.default_view);
            }
        });
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
                        onclick: move |_| view_mode.set(ProjectView::Board),
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
                        onclick: move |_| view_mode.set(ProjectView::List),
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
                ProjectView::List => rsx! { list_view::ListView { project_id: project_id.clone() } },
                _ => rsx! { board_view::BoardView { project_id: project_id.clone() } },
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
