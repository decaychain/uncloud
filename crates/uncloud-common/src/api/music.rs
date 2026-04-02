use serde::{Deserialize, Serialize};
use super::files::FileResponse;

/// Typed convenience struct for the `metadata["audio"]` key on FileResponse.
/// The processor stores this shape under `file.metadata["audio"]`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioMeta {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub year: Option<i32>,
    pub genre: Option<String>,
    pub duration_secs: Option<f64>,
    #[serde(default)]
    pub has_cover_art: bool,
}

/// A file enriched with top-level audio metadata fields for convenience.
/// The same data is also available via `file.metadata["audio"]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackResponse {
    #[serde(flatten)]
    pub file: FileResponse,
    #[serde(flatten)]
    pub audio: AudioMeta,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MusicTracksResponse {
    pub tracks: Vec<TrackResponse>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MusicFolderResponse {
    pub folder_id: String,
    /// `Some(id)` when the immediate parent is also a music-library folder.
    pub parent_folder_id: Option<String>,
    pub name: String,
    /// Breadcrumb path, e.g. "Music / Jazz / Miles Davis"
    pub path: String,
    pub track_count: i64,
    /// File ID of the most recent audio file for cover display.
    pub cover_file_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtistResponse {
    pub name: String,
    pub album_count: i64,
    pub track_count: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MusicAlbumResponse {
    pub name: String,
    pub artist: String,
    pub year: Option<i32>,
    pub track_count: i64,
    /// File ID of any track in the album whose `has_cover_art` is true.
    pub cover_file_id: Option<String>,
}
