//! Document-level safety net for drag-and-drop state.
//!
//! Local `onpointerup`/`onpointerleave` handlers only fire when the pointer
//! ends over the element they're bound to. If the user releases the mouse
//! outside the drop zone — or a touch is interrupted in a way that doesn't
//! reach the local handler — drag signals can leak. A leaked signal leaves
//! styles like `touch-action: none` or `opacity-30` stuck until the
//! component re-mounts, which the user sees as "scroll broken" or "buttons
//! don't respond".
//!
//! `use_drag_cleanup` registers a window-level `pointerup` and
//! `pointercancel` listener. Document/window listeners fire last in bubble
//! order, so any local handler still gets a chance to commit its drop
//! before this one runs.
use std::rc::Rc;

use dioxus::prelude::*;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

struct Guard {
    cb: Closure<dyn FnMut(web_sys::PointerEvent)>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(win) = web_sys::window() {
            let f: &js_sys::Function = self.cb.as_ref().unchecked_ref();
            let _ = win.remove_event_listener_with_callback("pointerup", f);
            let _ = win.remove_event_listener_with_callback("pointercancel", f);
        }
    }
}

pub fn use_drag_cleanup<F>(mut on_end: F)
where
    F: FnMut() + 'static,
{
    use_hook(|| {
        let cb = Closure::<dyn FnMut(web_sys::PointerEvent)>::new(move |_e| {
            on_end();
        });
        if let Some(win) = web_sys::window() {
            let f: &js_sys::Function = cb.as_ref().unchecked_ref();
            let _ = win.add_event_listener_with_callback("pointerup", f);
            let _ = win.add_event_listener_with_callback("pointercancel", f);
        }
        Rc::new(Guard { cb })
    });
}
