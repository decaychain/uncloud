//! `backup restore` — in-place restore of a snapshot back into Uncloud.
//!
//! See `docs/backup.md` → "Restore" for the algorithm. Outline:
//!
//! 1. Acquire lock; refuse if a migration is in progress.
//! 2. Open the rustic repo and resolve the requested snapshot.
//! 3. Read `/uncloud/database/storages.jsonl` from the snapshot. Build a
//!    remap from the snapshot's `storages._id` to the destination's
//!    `storages._id` by matching on `name`. Unmatched storages fall back
//!    to `--default-storage` (or the destination's `is_default: true`).
//! 4. Print the remap plan; require `--yes` (or `--dry-run` to preview).
//! 5. Apply conflict policy:
//!    - `abort` (default): refuse if any restore-target collection has rows.
//!    - `overwrite` (with `--yes-i-know-this-is-destructive`):
//!       drop_collection before insert.
//! 6. Restore each allowlisted collection from `/uncloud/database/*.jsonl`,
//!    rewriting `storage_id` fields through the remap and skipping
//!    `storages` entirely.
//! 7. Restore blob bytes from `/uncloud/blobs/<file_id>` and (if present)
//!    `/uncloud/versions/<file_id>/<version_id>` straight into the
//!    matched destination backends.

use std::path::PathBuf;
use std::sync::Arc;

use bson::oid::ObjectId;
use bson::{doc, Bson, Document};
use futures::stream::TryStreamExt;
use mongodb::Database;
use rustic_core::repofile::SnapshotFile;
use rustic_core::{IndexedFullStatus, OpenStatus, Repository, TreeId};
use tokio::io::AsyncWriteExt;
use tokio::sync::Notify;

use crate::backup::config::BackupTarget;
use crate::backup::dump;
use crate::backup::lock;
use crate::backup::repo;
use crate::backup::{ConflictPolicy, RestoreArgs};
use crate::config::Config;
use crate::db;
use crate::models::Storage;
use crate::services::StorageService;
use crate::storage::StorageBackend;

/// Collections we restore. Mirrors `dump::COLLECTION_ALLOWLIST` minus
/// `storages` (skipped — destination's own row set is authoritative).
const RESTORE_COLLECTIONS: &[&str] = &[
    "users",
    "folders",
    "files",
    "file_versions",
    "shares",
    "folder_shares",
    "api_tokens",
    "s3_credentials",
    "sftp_host_keys",
    "apps",
    "webhooks",
    "sync_events",
    "invites",
    "user_preferences",
    "playlists",
    "shopping_lists",
    "shopping_items",
    "shopping_list_items",
    "shopping_categories",
    "shops",
    "task_projects",
    "task_sections",
    "tasks",
    "task_comments",
    "task_labels",
];

pub async fn run(args: RestoreArgs) -> Result<(), Box<dyn std::error::Error>> {
    crate::backup::init_logging();

    let config = Config::load_or_default();
    let target = config
        .backup
        .target(&args.target)
        .ok_or_else(|| format!("backup target {:?} is not configured", args.target))?
        .clone();

    if args.conflict_policy == ConflictPolicy::Overwrite && !args.yes_i_know_this_is_destructive {
        return Err(
            "--conflict-policy=overwrite requires --yes-i-know-this-is-destructive".into(),
        );
    }

    let password = target.password.resolve()?;
    let db = db::connect(&config.database).await?;
    db::setup_indexes(&db).await?;

    if args.force_unlock {
        let cleared = lock::force_unlock(&db).await?;
        if cleared > 0 {
            println!("Cleared {cleared} stale backup-lock row(s).");
        }
    }
    if let Err(msg) = crate::migrate::check_no_active_migration(&db).await {
        return Err(msg.into());
    }
    let lock_id = lock::acquire(&db, format!("restore:{}", target.name)).await?;
    let stop_heartbeat = Arc::new(Notify::new());
    let heartbeat = lock::spawn_heartbeat(db.clone(), lock_id, stop_heartbeat.clone());

    let result = run_inner(&config, &db, &target, &password, &args).await;

    stop_heartbeat.notify_waiters();
    let _ = heartbeat.await;
    lock::release(&db, lock_id).await?;
    result
}

