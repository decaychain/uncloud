use std::cell::RefCell;
use std::rc::Rc;
use dioxus::prelude::*;
use dioxus_core::{current_scope_id, Runtime, RuntimeGuard};
use uncloud_common::ServerEvent;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{EventSource, MessageEvent};

use super::api;

/// Owns the JS-side resources tied to one component's SSE subscription.
/// Closing the EventSource on Drop is critical: when the component unmounts,
/// the user's handler closure captures Signals from the dropped scope; if
/// JS keeps firing events into it we panic with `ValueDroppedError`. Closing
/// the source stops the callbacks; dropping the closure releases its captures.
struct SseConnection {
    source: EventSource,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
}

impl Drop for SseConnection {
    fn drop(&mut self) {
        self.source.close();
    }
}

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

    // The connection lives for the component's lifetime via use_hook — when
    // the component is unmounted, this Rc drops, which calls SseConnection::Drop
    // and closes the EventSource so no further callbacks fire.
    let connection: Rc<RefCell<Option<SseConnection>>> =
        use_hook(|| Rc::new(RefCell::new(None)));

    // Establish the connection exactly once after mount.
    use_effect({
        let connection = connection.clone();
        let handler = handler.clone();
        let runtime = runtime.clone();
        move || {
            // Only initialize once.
            if connection.borrow().is_some() {
                return;
            }

            let mut url = format!("{}/api/events", api::api_base());
            // EventSource does not support custom headers, so pass the auth token
            // as a query parameter when one is stored (Tauri / Android mode).
            if let Some(token) = api::auth_token() {
                url = format!("{}?token={}", url, token);
            }

            let source = match EventSource::new(&url) {
                Ok(s) => s,
                Err(_) => return,
            };

            let handler = handler.clone();
            let runtime = runtime.clone();
            let on_message = Closure::wrap(Box::new(move |evt: MessageEvent| {
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
            }) as Box<dyn FnMut(MessageEvent)>);

            source.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
            *connection.borrow_mut() = Some(SseConnection {
                source,
                _on_message: on_message,
            });
        }
    });
}
