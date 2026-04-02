use dioxus::prelude::*;

#[derive(Clone, PartialEq)]
pub struct ContextMenuPosition {
    pub x: i32,
    pub y: i32,
}

#[component]
pub fn ContextMenu(
    position: ContextMenuPosition,
    on_close: EventHandler<()>,
    children: Element,
) -> Element {
    let on_backdrop_click = move |_| {
        on_close.call(());
    };

    rsx! {
        div {
            style: "position: fixed; inset: 0; z-index: 999;",
            onclick: on_backdrop_click,

            ul {
                class: "menu menu-sm bg-base-100 rounded-box shadow-lg border border-base-300 absolute p-1 w-48",
                style: "left: {position.x}px; top: {position.y}px;",
                onclick: move |evt| evt.stop_propagation(),
                {children}
            }
        }
    }
}

#[component]
pub fn ContextMenuItem(
    icon: String,
    label: String,
    danger: Option<bool>,
    on_click: EventHandler<()>,
) -> Element {
    let item_class = if danger.unwrap_or(false) {
        "text-error"
    } else {
        ""
    };

    rsx! {
        li {
            a {
                class: item_class,
                onclick: move |_| on_click.call(()),
                span { "{icon}" }
                span { "{label}" }
            }
        }
    }
}

#[component]
pub fn ContextMenuDivider() -> Element {
    rsx! {
        li { div { class: "divider my-0" } }
    }
}
