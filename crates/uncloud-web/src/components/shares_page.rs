use dioxus::prelude::*;
use uncloud_common::{FolderShareResponse, ServerEvent, ShareResourceType, ShareResponse};
use crate::hooks::{use_folder_shares, use_shares};
use crate::components::shared_with_me::PermissionBadge;
use crate::router::Route;

#[derive(Debug, Clone, Copy, PartialEq)]
enum ShareTab {
    Links,
    SharedByMe,
    SharedWithMe,
}

#[component]
pub fn SharesPage() -> Element {
    let mut active_tab = use_signal(|| ShareTab::Links);

    rsx! {
        div { class: "max-w-4xl mx-auto",
            // Tabs
            div { role: "tablist", class: "tabs tabs-bordered mb-6",
                button {
                    role: "tab",
                    class: if active_tab() == ShareTab::Links { "tab tab-active" } else { "tab" },
                    onclick: move |_| active_tab.set(ShareTab::Links),
                    "Share Links"
                }
                button {
                    role: "tab",
                    class: if active_tab() == ShareTab::SharedByMe { "tab tab-active" } else { "tab" },
                    onclick: move |_| active_tab.set(ShareTab::SharedByMe),
                    "Shared by me"
                }
                button {
                    role: "tab",
                    class: if active_tab() == ShareTab::SharedWithMe { "tab tab-active" } else { "tab" },
                    onclick: move |_| active_tab.set(ShareTab::SharedWithMe),
                    "Shared with me"
                }
            }

            match active_tab() {
                ShareTab::Links => rsx! { ShareLinksPanel {} },
                ShareTab::SharedByMe => rsx! { SharedByMePanel {} },
                ShareTab::SharedWithMe => rsx! { SharedWithMePanel {} },
            }
        }
    }
}

