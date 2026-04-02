use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileVersionResponse {
    pub id: String,
    pub version: i32,
    pub size_bytes: i64,
    pub checksum_sha256: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrashItemResponse {
    pub id: String,
    pub name: String,
    pub is_folder: bool,
    pub mime_type: Option<String>,
    pub size_bytes: Option<i64>,
    pub original_path: Option<String>,
    pub parent_id: Option<String>,
    pub deleted_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_delete_id: Option<String>,
}
