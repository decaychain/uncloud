use dioxus::prelude::*;

#[component]
pub fn ShareDialog(
    resource_id: String,
    resource_type: String,
    resource_name: String,
    on_close: EventHandler<()>,
) -> Element {
    let mut password = use_signal(String::new);
    let mut use_password = use_signal(|| false);
    let mut expires_hours = use_signal(|| None::<u64>);
    let mut max_downloads = use_signal(|| None::<i64>);
    let mut share_url = use_signal(|| None::<String>);
    let mut loading = use_signal(|| false);

    let on_create = move |_| {
        let resource_id = resource_id.clone();
        let resource_type = resource_type.clone();
        let password_val = if use_password() { Some(password()) } else { None };
        let expires = expires_hours();
        let max_dl = max_downloads();

        spawn(async move {
            loading.set(true);

            match crate::hooks::use_shares::create_share(
                &resource_id,
                &resource_type,
                password_val.as_deref(),
                expires,
                max_dl,
            )
            .await
            {
                Ok(share) => {
                    let base = web_sys::window()
                        .map(|w| w.location().origin().unwrap_or_default())
                        .unwrap_or_default();
                    share_url.set(Some(format!("{}/share/{}", base, share.token)));
                }
                Err(e) => {
                    web_sys::console::error_1(&e.into());
                }
            }

            loading.set(false);
        });
    };

    let on_copy = move |_| {
        if let Some(url) = share_url() {
            if let Some(window) = web_sys::window() {
                let navigator = window.navigator();
                let clipboard = navigator.clipboard();
                let _ = clipboard.write_text(&url);
            }
        }
    };

    rsx! {
        div { class: "modal modal-open",
            div { class: "modal-box w-full max-w-md",
                // Header
                div { class: "flex items-center justify-between mb-4",
                    h3 { class: "font-bold text-lg", "Share \"{resource_name}\"" }
                    button {
                        class: "btn btn-sm btn-circle btn-ghost",
                        onclick: move |_| on_close.call(()),
                        "✕"
                    }
                }

                // Body
                if share_url().is_some() {
                    div { class: "flex flex-col gap-3",
                        div { class: "alert alert-success",
                            span { "✓ Share link created!" }
                        }
                        div { class: "join w-full",
                            input {
                                class: "input input-bordered join-item flex-1 text-sm",
                                readonly: true,
                                value: "{share_url().unwrap_or_default()}",
                            }
                            button {
                                class: "btn btn-secondary join-item",
                                onclick: on_copy,
                                "Copy"
                            }
                        }
                    }
                } else {
                    div { class: "flex flex-col gap-4",
                        div { class: "form-control",
                            label { class: "label cursor-pointer",
                                span { class: "label-text", "Password protect" }
                                input {
                                    r#type: "checkbox",
                                    class: "checkbox checkbox-primary",
                                    checked: use_password(),
                                    onchange: move |evt| use_password.set(evt.checked()),
                                }
                            }
                            if use_password() {
                                input {
                                    class: "input input-bordered w-full mt-2",
                                    r#type: "password",
                                    placeholder: "Enter password",
                                    value: "{password}",
                                    oninput: move |evt| password.set(evt.value()),
                                }
                            }
                        }

                        div { class: "form-control",
                            label { class: "label",
                                span { class: "label-text", "Expiration" }
                            }
                            select {
                                class: "select select-bordered w-full",
                                onchange: move |evt| {
                                    let val = evt.value();
                                    expires_hours.set(val.parse().ok());
                                },
                                option { value: "", "Never" }
                                option { value: "1", "1 hour" }
                                option { value: "24", "1 day" }
                                option { value: "168", "1 week" }
                                option { value: "720", "30 days" }
                            }
                        }

                        div { class: "form-control",
                            label { class: "label",
                                span { class: "label-text", "Max downloads" }
                            }
                            select {
                                class: "select select-bordered w-full",
                                onchange: move |evt| {
                                    let val = evt.value();
                                    max_downloads.set(val.parse().ok());
                                },
                                option { value: "", "Unlimited" }
                                option { value: "1", "1 download" }
                                option { value: "5", "5 downloads" }
                                option { value: "10", "10 downloads" }
                                option { value: "100", "100 downloads" }
                            }
                        }
                    }
                }

                // Footer
                div { class: "modal-action",
                    button {
                        class: "btn btn-ghost",
                        onclick: move |_| on_close.call(()),
                        "Close"
                    }
                    if share_url().is_none() {
                        button {
                            class: "btn btn-primary",
                            onclick: on_create,
                            disabled: loading(),
                            if loading() {
                                span { class: "loading loading-spinner loading-sm" }
                                "Creating..."
                            } else {
                                "Create Link"
                            }
                        }
                    }
                }
            }
            // Backdrop click to close
            div { class: "modal-backdrop", onclick: move |_| on_close.call(()) }
        }
    }
}
