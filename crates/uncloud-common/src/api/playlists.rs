use serde::{Deserialize, Serialize};
use super::music::TrackResponse;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlaylistSummary {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub track_count: usize,
    /// File ID of the first track with cover art, for thumbnail display.
    pub cover_file_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlaylistResponse {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub tracks: Vec<TrackResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatePlaylistRequest {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdatePlaylistRequest {
    pub name: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddTracksRequest {
    pub file_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoveTracksRequest {
    pub file_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReorderTracksRequest {
    /// New full order of track file IDs.
    pub file_ids: Vec<String>,
}
