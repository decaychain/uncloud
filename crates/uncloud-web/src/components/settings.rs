use dioxus::prelude::*;
use gloo_storage::{LocalStorage, Storage};

use crate::components::dashboard::{all_tile_ids, default_tile_ids, tile_label};
use crate::hooks::tauri::{
    self, SyncLogRow, SyncPhase, SyncState, TauriListener,
};
use uncloud_common::{CreateInviteRequest, UpdatePreferencesRequest, UserRole, UserStatus};
use crate::hooks::use_auth;
use crate::hooks::use_preferences;
use crate::hooks::use_processing;
use crate::hooks::use_search;
use crate::hooks::use_storages;
use crate::hooks::use_s3;
use crate::hooks::use_shopping;
use crate::state::{AuthState, FontScale, RescanState, ThemeState};

#[component]
pub fn SettingsPage(tab: String) -> Element {
    let is_desktop = tauri::is_tauri();
    let auth_state = use_context::<Signal<AuthState>>();
    let search_enabled = use_context::<Signal<bool>>();
    let is_admin = auth_state().is_admin();

    // Activity's table + the Sync panel's local-log table both benefit from
    // more horizontal room than the narrow form-oriented sections.
    let wrapper_class = match tab.as_str() {
        "activity" | "sync" => "space-y-6 max-w-6xl",
        _ => "space-y-6 max-w-2xl",
    };

    rsx! {
        div { class: "{wrapper_class}",
            match tab.as_str() {
                "account" => rsx! {
                    h1 { class: "text-2xl font-bold", "Account" }
                    if is_desktop {
                        ConnectionSection {}
                    }
                    ChangePasswordSection {}
                    TotpSection {}
                    S3AccessKeysSection {}
                },
                "sync" if is_desktop => rsx! {
                    h1 { class: "text-2xl font-bold", "Sync" }
                    SyncSection {}
                    AutostartSection {}
                },
                "preferences" => rsx! {
                    h1 { class: "text-2xl font-bold", "Preferences" }
                    AppearanceSection {}
                    DashboardTilesSection {}
                    OptionalFeaturesSection {}
                    MusicSection {}
                },
                "activity" => rsx! {
                    h1 { class: "text-2xl font-bold", "Activity" }
                    crate::components::activity::ActivitySection {}
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
    let font_scale = theme_state().font_scale;

    let scale_options: &[(FontScale, &str)] = &[
        (FontScale::Small, "Small"),
        (FontScale::Default, "Default"),
        (FontScale::Large, "Large"),
        (FontScale::XLarge, "Extra large"),
    ];

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
                                let new_dark = !is_dark;
                                theme_state.write().dark = new_dark;
                                // Flip the Android system bar icons (light vs
                                // dark glyphs) to stay legible against the new
                                // theme. No-op on desktop / web.
                                tauri::set_android_theme(new_dark);
                            },
                        }
                    }
                }

                div { class: "flex items-start justify-between gap-4 pt-2",
                    div {
                        p { class: "font-medium text-sm", "Font size" }
                        p { class: "text-base-content/60 text-xs mt-0.5",
                            "Scales the UI for readability. Takes effect immediately."
                        }
                    }
                    select {
                        class: "select select-bordered select-sm w-40 shrink-0",
                        value: "{font_scale.as_str()}",
                        onchange: move |evt| {
                            if let Some(s) = FontScale::from_str(&evt.value()) {
                                theme_state.write().font_scale = s;
                                let _ = LocalStorage::set("uncloud_font_scale", s.as_str());
                            }
                        },
                        for (value, label) in scale_options {
                            option {
                                value: "{value.as_str()}",
                                selected: font_scale == *value,
                                "{label}"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn DashboardTilesSection() -> Element {
    let mut auth_state = use_context::<Signal<AuthState>>();
    let mut saving = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    let shopping_enabled = auth_state()
        .user
        .as_ref()
        .map(|u| u.features_enabled.contains(&"shopping".to_string()))
        .unwrap_or(false);

    // Current enabled set: user's stored preference, or defaults if unset.
    let enabled: Vec<String> = {
        let configured = auth_state()
            .user
            .as_ref()
            .map(|u| u.preferences.dashboard_tiles.clone())
            .unwrap_or_default();
        if configured.is_empty() { default_tile_ids() } else { configured }
    };

    let save = move |next: Vec<String>| {
        spawn(async move {
            saving.set(true);
            error.set(None);
            let req = UpdatePreferencesRequest { dashboard_tiles: Some(next) };
            match use_preferences::update_preferences(req).await {
                Ok(updated) => {
                    auth_state.write().user = Some(updated);
                }
                Err(e) => error.set(Some(e)),
            }
            saving.set(false);
        });
    };

    rsx! {
        div { class: "card bg-base-100 shadow",
            div { class: "card-body gap-3",
                div { class: "flex items-center justify-between",
                    h2 { class: "card-title text-lg", "Dashboard tiles" }
                    if saving() {
                        span { class: "loading loading-spinner loading-xs" }
                    }
                }
                p { class: "text-base-content/60 text-xs",
                    "Choose which shortcut tiles appear on the "
                    code { class: "text-xs", "/dashboard" }
                    " page. Useful mainly on mobile."
                }
                if let Some(msg) = error() {
                    div { class: "alert alert-error text-sm", "{msg}" }
                }
                div { class: "grid grid-cols-2 gap-2",
                    for id in all_tile_ids() {
                        {
                            let id_s = id.to_string();
                            let is_on = enabled.iter().any(|x| x == id);
                            let disabled_for_feature = *id == "shopping" && !shopping_enabled;
                            let enabled_now = enabled.clone();
                            let save = save.clone();
                            rsx! {
                                label {
                                    class: if disabled_for_feature {
                                        "flex items-center gap-2 py-1 cursor-not-allowed opacity-50"
                                    } else {
                                        "flex items-center gap-2 py-1 cursor-pointer"
                                    },
                                    input {
                                        r#type: "checkbox",
                                        class: "checkbox checkbox-sm",
                                        checked: is_on,
                                        disabled: disabled_for_feature || saving(),
                                        onchange: move |_| {
                                            // Toggle: preserve current order; append on turn-on, remove on turn-off.
                                            let mut next: Vec<String> = enabled_now
                                                .iter()
                                                .filter(|x| *x != &id_s)
                                                .cloned()
                                                .collect();
                                            if !is_on {
                                                next.push(id_s.clone());
                                            }
                                            save(next);
                                        },
                                    }
                                    span { class: "text-sm", "{tile_label(id)}" }
                                }
                            }
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
    let mut state = use_signal(|| None::<SyncState>);
    let mut log_rows = use_signal(Vec::<SyncLogRow>::new);
    let mut sync_loading = use_signal(|| false);
    let mut sync_msg = use_signal(|| None::<(bool, String)>); // (ok, message)

    // Listener handles — kept alive for the lifetime of the component so that
    // the underlying JS closures + Tauri subscriptions aren't dropped.
    let mut stats_listener = use_signal(|| None::<TauriListener>);
    let mut log_listener = use_signal(|| None::<TauriListener>);

    // Mount-only effect: fetch current state and the log tail, then subscribe
    // to the two push channels. No signals are read inside the body so this
    // does not re-run.
    use_effect(move || {
        spawn(async move {
            if let Ok(s) = tauri::get_status().await {
                state.set(Some(s));
            }
            if let Ok(rows) = tauri::get_local_sync_log(200).await {
                log_rows.set(rows);
            }

            if let Some(l) = tauri::listen_sync_stats(move |s| {
                state.set(Some(s));
            })
            .await
            {
                stats_listener.set(Some(l));
            }

            if let Some(l) = tauri::listen_sync_log_appended(move |row| {
                let mut rows = log_rows();
                rows.insert(0, row);
                if rows.len() > 500 {
                    rows.truncate(500);
                }
                log_rows.set(rows);
            })
            .await
            {
                log_listener.set(Some(l));
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
                }
                Err(e) => sync_msg.set(Some((false, e))),
            }
            sync_loading.set(false);
        });
    };

    let current = state();
    let phase_ref = current.as_ref().map(|s| &s.phase);
    let stats_ref = current.as_ref().map(|s| &s.stats);
    // Engine-level state — true whenever any sync is in flight, including
    // ones triggered by the poll loop, the tray, or another window. The
    // "Sync Now" button respects this so the user can't queue a redundant
    // run that would just block on the engine mutex.
    let is_syncing = matches!(phase_ref, Some(SyncPhase::Syncing));

    let status_badge = match phase_ref {
        Some(SyncPhase::Idle) => rsx! {
            div { class: "flex items-center gap-2",
                span { class: "badge badge-success badge-sm" }
                span { class: "text-sm text-base-content/70", "Up to date" }
            }
        },
        Some(SyncPhase::Syncing) => rsx! {
            div { class: "flex items-center gap-2",
                span { class: "loading loading-spinner loading-xs text-warning" }
                span { class: "text-sm text-base-content/70", "Syncing…" }
            }
        },
        Some(SyncPhase::Error { message }) => rsx! {
            div { class: "flex items-center gap-2",
                span { class: "badge badge-error badge-sm" }
                span { class: "text-sm text-error", "{message}" }
            }
        },
        _ => rsx! {
            span { class: "text-sm text-base-content/50", "Not configured" }
        },
    };

    let stats_panel = stats_ref.map(|s| {
        let last_sync_label = s
            .last_sync_at
            .as_deref()
            .map(format_last_sync)
            .unwrap_or_else(|| "never".to_string());
        rsx! {
            div { class: "grid grid-cols-3 gap-2 text-center",
                div { class: "rounded-lg bg-base-200 p-3",
                    div { class: "text-xs uppercase tracking-wide text-base-content/50", "Uploaded" }
                    div { class: "text-2xl font-semibold tabular-nums", "{s.session_uploaded}" }
                    div { class: "text-xs text-base-content/50",
                        "last run: {s.last_run_uploaded}"
                    }
                }
                div { class: "rounded-lg bg-base-200 p-3",
                    div { class: "text-xs uppercase tracking-wide text-base-content/50", "Downloaded" }
                    div { class: "text-2xl font-semibold tabular-nums", "{s.session_downloaded}" }
                    div { class: "text-xs text-base-content/50",
                        "last run: {s.last_run_downloaded}"
                    }
                }
                div { class: "rounded-lg bg-base-200 p-3",
                    div { class: "text-xs uppercase tracking-wide text-base-content/50", "Errors" }
                    div {
                        class: if s.session_errors > 0 { "text-2xl font-semibold tabular-nums text-error" } else { "text-2xl font-semibold tabular-nums" },
                        "{s.session_errors}"
                    }
                    div { class: "text-xs text-base-content/50",
                        "last run: {s.last_run_errors}"
                    }
                }
            }
            div { class: "text-xs text-base-content/50",
                "Last sync: {last_sync_label}"
            }
        }
    });

    let rows = log_rows();
    let log_panel = if rows.is_empty() {
        rsx! {
            div { class: "text-xs text-base-content/50 italic text-center py-6",
                "No sync activity yet."
            }
        }
    } else {
        rsx! {
            div { class: "max-h-96 overflow-y-auto border border-base-300 rounded-lg",
                table { class: "table table-xs",
                    thead { class: "sticky top-0 bg-base-200 z-10",
                        tr {
                            th { "Time" }
                            th { "Event" }
                            th { "Details" }
                        }
                    }
                    tbody {
                        for row in rows.iter() {
                            {
                                let time = format_local_log_time(&row.timestamp);
                                let op_badge_class = local_log_badge_class(&row.operation);
                                let op_label = local_log_op_label(&row.operation);
                                let row_class = local_log_row_class(&row.operation);
                                // One merged column: summary note wins for
                                // SyncEnd markers, `path → new_path` for
                                // renames/moves, otherwise the path itself.
                                // For SyncStart (no note), leave details empty
                                // rather than printing the placeholder "run".
                                let is_start = row.operation == "SyncStart";
                                let details = if let Some(note) = row.note.clone() {
                                    note
                                } else if let Some(new_path) = row.new_path.as_deref() {
                                    format!("{} → {}", row.path, new_path)
                                } else if is_start {
                                    String::new()
                                } else {
                                    row.path.clone()
                                };
                                rsx! {
                                    tr { key: "{row.id}", class: "{row_class}",
                                        td { class: "text-base-content/60 whitespace-nowrap", "{time}" }
                                        td {
                                            span { class: "{op_badge_class}", "{op_label}" }
                                        }
                                        td { class: "truncate max-w-[40rem]", title: "{details}", "{details}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    };

    rsx! {
        div { class: "card bg-base-100 shadow",
            div { class: "card-body gap-4",
                h2 { class: "card-title text-lg", "Sync" }

                div { class: "flex items-center justify-between",
                    div { {status_badge} }
                    button {
                        class: "btn btn-primary btn-sm",
                        // Block while EITHER our own click is in flight OR
                        // another sync (poll loop, tray, mobile resume) is
                        // running. The engine-level mutex would queue us
                        // anyway; this just makes the wait visible.
                        disabled: sync_loading() || is_syncing,
                        title: if is_syncing { "A sync is already in progress." } else { "" },
                        onclick: on_sync_now,
                        if sync_loading() || is_syncing {
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

                if let Some(panel) = stats_panel {
                    {panel}
                }

                div { class: "divider text-xs text-base-content/50 my-0", "This device — local activity" }
                {log_panel}

                p { class: "text-xs text-base-content/50",
                    "Sync runs automatically every 60 seconds in the background. Status and log update live over Tauri events — no polling."
                }
            }
        }
    }
}

#[component]
fn AutostartSection() -> Element {
    let mut enabled = use_signal(|| None::<bool>);
    let mut error = use_signal(|| None::<String>);
    let mut saving = use_signal(|| false);

    use_effect(move || {
        spawn(async move {
            match tauri::get_autostart().await {
                Ok(v) => enabled.set(Some(v)),
                Err(e) => error.set(Some(e)),
            }
        });
    });

    let on_toggle = move |_| {
        let next = !enabled().unwrap_or(false);
        spawn(async move {
            saving.set(true);
            error.set(None);
            match tauri::set_autostart(next).await {
                Ok(()) => enabled.set(Some(next)),
                Err(e) => error.set(Some(e)),
            }
            saving.set(false);
        });
    };

    rsx! {
        div { class: "card bg-base-100 shadow",
            div { class: "card-body gap-4",
                h2 { class: "card-title text-lg", "Launch on login" }
                p { class: "text-sm text-base-content/70",
                    "Start Uncloud automatically when you sign in to your computer. Runs in the system tray with sync active in the background."
                }
                if let Some(err) = error() {
                    div { class: "alert alert-error text-sm", span { "{err}" } }
                }
                div { class: "form-control",
                    label { class: "label cursor-pointer justify-start gap-3",
                        input {
                            r#type: "checkbox",
                            class: "toggle toggle-primary",
                            checked: enabled().unwrap_or(false),
                            disabled: saving() || enabled().is_none(),
                            onchange: on_toggle,
                        }
                        span { class: "label-text",
                            if saving() { "Saving…" } else if enabled().is_none() { "Loading…" } else { "Launch Uncloud at login" }
                        }
                    }
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
    let mut rerun_loading = use_signal(|| false);
    let mut rerun_msg: Signal<Option<(bool, String)>> = use_signal(|| None);
    let mut confirm_rerun = use_signal(|| false);

    // Rescan state lives at the app level so it survives navigation away from
    // Settings, and SSE events from `layout.rs` keep it fresh.
    let mut rescan_state = use_context::<Signal<RescanState>>();

    let on_rescan = move |_| {
        spawn(async move {
            {
                let mut s = rescan_state.write();
                s.starting = true;
                s.error = None;
                s.job = None;
            }
            let storages = match use_storages::list_storages().await {
                Ok(v) => v,
                Err(e) => {
                    let mut s = rescan_state.write();
                    s.error = Some(e);
                    s.starting = false;
                    return;
                }
            };
            // Prefer the default storage; fall back to the first one.
            let target = storages.iter().find(|s| s.is_default).or_else(|| storages.first());
            let Some(storage) = target else {
                let mut s = rescan_state.write();
                s.error = Some("No storage configured.".to_string());
                s.starting = false;
                return;
            };
            match use_storages::start_rescan(&storage.id).await {
                Ok(job) => {
                    let mut s = rescan_state.write();
                    s.job = Some(job);
                    s.starting = false;
                    // SSE will drive subsequent updates.
                }
                Err(e) => {
                    let mut s = rescan_state.write();
                    s.error = Some(e);
                    s.starting = false;
                }
            }
        });
    };

    let on_cancel_rescan = move |_| {
        if let Some(job) = rescan_state.read().job.clone() {
            spawn(async move {
                let _ = use_storages::cancel_rescan_job(&job.id).await;
            });
        }
    };

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

    let on_confirm_rerun = move |_| {
        confirm_rerun.set(false);
        spawn(async move {
            rerun_loading.set(true);
            rerun_msg.set(None);
            match use_processing::rerun_all().await {
                Ok(()) => rerun_msg.set(Some((
                    true,
                    "Post-processing queued for every file. Thumbnails and metadata will refresh over the next few minutes.".to_string(),
                ))),
                Err(e) => rerun_msg.set(Some((false, e))),
            }
            rerun_loading.set(false);
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

                div { class: "divider my-0" }

                div { class: "flex items-center justify-between gap-4",
                    div {
                        p { class: "font-medium text-sm", "Rerun post-processing" }
                        p { class: "text-base-content/60 text-xs mt-0.5",
                            "Drops thumbnails, audio metadata, and text extraction state for every file and re-queues the full pipeline. Use after fixing a bug or raising thumbnail_max_pixels."
                        }
                    }
                    button {
                        class: "btn btn-sm btn-outline btn-warning shrink-0",
                        disabled: rerun_loading(),
                        onclick: move |_| confirm_rerun.set(true),
                        if rerun_loading() {
                            span { class: "loading loading-spinner loading-xs" }
                        }
                        "Rerun"
                    }
                }

                if let Some((ok, msg)) = rerun_msg() {
                    div {
                        class: if ok { "alert alert-success text-sm" } else { "alert alert-error text-sm" },
                        span { "{msg}" }
                    }
                }

                div { class: "divider my-0" }

                {
                    let state = rescan_state();
                    let running = matches!(
                        state.job.as_ref().map(|j| j.status.clone()),
                        Some(use_storages::RescanStatus::Running)
                    );
                    let busy = state.starting || running;
                    rsx! {
                        div { class: "flex items-center justify-between gap-4",
                            div {
                                p { class: "font-medium text-sm", "Rescan storage" }
                                p { class: "text-base-content/60 text-xs mt-0.5",
                                    "Walks the default storage on disk and imports any folder or file that's missing from the database. Useful after copying files directly into the storage root."
                                }
                            }
                            div { class: "flex gap-2 shrink-0",
                                if running {
                                    button {
                                        class: "btn btn-sm btn-outline btn-error",
                                        onclick: on_cancel_rescan,
                                        "Cancel"
                                    }
                                }
                                button {
                                    class: "btn btn-sm btn-outline",
                                    disabled: busy,
                                    onclick: on_rescan,
                                    if busy {
                                        span { class: "loading loading-spinner loading-xs" }
                                    }
                                    "Rescan"
                                }
                            }
                        }
                    }
                }

                if let Some(job) = rescan_state().job.clone() {
                    match job.status {
                        use_storages::RescanStatus::Running => rsx! {
                            div { class: "alert alert-info text-sm flex-col items-start",
                                div { class: "flex items-center gap-2 w-full",
                                    span { class: "loading loading-spinner loading-xs" }
                                    span {
                                        if let Some(total) = job.total_entries {
                                            "Scanning… {job.processed_entries} / {total} entries processed"
                                        } else {
                                            "Scanning… {job.processed_entries} entries processed"
                                        }
                                    }
                                }
                                span { class: "text-xs opacity-70",
                                    "Imported {job.imported_files} file(s), {job.imported_folders} folder(s); skipped {job.skipped_existing} already tracked."
                                }
                            }
                        },
                        use_storages::RescanStatus::Completed => rsx! {
                            div { class: "alert alert-success text-sm flex-col items-start",
                                span {
                                    "Scanned {job.processed_entries} entries — imported {job.imported_folders} folder(s) and {job.imported_files} file(s), skipped {job.skipped_existing} already tracked."
                                }
                                if !job.conflicts.is_empty() {
                                    div { class: "mt-2 w-full",
                                        p { class: "font-medium", "{job.conflicts.len()} conflict(s):" }
                                        ul { class: "list-disc list-inside text-xs mt-1 max-h-40 overflow-y-auto",
                                            for c in job.conflicts.iter() {
                                                li { key: "{c.path}", "{c.path} — {c.reason}" }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                        use_storages::RescanStatus::Cancelled => rsx! {
                            div { class: "alert alert-warning text-sm flex-col items-start",
                                span {
                                    "Rescan cancelled after {job.processed_entries} entries — imported {job.imported_folders} folder(s) and {job.imported_files} file(s)."
                                }
                                if !job.conflicts.is_empty() {
                                    div { class: "mt-2 w-full",
                                        p { class: "font-medium", "{job.conflicts.len()} conflict(s):" }
                                        ul { class: "list-disc list-inside text-xs mt-1 max-h-40 overflow-y-auto",
                                            for c in job.conflicts.iter() {
                                                li { key: "{c.path}", "{c.path} — {c.reason}" }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                        use_storages::RescanStatus::Failed => rsx! {
                            div { class: "alert alert-error text-sm",
                                span {
                                    "Rescan failed: {job.error.clone().unwrap_or_else(|| \"unknown error\".to_string())}"
                                }
                            }
                        },
                    }
                }

                if let Some(err) = rescan_state().error.clone() {
                    div { class: "alert alert-error text-sm",
                        span { "Rescan failed: {err}" }
                    }
                }
            }
        }

        if confirm_rerun() {
            div { class: "modal modal-open",
                div { class: "modal-box",
                    h3 { class: "font-bold text-lg", "Rerun post-processing?" }
                    p { class: "py-3 text-sm",
                        "This will clear every file's processing state (thumbnails, audio metadata, text extraction, search index) and re-queue the pipeline. Thumbnails may appear blank for a few minutes on large libraries."
                    }
                    div { class: "modal-action",
                        button {
                            class: "btn btn-ghost",
                            onclick: move |_| confirm_rerun.set(false),
                            "Cancel"
                        }
                        button {
                            class: "btn btn-warning",
                            onclick: on_confirm_rerun,
                            "Yes, rerun"
                        }
                    }
                }
                div { class: "modal-backdrop",
                    onclick: move |_| confirm_rerun.set(false),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Change Password section
// ---------------------------------------------------------------------------

#[component]
fn ChangePasswordSection() -> Element {
    let mut current_password = use_signal(String::new);
    let mut new_password = use_signal(String::new);
    let mut confirm_password = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut success = use_signal(|| false);
    let mut loading = use_signal(|| false);

    let on_submit = move |evt: Event<FormData>| {
        evt.prevent_default();

        let current = current_password();
        let new_pw = new_password();
        let confirm = confirm_password();

        if new_pw != confirm {
            error.set(Some("Passwords do not match".to_string()));
            return;
        }

        spawn(async move {
            loading.set(true);
            error.set(None);
            success.set(false);

            match use_auth::change_password(&current, &new_pw).await {
                Ok(()) => {
                    success.set(true);
                    current_password.set(String::new());
                    new_password.set(String::new());
                    confirm_password.set(String::new());
                }
                Err(e) => error.set(Some(e)),
            }

            loading.set(false);
        });
    };

    rsx! {
        div { class: "card bg-base-100 shadow",
            div { class: "card-body gap-3",
                h2 { class: "card-title text-lg", "Change Password" }

                if let Some(err) = error() {
                    div { class: "alert alert-error text-sm", span { "{err}" } }
                }
                if success() {
                    div { class: "alert alert-success text-sm", span { "Password changed successfully." } }
                }

                form { class: "flex flex-col gap-3", onsubmit: on_submit,
                    div { class: "form-control",
                        label { class: "label",
                            span { class: "label-text text-sm", "Current password" }
                        }
                        input {
                            class: "input input-bordered input-sm w-full max-w-xs",
                            r#type: "password",
                            value: "{current_password}",
                            oninput: move |evt| current_password.set(evt.value()),
                            required: true,
                        }
                    }
                    div { class: "form-control",
                        label { class: "label",
                            span { class: "label-text text-sm", "New password" }
                        }
                        input {
                            class: "input input-bordered input-sm w-full max-w-xs",
                            r#type: "password",
                            value: "{new_password}",
                            oninput: move |evt| new_password.set(evt.value()),
                            required: true,
                        }
                    }
                    div { class: "form-control",
                        label { class: "label",
                            span { class: "label-text text-sm", "Confirm new password" }
                        }
                        input {
                            class: "input input-bordered input-sm w-full max-w-xs",
                            r#type: "password",
                            value: "{confirm_password}",
                            oninput: move |evt| confirm_password.set(evt.value()),
                            required: true,
                        }
                    }
                    button {
                        class: "btn btn-primary btn-sm w-fit",
                        r#type: "submit",
                        disabled: loading(),
                        if loading() {
                            span { class: "loading loading-spinner loading-xs" }
                        }
                        "Change password"
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
    let mut reset_pw_target = use_signal(|| None::<(String, String)>);
    let mut reset_pw_value = use_signal(String::new);
    let mut reset_pw_error = use_signal(|| None::<String>);
    let mut reset_pw_loading = use_signal(|| false);
    let mut reset_pw_success = use_signal(|| false);

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

    let auth_state = use_context::<Signal<AuthState>>();
    let current_user_id = auth_state().user.as_ref().map(|u| u.id.clone()).unwrap_or_default();

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
                        table { class: "table w-full",
                            thead {
                                tr {
                                    th { "Username" }
                                    th { "Email" }
                                    th { "Role" }
                                    th { "Status" }
                                    th { "2FA" }
                                    th { class: "w-10" }
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
                                        let uid_role = uid.clone();
                                        let uid_pw = uid.clone();
                                        let username_pw = user.username.clone();
                                        let is_self = uid == current_user_id;
                                        let is_acting = action_loading() == Some(uid.clone());
                                        let new_role = match user.role {
                                            UserRole::Admin => UserRole::User,
                                            UserRole::User => UserRole::Admin,
                                        };
                                        let role_label = match user.role {
                                            UserRole::Admin => "Demote to user",
                                            UserRole::User => "Promote to admin",
                                        };
                                        let has_menu_items = !is_self
                                            || user.status == UserStatus::Pending
                                            || user.totp_enabled;
                                        rsx! {
                                            tr {
                                                td { class: "font-medium", "{user.username}" }
                                                td { class: "text-base-content/70",
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
                                                        span { class: "text-success text-sm", "Enabled" }
                                                    } else {
                                                        span { class: "text-base-content/40 text-sm", "Off" }
                                                    }
                                                }
                                                td {
                                                    if is_acting {
                                                        span { class: "loading loading-spinner loading-xs" }
                                                    } else if has_menu_items {
                                                        div { class: "dropdown dropdown-end",
                                                            div {
                                                                tabindex: "0",
                                                                role: "button",
                                                                class: "btn btn-ghost btn-sm btn-circle",
                                                                "..."
                                                            }
                                                            ul {
                                                                tabindex: "0",
                                                                class: "dropdown-content z-10 menu menu-sm shadow bg-base-200 rounded-box w-48",

                                                                if user.status == UserStatus::Pending {
                                                                    li {
                                                                        a {
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
                                                                }
                                                                if user.status == UserStatus::Active && !is_self {
                                                                    li {
                                                                        a {
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
                                                                }
                                                                if user.status == UserStatus::Disabled {
                                                                    li {
                                                                        a {
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
                                                                }
                                                                if !is_self {
                                                                    li {
                                                                        a {
                                                                            onclick: move |_| {
                                                                                let id = uid_role.clone();
                                                                                action_loading.set(Some(id.clone()));
                                                                                spawn(async move {
                                                                                    let _ = use_auth::change_user_role(&id, new_role).await;
                                                                                    action_loading.set(None);
                                                                                    refresh_users();
                                                                                });
                                                                            },
                                                                            "{role_label}"
                                                                        }
                                                                    }
                                                                    li {
                                                                        a {
                                                                            onclick: move |_| {
                                                                                reset_pw_target.set(Some((uid_pw.clone(), username_pw.clone())));
                                                                                reset_pw_value.set(String::new());
                                                                                reset_pw_error.set(None);
                                                                                reset_pw_success.set(false);
                                                                            },
                                                                            "Reset password"
                                                                        }
                                                                    }
                                                                }
                                                                if user.totp_enabled {
                                                                    li {
                                                                        a {
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
                        }
                    }
                }

                CreateUserSection { on_created: move |_| refresh_users() }
            }
        }

        // Reset password modal
        if let Some((ref target_id, ref target_username)) = reset_pw_target() {
            {
                let target_id = target_id.clone();
                rsx! {
                    div { class: "modal modal-open",
                        div { class: "modal-box",
                            h3 { class: "font-bold text-lg", "Reset password for {target_username}" }

                            if let Some(err) = reset_pw_error() {
                                div { class: "alert alert-error text-sm mt-3", span { "{err}" } }
                            }
                            if reset_pw_success() {
                                div { class: "alert alert-success text-sm mt-3", span { "Password reset successfully." } }
                            }

                            if !reset_pw_success() {
                                div { class: "form-control mt-4",
                                    label { class: "label",
                                        span { class: "label-text", "New password" }
                                    }
                                    input {
                                        class: "input input-bordered w-full",
                                        r#type: "password",
                                        placeholder: "Enter new password",
                                        value: "{reset_pw_value}",
                                        oninput: move |evt| reset_pw_value.set(evt.value()),
                                    }
                                }
                            }

                            div { class: "modal-action",
                                if !reset_pw_success() {
                                    button {
                                        class: "btn btn-primary",
                                        disabled: reset_pw_loading() || reset_pw_value().len() < 8,
                                        onclick: move |_| {
                                            let id = target_id.clone();
                                            let pw = reset_pw_value();
                                            spawn(async move {
                                                reset_pw_loading.set(true);
                                                reset_pw_error.set(None);
                                                match use_auth::admin_reset_password(&id, &pw).await {
                                                    Ok(()) => reset_pw_success.set(true),
                                                    Err(e) => reset_pw_error.set(Some(e)),
                                                }
                                                reset_pw_loading.set(false);
                                            });
                                        },
                                        if reset_pw_loading() {
                                            span { class: "loading loading-spinner loading-xs" }
                                        }
                                        "Reset password"
                                    }
                                }
                                button {
                                    class: "btn",
                                    onclick: move |_| reset_pw_target.set(None),
                                    if reset_pw_success() { "Close" } else { "Cancel" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Format an RFC3339 timestamp as a relative label (e.g. "5s ago", "3m ago").
fn format_last_sync(rfc: &str) -> String {
    use chrono::{DateTime, Utc};
    let Ok(then) = DateTime::parse_from_rfc3339(rfc) else {
        return rfc.to_string();
    };
    let secs = (Utc::now() - then.with_timezone(&Utc)).num_seconds();
    if secs < 5 {
        "just now".to_string()
    } else if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3_600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3_600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

/// Format an ISO-8601 timestamp from the local `sync_log` as `HH:MM:SS` in the
/// browser's local timezone. Falls back to the raw string if parsing fails.
fn format_local_log_time(rfc: &str) -> String {
    use chrono::{DateTime, Local};
    match DateTime::parse_from_rfc3339(rfc) {
        Ok(t) => t.with_timezone(&Local).format("%H:%M:%S").to_string(),
        Err(_) => rfc.to_string(),
    }
}

/// DaisyUI badge class for a local sync_log operation. The labels match the
/// names the engine writes in `uncloud-sync::engine::log_*`, except that
/// `SyncStart` / `SyncEnd` also get a row-level highlight via
/// [`local_log_row_class`] so the bracketing markers stand out from the
/// per-file ops they enclose.
fn local_log_badge_class(op: &str) -> &'static str {
    match op {
        "Uploaded"            => "badge badge-success badge-sm",
        "Downloaded"          => "badge badge-success badge-sm",
        "Updated on server"   => "badge badge-info badge-sm",
        "Updated from server" => "badge badge-info badge-sm",
        "Deleted"             => "badge badge-error badge-sm",
        "SyncStart"           => "badge badge-primary badge-outline badge-sm font-semibold",
        "SyncEnd"             => "badge badge-primary badge-sm font-semibold",
        _                     => "badge badge-neutral badge-sm",
    }
}

fn local_log_op_label(op: &str) -> &str {
    match op {
        "SyncStart" => "Sync started",
        "SyncEnd"   => "Sync completed",
        other       => other,
    }
}

/// Row-level class that gives `SyncStart` / `SyncEnd` a muted background
/// tint so the log reads as "runs" rather than a flat stream of events.
fn local_log_row_class(op: &str) -> &'static str {
    match op {
        "SyncStart" | "SyncEnd" => "bg-base-200/60 italic",
        _                       => "",
    }
}
