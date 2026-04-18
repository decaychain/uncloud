use std::cell::RefCell;
use std::rc::Rc;
use dioxus::prelude::*;
use uncloud_common::FileResponse;
use wasm_bindgen::JsCast;
use crate::hooks::api::{self, api_url};
use crate::components::icons::IconX;

fn is_markdown(file: &FileResponse) -> bool {
    file.mime_type == "text/markdown"
        || file.mime_type == "text/x-markdown"
        || file.name.ends_with(".md")
        || file.name.ends_with(".markdown")
}

fn render_markdown(source: &str) -> String {
    use pulldown_cmark::{Parser, Options, html};
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    let parser = Parser::new_ext(source, opts);
    let mut html_output = String::new();
    html::push_html(&mut html_output, parser);
    html_output
}

// ── TextViewer ──────────────────────────────────────────────────────────────

#[component]
pub fn TextViewer(file: FileResponse, #[props(default = false)] start_editing: bool, on_close: EventHandler<()>) -> Element {
    let mut content: Signal<Option<Result<String, String>>> = use_signal(|| None);
    let mut editing = use_signal(move || start_editing);
    let mut draft = use_signal(String::new);
    let mut saving = use_signal(|| false);
    let mut save_error: Signal<Option<String>> = use_signal(|| None);
    let mut save_ok = use_signal(|| false);
    let file_id = file.id.clone();
    let file_name = file.name.clone();
    let markdown = is_markdown(&file);

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

            if start_editing {
                if let Ok(ref text) = result {
                    draft.set(text.clone());
                }
            }
            content.set(Some(result));
        });
    });

    // Escape key to close (only when not editing)
    {
        let on_close_kb = on_close.clone();
        use_effect(move || {
            let handler = Rc::new(RefCell::new(move |key: String| {
                if key == "Escape" && !editing() {
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
    let file_id_save = file.id.clone();

    // Save handler
    let on_save = move |_| {
        let fid = file_id_save.clone();
        saving.set(true);
        save_error.set(None);
        save_ok.set(false);
        spawn(async move {
            let text = draft();
            let result = async {
                // Create a Blob from the draft text
                let blob_parts = js_sys::Array::new();
                blob_parts.push(&wasm_bindgen::JsValue::from_str(&text));
                let opts = web_sys::BlobPropertyBag::new();
                opts.set_type("text/markdown");
                let blob = web_sys::Blob::new_with_str_sequence_and_options(&blob_parts, &opts)
                    .map_err(|_| "Failed to create Blob".to_string())?;

                let form = web_sys::FormData::new()
                    .map_err(|_| "Failed to create FormData".to_string())?;
                form.append_with_blob_and_filename("file", &blob, "file.md")
                    .map_err(|_| "Failed to append to FormData".to_string())?;

                let url = api_url(&format!("/files/{}/content", fid));
                let resp = crate::hooks::api::post_raw(&url)
                    .body(form)
                    .map_err(|e| format!("Request error: {:?}", e))?
                    .send()
                    .await
                    .map_err(|e| format!("Network error: {}", e))?;

                if resp.ok() {
                    Ok(())
                } else {
                    let body = resp.text().await.unwrap_or_default();
                    Err(format!("Save failed (HTTP {}): {}", resp.status(), body))
                }
            }
            .await;

            saving.set(false);
            match result {
                Ok(()) => {
                    // Update the content signal so preview reflects saved state
                    content.set(Some(Ok(draft())));
                    save_ok.set(true);
                }
                Err(e) => save_error.set(Some(e)),
            }
        });
    };

    let (modal_width, modal_height) = if editing() {
        ("max-w-7xl", "h-[100dvh] sm:h-[85vh]")
    } else {
        ("max-w-4xl", "max-h-[85vh]")
    };

    rsx! {
        // Backdrop
        div {
            class: if editing() {
                "fixed inset-0 z-50 bg-black/60 flex items-center justify-center p-0 sm:p-4"
            } else {
                "fixed inset-0 z-50 bg-black/60 flex items-center justify-center p-4"
            },
            onclick: move |_| {
                if !editing() { on_close.call(()); }
            },

            // Modal box — when editing (full-bleed on mobile) add safe-area
            // padding so header/footer clear the Android system bars.
            div {
                class: "modal-box w-full {modal_width} {modal_height} flex flex-col rounded-none sm:rounded-2xl",
                style: if editing() {
                    "padding-top: calc(1.5rem + env(safe-area-inset-top)); padding-bottom: calc(1.5rem + env(safe-area-inset-bottom))"
                } else {
                    ""
                },
                onclick: move |e| e.stop_propagation(),

                // Header
                div { class: "flex items-center justify-between mb-4",
                    h3 { class: "font-bold text-lg truncate", "{file_name}" }
                    div { class: "flex items-center gap-2",
                        if markdown && editing() {
                            span { class: "badge badge-info badge-sm", "Editing" }
                        }
                        button {
                            class: "btn btn-ghost btn-circle btn-sm",
                            onclick: move |_| {
                                if editing() {
                                    editing.set(false);
                                    save_ok.set(false);
                                    save_error.set(None);
                                } else {
                                    on_close.call(());
                                }
                            },
                            if editing() {
                                "Back"
                            } else {
                                IconX {}
                            }
                        }
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
                    Some(Ok(text)) => {
                        if editing() && markdown {
                            // Split editor: textarea + live preview
                            let preview_html = render_markdown(&draft());
                            rsx! {
                                div { class: "flex flex-col sm:flex-row gap-4 flex-1 overflow-hidden min-h-0",
                                    // Editor pane
                                    div { class: "flex-1 flex flex-col min-w-0 min-h-0",
                                        div { class: "text-xs font-semibold text-base-content/60 mb-1", "Source" }
                                        textarea {
                                            class: "textarea textarea-bordered w-full flex-1 font-mono text-sm resize-none",
                                            value: "{draft}",
                                            oninput: move |e| {
                                                draft.set(e.value());
                                                save_ok.set(false);
                                            },
                                        }
                                    }
                                    // Preview pane
                                    div { class: "flex-1 flex flex-col min-w-0 min-h-0",
                                        div { class: "text-xs font-semibold text-base-content/60 mb-1", "Preview" }
                                        div {
                                            class: "prose prose-sm max-w-none overflow-auto flex-1 bg-base-200 rounded p-4",
                                            dangerous_inner_html: "{preview_html}",
                                        }
                                    }
                                }
                            }
                        } else if editing() {
                            // Plain text editor (full width)
                            rsx! {
                                div { class: "flex-1 flex flex-col overflow-hidden min-h-0",
                                    textarea {
                                        class: "textarea textarea-bordered w-full flex-1 font-mono text-sm resize-none",
                                        value: "{draft}",
                                        oninput: move |e| {
                                            draft.set(e.value());
                                            save_ok.set(false);
                                        },
                                    }
                                }
                            }
                        } else if markdown {
                            // Read-only rendered markdown
                            let rendered = render_markdown(&text);
                            rsx! {
                                div {
                                    class: "prose prose-sm max-w-none overflow-auto flex-1 bg-base-200 rounded p-4",
                                    dangerous_inner_html: "{rendered}",
                                }
                            }
                        } else {
                            // Plain text read-only
                            rsx! {
                                pre {
                                    class: "bg-base-200 rounded p-4 text-sm overflow-auto flex-1 font-mono whitespace-pre-wrap break-all",
                                    "{text}"
                                }
                            }
                        }
                    },
                }

                // Footer
                div { class: "modal-action flex-wrap gap-2",
                    if let Some(err) = save_error() {
                        div { class: "alert alert-error alert-sm text-sm flex-1", "{err}" }
                    }
                    if save_ok() {
                        div { class: "alert alert-success alert-sm text-sm", "Saved" }
                    }

                    if editing() {
                        button {
                            class: "btn btn-primary",
                            disabled: saving(),
                            onclick: on_save,
                            if saving() {
                                span { class: "loading loading-spinner loading-sm" }
                            }
                            "Save"
                        }
                    } else {
                        button {
                            class: "btn btn-primary",
                            onclick: move |_| {
                                if let Some(Ok(ref text)) = content() {
                                    draft.set(text.clone());
                                }
                                editing.set(true);
                                save_ok.set(false);
                            },
                            "Edit"
                        }
                    }

                    a {
                        class: "btn btn-ghost",
                        href: "{download_url}",
                        download: "{file.name}",
                        "Download"
                    }
                    button {
                        class: "btn",
                        onclick: move |_| {
                            editing.set(false);
                            on_close.call(());
                        },
                        "Close"
                    }
                }
            }
        }
    }
}
