use uncloud_common::{
    AddTracksRequest, CreatePlaylistRequest, PlaylistResponse, PlaylistSummary,
    RemoveTracksRequest, ReorderTracksRequest, UpdatePlaylistRequest,
};

use super::api;

pub async fn list_playlists() -> Result<Vec<PlaylistSummary>, String> {
    let response = api::get("/playlists")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<PlaylistSummary>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load playlists".to_string())
    }
}

pub async fn create_playlist(
    name: &str,
    description: Option<&str>,
) -> Result<PlaylistSummary, String> {
    let body = CreatePlaylistRequest {
        name: name.to_string(),
        description: description.map(|s| s.to_string()),
    };
    let response = api::post("/playlists")
        .json(&body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 201 {
        response
            .json::<PlaylistSummary>()
            .await
            .map_err(|e| e.to_string())
    } else if response.status() == 409 {
        Err("CONFLICT".to_string())
    } else {
        Err("Failed to create playlist".to_string())
    }
}

pub async fn get_playlist(id: &str) -> Result<PlaylistResponse, String> {
    let response = api::get(&format!("/playlists/{}", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<PlaylistResponse>()
            .await
            .map_err(|e| e.to_string())
    } else if response.status() == 404 {
        Err("Playlist not found".to_string())
    } else {
        Err("Failed to load playlist".to_string())
    }
}

pub async fn update_playlist(
    id: &str,
    name: Option<&str>,
    description: Option<&str>,
) -> Result<PlaylistSummary, String> {
    let body = UpdatePlaylistRequest {
        name: name.map(|s| s.to_string()),
        description: description.map(|s| s.to_string()),
    };
    let response = api::put(&format!("/playlists/{}", id))
        .json(&body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<PlaylistSummary>()
            .await
            .map_err(|e| e.to_string())
    } else if response.status() == 409 {
        Err("CONFLICT".to_string())
    } else {
        Err("Failed to update playlist".to_string())
    }
}

pub async fn delete_playlist(id: &str) -> Result<(), String> {
    let response = api::delete(&format!("/playlists/{}", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to delete playlist".to_string())
    }
}

pub async fn add_to_playlist(playlist_id: &str, file_ids: &[&str]) -> Result<(), String> {
    let body = AddTracksRequest {
        file_ids: file_ids.iter().map(|s| s.to_string()).collect(),
    };
    let response = api::post(&format!("/playlists/{}/tracks", playlist_id))
        .json(&body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to add tracks".to_string())
    }
}

pub async fn remove_from_playlist(playlist_id: &str, file_ids: &[&str]) -> Result<(), String> {
    let body = RemoveTracksRequest {
        file_ids: file_ids.iter().map(|s| s.to_string()).collect(),
    };
    let response = api::delete(&format!("/playlists/{}/tracks", playlist_id))
        .json(&body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to remove tracks".to_string())
    }
}

pub async fn reorder_playlist(playlist_id: &str, file_ids: &[&str]) -> Result<(), String> {
    let body = ReorderTracksRequest {
        file_ids: file_ids.iter().map(|s| s.to_string()).collect(),
    };
    let response = api::put(&format!("/playlists/{}/tracks/reorder", playlist_id))
        .json(&body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to reorder playlist".to_string())
    }
}
