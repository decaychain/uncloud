use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentVaultEntry {
    pub file_id: String,
    pub file_name: String,
    /// Display path like "Documents/Secrets" — the folder the vault lives in
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_path: Option<String>,
    pub last_opened_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddRecentVaultRequest {
    pub file_id: String,
    pub file_name: String,
    #[serde(default)]
    pub folder_path: Option<String>,
}
