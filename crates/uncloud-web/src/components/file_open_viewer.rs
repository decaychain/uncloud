//! Renders the modal for whatever [`FileOpenTarget`] is currently set.
//!
//! Pair with [`use_file_opener`](crate::hooks::use_file_opener::use_file_opener):
//! the closure writes to the signal, this component reads it and mounts the
//! corresponding overlay. Closing any overlay clears the signal.

use dioxus::prelude::*;

use crate::components::file_viewer::TextViewer;
use crate::components::lightbox::Lightbox;
use crate::hooks::use_file_opener::FileOpenTarget;

#[component]
pub fn FileOpenViewer(target: Signal<Option<FileOpenTarget>>) -> Element {
    let mut target = target;
    let Some(t) = target() else {
        return rsx! {};
    };
    match t {
        FileOpenTarget::Image { files, index } => rsx! {
            Lightbox {
                images: files,
                initial_index: index,
                on_close: move |_| target.set(None),
            }
        },
        FileOpenTarget::Text(file) => rsx! {
            TextViewer {
                file,
                on_close: move |_| target.set(None),
            }
        },
        FileOpenTarget::TextEdit(file) => rsx! {
            TextViewer {
                file,
                start_editing: true,
                on_close: move |_| target.set(None),
            }
        },
    }
}
