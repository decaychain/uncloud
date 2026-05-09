use std::collections::HashMap;

use dioxus::prelude::*;

use crate::hooks::use_oauth::{self, AuthorizeSubmit, OAuthClient};
use crate::state::AuthState;

fn parse_query() -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(window) = web_sys::window() else {
        return out;
    };
    let search = window
        .location()
        .search()
        .unwrap_or_default()
        .trim_start_matches('?')
        .to_string();
    if search.is_empty() {
        return out;
    }
    for pair in search.split('&') {
        let mut iter = pair.splitn(2, '=');
        let key = iter.next().unwrap_or("").to_string();
        let val = iter.next().unwrap_or("").to_string();
        let val = urlencoding::decode(&val).map(|c| c.into_owned()).unwrap_or(val);
        out.insert(key, val);
    }
    out
}

fn scope_description(scope: &str) -> &'static str {
    match scope {
        "files:read" => "Read your files, folders, gallery, and music",
        "files:write" => "Create, upload, rename, and move files and folders",
        "files:delete" => "Move files and folders to trash",
        _ => "Unknown scope",
    }
}

#[component]
pub fn OAuthConsent() -> Element {
    let auth_state = use_context::<Signal<AuthState>>();
    let nav = use_navigator();

    // Parse the URL query string at mount. The Dioxus router normalises the
    // visible URL once it takes over (it has no params on the OAuthAuthorize
    // route), which strips ?client_id=...; reading later than mount races
    // and finds an empty search.
    let params = use_signal(parse_query);
    let mut client = use_signal(|| None::<OAuthClient>);
    let mut error = use_signal(|| None::<String>);
    let mut submitting = use_signal(|| false);
    let mut lookup_started = use_signal(|| false);

    use_effect(move || {
        // Wait for the app-level bootstrap (`use_auth::me()`) to settle
        // before deciding whether the user is signed in. Otherwise the
        // initial render with `loading=true, user=None` would bounce us
        // to /login on every page load.
        if auth_state.read().loading {
            return;
        }

        // If not logged in, bounce to /login carrying the full URL so we
        // can come back here after auth.
        if auth_state.read().user.is_none() {
            let p = params.read();
            let qs: Vec<String> = p
                .iter()
                .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
                .collect();
            let here = if qs.is_empty() {
                "/oauth/authorize".to_string()
            } else {
                format!("/oauth/authorize?{}", qs.join("&"))
            };
            let next = urlencoding::encode(&here);
            nav.replace(format!("/login?next={}", next));
            return;
        }

        if *lookup_started.read() {
            return;
        }
        let client_id = params.read().get("client_id").cloned();
        let Some(client_id) = client_id else {
            error.set(Some("Missing client_id".into()));
            return;
        };
        lookup_started.set(true);
        spawn(async move {
            match use_oauth::lookup_client(&client_id).await {
                Ok(c) => client.set(Some(c)),
                Err(e) => error.set(Some(format!("Could not load client: {}", e))),
            }
        });
    });

    let mut submit = move |decision: &'static str| {
        submitting.set(true);
        error.set(None);

        let p = params.read().clone();
        spawn(async move {
            let body = AuthorizeSubmit {
                client_id: p.get("client_id").cloned().unwrap_or_default(),
                redirect_uri: p.get("redirect_uri").cloned().unwrap_or_default(),
                response_type: p
                    .get("response_type")
                    .cloned()
                    .unwrap_or_else(|| "code".into()),
                scope: p.get("scope").cloned().unwrap_or_default(),
                state: p.get("state").cloned(),
                code_challenge: p.get("code_challenge").cloned().unwrap_or_default(),
                code_challenge_method: p
                    .get("code_challenge_method")
                    .cloned()
                    .unwrap_or_else(|| "S256".into()),
                decision: decision.to_string(),
            };
            match use_oauth::submit_authorize(body).await {
                Ok(resp) => {
                    if let Some(window) = web_sys::window() {
                        let _ = window.location().set_href(&resp.redirect_to);
                    }
                }
                Err(e) => {
                    error.set(Some(e));
                    submitting.set(false);
                }
            }
        });
    };

    let scope_str = params.read().get("scope").cloned().unwrap_or_default();
    let requested_scopes: Vec<String> = scope_str
        .split_ascii_whitespace()
        .map(|s| s.to_string())
        .collect();

    rsx! {
        div { class: "min-h-screen flex items-center justify-center bg-base-200 p-4",
            div { class: "card bg-base-100 shadow-xl w-full max-w-md",
                div { class: "card-body",
                    if let Some(err) = error.read().clone() {
                        div { class: "alert alert-error", "{err}" }
                    } else if let Some(c) = client.read().clone() {
                        h1 { class: "card-title text-2xl",
                            "Authorize "
                            span { class: "text-primary", "{c.client_name}" }
                        }
                        p { class: "text-base-content/70",
                            "This application is requesting access to your Uncloud account. It will be able to:"
                        }
                        ul { class: "list-disc list-inside py-3 space-y-1",
                            for scope in requested_scopes.iter() {
                                li { key: "{scope}",
                                    span { class: "font-medium", "{scope}" }
                                    span { class: "text-base-content/60", " — {scope_description(scope)}" }
                                }
                            }
                        }
                        p { class: "text-sm text-base-content/60",
                            "You can revoke access any time from Settings → Connected apps."
                        }
                        div { class: "card-actions justify-end pt-4",
                            button {
                                class: "btn btn-ghost",
                                disabled: submitting(),
                                onclick: move |_| submit("deny"),
                                "Deny"
                            }
                            button {
                                class: "btn btn-primary",
                                disabled: submitting(),
                                onclick: move |_| submit("allow"),
                                if submitting() {
                                    span { class: "loading loading-spinner loading-sm" }
                                }
                                "Allow"
                            }
                        }
                    } else {
                        div { class: "flex justify-center py-8",
                            span { class: "loading loading-spinner loading-lg" }
                        }
                    }
                }
            }
        }
    }
}
