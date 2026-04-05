use dioxus::prelude::*;
use gloo_storage::{LocalStorage, Storage};

use crate::hooks::tauri::{self, SyncStatus};
use uncloud_common::{CreateInviteRequest, UserRole, UserStatus};
use crate::hooks::use_auth;
use crate::hooks::use_search;
use crate::hooks::use_s3;
use crate::hooks::use_shopping;
use crate::state::{AuthState, ThemeState};

#[component]
pub fn SettingsPage(tab: String) -> Element {
    let is_desktop = tauri::is_tauri();
    let auth_state = use_context::<Signal<AuthState>>();
    let search_enabled = use_context::<Signal<bool>>();
    let is_admin = auth_state().is_admin();

    rsx! {
        div { class: "space-y-6 max-w-2xl",
            match tab.as_str() {
                "account" => rsx! {
                    h1 { class: "text-2xl font-bold", "Account" }
                    if is_desktop {
                        SyncSection {}
                        ConnectionSection {}
                    }
                    TotpSection {}
                    S3AccessKeysSection {}
                },
                "preferences" => rsx! {
                    h1 { class: "text-2xl font-bold", "Preferences" }
                    AppearanceSection {}
                    OptionalFeaturesSection {}
                    MusicSection {}
                },
                "users" if is_admin => rsx! {
                    h1 { class: "text-2xl font-bold", "Users" }
                    UserManagementSection {}
                    UsersTabInvites {}
                },
                "admin" if is_admin => rsx! {
                    h1 { class: "text-2xl font-bold", "Admin" }
                    AdminSection { search_enabled: search_enabled() }
                },
                _ => rsx! {
                    h1 { class: "text-2xl font-bold", "Settings" }
                    p { class: "text-base-content/60", "Page not found." }
                },
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

// ---------------------------------------------------------------------------
// TOTP (Two-Factor Authentication) section
// ---------------------------------------------------------------------------

#[component]
fn TotpSection() -> Element {
    let auth_state = use_context::<Signal<AuthState>>();
    let totp_enabled = auth_state()
        .user
        .as_ref()
        .map(|u| u.totp_enabled)
        .unwrap_or(false);

    let mut setup_data = use_signal(|| None::<uncloud_common::TotpSetupResponse>);
    let mut confirm_code = use_signal(String::new);
    let mut disable_code = use_signal(String::new);
    let mut loading = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut success = use_signal(|| None::<String>);
    let mut recovery_codes = use_signal(|| None::<Vec<String>>);
    let mut show_disable = use_signal(|| false);

    let on_setup = move |_| {
        spawn(async move {
            loading.set(true);
            error.set(None);
            match use_auth::totp_setup().await {
                Ok(data) => setup_data.set(Some(data)),
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    };

    let on_enable = move |evt: Event<FormData>| {
        evt.prevent_default();
        let code = confirm_code();
        spawn(async move {
            loading.set(true);
            error.set(None);
            match use_auth::totp_enable(&code).await {
                Ok(()) => {
                    if let Some(data) = setup_data() {
                        recovery_codes.set(Some(data.recovery_codes));
                    }
                    setup_data.set(None);
                    confirm_code.set(String::new());
                    success.set(Some("Two-factor authentication enabled.".to_string()));
                    if let Ok(user) = use_auth::me().await {
                        use_context::<Signal<AuthState>>().write().user = Some(user);
                    }
                }
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    };

    let on_disable = move |evt: Event<FormData>| {
        evt.prevent_default();
        let code = disable_code();
        spawn(async move {
            loading.set(true);
            error.set(None);
            match use_auth::totp_disable(&code).await {
                Ok(()) => {
                    show_disable.set(false);
                    disable_code.set(String::new());
                    success.set(Some("Two-factor authentication disabled.".to_string()));
                    if let Ok(user) = use_auth::me().await {
                        use_context::<Signal<AuthState>>().write().user = Some(user);
                    }
                }
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    };

    rsx! {
        div { class: "card bg-base-100 shadow",
            div { class: "card-body gap-4",
                h2 { class: "card-title text-lg", "Two-Factor Authentication" }

                if let Some(err) = error() {
                    div { class: "alert alert-error text-sm", span { "{err}" } }
                }
                if let Some(msg) = success() {
                    div { class: "alert alert-success text-sm", span { "{msg}" } }
                }

                // Show recovery codes after enabling
                if let Some(codes) = recovery_codes() {
                    div { class: "alert alert-warning",
                        div { class: "flex flex-col gap-2 w-full",
                            p { class: "font-bold text-sm", "Save your recovery codes — they will not be shown again!" }
                            div { class: "bg-base-300 text-base-content rounded p-3 font-mono text-xs grid grid-cols-2 gap-1",
                                for code in codes.iter() {
                                    span { "{code}" }
                                }
                            }
                            button {
                                class: "btn btn-sm btn-ghost mt-1 self-end",
                                onclick: move |_| recovery_codes.set(None),
                                "I've saved them"
                            }
                        }
                    }
                }

                if totp_enabled {
                    div { class: "flex items-center justify-between",
                        div { class: "flex items-center gap-2",
                            span { class: "badge badge-success badge-sm" }
                            span { class: "text-sm", "Two-factor authentication is enabled" }
                        }
                        if !show_disable() {
                            button {
                                class: "btn btn-sm btn-error btn-outline",
                                onclick: move |_| show_disable.set(true),
                                "Disable"
                            }
                        }
                    }
                    if show_disable() {
                        form { class: "flex items-end gap-2 mt-2", onsubmit: on_disable,
                            div { class: "form-control flex-1",
                                label { class: "label",
                                    span { class: "label-text text-sm", "Enter TOTP code to confirm" }
                                }
                                input {
                                    class: "input input-bordered input-sm",
                                    r#type: "text",
                                    inputmode: "numeric",
                                    placeholder: "000000",
                                    value: "{disable_code}",
                                    oninput: move |evt| disable_code.set(evt.value()),
                                    required: true,
                                }
                            }
                            button {
                                class: "btn btn-error btn-sm",
                                r#type: "submit",
                                disabled: loading(),
                                if loading() {
                                    span { class: "loading loading-spinner loading-xs" }
                                }
                                "Confirm disable"
                            }
                            button {
                                class: "btn btn-ghost btn-sm",
                                r#type: "button",
                                onclick: move |_| { show_disable.set(false); disable_code.set(String::new()); },
                                "Cancel"
                            }
                        }
                    }
                } else if let Some(data) = setup_data() {
                    div { class: "flex flex-col items-center gap-3",
                        p { class: "text-sm text-base-content/70 text-center",
                            "Scan this QR code with your authenticator app, then enter the 6-digit code to confirm."
                        }
                        div {
                            class: "p-4 bg-white rounded-lg",
                            dangerous_inner_html: "{data.qr_svg}",
                        }
                        form { class: "flex items-end gap-2 w-full", onsubmit: on_enable,
                            div { class: "form-control flex-1",
                                label { class: "label",
                                    span { class: "label-text text-sm", "Verification code" }
                                }
                                input {
                                    class: "input input-bordered input-sm text-center tracking-widest",
                                    r#type: "text",
                                    inputmode: "numeric",
                                    maxlength: "6",
                                    placeholder: "000000",
                                    value: "{confirm_code}",
                                    oninput: move |evt| confirm_code.set(evt.value()),
                                    required: true,
                                }
                            }
                            button {
                                class: "btn btn-primary btn-sm",
                                r#type: "submit",
                                disabled: loading(),
                                if loading() {
                                    span { class: "loading loading-spinner loading-xs" }
                                }
                                "Enable"
                            }
                            button {
                                class: "btn btn-ghost btn-sm",
                                r#type: "button",
                                onclick: move |_| { setup_data.set(None); confirm_code.set(String::new()); },
                                "Cancel"
                            }
                        }
                    }
                } else {
                    div { class: "flex items-center justify-between",
                        p { class: "text-sm text-base-content/70",
                            "Add an extra layer of security to your account with a TOTP authenticator app."
                        }
                        button {
                            class: "btn btn-sm btn-outline shrink-0",
                            disabled: loading(),
                            onclick: on_setup,
                            if loading() {
                                span { class: "loading loading-spinner loading-xs" }
                            }
                            "Set up"
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Invite Management section (admin only)
// ---------------------------------------------------------------------------

#[component]
fn InviteManagementSection() -> Element {
    let mut invites = use_signal(Vec::<uncloud_common::InviteResponse>::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| None::<String>);
    let mut show_create = use_signal(|| false);
    let mut invite_comment = use_signal(String::new);
    let mut invite_role = use_signal(|| "user".to_string());
    let mut creating = use_signal(|| false);
    let mut created_link = use_signal(|| None::<String>);
    let mut copied = use_signal(|| false);

    // Load invites on mount
    use_effect(move || {
        spawn(async move {
            loading.set(true);
            match use_auth::list_invites().await {
                Ok(list) => invites.set(list),
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    });

    let on_create = move |evt: Event<FormData>| {
        evt.prevent_default();
        let comment = {
            let c = invite_comment().trim().to_string();
            if c.is_empty() { None } else { Some(c) }
        };
        let role = match invite_role().as_str() {
            "admin" => Some(UserRole::Admin),
            _ => None,
        };
        spawn(async move {
            creating.set(true);
            error.set(None);
            let req = CreateInviteRequest {
                comment,
                role,
                expires_in_hours: Some(72),
            };
            match use_auth::create_invite(req).await {
                Ok(invite) => {
                    let base = web_sys::window()
                        .and_then(|w| w.location().origin().ok())
                        .unwrap_or_default();
                    let link = format!("{}/invite/{}", base, invite.token);
                    created_link.set(Some(link));
                    invite_comment.set(String::new());
                    invite_role.set("user".to_string());
                    show_create.set(false);
                    if let Ok(list) = use_auth::list_invites().await {
                        invites.set(list);
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
                h2 { class: "card-title text-lg", "Invites" }

                if let Some(err) = error() {
                    div { class: "alert alert-error text-sm", span { "{err}" } }
                }

                // Show created invite link
                if let Some(link) = created_link() {
                    div { class: "alert alert-success",
                        div { class: "flex flex-col gap-2 w-full",
                            p { class: "font-bold text-sm", "Invite link created — share it with the user:" }
                            div { class: "bg-base-300 text-base-content rounded p-2 font-mono text-xs break-all",
                                "{link}"
                            }
                            div { class: "flex gap-2 mt-1",
                                {
                                    let link_copy = link.clone();
                                    rsx! {
                                        button {
                                            class: "btn btn-sm btn-outline",
                                            onclick: move |_| {
                                                if let Some(window) = web_sys::window() {
                                                    let clipboard = window.navigator().clipboard();
                                                    let _ = clipboard.write_text(&link_copy);
                                                    copied.set(true);
                                                }
                                            },
                                            if copied() { "Copied!" } else { "Copy link" }
                                        }
                                        button {
                                            class: "btn btn-sm btn-ghost",
                                            onclick: move |_| { created_link.set(None); copied.set(false); },
                                            "Dismiss"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Invite list
                if loading() {
                    div { class: "flex justify-center py-4",
                        span { class: "loading loading-spinner loading-sm" }
                    }
                } else if invites().is_empty() {
                    p { class: "text-base-content/50 text-sm py-2", "No invites yet." }
                } else {
                    div { class: "overflow-x-auto",
                        table { class: "table table-sm",
                            thead {
                                tr {
                                    th { "Comment" }
                                    th { "Role" }
                                    th { "Status" }
                                    th { "Created" }
                                    th {}
                                }
                            }
                            tbody {
                                for invite in invites() {
                                    {
                                        let inv_id = invite.id.clone();
                                        let used = invite.used;
                                        rsx! {
                                            tr {
                                                td { class: "text-sm",
                                                    {invite.comment.as_deref().unwrap_or("—")}
                                                }
                                                td { class: "text-xs",
                                                    {invite.role.as_ref().map(|r| format!("{:?}", r)).unwrap_or_else(|| "User".to_string())}
                                                }
                                                td {
                                                    if used {
                                                        div { class: "flex flex-col gap-0.5",
                                                            span { class: "badge badge-ghost badge-sm", "Used" }
                                                            if let Some(ref uname) = invite.used_by_username {
                                                                span { class: "text-xs text-base-content/60",
                                                                    {
                                                                        let display = match &invite.used_by_email {
                                                                            Some(email) => format!("{uname} ({email})"),
                                                                            None => uname.clone(),
                                                                        };
                                                                        display
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    } else {
                                                        span { class: "badge badge-success badge-sm", "Active" }
                                                    }
                                                }
                                                td { class: "text-xs text-base-content/60",
                                                    {invite.created_at.split('T').next().unwrap_or(&invite.created_at)}
                                                }
                                                td {
                                                    button {
                                                        class: "btn btn-ghost btn-xs text-error",
                                                        onclick: move |_| {
                                                            let id = inv_id.clone();
                                                            spawn(async move {
                                                                if use_auth::delete_invite(&id).await.is_ok() {
                                                                    if let Ok(list) = use_auth::list_invites().await {
                                                                        invites.set(list);
                                                                    }
                                                                }
                                                            });
                                                        },
                                                        if used { "Delete" } else { "Revoke" }
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

                // Create invite form
                if show_create() {
                    form { class: "flex flex-col gap-2 mt-2", onsubmit: on_create,
                        div { class: "flex items-end gap-2",
                            div { class: "form-control flex-1",
                                label { class: "label",
                                    span { class: "label-text text-sm", "Comment " }
                                    span { class: "label-text-alt text-base-content/40", "optional" }
                                }
                                input {
                                    class: "input input-bordered input-sm",
                                    r#type: "text",
                                    placeholder: "e.g. For Alice, For the new team member",
                                    value: "{invite_comment()}",
                                    oninput: move |evt| invite_comment.set(evt.value()),
                                }
                            }
                            div { class: "form-control",
                                label { class: "label",
                                    span { class: "label-text text-sm", "Role" }
                                }
                                select {
                                    class: "select select-bordered select-sm",
                                    value: "{invite_role()}",
                                    onchange: move |evt| invite_role.set(evt.value()),
                                    option { value: "user", "User" }
                                    option { value: "admin", "Admin" }
                                }
                            }
                        }
                        div { class: "flex gap-2",
                            button {
                                class: "btn btn-primary btn-sm",
                                r#type: "submit",
                                disabled: creating(),
                                if creating() {
                                    span { class: "loading loading-spinner loading-xs" }
                                }
                                "Create invite"
                            }
                            button {
                                class: "btn btn-ghost btn-sm",
                                r#type: "button",
                                onclick: move |_| show_create.set(false),
                                "Cancel"
                            }
                        }
                    }
                } else {
                    div { class: "card-actions",
                        button {
                            class: "btn btn-sm btn-outline",
                            onclick: move |_| show_create.set(true),
                            "Create invite"
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Conditionally show invites (hidden when registration is disabled)
// ---------------------------------------------------------------------------

#[component]
fn UsersTabInvites() -> Element {
    let mut reg_mode = use_signal(|| None::<uncloud_common::RegistrationMode>);

    use_effect(move || {
        spawn(async move {
            if let Ok(info) = use_auth::server_info().await {
                reg_mode.set(Some(info.registration_mode));
            }
        });
    });

    match reg_mode() {
        Some(uncloud_common::RegistrationMode::Disabled) | None => rsx! {},
        _ => rsx! { InviteManagementSection {} },
    }
}

// ---------------------------------------------------------------------------
// Create User section (admin only)
// ---------------------------------------------------------------------------

#[component]
fn CreateUserSection(on_created: EventHandler) -> Element {
    let mut show_form = use_signal(|| false);
    let mut username = use_signal(String::new);
    let mut email = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut role = use_signal(|| "user".to_string());
    let mut creating = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    let on_submit = move |evt: Event<FormData>| {
        evt.prevent_default();
        let u = username().trim().to_string();
        let e = email().trim().to_string();
        let p = password().clone();
        let r = role().clone();
        spawn(async move {
            creating.set(true);
            error.set(None);
            let req = use_auth::CreateUserRequest {
                username: u,
                email: if e.is_empty() { None } else { Some(e) },
                password: p,
                role: if r == "admin" { Some(UserRole::Admin) } else { None },
            };
            match use_auth::create_user(req).await {
                Ok(()) => {
                    show_form.set(false);
                    username.set(String::new());
                    email.set(String::new());
                    password.set(String::new());
                    role.set("user".to_string());
                    on_created.call(());
                }
                Err(e) => error.set(Some(e)),
            }
            creating.set(false);
        });
    };

    if !show_form() {
        return rsx! {
            div { class: "card-actions",
                button {
                    class: "btn btn-sm btn-outline",
                    onclick: move |_| show_form.set(true),
                    "Create user"
                }
            }
        };
    }

    rsx! {
        div { class: "card bg-base-100 shadow",
            div { class: "card-body gap-3",
                h3 { class: "font-semibold text-sm", "Create user" }

                if let Some(err) = error() {
                    div { class: "alert alert-error text-sm", span { "{err}" } }
                }

                form { class: "flex flex-col gap-2", onsubmit: on_submit,
                    div { class: "flex gap-2",
                        div { class: "form-control flex-1",
                            label { class: "label",
                                span { class: "label-text text-sm", "Username" }
                            }
                            input {
                                class: "input input-bordered input-sm",
                                r#type: "text",
                                placeholder: "username",
                                value: "{username()}",
                                oninput: move |evt| username.set(evt.value()),
                                required: true,
                            }
                        }
                        div { class: "form-control flex-1",
                            label { class: "label",
                                span { class: "label-text text-sm", "Email " }
                                span { class: "label-text-alt text-base-content/40", "optional" }
                            }
                            input {
                                class: "input input-bordered input-sm",
                                r#type: "email",
                                placeholder: "user@example.com",
                                value: "{email()}",
                                oninput: move |evt| email.set(evt.value()),
                            }
                        }
                    }
                    div { class: "flex gap-2",
                        div { class: "form-control flex-1",
                            label { class: "label",
                                span { class: "label-text text-sm", "Password" }
                            }
                            input {
                                class: "input input-bordered input-sm",
                                r#type: "password",
                                placeholder: "min. 8 characters",
                                value: "{password()}",
                                oninput: move |evt| password.set(evt.value()),
                                required: true,
                            }
                        }
                        div { class: "form-control",
                            label { class: "label",
                                span { class: "label-text text-sm", "Role" }
                            }
                            select {
                                class: "select select-bordered select-sm",
                                value: "{role()}",
                                onchange: move |evt| role.set(evt.value()),
                                option { value: "user", "User" }
                                option { value: "admin", "Admin" }
                            }
                        }
                    }
                    div { class: "flex gap-2 mt-1",
                        button {
                            class: "btn btn-primary btn-sm",
                            r#type: "submit",
                            disabled: creating(),
                            if creating() {
                                span { class: "loading loading-spinner loading-xs" }
                            }
                            "Create"
                        }
                        button {
                            class: "btn btn-ghost btn-sm",
                            r#type: "button",
                            onclick: move |_| { show_form.set(false); error.set(None); },
                            "Cancel"
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// User Management section (admin only)
// ---------------------------------------------------------------------------

#[component]
fn UserManagementSection() -> Element {
    let mut users = use_signal(Vec::<use_auth::AdminUserResponse>::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| None::<String>);
    let mut action_loading = use_signal(|| None::<String>);

    let refresh_users = move || {
        spawn(async move {
            match use_auth::list_users().await {
                Ok(list) => users.set(list),
                Err(e) => error.set(Some(e)),
            }
        });
    };

    use_effect(move || {
        spawn(async move {
            loading.set(true);
            match use_auth::list_users().await {
                Ok(list) => users.set(list),
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    });

    rsx! {
        div { class: "card bg-base-100 shadow",
            div { class: "card-body gap-4",
                h2 { class: "card-title text-lg", "Users" }

                if let Some(err) = error() {
                    div { class: "alert alert-error text-sm", span { "{err}" } }
                }

                if loading() {
                    div { class: "flex justify-center py-4",
                        span { class: "loading loading-spinner loading-sm" }
                    }
                } else if users().is_empty() {
                    p { class: "text-base-content/50 text-sm py-2", "No users found." }
                } else {
                    div { class: "overflow-x-auto",
                        table { class: "table table-sm",
                            thead {
                                tr {
                                    th { "Username" }
                                    th { "Email" }
                                    th { "Role" }
                                    th { "Status" }
                                    th { "2FA" }
                                    th { "Actions" }
                                }
                            }
                            tbody {
                                for user in users() {
                                    {
                                        let uid = user.id.clone();
                                        let uid_approve = uid.clone();
                                        let uid_enable = uid.clone();
                                        let uid_disable = uid.clone();
                                        let uid_totp = uid.clone();
                                        let is_acting = action_loading() == Some(uid.clone());
                                        rsx! {
                                            tr {
                                                td { class: "font-medium text-sm", "{user.username}" }
                                                td { class: "text-xs text-base-content/70",
                                                    {user.email.as_deref().unwrap_or("—")}
                                                }
                                                td {
                                                    match user.role {
                                                        UserRole::Admin => rsx! { span { class: "badge badge-primary badge-sm", "Admin" } },
                                                        UserRole::User => rsx! { span { class: "badge badge-ghost badge-sm", "User" } },
                                                    }
                                                }
                                                td {
                                                    match user.status {
                                                        UserStatus::Active => rsx! { span { class: "badge badge-success badge-sm", "Active" } },
                                                        UserStatus::Pending => rsx! { span { class: "badge badge-warning badge-sm", "Pending" } },
                                                        UserStatus::Disabled => rsx! { span { class: "badge badge-error badge-sm", "Disabled" } },
                                                    }
                                                }
                                                td {
                                                    if user.totp_enabled {
                                                        span { class: "text-success text-xs", "Enabled" }
                                                    } else {
                                                        span { class: "text-base-content/40 text-xs", "Off" }
                                                    }
                                                }
                                                td { class: "flex gap-1 flex-wrap",
                                                    if is_acting {
                                                        span { class: "loading loading-spinner loading-xs" }
                                                    } else {
                                                        if user.status == UserStatus::Pending {
                                                            button {
                                                                class: "btn btn-success btn-xs",
                                                                onclick: move |_| {
                                                                    let id = uid_approve.clone();
                                                                    action_loading.set(Some(id.clone()));
                                                                    spawn(async move {
                                                                        let _ = use_auth::approve_user(&id).await;
                                                                        action_loading.set(None);
                                                                        refresh_users();
                                                                    });
                                                                },
                                                                "Approve"
                                                            }
                                                        }
                                                        if user.status == UserStatus::Active {
                                                            button {
                                                                class: "btn btn-error btn-xs btn-outline",
                                                                onclick: move |_| {
                                                                    let id = uid_disable.clone();
                                                                    action_loading.set(Some(id.clone()));
                                                                    spawn(async move {
                                                                        let _ = use_auth::disable_user(&id).await;
                                                                        action_loading.set(None);
                                                                        refresh_users();
                                                                    });
                                                                },
                                                                "Disable"
                                                            }
                                                        }
                                                        if user.status == UserStatus::Disabled {
                                                            button {
                                                                class: "btn btn-success btn-xs btn-outline",
                                                                onclick: move |_| {
                                                                    let id = uid_enable.clone();
                                                                    action_loading.set(Some(id.clone()));
                                                                    spawn(async move {
                                                                        let _ = use_auth::enable_user(&id).await;
                                                                        action_loading.set(None);
                                                                        refresh_users();
                                                                    });
                                                                },
                                                                "Enable"
                                                            }
                                                        }
                                                        if user.totp_enabled {
                                                            button {
                                                                class: "btn btn-warning btn-xs btn-outline",
                                                                onclick: move |_| {
                                                                    let id = uid_totp.clone();
                                                                    action_loading.set(Some(id.clone()));
                                                                    spawn(async move {
                                                                        let _ = use_auth::reset_user_totp(&id).await;
                                                                        action_loading.set(None);
                                                                        refresh_users();
                                                                    });
                                                                },
                                                                "Reset 2FA"
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
                    }
                }

                CreateUserSection { on_created: move |_| refresh_users() }
            }
        }
    }
}
