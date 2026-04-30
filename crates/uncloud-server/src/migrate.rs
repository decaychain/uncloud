//! Offline storage migration. Implements the `uncloud-server migrate` CLI
//! subcommand: copy every file blob owned by one storage backend to another,
//! atomically flip the `File.storage_id` pointer, then move on. The server
//! must be stopped while a migration runs — `setup_indexes` installs a
//! singleton-by-scope unique index on `migration_locks`, and both `serve` and
//! `migrate` refuse to start when a row is present.
//!
//! Algorithm (per-file):
//!   1. Skip if `file.storage_id == to` (idempotent on rerun after crash).
//!   2. Stream source blob into the destination at the same path.
//!   3. Verify (`size` by default; `hash` recomputes SHA-256 of the dest blob
//!      and compares against `file.checksum_sha256`).
//!   4. Copy `.thumbs/{file_id}.jpg` if it exists on the source.
//!   5. Atomic Mongo update: `{ _id, storage_id: from } → { storage_id: to }`.
//!
//! See `docs/storage-migration.md` for the full design.

use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use futures::TryStreamExt;
use mongodb::bson::{self, doc, oid::ObjectId};
use mongodb::Database;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::config::Config;
use crate::db;
use crate::models::{File, Folder, MigrationLock, Storage};
use crate::services::StorageService;
use crate::storage::StorageBackend;

/// Maximum age of `last_heartbeat` before we treat a lock row as stale.
const STALE_AFTER: chrono::Duration = chrono::Duration::minutes(5);

/// How often the heartbeat task refreshes `last_heartbeat`.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyMode {
    None,
    Size,
    Hash,
}

impl FromStr for VerifyMode {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "none" => Ok(VerifyMode::None),
            "size" => Ok(VerifyMode::Size),
            "hash" => Ok(VerifyMode::Hash),
            other => Err(format!("unknown verify mode {other:?}; expected none|size|hash")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MigrateArgs {
    pub from: String,
    pub to: String,
    pub folder: Option<String>,
    pub dry_run: bool,
    pub verify: VerifyMode,
    pub force_unlock: bool,
}

/// Check on server startup whether a migration is in progress. Returns an
/// error if a non-stale lock is present so the caller can refuse to start.
pub async fn check_no_active_migration(db: &Database) -> Result<(), String> {
    let coll = db.collection::<MigrationLock>("migration_locks");
    let lock = coll
        .find_one(doc! { "scope": MigrationLock::SCOPE })
        .await
        .map_err(|e| format!("failed to query migration_locks: {e}"))?;
    let Some(lock) = lock else { return Ok(()) };

    let age = Utc::now() - lock.last_heartbeat;
    if age < STALE_AFTER {
        return Err(format!(
            "a storage migration is in progress\n  from: {}\n  to:   {}\n  started: {}\n  pid: {}@{}\n\n\
             Wait for it to finish, or run `uncloud-server migrate --force-unlock` to clear the lock if the previous run crashed.",
            lock.from_storage_id, lock.to_storage_id, lock.started_at, lock.pid, lock.hostname,
        ));
    }
    // Stale row — refuse but with a clearer hint than "migration in progress".
    Err(format!(
        "found stale migration lock (last heartbeat {} ago, started by pid {}@{}). Run `uncloud-server migrate --force-unlock` to clear it.",
        format_age(age), lock.pid, lock.hostname,
    ))
}

fn format_age(d: chrono::Duration) -> String {
    let secs = d.num_seconds();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

pub async fn run(args: MigrateArgs) -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "info".into()))
        .try_init()
        .ok();

    let config = Config::load_or_default();
    let db = db::connect(&config.database).await?;
    db::setup_indexes(&db).await?;

    if args.force_unlock {
        let res = db
            .collection::<MigrationLock>("migration_locks")
            .delete_one(doc! { "scope": MigrationLock::SCOPE })
            .await?;
        if res.deleted_count > 0 {
            println!("Cleared {} stale lock row(s).", res.deleted_count);
        } else {
            println!("No lock to clear.");
        }
    }

    let storage_service = StorageService::new(&db, &config.storage).await?;
    let from_id = resolve_storage_id(&storage_service, &db, &args.from).await?;
    let to_id = resolve_storage_id(&storage_service, &db, &args.to).await?;
    if from_id == to_id {
        return Err("source and destination storages are the same".into());
    }

