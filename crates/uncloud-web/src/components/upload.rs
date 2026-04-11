use dioxus::prelude::*;
use web_sys::wasm_bindgen::JsCast;
use web_sys::HtmlInputElement;

use crate::components::icons::{IconAlertTriangle, IconFolderOpen};

pub(crate) const FILE_INPUT_ID: &str = "uc-file-upload";

#[component]
pub fn UploadZone(
    parent_id: Option<String>,
    on_complete: EventHandler<()>,
    /// When false only the hidden file input is rendered; the visible drop zone is hidden.
    show_zone: bool,
) -> Element {
    let mut uploading = use_signal(|| false);
    let mut progress = use_signal(|| 0.0f64);
    let mut dragover = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    // Delegate clicks on the zone to the hidden file input.
    let on_zone_click = move |_| {
        if let Some(elem) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id(FILE_INPUT_ID))
        {
            elem.unchecked_into::<HtmlInputElement>().click();
        }
    };

    // Called when the user picks files via the dialog.
    let on_file_change = {
        let parent_id = parent_id.clone();
        move |_: Event<FormData>| {
            let parent_id = parent_id.clone();

            let file_list = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.get_element_by_id(FILE_INPUT_ID))
                .and_then(|e| e.dyn_into::<HtmlInputElement>().ok())
                .and_then(|i| i.files());

            let Some(file_list) = file_list else { return };
            if file_list.length() == 0 {
                return;
            }

            let files: Vec<web_sys::File> = (0..file_list.length())
                .filter_map(|i| file_list.item(i))
                .collect();

            uploading.set(true);
            error.set(None);
            progress.set(0.0);

            spawn(async move {
                let total = files.len() as f64;

                for (i, file) in files.iter().enumerate() {
                    progress.set(i as f64 / total);

                    if let Err(e) =
                        crate::hooks::use_files::upload_file(file, parent_id.as_deref()).await
                    {
                        error.set(Some(e));
                        uploading.set(false);
                        return;
                    }
                }

                progress.set(1.0);
                uploading.set(false);
                on_complete.call(());

                // Reset so the same file can be re-uploaded if needed.
                if let Some(input) = web_sys::window()
                    .and_then(|w| w.document())
                    .and_then(|d| d.get_element_by_id(FILE_INPUT_ID))
                    .and_then(|e| e.dyn_into::<HtmlInputElement>().ok())
                {
                    input.set_value("");
                }
            });
        }
    };

    let on_dragover = move |evt: Event<DragData>| {
        evt.prevent_default();
        dragover.set(true);
    };

    let on_dragleave = move |_| {
        dragover.set(false);
    };

    let on_drop = move |evt: Event<DragData>| {
        evt.prevent_default();
        dragover.set(false);
    };

    let zone_class = if dragover() {
        "border-2 border-dashed border-primary bg-primary/10 rounded-box p-8 text-center cursor-pointer transition-all"
    } else {
        "border-2 border-dashed border-base-300 bg-base-200/50 rounded-box p-8 text-center cursor-pointer hover:border-primary hover:bg-primary/5 transition-all"
    };

    rsx! {
        // Hidden file input — always present in the DOM so it can be triggered
        // by both the drop zone click and the toolbar Upload button.
        input {
            r#type: "file",
            id: FILE_INPUT_ID,
            multiple: true,
            style: "display: none;",
            onchange: on_file_change,
        }

        if show_zone {
            div {
                class: zone_class,
                onclick: on_zone_click,
                ondrop: on_drop,
                ondragover: on_dragover,
                ondragleave: on_dragleave,

                if uploading() {
                    div { class: "flex flex-col items-center gap-3",
                        span { class: "loading loading-spinner loading-lg" }
                        p { class: "text-sm font-medium", "Uploading..." }
                        progress {
                            class: "progress progress-primary w-full max-w-xs",
                            value: "{progress() * 100.0}",
                            max: "100",
                        }
                    }
                } else if let Some(err) = error() {
                    div { class: "flex flex-col items-center gap-2",
                        IconAlertTriangle { class: "w-8 h-8 text-warning".to_string() }
                        p { class: "text-sm text-error", "{err}" }
                        p { class: "text-xs opacity-60", "Click to try again" }
                    }
                } else {
                    div { class: "flex flex-col items-center gap-2",
                        IconFolderOpen { class: "w-12 h-12 mb-2 text-base-content/40".to_string() }
                        p { class: "text-base-content/60 text-sm", "No files yet" }
                        p { class: "text-sm mt-1",
                            strong { "Click to upload" }
                            " or drag and drop files here"
                        }
                    }
                }
            }
        }
    }
}
