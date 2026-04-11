//! Lucide icons (https://lucide.dev) — inlined as SVG components.
//!
//! All icons use `stroke="currentColor"` so they inherit the surrounding text
//! colour, which means DaisyUI theme + `.active` styling colours them for free.

use dioxus::prelude::*;

fn default_class() -> String { "w-4 h-4".to_string() }

macro_rules! lucide_icon {
    ($name:ident, $($body:tt)*) => {
        #[component]
        pub fn $name(#[props(default = default_class())] class: String) -> Element {
            rsx! {
                svg {
                    class: "{class} shrink-0",
                    xmlns: "http://www.w3.org/2000/svg",
                    width: "24",
                    height: "24",
                    view_box: "0 0 24 24",
                    fill: "none",
                    stroke: "currentColor",
                    stroke_width: "2",
                    stroke_linecap: "round",
                    stroke_linejoin: "round",
                    $($body)*
                }
            }
        }
    };
}

lucide_icon!(IconFolder,
    path { d: "M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z" }
);

/// Filled folder — uses `fill="currentColor"` for a solid shape so it reads as
/// distinct from the stroke-based file icons. Meant for the file browser grid.
#[component]
pub fn IconFolderSolid(#[props(default = default_class())] class: String) -> Element {
    rsx! {
        svg {
            class: "{class} shrink-0",
            xmlns: "http://www.w3.org/2000/svg",
            width: "24",
            height: "24",
            view_box: "0 0 24 24",
            fill: "currentColor",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            path { d: "M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z" }
        }
    }
}

lucide_icon!(IconImage,
    rect { width: "18", height: "18", x: "3", y: "3", rx: "2", ry: "2" }
    circle { cx: "9", cy: "9", r: "2" }
    path { d: "m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21" }
);

lucide_icon!(IconMusic,
    path { d: "M9 18V5l12-2v13" }
    circle { cx: "6", cy: "18", r: "3" }
    circle { cx: "18", cy: "16", r: "3" }
);

lucide_icon!(IconShoppingCart,
    circle { cx: "8", cy: "21", r: "1" }
    circle { cx: "19", cy: "21", r: "1" }
    path { d: "M2.05 2.05h2l2.66 12.42a2 2 0 0 0 2 1.58h9.78a2 2 0 0 0 1.95-1.57l1.65-7.43H5.12" }
);

lucide_icon!(IconKey,
    path { d: "m21 2-9.6 9.6" }
    circle { cx: "7.5", cy: "15.5", r: "5.5" }
    path { d: "m21 2-1.5 1.5" }
    path { d: "M15.5 7.5 18 10" }
);

lucide_icon!(IconSettings,
    path { d: "M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z" }
    circle { cx: "12", cy: "12", r: "3" }
);

lucide_icon!(IconUser,
    path { d: "M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2" }
    circle { cx: "12", cy: "7", r: "4" }
);

lucide_icon!(IconPalette,
    circle { cx: "13.5", cy: "6.5", r: ".5", fill: "currentColor" }
    circle { cx: "17.5", cy: "10.5", r: ".5", fill: "currentColor" }
    circle { cx: "8.5", cy: "7.5", r: ".5", fill: "currentColor" }
    circle { cx: "6.5", cy: "12.5", r: ".5", fill: "currentColor" }
    path { d: "M12 2C6.5 2 2 6.5 2 12s4.5 10 10 10c.926 0 1.648-.746 1.648-1.688 0-.437-.18-.835-.437-1.125-.29-.289-.438-.652-.438-1.125a1.64 1.64 0 0 1 1.668-1.668h1.996c3.051 0 5.555-2.503 5.555-5.554C21.965 6.012 17.461 2 12 2z" }
);

lucide_icon!(IconUsers,
    path { d: "M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2" }
    circle { cx: "9", cy: "7", r: "4" }
    path { d: "M22 21v-2a4 4 0 0 0-3-3.87" }
    path { d: "M16 3.13a4 4 0 0 1 0 7.75" }
);

lucide_icon!(IconShield,
    path { d: "M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z" }
);

lucide_icon!(IconLink,
    path { d: "M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" }
    path { d: "M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" }
);

lucide_icon!(IconTrash,
    path { d: "M3 6h18" }
    path { d: "M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6" }
    path { d: "M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" }
    line { x1: "10", x2: "10", y1: "11", y2: "17" }
    line { x1: "14", x2: "14", y1: "11", y2: "17" }
);

