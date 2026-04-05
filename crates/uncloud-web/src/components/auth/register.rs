use dioxus::prelude::*;
use uncloud_common::{RegistrationMode, UserStatus};
use crate::state::AuthState;
use crate::hooks::use_auth;

#[component]
pub fn Register(#[props(default)] invite_token: Option<String>) -> Element {
    let mut username = use_signal(String::new);
    let mut email = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut confirm_password = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut loading = use_signal(|| false);
    let mut pending_approval = use_signal(|| false);
    let nav = use_navigator();
    let mut auth_state = use_context::<Signal<AuthState>>();

    // Validate invite token if present
    let mut invite_valid = use_signal(|| None::<bool>);

    {
        let token = invite_token.clone();
        use_effect(move || {
            if let Some(token) = token.clone() {
                spawn(async move {
                    match use_auth::validate_invite(&token).await {
                        Ok(info) => {
                            invite_valid.set(Some(info.valid));
                        }
                        Err(_) => {
                            invite_valid.set(Some(false));
                        }
                    }
                });
            }
        });
    }

    // Fetch server info for registration mode
    let mut reg_mode = use_signal(|| None::<RegistrationMode>);
    use_effect(move || {
        spawn(async move {
            if let Ok(info) = use_auth::server_info().await {
                reg_mode.set(Some(info.registration_mode));
            }
        });
    });

    let invite_for_submit = invite_token.clone();
    let on_submit = move |evt: Event<FormData>| {
        evt.prevent_default();

        let username_val = username();
        let email_val = email();
        let password_val = password();
        let confirm_val = confirm_password();
        let token = invite_for_submit.clone();

        if password_val != confirm_val {
            error.set(Some("Passwords do not match".to_string()));
            return;
        }

        spawn(async move {
            loading.set(true);
            error.set(None);

            let email_opt = if email_val.trim().is_empty() {
                None
            } else {
                Some(email_val.as_str())
            };

            match use_auth::register(&username_val, email_opt, &password_val, token).await {
                Ok(user) => {
                    if user.status == UserStatus::Pending {
                        pending_approval.set(true);
                    } else {
                        auth_state.write().user = Some(user);
                        nav.replace("/");
                    }
                }
                Err(e) => {
                    error.set(Some(e));
                }
            }

            loading.set(false);
        });
    };

    // If invite is invalid, show error
    if invite_token.is_some() && invite_valid() == Some(false) {
        return rsx! {
            div { class: "flex items-center justify-center min-h-screen bg-base-200",
                div { class: "card bg-base-100 shadow-xl w-full max-w-sm",
                    div { class: "card-body gap-4 text-center",
                        h1 { class: "text-2xl font-bold", "Invalid Invite" }
                        p { class: "text-base-content/60",
                            "This invite link is invalid or has expired."
                        }
                        Link { to: "/login", class: "btn btn-primary", "Back to Login" }
                    }
                }
            }
        };
    }

    // Show success message for pending approval
    if pending_approval() {
        return rsx! {
            div { class: "flex items-center justify-center min-h-screen bg-base-200",
                div { class: "card bg-base-100 shadow-xl w-full max-w-sm",
                    div { class: "card-body gap-4 text-center",
                        h1 { class: "text-2xl font-bold", "Registration Submitted" }
                        p { class: "text-base-content/60",
                            "Your account has been created and is pending approval by an administrator. You'll be able to sign in once approved."
                        }
                        Link { to: "/login", class: "btn btn-primary", "Back to Login" }
                    }
                }
            }
        };
    }

    let is_approval_mode = reg_mode() == Some(RegistrationMode::Approval) && invite_token.is_none();

    rsx! {
        div { class: "flex items-center justify-center min-h-screen bg-base-200",
            div { class: "card bg-base-100 shadow-xl w-full max-w-sm",
                div { class: "card-body gap-4",
                    div { class: "text-center",
                        h1 { class: "text-2xl font-bold", "Create account" }
                        p { class: "text-base-content/60 text-sm", "Start using your personal cloud" }
                    }

                    if is_approval_mode {
                        div { class: "alert alert-info text-sm",
                            span { "Your account will need to be approved by an administrator before you can sign in." }
                        }
                    }

                    form { class: "flex flex-col gap-3", onsubmit: on_submit,
                        if let Some(err) = error() {
                            div { class: "alert alert-error text-sm",
                                span { "{err}" }
                            }
                        }

                        div { class: "form-control",
                            label { class: "label", r#for: "username",
                                span { class: "label-text", "Username" }
                            }
                            input {
                                class: "input input-bordered w-full",
                                r#type: "text",
                                id: "username",
                                placeholder: "Choose a username",
                                value: "{username}",
                                oninput: move |evt| username.set(evt.value()),
                                required: true,
                            }
                        }

                        div { class: "form-control",
                            label { class: "label", r#for: "email",
                                span { class: "label-text", "Email " }
                                span { class: "label-text-alt text-base-content/40", "optional" }
                            }
                            input {
                                class: "input input-bordered w-full",
                                r#type: "email",
                                id: "email",
                                placeholder: "Enter your email",
                                value: "{email}",
                                oninput: move |evt| email.set(evt.value()),
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
                                placeholder: "Choose a password",
                                value: "{password}",
                                oninput: move |evt| password.set(evt.value()),
                                required: true,
                            }
                        }

                        div { class: "form-control",
                            label { class: "label", r#for: "confirm",
                                span { class: "label-text", "Confirm Password" }
                            }
                            input {
                                class: "input input-bordered w-full",
                                r#type: "password",
                                id: "confirm",
                                placeholder: "Confirm your password",
                                value: "{confirm_password}",
                                oninput: move |evt| confirm_password.set(evt.value()),
                                required: true,
                            }
                        }

                        button {
                            class: "btn btn-primary w-full mt-1",
                            r#type: "submit",
                            disabled: loading(),
                            if loading() {
                                span { class: "loading loading-spinner loading-sm" }
                                "Creating account..."
                            } else {
                                "Create account"
                            }
                        }
                    }

                    div { class: "text-center text-sm",
                        "Already have an account? "
                        Link { to: "/login", class: "link link-primary", "Sign in" }
                    }
                }
            }
        }
    }
}
