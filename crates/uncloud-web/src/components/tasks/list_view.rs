use dioxus::prelude::*;
use std::collections::HashSet;
use uncloud_common::{
    CreateTaskRequest, ServerEvent, TaskLabelResponse, TaskPriority, TaskResponse,
    TaskSectionResponse, TaskStatus, UpdateTaskStatusRequest,
};

use crate::hooks::use_drag_cleanup::use_drag_cleanup;
use crate::hooks::use_events::use_events;
use crate::hooks::use_tasks;

use super::label_color_for;
use super::task_detail::TaskDetail;
use super::{task_matches_label_filter, LabelFilterBar};

// ── Helpers ──

fn priority_dot_class(priority: &TaskPriority) -> &'static str {
    match priority {
        TaskPriority::High => "w-2 h-2 rounded-full bg-error shrink-0",
        TaskPriority::Medium => "w-2 h-2 rounded-full bg-warning shrink-0",
        TaskPriority::Low => "w-2 h-2 rounded-full bg-info shrink-0",
    }
}

fn status_badge_class(status: &TaskStatus) -> &'static str {
    match status {
        TaskStatus::Todo => "badge badge-ghost badge-sm cursor-pointer select-none",
        TaskStatus::InProgress => "badge badge-info badge-sm cursor-pointer select-none",
        TaskStatus::Blocked => "badge badge-warning badge-sm cursor-pointer select-none",
        TaskStatus::Done => "badge badge-success badge-sm cursor-pointer select-none",
        TaskStatus::Cancelled => "badge badge-error badge-sm cursor-pointer select-none",
    }
}

fn status_label(status: &TaskStatus) -> &'static str {
    match status {
        TaskStatus::Todo => "Todo",
        TaskStatus::InProgress => "In Progress",
        TaskStatus::Blocked => "Blocked",
        TaskStatus::Done => "Done",
        TaskStatus::Cancelled => "Cancelled",
    }
}

const STATUS_CYCLE: &[TaskStatus] = &[
    TaskStatus::Todo,
    TaskStatus::InProgress,
    TaskStatus::Blocked,
    TaskStatus::Done,
    TaskStatus::Cancelled,
];

fn next_status(current: &TaskStatus) -> TaskStatus {
    let idx = STATUS_CYCLE
        .iter()
        .position(|s| s == current)
        .unwrap_or(0);
    STATUS_CYCLE[(idx + 1) % STATUS_CYCLE.len()].clone()
}

/// Format a due-date ISO string for display. Returns (label, css_class).
fn due_date_display(due: &str) -> (String, &'static str) {
    let date_part = &due[..due.len().min(10)];

    let now = js_sys::Date::new_0();
    let today = format!(
        "{:04}-{:02}-{:02}",
        now.get_full_year(),
        now.get_month() + 1,
        now.get_date(),
    );

    // Compute tomorrow
    let tomorrow_ms = now.get_time() + 86_400_000.0;
    let tom = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(tomorrow_ms));
    let tomorrow = format!(
        "{:04}-{:02}-{:02}",
        tom.get_full_year(),
        tom.get_month() + 1,
        tom.get_date(),
    );

    if date_part < today.as_str() {
        (date_part.to_string(), "text-xs text-error font-medium")
    } else if date_part == today.as_str() {
        ("Today".to_string(), "text-xs text-warning font-medium")
    } else if date_part == tomorrow.as_str() {
        ("Tomorrow".to_string(), "text-xs text-info font-medium")
    } else {
        let label = format_short_date(date_part);
        (label, "text-xs text-base-content/60")
    }
}

fn format_short_date(iso: &str) -> String {
    let parts: Vec<&str> = iso.split('-').collect();
    if parts.len() < 3 {
        return iso.to_string();
    }
    let month = match parts[1] {
        "01" => "Jan",
        "02" => "Feb",
        "03" => "Mar",
        "04" => "Apr",
        "05" => "May",
        "06" => "Jun",
        "07" => "Jul",
        "08" => "Aug",
        "09" => "Sep",
        "10" => "Oct",
        "11" => "Nov",
        "12" => "Dec",
        _ => parts[1],
    };
    let day = parts[2].trim_start_matches('0');
    format!("{} {}", month, day)
}

