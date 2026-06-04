use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use bson::doc;
use chrono::Utc;
use mongodb::bson::oid::ObjectId;
use serde::Deserialize;

use crate::error::{AppError, Result};
use crate::middleware::AuthUser;
use crate::models::{File, Folder, FolderShare, MusicCategory};
use crate::routes::files::{build_folder_path, file_to_response, resolve_included_folder_ids_by};
use crate::services::sync_log::escape_regex;
use crate::AppState;
use uncloud_common::{
    ArtistResponse, AudioMeta, CreateMusicCategoryRequest, InheritableSetting, MusicAlbumResponse,
    MusicCategory as MusicCategoryDto, MusicFolderResponse, MusicSearchResponse,
    MusicTracksResponse, TrackResponse, UpdateMusicCategoryRequest,
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
        // All music-included folders (owned + shared)
        let ids = music_included_parent_ids(&state, user.id).await?;
        if ids.is_empty() {
            return Ok(Json(MusicTracksResponse {
                tracks: Vec::new(),
                next_cursor: None,
            }));
        }
        ids
    };

    let mut filter = doc! {
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
                file: file_resp,
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
    use futures::TryStreamExt;

    let folders_coll = state.db.collection::<Folder>("folders");
    let files_coll = state.db.collection::<File>("files");

    // --- Owned folders ---
    let mut folder_cursor = folders_coll
        .find(doc! { "owner_id": user.id, "deleted_at": bson::Bson::Null })
        .await?;
    let mut all_folders: Vec<Folder> = Vec::new();
    while folder_cursor.advance().await? {
        all_folders.push(folder_cursor.deserialize_current()?);
    }

    let included =
        resolve_included_folder_ids_by(&all_folders, |f| f.music_include.as_include_flag());

    let by_id: HashMap<ObjectId, &Folder> = all_folders.iter().map(|f| (f.id, f)).collect();
    let included_ids: HashSet<ObjectId> = included.iter().filter_map(|x| *x).collect();

    // --- Shared folders marked for music inclusion ---
    // Gather these up-front so the track-count/cover aggregation can include
    // their folder ids in one pass.
    let shares_coll = state.db.collection::<FolderShare>("folder_shares");
    let shares: Vec<FolderShare> = shares_coll
        .find(doc! { "grantee_id": user.id, "music_include": "include" })
        .await?
        .try_collect()
        .await?;

    // Map of owner_id → all their folders (loaded once per distinct owner).
    let mut owner_folders_cache: HashMap<ObjectId, Vec<Folder>> = HashMap::new();
    let mut shared_folder_ids: HashSet<ObjectId> = HashSet::new();

    for share in &shares {
        if !owner_folders_cache.contains_key(&share.owner_id) {
            let mut cursor = folders_coll
                .find(doc! { "owner_id": share.owner_id, "deleted_at": bson::Bson::Null })
                .await?;
            let mut owner_folders = Vec::new();
            while cursor.advance().await? {
                owner_folders.push(cursor.deserialize_current()?);
            }
            owner_folders_cache.insert(share.owner_id, owner_folders);
        }

        let owner_folders = owner_folders_cache.get(&share.owner_id).unwrap();

        let mut children_map: HashMap<ObjectId, Vec<ObjectId>> = HashMap::new();
        for f in owner_folders {
            if let Some(pid) = f.parent_id {
                children_map.entry(pid).or_default().push(f.id);
            }
        }

        shared_folder_ids.insert(share.folder_id);
        let mut stack = vec![share.folder_id];
        while let Some(fid) = stack.pop() {
            if let Some(children) = children_map.get(&fid) {
                for &child_id in children {
                    shared_folder_ids.insert(child_id);
                    stack.push(child_id);
                }
            }
        }
    }

    // Single aggregation for track counts + cover art across ALL displayed
    // folder ids (owned + shared). Replaces N × (count_documents + find_one)
    // sequential round-trips with one query.
    let mut all_target_ids: Vec<ObjectId> = included_ids.iter().copied().collect();
    all_target_ids.extend(shared_folder_ids.iter().copied());
    let stats = batch_audio_folder_stats(&files_coll, &all_target_ids).await?;

    let empty = FolderAudioStats::default();
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

        let s = stats.get(&folder_id).unwrap_or(&empty);

        let parent_folder_id = folder
            .parent_id
            .filter(|pid| included_ids.contains(pid))
            .map(|pid| pid.to_hex());

        result.push(MusicFolderResponse {
            folder_id: folder_id.to_hex(),
            parent_folder_id,
            name: folder.name.clone(),
            path: build_folder_path(folder_id, &by_id),
            track_count: s.track_count,
            cover_file_id: s.cover_file_id.map(|id| id.to_hex()),
        });
    }

    // Build folder responses for shared folders
    for (_, owner_folders) in &owner_folders_cache {
        let shared_by_id: HashMap<ObjectId, &Folder> =
            owner_folders.iter().map(|f| (f.id, f)).collect();
        for folder in owner_folders {
            if !shared_folder_ids.contains(&folder.id) {
                continue;
            }

            let s = stats.get(&folder.id).unwrap_or(&empty);

            let parent_folder_id = folder
                .parent_id
                .filter(|pid| shared_folder_ids.contains(pid))
                .map(|pid| pid.to_hex());

            result.push(MusicFolderResponse {
                folder_id: folder.id.to_hex(),
                parent_folder_id,
                name: folder.name.clone(),
                path: build_folder_path(folder.id, &shared_by_id),
                track_count: s.track_count,
                cover_file_id: s.cover_file_id.map(|id| id.to_hex()),
            });
        }
    }

    result.sort_by(|a, b| a.path.to_lowercase().cmp(&b.path.to_lowercase()));

    Ok(Json(result))
}

