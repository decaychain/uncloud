//! Backup lock — singleton-by-scope, mirrors `MigrationLock`.
//!
//! Mutually exclusive with `MigrationLock`: `backup` refuses if a migration
//! is in progress, and `migrate` refuses if a backup is in progress. The
//! server's startup interlock checks both.

use std::sync::Arc;
use std::time::Duration;

use bson::doc;
use chrono::Utc;
use mongodb::bson::oid::ObjectId;
use mongodb::{bson, Database};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::models::BackupLock;

/// Maximum age of `last_heartbeat` before we treat a lock row as stale.
const STALE_AFTER: chrono::Duration = chrono::Duration::minutes(5);
/// How often the heartbeat task refreshes `last_heartbeat`.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// Check whether a backup operation is in progress. Called from the server's
/// startup path (refuses to start) and from `migrate` (refuses to run).
pub async fn check_no_active_backup(db: &Database) -> Result<(), String> {
    let coll = db.collection::<BackupLock>("backup_locks");
    let lock = coll
        .find_one(doc! { "scope": BackupLock::SCOPE })
        .await
        .map_err(|e| format!("failed to query backup_locks: {e}"))?;
    let Some(lock) = lock else { return Ok(()) };

    let age = Utc::now() - lock.last_heartbeat;
    if age < STALE_AFTER {
        return Err(format!(
            "a backup operation is in progress\n  operation: {}\n  started: {}\n  pid: {}@{}\n\n\
             Wait for it to finish, or run the matching backup subcommand with --force-unlock to clear the lock if the previous run crashed.",
            lock.operation, lock.started_at, lock.pid, lock.hostname,
        ));
    }
    Err(format!(
        "found stale backup lock (last heartbeat {} ago, started by pid {}@{}). Use --force-unlock to clear it.",
        format_age(age), lock.pid, lock.hostname,
    ))
}

pub(crate) async fn acquire(
    db: &Database,
    operation: String,
) -> Result<ObjectId, Box<dyn std::error::Error>> {
    let coll = db.collection::<BackupLock>("backup_locks");
    let now = Utc::now();
    let lock = BackupLock {
        id: ObjectId::new(),
        scope: BackupLock::SCOPE.to_string(),
        operation,
        started_at: now,
        last_heartbeat: now,
        pid: std::process::id(),
        hostname: hostname_or_unknown(),
    };
    match coll.insert_one(&lock).await {
        Ok(_) => Ok(lock.id),
        Err(e) => {
            if let Some(existing) = coll
                .find_one(doc! { "scope": BackupLock::SCOPE })
                .await
                .ok()
                .flatten()
            {
                Err(format!(
                    "another backup operation is in progress: {} (started {} by pid {}@{}). \
                     Use --force-unlock to clear a stale lock.",
                    existing.operation, existing.started_at, existing.pid, existing.hostname,
                )
                .into())
            } else {
                Err(format!("failed to acquire backup lock: {e}").into())
            }
        }
    }
}

pub(crate) async fn release(
    db: &Database,
    lock_id: ObjectId,
) -> Result<(), Box<dyn std::error::Error>> {
    let coll = db.collection::<BackupLock>("backup_locks");
    coll.delete_one(doc! { "_id": lock_id }).await?;
    Ok(())
}

pub(crate) fn spawn_heartbeat(
    db: Database,
    lock_id: ObjectId,
    stop: Arc<Notify>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let coll = db.collection::<bson::Document>("backup_locks");
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
                        tracing::warn!("Backup heartbeat update failed: {e}");
                    }
                }
            }
        }
    })
}

pub(crate) async fn force_unlock(db: &Database) -> Result<u64, Box<dyn std::error::Error>> {
    let coll = db.collection::<BackupLock>("backup_locks");
    let res = coll.delete_one(doc! { "scope": BackupLock::SCOPE }).await?;
    Ok(res.deleted_count)
}

fn hostname_or_unknown() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
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
