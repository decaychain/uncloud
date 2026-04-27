use dioxus::prelude::*;
use uncloud_common::{
    CreateTaskLabelRequest, CreateTaskRequest, NthWeek, RecurrenceRule, TaskCommentResponse,
    TaskLabelResponse, TaskPriority, TaskResponse, TaskStatus, UpdateTaskRequest,
    UpdateTaskStatusRequest,
};

use crate::hooks::use_tasks;
use super::{LABEL_PALETTE, label_color_for};

fn format_recurrence(rule: &RecurrenceRule) -> String {
    match rule {
        RecurrenceRule::Daily => "Every day".to_string(),
        RecurrenceRule::Weekly { days } => {
            let day_names = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
            let selected: Vec<&str> = days.iter().filter_map(|d| day_names.get(*d as usize).copied()).collect();
            if selected.len() == 7 {
                "Every day".to_string()
            } else if selected.is_empty() {
                "Weekly".to_string()
            } else {
                format!("Every {}", selected.join(", "))
            }
        }
        RecurrenceRule::Monthly { day_of_month } => format!("Monthly on the {}{}", day_of_month, ordinal_suffix(*day_of_month)),
        RecurrenceRule::MonthlyByWeekday { nth, weekday } => {
            let day_names = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
            let nth_label = match nth {
                NthWeek::First => "first",
                NthWeek::Second => "second",
                NthWeek::Third => "third",
                NthWeek::Fourth => "fourth",
                NthWeek::Last => "last",
            };
            let day = day_names.get(*weekday as usize).copied().unwrap_or("???");
            format!("Monthly on the {} {}", nth_label, day)
        }
        RecurrenceRule::Yearly { month, day } => {
            let month_names = ["Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec"];
            let m = month_names.get((*month as usize).saturating_sub(1)).unwrap_or(&"???");
            format!("Every {} {}", m, day)
        }
        RecurrenceRule::Custom { interval_days } => {
            if *interval_days == 1 { "Every day".to_string() }
            else { format!("Every {} days", interval_days) }
        }
    }
}

fn ordinal_suffix(n: u8) -> &'static str {
    match (n % 10, n % 100) {
        (1, 11) => "th",
        (1, _) => "st",
        (2, 12) => "th",
        (2, _) => "nd",
        (3, 13) => "th",
        (3, _) => "rd",
        _ => "th",
    }
}

/// Helper to build an UpdateTaskRequest with a single field set.
fn update_req() -> UpdateTaskRequest {
    UpdateTaskRequest {
        section_id: None,
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
    }
}