#[derive(Default, Clone)]
struct FolderAudioStats {
    track_count: i64,
    cover_file_id: Option<ObjectId>,
}

/// Return track-count and most-recent cover file per folder in one aggregation
/// query. Folders with no audio files are absent from the map.
async fn batch_audio_folder_stats(
    files_coll: &mongodb::Collection<File>,
    folder_ids: &[ObjectId],
) -> Result<HashMap<ObjectId, FolderAudioStats>> {
    use futures::StreamExt;

    if folder_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let pipeline = vec![
        doc! {
            "$match": {
                "parent_id": { "$in": folder_ids },
                "mime_type": { "$regex": "^audio/" },
                "deleted_at": bson::Bson::Null,
            }
        },
        doc! { "$sort": { "created_at": -1 } },
        doc! {
            "$group": {
                "_id": "$parent_id",
                "count": { "$sum": 1 },
                "cover": { "$first": "$_id" },
            }
        },
    ];

    let mut cursor = files_coll.aggregate(pipeline).await?;
    let mut out: HashMap<ObjectId, FolderAudioStats> = HashMap::new();

    while let Some(doc) = cursor.next().await {
        let doc = doc?;
        if let Ok(id) = doc.get_object_id("_id") {
            let count = doc
                .get_i32("count")
                .map(|n| n as i64)
                .or_else(|_| doc.get_i64("count"))
                .unwrap_or(0);
            let cover = doc.get_object_id("cover").ok();
            out.insert(
                id,
                FolderAudioStats {
                    track_count: count,
                    cover_file_id: cover,
                },
            );
        }
    }

    Ok(out)
}

