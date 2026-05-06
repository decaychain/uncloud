use dioxus::prelude::*;
use uncloud_common::{
    AddProjectMemberRequest, CreateTaskLabelRequest, ProjectMemberResponse, ProjectPermission,
    TaskLabelResponse, UpdateProjectMemberRequest, UpdateTaskLabelRequest, UpdateTaskProjectRequest,
};

use crate::hooks::{use_shopping, use_tasks};
use super::LABEL_PALETTE;

#[component]
pub fn ProjectSettings(
    project_id: String,
    project_name: String,
    project_color: String,
    owner_id: String,
    members: Vec<ProjectMemberResponse>,
    available_labels: Signal<Vec<TaskLabelResponse>>,
    on_close: EventHandler<()>,
    on_updated: EventHandler<String>,
    on_deleted: EventHandler<()>,
) -> Element {
    let mut name_draft = use_signal(|| project_name.clone());
    let mut color_draft = use_signal(|| project_color.clone());
    let mut saving = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut confirm_delete = use_signal(|| false);

    // Members state
    let mut current_members: Signal<Vec<ProjectMemberResponse>> =
        use_signal(|| members.clone());
    let mut available_users: Signal<Vec<use_shopping::UserNameEntry>> = use_signal(Vec::new);
    let mut selected_user_id = use_signal(String::new);
    let mut selected_permission: Signal<ProjectPermission> =
        use_signal(|| ProjectPermission::Editor);
    let mut member_error: Signal<Option<String>> = use_signal(|| None);

    // Labels state — `labels` is the shared `available_labels` signal lifted to
    // TasksProjectPage so edits here propagate immediately to BoardView/ListView.
    let mut labels = available_labels;
    let mut new_label_name = use_signal(String::new);
    let mut new_label_color = use_signal(|| LABEL_PALETTE[5].to_string()); // blue default
    let mut editing_label_id: Signal<Option<String>> = use_signal(|| None);
    let mut edit_label_name = use_signal(String::new);
    let mut edit_label_color = use_signal(String::new);
    let mut label_error: Signal<Option<String>> = use_signal(|| None);

    let pid_save = project_id.clone();
    let pid_del = project_id.clone();
    let pid_member = project_id.clone();
    let pid_add_label = project_id.clone();

    // Fetch available users
    use_effect(move || {
        spawn(async move {
            if let Ok(entries) = use_shopping::list_user_entries().await {
                available_users.set(entries);
            }
        });
    });

    rsx! {
        // Backdrop
        div {
            class: "modal modal-open",
            onclick: move |_| on_close.call(()),

            div {
                class: "modal-box max-w-lg",
                onclick: move |e| e.stop_propagation(),

                h3 { class: "font-bold text-lg mb-4", "Project Settings" }

                if let Some(err) = error.read().as_ref() {
                    div { class: "alert alert-error mb-3 text-sm",
                        span { "{err}" }
                    }
                }

                // Name
                div { class: "form-control mb-3",
                    label { class: "label", span { class: "label-text", "Project Name" } }
                    input {
                        class: "input input-bordered w-full",
                        r#type: "text",
                        value: "{name_draft}",
                        oninput: move |e| name_draft.set(e.value()),
                    }
                }

                // Color
                div { class: "form-control mb-4",
                    label { class: "label", span { class: "label-text", "Color" } }
                    div { class: "flex items-center gap-2",
                        input {
                            class: "w-10 h-10 rounded cursor-pointer border-none p-0",
                            r#type: "color",
                            value: "{color_draft}",
                            oninput: move |e| color_draft.set(e.value()),
                        }
                        span { class: "text-sm text-base-content/60", "{color_draft}" }
                    }
                }

                // Members section
                div { class: "divider text-xs uppercase", "Members" }

                // Current members
                div { class: "space-y-2 mb-3",
                    for member in current_members.read().iter() {
                        {
                            let member_id = member.user_id.clone();
                            let member_id_role = member_id.clone();
                            let member_username = member.username.clone();
                            let is_owner = member.user_id == owner_id;
                            let current_perm = match member.permission {
                                ProjectPermission::Viewer => "viewer",
                                ProjectPermission::Editor => "editor",
                                ProjectPermission::Admin => "admin",
                            };
                            let pid_rm = pid_member.clone();
                            let pid_role = pid_member.clone();

                            rsx! {
                                div {
                                    key: "{member_id}",
                                    class: "flex items-center gap-2",

                                    // Avatar
                                    div { class: "avatar placeholder",
                                        div { class: "bg-neutral text-neutral-content rounded-full w-8 h-8",
                                            span { class: "text-xs",
                                                {member_username.chars().next().unwrap_or('?').to_uppercase().to_string()}
                                            }
                                        }
                                    }

                                    span { class: "flex-1 text-sm", "{member_username}" }

                                    if is_owner {
                                        span { class: "badge badge-sm badge-primary", "Owner" }
                                    } else {
                                        select {
                                            class: "select select-bordered select-xs",
                                            value: "{current_perm}",
                                            onchange: move |e| {
                                                let new_perm = match e.value().as_str() {
                                                    "viewer" => ProjectPermission::Viewer,
                                                    "admin" => ProjectPermission::Admin,
                                                    _ => ProjectPermission::Editor,
                                                };
                                                let uid = member_id_role.clone();
                                                let pid = pid_role.clone();
                                                spawn(async move {
                                                    let req = UpdateProjectMemberRequest {
                                                        permission: new_perm.clone(),
                                                    };
                                                    if use_tasks::update_member(&pid, &uid, &req)
                                                        .await
                                                        .is_ok()
                                                    {
                                                        let mut mw = current_members.write();
                                                        if let Some(m) =
                                                            mw.iter_mut().find(|m| m.user_id == uid)
                                                        {
                                                            m.permission = new_perm;
                                                        }
                                                    }
                                                });
                                            },
                                            option { value: "viewer", "Viewer" }
                                            option { value: "editor", "Editor" }
                                            option { value: "admin", "Admin" }
                                        }
                                        button {
                                            class: "btn btn-ghost btn-xs btn-circle text-error",
                                            title: "Remove member",
                                            onclick: move |_| {
                                                let uid = member_id.clone();
                                                let pid = pid_rm.clone();
                                                spawn(async move {
                                                    if use_tasks::remove_member(&pid, &uid).await.is_ok() {
                                                        current_members.write().retain(|m| m.user_id != uid);
                                                    }
                                                });
                                            },
                                            "×"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Add member
                {
                    let member_ids: Vec<String> = current_members.read().iter().map(|m| m.user_id.clone()).collect();
                    let has_available = available_users.read().iter().any(|u| !member_ids.contains(&u.id));
                    let pid_add = pid_member.clone();

                    rsx! {
                        if let Some(err) = member_error.read().as_ref() {
                            div { class: "text-error text-xs mb-1", "{err}" }
                        }

                        if has_available {
                            div { class: "flex items-center gap-2",
                                select {
                                    class: "select select-bordered select-sm flex-1",
                                    value: "{selected_user_id}",
                                    onchange: move |e| selected_user_id.set(e.value()),

                                    option { value: "", disabled: true, selected: true, "Select user..." }
                                    for user in available_users.read().iter() {
                                        if !member_ids.contains(&user.id) {
                                            option {
                                                value: "{user.id}",
                                                "{user.username}"
                                            }
                                        }
                                    }
                                }

                                select {
                                    class: "select select-bordered select-sm w-28",
                                    value: match *selected_permission.read() {
                                        ProjectPermission::Viewer => "viewer",
                                        ProjectPermission::Editor => "editor",
                                        ProjectPermission::Admin => "admin",
                                    },
                                    onchange: move |e| {
                                        let p = match e.value().as_str() {
                                            "viewer" => ProjectPermission::Viewer,
                                            "admin" => ProjectPermission::Admin,
                                            _ => ProjectPermission::Editor,
                                        };
                                        selected_permission.set(p);
                                    },
                                    option { value: "viewer", "Viewer" }
                                    option { value: "editor", "Editor" }
                                    option { value: "admin", "Admin" }
                                }

                                button {
                                    class: "btn btn-primary btn-sm",
                                    disabled: selected_user_id.read().is_empty(),
                                    onclick: move |_| {
                                        let uid = selected_user_id.peek().clone();
                                        if uid.is_empty() { return; }
                                        let perm = selected_permission.peek().clone();
                                        let pid = pid_add.clone();
                                        member_error.set(None);
                                        spawn(async move {
                                            let req = AddProjectMemberRequest {
                                                user_id: uid.clone(),
                                                permission: perm,
                                            };
                                            match use_tasks::add_member(&pid, &req).await {
                                                Ok(()) => {
                                                    if let Ok(p) = use_tasks::get_project(&pid).await {
                                                        current_members.set(p.members);
                                                    }
                                                    selected_user_id.set(String::new());
                                                }
                                                Err(e) => member_error.set(Some(e)),
                                            }
                                        });
                                    },
                                    "Add"
                                }
                            }
                        }
                    }
                }

                // Labels section
                div { class: "divider text-xs uppercase", "Labels" }

                if let Some(err) = label_error.read().as_ref() {
                    div { class: "text-error text-xs mb-1", "{err}" }
                }

                // Existing labels list
                div { class: "space-y-1.5 mb-3",
                    for label in labels.read().iter() {
                        {
                            let label_id = label.id.clone();
                            let label_name = label.name.clone();
                            let label_color = label.color.clone();
                            let is_editing = editing_label_id.read().as_ref() == Some(&label_id);
                            let label_id_edit = label_id.clone();
                            let label_id_save = label_id.clone();
                            let label_id_del = label_id.clone();

                            rsx! {
                                div {
                                    key: "{label_id}",
                                    class: "flex items-center gap-2",

                                    if is_editing {
                                        // Color swatches
                                        div { class: "flex gap-1",
                                            for color in LABEL_PALETTE.iter() {
                                                {
                                                    let c = color.to_string();
                                                    let c2 = c.clone();
                                                    let selected = *edit_label_color.read() == c;
                                                    rsx! {
                                                        button {
                                                            key: "{c}",
                                                            class: if selected {
                                                                "w-5 h-5 rounded-full ring-2 ring-offset-1 ring-base-content"
                                                            } else {
                                                                "w-5 h-5 rounded-full hover:ring-2 hover:ring-offset-1 hover:ring-base-content/40"
                                                            },
                                                            style: "background: {c};",
                                                            onclick: move |_| edit_label_color.set(c2.clone()),
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        input {
                                            class: "input input-bordered input-sm flex-1",
                                            r#type: "text",
                                            value: "{edit_label_name}",
                                            oninput: move |e| edit_label_name.set(e.value()),
                                        }
                                        button {
                                            class: "btn btn-primary btn-sm btn-circle",
                                            title: "Save",
                                            onclick: move |_| {
                                                let id = label_id_save.clone();
                                                let name = edit_label_name.peek().trim().to_string();
                                                if name.is_empty() {
                                                    label_error.set(Some("Name cannot be empty".into()));
                                                    return;
                                                }
                                                let color = edit_label_color.peek().clone();
                                                label_error.set(None);
                                                spawn(async move {
                                                    let req = UpdateTaskLabelRequest {
                                                        name: Some(name.clone()),
                                                        color: Some(color.clone()),
                                                    };
                                                    match use_tasks::update_label(&id, &req).await {
                                                        Ok(updated) => {
                                                            let mut lw = labels.write();
                                                            if let Some(l) = lw.iter_mut().find(|l| l.id == updated.id) {
                                                                *l = updated;
                                                            }
                                                            editing_label_id.set(None);
                                                        }
                                                        Err(e) => {
                                                            if e == "CONFLICT" {
                                                                label_error.set(Some("A label with that name already exists".into()));
                                                            } else {
                                                                label_error.set(Some(e));
                                                            }
                                                        }
                                                    }
                                                });
                                            },
                                            "✓"
                                        }
                                        button {
                                            class: "btn btn-ghost btn-sm btn-circle",
                                            title: "Cancel",
                                            onclick: move |_| {
                                                editing_label_id.set(None);
                                                label_error.set(None);
                                            },
                                            "×"
                                        }
                                    } else {
                                        span {
                                            class: "px-2 py-0.5 rounded text-xs font-medium text-white shrink-0",
                                            style: "background: {label_color};",
                                            "{label_name}"
                                        }
                                        span { class: "flex-1 text-xs text-base-content/50 font-mono", "{label_color}" }
                                        button {
                                            class: "btn btn-ghost btn-xs btn-circle opacity-50 hover:opacity-100",
                                            title: "Edit",
                                            onclick: move |_| {
                                                edit_label_name.set(label_name.clone());
                                                edit_label_color.set(label_color.clone());
                                                editing_label_id.set(Some(label_id_edit.clone()));
                                                label_error.set(None);
                                            },
                                            // Pencil
                                            svg {
                                                class: "w-3 h-3",
                                                xmlns: "http://www.w3.org/2000/svg",
                                                view_box: "0 0 24 24",
                                                fill: "none",
                                                stroke: "currentColor",
                                                stroke_width: "2",
                                                stroke_linecap: "round",
                                                stroke_linejoin: "round",
                                                path { d: "M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z" }
                                            }
                                        }
                                        button {
                                            class: "btn btn-ghost btn-xs btn-circle text-error opacity-50 hover:opacity-100",
                                            title: "Delete",
                                            onclick: move |_| {
                                                let id = label_id_del.clone();
                                                label_error.set(None);
                                                spawn(async move {
                                                    if use_tasks::delete_label(&id).await.is_ok() {
                                                        labels.write().retain(|l| l.id != id);
                                                    }
                                                });
                                            },
                                            "×"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Add label form
                div { class: "flex items-center gap-2",
                    div { class: "flex gap-1",
                        for color in LABEL_PALETTE.iter() {
                            {
                                let c = color.to_string();
                                let c2 = c.clone();
                                let selected = *new_label_color.read() == c;
                                rsx! {
                                    button {
                                        key: "{c}",
                                        class: if selected {
                                            "w-5 h-5 rounded-full ring-2 ring-offset-1 ring-base-content"
                                        } else {
                                            "w-5 h-5 rounded-full hover:ring-2 hover:ring-offset-1 hover:ring-base-content/40"
                                        },
                                        style: "background: {c};",
                                        onclick: move |_| new_label_color.set(c2.clone()),
                                    }
                                }
                            }
                        }
                    }
                    input {
                        class: "input input-bordered input-sm flex-1",
                        r#type: "text",
                        placeholder: "New label name",
                        value: "{new_label_name}",
                        oninput: move |e| new_label_name.set(e.value()),
                    }
                    button {
                        class: "btn btn-primary btn-sm",
                        disabled: new_label_name.read().trim().is_empty(),
                        onclick: move |_| {
                            let name = new_label_name.peek().trim().to_string();
                            if name.is_empty() { return; }
                            let color = new_label_color.peek().clone();
                            let pid = pid_add_label.clone();
                            label_error.set(None);
                            spawn(async move {
                                let req = CreateTaskLabelRequest { name: name.clone(), color: color.clone() };
                                match use_tasks::create_label(&pid, &req).await {
                                    Ok(label) => {
                                        labels.write().push(label);
                                        new_label_name.set(String::new());
                                    }
                                    Err(e) => {
                                        if e == "CONFLICT" {
                                            label_error.set(Some("A label with that name already exists".into()));
                                        } else {
                                            label_error.set(Some(e));
                                        }
                                    }
                                }
                            });
                        },
                        "Add"
                    }
                }

                // Save / Delete buttons
                div { class: "modal-action",
                    if *confirm_delete.read() {
                        div { class: "flex items-center gap-2 mr-auto",
                            span { class: "text-sm text-error", "Are you sure?" }
                            button {
                                class: "btn btn-error btn-sm",
                                onclick: move |_| {
                                    let pid = pid_del.clone();
                                    spawn(async move {
                                        match use_tasks::delete_project(&pid).await {
                                            Ok(()) => on_deleted.call(()),
                                            Err(e) => error.set(Some(e)),
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
                            class: "btn btn-ghost btn-sm text-error mr-auto",
                            onclick: move |_| confirm_delete.set(true),
                            "Delete Project"
                        }
                    }

                    button {
                        class: "btn btn-ghost",
                        onclick: move |_| on_close.call(()),
                        "Cancel"
                    }
                    button {
                        class: "btn btn-primary",
                        disabled: *saving.read(),
                        onclick: move |_| {
                            let name = name_draft.peek().trim().to_string();
                            if name.is_empty() {
                                error.set(Some("Name cannot be empty".into()));
                                return;
                            }
                            let color = color_draft.peek().clone();
                            let pid = pid_save.clone();
                            saving.set(true);
                            error.set(None);
                            spawn(async move {
                                let req = UpdateTaskProjectRequest {
                                    name: Some(name.clone()),
                                    description: None,
                                    color: Some(color),
                                    icon: None,
                                    default_view: None,
                                    archived: None,
                                };
                                match use_tasks::update_project(&pid, &req).await {
                                    Ok(_) => {
                                        on_updated.call(name);
                                        on_close.call(());
                                    }
                                    Err(e) => error.set(Some(e)),
                                }
                                saving.set(false);
                            });
                        },
                        if *saving.read() {
                            span { class: "loading loading-spinner loading-sm" }
                        }
                        "Save"
                    }
                }
            }
        }
    }
}
