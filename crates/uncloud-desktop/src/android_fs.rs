//! SAF-backed [`LocalFs`] implementation for Android.
//!
//! The sync engine's [`LocalFs`] trait is path-string oriented. On Android the
//! "path" string encodes a SAF tree URI plus an optional slash-separated
//! relative subpath, joined by an ASCII `|` which never appears in a valid
//! `content://` URI:
//!
//! ```text
//! content://.../tree/primary%3AMusic               ← root
//! content://.../tree/primary%3AMusic|Artist        ← descendant
//! content://.../tree/primary%3AMusic|Artist/Album  ← descendant
//! ```
//!
//! All resolution goes through `tauri-plugin-android-fs`, which handles the
//! DocumentProvider calls and respects the persisted URI permission taken
//! when the user picked the folder.
//!
//! This module is only compiled on mobile targets; the struct is stubbed out
//! on desktop so `lib.rs` can cfg-gate the wiring cleanly.

#![cfg(mobile)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use tauri::{AppHandle, Manager};
use tauri_plugin_android_fs::{AndroidFsExt, Entry, FileUri};
use uncloud_sync::{
    file_chunks, is_excluded_name, sha256_hex_reader, write_chunks, ByteStream, LocalFs,
    LocalFsError, WalkEntry,
};

/// Makes the name of each spooled download unique within the process.
static SPOOL_SEQ: AtomicU64 = AtomicU64::new(0);

/// One directory's entries by name: whether each is a directory, and its URI.
type Listing = Arc<HashMap<String, (bool, FileUri)>>;

/// `|` as delimiter is safe: SAF URIs percent-encode everything that isn't
/// in the unreserved/safe set, and `|` is not in either.
const SEP: char = '|';

pub struct AndroidSafFs {
    app: AppHandle,
    /// Directory listings taken to check paths the quick probes could not
    /// read, keyed by directory path and kept for one sync run. Writing a
    /// file leaves them alone on purpose: re-listing a directory after every
    /// download into it would be quadratic, and a listing that misses a new
    /// file only ever answers "absent", which is what the quick probe alone
    /// used to say.
    listings: Mutex<HashMap<String, Listing>>,
}

impl AndroidSafFs {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            listings: Mutex::new(HashMap::new()),
        }
    }

    fn parent_of(path: &str) -> String {
        let (root, rel) = Self::split(path);
        match rel.rsplit_once('/') {
            Some((parent, _)) => format!("{root}{SEP}{parent}"),
            None => root,
        }
    }

    /// Forget what was listed in `path`'s directory after changing it.
    fn forget_listing_around(&self, path: &str) {
        self.listings
            .lock()
            .unwrap()
            .remove(&Self::parent_of(path));
    }

    async fn listing(&self, dir_path: &str, dir: &FileUri) -> Result<Listing, LocalFsError> {
        if let Some(listing) = self.listings.lock().unwrap().get(dir_path).cloned() {
            return Ok(listing);
        }
        let entries = self
            .app
            .android_fs_async()
            .read_dir(dir)
            .await
            .map_err(|e| LocalFsError::other(format!("read_dir({dir_path}): {e}")))?;
        let listing: Listing = Arc::new(
            entries
                .into_iter()
                .map(|entry| match entry {
                    Entry::Dir { uri, name, .. } => (name, (true, uri)),
                    Entry::File { uri, name, .. } => (name, (false, uri)),
                })
                .collect(),
        );
        self.listings
            .lock()
            .unwrap()
            .insert(dir_path.to_owned(), listing.clone());
        Ok(listing)
    }

    /// What the directory listings say about `path`: `Some(is_dir)` when it
    /// is listed, `None` when a listing that succeeded does not have it. A
    /// listing that fails is an error, never an absence.
    async fn listed_kind(&self, path: &str) -> Result<Option<bool>, LocalFsError> {
        let (root, rel) = Self::split(path);
        let parts: Vec<&str> = rel.split('/').filter(|part| !part.is_empty()).collect();
        let mut dir_path = root.clone();
        let mut dir = Self::tree_uri(&root);
        for (index, part) in parts.iter().enumerate() {
            let listing = self.listing(&dir_path, &dir).await?;
            let Some((is_dir, uri)) = listing.get(*part).cloned() else {
                return Ok(None);
            };
            if index + 1 == parts.len() {
                return Ok(Some(is_dir));
            }
            if !is_dir {
                return Ok(None);
            }
            dir_path = self.join(&dir_path, part);
            dir = uri;
        }
        Ok(Some(true))
    }

    // SAF errors carry no kind, so a failed lookup alone cannot tell "not
    // there" from "the provider failed". The listings can: a name missing
    // from a listing that succeeded is absent, while a name that is listed
    // but could not be read is a provider failure the engine must not
    // mistake for a deletion or a missing file.
    async fn absent_or_failed(&self, path: &str, want_dir: bool) -> Result<bool, LocalFsError> {
        match self.listed_kind(path).await? {
            Some(is_dir) if is_dir == want_dir => Err(LocalFsError::other(format!(
                "the storage provider could not read {path}"
            ))),
            _ => Ok(false),
        }
    }

    fn split(path: &str) -> (String, String) {
        match path.find(SEP) {
            Some(idx) => (path[..idx].to_string(), path[idx + 1..].to_string()),
            None => (path.to_string(), String::new()),
        }
    }

    fn tree_uri(root: &str) -> FileUri {
        FileUri {
            uri: root.to_string(),
            document_top_tree_uri: Some(root.to_string()),
        }
    }

    async fn resolve_file(&self, root: &str, rel: &str) -> Result<FileUri, LocalFsError> {
        if rel.is_empty() {
            return Err(LocalFsError::other(
                "resolve_file called with empty rel path",
            ));
        }
        self.app
            .android_fs_async()
            .resolve_file_uri(&Self::tree_uri(root), rel)
            .await
            .map_err(|e| LocalFsError::other(format!("resolve_file_uri({rel}): {e}")))
    }
}