/// Helper: collect folder IDs from shared folders where the grantee has set
/// `music_include` to `Include`. For each such share, includes the shared folder
/// and all its (non-deleted) subfolders recursively.
async fn shared_music_folder_ids(
    state: &AppState,
    user_id: ObjectId,
) -> Result<Vec<mongodb::bson::Bson>> {
    use futures::TryStreamExt;

    let shares_coll = state.db.collection::<FolderShare>("folder_shares");
    let shares: Vec<FolderShare> = shares_coll
        .find(doc! { "grantee_id": user_id, "music_include": "include" })
        .await?
        .try_collect()
        .await?;

    if shares.is_empty() {
        return Ok(Vec::new());
    }

    let folders_coll = state.db.collection::<Folder>("folders");
    let mut result = Vec::new();

    for share in &shares {
        // Include the shared folder itself
        result.push(mongodb::bson::Bson::ObjectId(share.folder_id));

        // Load ALL non-deleted folders owned by the share owner, then find
        // descendants of the shared folder.
        let mut cursor = folders_coll
            .find(doc! { "owner_id": share.owner_id, "deleted_at": bson::Bson::Null })
            .await?;
        let mut owner_folders: Vec<Folder> = Vec::new();
        while cursor.advance().await? {
            owner_folders.push(cursor.deserialize_current()?);
        }

        // Build parent→children map and BFS from the shared folder
        let mut children_map: HashMap<ObjectId, Vec<ObjectId>> = HashMap::new();
        for f in &owner_folders {
            if let Some(pid) = f.parent_id {
                children_map.entry(pid).or_default().push(f.id);
            }
        }

        let mut stack = vec![share.folder_id];
        while let Some(fid) = stack.pop() {
            if let Some(children) = children_map.get(&fid) {
                for &child_id in children {
                    result.push(mongodb::bson::Bson::ObjectId(child_id));
                    stack.push(child_id);
                }
            }
        }
    }

    Ok(result)
}

/// Helper: fetch all music-included folder IDs for the user (owned + shared),
/// returning BSON values suitable for `$in` queries.
async fn music_included_parent_ids(
    state: &AppState,
    user_id: ObjectId,
) -> Result<Vec<mongodb::bson::Bson>> {
    let folders_coll = state.db.collection::<Folder>("folders");
    let mut folder_cursor = folders_coll
        .find(doc! { "owner_id": user_id, "deleted_at": bson::Bson::Null })
        .await?;
    let mut all_folders: Vec<Folder> = Vec::new();
    while folder_cursor.advance().await? {
        all_folders.push(folder_cursor.deserialize_current()?);
    }

    let included =
        resolve_included_folder_ids_by(&all_folders, |f| f.music_include.as_include_flag());

    let mut result: Vec<mongodb::bson::Bson> = included
        .into_iter()
        .map(|opt| match opt {
            Some(id) => mongodb::bson::Bson::ObjectId(id),
            None => mongodb::bson::Bson::Null,
        })
        .collect();

    // Also include shared folders marked for music inclusion
    let shared = shared_music_folder_ids(state, user_id).await?;
    result.extend(shared);

    Ok(result)
}