    let from_backend = storage_service.get_backend(from_id).await?;
    let to_backend = storage_service.get_backend(to_id).await?;

    let folder_filter = match &args.folder {
        Some(id_str) => {
            let folder_id = ObjectId::parse_str(id_str)
                .map_err(|_| format!("--folder must be a valid ObjectId, got {id_str:?}"))?;
            let descendants = collect_descendant_folder_ids(&db, folder_id).await?;
            println!(
                "Restricting to folder {} ({} descendant folders).",
                folder_id,
                descendants.len() - 1,
            );
            Some(descendants)
        }
        None => None,
    };

    let candidates = enumerate_candidates(&db, from_id, folder_filter.as_ref()).await?;
    let total_files = candidates.len();
    let total_bytes: i64 = candidates.iter().map(|f| f.size_bytes).sum();
    println!(
        "Migrating {} → {}\n  Files: {}\n  Bytes: {} ({})",
        args.from,
        args.to,
        total_files,
        total_bytes,
        humanize_bytes(total_bytes.max(0) as u64),
    );

    if args.dry_run {
        println!("Dry run — no data will be copied. Re-run without --dry-run to proceed.");
        return Ok(());
    }
    if total_files == 0 {
        println!("Nothing to do.");
        return Ok(());
    }

    let lock_id = acquire_lock(&db, from_id, to_id).await?;
    let stop_heartbeat = Arc::new(Notify::new());
    let heartbeat_handle = spawn_heartbeat(db.clone(), lock_id, stop_heartbeat.clone());

    let result = run_migration(
        &db,
        from_backend,
        to_backend,
        from_id,
        to_id,
        &candidates,
        args.verify,
    )
    .await;

    stop_heartbeat.notify_waiters();
    let _ = heartbeat_handle.await;
    release_lock(&db, lock_id).await?;

    result
}

/// Core migration loop — exposed for integration tests so they can drive the
/// per-file copy + flip without going through config loading or lock acquisition.
pub async fn run_migration(
    db: &Database,
    from: Arc<dyn StorageBackend>,
    to: Arc<dyn StorageBackend>,
    from_id: ObjectId,
    to_id: ObjectId,
    files: &[File],
    verify: VerifyMode,
) -> Result<(), Box<dyn std::error::Error>> {
    let files_coll = db.collection::<File>("files");

    let mut copied_files = 0u64;
    let mut copied_bytes = 0u64;
    let mut skipped = 0u64;
    let mut failed: Vec<(ObjectId, String)> = Vec::new();
    let started = std::time::Instant::now();

    for (idx, file) in files.iter().enumerate() {
        let progress_prefix = format!("[{}/{}]", idx + 1, files.len());
        if file.storage_id == to_id {
            skipped += 1;
            continue;
        }

        match migrate_one(&from, &to, file, verify).await {
            Ok(()) => {}
            Err(e) => {
                eprintln!("{progress_prefix} FAILED {} ({}): {}", file.id, file.name, e);
                failed.push((file.id, e));
                continue;
            }
        }

        // Atomic pointer flip. The `storage_id: from_id` predicate guards
        // against the (impossible-while-locked) case of a concurrent edit.
        let update = files_coll
            .update_one(
                doc! { "_id": file.id, "storage_id": from_id },
                doc! { "$set": { "storage_id": to_id } },
            )
            .await?;
        if update.matched_count == 0 {
            eprintln!(
                "{progress_prefix} pointer flip skipped (file gone or already moved): {}",
                file.id
            );
            continue;
        }

        copied_files += 1;
        copied_bytes += file.size_bytes.max(0) as u64;
        if idx % 10 == 0 || idx + 1 == files.len() {
            let elapsed = started.elapsed().as_secs_f64().max(0.001);
            println!(
                "{progress_prefix} {} ({}) — {:.1} MiB/s",
                file.name,
                humanize_bytes(file.size_bytes.max(0) as u64),
                copied_bytes as f64 / elapsed / 1_048_576.0,
            );
        }
    }

    println!();
    println!(
        "Done. Copied {} file(s) ({}); skipped {} already on dest; {} failure(s).",
        copied_files,
        humanize_bytes(copied_bytes),
        skipped,
        failed.len(),
    );
    if !failed.is_empty() {
        eprintln!("Failures:");
        for (id, err) in &failed {
            eprintln!("  {id}: {err}");
        }
        return Err(format!("{} file(s) failed to migrate", failed.len()).into());
    }
    Ok(())
}

