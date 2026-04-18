use uncloud_common::{
    AlbumResponse, CopyFileRequest, CopyFolderRequest, CreateFolderRequest, EffectiveStrategyResponse,
    FileResponse, FileVersionResponse, FolderResponse, GalleryInclude, GalleryResponse, MusicInclude,
    SyncStrategy, TrashItemResponse, UpdateFileRequest, UpdateFolderRequest,
};
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;

use super::api;

pub async fn list_contents(
    parent_id: Option<&str>,
) -> Result<(Vec<FileResponse>, Vec<FolderResponse>), String> {
    let files = list_files(parent_id).await?;
    let folders = list_folders(parent_id).await?;
    Ok((files, folders))
}

pub async fn get_file(file_id: &str) -> Result<FileResponse, String> {
    let response = api::get(&format!("/files/{}", file_id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<FileResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(format!("Failed to load file ({})", response.status()))
    }
}

pub async fn list_files(parent_id: Option<&str>) -> Result<Vec<FileResponse>, String> {
    let url = match parent_id {
        Some(id) => format!("{}/files?parent_id={}", api::api_url(""), id),
        None => api::api_url("/files"),
    };

    let response = api::get_raw(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<FileResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load files".to_string())
    }
}

pub async fn list_folders(parent_id: Option<&str>) -> Result<Vec<FolderResponse>, String> {
    let url = match parent_id {
        Some(id) => format!("{}/folders?parent_id={}", api::api_url(""), id),
        None => api::api_url("/folders"),
    };

    let response = api::get_raw(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<FolderResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load folders".to_string())
    }
}

pub async fn create_folder(name: &str, parent_id: Option<&str>) -> Result<FolderResponse, String> {
    let req = CreateFolderRequest {
        name: name.to_string(),
        parent_id: parent_id.map(|s| s.to_string()),
    };

    let response = api::post("/folders")
        .json(&req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<FolderResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to create folder".to_string())
    }
}

pub async fn get_breadcrumb(folder_id: &str) -> Result<Vec<FolderResponse>, String> {
    let response = api::get(&format!("/folders/{}/breadcrumb", folder_id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<FolderResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load breadcrumb".to_string())
    }
}

pub async fn delete_file(id: &str) -> Result<(), String> {
    let response = api::delete(&format!("/files/{}", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        Ok(())
    } else {
        Err("Failed to delete file".to_string())
    }
}

pub async fn delete_folder(id: &str) -> Result<(), String> {
    let response = api::delete(&format!("/folders/{}", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        Ok(())
    } else {
        Err("Failed to delete folder".to_string())
    }
}

pub async fn rename_file(id: &str, name: &str) -> Result<FileResponse, String> {
    let req = UpdateFileRequest { name: Some(name.to_string()), parent_id: None };
    update_file_req(id, &req).await
}

pub async fn rename_folder(id: &str, name: &str) -> Result<FolderResponse, String> {
    let req = UpdateFolderRequest { name: Some(name.to_string()), parent_id: None, sync_strategy: None, gallery_include: None, music_include: None };
    update_folder_req(id, &req).await
}

/// Move a file. `parent_id = None` -> root; `parent_id = Some(id)` -> folder.
/// `name` renames the file at the destination (used for conflict resolution).
pub async fn move_file(id: &str, parent_id: Option<&str>, name: Option<&str>) -> Result<FileResponse, String> {
    let req = UpdateFileRequest {
        name: name.map(|s| s.to_string()),
        parent_id: Some(parent_id.unwrap_or("").to_string()),
    };
    update_file_req(id, &req).await
}

/// Move a folder. `parent_id = None` -> root; `parent_id = Some(id)` -> folder.
/// `name` renames the folder at the destination (used for conflict resolution).
pub async fn move_folder(id: &str, parent_id: Option<&str>, name: Option<&str>) -> Result<FolderResponse, String> {
    let req = UpdateFolderRequest {
        name: name.map(|s| s.to_string()),
        parent_id: Some(parent_id.unwrap_or("").to_string()),
        sync_strategy: None,
        gallery_include: None,
        music_include: None,
    };
    update_folder_req(id, &req).await
}

/// Copy a folder recursively. `parent_id = None` -> same parent; `parent_id = Some("")` -> root.
pub async fn copy_folder(
    id: &str,
    parent_id: Option<&str>,
    name: Option<&str>,
) -> Result<FolderResponse, String> {
    let req = CopyFolderRequest {
        parent_id: Some(parent_id.unwrap_or("").to_string()),
        name: name.map(|s| s.to_string()),
    };
    let response = api::post(&format!("/folders/{}/copy", id))
        .json(&req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if response.ok() {
        response.json::<FolderResponse>().await.map_err(|e| e.to_string())
    } else if response.status() == 409 {
        Err("CONFLICT".to_string())
    } else {
        Err(format!("Failed to copy folder (HTTP {})", response.status()))
    }
}

/// Copy a file. `parent_id = None` -> same folder; `parent_id = Some("")` -> root.
pub async fn copy_file(
    id: &str,
    parent_id: Option<&str>,
    name: Option<&str>,
) -> Result<FileResponse, String> {
    let req = CopyFileRequest {
        parent_id: Some(parent_id.unwrap_or("").to_string()),
        name: name.map(|s| s.to_string()),
    };
    let response = api::post(&format!("/files/{}/copy", id))
        .json(&req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if response.ok() {
        response.json::<FileResponse>().await.map_err(|e| e.to_string())
    } else if response.status() == 409 {
        Err("CONFLICT".to_string())
    } else {
        Err(format!("Failed to copy file (HTTP {})", response.status()))
    }
}

async fn update_file_req(id: &str, req: &UpdateFileRequest) -> Result<FileResponse, String> {
    let response = api::put(&format!("/files/{}", id))
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if response.ok() {
        response.json::<FileResponse>().await.map_err(|e| e.to_string())
    } else if response.status() == 409 {
        Err("CONFLICT".to_string())
    } else {
        Err(format!("Failed to update file (HTTP {})", response.status()))
    }
}

async fn update_folder_req(id: &str, req: &UpdateFolderRequest) -> Result<FolderResponse, String> {
    let response = api::put(&format!("/folders/{}", id))
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if response.ok() {
        response.json::<FolderResponse>().await.map_err(|e| e.to_string())
    } else if response.status() == 409 {
        Err("CONFLICT".to_string())
    } else {
        Err(format!("Failed to update folder (HTTP {})", response.status()))
    }
}

pub async fn get_effective_strategy(id: &str) -> Result<EffectiveStrategyResponse, String> {
    let response = api::get(&format!("/folders/{}/effective-strategy", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if response.ok() {
        response
            .json::<EffectiveStrategyResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to get effective strategy".to_string())
    }
}

pub async fn update_folder_strategy(id: &str, strategy: SyncStrategy) -> Result<FolderResponse, String> {
    let req = UpdateFolderRequest { name: None, parent_id: None, sync_strategy: Some(strategy), gallery_include: None, music_include: None };
    update_folder_req(id, &req).await
}

pub async fn update_folder_gallery_include(id: &str, gallery: GalleryInclude) -> Result<FolderResponse, String> {
    let req = UpdateFolderRequest { name: None, parent_id: None, sync_strategy: None, gallery_include: Some(gallery), music_include: None };
    update_folder_req(id, &req).await
}

pub async fn update_folder_music_include(id: &str, music: MusicInclude) -> Result<FolderResponse, String> {
    let req = UpdateFolderRequest { name: None, parent_id: None, sync_strategy: None, gallery_include: None, music_include: Some(music) };
    update_folder_req(id, &req).await
}

pub async fn list_gallery(
    cursor: Option<&str>,
    limit: Option<u32>,
    folder_id: Option<&str>,
) -> Result<GalleryResponse, String> {
    let mut url = api::api_url("/gallery");
    let mut params = Vec::new();
    if let Some(c) = cursor {
        // RFC3339 contains `+` and `:` — URL-encode so the server sees the
        // same string it produced as `next_cursor`.
        let encoded = js_sys::encode_uri_component(c)
            .as_string()
            .unwrap_or_else(|| c.to_string());
        params.push(format!("cursor={}", encoded));
    }
    if let Some(l) = limit {
        params.push(format!("limit={}", l));
    }
    if let Some(fid) = folder_id {
        params.push(format!("folder_id={}", fid));
    }
    if !params.is_empty() {
        url = format!("{}?{}", url, params.join("&"));
    }

    let response = api::get_raw(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response.json::<GalleryResponse>().await.map_err(|e| e.to_string())
    } else {
        Err("Failed to load gallery".to_string())
    }
}

pub async fn list_gallery_albums() -> Result<Vec<AlbumResponse>, String> {
    let response = api::get("/gallery/albums")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response.json::<Vec<AlbumResponse>>().await.map_err(|e| e.to_string())
    } else {
        Err("Failed to load albums".to_string())
    }
}

pub fn download_url(id: &str) -> String {
    api::authenticated_media_url(&format!("/files/{}/download", id))
}

pub async fn upload_file(file: &web_sys::File, parent_id: Option<&str>) -> Result<FileResponse, String> {
    let form = web_sys::FormData::new()
        .map_err(|_| "Failed to create FormData".to_string())?;

    let blob = file.unchecked_ref::<web_sys::Blob>();
    form.append_with_blob_and_filename("file", blob, &file.name())
        .map_err(|_| "Failed to append file to form".to_string())?;

    if let Some(pid) = parent_id {
        form.append_with_str("parent_id", pid)
            .map_err(|_| "Failed to append parent_id".to_string())?;
    }

    let response = api::post("/uploads/simple")
        .body(JsValue::from(form))
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response.json::<FileResponse>().await.map_err(|e| e.to_string())
    } else {
        Err(format!("Upload failed (HTTP {})", response.status()))
    }
}

// --------------------------------------------------------------------------
// Trash
// --------------------------------------------------------------------------

pub async fn list_trash() -> Result<Vec<TrashItemResponse>, String> {
    let response = api::get("/trash")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response.json::<Vec<TrashItemResponse>>().await.map_err(|e| e.to_string())
    } else {
        Err("Failed to load trash".to_string())
    }
}

pub async fn restore_from_trash(id: &str, name: Option<&str>) -> Result<(), String> {
    let path = format!("/trash/{}/restore", id);

    let response = if let Some(n) = name {
        api::post(&path)
            .json(&serde_json::json!({ "name": n }))
            .map_err(|e| e.to_string())?
            .send()
            .await
            .map_err(|e| e.to_string())?
    } else {
        api::post(&path)
            .send()
            .await
            .map_err(|e| e.to_string())?
    };

    if response.ok() {
        Ok(())
    } else if response.status() == 409 {
        // Parse suggested name from response body: { "error": "CONFLICT", "suggest": "name (1).ext" }
        if let Ok(body) = response.json::<serde_json::Value>().await {
            if let Some(suggest) = body.get("suggest").and_then(|v| v.as_str()) {
                return Err(format!("CONFLICT:{}", suggest));
            }
        }
        Err("CONFLICT".to_string())
    } else {
        Err("Failed to restore item".to_string())
    }
}

pub async fn permanently_delete_trash(id: &str) -> Result<(), String> {
    let response = api::delete(&format!("/trash/{}", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        Ok(())
    } else {
        Err("Failed to permanently delete item".to_string())
    }
}

pub async fn empty_trash() -> Result<(), String> {
    let response = api::delete("/trash")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        Ok(())
    } else {
        Err("Failed to empty trash".to_string())
    }
}

// --------------------------------------------------------------------------
// Versions
// --------------------------------------------------------------------------

pub async fn list_versions(file_id: &str) -> Result<Vec<FileVersionResponse>, String> {
    let response = api::get(&format!("/files/{}/versions", file_id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response.json::<Vec<FileVersionResponse>>().await.map_err(|e| e.to_string())
    } else {
        Err("Failed to load versions".to_string())
    }
}

pub async fn restore_version(file_id: &str, version_id: &str) -> Result<(), String> {
    let response = api::post(&format!("/files/{}/versions/{}/restore", file_id, version_id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        Ok(())
    } else {
        Err("Failed to restore version".to_string())
    }
}
