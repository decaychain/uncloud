use std::cell::RefCell;
use std::rc::Rc;
use dioxus::prelude::*;
use uncloud_common::FileResponse;
use wasm_bindgen::JsCast;
use crate::hooks::api::{self, api_url};

// ── TextViewer ──────────────────────────────────────────────────────────────

#[component]
pub fn TextViewer(file: FileResponse, on_close: EventHandler<()>) -> Element {
    let mut content: Signal<Option<Result<String, String>>> = use_signal(|| None);
    let file_id = file.id.clone();
    let file_name = file.name.clone();

    // Fetch the file content on mount
    use_effect(move || {
        let fid = file_id.clone();
        spawn(async move {
            let url = api_url(&format!("/files/{}/download", fid));
            let result = async {
                let resp = crate::hooks::api::get_raw(&url)
                    .send()
                    .await
                    .map_err(|e| format!("Network error: {}", e))?;

                if !resp.ok() {
                    return Err(format!("HTTP {}", resp.status()));
                }

                let text = resp
                    .text()
                    .await
                    .map_err(|e| format!("Read error: {}", e))?;

                // Truncate at 1 MB
                const MAX_LEN: usize = 1_000_000;
                if text.len() > MAX_LEN {
                    let truncated = text[..MAX_LEN].to_string();
                    Ok(format!("{}\n\n[File truncated at 1 MB]", truncated))
                } else {
                    Ok(text)
                }
            }
            .await;

            content.set(Some(result));
        });
    });

    // Escape key to close
    {
        let on_close_kb = on_close.clone();
        use_effect(move || {
            let handler = Rc::new(RefCell::new(move |key: String| {
                if key == "Escape" {
                    on_close_kb.call(());
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

    let download_url = api::authenticated_media_url(&format!("/files/{}/download", file.id));

    rsx! {
        // Backdrop
        div {
            class: "fixed inset-0 z-50 bg-black/60 flex items-center justify-center p-4",
            onclick: move |_| on_close.call(()),

            // Modal box
            div {
                class: "modal-box w-full max-w-4xl max-h-[85vh] flex flex-col",
                onclick: move |e| e.stop_propagation(),

                // Header
                div { class: "flex items-center justify-between mb-4",
                    h3 { class: "font-bold text-lg truncate", "{file_name}" }
                    button {
                        class: "btn btn-ghost btn-circle btn-sm",
                        onclick: move |_| on_close.call(()),
                        "✕"
                    }
                }

                // Body
                match content() {
                    None => rsx! {
                        div { class: "flex items-center justify-center flex-1 py-20",
                            span { class: "loading loading-spinner loading-lg" }
                        }
                    },
                    Some(Err(err)) => rsx! {
                        div { class: "alert alert-error", "{err}" }
                    },
                    Some(Ok(text)) => rsx! {
                        pre {
                            class: "bg-base-200 rounded p-4 text-sm overflow-auto flex-1 font-mono whitespace-pre-wrap break-all",
                            "{text}"
                        }
                    },
                }

                // Footer
                div { class: "modal-action",
                    a {
                        class: "btn btn-ghost",
                        href: "{download_url}",
                        download: "{file.name}",
                        "Download"
                    }
                    button {
                        class: "btn",
                        onclick: move |_| on_close.call(()),
                        "Close"
                    }
                }
            }
        }
    }
}
