use dioxus::prelude::*;
use gloo_timers::future::TimeoutFuture;
use uncloud_common::SearchHit;

use crate::components::icons::{file_type_icon, IconSearch, IconX};
use crate::hooks::use_search::search_files;
use crate::router::Route;
use crate::state::HighlightTarget;

fn format_size(bytes: i64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

#[component]
pub fn SearchBar() -> Element {
    let mut query = use_signal(|| String::new());
    let mut results: Signal<Vec<SearchHit>> = use_signal(Vec::new);
    let mut loading = use_signal(|| false);
    let mut show_dropdown = use_signal(|| false);
    let mut debounce_gen = use_signal(|| 0u32);
    let nav = use_navigator();

    let on_input = move |evt: Event<FormData>| {
        let val: String = evt.value();
        query.set(val.clone());

        if val.trim().is_empty() {
            results.set(vec![]);
            show_dropdown.set(false);
            return;
        }

        // Bump generation to cancel previous debounce
        let gen = debounce_gen() + 1;
        debounce_gen.set(gen);

        spawn(async move {
            TimeoutFuture::new(300).await;
            // Check if this is still the latest input
            if debounce_gen() != gen {
                return;
            }
            loading.set(true);
            show_dropdown.set(true);
            match search_files(&query(), 20).await {
                Ok(hits) => {
                    // Only update if still the latest generation
                    if debounce_gen() == gen {
                        results.set(hits);
                    }
                }
                Err(_) => {
                    results.set(vec![]);
                }
            }
            loading.set(false);
        });
    };

    let on_focusout = move |_: Event<FocusData>| {
        // Delay closing so click handlers on results fire first
        spawn(async move {
            TimeoutFuture::new(200).await;
            show_dropdown.set(false);
        });
    };

    let on_focus = move |_: Event<FocusData>| {
        if !query().trim().is_empty() && !results().is_empty() {
            show_dropdown.set(true);
        }
    };

    rsx! {
        div { class: "relative w-full",
            onfocusout: on_focusout,

            // Search input
            div { class: "form-control",
                div { class: "input input-sm input-bordered flex items-center gap-2 w-full",
                    IconSearch { class: "w-4 h-4 opacity-70".to_string() }
                    input {
                        r#type: "text",
                        placeholder: "Search files...",
                        class: "grow bg-transparent border-none outline-none text-sm",
                        value: "{query}",
                        oninput: on_input,
                        onfocus: on_focus,
                    }
                    if loading() {
                        span { class: "loading loading-spinner loading-xs" }
                    }
                }
            }

            // Results dropdown
            if show_dropdown() {
                div {
                    class: "absolute top-full mt-1 left-0 right-0 min-w-72 bg-base-100 shadow-xl rounded-box border border-base-300 z-50 max-h-80 overflow-y-auto",

                    if results().is_empty() && !loading() && !query().trim().is_empty() {
                        div { class: "p-4 text-sm text-base-content/50 text-center",
                            "No results found"
                        }
                    }

                    for hit in results() {
                        {
                            let hit_id = hit.id.clone();
                            let hit_parent = hit.parent_id.clone();
                            let hit_name = hit.name.clone();
                            let hit_mime = hit.mime_type.clone();
                            let hit_size = hit.size_bytes;
                            let size_str = format_size(hit_size);
                            let nav = nav.clone();
                            let hit_id_hl = hit_id.clone();

                            rsx! {
                                button {
                                    class: "flex items-center gap-3 w-full px-3 py-2 hover:bg-base-200 transition-colors text-left cursor-pointer",
                                    onclick: move |_| {
                                        let target = match &hit_parent {
                                            Some(pid) => Route::Folder { id: pid.clone() },
                                            None => Route::Home {},
                                        };
                                        // Set highlight target before navigating
                                        let mut hl = consume_context::<Signal<HighlightTarget>>();
                                        hl.set(HighlightTarget { file_id: Some(hit_id_hl.clone()) });
                                        query.set(String::new());
                                        results.set(vec![]);
                                        show_dropdown.set(false);
                                        nav.push(target);
                                    },
                                    // Icon
                                    span { class: "flex-shrink-0 text-base-content/60",
                                        {file_type_icon(Some(&hit_mime), false, "w-5 h-5")}
                                    }
                                    // Name + size
                                    div { class: "flex-1 min-w-0",
                                        div { class: "text-sm font-medium truncate", "{hit_name}" }
                                        div { class: "text-xs text-base-content/50", "{size_str}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Search icon for mobile — opens a full-screen search overlay.
#[component]
pub fn SearchIconMobile() -> Element {
    let mut open = use_signal(|| false);

    rsx! {
        // Icon button — visible only on small screens
        button {
            class: "btn btn-ghost btn-circle sm:hidden",
            title: "Search",
            onclick: move |_| open.set(true),
            IconSearch { class: "w-5 h-5".to_string() }
        }

        // Full-screen overlay — extra top/bottom padding so controls clear
        // the Android system bars.
        if open() {
            div {
                class: "fixed inset-0 z-50 bg-base-100 px-4 flex flex-col gap-3",
                style: "padding-top: calc(1rem + env(safe-area-inset-top)); padding-bottom: calc(1rem + env(safe-area-inset-bottom))",
                div { class: "flex items-center gap-2",
                    button {
                        class: "btn btn-ghost btn-circle btn-sm",
                        onclick: move |_| open.set(false),
                        IconX {}
                    }
                    div { class: "flex-1",
                        SearchBar {}
                    }
                }
            }
        }
    }
}
