use dioxus::prelude::*;
use gloo_storage::{LocalStorage, Storage};

use crate::hooks::tauri::{self, SyncStatus};
use crate::hooks::use_search;
use crate::hooks::use_s3;
use crate::hooks::use_shopping;
use crate::state::{AuthState, ThemeState};

#[component]
pub fn Settings() -> Element {
    let is_desktop = tauri::is_tauri();

    rsx! {
        div { class: "space-y-6 max-w-2xl",
            h1 { class: "text-2xl font-bold", "Settings" }

            if is_desktop {
                SyncSection {}
                ConnectionSection {}
            } else {
                div { class: "card bg-base-100 shadow",
                    div { class: "card-body",
                        h2 { class: "card-title text-lg", "Account" }
                        p { class: "text-base-content/70 text-sm",
                            "You are signed in via the web interface. Additional settings coming soon."
                        }
                    }
                }
            }

            AppearanceSection {}
            OptionalFeaturesSection {}
            MusicSection {}
            S3AccessKeysSection {}

            {
                let auth_state = use_context::<Signal<AuthState>>();
                let search_enabled = use_context::<Signal<bool>>();
                if auth_state().is_admin() {
                    rsx! { AdminSection { search_enabled: search_enabled() } }
                } else {
                    rsx! {}
                }
            }
        }
    }
}