async fn run_inner(
    config: &Config,
    db: &Database,
    target: &BackupTarget,
    password: &str,
    args: &RestoreArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("── Restore from {:?} ({}) ──────────────────────────", target.name, target.repo);

    // Open + index + resolve snapshot, all on a blocking thread.
    let target_clone = target.clone();
    let password_owned = password.to_string();
    let snap_id_arg = args.snapshot.clone();
    let opened = tokio::task::spawn_blocking(
        move || -> Result<(Repository<IndexedFullStatus>, SnapshotFile), String> {
            let repo: Repository<OpenStatus> =
                repo::open(&target_clone, &password_owned).map_err(|e| e.to_string())?;
            let snap = repo
                .get_snapshot_from_str(&snap_id_arg, |_| true)
                .map_err(|e| format!("could not resolve snapshot {:?}: {e}", snap_id_arg))?;
            let indexed = repo
                .to_indexed()
                .map_err(|e| format!("repository indexing failed: {e}"))?;
            Ok((indexed, snap))
        },
    )
    .await??;
    let (indexed_repo, snap) = opened;
    let indexed_repo = Arc::new(indexed_repo);
    println!("Resolved snapshot {} from {}", snap.id, snap.time);

    // Read the snapshot's `storages.jsonl` (used only for matching) and the
    // destination's storages collection. Build remap.
    let snap_tree = snap.tree;
    let snap_options = read_snapshot_options(&indexed_repo, snap_tree).await?;
    println!(
        "Snapshot was created with: include_versions={}, include_trash={}, include_thumbnails={}",
        snap_options.include_versions, snap_options.include_trash, snap_options.include_thumbnails
    );
    let snap_storages = read_collection_jsonl(&indexed_repo, snap_tree, "storages").await?;
    println!("Snapshot contains {} storage definition(s).", snap_storages.len());

    let dest_storages: Vec<Storage> = {
        let coll = db.collection::<Storage>("storages");
        let mut cur = coll.find(doc! {}).await?;
        let mut v = Vec::new();
        while let Some(s) = cur.try_next().await? { v.push(s); }
        v
    };
    let default_storage_id = pick_default_storage(&dest_storages, args.default_storage.as_deref())?;

    let remap = build_remap(&snap_storages, &dest_storages, default_storage_id, &args.target)?;
    print_remap_plan(&remap, &snap_storages, &dest_storages);

    if args.dry_run {
        println!("Dry run — no changes made. Re-run without --dry-run to proceed.");
        return Ok(());
    }
    if !args.yes {
        return Err("Restore requires `--yes` to confirm the storage remap plan.".into());
    }

    // Conflict policy.
    apply_conflict_policy(db, args.conflict_policy).await?;

    // Restore database. Each collection is its own jsonl file.
    let mut total_rows = 0usize;
    for &name in RESTORE_COLLECTIONS {
        let rows = restore_collection(db, &indexed_repo, snap_tree, name, &remap).await?;
        if rows > 0 {
            println!("  {name:<24} {rows} row(s)");
        }
        total_rows += rows;
    }
    println!("Database restore: {total_rows} row(s) total");

    // Restore blobs. Open the destination StorageService AFTER the DB is
    // restored so newly-inserted Files resolve to the correct backends.
    let storage_service = Arc::new(StorageService::new(db, &config.storage).await?);

    let blob_count = restore_all_blobs(
        db,
        &indexed_repo,
        snap_tree,
        &storage_service,
        &remap,
        &snap_options,
    )
    .await?;
    println!("Blob restore: {blob_count} blob(s) written");

    println!("Restore complete. Re-applied indexes will run on next `serve` startup.");
    Ok(())
}

// ── Storage remap ──────────────────────────────────────────────────────────

