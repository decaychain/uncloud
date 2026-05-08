use uncloud_common::{
    CreateMusicCategoryRequest, MusicCategory, UpdateMusicCategoryRequest,
};

use super::api;

pub async fn list_categories() -> Result<Vec<MusicCategory>, String> {
    let response = api::get("/music/categories")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<MusicCategory>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load categories".to_string())
    }
}

pub async fn create_category(
    name: &str,
    folder_ids: Vec<String>,
) -> Result<MusicCategory, String> {
    let body = CreateMusicCategoryRequest {
        name: name.to_string(),
        folder_ids,
    };
    let response = api::post("/music/categories")
        .json(&body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 201 {
        response
            .json::<MusicCategory>()
            .await
            .map_err(|e| e.to_string())
    } else if response.status() == 409 {
        Err("CONFLICT".to_string())
    } else {
        Err("Failed to create category".to_string())
    }
}

pub async fn update_category(
    id: &str,
    name: Option<&str>,
    folder_ids: Option<Vec<String>>,
) -> Result<MusicCategory, String> {
    let body = UpdateMusicCategoryRequest {
        name: name.map(|s| s.to_string()),
        folder_ids,
    };
    let response = api::put(&format!("/music/categories/{}", id))
        .json(&body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<MusicCategory>()
            .await
            .map_err(|e| e.to_string())
    } else if response.status() == 409 {
        Err("CONFLICT".to_string())
    } else {
        Err("Failed to update category".to_string())
    }
}

pub async fn delete_category(id: &str) -> Result<(), String> {
    let response = api::delete(&format!("/music/categories/{}", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to delete category".to_string())
    }
}
