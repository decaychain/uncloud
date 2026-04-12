use dioxus::prelude::*;
use uncloud_common::UpdateTaskProjectRequest;

use crate::hooks::use_tasks;

#[component]
pub fn ProjectSettings(
    project_id: String,
    project_name: String,
    project_color: String,
    on_close: EventHandler<()>,
    on_updated: EventHandler<String>,
    on_deleted: EventHandler<()>,
) -> Element {
    let mut name_draft = use_signal(|| project_name.clone());
    let mut color_draft = use_signal(|| project_color.clone());
    let mut saving = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut confirm_delete = use_signal(|| false);

    let pid_save = project_id.clone();
    let pid_del = project_id.clone();

    rsx! {
        // Backdrop
        div {
            class: "modal modal-open",
            onclick: move |_| on_close.call(()),

            div {
                class: "modal-box",
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

                // Save button
                div { class: "modal-action",
                    // Delete button (left side)
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