#[component]
fn AppearanceSection() -> Element {
    let mut theme_state = use_context::<Signal<ThemeState>>();
    let is_dark = theme_state().dark;

    rsx! {
        div { class: "card bg-base-100 shadow",
            div { class: "card-body gap-3",
                h2 { class: "card-title text-lg", "Appearance" }
                div { class: "form-control",
                    label { class: "label cursor-pointer",
                        span { class: "label-text", "Dark mode" }
                        input {
                            r#type: "checkbox",
                            class: "toggle toggle-primary",
                            checked: is_dark,
                            onchange: move |_| {
                                theme_state.write().dark = !is_dark;
                            },
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn OptionalFeaturesSection() -> Element {
    let mut auth_state = use_context::<Signal<AuthState>>();
    let mut toggling = use_signal(|| false);

    let shopping_enabled = auth_state()
        .user
        .as_ref()
        .map(|u| u.features_enabled.contains(&"shopping".to_string()))
        .unwrap_or(false);

    rsx! {
        div { class: "card bg-base-100 shadow",
            div { class: "card-body gap-4",
                h2 { class: "card-title text-lg", "Optional Features" }

                div { class: "flex items-center justify-between gap-4",
                    div {
                        p { class: "font-medium text-sm", "Shopping Lists" }
                        p { class: "text-base-content/60 text-xs mt-0.5",
                            "Manage shopping lists with a shared item catalogue."
                        }
                    }
                    div { class: "flex items-center gap-2",
                        if toggling() {
                            span { class: "loading loading-spinner loading-xs" }
                        }
                        input {
                            r#type: "checkbox",
                            class: "toggle toggle-primary",
                            checked: shopping_enabled,
                            disabled: toggling(),
                            onchange: move |_| {
                                let new_val = !shopping_enabled;
                                spawn(async move {
                                    toggling.set(true);
                                    let req = uncloud_common::UpdateFeaturesRequest {
                                        shopping: Some(new_val),
                                    };
                                    match use_shopping::update_my_features(req).await {
                                        Ok(updated_user) => {
                                            auth_state.write().user = Some(updated_user);
                                        }
                                        Err(_) => {
                                            // Silently fail — toggle will revert on next render
                                        }
                                    }
                                    toggling.set(false);
                                });
                            },
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn MusicSection() -> Element {
    let mut expand_depth = use_context::<Signal<u32>>();

    let options: &[(&str, u32)] = &[
        ("All collapsed", 0),
        ("1 level — top folders only (default)", 1),
        ("2 levels", 2),
        ("3 levels", 3),
        ("Expand all", 999),
    ];

    rsx! {
        div { class: "card bg-base-100 shadow",
            div { class: "card-body gap-4",
                h2 { class: "card-title text-lg", "Music" }

                div { class: "flex items-center justify-between gap-4",
                    div {
                        p { class: "font-medium text-sm", "Default folder tree depth" }
                        p { class: "text-base-content/60 text-xs mt-0.5",
                            "How many levels of the music folder tree are expanded by default in the sidebar."
                        }
                    }
                    select {
                        class: "select select-bordered select-sm w-64 shrink-0",
                        value: "{expand_depth()}",
                        onchange: move |evt| {
                            if let Ok(v) = evt.value().parse::<u32>() {
                                expand_depth.set(v);
                                let _ = LocalStorage::set("uncloud_music_expand_depth", &v);
                            }
                        },
                        for (label, value) in options {
                            option {
                                value: "{value}",
                                selected: expand_depth() == *value,
                                "{label}"
                            }
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// S3 Access Keys section
// ---------------------------------------------------------------------------

#[component]
fn S3AccessKeysSection() -> Element {
    let mut credentials = use_signal(Vec::<use_s3::S3CredentialResponse>::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| None::<String>);
    let mut show_create = use_signal(|| false);
    let mut new_label = use_signal(String::new);
    let mut creating = use_signal(|| false);
    let mut created_secret = use_signal(|| None::<use_s3::CreateS3CredentialResponse>);
    let mut copied = use_signal(|| false);

    // Load credentials on mount
    use_effect(move || {
        spawn(async move {
            loading.set(true);
            match use_s3::list_credentials().await {
                Ok(creds) => credentials.set(creds),
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    });

    let on_create = move |_| {
        let label = new_label().trim().to_string();
        if label.is_empty() {
            return;
        }
        spawn(async move {
            creating.set(true);
            error.set(None);
            match use_s3::create_credential(&label).await {
                Ok(resp) => {
                    created_secret.set(Some(resp));
                    new_label.set(String::new());
                    show_create.set(false);
                    // Refresh list
                    if let Ok(creds) = use_s3::list_credentials().await {
                        credentials.set(creds);
                    }
                }
                Err(e) => error.set(Some(e)),
            }
            creating.set(false);
        });
    };

    rsx! {
        div { class: "card bg-base-100 shadow",
            div { class: "card-body gap-4",
                h2 { class: "card-title text-lg", "S3 Access Keys" }
                p { class: "text-base-content/60 text-xs",
                    "Use these credentials with S3-compatible tools like s5cmd, rclone, or aws-cli. The endpoint URL is your server address with /s3 appended."
                }

                if let Some(err) = error() {
                    div { class: "alert alert-error text-sm", span { "{err}" } }
                }

                // Show newly created secret (only shown once)
                if let Some(secret) = created_secret() {
                    div { class: "alert alert-warning",
                        div { class: "flex flex-col gap-2 w-full",
                            p { class: "font-bold text-sm", "Save these credentials now — the secret will not be shown again!" }
                            div { class: "bg-base-300 text-base-content rounded p-3 font-mono text-xs space-y-1",
                                div { class: "flex gap-2",
                                    span { class: "font-semibold w-36 shrink-0", "Access Key ID:" }
                                    span { "{secret.access_key_id}" }
                                }
                                div { class: "flex gap-2",
                                    span { class: "font-semibold w-36 shrink-0", "Secret Access Key:" }
                                    span { "{secret.secret_access_key}" }
                                }
                            }
                            div { class: "flex gap-2 mt-1",
                                {
                                    let secret_ak = secret.access_key_id.clone();
                                    let secret_sk = secret.secret_access_key.clone();
                                    rsx! {
                                        button {
                                            class: "btn btn-sm btn-outline",
                                            onclick: move |_| {
                                                let text = format!(
                                                    "[default]\naws_access_key_id = {}\naws_secret_access_key = {}",
                                                    secret_ak, secret_sk
                                                );
                                                if let Some(window) = web_sys::window() {
                                                    let clipboard = window.navigator().clipboard();
                                                    let _ = clipboard.write_text(&text);
                                                    copied.set(true);
                                                }
                                            },
                                            if copied() { "Copied!" } else { "Copy as credentials file" }
                                        }
                                        button {
                                            class: "btn btn-sm btn-ghost",
                                            onclick: move |_| {
                                                created_secret.set(None);
                                                copied.set(false);
                                            },
                                            "Dismiss"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Existing credentials list
                if loading() {
                    div { class: "flex justify-center py-4",
                        span { class: "loading loading-spinner loading-sm" }
                    }
                } else if credentials().is_empty() {
                    p { class: "text-base-content/50 text-sm py-2",
                        "No S3 access keys yet."
                    }
                } else {
                    div { class: "overflow-x-auto",
                        table { class: "table table-sm",
                            thead {
                                tr {
                                    th { "Label" }
                                    th { "Access Key ID" }
                                    th { "Created" }
                                    th {}
                                }
                            }
                            tbody {
                                for cred in credentials() {
                                    {
                                        let cred_id = cred.id.clone();
                                        rsx! {
                                            tr {
                                                td { class: "font-medium", "{cred.label}" }
                                                td { class: "font-mono text-xs", "{cred.access_key_id}" }
                                                td { class: "text-xs text-base-content/60",
                                                    "{cred.created_at.split('T').next().unwrap_or(&cred.created_at)}"
                                                }
                                                td {
                                                    button {
                                                        class: "btn btn-ghost btn-xs text-error",
                                                        onclick: move |_| {
                                                            let id = cred_id.clone();
                                                            spawn(async move {
                                                                if use_s3::delete_credential(&id).await.is_ok() {
                                                                    if let Ok(creds) = use_s3::list_credentials().await {
                                                                        credentials.set(creds);
                                                                    }
                                                                }
                                                            });
                                                        },
                                                        "Revoke"
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

                // Create new key form
                if show_create() {
                    div { class: "flex items-end gap-2",
                        div { class: "form-control flex-1",
                            label { class: "label",
                                span { class: "label-text text-sm", "Label" }
                            }
                            input {
                                class: "input input-bordered input-sm",
                                placeholder: "e.g. rclone, s5cmd, backup script",
                                value: "{new_label()}",
                                oninput: move |evt| new_label.set(evt.value()),
                            }
                        }
                        button {
                            class: "btn btn-primary btn-sm",
                            disabled: creating() || new_label().trim().is_empty(),
                            onclick: on_create,
                            if creating() {
                                span { class: "loading loading-spinner loading-xs" }
                            }
                            "Generate"
                        }
                        button {
                            class: "btn btn-ghost btn-sm",
                            onclick: move |_| show_create.set(false),
                            "Cancel"
                        }
                    }
                } else {
                    div { class: "card-actions",
                        button {
                            class: "btn btn-sm btn-outline",
                            onclick: move |_| show_create.set(true),
                            "Generate new key"
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Existing sections (unchanged)
// ---------------------------------------------------------------------------

#[component]
fn SyncSection() -> Element {
    let mut status = use_signal(|| None::<SyncStatus>);
    let mut sync_loading = use_signal(|| false);
    let mut sync_msg = use_signal(|| None::<(bool, String)>); // (ok, message)

    // Load status on mount.
    use_effect(move || {
        spawn(async move {
            if let Ok(s) = tauri::get_status().await {
                status.set(Some(s));
            }
        });
    });

    let on_sync_now = move |_| {
        spawn(async move {
            sync_loading.set(true);
            sync_msg.set(None);
            match tauri::sync_now().await {
                Ok(()) => {
                    sync_msg.set(Some((true, "Sync complete.".to_string())));
                    if let Ok(s) = tauri::get_status().await {
                        status.set(Some(s));
                    }
                }
                Err(e) => sync_msg.set(Some((false, e))),
            }
            sync_loading.set(false);
        });
    };

    let status_badge = match status() {
        Some(SyncStatus::Idle { ref last_sync }) => rsx! {
            div { class: "flex items-center gap-2",
                span { class: "badge badge-success badge-sm" }
                span { class: "text-sm text-base-content/70", "Up to date · {last_sync}" }
            }
        },
        Some(SyncStatus::Syncing) => rsx! {
            div { class: "flex items-center gap-2",
                span { class: "loading loading-spinner loading-xs text-warning" }
                span { class: "text-sm text-base-content/70", "Syncing…" }
            }
        },
        Some(SyncStatus::Error { ref message }) => rsx! {
            div { class: "flex items-center gap-2",
                span { class: "badge badge-error badge-sm" }
                span { class: "text-sm text-error", "{message}" }
            }
        },
        _ => rsx! {
            span { class: "text-sm text-base-content/50", "Not configured" }
        },
    };

    rsx! {
        div { class: "card bg-base-100 shadow",
            div { class: "card-body gap-4",
                h2 { class: "card-title text-lg", "Sync" }

                div { class: "flex items-center justify-between",
                    div { {status_badge} }
                    button {
                        class: "btn btn-primary btn-sm",
                        disabled: sync_loading(),
                        onclick: on_sync_now,
                        if sync_loading() {
                            span { class: "loading loading-spinner loading-xs" }
                            "Syncing…"
                        } else {
                            "Sync Now"
                        }
                    }
                }

                if let Some((ok, msg)) = sync_msg() {
                    div {
                        class: if ok { "alert alert-success text-sm" } else { "alert alert-error text-sm" },
                        span { "{msg}" }
                    }
                }

                p { class: "text-xs text-base-content/50",
                    "Sync runs automatically every 60 seconds in the background."
                }
            }
        }
    }
}

#[component]
fn ConnectionSection() -> Element {
    let mut auth_state = use_context::<Signal<AuthState>>();
    let nav = use_navigator();
    let mut config = use_signal(|| None::<(String, String, String)>); // (server_url, username, root_path)
    let mut disconnect_loading = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    use_effect(move || {
        spawn(async move {
            if let Some(cfg) = tauri::get_config().await {
                config.set(Some((cfg.server_url, cfg.username, cfg.root_path)));
            }
        });
    });

    let on_disconnect = move |_| {
        spawn(async move {
            disconnect_loading.set(true);
            error.set(None);
            match tauri::disconnect().await {
                Ok(()) => {
                    auth_state.write().user = None;
                    tauri::mark_needs_setup();
                    nav.push("/setup");
                }
                Err(e) => error.set(Some(e)),
            }
            disconnect_loading.set(false);
        });
    };

    rsx! {
        div { class: "card bg-base-100 shadow",
            div { class: "card-body gap-4",
                h2 { class: "card-title text-lg", "Connection" }

                if let Some((server_url, username, root_path)) = config() {
                    div { class: "space-y-2 text-sm",
                        div { class: "flex gap-2",
                            span { class: "font-medium w-24 shrink-0", "Server" }
                            span { class: "text-base-content/70 break-all", "{server_url}" }
                        }
                        div { class: "flex gap-2",
                            span { class: "font-medium w-24 shrink-0", "Username" }
                            span { class: "text-base-content/70", "{username}" }
                        }
                        div { class: "flex gap-2",
                            span { class: "font-medium w-24 shrink-0", "Sync folder" }
                            span { class: "text-base-content/70 break-all font-mono text-xs", "{root_path}" }
                        }
                    }
                } else {
                    p { class: "text-base-content/50 text-sm", "Loading…" }
                }

                if let Some(err) = error() {
                    div { class: "alert alert-error text-sm", span { "{err}" } }
                }

                div { class: "card-actions",
                    button {
                        class: "btn btn-error btn-sm",
                        disabled: disconnect_loading(),
                        onclick: on_disconnect,
                        if disconnect_loading() {
                            span { class: "loading loading-spinner loading-xs" }
                        }
                        "Disconnect"
                    }
                }
            }
        }
    }
}

#[component]
fn AdminSection(search_enabled: bool) -> Element {
    let mut reindex_loading = use_signal(|| false);
    let mut reindex_msg: Signal<Option<(bool, String)>> = use_signal(|| None);

    let on_reindex = move |_| {
        spawn(async move {
            reindex_loading.set(true);
            reindex_msg.set(None);
            match use_search::trigger_reindex().await {
                Ok(()) => reindex_msg.set(Some((true, "Reindex started in the background.".to_string()))),
                Err(e) => reindex_msg.set(Some((false, e))),
            }
            reindex_loading.set(false);
        });
    };

    rsx! {
        div { class: "card bg-base-100 shadow",
            div { class: "card-body gap-4",
                h2 { class: "card-title text-lg", "Admin" }

                div { class: "flex items-center justify-between gap-4",
                    div {
                        p { class: "font-medium text-sm", "Search index" }
                        p { class: "text-base-content/60 text-xs mt-0.5",
                            if search_enabled {
                                "Re-index all files in Meilisearch. Useful after enabling search on an existing library or recovering from Meilisearch downtime."
                            } else {
                                "Search is not enabled. Set search.enabled: true in config.yaml and restart the server."
                            }
                        }
                    }
                    button {
                        class: "btn btn-sm btn-outline shrink-0",
                        disabled: reindex_loading() || !search_enabled,
                        onclick: on_reindex,
                        if reindex_loading() {
                            span { class: "loading loading-spinner loading-xs" }
                        }
                        "Reindex"
                    }
                }

                if let Some((ok, msg)) = reindex_msg() {
                    div {
                        class: if ok { "alert alert-success text-sm" } else { "alert alert-error text-sm" },
                        span { "{msg}" }
                    }
                }
            }
        }
    }
}