fn pick_default_storage(
    dest: &[Storage],
    explicit: Option<&str>,
) -> Result<ObjectId, Box<dyn std::error::Error>> {
    if let Some(name) = explicit {
        let s = dest
            .iter()
            .find(|s| s.name == name)
            .ok_or_else(|| format!("--default-storage {name:?} is not configured"))?;
        Ok(s.id)
    } else {
        let s = dest
            .iter()
            .find(|s| s.is_default)
            .ok_or_else(|| {
                "no default storage on destination — pass --default-storage <name>".to_string()
            })?;
        Ok(s.id)
    }
}

fn build_remap(
    snap_storages: &[Document],
    dest_storages: &[Storage],
    default_id: ObjectId,
    _target_name: &str,
) -> Result<std::collections::HashMap<ObjectId, ObjectId>, Box<dyn std::error::Error>> {
    let mut map = std::collections::HashMap::new();
    for s in snap_storages {
        let id = s
            .get_object_id("_id")
            .map_err(|e| format!("snapshot storage missing `_id`: {e}"))?;
        let name = s
            .get_str("name")
            .map_err(|e| format!("snapshot storage missing `name`: {e}"))?;
        let dest = dest_storages.iter().find(|d| d.name == name);
        let dest_id = dest.map(|d| d.id).unwrap_or(default_id);
        map.insert(id, dest_id);
    }
    Ok(map)
}

fn print_remap_plan(
    remap: &std::collections::HashMap<ObjectId, ObjectId>,
    snap_storages: &[Document],
    dest_storages: &[Storage],
) {
    println!("Storage mapping:");
    for s in snap_storages {
        let snap_id = match s.get_object_id("_id") {
            Ok(i) => i,
            Err(_) => continue,
        };
        let snap_name = s.get_str("name").unwrap_or("(unnamed)");
        let dest_id = remap.get(&snap_id).copied();
        let dest_name = dest_id
            .and_then(|did| dest_storages.iter().find(|d| d.id == did).map(|d| d.name.clone()))
            .unwrap_or_else(|| "(unmapped)".into());
        let matched = dest_storages.iter().any(|d| d.name == snap_name);
        let suffix = if matched { "matched" } else { "default fallback" };
        println!("  {snap_name:?}  →  {dest_name:?}  ({suffix})");
    }
}

// ── Conflict policy ────────────────────────────────────────────────────────

async fn apply_conflict_policy(
    db: &Database,
    policy: ConflictPolicy,
) -> Result<(), Box<dyn std::error::Error>> {
    match policy {
        ConflictPolicy::Abort => {
            for name in RESTORE_COLLECTIONS {
                let n = db
                    .collection::<bson::Document>(name)
                    .count_documents(doc! {})
                    .await?;
                if n > 0 {
                    return Err(format!(
                        "conflict-policy=abort: collection `{name}` already has {n} document(s). \
                         Either point at an empty database or use --conflict-policy=overwrite \
                         --yes-i-know-this-is-destructive."
                    )
                    .into());
                }
            }
        }
        ConflictPolicy::Overwrite => {
            for name in RESTORE_COLLECTIONS {
                let _ = db.collection::<bson::Document>(name).drop().await;
            }
        }
    }
    Ok(())
}

// ── Database restore ───────────────────────────────────────────────────────

async fn restore_collection(
    db: &Database,
    repo: &Arc<Repository<IndexedFullStatus>>,
    snap_tree: TreeId,
    name: &str,
    remap: &std::collections::HashMap<ObjectId, ObjectId>,
) -> Result<usize, Box<dyn std::error::Error>> {
    let docs = read_collection_jsonl(repo, snap_tree, name).await?;
    if docs.is_empty() {
        return Ok(0);
    }
    let mut prepared = Vec::with_capacity(docs.len());
    for mut doc in docs {
        if matches!(name, "files" | "folders") {
            remap_storage_id(&mut doc, remap);
        } else if name == "sftp_host_keys" {
            // Drop any host-key row whose source storage isn't represented
            // on the destination. The destination will TOFU-pin its own
            // keys on first use.
            let Some(sid) = doc.get_object_id("storage_id").ok() else { continue };
            if !remap.contains_key(&sid) { continue; }
            remap_storage_id(&mut doc, remap);
        }
        prepared.push(doc);
    }
    if prepared.is_empty() {
        return Ok(0);
    }
    db.collection::<Document>(name)
        .insert_many(&prepared)
        .await?;
    Ok(prepared.len())
}

