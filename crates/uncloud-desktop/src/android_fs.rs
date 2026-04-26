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
use uncloud_sync::{LocalFs, LocalFsError, WalkEntry};

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
            return Err(LocalFsError::other("resolve_file called with empty rel path"));
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
                        let mtime = last_modified
                            .duration_since(UNIX_EPOCH)
                            .map(|d| d.as_secs() as i64)
                            .unwrap_or(0);
                        out.push(WalkEntry { rel_path: new_rel, mtime });
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

    fn join(&self, parent: &str, child: &str) -> String {
        if parent.contains(SEP) {
            format!("{parent}/{child}")
        } else {
            format!("{parent}{SEP}{child}")
        }
    }
}
