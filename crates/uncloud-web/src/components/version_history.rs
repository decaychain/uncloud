use dioxus::prelude::*;
use uncloud_common::FileVersionResponse;
use crate::hooks::use_files;

#[component]
pub fn VersionHistoryModal(
    file_id: String,
    file_name: String,
    on_close: EventHandler<()>,
    on_restored: EventHandler<()>,
) -> Element {
    let mut versions: Signal<Vec<FileVersionResponse>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut restoring: Signal<Option<String>> = use_signal(|| None);

    // Store file_id in a signal so closures can access it without move conflicts.
    let file_id_sig = use_signal(|| file_id.clone());

    use_effect(move || {
        let fid = file_id_sig().clone();
        spawn(async move {
            loading.set(true);
            match use_files::list_versions(&fid).await {
                Ok(v) => {
                    versions.set(v);
                    error.set(None);
                }
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    });

    rsx! {
        div { class: "modal modal-open",
            div { class: "modal-box max-w-lg",
                h3 { class: "font-bold text-lg mb-1", "Version History" }
                p { class: "text-sm text-base-content/60 mb-4 truncate", "{file_name}" }

                if let Some(err) = error() {
                    div { class: "alert alert-error mb-4 text-sm", "{err}" }
                }

                if loading() {
                    div { class: "flex justify-center py-8",
                        span { class: "loading loading-spinner loading-lg" }
                    }
                } else if versions().is_empty() {
                    div { class: "text-center py-8 text-base-content/60",
                        p { "No previous versions" }
                    }
                } else {
                    div { class: "overflow-y-auto max-h-80",
                        table { class: "table table-sm w-full",
                            thead {
                                tr {
                                    th { "Version" }
                                    th { "Size" }
                                    th { "Date" }
                                    th { class: "text-right", "Actions" }
                                }
                            }
                            tbody {
                                for ver in versions() {
                                    {
                                        let vid_restore = ver.id.clone();
                                        let vid_download = ver.id.clone();
                                        let fid_restore = file_id_sig().clone();
                                        let fid_download = file_id_sig().clone();
                                        let size_str = uncloud_common::validation::format_bytes(ver.size_bytes);
                                        let date_str = format_version_date(&ver.created_at);
                                        let is_restoring = restoring() == Some(ver.id.clone());
                                        rsx! {
                                            tr {
                                                td { "v{ver.version}" }
                                                td { "{size_str}" }
                                                td { class: "text-sm opacity-70", "{date_str}" }
                                                td { class: "text-right",
                                                    button {
                                                        class: "btn btn-ghost btn-xs mr-1",
                                                        title: "Download this version",
                                                        onclick: move |_| {
                                                            let url = crate::hooks::api::authenticated_media_url(&format!("/files/{}/versions/{}", fid_download, vid_download));
                                                            let _ = web_sys::window().and_then(|w| w.open_with_url(&url).ok());
                                                        },
                                                        "Download"
                                                    }
                                                    button {
                                                        class: "btn btn-ghost btn-xs btn-primary",
                                                        title: "Restore this version",
                                                        disabled: is_restoring,
                                                        onclick: move |_| {
                                                            let fid = fid_restore.clone();
                                                            let vid = vid_restore.clone();
                                                            restoring.set(Some(vid.clone()));
                                                            spawn(async move {
                                                                match use_files::restore_version(&fid, &vid).await {
                                                                    Ok(()) => {
                                                                        on_restored.call(());
                                                                    }
                                                                    Err(e) => {
                                                                        restoring.set(None);
                                                                        error.set(Some(e));
                                                                    }
                                                                }
                                                            });
                                                        },
                                                        if is_restoring {
                                                            span { class: "loading loading-spinner loading-sm" }
                                                        }
                                                        "Restore"
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

                div { class: "modal-action",
                    button {
                        class: "btn",
                        onclick: move |_| on_close.call(()),
                        "Close"
                    }
                }
            }
            div { class: "modal-backdrop", onclick: move |_| on_close.call(()) }
        }
    }
}

fn format_version_date(rfc3339: &str) -> String {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(rfc3339) {
        dt.format("%Y-%m-%d %H:%M").to_string()
    } else {
        rfc3339.to_string()
    }
}
