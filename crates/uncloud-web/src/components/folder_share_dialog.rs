use dioxus::prelude::*;
use uncloud_common::{
    CreateFolderShareRequest, FolderShareResponse, SharePermission, UpdateFolderShareRequest,
};
use crate::hooks::use_folder_shares;
use crate::components::shared_with_me::PermissionBadge;

/// Embeddable sharing panel — used inside FolderSettingsModal's "Sharing" tab.
#[component]
pub fn FolderSharePanel(
    folder_id: String,
) -> Element {
    let mut username = use_signal(String::new);
    let mut permission = use_signal(|| SharePermission::ReadOnly);
    let mut creating = use_signal(|| false);
    let mut create_error: Signal<Option<String>> = use_signal(|| None);

    let mut shares: Signal<Vec<FolderShareResponse>> = use_signal(Vec::new);
    let mut shares_loading = use_signal(|| true);
    let mut refresh = use_signal(|| 0u32);

    let folder_id_load = folder_id.clone();
    use_effect(move || {
        let _ = refresh();
        let fid = folder_id_load.clone();
        spawn(async move {
            match use_folder_shares::list_folder_shares(&fid).await {
                Ok(s) => shares.set(s),
                Err(_) => {}
            }
            shares_loading.set(false);
        });
    });

    let folder_id_create = folder_id.clone();
    let on_create = move |_| {
        let uname = username().trim().to_string();
        if uname.is_empty() {
            create_error.set(Some("Username is required".to_string()));
            return;
        }
        let perm = permission();
        let fid = folder_id_create.clone();
        creating.set(true);
        create_error.set(None);

        spawn(async move {
            let req = CreateFolderShareRequest {
                folder_id: fid,
                grantee_username: uname.clone(),
                permission: perm,
            };
            match use_folder_shares::create_folder_share(&req).await {
                Ok(_) => {
                    username.set(String::new());
                    let next = *refresh.peek() + 1;
                    refresh.set(next);
                }
                Err(e) => {
                    if e == "CONFLICT" {
                        create_error.set(Some(format!(
                            "This folder is already shared with \"{}\"",
                            uname
                        )));
                    } else {
                        create_error.set(Some(e));
                    }
                }
            }
            creating.set(false);
        });
    };

    rsx! {
        div { class: "flex flex-col gap-3",
            if let Some(err) = create_error() {
                div { class: "alert alert-error text-sm", "{err}" }
            }

            div { class: "form-control",
                label { class: "label",
                    span { class: "label-text", "Username" }
                }
                input {
                    class: "input input-bordered w-full",
                    r#type: "text",
                    placeholder: "Enter username to share with",
                    value: "{username}",
                    oninput: move |e| username.set(e.value()),
                }
            }

            div { class: "form-control",
                label { class: "label",
                    span { class: "label-text", "Permission" }
                }
                select {
                    class: "select select-bordered w-full",
                    value: match permission() {
                        SharePermission::ReadOnly => "read_only",
                        SharePermission::ReadWrite => "read_write",
                        SharePermission::Admin => "admin",
                    },
                    onchange: move |e| {
                        permission.set(match e.value().as_str() {
                            "read_write" => SharePermission::ReadWrite,
                            "admin" => SharePermission::Admin,
                            _ => SharePermission::ReadOnly,
                        });
                    },
                    option { value: "read_only", "Read Only" }
                    option { value: "read_write", "Read / Write" }
                    option { value: "admin", "Admin" }
                }
            }

            button {
                class: "btn btn-primary",
                disabled: creating() || username().trim().is_empty(),
                onclick: on_create,
                if creating() {
                    span { class: "loading loading-spinner loading-sm" }
                    "Sharing..."
                } else {
                    "Share"
                }
            }

            if !shares_loading() && !shares().is_empty() {
                div { class: "divider my-1", "Current shares" }
                div { class: "flex flex-col gap-2",
                    for share in shares() {
                        {
                            let share_id = share.id.clone();
                            let share_id_perm = share.id.clone();
                            rsx! {
                                div { class: "flex items-center justify-between gap-2 p-2 rounded-lg bg-base-200",
                                    div { class: "flex items-center gap-2 min-w-0 flex-1",
                                        span { class: "font-medium text-sm truncate",
                                            "{share.grantee_username}"
                                        }
                                        PermissionBadge { permission: share.permission }
                                    }
                                    div { class: "flex items-center gap-1 flex-shrink-0",
                                        select {
                                            class: "select select-bordered select-xs",
                                            value: match share.permission {
                                                SharePermission::ReadOnly => "read_only",
                                                SharePermission::ReadWrite => "read_write",
                                                SharePermission::Admin => "admin",
                                            },
                                            onchange: move |e| {
                                                let new_perm = match e.value().as_str() {
                                                    "read_write" => SharePermission::ReadWrite,
                                                    "admin" => SharePermission::Admin,
                                                    _ => SharePermission::ReadOnly,
                                                };
                                                let sid = share_id_perm.clone();
                                                spawn(async move {
                                                    let req = UpdateFolderShareRequest {
                                                        permission: Some(new_perm),
                                                        mount_parent_id: None,
                                                        mount_name: None,
                                                        music_include: None,
                                                        gallery_include: None,
                                                    };
                                                    let _ = use_folder_shares::update_folder_share(&sid, &req).await;
                                                    let next = *refresh.peek() + 1;
                                                    refresh.set(next);
                                                });
                                            },
                                            option { value: "read_only", "Read Only" }
                                            option { value: "read_write", "Read / Write" }
                                            option { value: "admin", "Admin" }
                                        }
                                        button {
                                            class: "btn btn-ghost btn-xs btn-circle text-error",
                                            title: "Revoke",
                                            onclick: move |_| {
                                                let sid = share_id.clone();
                                                spawn(async move {
                                                    let _ = use_folder_shares::delete_folder_share(&sid).await;
                                                    let next = *refresh.peek() + 1;
                                                    refresh.set(next);
                                                });
                                            },
                                            "✕"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if !shares_loading() && shares().is_empty() {
                p { class: "text-sm text-base-content/50",
                    "This folder is not shared with anyone."
                }
            }
        }
    }
}
