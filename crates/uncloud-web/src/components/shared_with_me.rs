use dioxus::prelude::*;
use uncloud_common::{FolderShareResponse, ServerEvent, SharePermission};
use crate::components::icons::{IconAlertTriangle, IconFolder, IconUsers, IconX};
use crate::hooks::use_folder_shares;
use crate::router::Route;

#[component]
pub fn SharedWithMePage() -> Element {
    let mut shares: Signal<Vec<FolderShareResponse>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut refresh = use_signal(|| 0u32);
    let nav = use_navigator();

    // Leave-share confirmation: Some((share_id, folder_name))
    let mut leave_target: Signal<Option<(String, String)>> = use_signal(|| None);

    // Subscribe to SSE events for share changes
    let sse_event = use_context::<Signal<Option<ServerEvent>>>();
    use_effect(move || {
        if let Some(event) = sse_event() {
            match event {
                ServerEvent::FolderShared { .. } | ServerEvent::FolderShareRevoked { .. } => {
                    let next = *refresh.peek() + 1;
                    refresh.set(next);
                }
                _ => {}
            }
        }
    });

    use_effect(move || {
        let _ = refresh();
        spawn(async move {
            error.set(None);
            match use_folder_shares::list_shares_with_me().await {
                Ok(s) => shares.set(s),
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    });

    if loading() {
        return rsx! {
            div { class: "flex items-center justify-center py-20",
                span { class: "loading loading-spinner loading-lg" }
            }
        };
    }

    if let Some(err) = error() {
        return rsx! {
            div { class: "flex flex-col items-center justify-center py-20 gap-3",
                IconAlertTriangle { class: "w-12 h-12 text-warning".to_string() }
                h3 { class: "text-lg font-semibold", "Error loading shared folders" }
                p { class: "text-base-content/60", "{err}" }
            }
        };
    }

    let share_list = shares();

    rsx! {
        div { class: "max-w-4xl mx-auto",
            h2 { class: "text-2xl font-bold mb-6", "Shared with me" }

            if share_list.is_empty() {
                div { class: "flex flex-col items-center justify-center py-20 gap-3",
                    IconUsers { class: "w-12 h-12 text-base-content/30".to_string() }
                    h3 { class: "text-lg font-semibold", "No shared folders" }
                    p { class: "text-base-content/60", "No folders have been shared with you yet." }
                }
            } else {
                div { class: "grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4",
                    for share in share_list {
                        {
                            let folder_id = share.folder_id.clone();
                            let share_id_leave = share.id.clone();
                            let folder_name = share.mount_name.clone()
                                .unwrap_or_else(|| share.folder_name.clone());
                            let folder_name_leave = folder_name.clone();
                            rsx! {
                                div {
                                    class: "card bg-base-100 shadow-sm border border-base-300 hover:shadow-md transition-all cursor-pointer group",
                                    onclick: move |_| {
                                        let _ = nav.push(Route::Folder { id: folder_id.clone() });
                                    },
                                    div { class: "card-body p-4 gap-2",
                                        div { class: "flex items-start justify-between",
                                            div { class: "flex items-center gap-2 min-w-0 flex-1",
                                                IconFolder { class: "w-6 h-6 flex-shrink-0 text-base-content/60".to_string() }
                                                div { class: "min-w-0",
                                                    div { class: "font-medium truncate", "{folder_name}" }
                                                    div { class: "text-xs text-base-content/50",
                                                        "Shared by {share.owner_username}"
                                                    }
                                                }
                                            }
                                            // Leave button
                                            button {
                                                class: "btn btn-ghost btn-xs btn-circle opacity-0 group-hover:opacity-100 transition-opacity flex-shrink-0",
                                                title: "Leave share",
                                                onclick: move |e| {
                                                    e.stop_propagation();
                                                    leave_target.set(Some((share_id_leave.clone(), folder_name_leave.clone())));
                                                },
                                                IconX {}
                                            }
                                        }
                                        div {
                                            PermissionBadge { permission: share.permission }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Leave confirmation modal
            if let Some((ref share_id, ref name)) = leave_target() {
                {
                    let sid = share_id.clone();
                    rsx! {
                        div { class: "modal modal-open",
                            div { class: "modal-box max-w-sm",
                                h3 { class: "font-bold text-lg mb-2", "Leave shared folder?" }
                                p { class: "text-base-content/70",
                                    "You will no longer have access to \"{name}\"."
                                }
                                div { class: "modal-action",
                                    button {
                                        class: "btn btn-ghost",
                                        onclick: move |_| leave_target.set(None),
                                        "Cancel"
                                    }
                                    button {
                                        class: "btn btn-error",
                                        onclick: move |_| {
                                            let id = sid.clone();
                                            spawn(async move {
                                                let _ = use_folder_shares::delete_folder_share(&id).await;
                                                leave_target.set(None);
                                                let next = *refresh.peek() + 1;
                                                refresh.set(next);
                                            });
                                        },
                                        "Leave"
                                    }
                                }
                            }
                            div { class: "modal-backdrop", onclick: move |_| leave_target.set(None) }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn PermissionBadge(permission: SharePermission) -> Element {
    let (label, class) = match permission {
        SharePermission::ReadOnly => ("Read Only", "badge badge-outline badge-sm"),
        SharePermission::ReadWrite => ("Read / Write", "badge badge-primary badge-sm"),
        SharePermission::Admin => ("Admin", "badge badge-secondary badge-sm"),
    };

    rsx! {
        span { class: "{class}", "{label}" }
    }
}
