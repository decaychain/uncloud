use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use bson::doc;
use chrono::Utc;
use mongodb::bson::oid::ObjectId;

use crate::error::{AppError, Result};
use crate::middleware::AuthUser;
use crate::models::{File, Playlist, PlaylistTrack};
use crate::routes::files::file_to_response;
use crate::AppState;
use uncloud_common::{
    AddTracksRequest, AudioMeta, CreatePlaylistRequest, PlaylistResponse, PlaylistSummary,
    RemoveTracksRequest, ReorderTracksRequest, TrackResponse, UpdatePlaylistRequest,
};

/// Convert a File into a TrackResponse (same pattern as routes/music.rs).
fn file_to_track(f: &File) -> TrackResponse {
    let file_resp = file_to_response(f);
    let audio: AudioMeta = file_resp
        .metadata
        .get("audio")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    TrackResponse {
        file: file_resp,
        audio,
    }
}

/// Build a PlaylistSummary from a Playlist document, looking up cover art info
/// from a pre-fetched map of file_id -> File.
fn playlist_to_summary(
    playlist: &Playlist,
    files_by_id: &HashMap<ObjectId, File>,
) -> PlaylistSummary {
    // Find first track with cover art
    let cover_file_id = playlist
        .tracks
        .iter()
        .find_map(|t| {
            let f = files_by_id.get(&t.file_id)?;
            let audio: AudioMeta = file_to_response(f)
                .metadata
                .get("audio")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            if audio.has_cover_art {
                Some(f.id.to_hex())
            } else {
                None
            }
        });

    PlaylistSummary {
        id: playlist.id.to_hex(),
        name: playlist.name.clone(),
        description: playlist.description.clone(),
        track_count: playlist.tracks.len(),
        cover_file_id,
    }
}

/// `GET /api/playlists`
pub async fn list_playlists(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<PlaylistSummary>>> {
    let coll = state.db.collection::<Playlist>("playlists");
    let options = mongodb::options::FindOptions::builder()
        .sort(doc! { "updated_at": -1 })
        .build();
    let mut cursor = coll
        .find(doc! { "owner_id": user.id })
        .with_options(options)
        .await?;

    let mut playlists: Vec<Playlist> = Vec::new();
    while cursor.advance().await? {
        playlists.push(cursor.deserialize_current()?);
    }

    // Collect all unique file IDs referenced by playlists for batch lookup
    let all_file_ids: Vec<ObjectId> = playlists
        .iter()
        .flat_map(|p| p.tracks.iter().map(|t| t.file_id))
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    let files_by_id = if all_file_ids.is_empty() {
        HashMap::new()
    } else {
        let files_coll = state.db.collection::<File>("files");
        let bson_ids: Vec<bson::Bson> = all_file_ids
            .iter()
            .map(|id| bson::Bson::ObjectId(*id))
            .collect();
        let mut file_cursor = files_coll
            .find(doc! { "_id": { "$in": &bson_ids }, "owner_id": user.id })
            .await?;
        let mut map = HashMap::new();
        while file_cursor.advance().await? {
            let f: File = file_cursor.deserialize_current()?;
            map.insert(f.id, f);
        }
        map
    };

    let summaries: Vec<PlaylistSummary> = playlists
        .iter()
        .map(|p| playlist_to_summary(p, &files_by_id))
        .collect();

    Ok(Json(summaries))
}

/// `POST /api/playlists`
pub async fn create_playlist(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<CreatePlaylistRequest>,
) -> Result<(StatusCode, Json<PlaylistSummary>)> {
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::BadRequest("Playlist name cannot be empty".to_string()));
    }

    let coll = state.db.collection::<Playlist>("playlists");

    // Check uniqueness by (owner_id, name)
    let existing = coll
        .find_one(doc! { "owner_id": user.id, "name": &name })
        .await?;
    if existing.is_some() {
        return Err(AppError::Conflict(format!(
            "A playlist named \"{}\" already exists",
            name
        )));
    }

    let mut playlist = Playlist::new(user.id, name);
    playlist.description = body.description;

    coll.insert_one(&playlist).await?;

    let summary = PlaylistSummary {
        id: playlist.id.to_hex(),
        name: playlist.name,
        description: playlist.description,
        track_count: 0,
        cover_file_id: None,
    };

    Ok((StatusCode::CREATED, Json(summary)))
}

