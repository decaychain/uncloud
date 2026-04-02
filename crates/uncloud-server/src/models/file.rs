use bson::serde_helpers::chrono_datetime_as_bson_datetime;
use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    Thumbnail,
    AudioMetadata,
    TextExtract,
    SearchIndex,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessingStatus {
    Pending,
    Done,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessingTask {
    pub task_type: TaskType,
    pub status: ProcessingStatus,
    pub attempts: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub queued_at: DateTime<Utc>,
    #[serde(default, with = "super::opt_dt")]
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct File {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub storage_id: ObjectId,
    pub storage_path: String,
    pub owner_id: ObjectId,
    pub parent_id: Option<ObjectId>,
    pub name: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub checksum_sha256: String,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub processing_tasks: Vec<ProcessingTask>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, mongodb::bson::Bson>,
    #[serde(default, with = "super::opt_dt")]
    pub deleted_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trash_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_delete_id: Option<String>,
}

impl File {
    pub fn new(
        storage_id: ObjectId,
        storage_path: String,
        owner_id: ObjectId,
        parent_id: Option<ObjectId>,
        name: String,
        mime_type: String,
        size_bytes: i64,
        checksum_sha256: String,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: ObjectId::new(),
            storage_id,
            storage_path,
            owner_id,
            parent_id,
            name,
            mime_type,
            size_bytes,
            checksum_sha256,
            created_at: now,
            updated_at: now,
            processing_tasks: vec![],
            metadata: HashMap::new(),
            deleted_at: None,
            trash_path: None,
            batch_delete_id: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileVersion {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub file_id: ObjectId,
    pub version: i32,
    pub storage_path: String,
    pub size_bytes: i64,
    pub checksum_sha256: String,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
}

impl FileVersion {
    pub fn new(
        file_id: ObjectId,
        version: i32,
        storage_path: String,
        size_bytes: i64,
        checksum_sha256: String,
    ) -> Self {
        Self {
            id: ObjectId::new(),
            file_id,
            version,
            storage_path,
            size_bytes,
            checksum_sha256,
            created_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadChunk {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub upload_id: String,
    pub user_id: ObjectId,
    pub filename: String,
    pub parent_id: Option<ObjectId>,
    pub storage_id: ObjectId,
    pub total_size: i64,
    pub chunk_size: i64,
    pub chunks_received: Vec<i32>,
    pub temp_path: String,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
}

impl UploadChunk {
    pub fn new(
        upload_id: String,
        user_id: ObjectId,
        filename: String,
        parent_id: Option<ObjectId>,
        storage_id: ObjectId,
        total_size: i64,
        chunk_size: i64,
        temp_path: String,
    ) -> Self {
        Self {
            id: ObjectId::new(),
            upload_id,
            user_id,
            filename,
            parent_id,
            storage_id,
            total_size,
            chunk_size,
            chunks_received: Vec::new(),
            temp_path,
            created_at: Utc::now(),
        }
    }

    pub fn total_chunks(&self) -> i32 {
        ((self.total_size as f64) / (self.chunk_size as f64)).ceil() as i32
    }

    pub fn is_complete(&self) -> bool {
        self.chunks_received.len() as i32 == self.total_chunks()
    }
}