/// Extract audio metadata from a File document.
fn extract_audio_meta(f: &File) -> AudioMeta {
    let resp = file_to_response(f);
    resp.metadata
        .get("audio")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

/// Restrict `parent_ids` (full music-included set) to those whose folder is
/// inside one of `scope_roots` or any of its descendants. Walks the parent→
/// children chains for owned + shared-owner folders so subtrees of shared
/// folders work too.
async fn restrict_to_subtrees(
    state: &AppState,
    user_id: ObjectId,
    parent_ids: Vec<mongodb::bson::Bson>,
    scope_roots: &[ObjectId],
) -> Result<Vec<mongodb::bson::Bson>> {
    use futures::TryStreamExt;

    if scope_roots.is_empty() {
        return Ok(Vec::new());
    }

    let folders_coll = state.db.collection::<Folder>("folders");

    let mut owner_ids: HashSet<ObjectId> = HashSet::from([user_id]);
    let shares_coll = state.db.collection::<FolderShare>("folder_shares");
    let shares: Vec<FolderShare> = shares_coll
        .find(doc! { "grantee_id": user_id, "music_include": "include" })
        .await?
        .try_collect()
        .await?;
    for s in &shares {
        owner_ids.insert(s.owner_id);
    }

    let owner_bson: Vec<mongodb::bson::Bson> = owner_ids
        .iter()
        .copied()
        .map(mongodb::bson::Bson::ObjectId)
        .collect();
    let mut cursor = folders_coll
        .find(doc! { "owner_id": { "$in": &owner_bson }, "deleted_at": bson::Bson::Null })
        .await?;
    let mut all_folders: Vec<Folder> = Vec::new();
    while cursor.advance().await? {
        all_folders.push(cursor.deserialize_current()?);
    }

    let mut children_map: HashMap<ObjectId, Vec<ObjectId>> = HashMap::new();
    for f in &all_folders {
        if let Some(pid) = f.parent_id {
            children_map.entry(pid).or_default().push(f.id);
        }
    }

    let mut allowed: HashSet<ObjectId> = HashSet::new();
    for &root in scope_roots {
        if !allowed.insert(root) {
            continue;
        }
        let mut stack = vec![root];
        while let Some(fid) = stack.pop() {
            if let Some(children) = children_map.get(&fid) {
                for &c in children {
                    if allowed.insert(c) {
                        stack.push(c);
                    }
                }
            }
        }
    }

    Ok(parent_ids
        .into_iter()
        .filter(|b| match b {
            mongodb::bson::Bson::ObjectId(oid) => allowed.contains(oid),
            _ => false,
        })
        .collect())
}

/// Resolve a (folder_id, category_id) pair into a list of root folder
/// ObjectIds to scope by. Validates that the user owns the category.
async fn resolve_scope_roots(
    state: &AppState,
    user_id: ObjectId,
    folder_id: Option<&str>,
    category_id: Option<&str>,
) -> Result<Option<Vec<ObjectId>>> {
    if let Some(fid) = folder_id {
        let oid = ObjectId::parse_str(fid)
            .map_err(|_| AppError::BadRequest("Invalid folder_id".to_string()))?;
        return Ok(Some(vec![oid]));
    }
    if let Some(cid) = category_id {
        let oid = ObjectId::parse_str(cid)
            .map_err(|_| AppError::BadRequest("Invalid category_id".to_string()))?;
        let coll = state.db.collection::<MusicCategory>("music_categories");
        let cat = coll
            .find_one(doc! { "_id": oid, "owner_id": user_id })
            .await?
            .ok_or_else(|| AppError::NotFound("MusicCategory".to_string()))?;
        return Ok(Some(cat.folder_ids));
    }
    Ok(None)
}

#[derive(Debug, Deserialize)]
pub struct LibraryScopeQuery {
    pub folder_id: Option<String>,
    pub category_id: Option<String>,
}

pub async fn list_artists(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(scope): Query<LibraryScopeQuery>,
) -> Result<Json<Vec<ArtistResponse>>> {
    use futures::StreamExt;

    let mut parent_ids = music_included_parent_ids(&state, user.id).await?;

    if let Some(roots) = resolve_scope_roots(
        &state,
        user.id,
        scope.folder_id.as_deref(),
        scope.category_id.as_deref(),
    )
    .await?
    {
        parent_ids = restrict_to_subtrees(&state, user.id, parent_ids, &roots).await?;
    }

    if parent_ids.is_empty() {
        return Ok(Json(Vec::new()));
    }

    // Two-stage `$group` runs entirely in BSON inside Mongo, returning one
    // row per artist regardless of track count. The previous implementation
    // shipped every audio `File` document over the wire and grouped them
    // in Rust — at ~1.7k artists that meant tens of megabytes of BSON and
    // most of the wall-clock cost of the endpoint (~2 s in DevTools
    // captures). The artist/album normalisation matches the old Rust
    // behaviour: a missing field, a BSON null, or an empty string all
    // collapse to "Unknown Artist" / "Unknown Album".
    let files_coll = state.db.collection::<File>("files");
    let normalise = |field: &str, default: &str| {
        doc! {
            "$let": {
                "vars": { "v": { "$ifNull": [field, ""] } },
                "in": { "$cond": [{ "$eq": ["$$v", ""] }, default, "$$v"] },
            }
        }
    };
    let pipeline = vec![
        doc! {
            "$match": {
                "parent_id": { "$in": &parent_ids },
                "mime_type": { "$regex": "^audio/" },
                "deleted_at": bson::Bson::Null,
            }
        },
        doc! {
            "$group": {
                "_id": {
                    "artist": normalise("$metadata.audio.artist", "Unknown Artist"),
                    "album":  normalise("$metadata.audio.album",  "Unknown Album"),
                },
                "tracks": { "$sum": 1 },
            }
        },
        doc! {
            "$group": {
                "_id": "$_id.artist",
                "album_count": { "$sum": 1 },
                "track_count": { "$sum": "$tracks" },
            }
        },
    ];

    let mut cursor = files_coll.aggregate(pipeline).await?;
    let mut result: Vec<ArtistResponse> = Vec::new();
    while let Some(doc) = cursor.next().await {
        let doc = doc?;
        let name = doc
            .get_str("_id")
            .map_err(|e| AppError::Internal(format!("artist _id: {e}")))?
            .to_owned();
        let album_count = doc
            .get_i32("album_count")
            .map(|n| n as i64)
            .or_else(|_| doc.get_i64("album_count"))
            .unwrap_or(0);
        let track_count = doc
            .get_i32("track_count")
            .map(|n| n as i64)
            .or_else(|_| doc.get_i64("track_count"))
            .unwrap_or(0);
        result.push(ArtistResponse {
            name,
            album_count,
            track_count,
        });
    }

    result.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    Ok(Json(result))
}

pub async fn list_artist_albums(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(artist_name): Path<String>,
    Query(scope): Query<LibraryScopeQuery>,
) -> Result<Json<Vec<MusicAlbumResponse>>> {
    let mut parent_ids = music_included_parent_ids(&state, user.id).await?;

    if let Some(roots) = resolve_scope_roots(
        &state,
        user.id,
        scope.folder_id.as_deref(),
        scope.category_id.as_deref(),
    )
    .await?
    {
        parent_ids = restrict_to_subtrees(&state, user.id, parent_ids, &roots).await?;
    }

    if parent_ids.is_empty() {
        return Ok(Json(Vec::new()));
    }

    let files_coll = state.db.collection::<File>("files");
    let filter = doc! {
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
    Path((artist_name, album_name)): Path<(String, String)>,
    Query(scope): Query<LibraryScopeQuery>,
) -> Result<Json<Vec<TrackResponse>>> {
    let mut parent_ids = music_included_parent_ids(&state, user.id).await?;

    if let Some(roots) = resolve_scope_roots(
        &state,
        user.id,
        scope.folder_id.as_deref(),
        scope.category_id.as_deref(),
    )
    .await?
    {
        parent_ids = restrict_to_subtrees(&state, user.id, parent_ids, &roots).await?;
    }

    if parent_ids.is_empty() {
        return Ok(Json(Vec::new()));
    }

    let files_coll = state.db.collection::<File>("files");

    let mut filters = vec![
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
                file: file_resp,
                audio,
            }
        })
        .collect();

    Ok(Json(tracks))
}

