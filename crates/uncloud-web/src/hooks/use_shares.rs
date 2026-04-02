use uncloud_common::{CreateShareRequest, ShareResourceType, ShareResponse};

use super::api;

pub async fn create_share(
    resource_id: &str,
    resource_type: &str,
    password: Option<&str>,
    expires_hours: Option<u64>,
    max_downloads: Option<i64>,
) -> Result<ShareResponse, String> {
    let resource_type = match resource_type {
        "folder" => ShareResourceType::Folder,
        _ => ShareResourceType::File,
    };

    let req = CreateShareRequest {
        resource_type,
        resource_id: resource_id.to_string(),
        password: password.map(|s| s.to_string()),
        expires_hours,
        max_downloads,
    };

    let response = api::post("/shares")
        .json(&req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<ShareResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to create share".to_string())
    }
}

pub async fn list_shares() -> Result<Vec<ShareResponse>, String> {
    let response = api::get("/shares")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<ShareResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load shares".to_string())
    }
}

pub async fn delete_share(id: &str) -> Result<(), String> {
    let response = api::delete(&format!("/shares/{}", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        Ok(())
    } else {
        Err("Failed to delete share".to_string())
    }
}
