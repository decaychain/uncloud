use std::collections::HashMap;

use chrono::Duration;
use dioxus::prelude::*;
use gloo_timers::future::TimeoutFuture;
use keepass::db::{Database, Entry, Group};
use keepass::DatabaseKey;
use uncloud_common::{FileResponse, FolderResponse};
use crate::components::icons::{IconChevronRight, IconFingerprint, IconFolder, IconLock, IconLockOpen, IconX};
use crate::hooks::{api, biometric, use_files};
use crate::state::{AuthState, VaultSession, VaultState};

/// Idle TTL for the unlocked vault. Past this, the next mount of
/// PasswordsPage (or a 30 s ticker while the page is open) clears the
/// session and forces a fresh biometric prompt or master-password entry.
const VAULT_IDLE_TTL_SECS: i64 = 5 * 60;

// ── Vault state ────────────────────────────────────────────────────────────

/// A flattened view of an entry for display.
#[derive(Clone)]
struct EntryView {
    uuid: uuid::Uuid,
    title: String,
    username: String,
    url: String,
    group_path: String,
}

/// A flattened view of a group for the sidebar.
#[derive(Clone)]
struct GroupView {
    uuid: uuid::Uuid,
    name: String,
    depth: usize,
    entry_count: usize,
}

fn collect_entries(group: &Group, path: &str) -> Vec<EntryView> {
    let mut result = Vec::new();
    let current_path = if path.is_empty() {
        group.name.clone()
    } else {
        format!("{}/{}", path, group.name)
    };

    for entry in &group.entries {
        result.push(EntryView {
            uuid: entry.uuid,
            title: entry.get_title().unwrap_or("Untitled").to_string(),
            username: entry.get_username().unwrap_or("").to_string(),
            url: entry.get_url().unwrap_or("").to_string(),
            group_path: current_path.clone(),
        });
    }

    for child in &group.groups {
        result.extend(collect_entries(child, &current_path));
    }
    result
}

fn collect_groups(group: &Group, depth: usize) -> Vec<GroupView> {
    let mut result = vec![GroupView {
        uuid: group.uuid,
        name: group.name.clone(),
        depth,
        entry_count: count_entries_recursive(group),
    }];
    for child in &group.groups {
        result.extend(collect_groups(child, depth + 1));
    }
    result
}

fn count_entries_recursive(group: &Group) -> usize {
    group.entries.len() + group.groups.iter().map(|g| count_entries_recursive(g)).sum::<usize>()
}

fn find_entry<'a>(group: &'a Group, uuid: uuid::Uuid) -> Option<&'a Entry> {
    group.entry_by_uuid(uuid)
}

fn find_entry_mut<'a>(group: &'a mut Group, uuid: uuid::Uuid) -> Option<&'a mut Entry> {
    group.entry_by_uuid_mut(uuid)
}

fn find_group_entries(group: &Group, group_uuid: uuid::Uuid) -> Vec<EntryView> {
    if group.uuid == group_uuid {
        return group.entries.iter().map(|e| EntryView {
            uuid: e.uuid,
            title: e.get_title().unwrap_or("Untitled").to_string(),
            username: e.get_username().unwrap_or("").to_string(),
            url: e.get_url().unwrap_or("").to_string(),
            group_path: group.name.clone(),
        }).collect();
    }
    for child in &group.groups {
        let r = find_group_entries(child, group_uuid);
        if !r.is_empty() {
            return r;
        }
    }
    Vec::new()
}

// ── Biometric enrolment prompt state ──────────────────────────────────────

/// Set when a manual unlock succeeds and the user is a candidate for
/// biometric enrolment. PasswordsPage shows `BiometricEnrolPrompt` when
/// this is `Some`.
#[derive(Clone)]
struct EnrolPromptState {
    user_id: String,
    vault_id: String,
    vault_name: String,
    master_password: String,
}

/// localStorage key for the per-vault "user said no" flag. Format
/// `uncloud:bio-skip:{user_id}.{vault_id}` so it never collides across
/// accounts on the same device.
fn skip_flag_key(user_id: &str, vault_id: &str) -> String {
    format!("uncloud:bio-skip:{user_id}.{vault_id}")
}

fn skip_flag_get(user_id: &str, vault_id: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item(&skip_flag_key(user_id, vault_id)).ok().flatten())
        .is_some()
}

fn skip_flag_set(user_id: &str, vault_id: &str) {
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = s.set_item(&skip_flag_key(user_id, vault_id), "1");
    }
}

fn skip_flag_clear(user_id: &str, vault_id: &str) {
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = s.remove_item(&skip_flag_key(user_id, vault_id));
    }
}

// ── Vault recents API ─────────────────────────────────────────────────────

async fn fetch_recent_vaults() -> Result<Vec<uncloud_common::RecentVaultEntry>, String> {
    let resp = api::get("/vault-recents")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.ok() {
        return Err(format!("HTTP {}", resp.status()));
    }
    resp.json::<Vec<uncloud_common::RecentVaultEntry>>()
        .await
        .map_err(|e| e.to_string())
}

async fn add_recent_vault_api(file_id: &str, file_name: &str, folder_path: Option<&str>) -> Result<(), String> {
    let req = uncloud_common::AddRecentVaultRequest {
        file_id: file_id.to_string(),
        file_name: file_name.to_string(),
        folder_path: folder_path.map(|s| s.to_string()),
    };
    let resp = api::post("/vault-recents")
        .json(&req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.ok() {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}

async fn remove_recent_vault_api(file_id: &str) -> Result<(), String> {
    let resp = api::delete(&format!("/vault-recents/{}", file_id))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.ok() {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}

// ── Kdbx file loading ─────────────────────────────────────────────────────

#[derive(Clone)]
struct KdbxFile {
    id: String,
    name: String,
    folder_path: Option<String>,
}

async fn download_file_bytes(file_id: &str) -> Result<Vec<u8>, String> {
    let url = api::api_url(&format!("/files/{}/download", file_id));
    let resp = api::get_raw(&url)
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;

    if !resp.ok() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let array_buffer = resp.binary().await
        .map_err(|e| format!("Read error: {:?}", e))?;

    Ok(array_buffer)
}

async fn save_vault_bytes(file_id: &str, data: Vec<u8>, file_name: &str) -> Result<(), String> {
    let blob_parts = js_sys::Array::new();
    let uint8 = js_sys::Uint8Array::from(data.as_slice());
    blob_parts.push(&uint8);
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type("application/octet-stream");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&blob_parts, &opts)
        .map_err(|_| "Failed to create Blob".to_string())?;

    let form = web_sys::FormData::new()
        .map_err(|_| "Failed to create FormData".to_string())?;
    form.append_with_blob_and_filename("file", &blob, file_name)
        .map_err(|_| "Failed to append to FormData".to_string())?;

    let url = api::api_url(&format!("/files/{}/content", file_id));
    let resp = api::post_raw(&url)
        .body(form)
        .map_err(|e| format!("Request error: {:?}", e))?
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;

    if resp.ok() {
        Ok(())
    } else {
        let body = resp.text().await.unwrap_or_default();
        Err(format!("Save failed (HTTP {}): {}", resp.status(), body))
    }
}

async fn create_new_vault_file(name: &str, data: Vec<u8>, parent_id: Option<&str>) -> Result<String, String> {
    let blob_parts = js_sys::Array::new();
    let uint8 = js_sys::Uint8Array::from(data.as_slice());
    blob_parts.push(&uint8);
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type("application/octet-stream");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&blob_parts, &opts)
        .map_err(|_| "Failed to create Blob".to_string())?;

    let form = web_sys::FormData::new()
        .map_err(|_| "Failed to create FormData".to_string())?;
    form.append_with_blob_and_filename("file", &blob, name)
        .map_err(|_| "Failed to append to FormData".to_string())?;
    if let Some(pid) = parent_id {
        form.append_with_str("parent_id", pid)
            .map_err(|_| "Failed to append parent_id".to_string())?;
    }

    let url = api::api_url("/uploads/simple");
    let resp = api::post_raw(&url)
        .body(form)
        .map_err(|e| format!("Request error: {:?}", e))?
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;

    if !resp.ok() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Upload failed (HTTP {}): {}", resp.status(), body));
    }

    let text = resp.text().await.map_err(|e| format!("Read error: {}", e))?;
    let file: uncloud_common::FileResponse = serde_json::from_str(&text)
        .map_err(|_| "Failed to parse upload response".to_string())?;
    Ok(file.id)
}

// ── Password generator ─────────────────────────────────────────────────────

fn generate_password(length: usize, uppercase: bool, lowercase: bool, digits: bool, symbols: bool) -> String {
    let mut chars = String::new();
    if uppercase { chars.push_str("ABCDEFGHIJKLMNOPQRSTUVWXYZ"); }
    if lowercase { chars.push_str("abcdefghijklmnopqrstuvwxyz"); }
    if digits { chars.push_str("0123456789"); }
    if symbols { chars.push_str("!@#$%^&*()-_=+[]{}|;:,.<>?/~`"); }
    if chars.is_empty() {
        chars = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789".to_string();
    }

    let char_bytes = chars.as_bytes();
    let mut result = Vec::with_capacity(length);

    // Use crypto.getRandomValues for secure randomness
    let mut random_bytes = vec![0u8; length];
    if let Some(crypto) = web_sys::window().and_then(|w| w.crypto().ok()) {
        let _ = crypto.get_random_values_with_u8_array(&mut random_bytes);
    }

    for i in 0..length {
        let idx = random_bytes.get(i).copied().unwrap_or(0) as usize % char_bytes.len();
        result.push(char_bytes[idx]);
    }

    String::from_utf8(result).unwrap_or_default()
}

