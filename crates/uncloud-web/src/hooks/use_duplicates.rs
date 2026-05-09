use uncloud_common::DuplicateReport;

use super::api;

pub async fn get_duplicate_report() -> Result<DuplicateReport, String> {
    let response = api::get("/duplicates")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status() == 200 {
        return Err(format!("Failed to load duplicates ({})", response.status()));
    }
    response
        .json::<DuplicateReport>()
        .await
        .map_err(|e| e.to_string())
}

/// Soft-delete a single file. Reuses the existing DELETE /api/files/{id}
/// handler, so the file goes to trash and remains recoverable.
pub async fn delete_file(file_id: &str) -> Result<(), String> {
    let response = api::delete(&format!("/files/{}", file_id))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    match response.status() {
        204 | 200 => Ok(()),
        s => Err(format!("Delete failed ({})", s)),
    }
}

/// Delete a batch of files in parallel with a small concurrency cap.
/// Returns (successful_count, failed_count).
pub async fn delete_files(ids: Vec<String>) -> (usize, Vec<String>) {
    use futures::stream::{self, StreamExt};
    const CONCURRENCY: usize = 8;

    let results: Vec<Result<(), String>> = stream::iter(ids.iter().cloned())
        .map(|id| async move { delete_file(&id).await })
        .buffer_unordered(CONCURRENCY)
        .collect()
        .await;

    let mut ok_count = 0;
    let mut errors = Vec::new();
    for r in results {
        match r {
            Ok(()) => ok_count += 1,
            Err(e) => errors.push(e),
        }
    }
    (ok_count, errors)
}
