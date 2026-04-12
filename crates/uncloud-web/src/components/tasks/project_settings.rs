use dioxus::prelude::*;
use uncloud_common::{
    AddProjectMemberRequest, ProjectMemberResponse, ProjectPermission, UpdateTaskProjectRequest,
};

use crate::hooks::{use_shopping, use_tasks};

#[component]
pub fn ProjectSettings(
    project_id: String,
    project_name: String,
    project_color: String,
    owner_id: String,
    members: Vec<ProjectMemberResponse>,
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

    let pid_save = project_id.clone();
    let pid_del = project_id.clone();
    let pid_member = project_id.clone();

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
                            let member_username = member.username.clone();
                            let is_owner = member.user_id == owner_id;
                            let perm_label = match member.permission {
                                ProjectPermission::Viewer => "Viewer",
                                ProjectPermission::Editor => "Editor",
                                ProjectPermission::Admin => "Admin",
                            };
                            let pid_rm = pid_member.clone();

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
                                        span { class: "badge badge-sm badge-ghost", "{perm_label}" }
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
