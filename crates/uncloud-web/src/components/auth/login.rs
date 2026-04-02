use dioxus::prelude::*;
use crate::state::AuthState;
use crate::hooks::{use_auth, use_search};

#[component]
pub fn Login() -> Element {
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut loading = use_signal(|| false);
    let nav = use_navigator();
    let mut auth_state = use_context::<Signal<AuthState>>();
    let mut search_enabled = use_context::<Signal<bool>>();

    let on_submit = move |evt: Event<FormData>| {
        evt.prevent_default();

        let username_val = username();
        let password_val = password();

        spawn(async move {
            loading.set(true);
            error.set(None);

            match use_auth::login(&username_val, &password_val).await {
                Ok(user) => {
                    auth_state.write().user = Some(user);
                    let enabled = use_search::fetch_search_enabled().await;
                    search_enabled.set(enabled);
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
                        h1 { class: "text-2xl font-bold", "Welcome back" }
                        p { class: "text-base-content/60 text-sm", "Sign in to your account" }
                    }

                    form { class: "flex flex-col gap-3", onsubmit: on_submit,
                        if let Some(err) = error() {
                            div { class: "alert alert-error text-sm",
                                span { "{err}" }
                            }
                        }

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

                    div { class: "text-center text-sm",
                        "Don't have an account? "
                        Link { to: "/register", class: "link link-primary", "Create one" }
                    }
                }
            }
        }
    }
}
