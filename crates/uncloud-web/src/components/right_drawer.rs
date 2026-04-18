use dioxus::prelude::*;

use crate::components::icons::IconX;

/// Right-side slide-out panel used by file & folder properties, and any future
/// detail pane. Renders a click-away backdrop plus a fixed panel on the right.
///
/// The caller controls open/close state via `open` + `on_close`.
#[component]
pub fn RightDrawer(
    /// Whether the drawer is visible.
    open: bool,
    /// Title shown in the drawer header.
    title: String,
    /// Called when the user clicks the backdrop or the close button.
    on_close: EventHandler<()>,
    /// Drawer body.
    children: Element,
) -> Element {
    if !open {
        return rsx! {};
    }

    rsx! {
        // Backdrop — closes on click.
        div {
            class: "fixed inset-0 bg-black/40 z-40",
            onclick: move |_| on_close.call(()),
        }
        // Panel — fixed to the right, full-height, scrollable body.
        div {
            class: "fixed top-0 right-0 h-full w-full sm:w-[28rem] max-w-full bg-base-100 shadow-2xl z-50 flex flex-col",
            // Header
            div { class: "flex items-center justify-between px-4 py-3 border-b border-base-300 shrink-0",
                h3 { class: "font-bold text-lg truncate", "{title}" }
                button {
                    class: "btn btn-ghost btn-sm btn-circle",
                    onclick: move |_| on_close.call(()),
                    IconX { class: "w-4 h-4".to_string() }
                }
            }
            // Body — scrolls independently.
            div { class: "flex-1 overflow-y-auto p-4",
                {children}
            }
        }
    }
}
