use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::Json;
use bson::doc;
use mongodb::bson::oid::ObjectId;
use serde::Deserialize;

use crate::error::{AppError, Result};
use crate::middleware::AuthUser;
use crate::models::{File, Folder};
use crate::routes::files::{build_folder_path, file_to_response, resolve_included_folder_ids_by};
use crate::AppState;
use uncloud_common::{
    ArtistResponse, AudioMeta, InheritableSetting, MusicAlbumResponse, MusicFolderResponse,
    MusicTracksResponse, TrackResponse,
};

#[derive(Debug, Deserialize)]
pub struct ListTracksQuery {
    pub folder_id: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

pub async fn list_music_tracks(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(query): Query<ListTracksQuery>,
) -> Result<Json<MusicTracksResponse>> {
    let limit = query.limit.unwrap_or(50).min(200);

    let files_coll = state.db.collection::<File>("files");

    let parent_ids: Vec<mongodb::bson::Bson> = if let Some(ref fid) = query.folder_id {
        // Scoped to a single folder
        let oid = ObjectId::parse_str(fid)
            .map_err(|_| AppError::BadRequest("Invalid folder_id".to_string()))?;
        vec![mongodb::bson::Bson::ObjectId(oid)]
    } else {
        // All music-included folders
        let folders_coll = state.db.collection::<Folder>("folders");
        let mut folder_cursor = folders_coll.find(doc! { "owner_id": user.id, "deleted_at": bson::Bson::Null }).await?;
        let mut all_folders: Vec<Folder> = Vec::new();
        while folder_cursor.advance().await? {
            all_folders.push(folder_cursor.deserialize_current()?);
        }

        let included = resolve_included_folder_ids_by(&all_folders, |f| f.music_include.as_include_flag());

        if included.is_empty() {
            return Ok(Json(MusicTracksResponse {
                tracks: Vec::new(),
                next_cursor: None,
            }));
        }

        included
            .into_iter()
            .map(|opt| match opt {
                Some(id) => mongodb::bson::Bson::ObjectId(id),
                None => mongodb::bson::Bson::Null,
            })
            .collect()
    };

    let mut filter = doc! {
        "owner_id": user.id,
        "parent_id": { "$in": &parent_ids },
        "mime_type": { "$regex": "^audio/" },
        "deleted_at": bson::Bson::Null,
    };

    if let Some(ref cursor_str) = query.cursor {
        let cursor_dt = chrono::DateTime::parse_from_rfc3339(cursor_str)
            .map_err(|_| AppError::BadRequest("Invalid cursor".to_string()))?;
        filter.insert(
            "created_at",
            doc! { "$lt": bson::DateTime::from_chrono(cursor_dt.with_timezone(&chrono::Utc)) },
        );
    }

    let options = mongodb::options::FindOptions::builder()
        .sort(doc! { "created_at": -1 })
        .limit(limit + 1)
        .build();

    let mut file_cursor = files_coll.find(filter).with_options(options).await?;

    let mut files: Vec<File> = Vec::new();
    while file_cursor.advance().await? {
        files.push(file_cursor.deserialize_current()?);
    }

    let next_cursor = if files.len() as i64 > limit {
        files.pop();
        files.last().map(|f| f.created_at.to_rfc3339())
    } else {
        None
    };

    let tracks: Vec<TrackResponse> = files
        .iter()
        .map(|f| {
            let file_resp = file_to_response(f);
            let audio: AudioMeta = file_resp
                .metadata
                .get("audio")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            TrackResponse {
                file: uncloud_common::FileResponse {
                    id: file_resp.id,
                    name: file_resp.name,
                    mime_type: file_resp.mime_type,
                    size_bytes: file_resp.size_bytes,
                    parent_id: file_resp.parent_id,
                    created_at: file_resp.created_at,
                    updated_at: file_resp.updated_at,
                    metadata: file_resp.metadata,
                },
                audio,
            }
        })
        .collect();

    Ok(Json(MusicTracksResponse {
        tracks,
        next_cursor,
    }))
}

pub async fn list_music_folders(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<MusicFolderResponse>>> {
    let folders_coll = state.db.collection::<Folder>("folders");
    let files_coll = state.db.collection::<File>("files");

    let mut folder_cursor = folders_coll.find(doc! { "owner_id": user.id, "deleted_at": bson::Bson::Null }).await?;
    let mut all_folders: Vec<Folder> = Vec::new();
    while folder_cursor.advance().await? {
        all_folders.push(folder_cursor.deserialize_current()?);
    }

    let included = resolve_included_folder_ids_by(&all_folders, |f| f.music_include.as_include_flag());

    let by_id: HashMap<ObjectId, &Folder> = all_folders.iter().map(|f| (f.id, f)).collect();
    let included_ids: HashSet<ObjectId> = included.iter().filter_map(|x| *x).collect();

    let mut result = Vec::new();
    for opt_id in &included {
        let folder_id = match opt_id {
            Some(id) => *id,
            None => continue,
        };

        let folder = match by_id.get(&folder_id) {
            Some(f) => f,
            None => continue,
        };

        let track_count = files_coll
            .count_documents(doc! {
                "owner_id": user.id,
                "parent_id": folder_id,
                "mime_type": { "$regex": "^audio/" },
                "deleted_at": bson::Bson::Null,
            })
            .await?;

        let cover = files_coll
            .find_one(doc! {
                "owner_id": user.id,
                "parent_id": folder_id,
                "mime_type": { "$regex": "^audio/" },
                "deleted_at": bson::Bson::Null,
            })
            .sort(doc! { "created_at": -1 })
            .await?;

        let parent_folder_id = folder
            .parent_id
            .filter(|pid| included_ids.contains(pid))
            .map(|pid| pid.to_hex());

        result.push(MusicFolderResponse {
            folder_id: folder_id.to_hex(),
            parent_folder_id,
            name: folder.name.clone(),
            path: build_folder_path(folder_id, &by_id),
            track_count: track_count as i64,
            cover_file_id: cover.map(|f| f.id.to_hex()),
        });
    }

    result.sort_by(|a, b| a.path.to_lowercase().cmp(&b.path.to_lowercase()));

    Ok(Json(result))
}

/// Helper: fetch all music-included folder IDs for the user, returning BSON values
/// suitable for `$in` queries. Returns an empty vec if no folders are included.
async fn music_included_parent_ids(
    state: &AppState,
    user_id: ObjectId,
) -> Result<Vec<mongodb::bson::Bson>> {
    let folders_coll = state.db.collection::<Folder>("folders");
    let mut folder_cursor = folders_coll.find(doc! { "owner_id": user_id, "deleted_at": bson::Bson::Null }).await?;
    let mut all_folders: Vec<Folder> = Vec::new();
    while folder_cursor.advance().await? {
        all_folders.push(folder_cursor.deserialize_current()?);
    }

    let included = resolve_included_folder_ids_by(&all_folders, |f| f.music_include.as_include_flag());

    Ok(included
        .into_iter()
        .map(|opt| match opt {
            Some(id) => mongodb::bson::Bson::ObjectId(id),
            None => mongodb::bson::Bson::Null,
        })
        .collect())
}

/// Helper: fetch all audio files in the given parent IDs for the user.
async fn fetch_audio_files(
    state: &AppState,
    user_id: ObjectId,
    parent_ids: &[mongodb::bson::Bson],
) -> Result<Vec<File>> {
    let files_coll = state.db.collection::<File>("files");
    let filter = doc! {
        "owner_id": user_id,
        "parent_id": { "$in": parent_ids },
        "mime_type": { "$regex": "^audio/" },
        "deleted_at": bson::Bson::Null,
    };
    let mut cursor = files_coll.find(filter).await?;
    let mut files = Vec::new();
    while cursor.advance().await? {
        files.push(cursor.deserialize_current()?);
    }
    Ok(files)
}

/// Extract audio metadata from a File document.
fn extract_audio_meta(f: &File) -> AudioMeta {
    let resp = file_to_response(f);
    resp.metadata
        .get("audio")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

pub async fn list_artists(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<ArtistResponse>>> {
    let parent_ids = music_included_parent_ids(&state, user.id).await?;
    if parent_ids.is_empty() {
        return Ok(Json(Vec::new()));
    }

    let files = fetch_audio_files(&state, user.id, &parent_ids).await?;

    // artist → (set of album names, track count)
    let mut artist_albums: HashMap<String, HashSet<String>> = HashMap::new();
    let mut artist_tracks: HashMap<String, i64> = HashMap::new();

    for f in &files {
        let audio = extract_audio_meta(f);
        let artist = audio
            .artist
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("Unknown Artist")
            .to_string();
        let album = audio
            .album
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("Unknown Album")
            .to_string();

        artist_albums
            .entry(artist.clone())
            .or_default()
            .insert(album);
        *artist_tracks.entry(artist).or_default() += 1;
    }

    let mut result: Vec<ArtistResponse> = artist_albums
        .into_iter()
        .map(|(name, albums)| ArtistResponse {
            track_count: artist_tracks.get(&name).copied().unwrap_or(0),
            album_count: albums.len() as i64,
            name,
        })
        .collect();

    result.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    Ok(Json(result))
}

pub async fn list_artist_albums(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    axum::extract::Path(artist_name): axum::extract::Path<String>,
) -> Result<Json<Vec<MusicAlbumResponse>>> {
    let parent_ids = music_included_parent_ids(&state, user.id).await?;
    if parent_ids.is_empty() {
        return Ok(Json(Vec::new()));
    }

    let files_coll = state.db.collection::<File>("files");
    let filter = doc! {
        "owner_id": user.id,
        "parent_id": { "$in": &parent_ids },
        "mime_type": { "$regex": "^audio/" },
        "deleted_at": bson::Bson::Null,
        "$or": [
            { "metadata.audio.artist": &artist_name },
            { "metadata.audio.album_artist": &artist_name },
        ],
    };
    let mut cursor = files_coll.find(filter).await?;
    let mut files: Vec<File> = Vec::new();
    while cursor.advance().await? {
        files.push(cursor.deserialize_current()?);
    }

    // Also include files with missing/null/empty artist if artist_name is "Unknown Artist"
    // Note: { field: null } matches both null values and missing fields in MongoDB
    if artist_name == "Unknown Artist" {
        let unknown_filter = doc! {
            "owner_id": user.id,
            "parent_id": { "$in": &parent_ids },
            "mime_type": { "$regex": "^audio/" },
            "deleted_at": bson::Bson::Null,
            "$or": [
                { "metadata.audio.artist": mongodb::bson::Bson::Null },
                { "metadata.audio.artist": "" },
            ],
        };
        let mut unknown_cursor = files_coll.find(unknown_filter).await?;
        while unknown_cursor.advance().await? {
            files.push(unknown_cursor.deserialize_current()?);
        }
    }

    // Group by album
    struct AlbumInfo {
        year: Option<i32>,
        track_count: i64,
        cover_file_id: Option<String>,
    }

    let mut albums: HashMap<String, AlbumInfo> = HashMap::new();

    for f in &files {
        let audio = extract_audio_meta(f);
        let album_name = audio
            .album
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("Unknown Album")
            .to_string();

        let entry = albums.entry(album_name).or_insert_with(|| AlbumInfo {
            year: audio.year,
            track_count: 0,
            cover_file_id: None,
        });

        entry.track_count += 1;

        // Only set cover_file_id when the track actually has embedded art
        if audio.has_cover_art && entry.cover_file_id.is_none() {
            entry.cover_file_id = Some(f.id.to_hex());
        }

        // Keep year if not set yet
        if entry.year.is_none() {
            entry.year = audio.year;
        }
    }

    let mut result: Vec<MusicAlbumResponse> = albums
        .into_iter()
        .map(|(name, info)| {
            let cover = info.cover_file_id;
            MusicAlbumResponse {
                name,
                artist: artist_name.clone(),
                year: info.year,
                track_count: info.track_count,
                cover_file_id: cover,
            }
        })
        .collect();

    // Sort by year asc (nulls last), then album name
    result.sort_by(|a, b| {
        let year_cmp = match (a.year, b.year) {
            (Some(ya), Some(yb)) => ya.cmp(&yb),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        };
        year_cmp.then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(Json(result))
}

pub async fn list_album_tracks(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    axum::extract::Path((artist_name, album_name)): axum::extract::Path<(String, String)>,
) -> Result<Json<Vec<TrackResponse>>> {
    let parent_ids = music_included_parent_ids(&state, user.id).await?;
    if parent_ids.is_empty() {
        return Ok(Json(Vec::new()));
    }

    let files_coll = state.db.collection::<File>("files");

    let mut filters = vec![
        doc! { "owner_id": user.id },
        doc! { "parent_id": { "$in": &parent_ids } },
        doc! { "mime_type": { "$regex": "^audio/" } },
        doc! { "deleted_at": bson::Bson::Null },
    ];

    // Album filter — handle "Unknown Album" by matching missing/null/empty album
    // Note: { field: null } matches both null values and missing fields in MongoDB
    if album_name == "Unknown Album" {
        filters.push(doc! {
            "$or": [
                { "metadata.audio.album": mongodb::bson::Bson::Null },
                { "metadata.audio.album": "" },
            ]
        });
    } else {
        filters.push(doc! { "metadata.audio.album": &album_name });
    }

    // Artist filter — handle "Unknown Artist" similarly
    if artist_name == "Unknown Artist" {
        filters.push(doc! {
            "$or": [
                { "metadata.audio.artist": mongodb::bson::Bson::Null },
                { "metadata.audio.artist": "" },
            ]
        });
    } else {
        filters.push(doc! {
            "$or": [
                { "metadata.audio.artist": &artist_name },
                { "metadata.audio.album_artist": &artist_name },
            ]
        });
    }

    let filter = doc! { "$and": filters };
    let mut cursor = files_coll.find(filter).await?;
    let mut files: Vec<File> = Vec::new();
    while cursor.advance().await? {
        files.push(cursor.deserialize_current()?);
    }

    // Sort by disc number, then track number
    files.sort_by(|a, b| {
        let audio_a = extract_audio_meta(a);
        let audio_b = extract_audio_meta(b);
        let disc_a = audio_a.disc_number.unwrap_or(0);
        let disc_b = audio_b.disc_number.unwrap_or(0);
        let track_a = audio_a.track_number.unwrap_or(0);
        let track_b = audio_b.track_number.unwrap_or(0);
        disc_a.cmp(&disc_b).then(track_a.cmp(&track_b))
    });

    let tracks: Vec<TrackResponse> = files
        .iter()
        .map(|f| {
            let file_resp = file_to_response(f);
            let audio: AudioMeta = file_resp
                .metadata
                .get("audio")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            TrackResponse {
                file: uncloud_common::FileResponse {
                    id: file_resp.id,
                    name: file_resp.name,
                    mime_type: file_resp.mime_type,
                    size_bytes: file_resp.size_bytes,
                    parent_id: file_resp.parent_id,
                    created_at: file_resp.created_at,
                    updated_at: file_resp.updated_at,
                    metadata: file_resp.metadata,
                },
                audio,
            }
        })
        .collect();

    Ok(Json(tracks))
}
