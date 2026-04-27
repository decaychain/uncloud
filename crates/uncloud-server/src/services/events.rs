use mongodb::bson::doc;
use mongodb::bson::oid::ObjectId;
use mongodb::Database;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

// (EventService derives Clone; its inner state is already Arc-wrapped.)

use crate::models::{File, Folder, TaskProject, TaskType, UserRole};
use crate::services::rescan::{RescanConflict, RescanJob, RescanStatus};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Event {
    FileCreated { file: FileEvent },
    FileUpdated { file: FileEvent },
    FileDeleted { file_id: String },
    FolderCreated { folder: FolderEvent },
    FolderUpdated { folder: FolderEvent },
    FolderDeleted { folder_id: String },
    UploadProgress { upload_id: String, progress: f64 },
    ProcessingCompleted { file_id: String, task_type: String, success: bool },
    FileRestored { file_id: String },
    FolderShared { folder_id: String, share_id: String },
    FolderShareRevoked { folder_id: String, share_id: String },
    RescanProgress {
        job_id: String,
        storage_id: String,
        status: String,
        processed_entries: u64,
        total_entries: Option<u64>,
        imported_folders: u64,
        imported_files: u64,
        skipped_existing: u64,
        conflicts_count: u64,
    },
    RescanFinished {
        job_id: String,
        storage_id: String,
        status: String,
        processed_entries: u64,
        total_entries: Option<u64>,
        imported_folders: u64,
        imported_files: u64,
        skipped_existing: u64,
        conflicts: Vec<RescanConflictData>,
        error: Option<String>,
    },
    SyncEventAppended {
        event: uncloud_common::SyncEventResponse,
    },
    /// Hint that something changed in a task project. Frontend re-fetches the
    /// affected views. `task_id` is `Some` when a single task was the target
    /// (create/update/delete/status); `None` for bulk changes (reorder, or
    /// section/label CRUD).
    TaskChanged {
        project_id: String,
        task_id: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct RescanConflictData {
    pub path: String,
    pub reason: String,
}

impl From<&RescanConflict> for RescanConflictData {
    fn from(c: &RescanConflict) -> Self {
        Self {
            path: c.path.clone(),
            reason: c.reason.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FileEvent {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub parent_id: Option<String>,
}

impl From<&File> for FileEvent {
    fn from(f: &File) -> Self {
        Self {
            id: f.id.to_hex(),
            name: f.name.clone(),
            mime_type: f.mime_type.clone(),
            size_bytes: f.size_bytes,
            parent_id: f.parent_id.map(|id| id.to_hex()),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FolderEvent {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
}

impl From<&Folder> for FolderEvent {
    fn from(f: &Folder) -> Self {
        Self {
            id: f.id.to_hex(),
            name: f.name.clone(),
            parent_id: f.parent_id.map(|id| id.to_hex()),
        }
    }
}

struct Channel {
    role: UserRole,
    sender: broadcast::Sender<Event>,
}

#[derive(Clone)]
pub struct EventService {
    // Per-user broadcast channels, tagged with the subscriber's role so we can
    // fan out admin-scoped events without a DB lookup per emit.
    channels: Arc<RwLock<HashMap<ObjectId, Channel>>>,
}

impl EventService {
    pub fn new() -> Self {
        Self {
            channels: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn subscribe(
        &self,
        user_id: ObjectId,
        role: UserRole,
    ) -> broadcast::Receiver<Event> {
        let mut channels = self.channels.write().await;

        match channels.get_mut(&user_id) {
            Some(channel) => {
                // Refresh role in case it changed since last subscribe.
                channel.role = role;
                channel.sender.subscribe()
            }
            None => {
                let (sender, receiver) = broadcast::channel(100);
                channels.insert(user_id, Channel { role, sender });
                receiver
            }
        }
    }

    pub async fn emit(&self, user_id: ObjectId, event: Event) {
        let channels = self.channels.read().await;
        if let Some(channel) = channels.get(&user_id) {
            // Ignore send errors (no active subscribers)
            let _ = channel.sender.send(event);
        }
    }

    /// Fan out to every subscriber whose role is `Admin`. Used for events that
    /// any admin may observe (e.g. storage rescan progress), not tied to a
    /// single initiator.
    pub async fn emit_to_admins(&self, event: Event) {
        let channels = self.channels.read().await;
        for channel in channels.values() {
            if channel.role == UserRole::Admin {
                let _ = channel.sender.send(event.clone());
            }
        }
    }

    pub async fn emit_file_created(&self, user_id: ObjectId, file: &File) {
        self.emit(
            user_id,
            Event::FileCreated {
                file: FileEvent::from(file),
            },
        )
        .await;
    }

    pub async fn emit_file_updated(&self, user_id: ObjectId, file: &File) {
        self.emit(
            user_id,
            Event::FileUpdated {
                file: FileEvent::from(file),
            },
        )
        .await;
    }

    pub async fn emit_file_deleted(&self, user_id: ObjectId, file_id: ObjectId) {
        self.emit(
            user_id,
            Event::FileDeleted {
                file_id: file_id.to_hex(),
            },
        )
        .await;
    }

    pub async fn emit_folder_created(&self, user_id: ObjectId, folder: &Folder) {
        self.emit(
            user_id,
            Event::FolderCreated {
                folder: FolderEvent::from(folder),
            },
        )
        .await;
    }

    pub async fn emit_folder_updated(&self, user_id: ObjectId, folder: &Folder) {
        self.emit(
            user_id,
            Event::FolderUpdated {
                folder: FolderEvent::from(folder),
            },
        )
        .await;
    }

    pub async fn emit_folder_deleted(&self, user_id: ObjectId, folder_id: ObjectId) {
        self.emit(
            user_id,
            Event::FolderDeleted {
                folder_id: folder_id.to_hex(),
            },
        )
        .await;
    }

    pub async fn emit_upload_progress(&self, user_id: ObjectId, upload_id: &str, progress: f64) {
        self.emit(
            user_id,
            Event::UploadProgress {
                upload_id: upload_id.to_string(),
                progress,
            },
        )
        .await;
    }

    pub async fn emit_processing_completed(
        &self,
        user_id: ObjectId,
        file_id: ObjectId,
        task_type: TaskType,
        success: bool,
    ) {
        let task_type_str = match task_type {
            TaskType::Thumbnail => "thumbnail",
            TaskType::AudioMetadata => "audio_metadata",
            TaskType::TextExtract => "text_extract",
            TaskType::SearchIndex => "search_index",
        };
        self.emit(
            user_id,
            Event::ProcessingCompleted {
                file_id: file_id.to_hex(),
                task_type: task_type_str.to_string(),
                success,
            },
        )
        .await;
    }

    pub async fn emit_file_restored(&self, user_id: ObjectId, file_id: ObjectId) {
        self.emit(
            user_id,
            Event::FileRestored {
                file_id: file_id.to_hex(),
            },
        )
        .await;
    }

    pub async fn emit_folder_shared(
        &self,
        user_id: ObjectId,
        folder_id: ObjectId,
        share_id: ObjectId,
    ) {
        self.emit(
            user_id,
            Event::FolderShared {
                folder_id: folder_id.to_hex(),
                share_id: share_id.to_hex(),
            },
        )
        .await;
    }

    pub async fn emit_folder_share_revoked(
        &self,
        user_id: ObjectId,
        folder_id: ObjectId,
        share_id: ObjectId,
    ) {
        self.emit(
            user_id,
            Event::FolderShareRevoked {
                folder_id: folder_id.to_hex(),
                share_id: share_id.to_hex(),
            },
        )
        .await;
    }

    /// Fan out a `TaskChanged` to a project's owner and every member. The
    /// project doc is loaded fresh so callers don't have to thread it
    /// through; if the lookup fails we silently drop the event (the worst
    /// case is a stale UI on another device, which the next manual refresh
    /// recovers from).
    pub async fn emit_task_changed(
        &self,
        db: &Database,
        project_id: ObjectId,
        task_id: Option<ObjectId>,
    ) {
        let coll = db.collection::<TaskProject>("task_projects");
        let project = match coll.find_one(doc! { "_id": project_id }).await {
            Ok(Some(p)) => p,
            _ => return,
        };

        let event = Event::TaskChanged {
            project_id: project_id.to_hex(),
            task_id: task_id.map(|id| id.to_hex()),
        };

        // Owner first, then every distinct member.
        self.emit(project.owner_id, event.clone()).await;
        for m in &project.members {
            if m.user_id != project.owner_id {
                self.emit(m.user_id, event.clone()).await;
            }
        }
    }

    pub async fn emit_rescan_progress(&self, job: &RescanJob) {
        self.emit_to_admins(Event::RescanProgress {
            job_id: job.id.clone(),
            storage_id: job.storage_id.clone(),
            status: rescan_status_str(&job.status).to_string(),
            processed_entries: job.processed_entries,
            total_entries: job.total_entries,
            imported_folders: job.imported_folders,
            imported_files: job.imported_files,
            skipped_existing: job.skipped_existing,
            conflicts_count: job.conflicts.len() as u64,
        })
        .await;
    }

    pub async fn emit_rescan_finished(&self, job: &RescanJob) {
        self.emit_to_admins(Event::RescanFinished {
            job_id: job.id.clone(),
            storage_id: job.storage_id.clone(),
            status: rescan_status_str(&job.status).to_string(),
            processed_entries: job.processed_entries,
            total_entries: job.total_entries,
            imported_folders: job.imported_folders,
            imported_files: job.imported_files,
            skipped_existing: job.skipped_existing,
            conflicts: job.conflicts.iter().map(RescanConflictData::from).collect(),
            error: job.error.clone(),
        })
        .await;
    }

    pub async fn cleanup_user(&self, user_id: ObjectId) {
        self.channels.write().await.remove(&user_id);
    }
}

fn rescan_status_str(status: &RescanStatus) -> &'static str {
    match status {
        RescanStatus::Running => "running",
        RescanStatus::Completed => "completed",
        RescanStatus::Failed => "failed",
        RescanStatus::Cancelled => "cancelled",
    }
}

impl Default for EventService {
    fn default() -> Self {
        Self::new()
    }
}
