use std::cell::RefCell;
use std::rc::Rc;
use dioxus::prelude::*;
use dioxus_core::{current_scope_id, Runtime, RuntimeGuard};
use uncloud_common::ServerEvent;
use wasm_bindgen::JsCast;

use super::api;

pub fn use_events<F>(on_event: F)
where
    F: FnMut(ServerEvent) + 'static,
{
    // use_hook gives us a stable Rc that persists across re-renders.
    // On every render we replace the inner handler with the latest closure so
    // captured signals are always current.  This update is safe: render runs
    // synchronously and SSE callbacks are only dispatched between JS tasks.
    let handler: Rc<RefCell<Box<dyn FnMut(ServerEvent)>>> = use_hook(|| {
        Rc::new(RefCell::new(Box::new(|_: ServerEvent| {}) as Box<dyn FnMut(ServerEvent)>))
    });

    *handler.borrow_mut() = Box::new(on_event);

    // Capture the runtime + scope of the calling component once. The SSE
    // callback fires from a JS event handler outside any Dioxus scope, so
    // any Signal access from the user's `on_event` would otherwise panic
    // ("called Option::unwrap() on a None value" inside `current_scope_id`).
    // We push a RuntimeGuard + scope context before invoking the handler.
    let runtime = Runtime::current();
    let scope = current_scope_id();

    // This effect reads no signals, so it runs exactly once after mount.
    // One EventSource -> one SSE connection for the lifetime of the component.
    use_effect(move || {
        let mut url = format!("{}/api/events", api::api_base());
        // EventSource does not support custom headers, so pass the auth token
        // as a query parameter when one is stored (Tauri / Android mode).
        if let Some(token) = api::auth_token() {
            url = format!("{}?token={}", url, token);
        }

        let source = match web_sys::EventSource::new(&url) {
            Ok(s) => s,
            Err(_) => return,
        };

        let handler = handler.clone();
        let runtime = runtime.clone();
        let on_message = wasm_bindgen::closure::Closure::wrap(Box::new(
            move |evt: web_sys::MessageEvent| {
                let Some(data) = evt.data().as_string() else { return };
                let Ok(event) = serde_json::from_str::<ServerEvent>(&data) else { return };
                // Provide the runtime + scope so Signal::set / read / write
                // inside the user's handler don't blow up.
                let _runtime_guard = RuntimeGuard::new(runtime.clone());
                runtime.in_scope(scope, || {
                    // try_borrow_mut avoids a panic on unexpected re-entrance.
                    if let Ok(mut guard) = handler.try_borrow_mut() {
                        guard(event);
                    }
                });
            },
        ) as Box<dyn FnMut(web_sys::MessageEvent)>);

        source.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        on_message.forget();
        // `source` drops here; the JS object stays alive via the onmessage handler.
    });
}
