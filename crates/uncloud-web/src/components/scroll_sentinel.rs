//! `ScrollSentinel` — an invisible div that fires `on_visible` when it
//! scrolls into view. Used to drive infinite-scroll patterns across
//! gallery, finance transactions, etc.

use dioxus::prelude::*;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

/// Renders an invisible sentinel at the end of a list. When it enters
/// the viewport (with 400px lead-in), `on_visible` fires. The parent
/// uses that to fetch the next page.
#[component]
pub fn ScrollSentinel(on_visible: EventHandler<()>) -> Element {
    let id = use_hook(|| {
        let r = js_sys::Math::random().to_bits();
        format!("scroll-sentinel-{:x}", r)
    });

    let id_effect = id.clone();
    use_effect(move || {
        let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return; };
        let Some(sentinel) = doc.get_element_by_id(&id_effect) else { return; };

        let callback = Closure::wrap(Box::new(
            move |entries: js_sys::Array, _: web_sys::IntersectionObserver| {
                for i in 0..entries.length() {
                    if let Some(entry) = entries
                        .get(i)
                        .dyn_ref::<web_sys::IntersectionObserverEntry>()
                    {
                        if entry.is_intersecting() {
                            on_visible.call(());
                            break;
                        }
                    }
                }
            },
        )
            as Box<dyn FnMut(js_sys::Array, web_sys::IntersectionObserver)>);

        let options = web_sys::IntersectionObserverInit::new();
        options.set_root_margin("400px");

        if let Ok(observer) = web_sys::IntersectionObserver::new_with_options(
            callback.as_ref().unchecked_ref(),
            &options,
        ) {
            observer.observe(&sentinel);
            // The browser keeps the observer alive while it's observing
            // a live element; unmounting the sentinel ends observation.
            callback.forget();
            std::mem::forget(observer);
        }
    });

    rsx! { div { id: "{id}", class: "h-px" } }
}
