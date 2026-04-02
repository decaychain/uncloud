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
