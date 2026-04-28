use serde::Deserialize;

use super::api;

#[derive(Debug, Clone, Deserialize)]
pub struct StorageSummary {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RescanStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RescanConflict {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RescanJob {
    pub id: String,
    pub storage_id: String,
    pub status: RescanStatus,
    pub total_entries: Option<u64>,
    pub processed_entries: u64,
    pub imported_folders: u64,
    pub imported_files: u64,
    pub skipped_existing: u64,
    pub conflicts: Vec<RescanConflict>,
    pub error: Option<String>,
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

/// Non-admin: GET /api/storages — used by the folder-create dropdown so any
/// signed-in user can pick which storage a new folder lives on.
pub async fn list_storages_for_user() -> Result<Vec<StorageSummary>, String> {
    let response = api::get("/storages")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if response.status() != 200 {
        return Err(format!("Failed to list storages ({})", response.status()));
    }
    response.json().await.map_err(|e| e.to_string())
}

/// Admin-only: POST /api/admin/storages/{id}/rescan — kicks off a background
/// rescan and returns the initial job state (status=Running, counters=0).
pub async fn start_rescan(storage_id: &str) -> Result<RescanJob, String> {
    let response = api::post(&format!("/admin/storages/{}/rescan", storage_id))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    if status != 202 && status != 200 {
        let body = response.text().await.unwrap_or_default();
        return Err(if body.is_empty() {
            format!("Rescan failed ({})", status)
        } else {
            body
        });
    }
    response.json().await.map_err(|e| e.to_string())
}




/// Admin-only: GET /api/admin/rescan-jobs/active — current running job, if any.
/// Used on mount to restore the live-progress panel after navigation/reload.
pub async fn get_active_rescan_job() -> Result<Option<RescanJob>, String> {
    let response = api::get("/admin/rescan-jobs/active")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if response.status() != 200 {
        return Err(format!("Failed to fetch active rescan job ({})", response.status()));
    }
    response.json().await.map_err(|e| e.to_string())
}

/// Admin-only: POST /api/admin/rescan-jobs/{id}/cancel — requests cancellation.
pub async fn cancel_rescan_job(job_id: &str) -> Result<(), String> {
    let response = api::post(&format!("/admin/rescan-jobs/{}/cancel", job_id))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if response.status() != 204 && response.status() != 200 {
        return Err(format!("Cancel failed ({})", response.status()));
    }
    Ok(())
}
