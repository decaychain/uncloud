use dioxus::prelude::*;
use uncloud_common::RegistrationMode;
use crate::state::AuthState;
use crate::hooks::{tauri, use_auth, use_search};

/// Set up the desktop's native sync engine when the webview-side login
/// succeeds outside of first-time setup. Without this, the webview is
/// authenticated but the Tauri sync engine has no client/credentials —
/// sync never starts and on next webview open we'd see the login form
/// again because `state.auth_token` is still empty.
///
/// No-op outside of Tauri-desktop. The TOTP / demo / mobile flows all
/// route through here harmlessly (Android relies on per-folder SAF picks
/// rather than a global engine, and demo has no real credentials).
async fn bridge_native_login(username: &str, password: &str) {
    if !tauri::is_tauri() || tauri::is_android() {
        return;
    }
    let Some(cfg) = tauri::get_config().await else {
        return;
    };
    if let Err(e) = tauri::login(&cfg.server_url, username, password, &cfg.root_path).await {
        web_sys::console::warn_1(
            &format!("Native sync engine handoff failed: {e}").into(),
        );
    }
}

#[component]
pub fn Login() -> Element {
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut loading = use_signal(|| false);
    let nav = use_navigator();
    let mut auth_state = use_context::<Signal<AuthState>>();
    let mut search_enabled = use_context::<Signal<bool>>();

    // TOTP two-step login state
    let mut totp_token = use_signal(|| None::<String>);
    let mut totp_code = use_signal(String::new);

    // Fetch server info to know registration mode
    let mut reg_mode = use_signal(|| None::<RegistrationMode>);
    use_effect(move || {
        spawn(async move {
            if let Ok(info) = use_auth::server_info().await {
                reg_mode.set(Some(info.registration_mode));
            }
        });
    });

    let complete_login = move |user: uncloud_common::UserResponse| {
        auth_state.write().user = Some(user);
        let nav = nav.clone();
        spawn(async move {
            let enabled = use_search::fetch_search_enabled().await;
            search_enabled.set(enabled);
            // Mobile (below Tailwind's `lg` breakpoint) lands on the Dashboard;
            // desktop keeps the traditional Files view because the sidebar
            // already exposes everything there.
            let is_mobile = web_sys::window()
                .and_then(|w| w.match_media("(max-width: 1023px)").ok().flatten())
                .map(|mql| mql.matches())
                .unwrap_or(false);
            nav.replace(if is_mobile { "/dashboard" } else { "/" });
        });
    };

    let on_submit = move |evt: Event<FormData>| {
        evt.prevent_default();

        let username_val = username();
        let password_val = password();
        let mut complete = complete_login.clone();

        spawn(async move {
            loading.set(true);
            error.set(None);

            match use_auth::login(&username_val, &password_val).await {
                Ok(resp) => {
                    if resp.totp_required {
                        totp_token.set(resp.totp_token);
                    } else if let Some(user) = resp.user {
                        bridge_native_login(&username_val, &password_val).await;
                        complete(user);
                    }
                }
                Err(e) => {
                    error.set(Some(e));
                }
            }

            loading.set(false);
        });
    };

    let on_totp_submit = move |evt: Event<FormData>| {
        evt.prevent_default();

        let token = totp_token().unwrap_or_default();
        let code = totp_code();
        let username_val = username();
        let password_val = password();
        let mut complete = complete_login.clone();

        spawn(async move {
            loading.set(true);
            error.set(None);

            match use_auth::totp_verify(&token, &code).await {
                Ok(resp) => {
                    if let Some(user) = resp.user {
                        bridge_native_login(&username_val, &password_val).await;
                        complete(user);
                    }
                }
                Err(e) => {
                    error.set(Some(e));
                    totp_code.set(String::new());
                }
            }

            loading.set(false);
        });
    };

    let on_demo = move |_| {
        let mut complete = complete_login.clone();
        spawn(async move {
            loading.set(true);
            error.set(None);

            match use_auth::demo_login().await {
                Ok(resp) => {
                    if let Some(user) = resp.user {
                        complete(user);
                    }
                }
                Err(e) => {
                    error.set(Some(e));
                }
            }

            loading.set(false);
        });
    };

    let show_register = matches!(
        reg_mode(),
        Some(RegistrationMode::Open) | Some(RegistrationMode::Approval)
    );
    let show_demo = matches!(reg_mode(), Some(RegistrationMode::Demo));

    rsx! {
        div { class: "flex items-center justify-center min-h-screen bg-base-200",
            div { class: "card bg-base-100 shadow-xl w-full max-w-sm",
                div { class: "card-body gap-4",
                    div { class: "text-center",
                        h1 { class: "text-2xl font-bold", "Welcome back" }
                        p { class: "text-base-content/60 text-sm", "Sign in to your account" }
                    }

                    if let Some(err) = error() {
                        div { class: "alert alert-error text-sm",
                            span { "{err}" }
                        }
                    }

                    if totp_token().is_some() {
                        // TOTP verification step
                        form { class: "flex flex-col gap-3", onsubmit: on_totp_submit,
                            div { class: "text-center text-sm text-base-content/70 mb-2",
                                "Enter the 6-digit code from your authenticator app"
                            }

                            div { class: "form-control",
                                input {
                                    class: "input input-bordered w-full text-center text-2xl tracking-widest",
                                    r#type: "text",
                                    inputmode: "numeric",
                                    autocomplete: "one-time-code",
                                    maxlength: "10",
                                    placeholder: "000000",
                                    value: "{totp_code}",
                                    oninput: move |evt| totp_code.set(evt.value()),
                                    required: true,
                                }
                            }

                            button {
                                class: "btn btn-primary w-full mt-1",
                                r#type: "submit",
                                disabled: loading(),
                                if loading() {
                                    span { class: "loading loading-spinner loading-sm" }
                                    "Verifying..."
                                } else {
                                    "Verify"
                                }
                            }

                            div { class: "text-center text-xs text-base-content/50 mt-1",
                                "You can also enter a recovery code"
                            }

                            button {
                                class: "btn btn-ghost btn-sm w-full",
                                r#type: "button",
                                onclick: move |_| {
                                    totp_token.set(None);
                                    totp_code.set(String::new());
                                },
                                "Back to login"
                            }
                        }
                    } else {
                        // Normal login form
                        form { class: "flex flex-col gap-3", onsubmit: on_submit,
                            div { class: "form-control",
                                label { class: "label", r#for: "username",
                                    span { class: "label-text", "Username or Email" }
                                }
                                input {
                                    class: "input input-bordered w-full",
                                    r#type: "text",
                                    id: "username",
                                    placeholder: "Enter your username",
                                    value: "{username}",
                                    oninput: move |evt| username.set(evt.value()),
                                    required: true,
                                }
                            }

                            div { class: "form-control",
                                label { class: "label", r#for: "password",
                                    span { class: "label-text", "Password" }
                                }
                                input {
                                    class: "input input-bordered w-full",
                                    r#type: "password",
                                    id: "password",
                                    placeholder: "Enter your password",
                                    value: "{password}",
                                    oninput: move |evt| password.set(evt.value()),
                                    required: true,
                                }
                            }

                            button {
                                class: "btn btn-primary w-full mt-1",
                                r#type: "submit",
                                disabled: loading(),
                                if loading() {
                                    span { class: "loading loading-spinner loading-sm" }
                                    "Signing in..."
                                } else {
                                    "Sign in"
                                }
                            }
                        }

                        if show_demo {
                            div { class: "divider text-xs", "OR" }
                            button {
                                class: "btn btn-outline btn-accent w-full",
                                disabled: loading(),
                                onclick: on_demo,
                                "Try Demo"
                            }
                        }

                        if show_register {
                            div { class: "text-center text-sm",
                                "Don't have an account? "
                                Link { to: "/register", class: "link link-primary", "Create one" }
                            }
                        }
                    }
                }
            }
        }
    }
}