// ── Top-level page ─────────────────────────────────────────────────────────

#[component]
pub fn PasswordsPage() -> Element {
    // App-level vault session — survives route navigation. Children still
    // see plain `Signal<Option<VaultState>>` and `Signal<String>` props,
    // hydrated from the session on mount and synced back via use_effect.
    let mut session = use_context::<Signal<VaultSession>>();

    // Synchronously expire stale sessions before any child sees them.
    // This handles "navigate to /passwords after >5 min away" — the
    // initial reads below pick up the cleared session, not the stale one.
    if session.peek().is_stale(Duration::seconds(VAULT_IDLE_TTL_SECS)) {
        session.write().clear();
    }

    let mut vault: Signal<Option<VaultState>> =
        use_signal(|| session.peek().state.clone());
    let mut master_password: Signal<String> =
        use_signal(|| session.peek().master_password.clone());
    let mut recent_vaults: Signal<Vec<uncloud_common::RecentVaultEntry>> = use_signal(Vec::new);
    let mut loading: Signal<bool> = use_signal(|| true);

    // Mirror local signals back into the session and stamp activity. The
    // session always reflects the latest decrypted state so route nav-back
    // can rehydrate without the user re-typing the master password.
    use_effect(move || {
        let v = vault();
        let mp = master_password();
        let mut s = session.write();
        s.state = v;
        s.master_password = mp;
        s.bump();
    });

    // Idle auto-lock: while PasswordsPage is mounted, check every 30 s.
    // The mount-time staleness check above handles the navigated-away case.
    use_future(move || async move {
        loop {
            TimeoutFuture::new(30_000).await;
            if session.peek().is_stale(Duration::seconds(VAULT_IDLE_TTL_SECS)) {
                session.write().clear();
                vault.set(None);
                master_password.set(String::new());
            }
        }
    });

    // Open from folder browser
    let mut show_file_picker: Signal<bool> = use_signal(|| false);
    // New vault dialog
    let mut show_new_vault: Signal<bool> = use_signal(|| false);

    // Biometric (Android-only): post-unlock prompt + per-recent enrolment cache.
    // `enrolled_map` is keyed by file_id; populated by a use_effect when
    // recent_vaults changes. Empty map / always-false on non-Android.
    let pending_enrol: Signal<Option<EnrolPromptState>> = use_signal(|| None);
    let mut enrolled_map: Signal<HashMap<String, bool>> = use_signal(HashMap::new);
    let auth_state = use_context::<Signal<AuthState>>();

    // Check if we were navigated here with a specific vault file to open
    let mut vault_open_target = use_context::<Signal<crate::state::VaultOpenTarget>>();

    // Fetch LRU list on mount; if navigated here via file browser, show that file too
    use_effect(move || {
        spawn(async move {
            loading.set(true);
            let recents = fetch_recent_vaults().await.unwrap_or_default();
            recent_vaults.set(recents);
            loading.set(false);

            // If navigated here via "Open" on a .kdbx file, add it to recents
            let target = vault_open_target();
            if let (Some(fid), Some(fname)) = (target.file_id, target.file_name) {
                let _ = add_recent_vault_api(&fid, &fname, None).await;
                // Refresh the list
                if let Ok(recents) = fetch_recent_vaults().await {
                    recent_vaults.set(recents);
                }
                vault_open_target.set(crate::state::VaultOpenTarget::default());
            }
        });
    });

    // Whenever the recents list or the signed-in user changes, refresh the
    // per-vault biometric-enrolment cache. Cheap on non-Android (every call
    // short-circuits to false). Used by the recents row to decide whether
    // to show the "Forget biometric" button.
    use_effect(move || {
        let recents = recent_vaults();
        let user_id = auth_state().user.as_ref().map(|u| u.id.clone());
        spawn(async move {
            let Some(uid) = user_id else { return };
            let mut next = HashMap::with_capacity(recents.len());
            for entry in &recents {
                let enrolled = biometric::is_enrolled(&uid, &entry.file_id).await;
                next.insert(entry.file_id.clone(), enrolled);
            }
            enrolled_map.set(next);
        });
    });

    // If vault is unlocked, show the vault UI. The biometric enrol prompt
    // overlays it so the user can react to the freshly-unlocked vault
    // ("Enable biometric unlock?") without being kicked back to the picker.
    if vault().is_some() {
        return rsx! {
            VaultBrowser {
                vault,
                master_password: master_password(),
            }
            if pending_enrol().is_some() {
                BiometricEnrolPrompt {
                    state: pending_enrol,
                    enrolled_map,
                }
            }
        };
    }

    // Otherwise show the unlock / pick screen
    rsx! {
        div { class: "max-w-lg mx-auto mt-8",
            div { class: "card bg-base-100 shadow-xl",
                div { class: "card-body",
                    h2 { class: "card-title text-2xl mb-2", "Password Vault" }

                    if loading() {
                        div { class: "flex items-center justify-center py-8",
                            span { class: "loading loading-spinner loading-lg" }
                        }
                    } else {
                        if !recent_vaults().is_empty() {
                            p { class: "text-base-content/70 mb-4",
                                "Recent vaults:"
                            }
                            for entry in recent_vaults().iter() {
                                {
                                    let file_id = entry.file_id.clone();
                                    let file_name = entry.file_name.clone();
                                    let folder_path = entry.folder_path.clone();
                                    let file_id_rm = file_id.clone();
                                    let file_id_bio = file_id.clone();
                                    let user_id_bio = auth_state()
                                        .user
                                        .as_ref()
                                        .map(|u| u.id.clone())
                                        .unwrap_or_default();
                                    let user_id_unlock = user_id_bio.clone();
                                    let bio_enrolled = enrolled_map()
                                        .get(&file_id)
                                        .copied()
                                        .unwrap_or(false);
                                    rsx! {
                                        div { class: "relative",
                                            VaultUnlockCard {
                                                file_id,
                                                file_name: file_name.clone(),
                                                folder_path,
                                                user_id: user_id_unlock,
                                                vault,
                                                master_password,
                                                recent_vaults,
                                                pending_enrol,
                                            }
                                            div { class: "absolute top-2 right-2 flex gap-1",
                                                if bio_enrolled {
                                                    button {
                                                        class: "btn btn-ghost btn-xs text-primary hover:text-error",
                                                        title: "Forget biometric for this vault",
                                                        onclick: move |_| {
                                                            let uid = user_id_bio.clone();
                                                            let fid = file_id_bio.clone();
                                                            spawn(async move {
                                                                let _ = biometric::clear(&uid, &fid).await;
                                                                skip_flag_clear(&uid, &fid);
                                                                let mut next = enrolled_map();
                                                                next.insert(fid, false);
                                                                enrolled_map.set(next);
                                                            });
                                                        },
                                                        IconFingerprint { class: "w-3 h-3".to_string() }
                                                    }
                                                }
                                                button {
                                                    class: "btn btn-ghost btn-xs text-base-content/40 hover:text-error",
                                                    title: "Remove from recent list",
                                                    onclick: move |_| {
                                                        let fid = file_id_rm.clone();
                                                        spawn(async move {
                                                            let _ = remove_recent_vault_api(&fid).await;
                                                            if let Ok(recents) = fetch_recent_vaults().await {
                                                                recent_vaults.set(recents);
                                                            }
                                                        });
                                                    },
                                                    IconX { class: "w-3 h-3".to_string() }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            div { class: "divider", "OR" }
                        } else {
                            p { class: "text-base-content/70 mb-4",
                                "No recent vaults. Open a .kdbx file from the file browser or create a new vault."
                            }
                        }

                        div { class: "flex flex-col gap-2",
                            button {
                                class: "btn btn-outline w-full",
                                onclick: move |_| show_file_picker.set(true),
                                "Open Vault from Folder..."
                            }
                            button {
                                class: "btn btn-primary w-full",
                                onclick: move |_| show_new_vault.set(true),
                                "Create New Vault"
                            }
                        }
                    }
                }
            }

            // Vault file picker modal
            if show_file_picker() {
                VaultFilePicker {
                    on_select: move |file: KdbxFile| {
                        show_file_picker.set(false);
                        let fid = file.id.clone();
                        let fname = file.name.clone();
                        let fpath = file.folder_path.clone();
                        spawn(async move {
                            let _ = add_recent_vault_api(&fid, &fname, fpath.as_deref()).await;
                            if let Ok(recents) = fetch_recent_vaults().await {
                                recent_vaults.set(recents);
                            }
                        });
                    },
                    on_close: move |_| show_file_picker.set(false),
                }
            }

            // New vault modal
            if show_new_vault() {
                NewVaultModal {
                    vault,
                    master_password,
                    on_close: move |_| show_new_vault.set(false),
                }
            }
        }
    }
}

// ── Vault file picker (browse folders for .kdbx files) ─────────────────────

#[component]
fn VaultFilePicker(
    on_select: EventHandler<KdbxFile>,
    on_close: EventHandler<()>,
) -> Element {
    let mut current_parent: Signal<Option<String>> = use_signal(|| None);
    let mut folders: Signal<Vec<FolderResponse>> = use_signal(Vec::new);
    let mut kdbx_files: Signal<Vec<FileResponse>> = use_signal(Vec::new);
    let mut breadcrumb: Signal<Vec<FolderResponse>> = use_signal(Vec::new);
    let mut loading = use_signal(|| false);

    // Load contents whenever current_parent changes
    use_effect(move || {
        let parent = current_parent();
        spawn(async move {
            loading.set(true);
            if let Ok(flds) = use_files::list_folders(parent.as_deref()).await {
                folders.set(flds);
            }
            if let Ok(files) = use_files::list_files(parent.as_deref()).await {
                let kdbx: Vec<FileResponse> = files.into_iter()
                    .filter(|f| f.name.ends_with(".kdbx"))
                    .collect();
                kdbx_files.set(kdbx);
            }
            match &parent {
                Some(pid) => {
                    if let Ok(crumbs) = use_files::get_breadcrumb(pid).await {
                        breadcrumb.set(crumbs);
                    }
                }
                None => breadcrumb.set(Vec::new()),
            }
            loading.set(false);
        });
    });

    rsx! {
        div { class: "modal modal-open",
            div { class: "modal-box max-w-md",
                h3 { class: "font-bold text-lg mb-3", "Open Vault File" }

                // Breadcrumb
                div { class: "text-sm breadcrumbs px-0 mb-1",
                    ul {
                        li {
                            a {
                                class: "cursor-pointer",
                                onclick: move |_| current_parent.set(None),
                                "Files"
                            }
                        }
                        for folder in breadcrumb() {
                            {
                                let id = folder.id.clone();
                                rsx! {
                                    li {
                                        a {
                                            class: "cursor-pointer",
                                            onclick: move |_| current_parent.set(Some(id.clone())),
                                            "{folder.name}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                div { class: "min-h-40 border border-base-300 rounded-box overflow-y-auto max-h-72",
                    if loading() {
                        div { class: "flex justify-center items-center h-28",
                            span { class: "loading loading-spinner loading-md" }
                        }
                    } else if folders().is_empty() && kdbx_files().is_empty() {
                        div { class: "flex justify-center items-center h-28 text-base-content/40 text-sm",
                            "No folders or .kdbx files here"
                        }
                    } else {
                        ul { class: "menu menu-sm p-1",
                            // Folders
                            for folder in folders() {
                                {
                                    let id = folder.id.clone();
                                    rsx! {
                                        li {
                                            a {
                                                onclick: move |_| current_parent.set(Some(id.clone())),
                                                IconFolder { class: "w-4 h-4".to_string() }
                                                span { "{folder.name}" }
                                                IconChevronRight { class: "w-4 h-4 ml-auto opacity-40".to_string() }
                                            }
                                        }
                                    }
                                }
                            }
                            // .kdbx files
                            for file in kdbx_files() {
                                {
                                    let fid = file.id.clone();
                                    let fname = file.name.clone();
                                    let fpath = breadcrumb().iter().map(|f| f.name.clone()).collect::<Vec<_>>().join("/");
                                    let folder_path = if fpath.is_empty() { None } else { Some(fpath) };
                                    rsx! {
                                        li {
                                            a {
                                                onclick: move |_| {
                                                    on_select.call(KdbxFile {
                                                        id: fid.clone(),
                                                        name: fname.clone(),
                                                        folder_path: folder_path.clone(),
                                                    });
                                                },
                                                IconLock { class: "w-4 h-4".to_string() }
                                                span { class: "font-medium", "{file.name}" }
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
                        "Cancel"
                    }
                }
            }
            div { class: "modal-backdrop", onclick: move |_| on_close.call(()) }
        }
    }
}

// ── New vault modal (with folder picker) ───────────────────────────────────

#[component]
fn NewVaultModal(
    vault: Signal<Option<VaultState>>,
    master_password: Signal<String>,
    on_close: EventHandler<()>,
) -> Element {
    let mut name = use_signal(|| "passwords.kdbx".to_string());
    let mut password = use_signal(String::new);
    let mut confirm = use_signal(String::new);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    // Folder picker state
    let mut selected_folder: Signal<Option<String>> = use_signal(|| None);
    let mut picker_parent: Signal<Option<String>> = use_signal(|| None);
    let mut picker_folders: Signal<Vec<FolderResponse>> = use_signal(Vec::new);
    let mut picker_breadcrumb: Signal<Vec<FolderResponse>> = use_signal(Vec::new);
    let mut picker_loading = use_signal(|| false);
    let mut show_folder_picker = use_signal(|| false);

    // Load folders for the picker
    use_effect(move || {
        if !show_folder_picker() { return; }
        let parent = picker_parent();
        spawn(async move {
            picker_loading.set(true);
            if let Ok(flds) = use_files::list_folders(parent.as_deref()).await {
                picker_folders.set(flds);
            }
            match &parent {
                Some(pid) => {
                    if let Ok(crumbs) = use_files::get_breadcrumb(pid).await {
                        picker_breadcrumb.set(crumbs);
                    }
                }
                None => picker_breadcrumb.set(Vec::new()),
            }
            picker_loading.set(false);
        });
    });

    let folder_label = if selected_folder().is_some() {
        // Show last breadcrumb name as hint
        picker_breadcrumb().last().map(|f| f.name.clone()).unwrap_or("(selected folder)".to_string())
    } else {
        "Root (Files)".to_string()
    };

    rsx! {
        div { class: "modal modal-open",
            div { class: "modal-box",
                h3 { class: "font-bold text-lg mb-4", "Create New Vault" }

                if let Some(err) = error() {
                    div { class: "alert alert-error mb-3 text-sm", "{err}" }
                }

                div { class: "form-control mb-3",
                    label { class: "label", span { class: "label-text", "File name" } }
                    input {
                        class: "input input-bordered w-full",
                        r#type: "text",
                        value: "{name}",
                        oninput: move |e| name.set(e.value()),
                    }
                }

                div { class: "form-control mb-3",
                    label { class: "label", span { class: "label-text", "Location" } }
                    div { class: "flex items-center gap-2",
                        span { class: "text-sm flex-1 truncate flex items-center gap-1",
                            IconFolder { class: "w-4 h-4 inline-block".to_string() }
                            "{folder_label}"
                        }
                        button {
                            class: "btn btn-outline btn-sm",
                            onclick: move |_| {
                                picker_parent.set(selected_folder());
                                show_folder_picker.set(true);
                            },
                            "Browse..."
                        }
                    }
                }

                // Inline folder picker
                if show_folder_picker() {
                    div { class: "border border-base-300 rounded-box mb-3",
                        // Breadcrumb
                        div { class: "text-xs breadcrumbs px-2 pt-1",
                            ul {
                                li {
                                    a {
                                        class: "cursor-pointer",
                                        onclick: move |_| picker_parent.set(None),
                                        "Files"
                                    }
                                }
                                for folder in picker_breadcrumb() {
                                    {
                                        let id = folder.id.clone();
                                        rsx! {
                                            li {
                                                a {
                                                    class: "cursor-pointer",
                                                    onclick: move |_| picker_parent.set(Some(id.clone())),
                                                    "{folder.name}"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        div { class: "overflow-y-auto max-h-40",
                            if picker_loading() {
                                div { class: "flex justify-center items-center h-16",
                                    span { class: "loading loading-spinner loading-sm" }
                                }
                            } else if picker_folders().is_empty() {
                                div { class: "flex justify-center items-center h-16 text-base-content/40 text-xs",
                                    "No subfolders"
                                }
                            } else {
                                ul { class: "menu menu-xs p-1",
                                    for folder in picker_folders() {
                                        {
                                            let id = folder.id.clone();
                                            rsx! {
                                                li {
                                                    a {
                                                        onclick: move |_| picker_parent.set(Some(id.clone())),
                                                        IconFolder { class: "w-4 h-4".to_string() }
                                                        span { "{folder.name}" }
                                                        IconChevronRight { class: "w-4 h-4 ml-auto opacity-40".to_string() }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        div { class: "flex justify-end gap-2 p-2 border-t border-base-300",
                            button {
                                class: "btn btn-ghost btn-xs",
                                onclick: move |_| show_folder_picker.set(false),
                                "Cancel"
                            }
                            button {
                                class: "btn btn-primary btn-xs",
                                onclick: move |_| {
                                    selected_folder.set(picker_parent());
                                    show_folder_picker.set(false);
                                },
                                "Select This Folder"
                            }
                        }
                    }
                }

                div { class: "form-control mb-3",
                    label { class: "label", span { class: "label-text", "Master password" } }
                    input {
                        class: "input input-bordered w-full",
                        r#type: "password",
                        placeholder: "Enter master password",
                        value: "{password}",
                        oninput: move |e| password.set(e.value()),
                    }
                }
                div { class: "form-control mb-3",
                    label { class: "label", span { class: "label-text", "Confirm password" } }
                    input {
                        class: "input input-bordered w-full",
                        r#type: "password",
                        placeholder: "Confirm master password",
                        value: "{confirm}",
                        oninput: move |e| confirm.set(e.value()),
                    }
                }

                div { class: "modal-action",
                    button {
                        class: "btn",
                        onclick: move |_| on_close.call(()),
                        "Cancel"
                    }
                    button {
                        class: "btn btn-primary",
                        disabled: password().is_empty() || password() != confirm(),
                        onclick: move |_| {
                            let fname = name().trim().to_string();
                            let pw = password();
                            if pw != confirm() {
                                error.set(Some("Passwords don't match".to_string()));
                                return;
                            }
                            if fname.is_empty() {
                                error.set(Some("File name is required".to_string()));
                                return;
                            }
                            let fname = if !fname.ends_with(".kdbx") {
                                format!("{}.kdbx", fname)
                            } else {
                                fname
                            };

                            let db = Database::new(Default::default());
                            let mut buffer = Vec::new();
                            let key = DatabaseKey::new().with_password(&pw);
                            match db.save(&mut buffer, key) {
                                Ok(()) => {},
                                Err(e) => {
                                    error.set(Some(format!("Failed to create vault: {:?}", e)));
                                    return;
                                }
                            }

                            let parent_id = selected_folder();
                            let fname_c = fname.clone();
                            spawn(async move {
                                match create_new_vault_file(&fname_c, buffer, parent_id.as_deref()).await {
                                    Ok(file_id) => {
                                        on_close.call(());
                                        master_password.set(pw);
                                        vault.set(Some(VaultState {
                                            db,
                                            file_id: Some(file_id),
                                            file_name: fname_c,
                                            dirty: false,
                                        }));
                                    }
                                    Err(e) => {
                                        error.set(Some(e));
                                    }
                                }
                            });
                        },
                        "Create"
                    }
                }
            }
            div { class: "modal-backdrop", onclick: move |_| on_close.call(()) }
        }
    }
}

// ── Unlock card for a single vault file ────────────────────────────────────

#[component]
fn VaultUnlockCard(
    file_id: String,
    file_name: String,
    folder_path: Option<String>,
    user_id: String,
    vault: Signal<Option<VaultState>>,
    master_password: Signal<String>,
    recent_vaults: Signal<Vec<uncloud_common::RecentVaultEntry>>,
    pending_enrol: Signal<Option<EnrolPromptState>>,
) -> Element {
    let mut password = use_signal(String::new);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut unlocking = use_signal(|| false);
    // Set true after the on-mount biometric attempt fires once, so navigation
    // back to the unlock screen (e.g. user cancelled the prompt) doesn't
    // re-trigger it on every render — they can tap the button to retry.
    let mut auto_attempted: Signal<bool> = use_signal(|| false);

    // On mount, if biometric is enrolled for this vault, auto-fire the
    // prompt. The card stays mounted with the password field visible so
    // cancel / KeyPermanentlyInvalidatedException falls back gracefully.
    {
        let file_id_eff = file_id.clone();
        let file_name_eff = file_name.clone();
        let user_id_eff = user_id.clone();
        use_effect(move || {
            if auto_attempted() {
                return;
            }
            auto_attempted.set(true);
            let file_id = file_id_eff.clone();
            let file_name = file_name_eff.clone();
            let user_id = user_id_eff.clone();
            spawn(async move {
                if !biometric::is_enrolled(&user_id, &file_id).await {
                    return;
                }
                unlocking.set(true);
                error.set(None);
                match biometric::unlock(&user_id, &file_id).await {
                    Ok(secret) => {
                        finish_unlock(
                            file_id,
                            file_name,
                            secret,
                            // The biometric path skips the post-success
                            // enrol prompt — they're already enrolled.
                            None,
                            user_id.clone(),
                            error,
                            unlocking,
                            master_password,
                            vault,
                            recent_vaults,
                        )
                        .await;
                    }
                    Err(e) => {
                        unlocking.set(false);
                        if e == "biometric_invalidated" {
                            error.set(Some(
                                "Biometric data changed — please enter your master password to re-enrol.".into(),
                            ));
                        }
                        // Other errors (user cancel, hw unavailable) — silently
                        // fall back to manual password entry. The card remains
                        // visible so the user just types the password.
                    }
                }
            });
        });
    }

    // Manual unlock from the password field. After success, optionally
    // promotes the vault to the biometric enrol prompt.
    fn do_unlock(
        file_id: String,
        file_name: String,
        user_id: String,
        password: Signal<String>,
        mut error: Signal<Option<String>>,
        mut unlocking: Signal<bool>,
        master_password: Signal<String>,
        vault: Signal<Option<VaultState>>,
        recent_vaults: Signal<Vec<uncloud_common::RecentVaultEntry>>,
        pending_enrol: Signal<Option<EnrolPromptState>>,
    ) {
        let pw = password();
        if pw.is_empty() {
            error.set(Some("Password is required".to_string()));
            return;
        }
        unlocking.set(true);
        error.set(None);

        spawn(async move {
            finish_unlock(
                file_id,
                file_name,
                pw,
                Some(pending_enrol),
                user_id,
                error,
                unlocking,
                master_password,
                vault,
                recent_vaults,
            )
            .await;
        });
    }

    // Shared post-secret flow: download bytes, parse with the secret, set
    // up VaultState. `enrol_target` is `Some` for the manual path so a
    // candidate vault can prompt for biometric enrolment after unlock; the
    // biometric-auto path passes `None` since the user is already enrolled.
    #[allow(clippy::too_many_arguments)]
    async fn finish_unlock(
        file_id: String,
        file_name: String,
        secret: String,
        enrol_target: Option<Signal<Option<EnrolPromptState>>>,
        user_id: String,
        mut error: Signal<Option<String>>,
        mut unlocking: Signal<bool>,
        mut master_password: Signal<String>,
        mut vault: Signal<Option<VaultState>>,
        mut recent_vaults: Signal<Vec<uncloud_common::RecentVaultEntry>>,
    ) {
        match download_file_bytes(&file_id).await {
            Ok(bytes) => {
                let key = DatabaseKey::new().with_password(&secret);
                match Database::parse(&bytes, key) {
                    Ok(db) => {
                        let fid = file_id.clone();
                        let fname = file_name.clone();
                        spawn(async move {
                            let _ = add_recent_vault_api(&fid, &fname, None).await;
                            if let Ok(recents) = fetch_recent_vaults().await {
                                recent_vaults.set(recents);
                            }
                        });

                        // Decide whether to surface the enrol prompt. Only
                        // applies on the manual-unlock path; biometric-auto
                        // skips it.
                        if let Some(mut target) = enrol_target {
                            if !skip_flag_get(&user_id, &file_id)
                                && !biometric::is_enrolled(&user_id, &file_id).await
                                && biometric::status().await.available
                            {
                                target.set(Some(EnrolPromptState {
                                    user_id: user_id.clone(),
                                    vault_id: file_id.clone(),
                                    vault_name: file_name.clone(),
                                    master_password: secret.clone(),
                                }));
                            }
                        }

                        master_password.set(secret);
                        vault.set(Some(VaultState {
                            db,
                            file_id: Some(file_id),
                            file_name,
                            dirty: false,
                        }));
                    }
                    Err(e) => {
                        error.set(Some(format!("Failed to open vault: {:?}", e)));
                        unlocking.set(false);
                    }
                }
            }
            Err(e) => {
                error.set(Some(e));
                unlocking.set(false);
            }
        }
    }

    let (fid_k, fname_k, uid_k) = (file_id.clone(), file_name.clone(), user_id.clone());
    let (fid_b, fname_b, uid_b) = (file_id.clone(), file_name.clone(), user_id.clone());
    let file_name_display = file_name.clone();
    let folder_display = folder_path.clone();

    rsx! {
        div { class: "card bg-base-200 mb-3",
            div { class: "card-body p-4",
                div { class: "flex items-center gap-2 mb-2",
                    IconLock { class: "w-5 h-5".to_string() }
                    div { class: "flex flex-col",
                        span { class: "font-medium", "{file_name_display}" }
                        if let Some(ref path) = folder_display {
                            span { class: "text-xs text-base-content/50", "{path}" }
                        }
                    }
                }
                if let Some(err) = error() {
                    div { class: "alert alert-error alert-sm text-sm mb-2", "{err}" }
                }
                div { class: "flex gap-2",
                    input {
                        class: "input input-bordered input-sm flex-1",
                        r#type: "password",
                        placeholder: "Master password",
                        value: "{password}",
                        disabled: unlocking(),
                        oninput: move |e| password.set(e.value()),
                        onkeypress: move |e| {
                            if e.key() == Key::Enter {
                                do_unlock(fid_k.clone(), fname_k.clone(), uid_k.clone(), password, error, unlocking, master_password, vault, recent_vaults, pending_enrol);
                            }
                        },
                    }
                    button {
                        class: "btn btn-primary btn-sm",
                        disabled: unlocking() || password().is_empty(),
                        onclick: move |_| {
                            do_unlock(fid_b.clone(), fname_b.clone(), uid_b.clone(), password, error, unlocking, master_password, vault, recent_vaults, pending_enrol);
                        },
                        if unlocking() {
                            span { class: "loading loading-spinner loading-xs" }
                        } else {
                            "Unlock"
                        }
                    }
                }
            }
        }
    }
}

// ── Biometric enrolment prompt (one-time per vault) ───────────────────────

#[component]
fn BiometricEnrolPrompt(
    state: Signal<Option<EnrolPromptState>>,
    enrolled_map: Signal<HashMap<String, bool>>,
) -> Element {
    let snapshot = state();
    let Some(s) = snapshot else { return rsx! {} };
    let mut state_w = state;
    let mut enrolled_map_w = enrolled_map;
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut working = use_signal(|| false);

    let vault_name = s.vault_name.clone();
    let user_id = s.user_id.clone();
    let vault_id = s.vault_id.clone();
    let secret = s.master_password.clone();

    let user_id_yes = user_id.clone();
    let vault_id_yes = vault_id.clone();
    let user_id_no = user_id.clone();
    let vault_id_no = vault_id.clone();

    rsx! {
        div { class: "modal modal-open",
            div { class: "modal-box max-w-md",
                h3 { class: "font-bold text-lg mb-2", "Enable biometric unlock?" }
                p { class: "text-sm text-base-content/70 mb-3",
                    "Use your fingerprint to unlock "
                    span { class: "font-medium", "{vault_name}" }
                    " on this device. The master password is stored in the device's secure keystore and only released after biometric authentication."
                }
                if let Some(err) = error() {
                    div { class: "alert alert-error alert-sm text-sm mb-2", "{err}" }
                }
                div { class: "modal-action",
                    button {
                        class: "btn btn-ghost",
                        disabled: working(),
                        onclick: move |_| {
                            skip_flag_set(&user_id_no, &vault_id_no);
                            state_w.set(None);
                        },
                        "Not now"
                    }
                    button {
                        class: "btn btn-primary",
                        disabled: working(),
                        onclick: move |_| {
                            let uid = user_id_yes.clone();
                            let vid = vault_id_yes.clone();
                            let pw = secret.clone();
                            working.set(true);
                            error.set(None);
                            spawn(async move {
                                match biometric::enroll(&uid, &vid, &pw).await {
                                    Ok(()) => {
                                        let mut next = enrolled_map_w();
                                        next.insert(vid.clone(), true);
                                        enrolled_map_w.set(next);
                                        skip_flag_clear(&uid, &vid);
                                        state_w.set(None);
                                    }
                                    Err(e) => {
                                        working.set(false);
                                        error.set(Some(format!("Failed to enable: {e}")));
                                    }
                                }
                            });
                        },
                        if working() {
                            span { class: "loading loading-spinner loading-xs" }
                        } else {
                            "Enable"
                        }
                    }
                }
            }
            div { class: "modal-backdrop" }
        }
    }
}

// ── Vault browser (main UI after unlock) ───────────────────────────────────

#[component]
fn VaultBrowser(
    vault: Signal<Option<VaultState>>,
    master_password: String,
) -> Element {
    let mut session = use_context::<Signal<VaultSession>>();
    let mut selected_group: Signal<Option<uuid::Uuid>> = use_signal(|| None);
    let mut selected_entry: Signal<Option<uuid::Uuid>> = use_signal(|| None);
    let mut search_query: Signal<String> = use_signal(String::new);
    let mut saving: Signal<bool> = use_signal(|| false);
    let mut save_error: Signal<Option<String>> = use_signal(|| None);
    let mut save_ok: Signal<bool> = use_signal(|| false);
    let mut show_new_entry: Signal<bool> = use_signal(|| false);
    let mut show_new_group: Signal<bool> = use_signal(|| false);
    let mut editing_entry: Signal<Option<uuid::Uuid>> = use_signal(|| None);
    let mut confirm_delete: Signal<Option<uuid::Uuid>> = use_signal(|| None);

    let vs = vault().unwrap();
    let groups = collect_groups(&vs.db.root, 0);
    let all_entries = collect_entries(&vs.db.root, "");

    // Filter entries by search or selected group
    let filtered_entries: Vec<EntryView> = if !search_query().is_empty() {
        let q = search_query().to_lowercase();
        all_entries.into_iter().filter(|e| {
            e.title.to_lowercase().contains(&q)
                || e.username.to_lowercase().contains(&q)
                || e.url.to_lowercase().contains(&q)
        }).collect()
    } else if let Some(gid) = selected_group() {
        find_group_entries(&vs.db.root, gid)
    } else {
        all_entries
    };

    let file_name = vs.file_name.clone();
    let mp = master_password.clone();

    let on_save = move |_| {
        saving.set(true);
        save_error.set(None);
        save_ok.set(false);

        let mp = mp.clone();
        spawn(async move {
            let vs = vault().unwrap();
            let key = DatabaseKey::new().with_password(&mp);
            let mut buffer = Vec::new();
            match vs.db.save(&mut buffer, key) {
                Ok(()) => {
                    if let Some(ref fid) = vs.file_id {
                        match save_vault_bytes(fid, buffer, &vs.file_name).await {
                            Ok(()) => {
                                let mut v = vault().unwrap();
                                v.dirty = false;
                                vault.set(Some(v));
                                save_ok.set(true);
                                saving.set(false);
                            }
                            Err(e) => {
                                save_error.set(Some(e));
                                saving.set(false);
                            }
                        }
                    }
                }
                Err(e) => {
                    save_error.set(Some(format!("Encryption failed: {:?}", e)));
                    saving.set(false);
                }
            }
        });
    };

    rsx! {
        div { class: "flex flex-col h-full",
            // Bump activity on any user input within the vault UI so the
            // 5-min idle TTL only fires after genuine inactivity.
            onpointerdown: move |_| { session.write().bump(); },
            onkeydown: move |_| { session.write().bump(); },
            // Toolbar
            div { class: "flex items-center gap-2 mb-4 flex-wrap",
                h2 { class: "text-xl font-bold flex-1 flex items-center gap-2",
                    IconLockOpen { class: "w-5 h-5".to_string() }
                    span { class: "truncate", "{file_name}" }
                    if vs.dirty {
                        span { class: "badge badge-warning badge-sm", "unsaved" }
                    }
                }

                button {
                    class: "btn btn-primary btn-sm",
                    disabled: saving() || !vs.dirty,
                    onclick: on_save,
                    if saving() {
                        span { class: "loading loading-spinner loading-xs" }
                    }
                    "Save"
                }

                button {
                    class: "btn btn-ghost btn-sm",
                    onclick: move |_| {
                        vault.set(None);
                    },
                    "Lock"
                }
            }

            if let Some(err) = save_error() {
                div { class: "alert alert-error mb-3 text-sm", "{err}" }
            }
            if save_ok() {
                div { class: "alert alert-success mb-3 text-sm", "Vault saved" }
            }

            // Search bar
            div { class: "mb-4",
                input {
                    class: "input input-bordered w-full",
                    r#type: "text",
                    placeholder: "Search entries...",
                    value: "{search_query}",
                    oninput: move |e| {
                        search_query.set(e.value());
                        selected_entry.set(None);
                    },
                }
            }

            // Main content: groups sidebar + entry list + entry detail
            div { class: "flex flex-col lg:flex-row gap-4 flex-1 min-h-0 overflow-hidden",
                // Groups sidebar
                div { class: "lg:w-48 shrink-0",
                    div { class: "flex items-center justify-between mb-2",
                        span { class: "text-sm font-semibold text-base-content/60", "Groups" }
                        button {
                            class: "btn btn-ghost btn-xs",
                            onclick: move |_| show_new_group.set(true),
                            "+"
                        }
                    }
                    ul { class: "menu menu-sm bg-base-200 rounded-box w-full",
                        li {
                            a {
                                class: if selected_group().is_none() && search_query().is_empty() { "active" } else { "" },
                                onclick: move |_| {
                                    selected_group.set(None);
                                    selected_entry.set(None);
                                    search_query.set(String::new());
                                },
                                "All entries"
                            }
                        }
                        for group in groups.iter() {
                            {
                                let gid = group.uuid;
                                let indent_px = group.depth * 12;
                                let is_active = selected_group() == Some(gid);
                                rsx! {
                                    li {
                                        a {
                                            class: if is_active { "active" } else { "" },
                                            style: "padding-left: calc(0.75rem + {indent_px}px)",
                                            onclick: move |_| {
                                                selected_group.set(Some(gid));
                                                selected_entry.set(None);
                                                search_query.set(String::new());
                                            },
                                            span { class: "truncate", "{group.name}" }
                                            span { class: "badge badge-sm badge-ghost", "{group.entry_count}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Entry list
                div { class: "flex-1 min-w-0 overflow-auto",
                    div { class: "flex items-center justify-between mb-2",
                        span { class: "text-sm font-semibold text-base-content/60",
                            "{filtered_entries.len()} entries"
                        }
                        button {
                            class: "btn btn-primary btn-xs",
                            onclick: move |_| show_new_entry.set(true),
                            "+ New Entry"
                        }
                    }

                    if filtered_entries.is_empty() {
                        div { class: "text-center py-8 text-base-content/50",
                            "No entries found"
                        }
                    } else {
                        div { class: "flex flex-col gap-1",
                            for entry in filtered_entries.iter() {
                                {
                                    let eid = entry.uuid;
                                    let is_selected = selected_entry() == Some(eid);
                                    rsx! {
                                        div {
                                            class: if is_selected {
                                                "p-3 rounded-lg bg-primary/10 border border-primary cursor-pointer"
                                            } else {
                                                "p-3 rounded-lg hover:bg-base-200 cursor-pointer border border-transparent"
                                            },
                                            onclick: move |_| {
                                                selected_entry.set(Some(eid));
                                                editing_entry.set(None);
                                            },
                                            div { class: "font-medium", "{entry.title}" }
                                            div { class: "text-sm text-base-content/60 flex items-center gap-2",
                                                if !entry.username.is_empty() {
                                                    span { "{entry.username}" }
                                                }
                                                if !entry.url.is_empty() {
                                                    span { class: "truncate", "{entry.url}" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Entry detail panel
                if let Some(eid) = selected_entry() {
                    {
                        if let Some(entry) = find_entry(&vs.db.root, eid) {
                            if editing_entry() == Some(eid) {
                                rsx! {
                                    EntryEditor {
                                        vault,
                                        entry_uuid: eid,
                                        on_done: move |_| editing_entry.set(None),
                                    }
                                }
                            } else {
                                rsx! {
                                    EntryDetail {
                                        entry: entry.clone(),
                                        on_edit: move |_| editing_entry.set(Some(eid)),
                                        on_delete: move |_| confirm_delete.set(Some(eid)),
                                    }
                                }
                            }
                        } else {
                            rsx! {
                                div { class: "lg:w-80 shrink-0 text-base-content/50 text-center py-8",
                                    "Entry not found"
                                }
                            }
                        }
                    }
                }
            }
        }

        // New entry modal
        if show_new_entry() {
            NewEntryModal {
                vault,
                group_uuid: selected_group(),
                on_close: move |_| show_new_entry.set(false),
                on_created: move |uuid| {
                    show_new_entry.set(false);
                    selected_entry.set(Some(uuid));
                },
            }
        }

        // New group modal
        if show_new_group() {
            NewGroupModal {
                vault,
                parent_uuid: selected_group(),
                on_close: move |_| show_new_group.set(false),
            }
        }

        // Delete confirmation
        if let Some(del_uuid) = confirm_delete() {
            {
                let title = find_entry(&vs.db.root, del_uuid)
                    .and_then(|e| e.get_title())
                    .unwrap_or("this entry")
                    .to_string();
                rsx! {
                    div { class: "modal modal-open",
                        div { class: "modal-box",
                            h3 { class: "font-bold text-lg mb-2", "Delete Entry" }
                            p { class: "text-base-content/70",
                                "Delete \"{title}\"? This cannot be undone."
                            }
                            div { class: "modal-action",
                                button {
                                    class: "btn",
                                    onclick: move |_| confirm_delete.set(None),
                                    "Cancel"
                                }
                                button {
                                    class: "btn btn-error",
                                    onclick: move |_| {
                                        let mut vs = vault().unwrap();
                                        delete_entry_from_group(&mut vs.db.root, del_uuid);
                                        vs.dirty = true;
                                        vault.set(Some(vs));
                                        selected_entry.set(None);
                                        confirm_delete.set(None);
                                    },
                                    "Delete"
                                }
                            }
                        }
                        div { class: "modal-backdrop", onclick: move |_| confirm_delete.set(None) }
                    }
                }
            }
        }
    }
}

fn delete_entry_from_group(group: &mut Group, uuid: uuid::Uuid) -> bool {
    if let Some(pos) = group.entries.iter().position(|e| e.uuid == uuid) {
        group.entries.remove(pos);
        return true;
    }
    for child in &mut group.groups {
        if delete_entry_from_group(child, uuid) {
            return true;
        }
    }
    false
}

// ── Entry detail (read-only view) ──────────────────────────────────────────

#[component]
fn EntryDetail(
    entry: Entry,
    on_edit: EventHandler<()>,
    on_delete: EventHandler<()>,
) -> Element {
    let mut show_password = use_signal(|| false);
    let mut copied: Signal<Option<String>> = use_signal(|| None);

    let title = entry.get_title().unwrap_or("Untitled").to_string();
    let username = entry.get_username().unwrap_or("").to_string();
    let password = entry.get_password().unwrap_or("").to_string();
    let url = entry.get_url().unwrap_or("").to_string();
    let notes = entry.get("Notes").unwrap_or("").to_string();

    let copy_to_clipboard = {
        fn do_copy(field: String, value: String, mut copied: Signal<Option<String>>) {
            if let Some(window) = web_sys::window() {
                let clipboard = window.navigator().clipboard();
                let _ = clipboard.write_text(&value);
                copied.set(Some(field));
                spawn(async move {
                    gloo_timers::future::TimeoutFuture::new(2000).await;
                    copied.set(None);
                });
            }
        }
        move |field: String, value: String| do_copy(field, value, copied)
    };

    rsx! {
        div { class: "lg:w-80 shrink-0 bg-base-200 rounded-lg p-4 overflow-auto",
            div { class: "flex items-center justify-between mb-4",
                h3 { class: "font-bold text-lg truncate", "{title}" }
                div { class: "flex gap-1",
                    button {
                        class: "btn btn-ghost btn-xs",
                        onclick: move |_| on_edit.call(()),
                        "Edit"
                    }
                    button {
                        class: "btn btn-ghost btn-xs text-error",
                        onclick: move |_| on_delete.call(()),
                        "Delete"
                    }
                }
            }

            // Fields
            div { class: "flex flex-col gap-3",
                if !username.is_empty() {
                    {
                        let uname = username.clone();
                        rsx! {
                            FieldRow {
                                label: "Username",
                                value: username,
                                copyable: true,
                                on_copy: move |_| copy_to_clipboard("username".to_string(), uname.clone()),
                                copied: copied() == Some("username".to_string()),
                            }
                        }
                    }
                }

                if !password.is_empty() {
                    {
                        let pw = password.clone();
                        rsx! {
                            div { class: "flex flex-col gap-1",
                                span { class: "text-xs font-semibold text-base-content/60", "Password" }
                                div { class: "flex items-center gap-1",
                                    code { class: "flex-1 text-sm bg-base-300 px-2 py-1 rounded truncate font-mono",
                                        if show_password() {
                                            "{password}"
                                        } else {
                                            "••••••••"
                                        }
                                    }
                                    button {
                                        class: "btn btn-ghost btn-xs",
                                        onclick: move |_| show_password.toggle(),
                                        if show_password() { "Hide" } else { "Show" }
                                    }
                                    button {
                                        class: "btn btn-ghost btn-xs",
                                        onclick: move |_| copy_to_clipboard("password".to_string(), pw.clone()),
                                        if copied() == Some("password".to_string()) { "Copied!" } else { "Copy" }
                                    }
                                }
                            }
                        }
                    }
                }

                if !url.is_empty() {
                    {
                        let u = url.clone();
                        rsx! {
                            div { class: "flex flex-col gap-1",
                                span { class: "text-xs font-semibold text-base-content/60", "URL" }
                                div { class: "flex items-center gap-1",
                                    a {
                                        class: "link link-primary text-sm truncate flex-1",
                                        href: "{url}",
                                        target: "_blank",
                                        rel: "noopener",
                                        "{url}"
                                    }
                                    button {
                                        class: "btn btn-ghost btn-xs",
                                        onclick: move |_| copy_to_clipboard("url".to_string(), u.clone()),
                                        if copied() == Some("url".to_string()) { "Copied!" } else { "Copy" }
                                    }
                                }
                            }
                        }
                    }
                }

                if !notes.is_empty() {
                    div { class: "flex flex-col gap-1",
                        span { class: "text-xs font-semibold text-base-content/60", "Notes" }
                        pre { class: "text-sm bg-base-300 px-2 py-1 rounded whitespace-pre-wrap break-words font-sans",
                            "{notes}"
                        }
                    }
                }

                // Show any extra/custom fields
                for (key, value) in entry.fields.iter() {
                    {
                        let key_str = key.clone();
                        let is_standard = matches!(key.as_str(), "Title" | "UserName" | "Password" | "URL" | "Notes");
                        if !is_standard && !value.is_empty() {
                            let val = value.as_str().to_string();
                            let val_copy = val.clone();
                            let key_copy = key_str.clone();
                            rsx! {
                                FieldRow {
                                    label: "{key_str}",
                                    value: val,
                                    copyable: true,
                                    on_copy: move |_| copy_to_clipboard(key_copy.clone(), val_copy.clone()),
                                    copied: copied() == Some(key_str.clone()),
                                }
                            }
                        } else {
                            rsx! {}
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn FieldRow(
    label: String,
    value: String,
    copyable: bool,
    on_copy: EventHandler<()>,
    copied: bool,
) -> Element {
    rsx! {
        div { class: "flex flex-col gap-1",
            span { class: "text-xs font-semibold text-base-content/60", "{label}" }
            div { class: "flex items-center gap-1",
                span { class: "flex-1 text-sm truncate", "{value}" }
                if copyable {
                    button {
                        class: "btn btn-ghost btn-xs",
                        onclick: move |_| on_copy.call(()),
                        if copied { "Copied!" } else { "Copy" }
                    }
                }
            }
        }
    }
}

// ── New entry modal ────────────────────────────────────────────────────────

#[component]
fn NewEntryModal(
    vault: Signal<Option<VaultState>>,
    group_uuid: Option<uuid::Uuid>,
    on_close: EventHandler<()>,
    on_created: EventHandler<uuid::Uuid>,
) -> Element {
    let mut title = use_signal(String::new);
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut url = use_signal(String::new);
    let mut notes = use_signal(String::new);

    // Password generator state
    let mut show_generator = use_signal(|| false);
    let mut gen_length = use_signal(|| 20u32);
    let mut gen_upper = use_signal(|| true);
    let mut gen_lower = use_signal(|| true);
    let mut gen_digits = use_signal(|| true);
    let mut gen_symbols = use_signal(|| true);

    rsx! {
        div { class: "modal modal-open",
            div { class: "modal-box max-w-lg",
                h3 { class: "font-bold text-lg mb-4", "New Entry" }

                div { class: "flex flex-col gap-3",
                    div { class: "form-control",
                        label { class: "label", span { class: "label-text", "Title" } }
                        input {
                            class: "input input-bordered w-full",
                            r#type: "text",
                            value: "{title}",
                            oninput: move |e| title.set(e.value()),
                        }
                    }
                    div { class: "form-control",
                        label { class: "label", span { class: "label-text", "Username" } }
                        input {
                            class: "input input-bordered w-full",
                            r#type: "text",
                            value: "{username}",
                            oninput: move |e| username.set(e.value()),
                        }
                    }
                    div { class: "form-control",
                        label { class: "label", span { class: "label-text", "Password" } }
                        div { class: "flex gap-2",
                            input {
                                class: "input input-bordered flex-1",
                                r#type: "text",
                                value: "{password}",
                                oninput: move |e| password.set(e.value()),
                            }
                            button {
                                class: "btn btn-outline btn-sm self-center",
                                onclick: move |_| show_generator.toggle(),
                                "Generate"
                            }
                        }

                        if show_generator() {
                            div { class: "mt-2 p-3 bg-base-200 rounded-lg",
                                div { class: "flex items-center gap-2 mb-2",
                                    label { class: "text-sm", "Length:" }
                                    input {
                                        class: "input input-bordered input-xs w-16",
                                        r#type: "number",
                                        min: "4",
                                        max: "128",
                                        value: "{gen_length}",
                                        oninput: move |e| {
                                            if let Ok(v) = e.value().parse::<u32>() {
                                                gen_length.set(v.clamp(4, 128));
                                            }
                                        },
                                    }
                                }
                                div { class: "flex flex-wrap gap-3 mb-2",
                                    label { class: "flex items-center gap-1 text-sm cursor-pointer",
                                        input {
                                            r#type: "checkbox",
                                            class: "checkbox checkbox-xs",
                                            checked: gen_upper(),
                                            onchange: move |_| gen_upper.toggle(),
                                        }
                                        "A-Z"
                                    }
                                    label { class: "flex items-center gap-1 text-sm cursor-pointer",
                                        input {
                                            r#type: "checkbox",
                                            class: "checkbox checkbox-xs",
                                            checked: gen_lower(),
                                            onchange: move |_| gen_lower.toggle(),
                                        }
                                        "a-z"
                                    }
                                    label { class: "flex items-center gap-1 text-sm cursor-pointer",
                                        input {
                                            r#type: "checkbox",
                                            class: "checkbox checkbox-xs",
                                            checked: gen_digits(),
                                            onchange: move |_| gen_digits.toggle(),
                                        }
                                        "0-9"
                                    }
                                    label { class: "flex items-center gap-1 text-sm cursor-pointer",
                                        input {
                                            r#type: "checkbox",
                                            class: "checkbox checkbox-xs",
                                            checked: gen_symbols(),
                                            onchange: move |_| gen_symbols.toggle(),
                                        }
                                        "!@#$"
                                    }
                                }
                                button {
                                    class: "btn btn-sm btn-primary",
                                    onclick: move |_| {
                                        let pw = generate_password(
                                            gen_length() as usize,
                                            gen_upper(),
                                            gen_lower(),
                                            gen_digits(),
                                            gen_symbols(),
                                        );
                                        password.set(pw);
                                    },
                                    "Generate Password"
                                }
                            }
                        }
                    }
                    div { class: "form-control",
                        label { class: "label", span { class: "label-text", "URL" } }
                        input {
                            class: "input input-bordered w-full",
                            r#type: "text",
                            value: "{url}",
                            oninput: move |e| url.set(e.value()),
                        }
                    }
                    div { class: "form-control",
                        label { class: "label", span { class: "label-text", "Notes" } }
                        textarea {
                            class: "textarea textarea-bordered w-full",
                            rows: "3",
                            value: "{notes}",
                            oninput: move |e| notes.set(e.value()),
                        }
                    }
                }

                div { class: "modal-action",
                    button {
                        class: "btn",
                        onclick: move |_| on_close.call(()),
                        "Cancel"
                    }
                    button {
                        class: "btn btn-primary",
                        disabled: title().trim().is_empty(),
                        onclick: move |_| {
                            let mut entry = Entry::new();
                            entry.set_unprotected("Title", title().trim());
                            if !username().is_empty() {
                                entry.set_unprotected("UserName", username().trim());
                            }
                            if !password().is_empty() {
                                entry.set_protected("Password", password());
                            }
                            if !url().is_empty() {
                                entry.set_unprotected("URL", url().trim());
                            }
                            if !notes().is_empty() {
                                entry.set_unprotected("Notes", notes());
                            }

                            let entry_uuid = entry.uuid;
                            let mut vs = vault().unwrap();

                            // Add to selected group or root
                            if let Some(gid) = group_uuid {
                                if let Some(g) = vs.db.root.group_by_uuid_mut(gid) {
                                    g.entries.push(entry);
                                } else {
                                    vs.db.root.entries.push(entry);
                                }
                            } else {
                                vs.db.root.entries.push(entry);
                            }

                            vs.dirty = true;
                            vault.set(Some(vs));
                            on_created.call(entry_uuid);
                        },
                        "Create"
                    }
                }
            }
            div { class: "modal-backdrop", onclick: move |_| on_close.call(()) }
        }
    }
}

// ── New group modal ────────────────────────────────────────────────────────

#[component]
fn NewGroupModal(
    vault: Signal<Option<VaultState>>,
    parent_uuid: Option<uuid::Uuid>,
    on_close: EventHandler<()>,
) -> Element {
    let mut name = use_signal(String::new);

    rsx! {
        div { class: "modal modal-open",
            div { class: "modal-box",
                h3 { class: "font-bold text-lg mb-4", "New Group" }
                div { class: "form-control",
                    label { class: "label", span { class: "label-text", "Group name" } }
                    input {
                        class: "input input-bordered w-full",
                        r#type: "text",
                        value: "{name}",
                        oninput: move |e| name.set(e.value()),
                    }
                }
                div { class: "modal-action",
                    button {
                        class: "btn",
                        onclick: move |_| on_close.call(()),
                        "Cancel"
                    }
                    button {
                        class: "btn btn-primary",
                        disabled: name().trim().is_empty(),
                        onclick: move |_| {
                            let group = Group::new(name().trim());
                            let mut vs = vault().unwrap();

                            if let Some(pid) = parent_uuid {
                                if let Some(g) = vs.db.root.group_by_uuid_mut(pid) {
                                    g.groups.push(group);
                                } else {
                                    vs.db.root.groups.push(group);
                                }
                            } else {
                                vs.db.root.groups.push(group);
                            }

                            vs.dirty = true;
                            vault.set(Some(vs));
                            on_close.call(());
                        },
                        "Create"
                    }
                }
            }
            div { class: "modal-backdrop", onclick: move |_| on_close.call(()) }
        }
    }
}

// ── Entry editor ───────────────────────────────────────────────────────────

#[component]
fn EntryEditor(
    vault: Signal<Option<VaultState>>,
    entry_uuid: uuid::Uuid,
    on_done: EventHandler<()>,
) -> Element {
    let vs = vault().unwrap();
    let entry = find_entry(&vs.db.root, entry_uuid);

    let mut title = use_signal(|| entry.and_then(|e| e.get_title()).unwrap_or("").to_string());
    let mut username = use_signal(|| entry.and_then(|e| e.get_username()).unwrap_or("").to_string());
    let mut password = use_signal(|| entry.and_then(|e| e.get_password()).unwrap_or("").to_string());
    let mut url = use_signal(|| entry.and_then(|e| e.get_url()).unwrap_or("").to_string());
    let mut notes = use_signal(|| entry.and_then(|e| e.get("Notes")).unwrap_or("").to_string());

    // Custom fields: (key, value, is_protected)
    let mut custom_fields: Signal<Vec<(String, String, bool)>> = use_signal(|| {
        let standard = ["Title", "UserName", "Password", "URL", "Notes"];
        entry.map(|e| {
            e.fields.iter()
                .filter(|(k, _)| !standard.contains(&k.as_str()))
                .filter(|(_, v)| !v.is_empty())
                .map(|(k, v)| (k.clone(), v.as_str().to_string(), v.is_protected()))
                .collect::<Vec<_>>()
        }).unwrap_or_default()
    });

    // Password generator
    let mut show_generator = use_signal(|| false);
    let mut gen_length = use_signal(|| 20u32);
    let mut gen_upper = use_signal(|| true);
    let mut gen_lower = use_signal(|| true);
    let mut gen_digits = use_signal(|| true);
    let mut gen_symbols = use_signal(|| true);

    rsx! {
        div { class: "lg:w-96 shrink-0 bg-base-200 rounded-lg p-4 overflow-auto",
            h3 { class: "font-bold text-lg mb-4", "Edit Entry" }

            div { class: "flex flex-col gap-3",
                div { class: "form-control",
                    label { class: "label", span { class: "label-text", "Title" } }
                    input {
                        class: "input input-bordered w-full input-sm",
                        r#type: "text",
                        value: "{title}",
                        oninput: move |e| title.set(e.value()),
                    }
                }
                div { class: "form-control",
                    label { class: "label", span { class: "label-text", "Username" } }
                    input {
                        class: "input input-bordered w-full input-sm",
                        r#type: "text",
                        value: "{username}",
                        oninput: move |e| username.set(e.value()),
                    }
                }
                div { class: "form-control",
                    label { class: "label", span { class: "label-text", "Password" } }
                    input {
                        class: "input input-bordered w-full input-sm",
                        r#type: "text",
                        value: "{password}",
                        oninput: move |e| password.set(e.value()),
                    }
                    button {
                        class: "btn btn-outline btn-xs mt-1 self-start",
                        onclick: move |_| show_generator.toggle(),
                        "Generate"
                    }
                    if show_generator() {
                        div { class: "mt-2 p-3 bg-base-300 rounded-lg",
                            div { class: "flex items-center gap-2 mb-2",
                                label { class: "text-sm", "Length:" }
                                input {
                                    class: "input input-bordered input-xs w-16",
                                    r#type: "number",
                                    min: "4",
                                    max: "128",
                                    value: "{gen_length}",
                                    oninput: move |e| {
                                        if let Ok(v) = e.value().parse::<u32>() {
                                            gen_length.set(v.clamp(4, 128));
                                        }
                                    },
                                }
                            }
                            div { class: "flex flex-wrap gap-3 mb-2",
                                label { class: "flex items-center gap-1 text-sm cursor-pointer",
                                    input {
                                        r#type: "checkbox",
                                        class: "checkbox checkbox-xs",
                                        checked: gen_upper(),
                                        onchange: move |_| gen_upper.toggle(),
                                    }
                                    "A-Z"
                                }
                                label { class: "flex items-center gap-1 text-sm cursor-pointer",
                                    input {
                                        r#type: "checkbox",
                                        class: "checkbox checkbox-xs",
                                        checked: gen_lower(),
                                        onchange: move |_| gen_lower.toggle(),
                                    }
                                    "a-z"
                                }
                                label { class: "flex items-center gap-1 text-sm cursor-pointer",
                                    input {
                                        r#type: "checkbox",
                                        class: "checkbox checkbox-xs",
                                        checked: gen_digits(),
                                        onchange: move |_| gen_digits.toggle(),
                                    }
                                    "0-9"
                                }
                                label { class: "flex items-center gap-1 text-sm cursor-pointer",
                                    input {
                                        r#type: "checkbox",
                                        class: "checkbox checkbox-xs",
                                        checked: gen_symbols(),
                                        onchange: move |_| gen_symbols.toggle(),
                                    }
                                    "!@#$"
                                }
                            }
                            button {
                                class: "btn btn-sm btn-primary",
                                onclick: move |_| {
                                    let pw = generate_password(
                                        gen_length() as usize,
                                        gen_upper(),
                                        gen_lower(),
                                        gen_digits(),
                                        gen_symbols(),
                                    );
                                    password.set(pw);
                                },
                                "Generate Password"
                            }
                        }
                    }
                }
                div { class: "form-control",
                    label { class: "label", span { class: "label-text", "URL" } }
                    input {
                        class: "input input-bordered w-full input-sm",
                        r#type: "text",
                        value: "{url}",
                        oninput: move |e| url.set(e.value()),
                    }
                }
                div { class: "form-control",
                    label { class: "label", span { class: "label-text", "Notes" } }
                    textarea {
                        class: "textarea textarea-bordered w-full textarea-sm",
                        rows: "3",
                        value: "{notes}",
                        oninput: move |e| notes.set(e.value()),
                    }
                }

                // Custom fields
                div { class: "divider text-xs text-base-content/50 my-1", "Custom Fields" }
                for (idx, (key, value, protected)) in custom_fields().iter().enumerate() {
                    {
                        let key = key.clone();
                        let value = value.clone();
                        let protected = *protected;
                        rsx! {
                            div { class: "flex flex-col gap-1 bg-base-300 rounded p-2",
                                div { class: "flex items-center gap-1",
                                    input {
                                        class: "input input-bordered input-xs flex-1",
                                        r#type: "text",
                                        placeholder: "Field name",
                                        value: "{key}",
                                        oninput: move |e| {
                                            let mut fields = custom_fields();
                                            fields[idx].0 = e.value();
                                            custom_fields.set(fields);
                                        },
                                    }
                                    label { class: "flex items-center gap-1 text-xs cursor-pointer whitespace-nowrap",
                                        input {
                                            r#type: "checkbox",
                                            class: "checkbox checkbox-xs",
                                            checked: protected,
                                            onchange: move |_| {
                                                let mut fields = custom_fields();
                                                fields[idx].2 = !fields[idx].2;
                                                custom_fields.set(fields);
                                            },
                                        }
                                        "Protected"
                                    }
                                    button {
                                        class: "btn btn-ghost btn-xs text-error",
                                        onclick: move |_| {
                                            let mut fields = custom_fields();
                                            fields.remove(idx);
                                            custom_fields.set(fields);
                                        },
                                        IconX { class: "w-3 h-3".to_string() }
                                    }
                                }
                                input {
                                    class: "input input-bordered input-xs w-full",
                                    r#type: if protected { "password" } else { "text" },
                                    placeholder: "Value",
                                    value: "{value}",
                                    oninput: move |e| {
                                        let mut fields = custom_fields();
                                        fields[idx].1 = e.value();
                                        custom_fields.set(fields);
                                    },
                                }
                            }
                        }
                    }
                }
                button {
                    class: "btn btn-outline btn-xs self-start",
                    onclick: move |_| {
                        let mut fields = custom_fields();
                        fields.push(("".to_string(), "".to_string(), false));
                        custom_fields.set(fields);
                    },
                    "+ Add Field"
                }
            }

            div { class: "flex gap-2 mt-4",
                button {
                    class: "btn btn-sm",
                    onclick: move |_| on_done.call(()),
                    "Cancel"
                }
                button {
                    class: "btn btn-primary btn-sm",
                    onclick: move |_| {
                        let mut vs = vault().unwrap();
                        if let Some(entry) = find_entry_mut(&mut vs.db.root, entry_uuid) {
                            entry.update_history();
                            entry.set_unprotected("Title", title().trim());
                            entry.set_unprotected("UserName", username().trim());
                            entry.set_protected("Password", password());
                            entry.set_unprotected("URL", url().trim());
                            entry.set_unprotected("Notes", notes());

                            // Remove old custom fields that are no longer present
                            let standard = ["Title", "UserName", "Password", "URL", "Notes"];
                            let new_keys: Vec<String> = custom_fields().iter().map(|(k, _, _)| k.clone()).collect();
                            let old_custom_keys: Vec<String> = entry.fields.keys()
                                .filter(|k| !standard.contains(&k.as_str()))
                                .cloned()
                                .collect();
                            for old_key in &old_custom_keys {
                                if !new_keys.contains(old_key) {
                                    entry.fields.remove(old_key);
                                }
                            }

                            // Set custom fields
                            for (key, value, protected) in custom_fields().iter() {
                                if !key.trim().is_empty() {
                                    if *protected {
                                        entry.set_protected(key.trim(), value.as_str());
                                    } else {
                                        entry.set_unprotected(key.trim(), value.as_str());
                                    }
                                }
                            }
                        }
                        vs.dirty = true;
                        vault.set(Some(vs));
                        on_done.call(());
                    },
                    "Save"
                }
            }
        }
    }
}
