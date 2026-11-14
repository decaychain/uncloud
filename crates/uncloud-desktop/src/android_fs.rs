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

use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use tauri::AppHandle;
use tauri_plugin_android_fs::{AndroidFsExt, Entry, FileUri};
use uncloud_sync::{LocalFs, LocalFsError, WalkEntry, EXCLUDED_NAMES};

/// `|` as delimiter is safe: SAF URIs percent-encode everything that isn't
/// in the unreserved/safe set, and `|` is not in either.
const SEP: char = '|';

pub struct AndroidSafFs {
    app: AppHandle,
}

impl AndroidSafFs {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
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
    async fn rename(&self, from: &str, to: &str) -> Result<(), LocalFsError> {
        let (root, rel) = Self::split(from);
        let (to_root, to_rel) = Self::split(to);
        let (parent, _) = rel.rsplit_once('/').unwrap_or(("", rel.as_str()));
        let (to_parent, name) = to_rel.rsplit_once('/').unwrap_or(("", to_rel.as_str()));
        if root != to_root || parent != to_parent || rel.is_empty() || to_rel.is_empty() {
            return Err(LocalFsError::other(
                "SAF cannot move entries between directories",
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
        api.rename(&uri, name)
            .await
            .map(|_| ())
            .map_err(|e| LocalFsError::other(format!("rename({rel}): {e}")))
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
                        if EXCLUDED_NAMES.iter().any(|&n| n == name) {
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

    async fn remove_file(&self, path: &str) -> Result<(), LocalFsError> {
        let (root, rel) = Self::split(path);
        if rel.is_empty() {
            return Ok(());
        }
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
        match api.resolve_file_uri(&Self::tree_uri(&root), &rel).await {
            Ok(uri) => match api.get_info(&uri).await {
                Ok(entry) => Ok(matches!(entry, Entry::File { .. })),
                Err(_) => Ok(false),
            },
            Err(_) => Ok(false),
        }
    }

    async fn is_dir(&self, path: &str) -> Result<bool, LocalFsError> {
        let (root, rel) = Self::split(path);
        // Empty rel = the SAF tree root itself, which always resolves to a
        // directory by construction.
        if rel.is_empty() {
            return Ok(true);
        }
        let api = self.app.android_fs_async();
        match api.resolve_file_uri(&Self::tree_uri(&root), &rel).await {
            Ok(uri) => match api.get_info(&uri).await {
                Ok(entry) => Ok(matches!(entry, Entry::Dir { .. })),
                Err(_) => Ok(false),
            },
            Err(_) => Ok(false),
        }
    }

    // SAF errors carry no kind, so `is_file` / `is_dir` cannot tell "not
    // there" from "the provider failed" and report both as absent. Before a
    // deletion is pushed, absence is established by listing every directory
    // on the way down from the tree root instead: a name missing from a
    // listing that succeeded is a positive observation, a listing that
    // fails is an error.
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