async fn migrate_one(
    from: &Arc<dyn StorageBackend>,
    to: &Arc<dyn StorageBackend>,
    file: &File,
    verify: VerifyMode,
) -> std::result::Result<(), String> {
    let path = if let Some(trash) = &file.trash_path {
        trash.clone()
    } else {
        file.storage_path.clone()
    };

    // Copy main blob.
    copy_blob(from, to, &path, file.size_bytes.max(0) as u64).await?;

    // Verify.
    match verify {
        VerifyMode::None => {}
        VerifyMode::Size => {
            verify_size(to, &path, file.size_bytes.max(0) as u64).await?;
        }
        VerifyMode::Hash => {
            verify_size(to, &path, file.size_bytes.max(0) as u64).await?;
            verify_hash(to, &path, &file.checksum_sha256).await?;
        }
    }

    // Copy thumbnail sidecar if present on source. Best-effort: a missing or
    // unreadable thumb is not fatal — the processing pipeline rebuilds on demand.
    let thumb_path = format!(".thumbs/{}.jpg", file.id);
    if from.exists(&thumb_path).await.unwrap_or(false) {
        if let Err(e) = copy_blob_unknown_size(from, to, &thumb_path).await {
            tracing::warn!(
                "Failed to migrate thumbnail for {}: {} — will be rebuilt on demand",
                file.id,
                e
            );
        }
    }

    Ok(())
}

async fn copy_blob(
    from: &Arc<dyn StorageBackend>,
    to: &Arc<dyn StorageBackend>,
    path: &str,
    size: u64,
) -> std::result::Result<(), String> {
    let reader = from
        .read(path)
        .await
        .map_err(|e| format!("read source: {e}"))?;
    to.write_stream(path, reader, size)
        .await
        .map_err(|e| format!("write dest: {e}"))?;
    Ok(())
}

/// Like `copy_blob` but used for sidecars where we don't have the size up
/// front. `write_stream`'s `size` parameter is advisory for backends like S3
/// that prefer it — passing 0 is acceptable for local/SFTP and falls back to
/// chunked upload on S3.
async fn copy_blob_unknown_size(
    from: &Arc<dyn StorageBackend>,
    to: &Arc<dyn StorageBackend>,
    path: &str,
) -> std::result::Result<(), String> {
    // Read into memory — sidecars are small (thumbnails are JPEGs at ~10–50 KiB).
    let mut reader = from
        .read(path)
        .await
        .map_err(|e| format!("read source: {e}"))?;
    let mut buf = Vec::new();
    reader
        .read_to_end(&mut buf)
        .await
        .map_err(|e| format!("read source body: {e}"))?;
    to.write(path, &buf)
        .await
        .map_err(|e| format!("write dest: {e}"))?;
    Ok(())
}

async fn verify_size(
    backend: &Arc<dyn StorageBackend>,
    path: &str,
    expected: u64,
) -> std::result::Result<(), String> {
    if !backend.exists(path).await.map_err(|e| format!("verify exists: {e}"))? {
        return Err("dest blob missing after write".into());
    }
    // No size accessor on the trait — rely on a streaming read. This adds a
    // round-trip to S3/SFTP, but is the only portable option and is far cheaper
    // than re-hashing.
    let mut reader = backend
        .read(path)
        .await
        .map_err(|e| format!("verify read: {e}"))?;
    let mut total: u64 = 0;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).await.map_err(|e| format!("verify read: {e}"))?;
        if n == 0 {
            break;
        }
        total += n as u64;
    }
    if total != expected {
        return Err(format!("size mismatch: expected {expected}, got {total}"));
    }
    Ok(())
}