lucide_icon!(IconListMusic,
    path { d: "M21 15V6" }
    path { d: "M18.5 18a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5Z" }
    path { d: "M12 12H3" }
    path { d: "M16 6H3" }
    path { d: "M12 18H3" }
);

lucide_icon!(IconX,
    path { d: "M18 6 6 18" }
    path { d: "m6 6 12 12" }
);

lucide_icon!(IconCheck,
    path { d: "M20 6 9 17l-5-5" }
);

lucide_icon!(IconCopy,
    rect { width: "14", height: "14", x: "8", y: "8", rx: "2", ry: "2" }
    path { d: "M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2" }
);

lucide_icon!(IconClipboard,
    rect { width: "8", height: "4", x: "8", y: "2", rx: "1", ry: "1" }
    path { d: "M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2" }
);

lucide_icon!(IconPencil,
    path { d: "M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z" }
    path { d: "m15 5 4 4" }
);

lucide_icon!(IconUpload,
    path { d: "M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" }
    polyline { points: "17 8 12 3 7 8" }
    line { x1: "12", y1: "3", x2: "12", y2: "15" }
);

lucide_icon!(IconFolderPlus,
    path { d: "M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z" }
    line { x1: "12", y1: "10", x2: "12", y2: "16" }
    line { x1: "9", y1: "13", x2: "15", y2: "13" }
);

lucide_icon!(IconFolderOpen,
    path { d: "m6 14 1.5-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.54 6a2 2 0 0 1-1.95 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H18a2 2 0 0 1 2 2v2" }
);

lucide_icon!(IconFile,
    path { d: "M14.5 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7.5L14.5 2z" }
    polyline { points: "14 2 14 8 20 8" }
);

lucide_icon!(IconFileText,
    path { d: "M14.5 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7.5L14.5 2z" }
    polyline { points: "14 2 14 8 20 8" }
    line { x1: "16", y1: "13", x2: "8", y2: "13" }
    line { x1: "16", y1: "17", x2: "8", y2: "17" }
    line { x1: "10", y1: "9", x2: "8", y2: "9" }
);

lucide_icon!(IconFileArchive,
    path { d: "M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" }
    polyline { points: "14 2 14 8 20 8" }
    circle { cx: "10", cy: "20", r: "2" }
    path { d: "M10 7V6" }
    path { d: "M10 12v-1" }
    path { d: "M10 18v-2" }
);

lucide_icon!(IconFilm,
    rect { width: "20", height: "20", x: "2", y: "2", rx: "2.18", ry: "2.18" }
    line { x1: "7", y1: "2", x2: "7", y2: "22" }
    line { x1: "17", y1: "2", x2: "17", y2: "22" }
    line { x1: "2", y1: "12", x2: "22", y2: "12" }
    line { x1: "2", y1: "7", x2: "7", y2: "7" }
    line { x1: "2", y1: "17", x2: "7", y2: "17" }
    line { x1: "17", y1: "17", x2: "22", y2: "17" }
    line { x1: "17", y1: "7", x2: "22", y2: "7" }
);

lucide_icon!(IconAlertTriangle,
    path { d: "m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3" }
    line { x1: "12", y1: "9", x2: "12", y2: "13" }
    line { x1: "12", y1: "17", x2: "12.01", y2: "17" }
);

lucide_icon!(IconLock,
    rect { width: "18", height: "11", x: "3", y: "11", rx: "2", ry: "2" }
    path { d: "M7 11V7a5 5 0 0 1 10 0v4" }
);

lucide_icon!(IconLockOpen,
    rect { width: "18", height: "11", x: "3", y: "11", rx: "2", ry: "2" }
    path { d: "M7 11V7a5 5 0 0 1 9.9-1" }
);

lucide_icon!(IconHistory,
    path { d: "M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" }
    path { d: "M3 3v5h5" }
    path { d: "M12 7v5l4 2" }
);

lucide_icon!(IconMoreVertical,
    circle { cx: "12", cy: "12", r: "1" }
    circle { cx: "12", cy: "5", r: "1" }
    circle { cx: "12", cy: "19", r: "1" }
);

lucide_icon!(IconChevronRight,
    path { d: "m9 18 6-6-6-6" }
);

lucide_icon!(IconChevronDown,
    path { d: "m6 9 6 6 6-6" }
);

lucide_icon!(IconMoveRight,
    path { d: "M18 8 22 12 18 16" }
    path { d: "M2 12h20" }
);

lucide_icon!(IconSearch,
    circle { cx: "11", cy: "11", r: "8" }
    path { d: "m21 21-4.3-4.3" }
);

