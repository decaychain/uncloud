use super::sync_events::SyncEventResponse;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ServerEvent {
    FileCreated { file: FileEventData },
    FileUpdated { file: FileEventData },
    FileDeleted { file_id: String },
    FolderCreated { folder: FolderEventData },
    FolderUpdated { folder: FolderEventData },
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
        conflicts: Vec<RescanConflictEventData>,
        error: Option<String>,
    },
    SyncEventAppended { event: SyncEventResponse },
    TaskChanged {
        project_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task_id: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RescanConflictEventData {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEventData {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderEventData {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
}
