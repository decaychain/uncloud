use uncloud_common::{CreateFolderShareRequest, FolderShareResponse, UpdateFolderShareRequest};

use super::api;

pub async fn create_folder_share(
    req: &CreateFolderShareRequest,
) -> Result<FolderShareResponse, String> {
    let response = api::post("/folder-shares")
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<FolderShareResponse>()
            .await
            .map_err(|e| e.to_string())
    } else if response.status() == 409 {
        Err("CONFLICT".to_string())
    } else if response.status() == 404 {
        Err("User not found".to_string())
    } else {
        let text = response.text().await.unwrap_or_default();
        Err(text)
    }
}

pub async fn list_shares_by_me() -> Result<Vec<FolderShareResponse>, String> {
    let response = api::get("/folder-shares/by-me")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<FolderShareResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load shares".to_string())
    }
}

pub async fn list_shares_with_me() -> Result<Vec<FolderShareResponse>, String> {
    let response = api::get("/folder-shares/with-me")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<FolderShareResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load shares".to_string())
    }
}

pub async fn list_folder_shares(folder_id: &str) -> Result<Vec<FolderShareResponse>, String> {
    let response = api::get(&format!("/folder-shares/folder/{}", folder_id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<FolderShareResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load folder shares".to_string())
    }
}

pub async fn update_folder_share(
    id: &str,
    req: &UpdateFolderShareRequest,
) -> Result<FolderShareResponse, String> {
    let response = api::put(&format!("/folder-shares/{}", id))
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<FolderShareResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        let text = response.text().await.unwrap_or_default();
        Err(text)
    }
}

pub async fn delete_folder_share(id: &str) -> Result<(), String> {
    let response = api::delete(&format!("/folder-shares/{}", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        Ok(())
    } else {
        Err("Failed to delete share".to_string())
    }
}
