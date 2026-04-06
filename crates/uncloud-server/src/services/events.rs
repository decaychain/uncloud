use mongodb::bson::oid::ObjectId;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

use crate::models::{File, Folder, TaskType};

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

pub struct EventService {
    // Per-user broadcast channels
    channels: Arc<RwLock<HashMap<ObjectId, broadcast::Sender<Event>>>>,
}

impl EventService {
    pub fn new() -> Self {
        Self {
            channels: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn subscribe(&self, user_id: ObjectId) -> broadcast::Receiver<Event> {
        let mut channels = self.channels.write().await;

        if let Some(sender) = channels.get(&user_id) {
            sender.subscribe()
        } else {
            let (sender, receiver) = broadcast::channel(100);
            channels.insert(user_id, sender);
            receiver
        }
    }

    pub async fn emit(&self, user_id: ObjectId, event: Event) {
        let channels = self.channels.read().await;
        if let Some(sender) = channels.get(&user_id) {
            // Ignore send errors (no active subscribers)
            let _ = sender.send(event);
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

    pub async fn cleanup_user(&self, user_id: ObjectId) {
        self.channels.write().await.remove(&user_id);
    }
}

impl Default for EventService {
    fn default() -> Self {
        Self::new()
    }
}