async fn verify_hash(
    backend: &Arc<dyn StorageBackend>,
    path: &str,
    expected_hex: &str,
) -> std::result::Result<(), String> {
    let mut reader = backend
        .read(path)
        .await
        .map_err(|e| format!("verify read: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .await
            .map_err(|e| format!("verify read: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let got = hex::encode(hasher.finalize());
    if !got.eq_ignore_ascii_case(expected_hex) {
        return Err(format!("hash mismatch: expected {expected_hex}, got {got}"));
    }
    Ok(())
}

async fn resolve_storage_id(
    service: &StorageService,
    db: &Database,
    spec: &str,
) -> Result<ObjectId, Box<dyn std::error::Error>> {
    if let Ok(oid) = ObjectId::parse_str(spec) {
        // Make sure it actually exists.
        let coll = db.collection::<Storage>("storages");
        if coll.find_one(doc! { "_id": oid }).await?.is_some() {
            return Ok(oid);
        }
    }
    if let Some(oid) = service.storage_id_by_name(spec).await {
        return Ok(oid);
    }
    Err(format!("no storage matches {spec:?} (tried as ObjectId and as name)").into())
}

async fn collect_descendant_folder_ids(
    db: &Database,
    root: ObjectId,
) -> Result<HashSet<ObjectId>, Box<dyn std::error::Error>> {
    let folders = db.collection::<Folder>("folders");
    let mut out: HashSet<ObjectId> = HashSet::new();
    out.insert(root);
    let mut frontier = vec![root];
    while !frontier.is_empty() {
        let mut cursor = folders
            .find(doc! { "parent_id": { "$in": frontier.clone() } })
            .await?;
        frontier.clear();
        while let Some(folder) = cursor.try_next().await? {
            if out.insert(folder.id) {
                frontier.push(folder.id);
            }
        }
    }
    Ok(out)
}

async fn enumerate_candidates(
    db: &Database,
    from_id: ObjectId,
    folder_filter: Option<&HashSet<ObjectId>>,
) -> Result<Vec<File>, Box<dyn std::error::Error>> {
    let files = db.collection::<File>("files");
    let mut filter = doc! { "storage_id": from_id };
    if let Some(ids) = folder_filter {
        let arr: Vec<ObjectId> = ids.iter().copied().collect();
        filter.insert("parent_id", doc! { "$in": arr });
    }
    let mut cursor = files.find(filter).await?;
    let mut out = Vec::new();
    while let Some(f) = cursor.try_next().await? {
        out.push(f);
    }
    Ok(out)
}

async fn acquire_lock(
    db: &Database,
    from_id: ObjectId,
    to_id: ObjectId,
) -> Result<ObjectId, Box<dyn std::error::Error>> {
    let coll = db.collection::<MigrationLock>("migration_locks");
    let now = Utc::now();
    let lock = MigrationLock {
        id: ObjectId::new(),
        scope: MigrationLock::SCOPE.to_string(),
        from_storage_id: from_id,
        to_storage_id: to_id,
        started_at: now,
        last_heartbeat: now,
        pid: std::process::id(),
        hostname: hostname_or_unknown(),
    };
    match coll.insert_one(&lock).await {
        Ok(_) => Ok(lock.id),
        Err(e) => {
            // Surface the existing lock's details so the user knows what to
            // wait for or force-unlock.
            if let Some(existing) = coll
                .find_one(doc! { "scope": MigrationLock::SCOPE })
                .await
                .ok()
                .flatten()
            {
                Err(format!(
                    "another migration is in progress: {} → {} (started {} by pid {}@{}). \
                     Use --force-unlock to clear a stale lock.",
                    existing.from_storage_id,
                    existing.to_storage_id,
                    existing.started_at,
                    existing.pid,
                    existing.hostname,
                )
                .into())
            } else {
                Err(format!("failed to acquire migration lock: {e}").into())
            }
        }
    }
}

async fn release_lock(db: &Database, lock_id: ObjectId) -> Result<(), Box<dyn std::error::Error>> {
    let coll = db.collection::<MigrationLock>("migration_locks");
    coll.delete_one(doc! { "_id": lock_id }).await?;
    Ok(())
}

fn spawn_heartbeat(db: Database, lock_id: ObjectId, stop: Arc<Notify>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let coll = db.collection::<bson::Document>("migration_locks");
        loop {
            tokio::select! {
                _ = stop.notified() => break,
                _ = tokio::time::sleep(HEARTBEAT_INTERVAL) => {
                    let now = bson::DateTime::from_chrono(Utc::now());
                    if let Err(e) = coll
                        .update_one(
                            doc! { "_id": lock_id },
                            doc! { "$set": { "last_heartbeat": now } },
                        )
                        .await
                    {
                        tracing::warn!("Heartbeat update failed: {e}");
                    }
                }
            }
        }
    })
}

fn hostname_or_unknown() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

fn humanize_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut size = n as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", n, UNITS[0])
    } else {
        format!("{:.2} {}", size, UNITS[unit])
    }
}
