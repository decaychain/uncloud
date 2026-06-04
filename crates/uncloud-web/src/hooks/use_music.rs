use uncloud_common::{
    ArtistResponse, MusicAlbumResponse, MusicFolderResponse, MusicSearchResponse,
    MusicTracksResponse, TrackResponse,
};

use super::api;

fn encode(s: &str) -> String {
    js_sys::encode_uri_component(s)
        .as_string()
        .unwrap_or_else(|| s.to_string())
}

/// Optional restriction applied to library queries (artists / albums / tracks).
#[derive(Debug, Clone, PartialEq, Default)]
pub enum LibraryScope {
    #[default]
    All,
    Folder(String),
    Category(String),
}

impl LibraryScope {
    /// Returns the query-string suffix including a leading `?`, or empty
    /// when the scope is `All`. Always safe to append to a URL with no
    /// existing query string.
    pub fn query_string(&self) -> String {
        match self {
            Self::All => String::new(),
            Self::Folder(id) => format!("?folder_id={}", encode(id)),
            Self::Category(id) => format!("?category_id={}", encode(id)),
        }
    }
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

    let response = api::get_raw(&url).send().await.map_err(|e| e.to_string())?;

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

pub async fn list_music_root_folders() -> Result<Vec<MusicFolderResponse>, String> {
    list_music_folder_url("/music/folders?root=true").await
}

pub async fn list_music_child_folders(
    parent_id: &str,
) -> Result<Vec<MusicFolderResponse>, String> {
    let url = format!("/music/folders?parent_id={}", encode(parent_id));
    list_music_folder_url(&url).await
}

pub async fn list_music_folders_by_ids(
    folder_ids: &[String],
) -> Result<Vec<MusicFolderResponse>, String> {
    if folder_ids.is_empty() {
        return Ok(Vec::new());
    }

    let ids = folder_ids
        .iter()
        .map(|id| encode(id))
        .collect::<Vec<_>>()
        .join(",");
    let url = format!("/music/folders?folder_ids={ids}");
    list_music_folder_url(&url).await
}

async fn list_music_folder_url(url: &str) -> Result<Vec<MusicFolderResponse>, String> {
    let response = api::get(url).send().await.map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<MusicFolderResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load music folders".to_string())
    }
}

pub async fn list_artists_scoped(scope: &LibraryScope) -> Result<Vec<ArtistResponse>, String> {
    let url = format!("/music/artists{}", scope.query_string());
    let response = api::get(&url).send().await.map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<ArtistResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load artists".to_string())
    }
}

pub async fn list_artist_albums_scoped(
    artist: &str,
    scope: &LibraryScope,
) -> Result<Vec<MusicAlbumResponse>, String> {
    let url = format!(
        "/music/artists/{}/albums{}",
        encode(artist),
        scope.query_string()
    );
    let response = api::get(&url).send().await.map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<MusicAlbumResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load albums".to_string())
    }
}

pub async fn list_album_tracks_scoped(
    artist: &str,
    album: &str,
    scope: &LibraryScope,
) -> Result<Vec<TrackResponse>, String> {
    let url = format!(
        "/music/albums/{}/{}/tracks{}",
        encode(artist),
        encode(album),
        scope.query_string()
    );
    let response = api::get(&url).send().await.map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<TrackResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load album tracks".to_string())
    }
}

pub async fn search_music(
    query: &str,
    scope: &LibraryScope,
    limit: Option<usize>,
) -> Result<MusicSearchResponse, String> {
    let mut params = vec![format!("q={}", encode(query))];
    match scope {
        LibraryScope::All => {}
        LibraryScope::Folder(id) => params.push(format!("folder_id={}", encode(id))),
        LibraryScope::Category(id) => params.push(format!("category_id={}", encode(id))),
    }
    if let Some(l) = limit {
        params.push(format!("limit={}", l));
    }
    let url = format!("/music/search?{}", params.join("&"));
    let response = api::get(&url).send().await.map_err(|e| e.to_string())?;
    if response.ok() {
        response
            .json::<MusicSearchResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Search failed".to_string())
    }
}
