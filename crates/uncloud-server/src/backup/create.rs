//! `backup create` end-to-end pipeline.
//!
//! Steps:
//! 1. Acquire the cross-feature backup lock; refuse if a migration holds the
//!    other lock.
//! 2. For each target:
//!    a. Resolve password + credentials.
//!    b. Open the rustic repository.
//!    c. Stage the semantic DB dump to a per-target tempdir
//!       (`<staging>/database/*.jsonl` + `database/manifest.json`).
//!    d. Walk the `files` collection (and optionally `file_versions`,
//!       `trash`), building `FileEntry`s that point at each blob's
//!       storage backend.
//!    e. Compute a top-level snapshot manifest.
//!    f. Hand the assembled `UncloudSource` to `repo.backup_with_source`
//!       inside `tokio::task::spawn_blocking`.
//! 3. Release the lock and report.

use std::path::PathBuf;
use std::sync::Arc;

use bson::doc;
use chrono::Utc;
use futures::stream::TryStreamExt;
use mongodb::Database;
use rustic_core::{BackupOptions, SnapshotOptions};
use tokio::sync::Notify;

use crate::backup::config::{BackupConfig, BackupTarget};
use crate::backup::dump;
use crate::backup::lock;
use crate::backup::repo;
use crate::backup::source::{FileEntry, StaticEntry, UncloudSource};
use crate::backup::CreateArgs;
use crate::config::Config;
use crate::db;
use crate::models::{File, FileVersion};
use crate::services::StorageService;

/// Top-level entry point. Resolves which targets to back up to and runs
/// each one in turn under a single backup-lock. Sequential by design —
/// concurrent runs against the same DB cursor would just contend on disk
/// and DB read I/O.
pub async fn run(args: CreateArgs) -> Result<(), Box<dyn std::error::Error>> {
    crate::backup::init_logging();

    let config = Config::load_or_default();
    if config.backup.targets.is_empty() {
        return Err(
            "No backup targets configured. Add at least one entry under `backup.targets:` in config.yaml."
                .into(),
        );
    }

    let targets = pick_targets(&config.backup, args.target.as_deref())?;
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

    let mut had_failure = false;
    for target in targets {
        if let Err(e) = run_one(&config, &db, &target, &args).await {
            eprintln!("Target {:?} failed: {e}", target.name);
            had_failure = true;
        }
    }

    if had_failure {
        Err("One or more backup targets failed; see logs above.".into())
    } else {
        Ok(())
    }
}

fn pick_targets(
    cfg: &BackupConfig,
    explicit: Option<&str>,
) -> Result<Vec<BackupTarget>, Box<dyn std::error::Error>> {
    if let Some(name) = explicit {
        let target = cfg
            .target(name)
            .ok_or_else(|| format!("backup target {name:?} is not configured"))?
            .clone();
        Ok(vec![target])
    } else {
        Ok(cfg.targets.clone())
    }
}

async fn run_one(
    config: &Config,
    db: &Database,
    target: &BackupTarget,
    args: &CreateArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "── Target {:?} ({}) ──────────────────────────",
        target.name, target.repo
    );
    let password = target.password.resolve()?;
    if target.password.is_inline() {
        tracing::warn!(
            "target {:?}: password configured inline in config.yaml — prefer password_file / password_env",
            target.name
        );
    }

    // Acquire lock + heartbeat *before* we open the repo so a crash
    // during open still leaves the lock-row for the next run to clear.
    let operation = format!("create:{}", target.name);
    let lock_id = lock::acquire(db, operation).await?;
    let stop_heartbeat = Arc::new(Notify::new());
    let heartbeat = lock::spawn_heartbeat(db.clone(), lock_id, stop_heartbeat.clone());

    let result = run_one_inner(config, db, target, &password, args).await;

    stop_heartbeat.notify_waiters();
    let _ = heartbeat.await;
    lock::release(db, lock_id).await?;

    result
}

