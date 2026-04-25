use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncOperation {
    Created,
    Renamed,
    Moved,
    Deleted,
    Restored,
    PermanentlyDeleted,
    ContentReplaced,
    Copied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncResourceType {
    File,
    Folder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncEventSource {
    UserWeb,
    UserDesktop,
    UserMobile,
    Sync,
    Admin,
    Public,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncClientOs {
    Linux,
    Windows,
    Macos,
    Android,
    Ios,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncEventResponse {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub operation: SyncOperation,
    pub resource_type: SyncResourceType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<String>,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_path: Option<String>,
    pub source: SyncEventSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_os: Option<SyncClientOs>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub affected_count: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncEventListResponse {
    pub events: Vec<SyncEventResponse>,
    pub has_more: bool,
}
