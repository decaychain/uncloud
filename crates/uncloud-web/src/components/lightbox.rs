use std::cell::RefCell;
use std::rc::Rc;
use dioxus::prelude::*;
use uncloud_common::FileResponse;
use wasm_bindgen::JsCast;
use crate::components::icons::{IconChevronRight, IconX};
use crate::hooks::api;

#[component]
pub fn Lightbox(
    images: Vec<FileResponse>,
    initial_index: usize,
    on_close: EventHandler<()>,
) -> Element {
    let mut index = use_signal(|| initial_index);
    let total = images.len();

    // Keyboard navigation: Escape, ArrowLeft, ArrowRight
    {
        let on_close_kb = on_close.clone();
        use_effect(move || {
            let max_idx = total.saturating_sub(1);
            let handler = Rc::new(RefCell::new(move |key: String| {
                match key.as_str() {
                    "Escape" => on_close_kb.call(()),
                    "ArrowLeft" => {
                        let cur = index();
                        if cur > 0 {
                            index.set(cur - 1);
                        }
                    }
                    "ArrowRight" => {
                        let cur = index();
                        if cur < max_idx {
                            index.set(cur + 1);
                        }
                    }
                    _ => {}
                }
            }));

            let handler_clone = handler.clone();
            let closure = wasm_bindgen::closure::Closure::wrap(Box::new(
                move |evt: web_sys::KeyboardEvent| {
                    handler_clone.borrow_mut()(evt.key());
                },
            ) as Box<dyn FnMut(web_sys::KeyboardEvent)>);

            if let Some(window) = web_sys::window() {
                let _ = window.add_event_listener_with_callback(
                    "keydown",
                    closure.as_ref().unchecked_ref(),
                );
            }
            closure.forget();
        });
    }

    let current = &images[index()];
    let src = api::authenticated_media_url(&format!("/files/{}/download", current.id));
    let can_prev = index() > 0;
    let can_next = index() < total - 1;
    let current_num = index() + 1;
    let mut img_loading = use_signal(|| true);

    rsx! {
        div {
            class: "fixed inset-0 z-50 bg-black/90 flex items-center justify-center",
            onclick: move |_| on_close.call(()),

            // Close button — offset down past the Android status bar.
            button {
                class: "absolute right-4 btn btn-ghost btn-circle text-white z-50",
                style: "top: calc(1rem + env(safe-area-inset-top))",
                onclick: move |e| { e.stop_propagation(); on_close.call(()); },
                IconX { class: "w-6 h-6".to_string() }
            }

            // Prev
            if can_prev {
                button {
                    class: "absolute left-4 btn btn-ghost btn-circle text-white z-50",
                    onclick: move |e| { e.stop_propagation(); index.set(index() - 1); },
                    IconChevronRight { class: "w-6 h-6 rotate-180".to_string() }
                }
            }

            // Next
            if can_next {
                button {
                    class: "absolute right-4 btn btn-ghost btn-circle text-white z-50",
                    onclick: move |e| { e.stop_propagation(); index.set(index() + 1); },
                    IconChevronRight { class: "w-6 h-6".to_string() }
                }
            }

            // Loading spinner while image loads
            if img_loading() {
                div { class: "absolute",
                    span { class: "loading loading-spinner loading-lg text-white" }
                }
            }

            // Image
            img {
                class: "max-w-[90vw] max-h-[90vh] object-contain",
                src: "{src}",
                onclick: move |e| e.stop_propagation(),
                onload: move |_| img_loading.set(false),
            }

            // Bottom bar
            div {
                class: "absolute bottom-0 left-0 right-0 bg-gradient-to-t from-black/60 to-transparent p-4 flex items-center justify-between text-white",
                onclick: move |e| e.stop_propagation(),
                div {
                    p { class: "font-medium truncate max-w-xs", "{current.name}" }
                    p { class: "text-sm text-white/70", "{current_num} / {total}" }
                }
                a {
                    class: "btn btn-ghost btn-sm text-white",
                    href: "{src}",
                    download: "{current.name}",
                    "Download"
                }
            }
        }
    }
}
