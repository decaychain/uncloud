//! Surfaces a DaisyUI toast when the server returns 401 mid-session,
//! e.g. the cookie expired after `session_duration_hours`. The wrapper in
//! `index.html` overrides `window.fetch` to dispatch a
//! `uncloud:session-expired` `CustomEvent`; this component listens for it
//! and renders the toast until the user dismisses or clicks "Log in".
//!
//! Only mounted under `Layout`, so unauthenticated routes (`/login`,
//! `/register`, `/setup`) cannot trigger this even if their own POSTs 401.

use std::rc::Rc;

use dioxus::prelude::*;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

use crate::components::icons::IconX;
use crate::hooks::api;
use crate::router::Route;
use crate::state::AuthState;

struct ListenerGuard {
    cb: Closure<dyn FnMut(web_sys::Event)>,
}

impl Drop for ListenerGuard {
    fn drop(&mut self) {
        if let Some(win) = web_sys::window() {
            let f: &js_sys::Function = self.cb.as_ref().unchecked_ref();
            let _ = win.remove_event_listener_with_callback("uncloud:session-expired", f);
        }
    }
}

#[component]
pub fn SessionExpiredToast() -> Element {
    let mut visible = use_signal(|| false);
    let mut auth_state = use_context::<Signal<AuthState>>();
    let nav = use_navigator();

    use_hook(move || {
        let cb = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e| {
            visible.set(true);
        });
        if let Some(win) = web_sys::window() {
            let f: &js_sys::Function = cb.as_ref().unchecked_ref();
            let _ = win.add_event_listener_with_callback("uncloud:session-expired", f);
        }
        Rc::new(ListenerGuard { cb })
    });

    if !*visible.read() {
        return rsx! {};
    }

    rsx! {
        div { class: "toast toast-top toast-end z-50",
            div { class: "alert alert-warning shadow-lg max-w-sm",
                div { class: "flex-1",
                    div { class: "font-medium", "Session expired" }
                    div { class: "text-sm opacity-80", "Please log in again to continue." }
                }
                div { class: "flex gap-1",
                    button {
                        class: "btn btn-sm btn-primary",
                        onclick: move |_| {
                            api::clear_stored_session();
                            api::clear_auth_token();
                            auth_state.set(AuthState::default());
                            visible.set(false);
                            nav.push(Route::Login {});
                        },
                        "Log in"
                    }
                    button {
                        class: "btn btn-sm btn-circle btn-ghost",
                        "aria-label": "Dismiss",
                        onclick: move |_| visible.set(false),
                        IconX {}
                    }
                }
            }
        }
    }
}
