use dioxus::prelude::*;
use crate::state::AuthState;
use crate::hooks::use_auth;

#[component]
pub fn Register() -> Element {
    let mut username = use_signal(String::new);
    let mut email = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut confirm_password = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut loading = use_signal(|| false);
    let nav = use_navigator();
    let mut auth_state = use_context::<Signal<AuthState>>();

    let on_submit = move |evt: Event<FormData>| {
        evt.prevent_default();

        let username_val = username();
        let email_val = email();
        let password_val = password();
        let confirm_val = confirm_password();

        if password_val != confirm_val {
            error.set(Some("Passwords do not match".to_string()));
            return;
        }

        spawn(async move {
            loading.set(true);
            error.set(None);

            match use_auth::register(&username_val, &email_val, &password_val).await {
                Ok(user) => {
                    auth_state.write().user = Some(user);
                    nav.replace("/");
                }
                Err(e) => {
                    error.set(Some(e));
                }
            }

            loading.set(false);
        });
    };

    rsx! {
        div { class: "flex items-center justify-center min-h-screen bg-base-200",
            div { class: "card bg-base-100 shadow-xl w-full max-w-sm",
                div { class: "card-body gap-4",
                    div { class: "text-center",
                        h1 { class: "text-2xl font-bold", "Create account" }
                        p { class: "text-base-content/60 text-sm", "Start using your personal cloud" }
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
                                span { class: "label-text", "Email" }
                            }
                            input {
                                class: "input input-bordered w-full",
                                r#type: "email",
                                id: "email",
                                placeholder: "Enter your email",
                                value: "{email}",
                                oninput: move |evt| email.set(evt.value()),
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