// ── Sentinel for the "Unsectioned" group ──

const UNSECTIONED_ID: &str = "__unsectioned__";

// ── Main component ──

#[component]
pub fn ListView(
    project_id: String,
    available_labels: Signal<Vec<TaskLabelResponse>>,
) -> Element {
    let mut sections: Signal<Vec<TaskSectionResponse>> = use_signal(Vec::new);
    let mut tasks: Signal<Vec<TaskResponse>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    // Collapsed section tracking
    let mut collapsed: Signal<HashSet<String>> = use_signal(HashSet::new);

    // Expanded subtask parents
    let expanded_parents: Signal<HashSet<String>> = use_signal(HashSet::new);

    // Add-task input: which section is active, and the draft text
    let mut adding_to_section: Signal<Option<String>> = use_signal(|| None);
    let mut add_title = use_signal(String::new);

    // Task detail slide-over
    let mut detail_task_id: Signal<Option<String>> = use_signal(|| None);
    let mut detail_refresh: Signal<u32> = use_signal(|| 0);

    // Drag state
    let mut drag_task_id: Signal<Option<String>> = use_signal(|| None);
    let mut drop_section_id: Signal<Option<String>> = use_signal(|| None);

    // Task IDs whose due-date label should briefly highlight after the
    // server confirms a status update — the visible signal that
    // "completing" a recurring task did something useful (it advanced the
    // due date). Entries are removed ~1s after insertion.
    let flashing_dates: Signal<HashSet<String>> = use_signal(HashSet::new);

    // Document-level safety net for drags ending outside a section's hit
    // box. Window listeners fire after local handlers have bubbled, so a
    // drop committed by the section's onpointerup still wins; this only
    // clears state if it's still dirty afterwards.
    use_drag_cleanup(move || {
        if drag_task_id.peek().is_some() {
            drag_task_id.set(None);
            drop_section_id.set(None);
        }
    });

    // Label filter (OR semantics — empty = no filter)
    let label_filter: Signal<HashSet<String>> = use_signal(HashSet::new);

    // Section management
    let mut adding_section = use_signal(|| false);
    let mut new_section_name = use_signal(String::new);
    let mut renaming_section_id: Signal<Option<String>> = use_signal(|| None);
    let mut rename_section_draft = use_signal(String::new);

    // Sync prop into a Signal so the fetch effect re-runs when the user
    // navigates to a different project via the sidebar.
    let mut pid_sig = use_signal(|| project_id.clone());
    if *pid_sig.peek() != project_id {
        pid_sig.set(project_id.clone());
    }

    // Live updates: refetch on TaskChanged for the current project. Bumps
    // detail_refresh too when the open task is the one that changed.
    use_events(move |evt| {
        if let ServerEvent::TaskChanged { project_id: ev_pid, task_id } = evt {
            if ev_pid == *pid_sig.peek() {
                let pid = ev_pid.clone();
                spawn(async move {
                    if let Ok(secs) = use_tasks::list_sections(&pid).await {
                        sections.set(secs);
                    }
                    if let Ok(t) = use_tasks::list_all_tasks(&pid).await {
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

    // Initial fetch (re-runs when pid_sig changes)
    use_effect(move || {
        let pid = pid_sig.read().clone();
        spawn(async move {
            loading.set(true);
            error.set(None);

            let (sec_res, tasks_res) = futures::join!(
                use_tasks::list_sections(&pid),
                use_tasks::list_all_tasks(&pid),
            );

            match sec_res {
                Ok(s) => sections.set(s),
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

    let section_list = sections.read().clone();
    let all_tasks = tasks.read().clone();

    // Only top-level tasks (no parent)
    let top_level: Vec<&TaskResponse> = all_tasks
        .iter()
        .filter(|t| t.parent_task_id.is_none())
        .collect();

    let has_unsectioned = top_level.iter().any(|t| t.section_id.is_none());

    // IDs of Done tasks across the whole project — drives the global
    // "Clear completed" button (mirror of the per-column Clear in the
    // board view).
    let done_task_ids: Vec<String> = all_tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Done)
        .map(|t| t.id.clone())
        .collect();
    let done_count = done_task_ids.len();

    rsx! {
        // Label filter strip
        LabelFilterBar {
            available_labels: available_labels.read().clone(),
            selected: label_filter,
        }

        if done_count > 0 {
            div { class: "flex items-center justify-end mb-1",
                button {
                    class: "btn btn-ghost btn-xs text-error/60 hover:text-error",
                    title: "Delete every Done task in this project",
                    onclick: move |_| {
                        let ids = done_task_ids.clone();
                        spawn(async move {
                            for id in &ids {
                                let _ = use_tasks::delete_task(id).await;
                            }
                            tasks.write().retain(|t| !ids.contains(&t.id));
                        });
                    },
                    "Clear completed ({done_count})"
                }
            }
        }

        div { class: "space-y-3",
            // Sections
            for section in section_list.iter() {
                {
                    let sec_id = section.id.clone();
                    let sec_id_toggle = section.id.clone();
                    let sec_id_drop = section.id.clone();
                    let sec_name = section.name.clone();
                    let is_collapsed = collapsed.read().contains(&sec_id);

                    let section_tasks: Vec<TaskResponse> = {
                        let filter = label_filter.read();
                        top_level
                            .iter()
                            .filter(|t| t.section_id.as_ref() == Some(&sec_id))
                            .filter(|t| task_matches_label_filter(&t.labels, &filter))
                            .cloned()
                            .cloned()
                            .collect()
                    };
                    let count = section_tasks.len();

                    let is_drop_target = drop_section_id.read().as_ref() == Some(&sec_id);
                    let has_drag = drag_task_id.read().is_some();

                    let section_class = if is_drop_target && has_drag {
                        "bg-base-200 rounded-box ring-2 ring-primary"
                    } else {
                        "bg-base-200 rounded-box"
                    };

                    rsx! {
                        div {
                            class: "{section_class}",
                            onpointerenter: move |_| {
                                if drag_task_id.read().is_some() {
                                    drop_section_id.set(Some(sec_id_drop.clone()));
                                }
                            },
                            onpointerup: move |_| {
                                let task_id = drag_task_id.peek().clone();
                                let target = drop_section_id.peek().clone();
                                drag_task_id.set(None);
                                drop_section_id.set(None);

                                if let (Some(tid), Some(new_sec)) = (task_id, target) {
                                    let current_sec = tasks.peek().iter().find(|t| t.id == tid).and_then(|t| t.section_id.clone());
                                    if current_sec.as_ref() == Some(&new_sec) {
                                        return;
                                    }
                                    let pid = pid_sig.peek().clone();
                                    spawn(async move {
                                        let req = uncloud_common::UpdateTaskRequest {
                                            section_id: Some(new_sec),
                                            title: None,
                                            description: None,
                                            status: None,
                                            status_note: None,
                                            priority: None,
                                            assignee_id: None,
                                            labels: None,
                                            due_date: None,
                                            recurrence_rule: None,
                                            position: None,
                                        };
                                        if let Ok(updated) = use_tasks::update_task(&tid, &req).await {
                                            let mut tw = tasks.write();
                                            if let Some(t) = tw.iter_mut().find(|t| t.id == updated.id) {
                                                *t = updated;
                                            }
                                        }
                                        // Re-fetch to get accurate ordering
                                        if let Ok(t) = use_tasks::list_tasks(&pid, None, None).await {
                                            tasks.set(t);
                                        }
                                    });
                                }
                            },

                            // Section header
                            div {
                                class: "group flex items-center gap-2 px-4 py-2.5 cursor-pointer select-none",
                                onclick: move |_| {
                                    let mut c = collapsed.write();
                                    if c.contains(&sec_id_toggle) {
                                        c.remove(&sec_id_toggle);
                                    } else {
                                        c.insert(sec_id_toggle.clone());
                                    }
                                },

                                // Collapse arrow
                                svg {
                                    class: if is_collapsed {
                                        "w-4 h-4 shrink-0 transition-transform -rotate-90"
                                    } else {
                                        "w-4 h-4 shrink-0 transition-transform"
                                    },
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
                                // Section name (inline rename on double-click)
                                {
                                    let sec_id_rename = sec_id.clone();
                                    let sec_id_rename2 = sec_id.clone();
                                    let sec_id_del = sec_id.clone();
                                    let is_renaming = renaming_section_id.read().as_ref() == Some(&sec_id);

                                    if is_renaming {
                                        rsx! {
                                            input {
                                                class: "input input-bordered input-xs flex-1",
                                                r#type: "text",
                                                autofocus: true,
                                                value: "{rename_section_draft}",
                                                onclick: move |e| { e.stop_propagation(); },
                                                oninput: move |e| rename_section_draft.set(e.value()),
                                                onkeydown: move |e: KeyboardEvent| {
                                                    if e.key() == Key::Enter {
                                                        let name = rename_section_draft.peek().trim().to_string();
                                                        let sid = sec_id_rename.clone();
                                                        if !name.is_empty() {
                                                            spawn(async move {
                                                                let req = uncloud_common::UpdateTaskSectionRequest { name: Some(name), position: None, collapsed: None };
                                                                if let Ok(updated) = use_tasks::update_section(&sid, &req).await {
                                                                    let mut sw = sections.write();
                                                                    if let Some(s) = sw.iter_mut().find(|s| s.id == updated.id) {
                                                                        *s = updated;
                                                                    }
                                                                }
                                                                renaming_section_id.set(None);
                                                            });
                                                        } else {
                                                            renaming_section_id.set(None);
                                                        }
                                                    } else if e.key() == Key::Escape {
                                                        renaming_section_id.set(None);
                                                    }
                                                },
                                            }
                                        }
                                    } else {
                                        rsx! {
                                            span {
                                                class: "font-semibold text-sm flex-1",
                                                ondoubleclick: move |e| {
                                                    e.stop_propagation();
                                                    rename_section_draft.set(sec_name.clone());
                                                    renaming_section_id.set(Some(sec_id_rename2.clone()));
                                                },
                                                "{sec_name}"
                                            }
                                            // Delete section button
                                            button {
                                                class: "btn btn-ghost btn-xs btn-circle opacity-0 group-hover:opacity-100",
                                                onclick: move |e| {
                                                    e.stop_propagation();
                                                    let sid = sec_id_del.clone();
                                                    spawn(async move {
                                                        let _ = use_tasks::delete_section(&sid).await;
                                                        sections.write().retain(|s| s.id != sid);
                                                    });
                                                },
                                                "×"
                                            }
                                        }
                                    }
                                }
                                span { class: "badge badge-sm badge-ghost", "{count}" }
                            }

                            // Tasks (hidden when collapsed)
                            if !is_collapsed {
                                {
                                    let sec_id_add = sec_id.clone();
                                    let sec_id_submit = sec_id.clone();
                                    let is_adding = adding_to_section.read().as_ref() == Some(&sec_id);

                                    rsx! {
                                        div { class: "px-2 pb-2",
                                            for task in section_tasks.iter() {
                                                {
                                                    let avail_labels = available_labels.read().clone();
                                                    render_task_row(
                                                        task,
                                                        &all_tasks,
                                                        &avail_labels,
                                                        0,
                                                        tasks,
                                                        detail_task_id,
                                                        drag_task_id,
                                                        expanded_parents,
                                                        flashing_dates,
                                                    )
                                                }
                                            }

                                            if section_tasks.is_empty() && !is_adding {
                                                div { class: "text-base-content/40 text-center text-sm py-4",
                                                    "No tasks in this section"
                                                }
                                            }

                                            // Add task input
                                            if is_adding {
                                                div { class: "flex items-center gap-2 px-3 py-1.5 mt-1",
                                                    div { class: "w-5" }
                                                    input {
                                                        class: "input input-bordered input-sm flex-1",
                                                        r#type: "text",
                                                        placeholder: "Task title...",
                                                        autofocus: true,
                                                        value: "{add_title}",
                                                        oninput: move |e| add_title.set(e.value()),
                                                        onkeydown: move |e: KeyboardEvent| {
                                                            if e.key() == Key::Enter {
                                                                let title = add_title.peek().trim().to_string();
                                                                if title.is_empty() {
                                                                    adding_to_section.set(None);
                                                                    return;
                                                                }
                                                                let pid = pid_sig.peek().clone();
                                                                let sid = sec_id_submit.clone();
                                                                spawn(async move {
                                                                    let req = CreateTaskRequest {
                                                                        title,
                                                                        section_id: Some(sid),
                                                                        status: None,
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
                                                                    adding_to_section.set(None);
                                                                    add_title.set(String::new());
                                                                });
                                                            } else if e.key() == Key::Escape {
                                                                adding_to_section.set(None);
                                                                add_title.set(String::new());
                                                            }
                                                        },
                                                    }
                                                }
                                            } else {
                                                button {
                                                    class: "btn btn-ghost btn-xs mt-1 ml-2 text-base-content/50",
                                                    onclick: move |_| {
                                                        adding_to_section.set(Some(sec_id_add.clone()));
                                                        add_title.set(String::new());
                                                    },
                                                    "+ Add task"
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

            // Unsectioned group
            if has_unsectioned {
                {
                    let is_collapsed_unsec = collapsed.read().contains(UNSECTIONED_ID);
                    let unsectioned_tasks: Vec<TaskResponse> = {
                        let filter = label_filter.read();
                        top_level
                            .iter()
                            .filter(|t| t.section_id.is_none())
                            .filter(|t| task_matches_label_filter(&t.labels, &filter))
                            .cloned()
                            .cloned()
                            .collect()
                    };
                    let unsec_count = unsectioned_tasks.len();
                    let is_adding_unsec = adding_to_section.read().as_ref().map(|s| s.as_str()) == Some(UNSECTIONED_ID);

                    rsx! {
                        div { class: "bg-base-200 rounded-box",
                            // Header
                            div {
                                class: "flex items-center gap-2 px-4 py-2.5 cursor-pointer select-none",
                                onclick: move |_| {
                                    let mut c = collapsed.write();
                                    let key = UNSECTIONED_ID.to_string();
                                    if c.contains(&key) {
                                        c.remove(&key);
                                    } else {
                                        c.insert(key);
                                    }
                                },
                                svg {
                                    class: if is_collapsed_unsec {
                                        "w-4 h-4 shrink-0 transition-transform -rotate-90"
                                    } else {
                                        "w-4 h-4 shrink-0 transition-transform"
                                    },
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
                                span { class: "font-semibold text-sm flex-1 text-base-content/60 italic", "Unsectioned" }
                                span { class: "badge badge-sm badge-ghost", "{unsec_count}" }
                            }

                            if !is_collapsed_unsec {
                                div { class: "px-2 pb-2",
                                    for task in unsectioned_tasks.iter() {
                                        {
                                            let avail_labels = available_labels.read().clone();
                                            render_task_row(
                                                task,
                                                &all_tasks,
                                                &avail_labels,
                                                0,
                                                tasks,
                                                detail_task_id,
                                                drag_task_id,
                                                expanded_parents,
                                                        flashing_dates,
                                            )
                                        }
                                    }

                                    if unsectioned_tasks.is_empty() && !is_adding_unsec {
                                        div { class: "text-base-content/40 text-center text-sm py-4",
                                            "No unsectioned tasks"
                                        }
                                    }

                                    if is_adding_unsec {
                                        div { class: "flex items-center gap-2 px-3 py-1.5 mt-1",
                                            div { class: "w-5" }
                                            input {
                                                class: "input input-bordered input-sm flex-1",
                                                r#type: "text",
                                                placeholder: "Task title...",
                                                autofocus: true,
                                                value: "{add_title}",
                                                oninput: move |e| add_title.set(e.value()),
                                                onkeydown: move |e: KeyboardEvent| {
                                                    if e.key() == Key::Enter {
                                                        let title = add_title.peek().trim().to_string();
                                                        if title.is_empty() {
                                                            adding_to_section.set(None);
                                                            return;
                                                        }
                                                        let pid = pid_sig.peek().clone();
                                                        spawn(async move {
                                                            let req = CreateTaskRequest {
                                                                title,
                                                                section_id: None,
                                                                status: None,
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
                                                            adding_to_section.set(None);
                                                            add_title.set(String::new());
                                                        });
                                                    } else if e.key() == Key::Escape {
                                                        adding_to_section.set(None);
                                                        add_title.set(String::new());
                                                    }
                                                },
                                            }
                                        }
                                    } else {
                                        button {
                                            class: "btn btn-ghost btn-xs mt-1 ml-2 text-base-content/50",
                                            onclick: move |_| {
                                                adding_to_section.set(Some(UNSECTIONED_ID.to_string()));
                                                add_title.set(String::new());
                                            },
                                            "+ Add task"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Add section button/input
        if *adding_section.read() {
            div { class: "flex items-center gap-2 px-4 py-2",
                input {
                    class: "input input-bordered input-sm",
                    r#type: "text",
                    autofocus: true,
                    placeholder: "Section name...",
                    value: "{new_section_name}",
                    oninput: move |e| new_section_name.set(e.value()),
                    onkeydown: move |e: KeyboardEvent| {
                        if e.key() == Key::Enter {
                            let name = new_section_name.peek().trim().to_string();
                            if !name.is_empty() {
                                let pid = pid_sig.peek().clone();
                                spawn(async move {
                                    let req = uncloud_common::CreateTaskSectionRequest {
                                        name,
                                        position: None,
                                    };
                                    if let Ok(sec) = use_tasks::create_section(&pid, &req).await {
                                        sections.write().push(sec);
                                    }
                                    adding_section.set(false);
                                    new_section_name.set(String::new());
                                });
                            } else {
                                adding_section.set(false);
                            }
                        } else if e.key() == Key::Escape {
                            adding_section.set(false);
                            new_section_name.set(String::new());
                        }
                    },
                }
            }
        } else {
            button {
                class: "btn btn-ghost btn-sm text-base-content/50",
                onclick: move |_| {
                    adding_section.set(true);
                    new_section_name.set(String::new());
                },
                "+ New section"
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
                        if let Ok(t) = use_tasks::list_all_tasks(&pid).await {
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

// ── Task row renderer (recursive for subtasks) ──

fn render_task_row(
    task: &TaskResponse,
    all_tasks: &[TaskResponse],
    available_labels: &[TaskLabelResponse],
    depth: usize,
    mut tasks: Signal<Vec<TaskResponse>>,
    mut detail_task_id: Signal<Option<String>>,
    mut drag_task_id: Signal<Option<String>>,
    mut expanded_parents: Signal<HashSet<String>>,
    mut flashing_dates: Signal<HashSet<String>>,
) -> Element {
    let task_id = task.id.clone();
    let task_id_check = task.id.clone();
    let task_id_click = task.id.clone();
    let task_id_drag = task.id.clone();
    let task_id_status = task.id.clone();
    let task_id_expand = task.id.clone();
    let parent_id_check = task.parent_task_id.clone();
    let parent_id_status = task.parent_task_id.clone();

    let is_done = task.status == TaskStatus::Done || task.status == TaskStatus::Cancelled;
    let has_subtasks = task.subtask_count > 0;
    let is_expanded = expanded_parents.read().contains(&task_id);

    let indent_class = match depth {
        0 => "pl-3",
        1 => "pl-11",
        _ => "pl-19",
    };

    let is_dragging = drag_task_id.read().as_ref() == Some(&task_id);
    let row_class = if is_dragging {
        format!(
            "{} flex items-center gap-2 pr-3 py-1.5 rounded-lg opacity-30 group select-none",
            indent_class
        )
    } else {
        format!(
            "{} flex items-center gap-2 pr-3 py-1.5 hover:bg-base-300 rounded-lg group",
            indent_class
        )
    };

    // Collect subtasks for this parent
    let subtask_list: Vec<TaskResponse> = if has_subtasks && is_expanded {
        all_tasks
            .iter()
            .filter(|t| t.parent_task_id.as_ref() == Some(&task_id))
            .cloned()
            .collect()
    } else {
        Vec::new()
    };

    let status = task.status.clone();
    let status_click = task.status.clone();
    let priority = task.priority.clone();
    let title = task.title.clone();
    let labels = task.labels.clone();
    let due_date = task.due_date.clone();
    let assignee_username = task.assignee_username.clone();
    let subtask_count = task.subtask_count;
    let subtask_done_count = task.subtask_done_count;

    rsx! {
        div {
            key: "{task_id}",

            // Main row
            div {
                class: "{row_class}",

                // Drag handle (only for top-level tasks)
                if depth == 0 {
                    span {
                        class: "cursor-grab active:cursor-grabbing opacity-30 hover:opacity-70 shrink-0",
                        style: "touch-action: none;",
                        onpointerdown: move |e| {
                            e.stop_propagation();
                            e.prevent_default();
                            drag_task_id.set(Some(task_id_drag.clone()));
                        },
                        "⠿"
                    }
                }

                // Subtask expand/collapse toggle (or spacer)
                if has_subtasks {
                    button {
                        class: "btn btn-ghost btn-xs btn-circle w-5 h-5 min-h-0 p-0",
                        onclick: move |e| {
                            e.stop_propagation();
                            let mut ep = expanded_parents.write();
                            if ep.contains(&task_id_expand) {
                                ep.remove(&task_id_expand);
                            } else {
                                ep.insert(task_id_expand.clone());
                            }
                        },
                        svg {
                            class: if is_expanded {
                                "w-3 h-3 transition-transform"
                            } else {
                                "w-3 h-3 transition-transform -rotate-90"
                            },
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
                    }
                } else {
                    div { class: "w-5 shrink-0" }
                }

                // Checkbox. Driven entirely from the `checked` prop — we
                // `prevent_default()` in onclick so the browser doesn't
                // optimistically toggle the visual state. Otherwise a
                // recurring completion (which flips the task back to Todo
                // server-side) would leave the checkbox stuck checked
                // because the post-update prop value (`is_done = false`)
                // matches the pre-update value, so Dioxus has no DOM diff
                // to apply and the browser-toggled state lingers.
                input {
                    class: "checkbox checkbox-sm",
                    r#type: "checkbox",
                    checked: is_done,
                    onclick: move |e| {
                        e.stop_propagation();
                        e.prevent_default();
                        let tid = task_id_check.clone();
                        let pid = parent_id_check.clone();
                        let new_status = if is_done { TaskStatus::Todo } else { TaskStatus::Done };
                        spawn(async move {
                            let req = UpdateTaskStatusRequest {
                                status: new_status,
                                status_note: None,
                            };
                            // Recurring completion can rewrite *several* docs
                            // server-side (the task itself flips date, every
                            // subtask resets to Todo). Patching the row
                            // locally would only catch the parent — refetch
                            // the whole list so the subtree updates with it.
                            // The SSE TaskChanged refetch normally covers
                            // this, but the explicit fetch removes any race.
                            if let Ok(updated) = use_tasks::update_task_status(&tid, &req).await {
                                let updated_id = updated.id.clone();
                                if let Ok(all) =
                                    use_tasks::list_all_tasks(&updated.project_id).await
                                {
                                    tasks.set(all);
                                }
                                // Briefly highlight the updated task's date
                                // label so a recurring completion (whose
                                // only visible change is `due_date`) is
                                // unmissable.
                                flashing_dates.write().insert(updated_id.clone());
                                spawn(async move {
                                    gloo_timers::future::TimeoutFuture::new(1200).await;
                                    flashing_dates.write().remove(&updated_id);
                                });
                            }
                            // Re-fetch parent to update subtask counters
                            if let Some(parent_id) = pid {
                                if let Ok(parent) = use_tasks::get_task(&parent_id).await {
                                    let mut tw = tasks.write();
                                    if let Some(t) = tw.iter_mut().find(|t| t.id == parent.id) {
                                        *t = parent;
                                    }
                                }
                            }
                        });
                    },
                }

                // Priority dot
                span { class: priority_dot_class(&priority) }

                // Title (click to open detail)
                span {
                    class: if is_done {
                        "text-sm flex-1 truncate cursor-pointer hover:text-primary line-through text-base-content/50"
                    } else {
                        "text-sm flex-1 truncate cursor-pointer hover:text-primary"
                    },
                    onclick: move |_| {
                        detail_task_id.set(Some(task_id_click.clone()));
                    },
                    "{title}"
                }

                // Subtask progress (compact)
                if subtask_count > 0 {
                    span { class: "text-xs text-base-content/50 shrink-0",
                        "{subtask_done_count}/{subtask_count}"
                    }
                }

                // Labels (show first two as tiny coloured chips, "+N" overflow chip beyond)
                for label in labels.iter().take(2) {
                    {
                        let color = label_color_for(available_labels, label).to_string();
                        rsx! {
                            span {
                                key: "{label}",
                                class: "px-1.5 py-0.5 rounded text-[10px] font-medium text-white hidden sm:inline-flex shrink-0",
                                style: "background: {color};",
                                "{label}"
                            }
                        }
                    }
                }
                if labels.len() > 2 {
                    span {
                        class: "text-[10px] text-base-content/50 hidden sm:inline-flex shrink-0",
                        title: "{labels.iter().skip(2).cloned().collect::<Vec<_>>().join(\", \")}",
                        "+{labels.len() - 2}"
                    }
                }

                // Due date. Briefly highlights when this task's status was
                // just toggled — without it a recurring completion looks
                // like a no-op (status unchanged, only `due_date` moved).
                if let Some(ref due) = due_date {
                    {
                        let (label, class) = due_date_display(due);
                        let is_flashing = flashing_dates.read().contains(&task_id);
                        let span_class = if is_flashing {
                            format!("{class} shrink-0 hidden sm:inline bg-success/30 rounded px-1 transition-colors duration-700")
                        } else {
                            format!("{class} shrink-0 hidden sm:inline transition-colors duration-700")
                        };
                        rsx! {
                            span { class: "{span_class}", "{label}" }
                        }
                    }
                }

                // Status chip (click to cycle)
                span {
                    class: status_badge_class(&status),
                    onclick: move |e| {
                        e.stop_propagation();
                        let tid = task_id_status.clone();
                        let pid = parent_id_status.clone();
                        let new_s = next_status(&status_click);
                        spawn(async move {
                            let req = UpdateTaskStatusRequest {
                                status: new_s,
                                status_note: None,
                            };
                            if let Ok(updated) = use_tasks::update_task_status(&tid, &req).await {
                                let mut tw = tasks.write();
                                if let Some(t) = tw.iter_mut().find(|t| t.id == updated.id) {
                                    *t = updated;
                                }
                            }
                            // Re-fetch parent to update subtask counters
                            if let Some(parent_id) = pid {
                                if let Ok(parent) = use_tasks::get_task(&parent_id).await {
                                    let mut tw = tasks.write();
                                    if let Some(t) = tw.iter_mut().find(|t| t.id == parent.id) {
                                        *t = parent;
                                    }
                                }
                            }
                        });
                    },
                    "{status_label(&status)}"
                }

                // Assignee avatar
                if let Some(ref username) = assignee_username {
                    div {
                        class: "avatar placeholder shrink-0",
                        div { class: "bg-neutral text-neutral-content w-5 h-5 rounded-full",
                            span { class: "text-[10px]",
                                {username.chars().next().unwrap_or('?').to_uppercase().to_string()}
                            }
                        }
                    }
                }
            }

            // Subtask rows (indented)
            if is_expanded {
                for sub in subtask_list.iter() {
                    {
                        render_task_row(
                            sub,
                            all_tasks,
                            available_labels,
                            depth + 1,
                            tasks,
                            detail_task_id,
                            drag_task_id,
                            expanded_parents,
                                                        flashing_dates,
                        )
                    }
                }
            }
        }
    }
}
