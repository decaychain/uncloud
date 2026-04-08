use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShareResourceType {
    File,
    Folder,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateShareRequest {
    pub resource_type: ShareResourceType,
    pub resource_id: String,
    pub password: Option<String>,
    pub expires_hours: Option<u64>,
    pub max_downloads: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareResponse {
    pub id: String,
    pub token: String,
    pub resource_type: ShareResourceType,
    pub resource_id: String,
    #[serde(default)]
    pub resource_name: String,
    pub has_password: bool,
    pub expires_at: Option<String>,
    pub download_count: i64,
    pub max_downloads: Option<i64>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicShareResponse {
    pub resource_type: ShareResourceType,
    pub name: String,
    pub size_bytes: Option<i64>,
    pub has_password: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyPasswordRequest {
    pub password: String,
}

impl ShareResponse {
    pub fn share_url(&self, base_url: &str) -> String {
        format!("{}/share/{}", base_url.trim_end_matches('/'), self.token)
    }
}