#[component]
pub fn TaskDetail(
    task_id: String,
    #[props(default = 0)] refresh_key: u32,
    /// Project's label catalogue, lifted from the parent so edits propagate to
    /// sibling views (board cards, list rows). Parents that don't share a
    /// catalogue (e.g. ScheduleView) can pass a fresh `use_signal(Vec::new)` —
    /// TaskDetail populates it on task load.
    available_labels: Signal<Vec<TaskLabelResponse>>,
    on_close: EventHandler<()>,
    on_updated: EventHandler<()>,
    #[props(default)] on_deleted: EventHandler<String>,
) -> Element {
    let mut task: Signal<Option<TaskResponse>> = use_signal(|| None);
    let mut comments: Signal<Vec<TaskCommentResponse>> = use_signal(Vec::new);
    let mut subtasks: Signal<Vec<TaskResponse>> = use_signal(Vec::new);
    let mut available_labels = available_labels;
    let mut label_picker_open = use_signal(|| false);
    let mut label_picker_filter = use_signal(String::new);
    let mut label_picker_color = use_signal(|| LABEL_PALETTE[5].to_string());
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    // Editing states
    let mut editing_title = use_signal(|| false);
    let mut title_draft = use_signal(String::new);
    let mut desc_draft = use_signal(String::new);
    let mut editing_desc = use_signal(|| false);
    let mut new_comment = use_signal(String::new);
    let mut new_subtask_title = use_signal(String::new);
    let mut adding_subtask = use_signal(|| false);

    // Recurrence editing
    let mut editing_recurrence = use_signal(|| false);
    let mut rec_type = use_signal(|| "none".to_string());
    let mut rec_weekly_days: Signal<Vec<u8>> = use_signal(Vec::new);
    let mut rec_monthly_day = use_signal(|| 1u8);
    let mut rec_monthly_nth = use_signal(|| NthWeek::First);
    let mut rec_monthly_weekday = use_signal(|| 5u8); // Saturday
    let mut rec_yearly_month = use_signal(|| 1u8);
    let mut rec_yearly_day = use_signal(|| 1u8);
    let mut rec_custom_days = use_signal(|| 7u32);

    // Status note editing
    let mut status_note_val = use_signal(String::new);

    // Delete confirmation
    let mut confirm_delete = use_signal(|| false);

    // Track refresh_key in a signal so the effect re-runs when it changes
    let mut refresh_sig = use_signal(|| refresh_key);
    if *refresh_sig.peek() != refresh_key {
        refresh_sig.set(refresh_key);
    }

    // Sync task_id prop into a signal so the fetch effect re-runs if the parent
    // swaps tasks without remounting (defensive — most parents do remount).
    let mut tid_sig = use_signal(|| task_id.clone());
    if *tid_sig.peek() != task_id {
        tid_sig.set(task_id.clone());
    }

    // Fetch task + comments
    use_effect(move || {
        let _key = *refresh_sig.read(); // subscribe to refresh_key changes
        let tid = tid_sig.read().clone();
        spawn(async move {
            // Only show the loading spinner for the *initial* fetch (when we
            // don't have a task yet). Background refreshes triggered by
            // refresh_key bumps (e.g. SSE TaskChanged from another device or
            // tab) keep the form mounted so user-edited fields like the
            // Status / Priority selects don't lose their value to a brief
            // unmount-remount cycle that resets the select to its first
            // option.
            let initial_load = task.peek().is_none();
            if initial_load {
                loading.set(true);
            }
            error.set(None);

            let (task_res, comments_res) = futures::join!(
                use_tasks::get_task(&tid),
                use_tasks::list_comments(&tid),
            );

            match task_res {
                Ok(t) => {
                    title_draft.set(t.title.clone());
                    desc_draft.set(t.description.clone().unwrap_or_default());
                    if t.subtask_count > 0 {
                        if let Ok(subs) = use_tasks::list_subtasks(&t.project_id, &t.id).await {
                            subtasks.set(subs);
                        }
                    } else {
                        subtasks.set(Vec::new());
                    }
                    // Always fetch the task's project labels so a detail opened
                    // from ScheduleView (which spans projects) shows the right
                    // catalogue. When opened from BoardView/ListView the parent
                    // has populated the same signal already; the re-fetch is
                    // redundant but harmless.
                    if let Ok(ls) = use_tasks::list_labels(&t.project_id).await {
                        available_labels.set(ls);
                    }
                    task.set(Some(t));
                }
                Err(e) => error.set(Some(e)),
            }
            if let Ok(c) = comments_res {
                comments.set(c);
            }

            if initial_load {
                loading.set(false);
            }
        });
    });

    // Shared save-title logic extracted into a plain fn that takes signals
    let do_save_title = {
        let task_id = task_id.clone();
        move || {
            let tid = task_id.clone();
            let new_title = title_draft.peek().clone();
            if new_title.trim().is_empty() {
                editing_title.set(false);
                return;
            }
            editing_title.set(false);
            spawn(async move {
                let mut req = update_req();
                req.title = Some(new_title);
                if let Ok(updated) = use_tasks::update_task(&tid, &req).await {
                    task.set(Some(updated));
                    on_updated.call(());
                }
            });
        }
    };

    // Shared save-description logic
    let do_save_desc = {
        let task_id = task_id.clone();
        move || {
            let tid = task_id.clone();
            let new_desc = desc_draft.peek().clone();
            editing_desc.set(false);
            spawn(async move {
                let mut req = update_req();
                req.description = Some(new_desc);
                if let Ok(updated) = use_tasks::update_task(&tid, &req).await {
                    task.set(Some(updated));
                    on_updated.call(());
                }
            });
        }
    };

    // Shared post-comment logic
    let do_post_comment = {
        let task_id = task_id.clone();
        move || {
            let body = new_comment.peek().trim().to_string();
            if body.is_empty() {
                return;
            }
            let tid = task_id.clone();
            new_comment.set(String::new());
            spawn(async move {
                if let Ok(c) = use_tasks::create_comment(&tid, &body).await {
                    comments.write().push(c);
                }
            });
        }
    };

    if *loading.read() {
        return rsx! {
            div { class: "fixed inset-0 bg-black/30 z-40" }
            div { class: "fixed top-0 right-0 h-full w-[28rem] max-w-full bg-base-100 shadow-xl z-50 flex items-center justify-center",
                style: "padding-top: env(safe-area-inset-top); padding-bottom: env(safe-area-inset-bottom)",
                span { class: "loading loading-spinner loading-lg" }
            }
        };
    }

    if let Some(err) = error.read().as_ref() {
        return rsx! {
            div { class: "fixed inset-0 bg-black/30 z-40",
                onclick: move |_| on_close.call(()),
            }
            div { class: "fixed top-0 right-0 h-full w-[28rem] max-w-full bg-base-100 shadow-xl z-50 px-6",
                style: "padding-top: calc(1.5rem + env(safe-area-inset-top)); padding-bottom: calc(1.5rem + env(safe-area-inset-bottom))",
                div { class: "alert alert-error", "{err}" }
            }
        };
    }

    let t = match task.read().as_ref() {
        Some(t) => t.clone(),
        None => return rsx! {},
    };

    let current_status_str = match &t.status {
        TaskStatus::Todo => "todo",
        TaskStatus::InProgress => "in_progress",
        TaskStatus::Blocked => "blocked",
        TaskStatus::Done => "done",
        TaskStatus::Cancelled => "cancelled",
    };

    let current_priority_str = match &t.priority {
        TaskPriority::High => "high",
        TaskPriority::Medium => "medium",
        TaskPriority::Low => "low",
    };

    let status_options: [(TaskStatus, &str); 5] = [
        (TaskStatus::Todo, "Backlog"),
        (TaskStatus::InProgress, "In Progress"),
        (TaskStatus::Blocked, "Blocked"),
        (TaskStatus::Done, "Done"),
        (TaskStatus::Cancelled, "Cancelled"),
    ];

    let priority_options: [(TaskPriority, &str); 3] = [
        (TaskPriority::High, "High"),
        (TaskPriority::Medium, "Medium"),
        (TaskPriority::Low, "Low"),
    ];

    // Pre-clone for closures that need separate copies
    let (mut save_title_a, mut save_title_b) = (do_save_title.clone(), do_save_title.clone());
    let (mut save_desc_a, mut _save_desc_b) = (do_save_desc.clone(), do_save_desc.clone());
    let (mut post_comment_a, mut post_comment_b) = (do_post_comment.clone(), do_post_comment.clone());

    let tid_status = task_id.clone();
    let tid_priority = task_id.clone();
    let tid_due = task_id.clone();
    let tid_subtask = task_id.clone();
    let tid_labels = task_id.clone();
    let tid_label_create = task_id.clone();

    rsx! {
        // Backdrop
        div {
            class: "fixed inset-0 bg-black/30 z-40",
            onclick: move |_| on_close.call(()),
        }

        // Panel — extra bottom padding so the last item clears the Android nav bar.
        div {
            class: "fixed top-0 right-0 h-full w-[28rem] max-w-full bg-base-100 shadow-xl z-50 flex flex-col overflow-y-auto",
            style: "padding-bottom: env(safe-area-inset-bottom)",

            // Header — extra top padding so the title clears the Android status bar.
            div {
                class: "flex items-start justify-between px-4 pb-4 border-b border-base-300",
                style: "padding-top: calc(1rem + env(safe-area-inset-top))",
                div { class: "flex-1 mr-2",
                    if *editing_title.read() {
                        input {
                            class: "input input-bordered input-sm w-full text-lg font-bold",
                            value: "{title_draft}",
                            autofocus: true,
                            oninput: move |e| title_draft.set(e.value()),
                            onkeydown: move |e: KeyboardEvent| {
                                if e.key() == Key::Enter {
                                    save_title_a();
                                } else if e.key() == Key::Escape {
                                    editing_title.set(false);
                                    if let Some(t) = task.read().as_ref() {
                                        title_draft.set(t.title.clone());
                                    }
                                }
                            },
                            onblur: move |_| save_title_b(),
                        }
                    } else {
                        h2 {
                            class: "text-lg font-bold cursor-pointer hover:text-primary",
                            onclick: move |_| editing_title.set(true),
                            "{t.title}"
                        }
                    }
                }
                button {
                    class: "btn btn-ghost btn-sm btn-circle",
                    onclick: move |_| on_close.call(()),
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
                        path { d: "M18 6 6 18" }
                        path { d: "m6 6 12 12" }
                    }
                }
            }

            // Body
            div { class: "p-4 flex flex-col gap-4",

                // Status + Priority row
                div { class: "grid grid-cols-2 gap-3",
                    div {
                        label { class: "label", span { class: "label-text text-xs font-semibold uppercase", "Status" } }
                        select {
                            class: "select select-bordered select-sm w-full",
                            onchange: move |e| {
                                let val = e.value();
                                let s = match val.as_str() {
                                    "todo" => TaskStatus::Todo,
                                    "in_progress" => TaskStatus::InProgress,
                                    "blocked" => TaskStatus::Blocked,
                                    "done" => TaskStatus::Done,
                                    "cancelled" => TaskStatus::Cancelled,
                                    _ => return,
                                };
                                let tid = tid_status.clone();
                                spawn(async move {
                                    let req = UpdateTaskStatusRequest { status: s, status_note: None };
                                    if let Ok(updated) = use_tasks::update_task_status(&tid, &req).await {
                                        title_draft.set(updated.title.clone());
                                        desc_draft.set(updated.description.clone().unwrap_or_default());
                                        task.set(Some(updated));
                                        on_updated.call(());
                                    }
                                });
                            },
                            for (status, slabel) in status_options.iter() {
                                {
                                    let val = match status {
                                        TaskStatus::Todo => "todo",
                                        TaskStatus::InProgress => "in_progress",
                                        TaskStatus::Blocked => "blocked",
                                        TaskStatus::Done => "done",
                                        TaskStatus::Cancelled => "cancelled",
                                    };
                                    let is_selected = val == current_status_str;
                                    rsx! {
                                        option {
                                            value: "{val}",
                                            selected: is_selected,
                                            "{slabel}"
                                        }
                                    }
                                }
                            }
                        }
                    }

                    div {
                        label { class: "label", span { class: "label-text text-xs font-semibold uppercase", "Priority" } }
                        select {
                            class: "select select-bordered select-sm w-full",
                            onchange: move |e| {
                                let val = e.value();
                                let p = match val.as_str() {
                                    "high" => TaskPriority::High,
                                    "medium" => TaskPriority::Medium,
                                    "low" => TaskPriority::Low,
                                    _ => return,
                                };
                                let tid = tid_priority.clone();
                                spawn(async move {
                                    let mut req = update_req();
                                    req.priority = Some(p);
                                    if let Ok(updated) = use_tasks::update_task(&tid, &req).await {
                                        task.set(Some(updated));
                                        on_updated.call(());
                                    }
                                });
                            },
                            for (priority, plabel) in priority_options.iter() {
                                {
                                    let val = match priority {
                                        TaskPriority::High => "high",
                                        TaskPriority::Medium => "medium",
                                        TaskPriority::Low => "low",
                                    };
                                    let is_selected = val == current_priority_str;
                                    rsx! {
                                        option {
                                            value: "{val}",
                                            selected: is_selected,
                                            "{plabel}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Status note
                {
                    let tid_note = task_id.clone();
                    let current_note = t.status_note.clone().unwrap_or_default();
                    rsx! {
                        div {
                            label { class: "label", span { class: "label-text text-xs font-semibold uppercase", "Status Note" } }
                            input {
                                class: "input input-bordered input-sm w-full",
                                r#type: "text",
                                placeholder: "e.g. waiting for delivery...",
                                value: "{current_note}",
                                oninput: move |e| {
                                    status_note_val.set(e.value());
                                },
                                onblur: move |_| {
                                    let note = status_note_val.peek().clone();
                                    let tid = tid_note.clone();
                                    spawn(async move {
                                        let mut req = update_req();
                                        req.status_note = Some(note);
                                        if let Ok(updated) = use_tasks::update_task(&tid, &req).await {
                                            task.set(Some(updated));
                                            on_updated.call(());
                                        }
                                    });
                                },
                            }
                        }
                    }
                }

                // Due date
                div {
                    label { class: "label", span { class: "label-text text-xs font-semibold uppercase", "Due Date" } }
                    input {
                        class: "input input-bordered input-sm w-full",
                        r#type: "date",
                        value: "{t.due_date.as_deref().unwrap_or(\"\").get(..10).unwrap_or(\"\")}",
                        onchange: move |e| {
                            let date = e.value();
                            let tid = tid_due.clone();
                            spawn(async move {
                                let mut req = update_req();
                                req.due_date = Some(date);
                                if let Ok(updated) = use_tasks::update_task(&tid, &req).await {
                                    task.set(Some(updated));
                                    on_updated.call(());
                                }
                            });
                        },
                    }
                }

                // Recurrence
                div {
                    label { class: "label", span { class: "label-text text-xs font-semibold uppercase", "Repeat" } }
                    if *editing_recurrence.read() {
                        {
                            let tid_rec = task_id.clone();
                            rsx! {
                                div { class: "flex flex-col gap-2 p-3 bg-base-200 rounded-box",
                                    select {
                                        class: "select select-bordered select-sm w-full",
                                        value: "{rec_type}",
                                        onchange: move |e| rec_type.set(e.value()),
                                        option { value: "none", "None" }
                                        option { value: "daily", "Daily" }
                                        option { value: "weekly", "Weekly" }
                                        option { value: "monthly", "Monthly (by date)" }
                                        option { value: "monthly_by_weekday", "Monthly (by weekday)" }
                                        option { value: "yearly", "Yearly" }
                                        option { value: "custom", "Custom interval" }
                                    }

                                    // Weekly day picker
                                    if rec_type() == "weekly" {
                                        div { class: "flex gap-1 flex-wrap",
                                            {
                                                let days_labels = [("Mon",0u8),("Tue",1),("Wed",2),("Thu",3),("Fri",4),("Sat",5),("Sun",6)];
                                                rsx! {
                                                    for (label, num) in days_labels {
                                                        {
                                                            let selected = rec_weekly_days().contains(&num);
                                                            rsx! {
                                                                button {
                                                                    class: if selected { "btn btn-xs btn-primary" } else { "btn btn-xs btn-outline" },
                                                                    onclick: move |_| {
                                                                        let mut d = rec_weekly_days.write();
                                                                        if d.contains(&num) { d.retain(|&x| x != num); }
                                                                        else { d.push(num); d.sort(); }
                                                                    },
                                                                    "{label}"
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    // Monthly day picker
                                    if rec_type() == "monthly" {
                                        div { class: "flex items-center gap-2",
                                            span { class: "text-sm", "Day of month:" }
                                            input {
                                                class: "input input-bordered input-sm w-20",
                                                r#type: "number",
                                                min: "1",
                                                max: "31",
                                                value: "{rec_monthly_day}",
                                                onchange: move |e| {
                                                    if let Ok(v) = e.value().parse::<u8>() {
                                                        rec_monthly_day.set(v.clamp(1, 31));
                                                    }
                                                },
                                            }
                                        }
                                    }

                                    // Monthly by-weekday picker (e.g. "first Saturday")
                                    if rec_type() == "monthly_by_weekday" {
                                        div { class: "flex items-center gap-2 flex-wrap",
                                            span { class: "text-sm", "Every" }
                                            select {
                                                class: "select select-bordered select-sm",
                                                onchange: move |e| {
                                                    let v = match e.value().as_str() {
                                                        "first" => NthWeek::First,
                                                        "second" => NthWeek::Second,
                                                        "third" => NthWeek::Third,
                                                        "fourth" => NthWeek::Fourth,
                                                        "last" => NthWeek::Last,
                                                        _ => NthWeek::First,
                                                    };
                                                    rec_monthly_nth.set(v);
                                                },
                                                {
                                                    let cur = *rec_monthly_nth.read();
                                                    rsx! {
                                                        option { value: "first",  selected: matches!(cur, NthWeek::First),  "first" }
                                                        option { value: "second", selected: matches!(cur, NthWeek::Second), "second" }
                                                        option { value: "third",  selected: matches!(cur, NthWeek::Third),  "third" }
                                                        option { value: "fourth", selected: matches!(cur, NthWeek::Fourth), "fourth" }
                                                        option { value: "last",   selected: matches!(cur, NthWeek::Last),   "last" }
                                                    }
                                                }
                                            }
                                            select {
                                                class: "select select-bordered select-sm",
                                                onchange: move |e| {
                                                    if let Ok(v) = e.value().parse::<u8>() {
                                                        rec_monthly_weekday.set(v.min(6));
                                                    }
                                                },
                                                {
                                                    let cur = *rec_monthly_weekday.read();
                                                    rsx! {
                                                        option { value: "0", selected: cur == 0, "Monday" }
                                                        option { value: "1", selected: cur == 1, "Tuesday" }
                                                        option { value: "2", selected: cur == 2, "Wednesday" }
                                                        option { value: "3", selected: cur == 3, "Thursday" }
                                                        option { value: "4", selected: cur == 4, "Friday" }
                                                        option { value: "5", selected: cur == 5, "Saturday" }
                                                        option { value: "6", selected: cur == 6, "Sunday" }
                                                    }
                                                }
                                            }
                                            span { class: "text-sm", "of the month" }
                                        }
                                    }

                                    // Yearly picker
                                    if rec_type() == "yearly" {
                                        div { class: "flex items-center gap-2",
                                            select {
                                                class: "select select-bordered select-sm",
                                                value: "{rec_yearly_month}",
                                                onchange: move |e| {
                                                    if let Ok(v) = e.value().parse::<u8>() {
                                                        rec_yearly_month.set(v);
                                                    }
                                                },
                                                option { value: "1", "Jan" }
                                                option { value: "2", "Feb" }
                                                option { value: "3", "Mar" }
                                                option { value: "4", "Apr" }
                                                option { value: "5", "May" }
                                                option { value: "6", "Jun" }
                                                option { value: "7", "Jul" }
                                                option { value: "8", "Aug" }
                                                option { value: "9", "Sep" }
                                                option { value: "10", "Oct" }
                                                option { value: "11", "Nov" }
                                                option { value: "12", "Dec" }
                                            }
                                            input {
                                                class: "input input-bordered input-sm w-16",
                                                r#type: "number",
                                                min: "1",
                                                max: "31",
                                                value: "{rec_yearly_day}",
                                                onchange: move |e| {
                                                    if let Ok(v) = e.value().parse::<u8>() {
                                                        rec_yearly_day.set(v.clamp(1, 31));
                                                    }
                                                },
                                            }
                                        }
                                    }

                                    // Custom interval
                                    if rec_type() == "custom" {
                                        div { class: "flex items-center gap-2",
                                            span { class: "text-sm", "Every" }
                                            input {
                                                class: "input input-bordered input-sm w-20",
                                                r#type: "number",
                                                min: "1",
                                                value: "{rec_custom_days}",
                                                onchange: move |e| {
                                                    if let Ok(v) = e.value().parse::<u32>() {
                                                        rec_custom_days.set(v.max(1));
                                                    }
                                                },
                                            }
                                            span { class: "text-sm", "days" }
                                        }
                                    }

                                    div { class: "flex gap-2 mt-2",
                                        button {
                                            class: "btn btn-primary btn-sm",
                                            onclick: move |_| {
                                                let rule = match rec_type().as_str() {
                                                    "daily" => Some(RecurrenceRule::Daily),
                                                    "weekly" => {
                                                        let days = rec_weekly_days();
                                                        if days.is_empty() { Some(RecurrenceRule::Daily) }
                                                        else { Some(RecurrenceRule::Weekly { days }) }
                                                    }
                                                    "monthly" => Some(RecurrenceRule::Monthly { day_of_month: rec_monthly_day() }),
                                                    "monthly_by_weekday" => Some(RecurrenceRule::MonthlyByWeekday {
                                                        nth: rec_monthly_nth(),
                                                        weekday: rec_monthly_weekday(),
                                                    }),
                                                    "yearly" => Some(RecurrenceRule::Yearly { month: rec_yearly_month(), day: rec_yearly_day() }),
                                                    "custom" => Some(RecurrenceRule::Custom { interval_days: rec_custom_days() }),
                                                    _ => None,
                                                };
                                                let tid = tid_rec.clone();
                                                editing_recurrence.set(false);
                                                spawn(async move {
                                                    let mut req = update_req();
                                                    req.recurrence_rule = rule;
                                                    if let Ok(updated) = use_tasks::update_task(&tid, &req).await {
                                                        task.set(Some(updated));
                                                        on_updated.call(());
                                                    }
                                                });
                                            },
                                            "Save"
                                        }
                                        button {
                                            class: "btn btn-ghost btn-sm",
                                            onclick: move |_| editing_recurrence.set(false),
                                            "Cancel"
                                        }
                                    }
                                }
                            }
                        }
                    } else {
                        {
                            let has_rule = t.recurrence_rule.is_some();
                            let display = t.recurrence_rule.as_ref().map(format_recurrence).unwrap_or_else(|| "None".to_string());
                            rsx! {
                                button {
                                    class: "btn btn-ghost btn-sm justify-start font-normal",
                                    onclick: move |_| {
                                        // Initialize picker from current value
                                        if let Some(ref t) = *task.read() {
                                            match &t.recurrence_rule {
                                                Some(RecurrenceRule::Daily) => rec_type.set("daily".to_string()),
                                                Some(RecurrenceRule::Weekly { days }) => {
                                                    rec_type.set("weekly".to_string());
                                                    rec_weekly_days.set(days.clone());
                                                }
                                                Some(RecurrenceRule::Monthly { day_of_month }) => {
                                                    rec_type.set("monthly".to_string());
                                                    rec_monthly_day.set(*day_of_month);
                                                }
                                                Some(RecurrenceRule::MonthlyByWeekday { nth, weekday }) => {
                                                    rec_type.set("monthly_by_weekday".to_string());
                                                    rec_monthly_nth.set(*nth);
                                                    rec_monthly_weekday.set(*weekday);
                                                }
                                                Some(RecurrenceRule::Yearly { month, day }) => {
                                                    rec_type.set("yearly".to_string());
                                                    rec_yearly_month.set(*month);
                                                    rec_yearly_day.set(*day);
                                                }
                                                Some(RecurrenceRule::Custom { interval_days }) => {
                                                    rec_type.set("custom".to_string());
                                                    rec_custom_days.set(*interval_days);
                                                }
                                                None => rec_type.set("none".to_string()),
                                            }
                                        }
                                        editing_recurrence.set(true);
                                    },
                                    if has_rule {
                                        span { class: "text-primary", "🔄 {display}" }
                                    } else {
                                        span { class: "text-base-content/40", "{display}" }
                                    }
                                }
                            }
                        }
                    }
                }

                // Labels (chip-input + picker)
                {
                    let current_labels = t.labels.clone();
                    let project_id = t.project_id.clone();
                    let filter = label_picker_filter.read().to_lowercase();
                    let exact_match_exists = available_labels
                        .read()
                        .iter()
                        .any(|l| l.name.to_lowercase() == filter);

                    rsx! {
                        div {
                            label { class: "label", span { class: "label-text text-xs font-semibold uppercase", "Labels" } }

                            // Selected chips + add button
                            div { class: "flex flex-wrap items-center gap-1",
                                for lbl in current_labels.iter() {
                                    {
                                        let lbl_str = lbl.clone();
                                        let lbl_for_remove = lbl.clone();
                                        let color = label_color_for(&available_labels.read(), &lbl_str).to_string();
                                        let tid = tid_labels.clone();
                                        let labels_for_remove = current_labels.clone();
                                        rsx! {
                                            span {
                                                key: "{lbl_str}",
                                                class: "inline-flex items-center gap-1 px-2 py-0.5 rounded text-xs font-medium text-white",
                                                style: "background: {color};",
                                                "{lbl_str}"
                                                button {
                                                    class: "opacity-70 hover:opacity-100 leading-none",
                                                    title: "Remove",
                                                    onclick: move |_| {
                                                        let tid = tid.clone();
                                                        let new_labels: Vec<String> = labels_for_remove
                                                            .iter()
                                                            .filter(|l| **l != lbl_for_remove)
                                                            .cloned()
                                                            .collect();
                                                        spawn(async move {
                                                            let mut req = update_req();
                                                            req.labels = Some(new_labels);
                                                            if let Ok(updated) = use_tasks::update_task(&tid, &req).await {
                                                                task.set(Some(updated));
                                                                on_updated.call(());
                                                            }
                                                        });
                                                    },
                                                    "×"
                                                }
                                            }
                                        }
                                    }
                                }

                                button {
                                    class: "btn btn-ghost btn-xs",
                                    onclick: move |_| {
                                        let next = !*label_picker_open.peek();
                                        label_picker_open.set(next);
                                        if next {
                                            label_picker_filter.set(String::new());
                                        }
                                    },
                                    if *label_picker_open.read() { "Close" } else { "+ Add label" }
                                }
                            }

                            // Picker dropdown
                            if *label_picker_open.read() {
                                div { class: "mt-2 p-2 bg-base-200 rounded-box space-y-2",
                                    input {
                                        class: "input input-bordered input-sm w-full",
                                        r#type: "text",
                                        placeholder: "Type to filter or create...",
                                        value: "{label_picker_filter}",
                                        oninput: move |e| label_picker_filter.set(e.value()),
                                    }

                                    // Existing labels list (filtered)
                                    div { class: "max-h-48 overflow-y-auto space-y-1",
                                        {
                                            let filter_lower = label_picker_filter.read().to_lowercase();
                                            let avail_filtered: Vec<TaskLabelResponse> = available_labels
                                                .read()
                                                .iter()
                                                .filter(|l| {
                                                    filter_lower.is_empty()
                                                        || l.name.to_lowercase().contains(&filter_lower)
                                                })
                                                .cloned()
                                                .collect();

                                            rsx! {
                                                for label in avail_filtered.iter() {
                                                    {
                                                        let l_name = label.name.clone();
                                                        let l_color = label.color.clone();
                                                        let l_id = label.id.clone();
                                                        let is_selected = current_labels.contains(&l_name);
                                                        let tid = tid_labels.clone();
                                                        let labels_for_toggle = current_labels.clone();
                                                        let l_name_toggle = l_name.clone();

                                                        rsx! {
                                                            button {
                                                                key: "{l_id}",
                                                                class: "w-full flex items-center gap-2 px-2 py-1 rounded hover:bg-base-300 text-left",
                                                                onclick: move |_| {
                                                                    let tid = tid.clone();
                                                                    let mut new_labels: Vec<String> =
                                                                        labels_for_toggle.iter().cloned().collect();
                                                                    if is_selected {
                                                                        new_labels.retain(|l| *l != l_name_toggle);
                                                                    } else {
                                                                        new_labels.push(l_name_toggle.clone());
                                                                    }
                                                                    spawn(async move {
                                                                        let mut req = update_req();
                                                                        req.labels = Some(new_labels);
                                                                        if let Ok(updated) =
                                                                            use_tasks::update_task(&tid, &req).await
                                                                        {
                                                                            task.set(Some(updated));
                                                                            on_updated.call(());
                                                                        }
                                                                    });
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

                                    // Create new label inline
                                    if !filter.is_empty() && !exact_match_exists {
                                        div { class: "border-t border-base-300 pt-2",
                                            div { class: "flex gap-1 mb-1",
                                                for color in LABEL_PALETTE.iter() {
                                                    {
                                                        let c = color.to_string();
                                                        let c2 = c.clone();
                                                        let selected = *label_picker_color.read() == c;
                                                        rsx! {
                                                            button {
                                                                key: "{c}",
                                                                class: if selected {
                                                                    "w-4 h-4 rounded-full ring-2 ring-offset-1 ring-base-content"
                                                                } else {
                                                                    "w-4 h-4 rounded-full hover:ring-2 hover:ring-offset-1 hover:ring-base-content/40"
                                                                },
                                                                style: "background: {c};",
                                                                onclick: move |_| label_picker_color.set(c2.clone()),
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                            button {
                                                class: "btn btn-primary btn-xs w-full",
                                                onclick: move |_| {
                                                    let pid = project_id.clone();
                                                    let tid = tid_label_create.clone();
                                                    let name = label_picker_filter.peek().trim().to_string();
                                                    if name.is_empty() { return; }
                                                    let color = label_picker_color.peek().clone();
                                                    let mut existing = current_labels.clone();
                                                    spawn(async move {
                                                        let req = CreateTaskLabelRequest {
                                                            name: name.clone(),
                                                            color: color.clone(),
                                                        };
                                                        match use_tasks::create_label(&pid, &req).await {
                                                            Ok(label) => {
                                                                available_labels.write().push(label);
                                                                existing.push(name);
                                                                let mut req = update_req();
                                                                req.labels = Some(existing);
                                                                if let Ok(updated) =
                                                                    use_tasks::update_task(&tid, &req).await
                                                                {
                                                                    task.set(Some(updated));
                                                                    on_updated.call(());
                                                                }
                                                                label_picker_filter.set(String::new());
                                                            }
                                                            Err(_) => {}
                                                        }
                                                    });
                                                },
                                                {format!("+ Create \"{}\"", filter)}
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                div { class: "divider my-0" }

                // Description
                div {
                    label { class: "label", span { class: "label-text text-xs font-semibold uppercase", "Description" } }
                    if *editing_desc.read() {
                        textarea {
                            class: "textarea textarea-bordered w-full min-h-[6rem]",
                            value: "{desc_draft}",
                            autofocus: true,
                            oninput: move |e| desc_draft.set(e.value()),
                            onblur: move |_| save_desc_a(),
                            onkeydown: move |e: KeyboardEvent| {
                                if e.key() == Key::Escape {
                                    editing_desc.set(false);
                                    if let Some(t) = task.read().as_ref() {
                                        desc_draft.set(t.description.clone().unwrap_or_default());
                                    }
                                }
                            },
                        }
                    } else {
                        div {
                            class: "min-h-[3rem] p-2 rounded cursor-pointer hover:bg-base-200 text-sm whitespace-pre-wrap",
                            onclick: move |_| editing_desc.set(true),
                            if t.description.as_ref().map_or(true, |d| d.is_empty()) {
                                span { class: "text-base-content/40 italic", "Add a description..." }
                            } else {
                                "{t.description.as_deref().unwrap_or(\"\")}"
                            }
                        }
                    }
                }

                div { class: "divider my-0" }

                // Subtasks
                div {
                    div { class: "flex items-center justify-between",
                        label { class: "label", span { class: "label-text text-xs font-semibold uppercase", "Subtasks" } }
                        button {
                            class: "btn btn-ghost btn-xs",
                            onclick: move |_| adding_subtask.set(true),
                            "+ Add"
                        }
                    }

                    div { class: "flex flex-col gap-1",
                        for sub in subtasks.read().iter() {
                            {
                                let sub_id = sub.id.clone();
                                let sub_done = sub.status == TaskStatus::Done;
                                let sub_title = sub.title.clone();
                                rsx! {
                                    div { class: "flex items-center gap-2 py-1",
                                        input {
                                            class: "checkbox checkbox-sm",
                                            r#type: "checkbox",
                                            checked: sub_done,
                                            onchange: move |_| {
                                                let sid = sub_id.clone();
                                                let new_s = if sub_done { TaskStatus::Todo } else { TaskStatus::Done };
                                                spawn(async move {
                                                    let req = UpdateTaskStatusRequest {
                                                        status: new_s,
                                                        status_note: None,
                                                    };
                                                    if let Ok(updated) = use_tasks::update_task_status(&sid, &req).await {
                                                        let mut sw = subtasks.write();
                                                        if let Some(s) = sw.iter_mut().find(|s| s.id == updated.id) {
                                                            *s = updated;
                                                        }
                                                        on_updated.call(());
                                                    }
                                                });
                                            },
                                        }
                                        span {
                                            class: if sub_done { "text-sm line-through text-base-content/50" } else { "text-sm" },
                                            "{sub_title}"
                                        }
                                    }
                                }
                            }
                        }

                        if *adding_subtask.read() {
                            div { class: "flex items-center gap-2 py-1",
                                input {
                                    class: "input input-bordered input-sm flex-1",
                                    r#type: "text",
                                    placeholder: "Subtask title...",
                                    autofocus: true,
                                    value: "{new_subtask_title}",
                                    oninput: move |e| new_subtask_title.set(e.value()),
                                    onkeydown: move |e: KeyboardEvent| {
                                        if e.key() == Key::Enter {
                                            let title = new_subtask_title.peek().trim().to_string();
                                            if title.is_empty() {
                                                adding_subtask.set(false);
                                                return;
                                            }
                                            let tid = tid_subtask.clone();
                                            new_subtask_title.set(String::new());
                                            adding_subtask.set(false);
                                            spawn(async move {
                                                let req = CreateTaskRequest {
                                                    title,
                                                    parent_task_id: Some(tid.clone()),
                                                    section_id: None,
                                                    description: None,
                                                    status: None,
                                                    priority: None,
                                                    assignee_id: None,
                                                    labels: None,
                                                    due_date: None,
                                                    recurrence_rule: None,
                                                    position: None,
                                                };
                                                if let Ok(sub) = use_tasks::create_subtask(&tid, &req).await {
                                                    subtasks.write().push(sub);
                                                    if let Some(t) = task.write().as_mut() {
                                                        t.subtask_count += 1;
                                                    }
                                                    on_updated.call(());
                                                }
                                            });
                                        } else if e.key() == Key::Escape {
                                            adding_subtask.set(false);
                                            new_subtask_title.set(String::new());
                                        }
                                    },
                                }
                            }
                        }

                        if subtasks.read().is_empty() && !*adding_subtask.read() {
                            p { class: "text-sm text-base-content/40 py-2", "No subtasks" }
                        }
                    }
                }

                div { class: "divider my-0" }

                // Comments
                div {
                    label { class: "label", span { class: "label-text text-xs font-semibold uppercase", "Comments" } }

                    div { class: "flex flex-col gap-3",
                        for comment in comments.read().iter() {
                            div { class: "bg-base-200 rounded-lg p-3",
                                div { class: "flex items-center gap-2 mb-1",
                                    div { class: "avatar placeholder",
                                        div { class: "bg-neutral text-neutral-content w-5 h-5 rounded-full",
                                            span { class: "text-[10px]",
                                                {comment.author_username.chars().next().unwrap_or('?').to_uppercase().to_string()}
                                            }
                                        }
                                    }
                                    span { class: "text-xs font-semibold", "{comment.author_username}" }
                                    span { class: "text-xs text-base-content/50",
                                        {comment.created_at.get(..10).unwrap_or(&comment.created_at)}
                                    }
                                }
                                p { class: "text-sm whitespace-pre-wrap", "{comment.body}" }
                            }
                        }

                        if comments.read().is_empty() {
                            p { class: "text-sm text-base-content/40 py-2", "No comments yet" }
                        }

                        // Add comment
                        div { class: "flex gap-2",
                            input {
                                class: "input input-bordered input-sm flex-1",
                                r#type: "text",
                                placeholder: "Write a comment...",
                                value: "{new_comment}",
                                oninput: move |e| new_comment.set(e.value()),
                                onkeydown: move |e: KeyboardEvent| {
                                    if e.key() == Key::Enter {
                                        post_comment_a();
                                    }
                                },
                            }
                            button {
                                class: "btn btn-primary btn-sm",
                                onclick: move |_| post_comment_b(),
                                "Send"
                            }
                        }
                    }

                    // Delete task
                    div { class: "divider" }
                    {
                        let tid_del = task_id.clone();
                        rsx! {
                            if *confirm_delete.read() {
                                div { class: "flex items-center gap-2",
                                    span { class: "text-sm text-error", "Delete this task?" }
                                    button {
                                        class: "btn btn-error btn-sm",
                                        onclick: move |_| {
                                            let tid = tid_del.clone();
                                            spawn(async move {
                                                if use_tasks::delete_task(&tid).await.is_ok() {
                                                    on_deleted.call(tid);
                                                    on_close.call(());
                                                }
                                            });
                                        },
                                        "Delete"
                                    }
                                    button {
                                        class: "btn btn-ghost btn-sm",
                                        onclick: move |_| confirm_delete.set(false),
                                        "Cancel"
                                    }
                                }
                            } else {
                                button {
                                    class: "btn btn-ghost btn-sm text-error",
                                    onclick: move |_| confirm_delete.set(true),
                                    "Delete task"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
