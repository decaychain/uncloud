use uncloud_common::{
    ArtistResponse, MusicAlbumResponse, MusicFolderResponse, MusicTracksResponse, TrackResponse,
};

use super::api;

fn encode(s: &str) -> String {
    js_sys::encode_uri_component(s).as_string().unwrap_or_else(|| s.to_string())
}

pub async fn list_music_tracks(
    folder_id: Option<&str>,
    cursor: Option<&str>,
) -> Result<MusicTracksResponse, String> {
    let mut url = api::api_url("/music/tracks");
    let mut params = Vec::new();
    if let Some(fid) = folder_id {
        params.push(format!("folder_id={}", fid));
    }
    if let Some(c) = cursor {
        params.push(format!("cursor={}", c));
    }
    if !params.is_empty() {
        url = format!("{}?{}", url, params.join("&"));
    }

    let response = api::get_raw(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<MusicTracksResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load tracks".to_string())
    }
}

pub async fn list_music_folders() -> Result<Vec<MusicFolderResponse>, String> {
    let response = api::get("/music/folders")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<MusicFolderResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load music folders".to_string())
    }
}

pub async fn list_artists() -> Result<Vec<ArtistResponse>, String> {
    let response = api::get("/music/artists")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<ArtistResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load artists".to_string())
    }
}

pub async fn list_artist_albums(artist: &str) -> Result<Vec<MusicAlbumResponse>, String> {
    let response = api::get(&format!("/music/artists/{}/albums", encode(artist)))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<MusicAlbumResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load albums".to_string())
    }
}

pub async fn list_album_tracks(artist: &str, album: &str) -> Result<Vec<TrackResponse>, String> {
    let response = api::get(&format!(
        "/music/albums/{}/{}/tracks",
        encode(artist),
        encode(album)
    ))
    .send()
    .await
    .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<TrackResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load album tracks".to_string())
    }
}
