use dioxus::prelude::*;

use crate::hooks::{api, tauri, use_auth, use_search};
use crate::state::AuthState;

/// First-run onboarding screen for the desktop app.
///
/// Shown when running inside Tauri and no saved config is found.
/// Collects server URL, credentials, and local sync folder path.
/// On success, seeds the API base URL, initialises the sync engine,
/// establishes a browser session, then navigates to `/`.
#[component]
pub fn Setup() -> Element {
    let mut server_url = use_signal(String::new);
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut root_path = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut loading = use_signal(|| false);
    let nav = use_navigator();
    let mut auth_state = use_context::<Signal<AuthState>>();
    let mut search_enabled = use_context::<Signal<bool>>();
    let is_android = tauri::is_android();

    // Pre-fill the sync folder with a platform-appropriate default (desktop only).
    use_effect(move || {
        if !is_android {
            spawn(async move {
                if let Some(default) = tauri::default_sync_folder().await {
                    if root_path.peek().is_empty() {
                        root_path.set(default);
                    }
                }
            });
        }
    });

    let on_submit = move |evt: Event<FormData>| {
        evt.prevent_default();
        let server = server_url();
        let user = username();
        let pass = password();
        let path = root_path();

        spawn(async move {
            loading.set(true);
            error.set(None);

            // Seed the API base so subsequent web requests go to the right server.
            api::seed_api_base(server.clone());

            // On desktop, validate that a sync folder was chosen.
            if !is_android && path.is_empty() {
                error.set(Some("Please select a sync folder".to_string()));
                loading.set(false);
                return;
            }

            // Initialise the sync engine. On Android, root_path is empty —
            // the backend uses a placeholder; per-folder paths override it.
            if let Err(e) = tauri::login(&server, &user, &pass, &path).await {
                error.set(Some(format!("Connection failed: {e}")));
                loading.set(false);
                return;
            }

            // Establish a browser-level session for the file browser UI.
            match use_auth::login(&user, &pass).await {
                Ok(resp) => {
                    if resp.totp_required {
                        error.set(Some("TOTP is not supported during setup yet".to_string()));
                    } else if let Some(user_resp) = resp.user {
                        auth_state.write().user = Some(user_resp);
                        let enabled = use_search::fetch_search_enabled().await;
                        search_enabled.set(enabled);
                        tauri::mark_setup_complete();
                        nav.replace("/");
                    }
                }
                Err(e) => {
                    error.set(Some(format!("Login failed: {e}")));
                }
            }

            loading.set(false);
        });
    };

    rsx! {
        div { class: "flex items-center justify-center min-h-screen bg-base-200",
            div { class: "card bg-base-100 shadow-xl w-full max-w-md",
                div { class: "card-body gap-4",
                    div { class: "text-center",
                        div { class: "text-5xl mb-2", "\u{2601}" }
                        h1 { class: "text-2xl font-bold", "Welcome to Uncloud" }
                        p { class: "text-base-content/60 text-sm",
                            "Connect to your server to get started."
                        }
                    }

                    form { class: "flex flex-col gap-3", onsubmit: on_submit,
                        if let Some(err) = error() {
                            div { class: "alert alert-error text-sm",
                                span { "{err}" }
                            }
                        }

                        div { class: "form-control",
                            label { class: "label", r#for: "server-url",
                                span { class: "label-text", "Server URL" }
                            }
                            input {
                                class: "input input-bordered w-full",
                                r#type: "url",
                                id: "server-url",
                                placeholder: "http://localhost:8080",
                                value: "{server_url}",
                                oninput: move |evt| server_url.set(evt.value()),
                                required: true,
                            }
                        }

                        div { class: "form-control",
                            label { class: "label", r#for: "setup-username",
                                span { class: "label-text", "Username" }
                            }
                            input {
                                class: "input input-bordered w-full",
                                r#type: "text",
                                id: "setup-username",
                                placeholder: "Enter your username",
                                value: "{username}",
                                oninput: move |evt| username.set(evt.value()),
                                required: true,
                            }
                        }

                        div { class: "form-control",
                            label { class: "label", r#for: "setup-password",
                                span { class: "label-text", "Password" }
                            }
                            input {
                                class: "input input-bordered w-full",
                                r#type: "password",
                                id: "setup-password",
                                placeholder: "Enter your password",
                                value: "{password}",
                                oninput: move |evt| password.set(evt.value()),
                                required: true,
                            }
                        }

                        if !is_android {
                            div { class: "divider text-xs opacity-50", "Sync" }

                            div { class: "form-control",
                                label { class: "label", r#for: "root-path",
                                    span { class: "label-text", "Local sync folder" }
                                }
                                div { class: "join w-full",
                                    input {
                                        class: "input input-bordered join-item flex-1",
                                        r#type: "text",
                                        id: "root-path",
                                        readonly: true,
                                        value: "{root_path}",
                                        placeholder: "Select a folder\u{2026}",
                                    }
                                    button {
                                        class: "btn btn-neutral join-item",
                                        r#type: "button",
                                        onclick: move |_| {
                                            spawn(async move {
                                                if let Some(path) = tauri::pick_folder().await {
                                                    root_path.set(path);
                                                }
                                            });
                                        },
                                        "Browse\u{2026}"
                                    }
                                }
                                label { class: "label",
                                    span { class: "label-text-alt text-base-content/50",
                                        "Files will be synced to and from this folder."
                                    }
                                }
                            }
                        }

                        button {
                            class: "btn btn-primary w-full mt-1",
                            r#type: "submit",
                            disabled: loading(),
                            if loading() {
                                span { class: "loading loading-spinner loading-sm" }
                                "Connecting\u{2026}"
                            } else {
                                "Connect"
                            }
                        }
                    }
                }
            }
        }
    }
}