fn remap_storage_id(doc: &mut Document, remap: &std::collections::HashMap<ObjectId, ObjectId>) {
    if let Ok(sid) = doc.get_object_id("storage_id") {
        if let Some(new) = remap.get(&sid).copied() {
            doc.insert("storage_id", Bson::ObjectId(new));
        }
    }
}

// ── Snapshot file readers ──────────────────────────────────────────────────

/// Read a `<name>.jsonl` file out of a snapshot's `/uncloud/database/`
/// subtree, returning one `Document` per line. Returns an empty Vec if the
/// file isn't in the snapshot (older / partial dumps).
async fn read_collection_jsonl(
    repo: &Arc<Repository<IndexedFullStatus>>,
    snap_tree: TreeId,
    name: &str,
) -> Result<Vec<Document>, Box<dyn std::error::Error>> {
    let path = PathBuf::from(format!("/uncloud/database/{name}.jsonl"));
    let bytes = match read_snapshot_file_bytes(repo, snap_tree, &path).await? {
        Some(b) => b,
        None => return Ok(Vec::new()),
    };
    let text = String::from_utf8(bytes)
        .map_err(|e| format!("collection `{name}` is not valid UTF-8: {e}"))?;
    let mut out = Vec::new();
    for (line_no, line) in text.lines().enumerate() {
        if line.trim().is_empty() { continue; }
        let value: serde_json::Value = serde_json::from_str(line).map_err(|e| {
            format!("collection `{name}` line {}: invalid JSON: {e}", line_no + 1)
        })?;
        let doc = dump::json_to_document(value)?;
        out.push(doc);
    }
    Ok(out)
}

async fn read_snapshot_file_bytes(
    repo: &Arc<Repository<IndexedFullStatus>>,
    snap_tree: TreeId,
    path: &std::path::Path,
) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error>> {
    let repo = repo.clone();
    let path = path.to_path_buf();
    let result = tokio::task::spawn_blocking(move || -> Result<Option<Vec<u8>>, String> {
        let node = match repo.node_from_path(snap_tree, &path) {
            Ok(n) => n,
            Err(_) => return Ok(None),
        };
        if !node.is_file() {
            return Ok(None);
        }
        let size = node.meta.size as usize;
        let f = repo
            .open_file(&node)
            .map_err(|e| format!("open_file({:?}): {e}", path.display()))?;
        let bytes = f
            .read_at(&*repo, 0, size)
            .map_err(|e| format!("read_at({:?}): {e}", path.display()))?;
        Ok(Some(bytes.to_vec()))
    })
    .await??;
    Ok(result)
}

// ── Blob restore ───────────────────────────────────────────────────────────

async fn restore_all_blobs(
    db: &Database,
    repo: &Arc<Repository<IndexedFullStatus>>,
    snap_tree: TreeId,
    storage_service: &Arc<StorageService>,
    _remap: &std::collections::HashMap<ObjectId, ObjectId>,
    snap_options: &SnapshotOptionsManifest,
) -> Result<usize, Box<dyn std::error::Error>> {
    use crate::models::{File, FileVersion};
    let mut count = 0usize;

    // Match the inclusion filter the snapshot was created with: if the
    // snapshot didn't include trashed files, don't try to restore blobs
    // for them — those blobs are intentionally not in the snapshot, and
    // the file's metadata alone is enough to round-trip the soft-deleted
    // state.
    let files_filter = if snap_options.include_trash {
        doc! {}
    } else {
        doc! { "deleted_at": mongodb::bson::Bson::Null }
    };
    let mut cur = db.collection::<File>("files").find(files_filter).await?;
    while let Some(f) = cur.try_next().await? {
        let backend = match storage_service.get_backend(f.storage_id).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("skipping blob for file {}: {e}", f.id);
                continue;
            }
        };
        let snap_path = PathBuf::from(format!("/uncloud/blobs/{}", f.id.to_hex()));
        let target_path = match (&f.deleted_at, &f.trash_path) {
            (Some(_), Some(t)) => t.clone(),
            _ => f.storage_path.clone(),
        };
        if restore_blob(repo, snap_tree, &snap_path, &backend, &target_path).await? {
            count += 1;
        } else {
            // Inside the inclusion filter, a missing blob is a real
            // anomaly — surface it. Outside the filter (trashed-and-
            // skipped), we wouldn't even iterate.
            tracing::warn!("snapshot has no blob for file {} — skipped", f.id);
        }
    }

    if snap_options.include_versions {
        let files_coll = db.collection::<File>("files");
        let mut cur = db
            .collection::<FileVersion>("file_versions")
            .find(doc! {})
            .await?;
        while let Some(v) = cur.try_next().await? {
            let Some(parent) = files_coll.find_one(doc! { "_id": v.file_id }).await? else {
                continue;
            };
            let backend = match storage_service.get_backend(parent.storage_id).await {
                Ok(b) => b,
                Err(_) => continue,
            };
            let snap_path = PathBuf::from(format!(
                "/uncloud/versions/{}/{}",
                v.file_id.to_hex(),
                v.id.to_hex()
            ));
            if restore_blob(repo, snap_tree, &snap_path, &backend, &v.storage_path).await? {
                count += 1;
            }
        }
    }

    Ok(count)
}

