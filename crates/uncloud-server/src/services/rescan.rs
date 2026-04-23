//! In-memory registry of storage rescan jobs.
//!
//! A rescan walks an entire storage backend, so it can take minutes to hours.
//! The HTTP endpoint spawns the work in the background and returns a job id
//! the client can poll (and cancel). Jobs live only in memory and are lost on
//! server restart — that's fine, the rescan is idempotent and can simply be
//! restarted.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::Serialize;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RescanStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
pub struct RescanConflict {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RescanJob {
    pub id: String,
    pub storage_id: String,
    pub status: RescanStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub total_entries: Option<u64>,
    pub processed_entries: u64,
    pub imported_folders: u64,
    pub imported_files: u64,
    pub skipped_existing: u64,
    pub conflicts: Vec<RescanConflict>,
    pub error: Option<String>,
}

/// A running or finished job plus the cancel flag its worker polls.
pub struct RescanJobHandle {
    pub job: RwLock<RescanJob>,
    pub cancel_flag: AtomicBool,
}

impl RescanJobHandle {
    pub fn is_cancelled(&self) -> bool {
        self.cancel_flag.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
pub struct RescanService {
    jobs: RwLock<HashMap<ObjectId, Arc<RescanJobHandle>>>,
    active_by_storage: RwLock<HashMap<ObjectId, ObjectId>>,
}

impl RescanService {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new job for `storage_id`. Returns an error if one is already
    /// running for that storage. The returned handle is shared between the
    /// HTTP response, the worker, and subsequent status/cancel calls.
    pub async fn start_job(
        &self,
        storage_id: ObjectId,
    ) -> std::result::Result<(ObjectId, Arc<RescanJobHandle>), String> {
        let mut active = self.active_by_storage.write().await;
        if active.contains_key(&storage_id) {
            return Err("A rescan is already running for this storage".to_string());
        }
        let job_id = ObjectId::new();
        let job = RescanJob {
            id: job_id.to_hex(),
            storage_id: storage_id.to_hex(),
            status: RescanStatus::Running,
            started_at: Utc::now(),
            finished_at: None,
            total_entries: None,
            processed_entries: 0,
            imported_folders: 0,
            imported_files: 0,
            skipped_existing: 0,
            conflicts: Vec::new(),
            error: None,
        };
        let handle = Arc::new(RescanJobHandle {
            job: RwLock::new(job),
            cancel_flag: AtomicBool::new(false),
        });
        active.insert(storage_id, job_id);
        self.jobs.write().await.insert(job_id, handle.clone());
        Ok((job_id, handle))
    }

    /// Called by the worker once it's finished so a fresh rescan can start.
    pub async fn release(&self, storage_id: ObjectId) {
        self.active_by_storage.write().await.remove(&storage_id);
    }

    pub async fn get(&self, job_id: ObjectId) -> Option<Arc<RescanJobHandle>> {
        self.jobs.read().await.get(&job_id).cloned()
    }

    /// Returns a handle to any currently-running rescan, or `None` if nothing is
    /// active. There is at most one active job per storage, but across all
    /// storages there could be more than one; this returns the first we find
    /// (matches today's single-storage deployments).
    pub async fn any_active(&self) -> Option<Arc<RescanJobHandle>> {
        let active = self.active_by_storage.read().await;
        let jobs = self.jobs.read().await;
        for (_storage_id, job_id) in active.iter() {
            if let Some(handle) = jobs.get(job_id) {
                return Some(handle.clone());
            }
        }
        None
    }

    pub async fn cancel(&self, job_id: ObjectId) -> bool {
        let jobs = self.jobs.read().await;
        match jobs.get(&job_id) {
            Some(handle) => {
                handle.cancel_flag.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }
}