lucide_icon!(IconGrid,
    rect { width: "7", height: "7", x: "3", y: "3", rx: "1" }
    rect { width: "7", height: "7", x: "14", y: "3", rx: "1" }
    rect { width: "7", height: "7", x: "14", y: "14", rx: "1" }
    rect { width: "7", height: "7", x: "3", y: "14", rx: "1" }
);

lucide_icon!(IconList,
    line { x1: "8", y1: "6", x2: "21", y2: "6" }
    line { x1: "8", y1: "12", x2: "21", y2: "12" }
    line { x1: "8", y1: "18", x2: "21", y2: "18" }
    line { x1: "3", y1: "6", x2: "3.01", y2: "6" }
    line { x1: "3", y1: "12", x2: "3.01", y2: "12" }
    line { x1: "3", y1: "18", x2: "3.01", y2: "18" }
);

lucide_icon!(IconMenu,
    line { x1: "4", y1: "6", x2: "20", y2: "6" }
    line { x1: "4", y1: "12", x2: "20", y2: "12" }
    line { x1: "4", y1: "18", x2: "20", y2: "18" }
);

lucide_icon!(IconEye,
    path { d: "M2 12s3-7 10-7 10 7 10 7-3 7-10 7-10-7-10-7Z" }
    circle { cx: "12", cy: "12", r: "3" }
);

lucide_icon!(IconDownload,
    path { d: "M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" }
    polyline { points: "7 10 12 15 17 10" }
    line { x1: "12", y1: "15", x2: "12", y2: "3" }
);

lucide_icon!(IconShare,
    circle { cx: "18", cy: "5", r: "3" }
    circle { cx: "6", cy: "12", r: "3" }
    circle { cx: "18", cy: "19", r: "3" }
    line { x1: "8.59", y1: "13.51", x2: "15.42", y2: "17.49" }
    line { x1: "15.41", y1: "6.51", x2: "8.59", y2: "10.49" }
);

lucide_icon!(IconPlay,
    polygon { points: "6 3 20 12 6 21 6 3" }
);

lucide_icon!(IconPause,
    rect { x: "6", y: "4", width: "4", height: "16", rx: "1" }
    rect { x: "14", y: "4", width: "4", height: "16", rx: "1" }
);

lucide_icon!(IconSkipBack,
    polygon { points: "19 20 9 12 19 4 19 20" }
    line { x1: "5", y1: "19", x2: "5", y2: "5" }
);

lucide_icon!(IconSkipForward,
    polygon { points: "5 4 15 12 5 20 5 4" }
    line { x1: "19", y1: "5", x2: "19", y2: "19" }
);

lucide_icon!(IconVolume2,
    polygon { points: "11 5 6 9 2 9 2 15 6 15 11 19 11 5" }
    path { d: "M15.54 8.46a5 5 0 0 1 0 7.07" }
    path { d: "M19.07 4.93a10 10 0 0 1 0 14.14" }
);

lucide_icon!(IconPlus,
    line { x1: "12", y1: "5", x2: "12", y2: "19" }
    line { x1: "5", y1: "12", x2: "19", y2: "12" }
);

lucide_icon!(IconArrowUp,
    line { x1: "12", y1: "19", x2: "12", y2: "5" }
    polyline { points: "5 12 12 5 19 12" }
);

lucide_icon!(IconArrowDown,
    line { x1: "12", y1: "5", x2: "12", y2: "19" }
    polyline { points: "19 12 12 19 5 12" }
);

/// Pick a file-type icon component based on MIME type.
/// Returns a rendered Element with the given class.
pub fn file_type_icon(mime: Option<&str>, is_folder: bool, class: &str) -> Element {
    if is_folder {
        return rsx! { IconFolderSolid { class: format!("{} text-warning", class) } };
    }
    match mime {
        Some(t) if t.starts_with("image/") => rsx! { IconImage { class: class.to_string() } },
        Some(t) if t.starts_with("video/") => rsx! { IconFilm { class: class.to_string() } },
        Some(t) if t.starts_with("audio/") => rsx! { IconMusic { class: class.to_string() } },
        Some(t) if t.starts_with("text/") => rsx! { IconFileText { class: class.to_string() } },
        Some("application/pdf") => rsx! { IconFileText { class: class.to_string() } },
        Some(t) if t.contains("zip") || t.contains("tar") || t.contains("rar") => {
            rsx! { IconFileArchive { class: class.to_string() } }
        }
        _ => rsx! { IconFile { class: class.to_string() } },
    }
}