/// Manifest options the restore engine cares about. Anything else in
/// `manifest.json` is for diagnostics, not control flow.
#[derive(Debug, Clone, Default)]
pub struct SnapshotOptionsManifest {
    pub include_versions: bool,
    pub include_trash: bool,
    pub include_thumbnails: bool,
}

/// Read `/uncloud/manifest.json` from the snapshot. Falls back to defaults
/// if the manifest is missing or doesn't have an `options` block (older
/// snapshots from before the manifest carried these fields).
async fn read_snapshot_options(
    repo: &Arc<Repository<IndexedFullStatus>>,
    snap_tree: TreeId,
) -> Result<SnapshotOptionsManifest, Box<dyn std::error::Error>> {
    let path = PathBuf::from("/uncloud/manifest.json");
    let bytes = match read_snapshot_file_bytes(repo, snap_tree, &path).await? {
        Some(b) => b,
        None => return Ok(default_pre_options_manifest()),
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    let opts = value.get("options").cloned().unwrap_or(serde_json::Value::Null);
    if opts.is_null() {
        return Ok(default_pre_options_manifest());
    }
    Ok(SnapshotOptionsManifest {
        include_versions: opts
            .get("include_versions")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        include_trash: opts
            .get("include_trash")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        include_thumbnails: opts
            .get("include_thumbnails")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    })
}

/// Defaults that mirror `BackupOptions::default()` — used when a snapshot
/// pre-dates the manifest carrying its `options` field.
fn default_pre_options_manifest() -> SnapshotOptionsManifest {
    SnapshotOptionsManifest {
        include_versions: true,
        include_trash: false,
        include_thumbnails: false,
    }
}

/// Stream a single blob from the snapshot into the destination backend.
/// Returns `false` if the blob isn't present in the snapshot (allowed —
/// callers warn and continue), `true` if it was written.
async fn restore_blob(
    repo: &Arc<Repository<IndexedFullStatus>>,
    snap_tree: TreeId,
    snap_path: &std::path::Path,
    backend: &Arc<dyn StorageBackend>,
    target_path: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    let bytes = match read_snapshot_file_bytes(repo, snap_tree, snap_path).await? {
        Some(b) => b,
        None => return Ok(false),
    };
    // Stage to a tempfile so write_stream gets a real `AsyncRead` of known
    // size. Avoids holding the whole blob in memory inside the runtime.
    let tmp = tempfile::NamedTempFile::new()?;
    let path = tmp.path().to_path_buf();
    let len = bytes.len() as u64;
    let mut f = tokio::fs::File::create(&path).await?;
    f.write_all(&bytes).await?;
    f.flush().await?;
    drop(f);
    let f = tokio::fs::File::open(&path).await?;
    let reader: crate::storage::BoxedAsyncRead = Box::pin(f);
    backend.write_stream(target_path, reader, len).await?;
    Ok(true)
}