#[component]
fn ShareLinksPanel() -> Element {
    let mut shares: Signal<Vec<ShareResponse>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut refresh = use_signal(|| 0u32);
    let mut delete_target: Signal<Option<(String, String)>> = use_signal(|| None);
    let mut copied_id: Signal<Option<String>> = use_signal(|| None);

    use_effect(move || {
        let _ = refresh();
        spawn(async move {
            error.set(None);
            match use_shares::list_shares().await {
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
                div { class: "text-5xl", "⚠" }
                h3 { class: "text-lg font-semibold", "Error loading share links" }
                p { class: "text-base-content/60", "{err}" }
            }
        };
    }

    let share_list = shares();

    rsx! {
        if share_list.is_empty() {
            div { class: "flex flex-col items-center justify-center py-20 gap-3",
                div { class: "text-5xl", "🔗" }
                h3 { class: "text-lg font-semibold", "No share links" }
                p { class: "text-base-content/60", "Share links you create will appear here." }
            }
        } else {
            div { class: "flex flex-col gap-3",
                for share in share_list {
                    {
                        let share_id = share.id.clone();
                        let share_id_del = share.id.clone();
                        let share_token = share.token.clone();
                        let share_token_copy = share.token.clone();
                        let share_id_copy = share.id.clone();
                        let is_copied = copied_id() == Some(share_id.clone());

                        let type_icon = match share.resource_type {
                            ShareResourceType::File => "📄",
                            ShareResourceType::Folder => "📁",
                        };
                        let resource_name = share.resource_name.clone();

                        rsx! {
                            div { class: "card bg-base-100 shadow-sm border border-base-300",
                                div { class: "card-body p-4",
                                    div { class: "flex items-start justify-between gap-3",
                                        div { class: "flex-1 min-w-0",
                                            // Name + type row
                                            div { class: "flex items-center gap-2 mb-2",
                                                span { class: "text-xl", "{type_icon}" }
                                                span { class: "font-medium truncate", "{resource_name}" }
                                                if share.has_password {
                                                    span { class: "badge badge-outline badge-sm", "🔒" }
                                                }
                                            }

                                            // Link
                                            div { class: "flex items-center gap-2 mb-2",
                                                code { class: "text-sm text-base-content/50 truncate block flex-1 min-w-0",
                                                    "/share/{share_token}"
                                                }
                                                button {
                                                    class: if is_copied { "btn btn-ghost btn-xs text-success" } else { "btn btn-ghost btn-xs" },
                                                    title: "Copy link",
                                                    onclick: move |_| {
                                                        let token = share_token_copy.clone();
                                                        let id = share_id_copy.clone();
                                                        spawn(async move {
                                                            let origin = web_sys::window()
                                                                .and_then(|w| w.location().origin().ok())
                                                                .unwrap_or_default();
                                                            let url = format!("{}/share/{}", origin, token);
                                                            if let Some(window) = web_sys::window() {
                                                                let clipboard = window.navigator().clipboard();
                                                                let _ = wasm_bindgen_futures::JsFuture::from(
                                                                    clipboard.write_text(&url)
                                                                ).await;
                                                                copied_id.set(Some(id));
                                                                gloo_timers::future::TimeoutFuture::new(2000).await;
                                                                copied_id.set(None);
                                                            }
                                                        });
                                                    },
                                                    if is_copied { "✓" } else { "📋" }
                                                }
                                            }

                                            // Details row
                                            div { class: "flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-base-content/50",
                                                if let Some(ref expires) = share.expires_at {
                                                    span { "Expires: {expires}" }
                                                }
                                                {
                                                    let dl_text = if let Some(max) = share.max_downloads {
                                                        format!("Downloads: {} / {}", share.download_count, max)
                                                    } else {
                                                        format!("Downloads: {}", share.download_count)
                                                    };
                                                    rsx! { span { "{dl_text}" } }
                                                }
                                                span { "Created: {share.created_at}" }
                                            }
                                        }

                                        // Delete button
                                        button {
                                            class: "btn btn-ghost btn-xs btn-circle text-error flex-shrink-0",
                                            title: "Delete share link",
                                            onclick: move |_| {
                                                delete_target.set(Some((share_id_del.clone(), share_token.clone())));
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
        }

        // Delete confirmation modal
        if let Some((ref del_id, ref del_token)) = delete_target() {
            {
                let did = del_id.clone();
                rsx! {
                    div { class: "modal modal-open",
                        div { class: "modal-box max-w-sm",
                            h3 { class: "font-bold text-lg mb-2", "Delete share link?" }
                            p { class: "text-base-content/70",
                                "This will permanently remove the share link for token "
                                code { "{del_token}" }
                                ". Anyone with this link will lose access."
                            }
                            div { class: "modal-action",
                                button {
                                    class: "btn btn-ghost",
                                    onclick: move |_| delete_target.set(None),
                                    "Cancel"
                                }
                                button {
                                    class: "btn btn-error",
                                    onclick: move |_| {
                                        let id = did.clone();
                                        spawn(async move {
                                            let _ = use_shares::delete_share(&id).await;
                                            delete_target.set(None);
                                            let next = *refresh.peek() + 1;
                                            refresh.set(next);
                                        });
                                    },
                                    "Delete"
                                }
                            }
                        }
                        div { class: "modal-backdrop", onclick: move |_| delete_target.set(None) }
                    }
                }
            }
        }
    }
}

#[component]
fn SharedByMePanel() -> Element {
    let mut shares: Signal<Vec<FolderShareResponse>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut refresh = use_signal(|| 0u32);
    let nav = use_navigator();

    let mut revoke_target: Signal<Option<(String, String, String)>> = use_signal(|| None);

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
            match use_folder_shares::list_shares_by_me().await {
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
                div { class: "text-5xl", "⚠" }
                h3 { class: "text-lg font-semibold", "Error loading shares" }
                p { class: "text-base-content/60", "{err}" }
            }
        };
    }

    let share_list = shares();

    rsx! {
        if share_list.is_empty() {
            div { class: "flex flex-col items-center justify-center py-20 gap-3",
                div { class: "text-5xl", "👥" }
                h3 { class: "text-lg font-semibold", "No shared folders" }
                p { class: "text-base-content/60", "You haven't shared any folders with other users yet." }
            }
        } else {
            div { class: "grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4",
                for share in share_list {
                    {
                        let folder_id = share.folder_id.clone();
                        let share_id_revoke = share.id.clone();
                        let folder_name = share.folder_name.clone();
                        let grantee = share.grantee_username.clone();
                        let folder_name_revoke = folder_name.clone();
                        let grantee_revoke = grantee.clone();
                        rsx! {
                            div {
                                class: "card bg-base-100 shadow-sm border border-base-300 hover:shadow-md transition-all cursor-pointer group",
                                onclick: move |_| {
                                    let _ = nav.push(Route::Folder { id: folder_id.clone() });
                                },
                                div { class: "card-body p-4 gap-2",
                                    div { class: "flex items-start justify-between",
                                        div { class: "flex items-center gap-2 min-w-0 flex-1",
                                            span { class: "text-2xl flex-shrink-0", "📁" }
                                            div { class: "min-w-0",
                                                div { class: "font-medium truncate", "{folder_name}" }
                                                div { class: "text-xs text-base-content/50",
                                                    "Shared with {grantee}"
                                                }
                                            }
                                        }
                                        button {
                                            class: "btn btn-ghost btn-xs btn-circle opacity-0 group-hover:opacity-100 transition-opacity flex-shrink-0",
                                            title: "Revoke share",
                                            onclick: move |e| {
                                                e.stop_propagation();
                                                revoke_target.set(Some((share_id_revoke.clone(), folder_name_revoke.clone(), grantee_revoke.clone())));
                                            },
                                            "✕"
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

        // Revoke confirmation modal
        if let Some((ref share_id, ref name, ref grantee)) = revoke_target() {
            {
                let sid = share_id.clone();
                rsx! {
                    div { class: "modal modal-open",
                        div { class: "modal-box max-w-sm",
                            h3 { class: "font-bold text-lg mb-2", "Revoke share?" }
                            p { class: "text-base-content/70",
                                "\"{name}\" will no longer be shared with {grantee}."
                            }
                            div { class: "modal-action",
                                button {
                                    class: "btn btn-ghost",
                                    onclick: move |_| revoke_target.set(None),
                                    "Cancel"
                                }
                                button {
                                    class: "btn btn-error",
                                    onclick: move |_| {
                                        let id = sid.clone();
                                        spawn(async move {
                                            let _ = use_folder_shares::delete_folder_share(&id).await;
                                            revoke_target.set(None);
                                            let next = *refresh.peek() + 1;
                                            refresh.set(next);
                                        });
                                    },
                                    "Revoke"
                                }
                            }
                        }
                        div { class: "modal-backdrop", onclick: move |_| revoke_target.set(None) }
                    }
                }
            }
        }
    }
}

#[component]
pub fn SharedWithMePanel() -> Element {
    let mut shares: Signal<Vec<FolderShareResponse>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut refresh = use_signal(|| 0u32);
    let nav = use_navigator();

    let mut leave_target: Signal<Option<(String, String)>> = use_signal(|| None);

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
                div { class: "text-5xl", "⚠" }
                h3 { class: "text-lg font-semibold", "Error loading shared folders" }
                p { class: "text-base-content/60", "{err}" }
            }
        };
    }

    let share_list = shares();

    rsx! {
        if share_list.is_empty() {
            div { class: "flex flex-col items-center justify-center py-20 gap-3",
                div { class: "text-5xl", "👥" }
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
                                            span { class: "text-2xl flex-shrink-0", "📁" }
                                            div { class: "min-w-0",
                                                div { class: "font-medium truncate", "{folder_name}" }
                                                div { class: "text-xs text-base-content/50",
                                                    "Shared by {share.owner_username}"
                                                }
                                            }
                                        }
                                        button {
                                            class: "btn btn-ghost btn-xs btn-circle opacity-0 group-hover:opacity-100 transition-opacity flex-shrink-0",
                                            title: "Leave share",
                                            onclick: move |e| {
                                                e.stop_propagation();
                                                leave_target.set(Some((share_id_leave.clone(), folder_name_leave.clone())));
                                            },
                                            "✕"
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