// ── Music Categories ────────────────────────────────────────────────────────

fn category_to_dto(c: &MusicCategory) -> MusicCategoryDto {
    MusicCategoryDto {
        id: c.id.to_hex(),
        name: c.name.clone(),
        folder_ids: c.folder_ids.iter().map(|i| i.to_hex()).collect(),
    }
}

fn parse_folder_ids(ids: &[String]) -> Result<Vec<ObjectId>> {
    ids.iter()
        .map(|s| {
            ObjectId::parse_str(s)
                .map_err(|_| AppError::BadRequest(format!("Invalid folder_id: {}", s)))
        })
        .collect()
}

/// Read a numeric field that Mongo may have returned as either Int32 or
/// Int64 depending on accumulator type and document size.
fn get_int(d: &mongodb::bson::Document, key: &str) -> i64 {
    d.get_i32(key)
        .map(|n| n as i64)
        .or_else(|_| d.get_i64(key))
        .unwrap_or(0)
}

pub async fn list_categories(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<MusicCategoryDto>>> {
    let coll = state.db.collection::<MusicCategory>("music_categories");
    let options = mongodb::options::FindOptions::builder()
        .sort(doc! { "name": 1 })
        .build();
    let mut cursor = coll
        .find(doc! { "owner_id": user.id })
        .with_options(options)
        .await?;

    let mut out = Vec::new();
    while cursor.advance().await? {
        let cat: MusicCategory = cursor.deserialize_current()?;
        out.push(category_to_dto(&cat));
    }

    Ok(Json(out))
}

pub async fn create_category(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<CreateMusicCategoryRequest>,
) -> Result<(StatusCode, Json<MusicCategoryDto>)> {
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::BadRequest(
            "Category name cannot be empty".to_string(),
        ));
    }
    let folder_ids = parse_folder_ids(&body.folder_ids)?;

    let coll = state.db.collection::<MusicCategory>("music_categories");

    let existing = coll
        .find_one(doc! { "owner_id": user.id, "name": &name })
        .await?;
    if existing.is_some() {
        return Err(AppError::Conflict(format!(
            "A category named \"{}\" already exists",
            name
        )));
    }

    let cat = MusicCategory::new(user.id, name, folder_ids);
    coll.insert_one(&cat).await?;

    Ok((StatusCode::CREATED, Json(category_to_dto(&cat))))
}