/// `GET /api/playlists/{id}`
pub async fn get_playlist(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<PlaylistResponse>> {
    let playlist_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid playlist ID".to_string()))?;

    let coll = state.db.collection::<Playlist>("playlists");
    let playlist = coll
        .find_one(doc! { "_id": playlist_id, "owner_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Playlist".to_string()))?;

    // Batch-fetch all referenced files
    let file_ids: Vec<bson::Bson> = playlist
        .tracks
        .iter()
        .map(|t| bson::Bson::ObjectId(t.file_id))
        .collect();

    let files_by_id = if file_ids.is_empty() {
        HashMap::new()
    } else {
        let files_coll = state.db.collection::<File>("files");
        let mut file_cursor = files_coll
            .find(doc! { "_id": { "$in": &file_ids }, "owner_id": user.id })
            .await?;
        let mut map = HashMap::new();
        while file_cursor.advance().await? {
            let f: File = file_cursor.deserialize_current()?;
            map.insert(f.id, f);
        }
        map
    };

    // Build tracks in playlist order, skipping any that no longer exist
    let tracks: Vec<TrackResponse> = playlist
        .tracks
        .iter()
        .filter_map(|t| files_by_id.get(&t.file_id).map(file_to_track))
        .collect();

    Ok(Json(PlaylistResponse {
        id: playlist.id.to_hex(),
        name: playlist.name,
        description: playlist.description,
        tracks,
    }))
}

/// `PUT /api/playlists/{id}`
pub async fn update_playlist(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<UpdatePlaylistRequest>,
) -> Result<Json<PlaylistSummary>> {
    let playlist_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid playlist ID".to_string()))?;

    let coll = state.db.collection::<Playlist>("playlists");
    let playlist = coll
        .find_one(doc! { "_id": playlist_id, "owner_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Playlist".to_string()))?;

    let mut update_doc = doc! {};
    if let Some(ref name) = body.name {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::BadRequest("Playlist name cannot be empty".to_string()));
        }
        // Check uniqueness if name is changing
        if name != playlist.name {
            let existing = coll
                .find_one(doc! { "owner_id": user.id, "name": name, "_id": { "$ne": playlist_id } })
                .await?;
            if existing.is_some() {
                return Err(AppError::Conflict(format!(
                    "A playlist named \"{}\" already exists",
                    name
                )));
            }
        }
        update_doc.insert("name", name);
    }
    if let Some(ref desc) = body.description {
        update_doc.insert("description", desc);
    }

    if update_doc.is_empty() {
        // Nothing to update — return current state
        let summary = PlaylistSummary {
            id: playlist.id.to_hex(),
            name: playlist.name,
            description: playlist.description,
            track_count: playlist.tracks.len(),
            cover_file_id: None,
        };
        return Ok(Json(summary));
    }

    update_doc.insert("updated_at", bson::DateTime::from_chrono(Utc::now()));

    coll.update_one(
        doc! { "_id": playlist_id, "owner_id": user.id },
        doc! { "$set": update_doc },
    )
    .await?;

    // Re-fetch for response
    let updated = coll
        .find_one(doc! { "_id": playlist_id, "owner_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Playlist".to_string()))?;

    let summary = PlaylistSummary {
        id: updated.id.to_hex(),
        name: updated.name,
        description: updated.description,
        track_count: updated.tracks.len(),
        cover_file_id: None,
    };

    Ok(Json(summary))
}

