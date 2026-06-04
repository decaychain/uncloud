use std::rc::Rc;

use dioxus::prelude::*;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;

use crate::components::icons::{IconCheck, IconCopy, IconX};

struct ListenerGuard {
    cb: Closure<dyn FnMut(web_sys::Event)>,
}

impl Drop for ListenerGuard {
    fn drop(&mut self) {
        if let Some(win) = web_sys::window() {
            let f: &js_sys::Function = self.cb.as_ref().unchecked_ref();
            let _ = win.remove_event_listener_with_callback("uncloud:download", f);
        }
    }
}

fn detail_string(detail: &JsValue, key: &str) -> String {
    js_sys::Reflect::get(detail, &JsValue::from_str(key))
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default()
}

fn path_segments(path: &str) -> Vec<&str> {
    path.split(|ch| ch == '/' || ch == '\\')
        .filter(|part| !part.is_empty())
        .collect()
}

fn short_location(path: &str, fallback_name: &str) -> String {
    let parts = path_segments(path);
    let file = parts
        .last()
        .copied()
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback_name);
    let folder = parts
        .len()
        .checked_sub(2)
        .and_then(|idx| parts.get(idx))
        .copied()
        .filter(|s| !s.is_empty())
        .unwrap_or("Downloads");
    format!("{file} in {folder}")
}

fn copy_to_clipboard(path: &str) {
    if path.is_empty() {
        return;
    }
    if let Some(window) = web_sys::window() {
        let _ = window.navigator().clipboard().write_text(path);
    }
}

#[component]
pub fn DownloadToast() -> Element {
    let mut visible = use_signal(|| false);
    let mut status = use_signal(|| String::new());
    let mut filename = use_signal(|| String::new());
    let mut saved_path = use_signal(|| String::new());
    let mut error = use_signal(|| String::new());
    let mut copied = use_signal(|| false);

    use_hook(move || {
        let cb = Closure::<dyn FnMut(web_sys::Event)>::new(move |e: web_sys::Event| {
            let Some(custom) = e.dyn_ref::<web_sys::CustomEvent>() else {
                return;
            };
            let detail = custom.detail();
            let next_status = detail_string(&detail, "status");
            status.set(next_status.clone());
            filename.set(detail_string(&detail, "filename"));
            saved_path.set(detail_string(&detail, "path"));
            error.set(detail_string(&detail, "error"));
            copied.set(false);
            visible.set(!next_status.is_empty());
        });
        if let Some(win) = web_sys::window() {
            let f: &js_sys::Function = cb.as_ref().unchecked_ref();
            let _ = win.add_event_listener_with_callback("uncloud:download", f);
        }
        Rc::new(ListenerGuard { cb })
    });

    if !*visible.read() {
        return rsx! {};
    }

    let current_status = status();
    let is_running = current_status == "started";
    let is_error = current_status == "failed";
    let is_complete = current_status == "completed";
    let title = if is_running {
        "Downloading"
    } else if is_error {
        "Download failed"
    } else {
        "Saved to Downloads"
    };
    let file_name = filename();
    let full_path = saved_path();
    let summary = if is_complete && !full_path.is_empty() {
        short_location(&full_path, &file_name)
    } else if file_name.is_empty() {
        "Preparing download".to_string()
    } else {
        format!("{file_name} in Downloads")
    };
    let full_path_for_open = full_path.clone();
    let full_path_for_copy = full_path.clone();
    let copied_now = copied();
    let icon_box_class = if is_running {
        "mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-primary/10 text-primary"
    } else if is_error {
        "mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-error/10 text-error"
    } else {
        "mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-success/10 text-success"
    };

    rsx! {
        div { class: "toast toast-top toast-end z-50 px-2 pt-4",
            div {
                class: "w-[calc(100vw-1rem)] sm:w-96 rounded-2xl border border-base-300 bg-base-100/95 text-base-content shadow-2xl backdrop-blur p-4",
                role: "status",
                div { class: "flex gap-3",
                    div { class: icon_box_class,
                        if is_running {
                            span { class: "loading loading-spinner loading-sm" }
                        } else if is_error {
                            IconX { class: "w-4 h-4" }
                        } else {
                            IconCheck { class: "w-4 h-4" }
                        }
                    }
                    div { class: "min-w-0 flex-1",
                        div { class: "flex items-start gap-2",
                            div { class: "min-w-0 flex-1",
                                div { class: "font-medium leading-tight", "{title}" }
                                div {
                                    class: "mt-1 text-sm text-base-content/70 truncate",
                                    title: "{full_path}",
                                    "{summary}"
                                }
                            }
                            button {
                                class: "btn btn-xs btn-circle btn-ghost -mr-1 -mt-1",
                                "aria-label": "Dismiss",
                                onclick: move |_| visible.set(false),
                                IconX { class: "w-3.5 h-3.5" }
                            }
                        }

                        if is_running {
                            div { class: "mt-3 text-xs text-base-content/50", "Saving file..." }
                        } else if is_error {
                            div { class: "mt-3 text-xs text-error break-words", "{error}" }
                        } else if is_complete {
                            div { class: "mt-3 flex flex-wrap justify-end gap-2",
                                button {
                                    class: "btn btn-xs btn-ghost",
                                    onclick: move |_| {
                                        copy_to_clipboard(&full_path_for_copy);
                                        copied.set(true);
                                    },
                                    if copied_now {
                                        IconCheck { class: "w-3.5 h-3.5" }
                                        "Copied"
                                    } else {
                                        IconCopy { class: "w-3.5 h-3.5" }
                                        "Copy path"
                                    }
                                }
                                button {
                                    class: "btn btn-xs btn-primary",
                                    onclick: move |_| {
                                        crate::hooks::tauri::open_downloaded_file(&full_path_for_open);
                                    },
                                    "Open"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