async fn run_one_inner(
    config: &Config,
    db: &Database,
    target: &BackupTarget,
    password: &str,
    args: &CreateArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let target_for_open = target.clone();
    let password_for_open = password.to_string();
    let repo_handle =
        tokio::task::spawn_blocking(move || repo::open(&target_for_open, &password_for_open))
            .await??;

    // Per-run staging dir. The DB dump and run-manifest go here. Cleaned up
    // on drop (or if the process crashes — `tempfile::TempDir::into_path()`
    // would leak, we do not call it).
    let staging_root = config.backup.options.staging_dir.clone();
    let staging = build_staging(staging_root.as_ref()).await?;
    println!("Staging directory: {}", staging.path().display());

    // Step 1 — semantic DB dump. Skip for dry-run; we still want to know
    // counts though, so we'd ideally gather them lazily. v1 keeps this
    // simple: dry-run dumps to /dev/null-equivalent isn't worth the
    // complexity — just report file/version counts and bail.
    if args.dry_run {
        let (files, versions) = count_work(db).await?;
        println!("Dry run — would back up:");
        println!("  Files:        {files}");
        println!("  Versions:     {versions}");
        println!("  Collections:  {}", dump::COLLECTION_ALLOWLIST.len());
        return Ok(());
    }

    let database_dir = staging.path().join("database");
    let collection_counts = dump::dump_all(db, &database_dir).await?;
    println!(
        "Database dumped: {} collection(s), {} row(s) total",
        collection_counts.len(),
        collection_counts.iter().map(|(_, n)| n).sum::<usize>()
    );

    // Step 2 — enumerate file blobs (and versions, if enabled).
    let storage_service = StorageService::new(db, &config.storage).await?;
    let storage_service = Arc::new(storage_service);

    let files = collect_file_entries(db, &storage_service, &config.backup).await?;
    let versions = if config.backup.options.include_versions {
        collect_version_entries(db, &storage_service).await?
    } else {
        Vec::new()
    };
    println!(
        "Blob enumeration: {} file(s), {} version(s)",
        files.len(),
        versions.len()
    );

    // Step 3 — top-level run manifest. Stats reflect what the run plans to
    // back up, not what was actually committed (we won't know until rustic
    // finishes), but the schema_version + paths are stable and that's the
    // load-bearing part.
    write_run_manifest(staging.path(), target, &collection_counts, files.len(), versions.len())
        .await?;

    // Step 4 — gather all on-disk static entries (DB jsonls, manifests).
    let statics = enumerate_static_entries(staging.path()).await?;

    // Step 5 — hand to rustic on a blocking thread.
    let handle = tokio::runtime::Handle::current();
    let mut all_files = files;
    all_files.extend(versions);
    let total_blobs = all_files.len();
    let source = UncloudSource::new(handle, statics, all_files);
    let failure_counter = source.failures();

    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown".to_string());

    let mut snap_opts = SnapshotOptions::default().host(host);
    snap_opts = snap_opts
        .add_tags("app:uncloud")?
        .add_tags(&format!("uncloud:{}", env!("CARGO_PKG_VERSION")))?;
    if let Some(custom) = &args.tag {
        snap_opts = snap_opts.add_tags(custom)?;
    }
    let snap = snap_opts
        .to_snapshot()
        .map_err(|e| format!("failed to construct snapshot template: {e}"))?;
    let backup_opts = BackupOptions::default();
    let snapshot_path = PathBuf::from("/uncloud");

    let snap = tokio::task::spawn_blocking(move || {
        repo_handle
            .to_indexed_ids()?
            .backup_with_source(&backup_opts, &source, &snapshot_path, snap)
    })
    .await?
    .map_err(|e| format!("rustic backup failed: {e}"))?;

    let failures = failure_counter.load(std::sync::atomic::Ordering::Relaxed);
    println!(
        "Snapshot {} written: {} files, {} bytes",
        snap.id,
        snap.summary
            .as_ref()
            .map(|s| s.total_files_processed)
            .unwrap_or(0),
        snap.summary
            .as_ref()
            .map(|s| s.total_bytes_processed)
            .unwrap_or(0)
    );
    if failures > 0 {
        println!();
        println!(
            "WARNING: {failures} of {total_blobs} blob(s) failed to read from their \
             storage backend. Their content is NOT in this snapshot — the snapshot is \
             partial. Most common cause: file_versions documents whose archive blob is \
             no longer on the matching storage (e.g. left over from a previous \
             migration that didn't copy version blobs)."
        );
        println!(
            "Per-blob errors are logged at WARN level above; grep the run output for \
             `Failed to open backup blob`."
        );
    }

    Ok(())
}