/// `DELETE /api/playlists/{id}`
pub async fn delete_playlist(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let playlist_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid playlist ID".to_string()))?;

    let coll = state.db.collection::<Playlist>("playlists");
    let result = coll
        .delete_one(doc! { "_id": playlist_id, "owner_id": user.id })
        .await?;

    if result.deleted_count == 0 {
        return Err(AppError::NotFound("Playlist".to_string()));
    }

    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/playlists/{id}/tracks`
pub async fn add_tracks(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<AddTracksRequest>,
) -> Result<StatusCode> {
    let playlist_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid playlist ID".to_string()))?;

    let coll = state.db.collection::<Playlist>("playlists");
    let playlist = coll
        .find_one(doc! { "_id": playlist_id, "owner_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Playlist".to_string()))?;

    // Parse and verify file IDs belong to the user
    let files_coll = state.db.collection::<File>("files");
    let mut new_tracks: Vec<PlaylistTrack> = Vec::new();
    let mut next_pos = playlist
        .tracks
        .iter()
        .map(|t| t.position)
        .max()
        .unwrap_or(0);

    let now = Utc::now();

    for fid_str in &body.file_ids {
        let file_id = ObjectId::parse_str(fid_str)
            .map_err(|_| AppError::BadRequest(format!("Invalid file ID: {}", fid_str)))?;

        // Verify file belongs to user
        let exists = files_coll
            .find_one(doc! { "_id": file_id, "owner_id": user.id })
            .await?;
        if exists.is_none() {
            return Err(AppError::NotFound(format!("File {}", fid_str)));
        }

        // Skip if already in playlist
        if playlist.tracks.iter().any(|t| t.file_id == file_id) {
            continue;
        }

        next_pos += 1;
        new_tracks.push(PlaylistTrack {
            file_id,
            position: next_pos,
            added_at: now,
        });
    }

    if !new_tracks.is_empty() {
        let bson_tracks: Vec<bson::Bson> = new_tracks
            .iter()
            .map(|t| bson::to_bson(t).unwrap())
            .collect();

        coll.update_one(
            doc! { "_id": playlist_id, "owner_id": user.id },
            doc! {
                "$push": { "tracks": { "$each": bson_tracks } },
                "$set": { "updated_at": bson::DateTime::from_chrono(Utc::now()) }
            },
        )
        .await?;
    }

    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/playlists/{id}/tracks`
pub async fn remove_tracks(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<RemoveTracksRequest>,
) -> Result<StatusCode> {
    let playlist_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid playlist ID".to_string()))?;

    let coll = state.db.collection::<Playlist>("playlists");

    // Verify playlist belongs to user
    let _playlist = coll
        .find_one(doc! { "_id": playlist_id, "owner_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Playlist".to_string()))?;

    // Parse file IDs to remove
    let remove_ids: Vec<ObjectId> = body
        .file_ids
        .iter()
        .filter_map(|s| ObjectId::parse_str(s).ok())
        .collect();

    if remove_ids.is_empty() {
        return Ok(StatusCode::NO_CONTENT);
    }

    let bson_ids: Vec<bson::Bson> = remove_ids
        .iter()
        .map(|id| bson::Bson::ObjectId(*id))
        .collect();

    // Pull matching tracks
    coll.update_one(
        doc! { "_id": playlist_id, "owner_id": user.id },
        doc! {
            "$pull": { "tracks": { "file_id": { "$in": &bson_ids } } },
            "$set": { "updated_at": bson::DateTime::from_chrono(Utc::now()) }
        },
    )
    .await?;

    // Renumber positions
    let updated = coll
        .find_one(doc! { "_id": playlist_id, "owner_id": user.id })
        .await?;
    if let Some(mut pl) = updated {
        for (i, track) in pl.tracks.iter_mut().enumerate() {
            track.position = (i + 1) as u32;
        }
        let bson_tracks: Vec<bson::Bson> = pl
            .tracks
            .iter()
            .map(|t| bson::to_bson(t).unwrap())
            .collect();
        coll.update_one(
            doc! { "_id": playlist_id, "owner_id": user.id },
            doc! { "$set": { "tracks": bson_tracks } },
        )
        .await?;
    }

    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /api/playlists/{id}/tracks/reorder`
pub async fn reorder_tracks(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<ReorderTracksRequest>,
) -> Result<StatusCode> {
    let playlist_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid playlist ID".to_string()))?;

    let coll = state.db.collection::<Playlist>("playlists");
    let playlist = coll
        .find_one(doc! { "_id": playlist_id, "owner_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Playlist".to_string()))?;

    // Build a map of existing tracks by file_id
    let existing: HashMap<ObjectId, PlaylistTrack> = playlist
        .tracks
        .into_iter()
        .map(|t| (t.file_id, t))
        .collect();

    // Rebuild in new order
    let now = Utc::now();
    let mut new_tracks: Vec<PlaylistTrack> = Vec::new();
    for (i, fid_str) in body.file_ids.iter().enumerate() {
        let file_id = ObjectId::parse_str(fid_str)
            .map_err(|_| AppError::BadRequest(format!("Invalid file ID: {}", fid_str)))?;

        if let Some(mut track) = existing.get(&file_id).cloned() {
            track.position = (i + 1) as u32;
            new_tracks.push(track);
        }
        // Skip file_ids not in the current playlist
    }

    let bson_tracks: Vec<bson::Bson> = new_tracks
        .iter()
        .map(|t| bson::to_bson(t).unwrap())
        .collect();

    coll.update_one(
        doc! { "_id": playlist_id, "owner_id": user.id },
        doc! {
            "$set": {
                "tracks": bson_tracks,
                "updated_at": bson::DateTime::from_chrono(now)
            }
        },
    )
    .await?;

    Ok(StatusCode::NO_CONTENT)
}