#[async_trait]
impl LocalFs for AndroidSafFs {
    fn begin_run(&self) {
        self.listings.lock().unwrap().clear();
    }

    async fn rename(&self, from: &str, to: &str) -> Result<(), LocalFsError> {
        let (root, rel) = Self::split(from);
        let (to_root, to_rel) = Self::split(to);
        let (parent, old_name) = rel.rsplit_once('/').unwrap_or(("", rel.as_str()));
        let (to_parent, name) = to_rel.rsplit_once('/').unwrap_or(("", to_rel.as_str()));
        if rel.is_empty() || to_rel.is_empty() {
            return Err(LocalFsError::other("a picked folder cannot be renamed"));
        }
        if root != to_root || parent != to_parent {
            return Err(LocalFsError::MoveUnsupported(
                "SAF cannot move entries between directories".to_owned(),
            ));
        }
        if !self.confirm_absent(to).await? {
            return Err(LocalFsError::other("rename destination exists"));
        }
        let api = self.app.android_fs_async();
        let uri = api
            .resolve_dir_uri(&Self::tree_uri(&root), &rel)
            .await
            .map_err(|e| LocalFsError::other(format!("resolve rename source: {e}")))?;
        self.listings.lock().unwrap().clear();
        let renamed = api
            .rename(&uri, name)
            .await
            .map_err(|e| LocalFsError::other(format!("rename({rel}): {e}")))?;
        // A provider may sanitise the name or add a suffix instead of
        // failing. The engine tracks the folder at exactly `to`, so anything
        // else is undone and reported.
        let stored = api.get_name(&renamed).await.unwrap_or_default();
        if stored == name {
            return Ok(());
        }
        match api.rename(&renamed, old_name).await {
            Ok(_) => Err(LocalFsError::other(format!(
                "the storage provider would store {name} as {stored}"
            ))),
            Err(e) => Err(LocalFsError::other(format!(
                "the storage provider stored {name} as {stored} and it could not be renamed back: {e}"
            ))),
        }
    }

    // Shared storage is case-insensitive: the quick probe finds `docs` when
    // asked for `Docs`, while the listings compare names exactly. The engine
    // only asks about two paths it found present, so "same spelling apart
    // from case, and only one of them listed" means one directory.
    async fn same_entry(&self, a: &str, b: &str) -> Result<bool, LocalFsError> {
        if a == b || a.to_lowercase() != b.to_lowercase() {
            return Ok(false);
        }
        Ok(self.confirm_absent(a).await? != self.confirm_absent(b).await?)
    }