pub async fn update_category(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateMusicCategoryRequest>,
) -> Result<Json<MusicCategoryDto>> {
    let cat_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid category ID".to_string()))?;

    let coll = state.db.collection::<MusicCategory>("music_categories");
    let cat = coll
        .find_one(doc! { "_id": cat_id, "owner_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("MusicCategory".to_string()))?;

    let mut update_doc = doc! {};

    if let Some(ref name) = body.name {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::BadRequest(
                "Category name cannot be empty".to_string(),
            ));
        }
        if name != cat.name {
            let dup = coll
                .find_one(doc! {
                    "owner_id": user.id,
                    "name": name,
                    "_id": { "$ne": cat_id },
                })
                .await?;
            if dup.is_some() {
                return Err(AppError::Conflict(format!(
                    "A category named \"{}\" already exists",
                    name
                )));
            }
        }
        update_doc.insert("name", name);
    }

    if let Some(folder_ids) = body.folder_ids {
        let parsed = parse_folder_ids(&folder_ids)?;
        let bson_ids: Vec<bson::Bson> = parsed.iter().copied().map(bson::Bson::ObjectId).collect();
        update_doc.insert("folder_ids", bson_ids);
    }

    if !update_doc.is_empty() {
        update_doc.insert("updated_at", bson::DateTime::from_chrono(Utc::now()));
        coll.update_one(
            doc! { "_id": cat_id, "owner_id": user.id },
            doc! { "$set": update_doc },
        )
        .await?;
    }

    let updated = coll
        .find_one(doc! { "_id": cat_id, "owner_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("MusicCategory".to_string()))?;

    Ok(Json(category_to_dto(&updated)))
}

pub async fn delete_category(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let cat_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid category ID".to_string()))?;

    let coll = state.db.collection::<MusicCategory>("music_categories");
    let result = coll
        .delete_one(doc! { "_id": cat_id, "owner_id": user.id })
        .await?;

    if result.deleted_count == 0 {
        return Err(AppError::NotFound("MusicCategory".to_string()));
    }

    Ok(StatusCode::NO_CONTENT)
}

// ── Music search ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
    pub folder_id: Option<String>,
    pub category_id: Option<String>,
    pub limit: Option<usize>,
}

const SEARCH_DEFAULT_LIMIT: usize = 25;
const SEARCH_MAX_LIMIT: usize = 200;

pub async fn search_music(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(query): Query<SearchQuery>,
) -> Result<Json<MusicSearchResponse>> {
    use futures::StreamExt;

    let q = query.q.unwrap_or_default().trim().to_lowercase();
    let limit = query
        .limit
        .unwrap_or(SEARCH_DEFAULT_LIMIT)
        .clamp(1, SEARCH_MAX_LIMIT);

    let mut parent_ids = music_included_parent_ids(&state, user.id).await?;

    if let Some(roots) = resolve_scope_roots(
        &state,
        user.id,
        query.folder_id.as_deref(),
        query.category_id.as_deref(),
    )
    .await?
    {
        parent_ids = restrict_to_subtrees(&state, user.id, parent_ids, &roots).await?;
    }

    if q.is_empty() || parent_ids.is_empty() {
        return Ok(Json(MusicSearchResponse {
            artists: Vec::new(),
            albums: Vec::new(),
            tracks: Vec::new(),
            total_artists: 0,
            total_albums: 0,
            total_tracks: 0,
        }));
    }

    // Pushed-down search. Old impl loaded every audio File doc and walked
    // them in Rust to build three separate aggregations, which on libraries
    // of any size dominated the request time. Three pipelines run in
    // parallel; tracks pipeline uses `$facet` to return the total alongside
    // the limited page so we don't ship every match just to count them.
    let files_coll = state.db.collection::<File>("files");
    let q_rx = doc! { "$regex": escape_regex(&q), "$options": "i" };

    let base_match = doc! {
        "$match": {
            "parent_id": { "$in": &parent_ids },
            "mime_type": { "$regex": "^audio/" },
            "deleted_at": bson::Bson::Null,
        }
    };
    let artist_norm = doc! {
        "$let": {
            "vars": { "v": { "$ifNull": ["$metadata.audio.artist", ""] } },
            "in": { "$cond": [{ "$eq": ["$$v", ""] }, "Unknown Artist", "$$v"] },
        }
    };
    let album_norm = doc! {
        "$let": {
            "vars": { "v": { "$ifNull": ["$metadata.audio.album", ""] } },
            "in": { "$cond": [{ "$eq": ["$$v", ""] }, "Unknown Album", "$$v"] },
        }
    };

    let artists_pipeline = vec![
        base_match.clone(),
        doc! { "$group": {
            "_id": { "artist": &artist_norm, "album": &album_norm },
            "tracks": { "$sum": 1 },
        }},
        doc! { "$group": {
            "_id": "$_id.artist",
            "album_count": { "$sum": 1 },
            "track_count": { "$sum": "$tracks" },
        }},
        doc! { "$match": { "_id": &q_rx } },
    ];

    let albums_pipeline = vec![
        base_match.clone(),
        doc! { "$group": {
            "_id": { "artist": &artist_norm, "album": &album_norm },
            "track_count": { "$sum": 1 },
            "year": { "$first": "$metadata.audio.year" },
            // Collect ids of tracks that carry embedded cover art; the
            // first one (whatever order Mongo materialises) wins, matching
            // the old "first walked with art" behaviour.
            "cover_candidates": { "$push": { "$cond": [
                { "$eq": ["$metadata.audio.has_cover_art", true] },
                "$_id",
                "$$REMOVE",
            ]}},
        }},
        doc! { "$addFields": {
            "cover_file_id": { "$arrayElemAt": ["$cover_candidates", 0] },
        }},
        doc! { "$match": { "$or": [
            { "_id.artist": &q_rx },
            { "_id.album":  &q_rx },
        ]}},
    ];

    let tracks_pipeline = vec![
        base_match,
        doc! { "$match": { "$or": [
            { "metadata.audio.title":        &q_rx },
            { "metadata.audio.artist":       &q_rx },
            { "metadata.audio.album":        &q_rx },
            { "metadata.audio.album_artist": &q_rx },
        ]}},
        doc! { "$facet": {
            "total": [{ "$count": "n" }],
            "page": [
                { "$sort": { "metadata.audio.title": 1, "created_at": -1 } },
                { "$limit": limit as i64 },
            ],
        }},
    ];

    let run = |pipeline: Vec<mongodb::bson::Document>| {
        let coll = files_coll.clone();
        async move {
            let mut cursor = coll.aggregate(pipeline).await?;
            let mut docs = Vec::new();
            while let Some(d) = cursor.next().await {
                docs.push(d?);
            }
            Result::<Vec<mongodb::bson::Document>>::Ok(docs)
        }
    };

    let (artists_docs, albums_docs, tracks_docs) = tokio::try_join!(
        run(artists_pipeline),
        run(albums_pipeline),
        run(tracks_pipeline),
    )?;

    // Artists: { _id: artist_name, album_count, track_count }
    let mut artists: Vec<ArtistResponse> = artists_docs
        .into_iter()
        .filter_map(|d| {
            let name = d.get_str("_id").ok()?.to_owned();
            let album_count = get_int(&d, "album_count");
            let track_count = get_int(&d, "track_count");
            Some(ArtistResponse {
                name,
                album_count,
                track_count,
            })
        })
        .collect();
    artists.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    let total_artists = artists.len();
    artists.truncate(limit);

    // Albums: { _id: { artist, album }, track_count, year, cover_file_id }
    let mut albums: Vec<MusicAlbumResponse> = albums_docs
        .into_iter()
        .filter_map(|d| {
            let id = d.get_document("_id").ok()?;
            let artist = id.get_str("artist").ok()?.to_owned();
            let name = id.get_str("album").ok()?.to_owned();
            let track_count = get_int(&d, "track_count");
            let year = d
                .get_i32("year")
                .ok()
                .or_else(|| d.get_i64("year").ok().map(|n| n as i32));
            let cover_file_id = d
                .get_object_id("cover_file_id")
                .ok()
                .map(|oid| oid.to_hex());
            Some(MusicAlbumResponse {
                name,
                artist,
                year,
                track_count,
                cover_file_id,
            })
        })
        .collect();
    albums.sort_by(|a, b| {
        let year_cmp = match (a.year, b.year) {
            (Some(ya), Some(yb)) => ya.cmp(&yb),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        };
        year_cmp.then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    let total_albums = albums.len();
    albums.truncate(limit);

    // Tracks: single `$facet` doc with { total: [{ n }], page: [<files>] }.
    let mut total_tracks: usize = 0;
    let mut tracks: Vec<TrackResponse> = Vec::new();
    if let Some(facet) = tracks_docs.into_iter().next() {
        if let Ok(total_arr) = facet.get_array("total") {
            if let Some(first) = total_arr.first().and_then(|b| b.as_document()) {
                total_tracks = get_int(first, "n") as usize;
            }
        }
        if let Ok(page) = facet.get_array("page") {
            tracks.reserve(page.len());
            for doc in page {
                if let Some(doc) = doc.as_document() {
                    let f: File = mongodb::bson::from_document(doc.clone())
                        .map_err(|e| AppError::Internal(format!("track decode: {e}")))?;
                    let file_resp = file_to_response(&f);
                    let audio: AudioMeta = file_resp
                        .metadata
                        .get("audio")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default();
                    tracks.push(TrackResponse {
                        file: file_resp,
                        audio,
                    });
                }
            }
        }
    }
    // Re-sort the page case-insensitively. The Mongo sort is byte-order;
    // for the small returned page this is cheap and matches the previous
    // Rust behaviour exactly (title lowercased, then most-recent first).
    tracks.sort_by(|a, b| {
        let ta = a.audio.title.as_deref().unwrap_or("");
        let tb = b.audio.title.as_deref().unwrap_or("");
        ta.to_lowercase()
            .cmp(&tb.to_lowercase())
            .then_with(|| b.file.created_at.cmp(&a.file.created_at))
    });

    Ok(Json(MusicSearchResponse {
        artists,
        albums,
        tracks,
        total_artists,
        total_albums,
        total_tracks,
    }))
}
