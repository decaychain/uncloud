use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileResponse {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub parent_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitUploadRequest {
    pub filename: String,
    pub size: i64,
    pub parent_id: Option<String>,
    pub chunk_size: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitUploadResponse {
    pub upload_id: String,
    pub chunk_size: i64,
    pub total_chunks: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateFileRequest {
    pub name: Option<String>,
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopyFileRequest {
    /// Destination folder ID. None = same folder as source; empty string = root.
    pub parent_id: Option<String>,
    /// New filename. None = "Copy of {original}".
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GalleryResponse {
    pub files: Vec<FileResponse>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlbumResponse {
    pub folder_id: String,
    /// `Some(id)` when the immediate parent folder is also a gallery album.
    pub parent_folder_id: Option<String>,
    pub name: String,
    /// Breadcrumb-style path: "photos / vacation"
    pub path: String,
    pub image_count: i64,
    /// Most recent image ID for cover thumbnail
    pub cover_image_id: Option<String>,
}

impl FileResponse {
    pub fn is_image(&self) -> bool {
        self.mime_type.starts_with("image/")
    }

    pub fn is_video(&self) -> bool {
        self.mime_type.starts_with("video/")
    }

    pub fn is_audio(&self) -> bool {
        self.mime_type.starts_with("audio/")
    }

    pub fn is_document(&self) -> bool {
        matches!(
            self.mime_type.as_str(),
            "application/pdf"
                | "application/msword"
                | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                | "text/plain"
                | "text/markdown"
        )
    }

    pub fn formatted_size(&self) -> String {
        const KB: i64 = 1024;
        const MB: i64 = KB * 1024;
        const GB: i64 = MB * 1024;

        if self.size_bytes >= GB {
            format!("{:.2} GB", self.size_bytes as f64 / GB as f64)
        } else if self.size_bytes >= MB {
            format!("{:.2} MB", self.size_bytes as f64 / MB as f64)
        } else if self.size_bytes >= KB {
            format!("{:.2} KB", self.size_bytes as f64 / KB as f64)
        } else {
            format!("{} B", self.size_bytes)
        }
    }
}