    async fn copy_file(&self, from: &str, to: &str) -> Result<(), LocalFsError> {
        let (root, rel) = Self::split(from);
        let (to_root, to_rel) = Self::split(to);
        if to_rel.is_empty() {
            return Err(LocalFsError::other("copy_file called on tree root"));
        }
        let source = self.resolve_file(&root, &rel).await?;
        let api = self.app.android_fs_async();
        let tree = Self::tree_uri(&to_root);
        let dest = match api.resolve_file_uri(&tree, &to_rel).await {
            Ok(uri) => uri,
            Err(_) => api
                .create_new_file(&tree, &to_rel, None)
                .await
                .map_err(|e| LocalFsError::other(format!("create_new_file({to_rel}): {e}")))?,
        };
        api.copy(&source, &dest)
            .await
            .map_err(|e| LocalFsError::other(format!("copy({rel}): {e}")))
    }

    async fn sha256(&self, path: &str) -> Result<String, LocalFsError> {
        let (root, rel) = Self::split(path);
        let uri = self.resolve_file(&root, &rel).await?;
        let file = self
            .app
            .android_fs_async()
            .open_file_readable(&uri)
            .await
            .map_err(|e| LocalFsError::other(format!("open({rel}): {e}")))?;
        tauri::async_runtime::spawn_blocking(move || sha256_hex_reader(file))
            .await
            .map_err(|e| LocalFsError::other(format!("sha256 task failed: {e}")))?
            .map_err(|e| LocalFsError::other(format!("read({rel}): {e}")))
    }

