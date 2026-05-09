use std::collections::HashSet;

use dioxus::prelude::*;
use uncloud_common::{DuplicateReport, MirrorCluster, StraySet, SubsetPair};

use crate::components::icons::{IconCopy, IconRefreshCw, IconTrash};
use crate::hooks::use_duplicates;

#[component]
pub fn DuplicatesPage() -> Element {
    let mut report: Signal<Option<DuplicateReport>> = use_signal(|| None);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut refresh = use_signal(|| 0u32);

    use_effect(move || {
        let _ = refresh();
        spawn(async move {
            loading.set(true);
            error.set(None);
            match use_duplicates::get_duplicate_report().await {
                Ok(r) => report.set(Some(r)),
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    });

    let do_rescan = move |_| {
        refresh.set(refresh() + 1);
    };

    rsx! {
        div { class: "p-4 max-w-5xl mx-auto",
            div { class: "flex items-center justify-between mb-4 gap-2",
                h2 { class: "text-2xl font-bold flex items-center gap-2",
                    IconCopy { class: "w-6 h-6".to_string() }
                    "Duplicates"
                }
                button {
                    class: "btn btn-ghost btn-sm",
                    onclick: do_rescan,
                    disabled: loading(),
                    IconRefreshCw { class: "w-4 h-4".to_string() }
                    "Rescan"
                }
            }

            if let Some(err) = error() {
                div { class: "alert alert-error mb-4", "{err}" }
            }

            if loading() {
                div { class: "flex justify-center py-12",
                    span { class: "loading loading-spinner loading-lg" }
                }
            } else if let Some(r) = report() {
                ReportSummary { report: r.clone() }

                if r.mirror_clusters.is_empty() && r.subsets.is_empty() && r.stray_sets.is_empty() {
                    div { class: "card bg-base-100 shadow",
                        div { class: "card-body items-center text-center py-12",
                            IconCopy { class: "w-12 h-12 mb-4 text-base-content/30".to_string() }
                            p { class: "text-base-content/70", "No duplicates found." }
                        }
                    }
                }

                if !r.mirror_clusters.is_empty() {
                    h3 { class: "text-lg font-semibold mt-6 mb-3", "Mirror folders" }
                    div { class: "flex flex-col gap-3",
                        for cluster in r.mirror_clusters.iter() {
                            MirrorCard {
                                key: "{cluster.id}",
                                cluster: cluster.clone(),
                                on_resolved: move |_| { refresh.set(refresh() + 1); },
                            }
                        }
                    }
                }

                if !r.subsets.is_empty() {
                    h3 { class: "text-lg font-semibold mt-6 mb-3", "Subsets" }
                    div { class: "flex flex-col gap-3",
                        for pair in r.subsets.iter() {
                            SubsetCard {
                                key: "{pair.id}",
                                pair: pair.clone(),
                                on_resolved: move |_| { refresh.set(refresh() + 1); },
                            }
                        }
                    }
                }

                if !r.stray_sets.is_empty() {
                    h3 { class: "text-lg font-semibold mt-6 mb-3", "Stray duplicates" }
                    div { class: "flex flex-col gap-3",
                        for set in r.stray_sets.iter() {
                            StrayCard {
                                key: "{set.id}",
                                set: set.clone(),
                                on_resolved: move |_| { refresh.set(refresh() + 1); },
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn ReportSummary(report: DuplicateReport) -> Element {
    let mirror_count = report.mirror_clusters.len();
    let subset_count = report.subsets.len();
    let stray_count = report.stray_sets.len();
    let summary = if report.total_duplicate_files == 0 {
        "Nothing to clean up — no duplicates detected.".to_string()
    } else {
        format!(
            "Found {} duplicate file(s) across {} mirror folder(s), {} subset(s), and {} stray set(s) — {} recoverable.",
            report.total_duplicate_files,
            mirror_count,
            subset_count,
            stray_count,
            format_bytes(report.total_wasted_bytes)
        )
    };
    rsx! {
        div { class: "alert alert-info mb-4", "{summary}" }
    }
}

#[component]
fn MirrorCard(cluster: MirrorCluster, on_resolved: EventHandler<()>) -> Element {
    let mut keep_id = use_signal(|| cluster.suggested_keep_folder_id.clone());
    let mut deleting = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let folders = cluster.folders.clone();
    let bytes_label = format_bytes(cluster.total_bytes);
    let cluster_total_files: u32 = cluster.folders.iter().map(|f| f.file_count).sum();

    let do_delete = {
        let folders = folders.clone();
        move |_| {
            let folders = folders.clone();
            spawn(async move {
                deleting.set(true);
                error.set(None);
                let keep = keep_id();
                let to_delete: HashSet<String> = folders
                    .iter()
                    .filter(|f| f.id != keep)
                    .map(|f| f.id.clone())
                    .collect();

                match collect_files_in_folders(&to_delete).await {
                    Ok(file_ids) => {
                        let (_ok, errs) = use_duplicates::delete_files(file_ids).await;
                        if !errs.is_empty() {
                            error.set(Some(format!(
                                "{} deletions failed",
                                errs.len()
                            )));
                        }
                    }
                    Err(e) => error.set(Some(e)),
                }
                deleting.set(false);
                on_resolved.call(());
            });
        }
    };

    rsx! {
        div { class: "card bg-base-100 shadow",
            div { class: "card-body py-4",
                div { class: "flex justify-between items-start flex-wrap gap-2",
                    div {
                        h4 { class: "font-semibold",
                            "{cluster_total_files} files duplicated across {folders.len()} folders"
                        }
                        p { class: "text-sm text-base-content/70", "{bytes_label} recoverable" }
                    }
                }

                ul { class: "list-disc pl-6 mt-2 text-sm",
                    for f in folders.iter() {
                        li { key: "{f.id}",
                            span { class: "font-mono", "{f.path}" }
                            span { class: "text-base-content/60", " ({f.file_count} files)" }
                        }
                    }
                }

                div { class: "flex flex-wrap items-center gap-2 mt-3",
                    span { class: "text-sm", "Keep:" }
                    select {
                        class: "select select-bordered select-sm flex-1 min-w-0",
                        disabled: deleting(),
                        onchange: move |evt| keep_id.set(evt.value()),
                        for f in folders.iter() {
                            option {
                                key: "{f.id}",
                                value: "{f.id}",
                                selected: f.id == keep_id(),
                                "{f.path}"
                            }
                        }
                    }
                    button {
                        class: "btn btn-error btn-sm",
                        disabled: deleting(),
                        onclick: do_delete,
                        IconTrash { class: "w-4 h-4".to_string() }
                        if deleting() { "Deleting…" } else { "Delete the others" }
                    }
                }

                if let Some(err) = error() {
                    div { class: "alert alert-error mt-2 text-sm", "{err}" }
                }
            }
        }
    }
}

#[component]
fn SubsetCard(pair: SubsetPair, on_resolved: EventHandler<()>) -> Element {
    let mut deleting = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let bytes_label = format_bytes(pair.total_bytes);
    let subset_id = pair.subset.id.clone();
    let subset_path = pair.subset.path.clone();
    let superset_path = pair.superset.path.clone();
    let file_count = pair.file_count;

    let do_delete = move |_| {
        let subset_id = subset_id.clone();
        spawn(async move {
            deleting.set(true);
            error.set(None);
            let mut folders = HashSet::new();
            folders.insert(subset_id);
            match collect_files_in_folders(&folders).await {
                Ok(file_ids) => {
                    let (_ok, errs) = use_duplicates::delete_files(file_ids).await;
                    if !errs.is_empty() {
                        error.set(Some(format!(
                            "{} deletions failed",
                            errs.len()
                        )));
                    }
                }
                Err(e) => error.set(Some(e)),
            }
            deleting.set(false);
            on_resolved.call(());
        });
    };

    rsx! {
        div { class: "card bg-base-100 shadow",
            div { class: "card-body py-4",
                div { class: "flex justify-between items-start flex-wrap gap-2",
                    div {
                        h4 { class: "font-semibold",
                            span { class: "font-mono", "{subset_path}" }
                            " is already inside "
                            span { class: "font-mono", "{superset_path}" }
                        }
                        p { class: "text-sm text-base-content/70",
                            "{file_count} files · {bytes_label} recoverable"
                        }
                    }
                    button {
                        class: "btn btn-error btn-sm",
                        disabled: deleting(),
                        onclick: do_delete,
                        IconTrash { class: "w-4 h-4".to_string() }
                        if deleting() { "Deleting…" } else { "Delete subset" }
                    }
                }
                if let Some(err) = error() {
                    div { class: "alert alert-error mt-2 text-sm", "{err}" }
                }
            }
        }
    }
}

#[component]
fn StrayCard(set: StraySet, on_resolved: EventHandler<()>) -> Element {
    // Pre-select all but the first (oldest) file by default.
    let initial_selected: HashSet<String> = set
        .files
        .iter()
        .skip(1)
        .map(|f| f.id.clone())
        .collect();
    let mut selected: Signal<HashSet<String>> = use_signal(move || initial_selected.clone());
    let mut deleting = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let bytes_label = format_bytes(set.size_bytes);
    let total_files = set.files.len();
    let files = set.files.clone();
    let selected_count = selected().len();

    let do_delete = {
        let files = files.clone();
        move |_| {
            let _ = files.clone();
            spawn(async move {
                deleting.set(true);
                error.set(None);
                let ids: Vec<String> = selected().iter().cloned().collect();
                let (_ok, errs) = use_duplicates::delete_files(ids).await;
                if !errs.is_empty() {
                    error.set(Some(format!("{} deletions failed", errs.len())));
                }
                deleting.set(false);
                on_resolved.call(());
            });
        }
    };

    rsx! {
        div { class: "card bg-base-100 shadow",
            div { class: "card-body py-4",
                div { class: "flex justify-between items-start flex-wrap gap-2",
                    div {
                        h4 { class: "font-semibold", "{total_files} copies · {bytes_label} each" }
                        p { class: "text-xs text-base-content/50 font-mono break-all",
                            "{set.checksum}"
                        }
                    }
                    button {
                        class: "btn btn-error btn-sm",
                        disabled: deleting() || selected_count == 0,
                        onclick: do_delete,
                        IconTrash { class: "w-4 h-4".to_string() }
                        if deleting() {
                            "Deleting…"
                        } else {
                            "Delete selected ({selected_count})"
                        }
                    }
                }
                ul { class: "mt-2 text-sm",
                    for f in files.iter() {
                        {
                            let id = f.id.clone();
                            let id_change = id.clone();
                            rsx! {
                                li { key: "{f.id}", class: "flex items-center gap-2 py-1",
                                    input {
                                        r#type: "checkbox",
                                        class: "checkbox checkbox-sm",
                                        checked: selected().contains(&id),
                                        disabled: deleting(),
                                        onchange: move |evt| {
                                            let mut s = selected();
                                            if evt.value() == "true" {
                                                s.insert(id_change.clone());
                                            } else {
                                                s.remove(&id_change);
                                            }
                                            selected.set(s);
                                        },
                                    }
                                    span { class: "font-mono break-all", "{f.path}" }
                                }
                            }
                        }
                    }
                }
                if let Some(err) = error() {
                    div { class: "alert alert-error mt-2 text-sm", "{err}" }
                }
            }
        }
    }
}

/// Walk the user's folder tree (limited to one level — folders only) and
/// collect the IDs of every live file inside `folder_ids` and their
/// subtrees. Reuses the existing `/files?parent_id=` and `/folders?parent_id=`
/// listing endpoints.
async fn collect_files_in_folders(folder_ids: &HashSet<String>) -> Result<Vec<String>, String> {
    use crate::hooks::api;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct FileLite {
        id: String,
    }
    #[derive(Deserialize)]
    struct FolderLite {
        id: String,
    }

    let mut to_visit: Vec<String> = folder_ids.iter().cloned().collect();
    let mut all_file_ids = Vec::new();

    while let Some(fid) = to_visit.pop() {
        let files: Vec<FileLite> = api::get(&format!("/files?parent_id={}", fid))
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;
        for f in files {
            all_file_ids.push(f.id);
        }

        let subfolders: Vec<FolderLite> = api::get(&format!("/folders?parent_id={}", fid))
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;
        for sub in subfolders {
            to_visit.push(sub.id);
        }
    }

    Ok(all_file_ids)
}

fn format_bytes(b: i64) -> String {
    let b = b.max(0) as f64;
    if b < 1024.0 {
        return format!("{} B", b as i64);
    }
    let units = ["KB", "MB", "GB", "TB"];
    let mut value = b / 1024.0;
    let mut unit = units[0];
    for u in &units[1..] {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = u;
    }
    format!("{:.1} {}", value, unit)
}
