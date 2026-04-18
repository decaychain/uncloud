use super::api;

/// Admin-only: POST /api/admin/processing/rerun — clears the processing state
/// of every file and re-queues the full pipeline.
pub async fn rerun_all() -> Result<(), String> {
    let response = api::post("/admin/processing/rerun")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    match response.status() {
        202 => Ok(()),
        403 => Err("Admin role required.".to_string()),
        s => Err(format!("Rerun failed ({})", s)),
    }
}
