use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharePermission {
    ReadOnly,
    ReadWrite,
    Admin,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateFolderShareRequest {
    pub folder_id: String,
    pub grantee_username: String,
    pub permission: SharePermission,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateFolderShareRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<SharePermission>,
    /// Empty string = move back to "Shared with me" root
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount_parent_id: Option<String>,
    /// Empty string = clear override, use real folder name
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderShareResponse {
    pub id: String,
    pub folder_id: String,
    pub folder_name: String,
    pub owner_id: String,
    pub owner_username: String,
    pub grantee_id: String,
    pub grantee_username: String,
    pub permission: SharePermission,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount_parent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount_name: Option<String>,
    pub created_at: String,
}
