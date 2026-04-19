use serde::Deserialize;

use super::api;

#[derive(Debug, Clone, Deserialize)]
pub struct StorageSummary {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RescanResult {
    pub scanned_entries: usize,
    pub imported_folders: usize,
    pub imported_files: usize,
    pub skipped_existing: usize,
    pub conflicts: Vec<RescanConflict>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RescanConflict {
    pub path: String,
    pub reason: String,
}

/// Admin-only: GET /api/admin/storages.
pub async fn list_storages() -> Result<Vec<StorageSummary>, String> {
    let response = api::get("/admin/storages")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if response.status() != 200 {
        return Err(format!("Failed to list storages ({})", response.status()));
    }
    response.json().await.map_err(|e| e.to_string())
}

/// Admin-only: POST /api/admin/storages/{id}/rescan — walks the backend and
/// imports any on-disk file/folder missing from the DB.
pub async fn rescan(storage_id: &str) -> Result<RescanResult, String> {
    let response = api::post(&format!("/admin/storages/{}/rescan", storage_id))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if response.status() != 200 {
        return Err(format!("Rescan failed ({})", response.status()));
    }
    response.json().await.map_err(|e| e.to_string())
}
