//! Single dispatch site for "open this file" across the app.
//!
//! [`FileBrowser`](crate::components::file_browser) and the task-attachment
//! chips in [`TaskDetail`](crate::components::tasks::task_detail) both want
//! the same behaviour when the user opens a file: route `.kdbx` to the
//! Passwords page, enqueue audio in the global player, show images in the
//! [`Lightbox`](crate::components::lightbox::Lightbox), open text in the
//! [`TextViewer`](crate::components::file_viewer::TextViewer), and fall back
//! to the authenticated download URL for everything else.
//!
//! [`use_file_opener`] captures the global pieces (player Signal, vault open
//! target, navigator) from context and returns a closure that performs the
//! dispatch. Image and text variants set a [`FileOpenTarget`] signal owned by
//! the caller; that same signal feeds [`FileOpenViewer`](crate::components::file_open_viewer::FileOpenViewer)
//! to render the actual modal.

use dioxus::prelude::*;
use uncloud_common::{AudioMeta, FileResponse, TrackResponse};

use crate::hooks::{api, use_player};
use crate::router::Route;
use crate::state::{PlayerState, VaultOpenTarget};

#[derive(Clone)]
pub enum FileOpenTarget {
    Image { files: Vec<FileResponse>, index: usize },
    Text(FileResponse),
    TextEdit(FileResponse),
}

/// Build a closure that opens a `FileResponse` using the standard dispatch.
///
/// `carousel` (passed when calling the closure) is the ordered list of files
/// to use as the lightbox carousel for image opens. Pass every visible image
/// when the user is browsing a folder; pass just the file (or every image
/// attached to the same task) elsewhere. Non-image opens ignore it.
pub fn use_file_opener(
    mut viewer: Signal<Option<FileOpenTarget>>,
) -> impl FnMut(FileResponse, Vec<FileResponse>) + Clone {
    let player = use_context::<Signal<PlayerState>>();
    let mut vault_open_target = use_context::<Signal<VaultOpenTarget>>();
    let nav = use_navigator();
    move |file: FileResponse, carousel: Vec<FileResponse>| {
        if file.name.ends_with(".kdbx") {
            vault_open_target.set(VaultOpenTarget {
                file_id: Some(file.id.clone()),
                file_name: Some(file.name.clone()),
            });
            let _ = nav.push(Route::Passwords {});
            return;
        }
        let mime = file.mime_type.as_str();
        if mime.starts_with("audio/") {
            let audio: AudioMeta = file
                .metadata
                .get("audio")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            let track = TrackResponse { file: file.clone(), audio };
            use_player::play_queue(player, vec![track], 0);
        } else if mime.starts_with("image/") {
            let idx = carousel
                .iter()
                .position(|fi| fi.id == file.id)
                .unwrap_or(0);
            viewer.set(Some(FileOpenTarget::Image { files: carousel, index: idx }));
        } else if mime.starts_with("text/")
            || mime == "application/json"
            || mime == "application/xml"
        {
            viewer.set(Some(FileOpenTarget::Text(file)));
        } else {
            // application/pdf and everything else: open in a new tab so the
            // browser handles native preview / download as appropriate.
            let url = api::authenticated_media_url(&format!("/files/{}/download", file.id));
            let _ = web_sys::window().and_then(|w| w.open_with_url(&url).ok());
        }
    }
}