async fn collect_file_entries(
    db: &Database,
    storage: &Arc<StorageService>,
    backup_cfg: &BackupConfig,
) -> Result<Vec<FileEntry>, Box<dyn std::error::Error>> {
    let coll = db.collection::<File>("files");
    let filter = if backup_cfg.options.include_trash {
        doc! {}
    } else {
        doc! { "deleted_at": mongodb::bson::Bson::Null }
    };
    let mut cursor = coll.find(filter).await?;
    let mut out = Vec::new();
    while let Some(f) = cursor.try_next().await? {
        let backend = match storage.get_backend(f.storage_id).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(
                    "skipping file {}: backend {} unavailable: {e}",
                    f.id,
                    f.storage_id
                );
                continue;
            }
        };
        let storage_path = match (&f.deleted_at, &f.trash_path) {
            (Some(_), Some(t)) => t.clone(),
            _ => f.storage_path.clone(),
        };
        let snapshot_path = PathBuf::from(format!("/uncloud/blobs/{}", f.id.to_hex()));
        out.push(FileEntry {
            backend,
            storage_path,
            snapshot_path,
            size: f.size_bytes.max(0) as u64,
        });
    }
    Ok(out)
}

async fn collect_version_entries(
    db: &Database,
    storage: &Arc<StorageService>,
) -> Result<Vec<FileEntry>, Box<dyn std::error::Error>> {
    let files = db.collection::<File>("files");
    let versions = db.collection::<FileVersion>("file_versions");
    let mut cursor = versions.find(doc! {}).await?;
    let mut out = Vec::new();
    while let Some(v) = cursor.try_next().await? {
        // Look up the parent File to find the current storage backend.
        let Some(parent) = files.find_one(doc! { "_id": v.file_id }).await? else {
            tracing::warn!(
                "skipping version {} of file {}: parent file document not found",
                v.id,
                v.file_id
            );
            continue;
        };
        let backend = match storage.get_backend(parent.storage_id).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(
                    "skipping version {} of file {}: backend unavailable: {e}",
                    v.id,
                    v.file_id
                );
                continue;
            }
        };
        let snapshot_path = PathBuf::from(format!(
            "/uncloud/versions/{}/{}",
            v.file_id.to_hex(),
            v.id.to_hex()
        ));
        out.push(FileEntry {
            backend,
            storage_path: v.storage_path,
            snapshot_path,
            size: v.size_bytes.max(0) as u64,
        });
    }
    Ok(out)
}

async fn enumerate_static_entries(
    root: &std::path::Path,
) -> Result<Vec<StaticEntry>, Box<dyn std::error::Error>> {
    let mut out = Vec::new();
    push_dir(root, root, &mut out).await?;
    Ok(out)
}

async fn push_dir(
    root: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<StaticEntry>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(e) = entries.next_entry().await? {
        let path = e.path();
        let ft = e.file_type().await?;
        if ft.is_dir() {
            Box::pin(push_dir(root, &path, out)).await?;
        } else if ft.is_file() {
            let meta = e.metadata().await?;
            let rel = path
                .strip_prefix(root)
                .map_err(|e| format!("strip_prefix on staging file: {e}"))?;
            let snapshot_path = PathBuf::from("/uncloud").join(rel);
            out.push(StaticEntry {
                local_path: path,
                snapshot_path,
                size: meta.len(),
            });
        }
    }
    Ok(())
}

async fn count_work(db: &Database) -> Result<(u64, u64), Box<dyn std::error::Error>> {
    let files: u64 = db
        .collection::<bson::Document>("files")
        .count_documents(doc! {})
        .await?;
    let versions: u64 = db
        .collection::<bson::Document>("file_versions")
        .count_documents(doc! {})
        .await?;
    Ok((files, versions))
}

async fn build_staging(
    configured: Option<&PathBuf>,
) -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("uncloud-backup-");
    let dir = match configured {
        Some(parent) => {
            tokio::fs::create_dir_all(parent).await?;
            builder.tempdir_in(parent)?
        }
        None => builder.tempdir()?,
    };
    Ok(dir)
}

async fn write_run_manifest(
    staging: &std::path::Path,
    target: &BackupTarget,
    counts: &[(String, usize)],
    files: usize,
    versions: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let collections: Vec<serde_json::Value> = counts
        .iter()
        .map(|(name, rows)| serde_json::json!({ "name": name, "rows": rows }))
        .collect();
    let body = serde_json::json!({
        "schema_version": dump::SCHEMA_VERSION,
        "uncloud_version": env!("CARGO_PKG_VERSION"),
        "target": target.name,
        "started_at": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "stats": {
            "files": files,
            "versions": versions,
            "db_rows": counts.iter().map(|(_, n)| n).sum::<usize>(),
        },
        "collections": collections,
    });
    let path = staging.join("manifest.json");
    let bytes = serde_json::to_vec_pretty(&body)?;
    tokio::fs::write(&path, bytes).await?;
    Ok(())
}