    async fn walk(&self, root: &str) -> Result<Vec<WalkEntry>, LocalFsError> {
        let (root_uri, rel_prefix) = Self::split(root);
        let api = self.app.android_fs_async();

        // Resolve the starting directory: the tree root, or a descendant dir.
        let start: FileUri = if rel_prefix.is_empty() {
            Self::tree_uri(&root_uri)
        } else {
            api.resolve_dir_uri(&Self::tree_uri(&root_uri), &rel_prefix)
                .await
                .map_err(|e| LocalFsError::other(format!("resolve_dir_uri({rel_prefix}): {e}")))?
        };

        let mut out = Vec::new();
        let mut stack: Vec<(FileUri, String)> = vec![(start, String::new())];

        while let Some((dir, prefix)) = stack.pop() {
            let entries = api
                .read_dir(&dir)
                .await
                .map_err(|e| LocalFsError::other(format!("read_dir: {e}")))?;

            for entry in entries {
                let name = entry.name().to_string();
                let new_rel = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}/{name}")
                };
                match entry {
                    Entry::File { last_modified, .. } => {
                        // Sentinel + journal db must never be uploaded or
                        // compared against the journal. The desktop walker
                        // applies the same filter; without it on Android,
                        // `.uncloud-root.json` propagates to the server and
                        // then to every other client.
                        if is_excluded_name(&name) {
                            continue;
                        }
                        let mtime = last_modified
                            .duration_since(UNIX_EPOCH)
                            .map(|d| d.as_secs() as i64)
                            .unwrap_or(0);
                        out.push(WalkEntry {
                            rel_path: new_rel,
                            mtime,
                        });
                    }
                    Entry::Dir { uri, .. } => {
                        stack.push((uri, new_rel));
                    }
                }
            }
        }

        Ok(out)
    }

    async fn walk_dirs(&self, root: &str) -> Result<Vec<String>, LocalFsError> {
        let (root_uri, rel_prefix) = Self::split(root);
        let api = self.app.android_fs_async();

        let start: FileUri = if rel_prefix.is_empty() {
            Self::tree_uri(&root_uri)
        } else {
            api.resolve_dir_uri(&Self::tree_uri(&root_uri), &rel_prefix)
                .await
                .map_err(|e| LocalFsError::other(format!("resolve_dir_uri({rel_prefix}): {e}")))?
        };

        let mut out = Vec::new();
        let mut stack: Vec<(FileUri, String)> = vec![(start, String::new())];

        while let Some((dir, prefix)) = stack.pop() {
            let entries = api
                .read_dir(&dir)
                .await
                .map_err(|e| LocalFsError::other(format!("read_dir: {e}")))?;

            for entry in entries {
                let name = entry.name().to_string();
                let new_rel = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}/{name}")
                };
                if let Entry::Dir { uri, .. } = entry {
                    out.push(new_rel.clone());
                    stack.push((uri, new_rel));
                }
            }
        }

        Ok(out)
    }

    async fn create_dir_all(&self, path: &str) -> Result<(), LocalFsError> {
        let (root, rel) = Self::split(path);
        if rel.is_empty() {
            // The tree root always exists (the user picked it).
            return Ok(());
        }
        self.listings.lock().unwrap().clear();
        self.app
            .android_fs_async()
            .create_dir_all(&Self::tree_uri(&root), &rel)
            .await
            .map(|_| ())
            .map_err(|e| LocalFsError::other(format!("create_dir_all({rel}): {e}")))
    }

    async fn read(&self, path: &str) -> Result<Vec<u8>, LocalFsError> {
        let (root, rel) = Self::split(path);
        let uri = self.resolve_file(&root, &rel).await?;
        self.app
            .android_fs_async()
            .read(&uri)
            .await
            .map_err(|e| LocalFsError::other(format!("read({rel}): {e}")))
    }

    async fn write(&self, path: &str, data: &[u8]) -> Result<(), LocalFsError> {
        let (root, rel) = Self::split(path);
        if rel.is_empty() {
            return Err(LocalFsError::other("write called on tree root"));
        }
        let api = self.app.android_fs_async();
        let tree = Self::tree_uri(&root);

        // Overwrite-or-create: try resolve first, otherwise create the file.
        let uri = match api.resolve_file_uri(&tree, &rel).await {
            Ok(u) => u,
            Err(_) => api
                .create_new_file(&tree, &rel, None)
                .await
                .map_err(|e| LocalFsError::other(format!("create_new_file({rel}): {e}")))?,
        };

        api.write(&uri, data)
            .await
            .map_err(|e| LocalFsError::other(format!("write({rel}): {e}")))
    }

    async fn read_stream(&self, path: &str) -> Result<ByteStream<'static>, LocalFsError> {
        let (root, rel) = Self::split(path);
        let uri = self.resolve_file(&root, &rel).await?;
        let file = self
            .app
            .android_fs_async()
            .open_file_readable(&uri)
            .await
            .map_err(|e| LocalFsError::other(format!("open({rel}): {e}")))?;
        Ok(file_chunks(file))
    }

    // SAF has no way to replace a document in one step, so the stream is
    // spooled to the app's cache directory first and only a complete file
    // is copied over the destination. A download that dies half-way never
    // touches what was there.
    async fn write_stream(&self, path: &str, chunks: ByteStream<'_>) -> Result<(), LocalFsError> {
        let (root, rel) = Self::split(path);
        if rel.is_empty() {
            return Err(LocalFsError::other("write called on tree root"));
        }
        let spool = self
            .app
            .path()
            .app_cache_dir()
            .map_err(|e| LocalFsError::other(format!("no cache directory: {e}")))?
            .join(format!(
                "uncloud-download-{}-{}",
                std::process::id(),
                SPOOL_SEQ.fetch_add(1, Ordering::Relaxed)
            ));
        let written = async {
            if let Some(dir) = spool.parent() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| LocalFsError::other(format!("create cache directory: {e}")))?;
            }
            let file = std::fs::File::create(&spool)
                .map_err(|e| LocalFsError::other(format!("create spool file: {e}")))?;
            write_chunks(file, chunks).await?;

            let api = self.app.android_fs_async();
            let tree = Self::tree_uri(&root);
            let uri = match api.resolve_file_uri(&tree, &rel).await {
                Ok(uri) => uri,
                Err(_) => api
                    .create_new_file(&tree, &rel, None)
                    .await
                    .map_err(|e| LocalFsError::other(format!("create_new_file({rel}): {e}")))?,
            };
            let mut dest = api
                .open_file_writable(&uri)
                .await
                .map_err(|e| LocalFsError::other(format!("open({rel}): {e}")))?;
            let mut source = std::fs::File::open(&spool)
                .map_err(|e| LocalFsError::other(format!("open spool file: {e}")))?;
            tauri::async_runtime::spawn_blocking(move || std::io::copy(&mut source, &mut dest))
                .await
                .map_err(|e| LocalFsError::other(format!("copy task failed: {e}")))?
                .map(|_| ())
                .map_err(|e| LocalFsError::other(format!("write({rel}): {e}")))
        }
        .await;
        let _ = std::fs::remove_file(&spool);
        written
    }

    async fn remove_file(&self, path: &str) -> Result<(), LocalFsError> {
        let (root, rel) = Self::split(path);
        if rel.is_empty() {
            return Ok(());
        }
        self.forget_listing_around(path);
        let api = self.app.android_fs_async();
        match api.resolve_file_uri(&Self::tree_uri(&root), &rel).await {
            Ok(uri) => api
                .remove_file(&uri)
                .await
                .map_err(|e| LocalFsError::other(format!("remove_file({rel}): {e}"))),
            // Idempotent: resolution error means the file is already gone.
            Err(_) => Ok(()),
        }
    }

    async fn remove_dir(&self, path: &str) -> Result<(), LocalFsError> {
        let (root, rel) = Self::split(path);
        if rel.is_empty() {
            return Err(LocalFsError::other("remove_dir called on tree root"));
        }
        self.listings.lock().unwrap().clear();
        let api = self.app.android_fs_async();
        let uri = match api.resolve_dir_uri(&Self::tree_uri(&root), &rel).await {
            Ok(u) => u,
            // Idempotent: dir is already gone.
            Err(_) => return Ok(()),
        };
        let entries = api
            .read_dir(&uri)
            .await
            .map_err(|e| LocalFsError::other(format!("read_dir({rel}): {e}")))?;
        if !entries.is_empty() {
            return Err(LocalFsError::other(format!(
                "remove_dir({rel}): directory not empty"
            )));
        }
        api.remove_dir_all(&uri)
            .await
            .map_err(|e| LocalFsError::other(format!("remove_dir({rel}): {e}")))
    }

    async fn mtime(&self, path: &str) -> Result<Option<i64>, LocalFsError> {
        let (root, rel) = Self::split(path);
        if rel.is_empty() {
            return Ok(None);
        }
        let api = self.app.android_fs_async();
        match api.resolve_file_uri(&Self::tree_uri(&root), &rel).await {
            Ok(uri) => match api.get_info(&uri).await {
                Ok(entry) => Ok(entry
                    .last_modified()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .ok()),
                Err(_) => Ok(None),
            },
            Err(_) => Ok(None),
        }
    }

    async fn is_file(&self, path: &str) -> Result<bool, LocalFsError> {
        let (root, rel) = Self::split(path);
        if rel.is_empty() {
            return Ok(false);
        }
        let api = self.app.android_fs_async();
        if let Ok(uri) = api.resolve_file_uri(&Self::tree_uri(&root), &rel).await {
            if let Ok(entry) = api.get_info(&uri).await {
                return Ok(matches!(entry, Entry::File { .. }));
            }
        }
        self.absent_or_failed(path, false).await
    }

    async fn is_dir(&self, path: &str) -> Result<bool, LocalFsError> {
        let (root, rel) = Self::split(path);
        // Empty rel = the SAF tree root itself, which always resolves to a
        // directory by construction.
        if rel.is_empty() {
            return Ok(true);
        }
        let api = self.app.android_fs_async();
        if let Ok(uri) = api.resolve_file_uri(&Self::tree_uri(&root), &rel).await {
            if let Ok(entry) = api.get_info(&uri).await {
                return Ok(matches!(entry, Entry::Dir { .. }));
            }
        }
        self.absent_or_failed(path, true).await
    }

    // Before a deletion is pushed, absence is established from fresh
    // listings of every directory on the way down from the tree root, never
    // from the per-run cache: a name missing from a listing that succeeded
    // is a positive observation, a listing that fails is an error.
    async fn confirm_absent(&self, path: &str) -> Result<bool, LocalFsError> {
        let (root, rel) = Self::split(path);
        if rel.is_empty() {
            return Ok(false);
        }
        let api = self.app.android_fs_async();
        let mut dir = Self::tree_uri(&root);
        let parts: Vec<String> = rel
            .split('/')
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect();
        let part_count = parts.len();
        for (index, part) in parts.into_iter().enumerate() {
            let entries = api.read_dir(&dir).await.map_err(|e| {
                LocalFsError::other(format!("read_dir while confirming {rel} is gone: {e}"))
            })?;
            let Some(entry) = entries.into_iter().find(|entry| entry.name() == part) else {
                return Ok(true);
            };
            if index + 1 == part_count {
                return Ok(false);
            }
            match entry {
                Entry::Dir { uri, .. } => dir = uri,
                Entry::File { .. } => return Ok(true),
            }
        }
        Ok(false)
    }

    fn join(&self, parent: &str, child: &str) -> String {
        if parent.contains(SEP) {
            format!("{parent}/{child}")
        } else {
            format!("{parent}{SEP}{child}")
        }
    }
}
