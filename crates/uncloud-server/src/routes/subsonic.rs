use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use axum::Json;
use axum::body::{Body, to_bytes};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use bson::{Bson, doc};
use chrono::Utc;
use futures::{StreamExt, TryStreamExt};
use md5::{Digest as Md5Digest, Md5};
use mongodb::bson::oid::ObjectId;
use mongodb::options::FindOptions;
use rand::RngCore;
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use tokio_util::io::ReaderStream;
use tracing::debug;

use crate::AppState;
use crate::error::{AppError, Result};
use crate::middleware::AuthUser;
use crate::models::{
    File, Folder, Playlist, PlaylistTrack, ProcessingStatus, SubsonicCredential, SubsonicId,
    SubsonicIdKind, TaskType, User, UserStatus,
};
use crate::routes::files::build_folder_path;
use crate::routes::music::{extract_audio_meta, music_included_parent_ids, restrict_to_subtrees};
use crate::services::SecretCipher;
use crate::services::sharing::check_file_access;
use uncloud_common::{
    AudioMeta, CreateSubsonicCredentialRequest, CreateSubsonicCredentialResponse,
    InheritableSetting, SubsonicCredentialResponse,
};

const API_VERSION: &str = "1.16.1";
const SERVER_TYPE: &str = "Uncloud";
const OPEN_SUBSONIC: bool = true;
const MAX_FORM_BYTES: usize = 64 * 1024;
const MAX_PAGE_SIZE: i64 = 500;

#[derive(Clone, Default)]
struct ParamMap(HashMap<String, Vec<String>>);

impl ParamMap {
    fn push(&mut self, key: String, value: String) {
        self.0.entry(key).or_default().push(value);
    }

    fn first(&self, key: &str) -> Option<&str> {
        self.0.get(key)?.first().map(String::as_str)
    }

    fn all(&self, key: &str) -> Vec<&str> {
        self.0
            .get(key)
            .map(|values| values.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }

    fn required(&self, key: &str) -> std::result::Result<&str, SubsonicError> {
        self.first(key)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| SubsonicError::missing(key))
    }

    fn format(&self) -> ResponseFormat {
        match self
            .first("f")
            .unwrap_or("xml")
            .to_ascii_lowercase()
            .as_str()
        {
            "json" => ResponseFormat::Json,
            _ => ResponseFormat::Xml,
        }
    }

    fn i64_param(&self, key: &str, default: i64, max: i64) -> i64 {
        self.first(key)
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(default)
            .clamp(0, max)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResponseFormat {
    Xml,
    Json,
}

#[derive(Debug, Clone)]
struct SubsonicError {
    code: i32,
    message: String,
}

impl SubsonicError {
    fn missing(param: &str) -> Self {
        Self {
            code: 10,
            message: format!("Required parameter `{param}` is missing"),
        }
    }

    fn auth() -> Self {
        Self {
            code: 40,
            message: "Wrong username or password".to_string(),
        }
    }

    fn forbidden() -> Self {
        Self {
            code: 50,
            message: "User is not authorized for the given operation".to_string(),
        }
    }

    fn not_found(label: &str) -> Self {
        Self {
            code: 70,
            message: format!("{label} not found"),
        }
    }

    fn generic(message: impl Into<String>) -> Self {
        Self {
            code: 0,
            message: message.into(),
        }
    }
}

struct SubsonicPayload {
    json_key: &'static str,
    json_value: Value,
    xml: XmlElement,
}

#[derive(Clone)]
struct XmlElement {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<XmlElement>,
    text: Option<String>,
}

impl XmlElement {
    fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            attrs: Vec::new(),
            children: Vec::new(),
            text: None,
        }
    }

    fn attr(mut self, key: impl Into<String>, value: impl ToString) -> Self {
        self.attrs.push((key.into(), value.to_string()));
        self
    }

    fn opt_attr<T: ToString>(self, key: impl Into<String>, value: Option<T>) -> Self {
        match value {
            Some(value) => self.attr(key, value),
            None => self,
        }
    }

    fn children(mut self, children: Vec<XmlElement>) -> Self {
        self.children.extend(children);
        self
    }

    fn text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }
}

pub async fn list_credentials(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<SubsonicCredentialResponse>>> {
    let coll = state
        .db
        .collection::<SubsonicCredential>("subsonic_credentials");
    let options = FindOptions::builder()
        .sort(doc! { "created_at": -1 })
        .build();
    let mut cursor = coll
        .find(doc! { "owner_id": user.id })
        .with_options(options)
        .await?;

    let mut out = Vec::new();
    while cursor.advance().await? {
        let credential = cursor.deserialize_current()?;
        out.push(credential_to_response(&credential));
    }

    Ok(Json(out))
}

pub async fn create_credential(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<CreateSubsonicCredentialRequest>,
) -> Result<Json<CreateSubsonicCredentialResponse>> {
    let label = body.label.trim().to_string();
    if label.is_empty() || label.len() > 128 {
        return Err(AppError::BadRequest(
            "Credential label must be between 1 and 128 characters".into(),
        ));
    }

    let app_password = generate_app_password();
    let encrypted = SecretCipher::from_config(&state.config.secrets)?
        .encrypt_subsonic_credential(&app_password)?;
    let credential = SubsonicCredential::new(user.id, label, encrypted);

    let coll = state
        .db
        .collection::<SubsonicCredential>("subsonic_credentials");
    coll.insert_one(&credential).await?;

    Ok(Json(CreateSubsonicCredentialResponse {
        id: credential.id.to_hex(),
        label: credential.label,
        app_password,
        created_at: credential.created_at.to_rfc3339(),
    }))
}

pub async fn delete_credential(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let credential_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid Subsonic credential ID".into()))?;
    let coll = state
        .db
        .collection::<SubsonicCredential>("subsonic_credentials");
    let result = coll
        .delete_one(doc! { "_id": credential_id, "owner_id": user.id })
        .await?;
    if result.deleted_count == 0 {
        return Err(AppError::NotFound("Subsonic credential".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn handle_rest(
    State(state): State<Arc<AppState>>,
    Path(method): Path<String>,
    headers: HeaderMap,
    request: axum::extract::Request,
) -> Response {
    match handle_rest_inner(state, method, headers, request).await {
        Ok(response) => response,
        Err(err) => {
            debug!(
                error_code = err.code,
                error = %err.message,
                "subsonic request failed before response format was known",
            );
            subsonic_error_response(ResponseFormat::Xml, err).into_response()
        }
    }
}

async fn handle_rest_inner(
    state: Arc<AppState>,
    mut method_name: String,
    headers: HeaderMap,
    request: axum::extract::Request,
) -> std::result::Result<Response, SubsonicError> {
    if let Some(stripped) = method_name.strip_suffix(".view") {
        method_name = stripped.to_string();
    }

    let params = parse_params(request).await?;
    let format = params.format();
    let method = method_name.to_ascii_lowercase();

    if method == "getopensubsonicextensions" {
        return Ok(get_open_subsonic_extensions(format));
    }

    let user = match authenticate(&state, &params).await {
        Ok(user) => user,
        Err(err) => {
            debug!(
                method = %method_name,
                error_code = err.code,
                error = %err.message,
                "subsonic authentication failed",
            );
            return Ok(subsonic_error_response(format, err).into_response());
        }
    };
    if !music_feature_enabled(&state, &user) {
        let err = SubsonicError::forbidden();
        debug!(
            method = %method_name,
            error_code = err.code,
            error = %err.message,
            "subsonic request failed",
        );
        return Ok(subsonic_error_response(format, err).into_response());
    }

    let result = match method.as_str() {
        "ping" => Ok(ok_response(format, None)),
        "getlicense" => get_license(format),
        "getmusicfolders" => get_music_folders(&state, &user, format).await,
        "getindexes" => get_indexes(&state, &user, &params, format).await,
        "getmusicdirectory" => get_music_directory(&state, &user, &params, format).await,
        "getartists" => get_artists(&state, &user, &params, format).await,
        "getartist" => get_artist(&state, &user, &params, format).await,
        "getalbum" => get_album(&state, &user, &params, format).await,
        "getsong" => get_song(&state, &user, &params, format).await,
        "search3" => search3(&state, &user, &params, format).await,
        "getalbumlist2" => get_album_list2(&state, &user, &params, format).await,
        "getrandomsongs" => get_random_songs(&state, &user, &params, format).await,
        "getstarred" => Ok(empty_starred(format, "starred")),
        "getstarred2" => Ok(empty_starred(format, "starred2")),
        "getbookmarks" => Ok(empty_bookmarks(format)),
        "getgenres" => Ok(empty_genres(format)),
        "stream" => stream_song(&state, &user, &params, &headers, false).await,
        "download" => stream_song(&state, &user, &params, &headers, true).await,
        "getcoverart" => get_cover_art(&state, &user, &params).await,
        "getplaylists" => get_playlists(&state, &user, format).await,
        "getplaylist" => get_playlist(&state, &user, &params, format).await,
        "createplaylist" => create_playlist(&state, &user, &params, format).await,
        "updateplaylist" => update_playlist(&state, &user, &params, format).await,
        "deleteplaylist" => delete_playlist(&state, &user, &params, format).await,
        "scrobble" => Ok(ok_response(format, None)),
        _ => Err(SubsonicError::generic(format!(
            "Subsonic method `{method_name}` is not supported"
        ))),
    };

    Ok(match result {
        Ok(response) => response,
        Err(err) => {
            debug!(
                method = %method_name,
                error_code = err.code,
                error = %err.message,
                "subsonic request failed",
            );
            subsonic_error_response(format, err).into_response()
        }
    })
}

async fn parse_params(
    request: axum::extract::Request,
) -> std::result::Result<ParamMap, SubsonicError> {
    let (parts, body) = request.into_parts();
    let mut params = ParamMap::default();
    if let Some(query) = parts.uri.query() {
        for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
            params.push(key.into_owned(), value.into_owned());
        }
    }

    if parts.method == Method::POST {
        let bytes = to_bytes(body, MAX_FORM_BYTES)
            .await
            .map_err(|_| SubsonicError::generic("Failed to read request body"))?;
        if !bytes.is_empty() {
            for (key, value) in url::form_urlencoded::parse(&bytes) {
                params.push(key.into_owned(), value.into_owned());
            }
        }
    }

    Ok(params)
}

async fn authenticate(
    state: &AppState,
    params: &ParamMap,
) -> std::result::Result<User, SubsonicError> {
    let username = params.required("u")?;
    params.required("v")?;
    params.required("c")?;

    let users = state.db.collection::<User>("users");
    let user = users
        .find_one(doc! { "username": username })
        .await
        .map_err(|_| SubsonicError::auth())?
        .ok_or_else(SubsonicError::auth)?;
    if user.status != UserStatus::Active {
        return Err(SubsonicError::auth());
    }

    let coll = state
        .db
        .collection::<SubsonicCredential>("subsonic_credentials");
    let credentials: Vec<SubsonicCredential> = coll
        .find(doc! { "owner_id": user.id })
        .await
        .map_err(|_| SubsonicError::auth())?
        .try_collect()
        .await
        .map_err(|_| SubsonicError::auth())?;

    let cipher =
        SecretCipher::from_config(&state.config.secrets).map_err(|_| SubsonicError::auth())?;
    for credential in credentials {
        let Ok(app_password) = cipher.decrypt_subsonic_credential(&credential.credential) else {
            continue;
        };
        if credential_matches(&app_password, params)? {
            let _ = coll
                .update_one(
                    doc! { "_id": credential.id },
                    doc! { "$set": { "last_used_at": bson::DateTime::from_chrono(Utc::now()) } },
                )
                .await;
            return Ok(user);
        }
    }

    Err(SubsonicError::auth())
}

fn credential_matches(
    app_password: &str,
    params: &ParamMap,
) -> std::result::Result<bool, SubsonicError> {
    if let (Some(token), Some(salt)) = (params.first("t"), params.first("s")) {
        let mut hasher = Md5::new();
        hasher.update(app_password.as_bytes());
        hasher.update(salt.as_bytes());
        let expected = hex::encode(hasher.finalize());
        return Ok(expected
            .as_bytes()
            .ct_eq(token.to_ascii_lowercase().as_bytes())
            .into());
    }

    if let Some(password) = params.first("p") {
        let password = decode_password_param(password)?;
        return Ok(app_password.as_bytes().ct_eq(password.as_bytes()).into());
    }

    Err(SubsonicError::missing("p or t/s"))
}

fn decode_password_param(password: &str) -> std::result::Result<String, SubsonicError> {
    if let Some(hex_value) = password.strip_prefix("enc:") {
        let bytes = hex::decode(hex_value)
            .map_err(|_| SubsonicError::generic("Invalid hex-encoded password"))?;
        String::from_utf8(bytes).map_err(|_| SubsonicError::generic("Password is not valid UTF-8"))
    } else {
        Ok(password.to_string())
    }
}

fn music_feature_enabled(state: &AppState, user: &User) -> bool {
    state.config.features.music
        && !user
            .disabled_features
            .iter()
            .any(|feature| feature == crate::config::FEATURE_MUSIC)
}

fn credential_to_response(credential: &SubsonicCredential) -> SubsonicCredentialResponse {
    SubsonicCredentialResponse {
        id: credential.id.to_hex(),
        label: credential.label.clone(),
        created_at: credential.created_at.to_rfc3339(),
        last_used_at: credential.last_used_at.map(|dt| dt.to_rfc3339()),
    }
}

fn generate_app_password() -> String {
    let mut bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("ucsub_{}", URL_SAFE_NO_PAD.encode(bytes))
}

fn get_license(format: ResponseFormat) -> std::result::Result<Response, SubsonicError> {
    let expires = "2099-12-31T23:59:59Z";
    let payload = SubsonicPayload {
        json_key: "license",
        json_value: json!({
            "valid": true,
            "email": "uncloud",
            "licenseExpires": expires,
            "trialExpires": expires,
        }),
        xml: XmlElement::new("license")
            .attr("valid", "true")
            .attr("email", "uncloud")
            .attr("licenseExpires", expires)
            .attr("trialExpires", expires),
    };
    Ok(ok_response(format, Some(payload)))
}

fn get_open_subsonic_extensions(format: ResponseFormat) -> Response {
    let payload = SubsonicPayload {
        json_key: "openSubsonicExtensions",
        json_value: json!([
            {
                "name": "formPost",
                "versions": [1],
            }
        ]),
        xml: XmlElement::new("openSubsonicExtensions").children(vec![
            XmlElement::new("openSubsonicExtension")
                .attr("name", "formPost")
                .children(vec![XmlElement::new("version").text("1")]),
        ]),
    };
    ok_response(format, Some(payload))
}

fn empty_starred(format: ResponseFormat, root: &'static str) -> Response {
    let payload = SubsonicPayload {
        json_key: root,
        json_value: json!({
            "artist": [],
            "album": [],
            "song": [],
        }),
        xml: XmlElement::new(root),
    };
    ok_response(format, Some(payload))
}

fn empty_bookmarks(format: ResponseFormat) -> Response {
    let payload = SubsonicPayload {
        json_key: "bookmarks",
        json_value: json!({ "bookmark": [] }),
        xml: XmlElement::new("bookmarks"),
    };
    ok_response(format, Some(payload))
}

fn empty_genres(format: ResponseFormat) -> Response {
    let payload = SubsonicPayload {
        json_key: "genres",
        json_value: json!({ "genre": [] }),
        xml: XmlElement::new("genres"),
    };
    ok_response(format, Some(payload))
}

async fn get_music_folders(
    state: &AppState,
    user: &User,
    format: ResponseFormat,
) -> std::result::Result<Response, SubsonicError> {
    let folders = accessible_music_folders(state, user.id)
        .await
        .map_err(to_subsonic_error)?;
    let accessible: HashSet<ObjectId> = folders.iter().map(|folder| folder.folder.id).collect();
    let mut roots: Vec<AccessibleFolder> = folders
        .into_iter()
        .filter(|folder| {
            folder
                .folder
                .parent_id
                .map(|parent| !accessible.contains(&parent))
                .unwrap_or(true)
        })
        .collect();
    roots.sort_by(|a, b| a.path.to_lowercase().cmp(&b.path.to_lowercase()));

    let mut json_rows = Vec::new();
    let mut xml_rows = Vec::new();
    for folder in roots {
        let id = subsonic_id_for(
            state,
            user.id,
            SubsonicIdKind::Folder,
            folder.folder.id.to_hex(),
        )
        .await?;
        json_rows.push(json!({ "id": id, "name": folder.path }));
        xml_rows.push(
            XmlElement::new("musicFolder")
                .attr("id", id)
                .attr("name", folder.path),
        );
    }

    let payload = SubsonicPayload {
        json_key: "musicFolders",
        json_value: json!({ "musicFolder": json_rows }),
        xml: XmlElement::new("musicFolders").children(xml_rows),
    };
    Ok(ok_response(format, Some(payload)))
}

async fn get_indexes(
    state: &AppState,
    user: &User,
    params: &ParamMap,
    format: ResponseFormat,
) -> std::result::Result<Response, SubsonicError> {
    let artists = artist_rows(state, user, params.first("musicFolderId")).await?;
    let groups = group_artists(artists);
    let modified = Utc::now().timestamp_millis();

    let mut json_indexes = Vec::new();
    let mut xml_indexes = Vec::new();
    for (letter, artists) in groups {
        let mut json_artists = Vec::new();
        let mut xml_artists = Vec::new();
        for artist in artists {
            json_artists.push(json!({
                "id": artist.id,
                "name": artist.name,
                "albumCount": artist.album_count,
            }));
            xml_artists.push(
                XmlElement::new("artist")
                    .attr("id", artist.id)
                    .attr("name", artist.name)
                    .attr("albumCount", artist.album_count),
            );
        }
        json_indexes.push(json!({ "name": letter, "artist": json_artists }));
        xml_indexes.push(
            XmlElement::new("index")
                .attr("name", letter)
                .children(xml_artists),
        );
    }

    let payload = SubsonicPayload {
        json_key: "indexes",
        json_value: json!({
            "lastModified": modified,
            "ignoredArticles": "",
            "index": json_indexes,
        }),
        xml: XmlElement::new("indexes")
            .attr("lastModified", modified)
            .attr("ignoredArticles", "")
            .children(xml_indexes),
    };
    Ok(ok_response(format, Some(payload)))
}

async fn get_artists(
    state: &AppState,
    user: &User,
    params: &ParamMap,
    format: ResponseFormat,
) -> std::result::Result<Response, SubsonicError> {
    let artists = artist_rows(state, user, params.first("musicFolderId")).await?;
    let groups = group_artists(artists);

    let mut json_indexes = Vec::new();
    let mut xml_indexes = Vec::new();
    for (letter, artists) in groups {
        let mut json_artists = Vec::new();
        let mut xml_artists = Vec::new();
        for artist in artists {
            json_artists.push(json!({
                "id": artist.id,
                "name": artist.name,
                "albumCount": artist.album_count,
            }));
            xml_artists.push(
                XmlElement::new("artist")
                    .attr("id", artist.id)
                    .attr("name", artist.name)
                    .attr("albumCount", artist.album_count),
            );
        }
        json_indexes.push(json!({ "name": letter, "artist": json_artists }));
        xml_indexes.push(
            XmlElement::new("index")
                .attr("name", letter)
                .children(xml_artists),
        );
    }

    let payload = SubsonicPayload {
        json_key: "artists",
        json_value: json!({ "ignoredArticles": "", "index": json_indexes }),
        xml: XmlElement::new("artists")
            .attr("ignoredArticles", "")
            .children(xml_indexes),
    };
    Ok(ok_response(format, Some(payload)))
}

async fn get_music_directory(
    state: &AppState,
    user: &User,
    params: &ParamMap,
    format: ResponseFormat,
) -> std::result::Result<Response, SubsonicError> {
    let alias = alias_for_param(state, user.id, params.required("id")?).await?;
    if alias.kind != SubsonicIdKind::Folder.as_str() {
        return Err(SubsonicError::not_found("Directory"));
    }
    let folder_id = ObjectId::parse_str(&alias.internal_key)
        .map_err(|_| SubsonicError::not_found("Directory"))?;
    let folders = accessible_music_folders(state, user.id)
        .await
        .map_err(to_subsonic_error)?;
    if !folders.iter().any(|folder| folder.folder.id == folder_id) {
        return Err(SubsonicError::not_found("Directory"));
    }

    let folders_by_id: HashMap<ObjectId, AccessibleFolder> = folders
        .into_iter()
        .map(|folder| (folder.folder.id, folder))
        .collect();
    let folder = folders_by_id
        .get(&folder_id)
        .ok_or_else(|| SubsonicError::not_found("Directory"))?;
    let dir_id =
        subsonic_id_for(state, user.id, SubsonicIdKind::Folder, folder_id.to_hex()).await?;
    let parent_id = if let Some(parent_id) = folder.folder.parent_id {
        if folders_by_id.contains_key(&parent_id) {
            Some(subsonic_id_for(state, user.id, SubsonicIdKind::Folder, parent_id.to_hex()).await?)
        } else {
            None
        }
    } else {
        None
    };

    let mut json_children = Vec::new();
    let mut xml_children = Vec::new();

    let mut child_folders: Vec<&AccessibleFolder> = folders_by_id
        .values()
        .filter(|candidate| candidate.folder.parent_id == Some(folder_id))
        .collect();
    child_folders.sort_by(|a, b| {
        a.folder
            .name
            .to_lowercase()
            .cmp(&b.folder.name.to_lowercase())
    });
    for child in child_folders {
        let child_id = subsonic_id_for(
            state,
            user.id,
            SubsonicIdKind::Folder,
            child.folder.id.to_hex(),
        )
        .await?;
        let child_json = json!({
            "id": child_id,
            "parent": dir_id,
            "title": child.folder.name,
            "name": child.folder.name,
            "isDir": true,
            "path": child.path,
        });
        let child_xml = XmlElement::new("child")
            .attr("id", child_id)
            .attr("parent", &dir_id)
            .attr("title", &child.folder.name)
            .attr("isDir", "true")
            .attr("path", &child.path);
        json_children.push(child_json);
        xml_children.push(child_xml);
    }

    let tracks = direct_folder_tracks(state, folder_id).await?;
    for track in tracks {
        let (json, xml) =
            song_entry(state, user.id, &track.file, &track.audio, Some(&dir_id)).await?;
        json_children.push(json);
        xml_children.push(xml);
    }

    let payload = SubsonicPayload {
        json_key: "directory",
        json_value: json!({
            "id": dir_id,
            "parent": parent_id,
            "name": folder.folder.name,
            "child": json_children,
        }),
        xml: XmlElement::new("directory")
            .attr("id", dir_id)
            .opt_attr("parent", parent_id)
            .attr("name", &folder.folder.name)
            .children(xml_children),
    };
    Ok(ok_response(format, Some(payload)))
}

async fn get_artist(
    state: &AppState,
    user: &User,
    params: &ParamMap,
    format: ResponseFormat,
) -> std::result::Result<Response, SubsonicError> {
    let alias = alias_for_param(state, user.id, params.required("id")?).await?;
    if alias.kind != SubsonicIdKind::Artist.as_str() {
        return Err(SubsonicError::not_found("Artist"));
    }
    let artist =
        parse_artist_key(&alias.internal_key).ok_or_else(|| SubsonicError::not_found("Artist"))?;
    let albums = album_rows_for_artist(state, user, &artist, None).await?;
    let album_count = albums.len();
    let mut json_albums = Vec::new();
    let mut xml_albums = Vec::new();
    for album in albums {
        json_albums.push(album.json);
        xml_albums.push(album.xml);
    }

    let payload = SubsonicPayload {
        json_key: "artist",
        json_value: json!({
            "id": alias.numeric_id.to_string(),
            "name": artist,
            "albumCount": album_count,
            "album": json_albums,
        }),
        xml: XmlElement::new("artist")
            .attr("id", alias.numeric_id)
            .attr("name", artist)
            .attr("albumCount", album_count)
            .children(xml_albums),
    };
    Ok(ok_response(format, Some(payload)))
}

async fn get_album(
    state: &AppState,
    user: &User,
    params: &ParamMap,
    format: ResponseFormat,
) -> std::result::Result<Response, SubsonicError> {
    let alias = alias_for_param(state, user.id, params.required("id")?).await?;
    if alias.kind != SubsonicIdKind::Album.as_str() {
        return Err(SubsonicError::not_found("Album"));
    }
    let (artist, album) =
        parse_album_key(&alias.internal_key).ok_or_else(|| SubsonicError::not_found("Album"))?;
    let tracks = album_tracks(state, user, &artist, &album).await?;
    if tracks.is_empty() {
        return Err(SubsonicError::not_found("Album"));
    }

    let mut duration = 0_i64;
    let mut year = None;
    let mut cover_art = None;
    let mut json_songs = Vec::new();
    let mut xml_songs = Vec::new();
    let artist_id =
        subsonic_id_for(state, user.id, SubsonicIdKind::Artist, artist_key(&artist)).await?;
    for track in &tracks {
        duration += track.audio.duration_secs.unwrap_or(0.0).round() as i64;
        year = year.or(track.audio.year);
        if cover_art.is_none() && track.audio.has_cover_art {
            cover_art = Some(
                subsonic_id_for(state, user.id, SubsonicIdKind::Song, track.file.id.to_hex())
                    .await?,
            );
        }
    }
    for track in tracks {
        let (json, xml) = song_entry(state, user.id, &track.file, &track.audio, None).await?;
        json_songs.push(json);
        xml_songs.push(xml);
    }

    let payload = SubsonicPayload {
        json_key: "album",
        json_value: json!({
            "id": alias.numeric_id.to_string(),
            "name": album,
            "artist": artist,
            "artistId": &artist_id,
            "songCount": json_songs.len(),
            "duration": duration,
            "coverArt": cover_art,
            "year": year,
            "song": json_songs,
        }),
        xml: XmlElement::new("album")
            .attr("id", alias.numeric_id)
            .attr("name", album)
            .attr("artist", artist)
            .attr("artistId", artist_id)
            .attr("songCount", json_songs.len())
            .attr("duration", duration)
            .opt_attr("coverArt", cover_art)
            .opt_attr("year", year)
            .children(xml_songs),
    };
    Ok(ok_response(format, Some(payload)))
}

async fn get_song(
    state: &AppState,
    user: &User,
    params: &ParamMap,
    format: ResponseFormat,
) -> std::result::Result<Response, SubsonicError> {
    let file = file_for_song_id(state, user, params.required("id")?).await?;
    let audio = extract_audio_meta(&file);
    let (json, xml) = song_entry(state, user.id, &file, &audio, None).await?;
    let payload = SubsonicPayload {
        json_key: "song",
        json_value: json,
        xml: XmlElement {
            name: "song".to_string(),
            ..xml
        },
    };
    Ok(ok_response(format, Some(payload)))
}

async fn search3(
    state: &AppState,
    user: &User,
    params: &ParamMap,
    format: ResponseFormat,
) -> std::result::Result<Response, SubsonicError> {
    let query = params.first("query").unwrap_or("").trim().to_lowercase();
    let artist_count = params.i64_param("artistCount", 20, MAX_PAGE_SIZE) as usize;
    let album_count = params.i64_param("albumCount", 20, MAX_PAGE_SIZE) as usize;
    let artist_offset = params.i64_param("artistOffset", 0, i64::MAX) as usize;
    let album_offset = params.i64_param("albumOffset", 0, i64::MAX) as usize;
    let song_offset = params.i64_param("songOffset", 0, i64::MAX) as u64;
    let song_count = params.i64_param("songCount", 20, MAX_PAGE_SIZE);
    let music_folder_id = params.first("musicFolderId");

    let artists = artist_rows(state, user, music_folder_id).await?;
    let mut json_artists = Vec::new();
    let mut xml_artists = Vec::new();
    for artist in artists
        .into_iter()
        .filter(|artist| query.is_empty() || artist.name.to_lowercase().contains(&query))
        .skip(artist_offset)
        .take(artist_count)
    {
        json_artists.push(json!({
            "id": artist.id,
            "name": artist.name,
            "albumCount": artist.album_count,
        }));
        xml_artists.push(
            XmlElement::new("artist")
                .attr("id", artist.id)
                .attr("name", artist.name)
                .attr("albumCount", artist.album_count),
        );
    }

    let albums = album_rows(state, user, music_folder_id, None).await?;
    let mut json_albums = Vec::new();
    let mut xml_albums = Vec::new();
    for album in albums
        .into_iter()
        .filter(|album| {
            query.is_empty()
                || album.name.to_lowercase().contains(&query)
                || album.artist.to_lowercase().contains(&query)
        })
        .skip(album_offset)
        .take(album_count)
    {
        json_albums.push(album.json);
        xml_albums.push(album.xml);
    }

    let files = search_tracks(state, user, music_folder_id, &query, song_offset, song_count).await?;
    let mut json_songs = Vec::new();
    let mut xml_songs = Vec::new();
    for track in files {
        let (json, xml) = song_entry(state, user.id, &track.file, &track.audio, None).await?;
        json_songs.push(json);
        xml_songs.push(xml);
    }

    let payload = SubsonicPayload {
        json_key: "searchResult3",
        json_value: json!({
            "artist": json_artists,
            "album": json_albums,
            "song": json_songs,
        }),
        xml: XmlElement::new("searchResult3")
            .children(xml_artists)
            .children(xml_albums)
            .children(xml_songs),
    };
    Ok(ok_response(format, Some(payload)))
}

async fn get_album_list2(
    state: &AppState,
    user: &User,
    params: &ParamMap,
    format: ResponseFormat,
) -> std::result::Result<Response, SubsonicError> {
    let list_type = params.required("type")?.to_ascii_lowercase();
    let size = params.i64_param("size", 10, MAX_PAGE_SIZE) as usize;
    let offset = params.i64_param("offset", 0, i64::MAX) as usize;
    let music_folder_id = params.first("musicFolderId");

    let mut albums = match list_type.as_str() {
        "random" | "newest" | "alphabeticalbyname" | "alphabeticalbyartist" => {
            album_rows(state, user, music_folder_id, None).await?
        }
        "frequent" | "recent" | "starred" | "byyear" | "bygenre" => Vec::new(),
        _ => Vec::new(),
    };

    match list_type.as_str() {
        "random" => {
            // Deterministic enough for one request; avoid adding a RNG trait
            // dependency just for a compatibility endpoint.
            albums.sort_by(|a, b| a.id.cmp(&b.id));
            let len = albums.len().max(1);
            let offset =
                (Utc::now().timestamp_nanos_opt().unwrap_or_default() as usize).wrapping_rem(len);
            albums.rotate_left(offset);
        }
        "newest" => albums.sort_by(|a, b| b.created_at.cmp(&a.created_at)),
        "alphabeticalbyartist" => albums.sort_by(|a, b| {
            a.artist
                .to_lowercase()
                .cmp(&b.artist.to_lowercase())
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        }),
        _ => albums.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
    }

    let page = albums.into_iter().skip(offset).take(size);
    let mut json_albums = Vec::new();
    let mut xml_albums = Vec::new();
    for album in page {
        json_albums.push(album.json);
        xml_albums.push(album.xml);
    }

    let payload = SubsonicPayload {
        json_key: "albumList2",
        json_value: json!({ "album": json_albums }),
        xml: XmlElement::new("albumList2").children(xml_albums),
    };
    Ok(ok_response(format, Some(payload)))
}

async fn get_random_songs(
    state: &AppState,
    user: &User,
    params: &ParamMap,
    format: ResponseFormat,
) -> std::result::Result<Response, SubsonicError> {
    let size = params.i64_param("size", 10, MAX_PAGE_SIZE);
    let mut parent_ids = scoped_parent_ids(state, user, params.first("musicFolderId")).await?;
    if parent_ids.is_empty() {
        parent_ids = vec![Bson::Null];
    }
    let files_coll = state.db.collection::<File>("files");
    let pipeline = vec![
        doc! {
            "$match": {
                "parent_id": { "$in": &parent_ids },
                "mime_type": { "$regex": "^audio/" },
                "deleted_at": Bson::Null,
            }
        },
        doc! { "$sample": { "size": size } },
    ];
    let mut cursor = files_coll
        .aggregate(pipeline)
        .await
        .map_err(to_subsonic_error)?;
    let mut tracks = Vec::new();
    while let Some(doc) = cursor.next().await {
        let file: File = bson::from_document(doc.map_err(to_subsonic_error)?)
            .map_err(|e| SubsonicError::generic(e.to_string()))?;
        tracks.push(TrackWithMeta::from_file(file));
    }

    let mut json_songs = Vec::new();
    let mut xml_songs = Vec::new();
    for track in tracks {
        let (json, xml) = song_entry(state, user.id, &track.file, &track.audio, None).await?;
        json_songs.push(json);
        xml_songs.push(xml);
    }
    let payload = SubsonicPayload {
        json_key: "randomSongs",
        json_value: json!({ "song": json_songs }),
        xml: XmlElement::new("randomSongs").children(xml_songs),
    };
    Ok(ok_response(format, Some(payload)))
}

async fn stream_song(
    state: &AppState,
    user: &User,
    params: &ParamMap,
    headers: &HeaderMap,
    attachment: bool,
) -> std::result::Result<Response, SubsonicError> {
    let file = file_for_song_id(state, user, params.required("id")?).await?;
    stream_file(state, user, &file, headers, attachment).await
}

async fn get_cover_art(
    state: &AppState,
    user: &User,
    params: &ParamMap,
) -> std::result::Result<Response, SubsonicError> {
    let alias = alias_for_param(state, user.id, params.required("id")?).await?;
    let file = match alias.kind.as_str() {
        "song" => {
            let file = file_for_alias(state, user, &alias).await?;
            if extract_audio_meta(&file).has_cover_art {
                Some(file)
            } else {
                None
            }
        }
        "album" => {
            let (artist, album) = parse_album_key(&alias.internal_key)
                .ok_or_else(|| SubsonicError::not_found("Cover art"))?;
            album_tracks(state, user, &artist, &album)
                .await?
                .into_iter()
                .find(|track| track.audio.has_cover_art)
                .map(|track| track.file)
        }
        "artist" => {
            let artist = parse_artist_key(&alias.internal_key)
                .ok_or_else(|| SubsonicError::not_found("Cover art"))?;
            artist_tracks(state, user, &artist)
                .await?
                .into_iter()
                .find(|track| track.audio.has_cover_art)
                .map(|track| track.file)
        }
        "folder" => {
            let folder_id = ObjectId::parse_str(&alias.internal_key)
                .map_err(|_| SubsonicError::not_found("Cover art"))?;
            if !accessible_music_folders(state, user.id)
                .await
                .map_err(to_subsonic_error)?
                .iter()
                .any(|folder| folder.folder.id == folder_id)
            {
                return Err(SubsonicError::not_found("Cover art"));
            }
            direct_folder_tracks(state, folder_id)
                .await?
                .into_iter()
                .find(|track| track.audio.has_cover_art)
                .map(|track| track.file)
        }
        _ => None,
    }
    .ok_or_else(|| SubsonicError::not_found("Cover art"))?;

    stream_thumbnail(state, &file).await
}

async fn get_playlists(
    state: &AppState,
    user: &User,
    format: ResponseFormat,
) -> std::result::Result<Response, SubsonicError> {
    let coll = state.db.collection::<Playlist>("playlists");
    let options = FindOptions::builder()
        .sort(doc! { "updated_at": -1 })
        .build();
    let mut cursor = coll
        .find(doc! { "owner_id": user.id })
        .with_options(options)
        .await
        .map_err(to_subsonic_error)?;
    let mut json_rows = Vec::new();
    let mut xml_rows = Vec::new();
    while cursor.advance().await.map_err(to_subsonic_error)? {
        let playlist = cursor.deserialize_current().map_err(to_subsonic_error)?;
        let id = subsonic_id_for(
            state,
            user.id,
            SubsonicIdKind::Playlist,
            playlist.id.to_hex(),
        )
        .await?;
        let duration = playlist_duration(state, user, &playlist).await?;
        json_rows.push(json!({
            "id": id,
            "name": playlist.name,
            "comment": playlist.description,
            "songCount": playlist.tracks.len(),
            "duration": duration,
            "created": playlist.created_at.to_rfc3339(),
            "changed": playlist.updated_at.to_rfc3339(),
            "owner": user.username,
            "public": false,
        }));
        xml_rows.push(
            XmlElement::new("playlist")
                .attr("id", id)
                .attr("name", playlist.name)
                .opt_attr("comment", playlist.description)
                .attr("songCount", playlist.tracks.len())
                .attr("duration", duration)
                .attr("created", playlist.created_at.to_rfc3339())
                .attr("changed", playlist.updated_at.to_rfc3339())
                .attr("owner", &user.username)
                .attr("public", "false"),
        );
    }

    let payload = SubsonicPayload {
        json_key: "playlists",
        json_value: json!({ "playlist": json_rows }),
        xml: XmlElement::new("playlists").children(xml_rows),
    };
    Ok(ok_response(format, Some(payload)))
}

async fn get_playlist(
    state: &AppState,
    user: &User,
    params: &ParamMap,
    format: ResponseFormat,
) -> std::result::Result<Response, SubsonicError> {
    let playlist = playlist_for_id(state, user, params.required("id")?).await?;
    let (json_playlist, xml_playlist) = playlist_detail(state, user, playlist).await?;
    let payload = SubsonicPayload {
        json_key: "playlist",
        json_value: json_playlist,
        xml: xml_playlist,
    };
    Ok(ok_response(format, Some(payload)))
}

async fn create_playlist(
    state: &AppState,
    user: &User,
    params: &ParamMap,
    format: ResponseFormat,
) -> std::result::Result<Response, SubsonicError> {
    let name = params.required("name")?.trim().to_string();
    if name.is_empty() {
        return Err(SubsonicError::generic("Playlist name cannot be empty"));
    }
    let coll = state.db.collection::<Playlist>("playlists");
    if coll
        .find_one(doc! { "owner_id": user.id, "name": &name })
        .await
        .map_err(to_subsonic_error)?
        .is_some()
    {
        return Err(SubsonicError::generic("Playlist already exists"));
    }

    let mut playlist = Playlist::new(user.id, name);
    let mut position = 0_u32;
    for song_id in params.all("songId") {
        let file = file_for_song_id(state, user, song_id).await?;
        if playlist.tracks.iter().any(|track| track.file_id == file.id) {
            continue;
        }
        position += 1;
        playlist.tracks.push(PlaylistTrack {
            file_id: file.id,
            position,
            added_at: Utc::now(),
        });
    }
    coll.insert_one(&playlist)
        .await
        .map_err(to_subsonic_error)?;

    let (json_playlist, xml_playlist) = playlist_detail(state, user, playlist).await?;
    let payload = SubsonicPayload {
        json_key: "playlist",
        json_value: json_playlist,
        xml: xml_playlist,
    };
    Ok(ok_response(format, Some(payload)))
}

async fn update_playlist(
    state: &AppState,
    user: &User,
    params: &ParamMap,
    format: ResponseFormat,
) -> std::result::Result<Response, SubsonicError> {
    let mut playlist = playlist_for_id(state, user, params.required("playlistId")?).await?;
    if let Some(name) = params.first("name") {
        playlist.name = name.trim().to_string();
    }
    if let Some(comment) = params.first("comment") {
        playlist.description = if comment.trim().is_empty() {
            None
        } else {
            Some(comment.trim().to_string())
        };
    }

    let mut remove_ids: HashSet<ObjectId> = HashSet::new();
    for song_id in params.all("songIdToRemove") {
        remove_ids.insert(file_for_song_id(state, user, song_id).await?.id);
    }
    if !remove_ids.is_empty() {
        playlist
            .tracks
            .retain(|track| !remove_ids.contains(&track.file_id));
    }
    let mut remove_indexes: Vec<usize> = params
        .all("songIndexToRemove")
        .into_iter()
        .filter_map(|idx| idx.parse::<usize>().ok())
        .collect();
    remove_indexes.sort_unstable_by(|a, b| b.cmp(a));
    for idx in remove_indexes {
        if idx < playlist.tracks.len() {
            playlist.tracks.remove(idx);
        }
    }

    let mut next_position = playlist
        .tracks
        .iter()
        .map(|track| track.position)
        .max()
        .unwrap_or(0);
    for song_id in params.all("songIdToAdd") {
        let file = file_for_song_id(state, user, song_id).await?;
        if playlist.tracks.iter().any(|track| track.file_id == file.id) {
            continue;
        }
        next_position += 1;
        playlist.tracks.push(PlaylistTrack {
            file_id: file.id,
            position: next_position,
            added_at: Utc::now(),
        });
    }
    renumber_playlist(&mut playlist);
    playlist.updated_at = Utc::now();

    let tracks_bson: Vec<Bson> = playlist
        .tracks
        .iter()
        .map(|track| bson::to_bson(track).unwrap_or(Bson::Null))
        .collect();
    state
        .db
        .collection::<Playlist>("playlists")
        .update_one(
            doc! { "_id": playlist.id, "owner_id": user.id },
            doc! {
                "$set": {
                    "name": &playlist.name,
                    "description": &playlist.description,
                    "tracks": tracks_bson,
                    "updated_at": bson::DateTime::from_chrono(playlist.updated_at),
                }
            },
        )
        .await
        .map_err(to_subsonic_error)?;

    let (json_playlist, xml_playlist) = playlist_detail(state, user, playlist).await?;
    let payload = SubsonicPayload {
        json_key: "playlist",
        json_value: json_playlist,
        xml: xml_playlist,
    };
    Ok(ok_response(format, Some(payload)))
}

async fn delete_playlist(
    state: &AppState,
    user: &User,
    params: &ParamMap,
    format: ResponseFormat,
) -> std::result::Result<Response, SubsonicError> {
    let playlist = playlist_for_id(state, user, params.required("id")?).await?;
    state
        .db
        .collection::<Playlist>("playlists")
        .delete_one(doc! { "_id": playlist.id, "owner_id": user.id })
        .await
        .map_err(to_subsonic_error)?;
    Ok(ok_response(format, None))
}

fn renumber_playlist(playlist: &mut Playlist) {
    for (idx, track) in playlist.tracks.iter_mut().enumerate() {
        track.position = (idx + 1) as u32;
    }
}

async fn playlist_for_id(
    state: &AppState,
    user: &User,
    id: &str,
) -> std::result::Result<Playlist, SubsonicError> {
    let alias = alias_for_param(state, user.id, id).await?;
    if alias.kind != SubsonicIdKind::Playlist.as_str() {
        return Err(SubsonicError::not_found("Playlist"));
    }
    let playlist_id = ObjectId::parse_str(&alias.internal_key)
        .map_err(|_| SubsonicError::not_found("Playlist"))?;
    state
        .db
        .collection::<Playlist>("playlists")
        .find_one(doc! { "_id": playlist_id, "owner_id": user.id })
        .await
        .map_err(to_subsonic_error)?
        .ok_or_else(|| SubsonicError::not_found("Playlist"))
}

async fn playlist_detail(
    state: &AppState,
    user: &User,
    mut playlist: Playlist,
) -> std::result::Result<(Value, XmlElement), SubsonicError> {
    playlist.tracks.sort_by_key(|track| track.position);
    let id = subsonic_id_for(
        state,
        user.id,
        SubsonicIdKind::Playlist,
        playlist.id.to_hex(),
    )
    .await?;
    let mut duration = 0_i64;
    let mut json_entries = Vec::new();
    let mut xml_entries = Vec::new();
    for track in &playlist.tracks {
        if let Some(file) = state
            .db
            .collection::<File>("files")
            .find_one(doc! { "_id": track.file_id, "deleted_at": Bson::Null })
            .await
            .map_err(to_subsonic_error)?
        {
            if !file_is_in_music_library(state, user, &file).await? {
                continue;
            }
            let audio = extract_audio_meta(&file);
            duration += audio.duration_secs.unwrap_or(0.0).round() as i64;
            let (json, mut xml) = song_entry(state, user.id, &file, &audio, None).await?;
            xml.name = "entry".to_string();
            json_entries.push(json);
            xml_entries.push(xml);
        }
    }
    let json = json!({
        "id": id,
        "name": playlist.name,
        "comment": playlist.description,
        "songCount": json_entries.len(),
        "duration": duration,
        "created": playlist.created_at.to_rfc3339(),
        "changed": playlist.updated_at.to_rfc3339(),
        "owner": user.username,
        "public": false,
        "entry": json_entries,
    });
    let xml = XmlElement::new("playlist")
        .attr("id", id)
        .attr("name", playlist.name)
        .opt_attr("comment", playlist.description)
        .attr("songCount", json_entries.len())
        .attr("duration", duration)
        .attr("created", playlist.created_at.to_rfc3339())
        .attr("changed", playlist.updated_at.to_rfc3339())
        .attr("owner", &user.username)
        .attr("public", "false")
        .children(xml_entries);
    Ok((json, xml))
}

async fn playlist_duration(
    state: &AppState,
    user: &User,
    playlist: &Playlist,
) -> std::result::Result<i64, SubsonicError> {
    let file_ids: Vec<Bson> = playlist
        .tracks
        .iter()
        .map(|track| Bson::ObjectId(track.file_id))
        .collect();
    if file_ids.is_empty() {
        return Ok(0);
    }
    let mut cursor = state
        .db
        .collection::<File>("files")
        .find(doc! { "_id": { "$in": file_ids }, "deleted_at": Bson::Null })
        .await
        .map_err(to_subsonic_error)?;
    let mut duration = 0_i64;
    while cursor.advance().await.map_err(to_subsonic_error)? {
        let file = cursor.deserialize_current().map_err(to_subsonic_error)?;
        if file_is_in_music_library(state, user, &file).await? {
            duration += extract_audio_meta(&file)
                .duration_secs
                .unwrap_or(0.0)
                .round() as i64;
        }
    }
    Ok(duration)
}

#[derive(Clone)]
struct AccessibleFolder {
    folder: Folder,
    path: String,
}

async fn accessible_music_folders(
    state: &AppState,
    user_id: ObjectId,
) -> Result<Vec<AccessibleFolder>> {
    let folders_coll = state.db.collection::<Folder>("folders");
    let mut owned_cursor = folders_coll
        .find(doc! { "owner_id": user_id, "deleted_at": Bson::Null })
        .await?;
    let mut owned = Vec::new();
    while owned_cursor.advance().await? {
        owned.push(owned_cursor.deserialize_current()?);
    }
    let included = crate::routes::files::resolve_included_folder_ids_by(&owned, |folder| {
        folder.music_include.as_include_flag()
    });
    let owned_by_id: HashMap<ObjectId, &Folder> = owned.iter().map(|f| (f.id, f)).collect();
    let mut out = Vec::new();
    for folder_id in included.into_iter().flatten() {
        if let Some(folder) = owned_by_id.get(&folder_id) {
            out.push(AccessibleFolder {
                folder: (*folder).clone(),
                path: build_folder_path(folder_id, &owned_by_id),
            });
        }
    }

    let shares_coll = state
        .db
        .collection::<crate::models::FolderShare>("folder_shares");
    let shares: Vec<crate::models::FolderShare> = shares_coll
        .find(doc! { "grantee_id": user_id, "music_include": "include" })
        .await?
        .try_collect()
        .await?;
    let mut owner_cache: HashMap<ObjectId, Vec<Folder>> = HashMap::new();
    for share in shares {
        if !owner_cache.contains_key(&share.owner_id) {
            let owner_folders: Vec<Folder> = folders_coll
                .find(doc! { "owner_id": share.owner_id, "deleted_at": Bson::Null })
                .await?
                .try_collect()
                .await?;
            owner_cache.insert(share.owner_id, owner_folders);
        }
        let Some(owner_folders) = owner_cache.get(&share.owner_id) else {
            continue;
        };
        let by_id: HashMap<ObjectId, &Folder> = owner_folders.iter().map(|f| (f.id, f)).collect();
        let mut children: HashMap<ObjectId, Vec<ObjectId>> = HashMap::new();
        for folder in owner_folders {
            if let Some(parent_id) = folder.parent_id {
                children.entry(parent_id).or_default().push(folder.id);
            }
        }
        let mut stack = vec![share.folder_id];
        let mut seen = HashSet::new();
        while let Some(folder_id) = stack.pop() {
            if !seen.insert(folder_id) {
                continue;
            }
            if let Some(folder) = by_id.get(&folder_id) {
                out.push(AccessibleFolder {
                    folder: (*folder).clone(),
                    path: build_folder_path(folder_id, &by_id),
                });
            }
            if let Some(child_ids) = children.get(&folder_id) {
                stack.extend(child_ids);
            }
        }
    }

    out.sort_by(|a, b| a.path.to_lowercase().cmp(&b.path.to_lowercase()));
    Ok(out)
}

async fn scoped_parent_ids(
    state: &AppState,
    user: &User,
    music_folder_id: Option<&str>,
) -> std::result::Result<Vec<Bson>, SubsonicError> {
    let mut parent_ids = music_included_parent_ids(state, user.id)
        .await
        .map_err(to_subsonic_error)?;
    if let Some(id) = music_folder_id {
        let alias = alias_for_param(state, user.id, id).await?;
        if alias.kind != SubsonicIdKind::Folder.as_str() {
            return Err(SubsonicError::not_found("Music folder"));
        }
        let folder_id = ObjectId::parse_str(&alias.internal_key)
            .map_err(|_| SubsonicError::not_found("Music folder"))?;
        parent_ids = restrict_to_subtrees(state, user.id, parent_ids, &[folder_id])
            .await
            .map_err(to_subsonic_error)?;
    }
    Ok(parent_ids)
}

#[derive(Clone)]
struct ArtistRow {
    id: String,
    name: String,
    album_count: i64,
}

async fn artist_rows(
    state: &AppState,
    user: &User,
    music_folder_id: Option<&str>,
) -> std::result::Result<Vec<ArtistRow>, SubsonicError> {
    let parent_ids = scoped_parent_ids(state, user, music_folder_id).await?;
    if parent_ids.is_empty() {
        return Ok(Vec::new());
    }
    let normalise = |field: &str, default: &str| {
        doc! {
            "$let": {
                "vars": { "v": { "$ifNull": [field, ""] } },
                "in": { "$cond": [{ "$eq": ["$$v", ""] }, default, "$$v"] },
            }
        }
    };
    let pipeline = vec![
        doc! { "$match": {
            "parent_id": { "$in": &parent_ids },
            "mime_type": { "$regex": "^audio/" },
            "deleted_at": Bson::Null,
        }},
        doc! { "$group": {
            "_id": {
                "artist": normalise("$metadata.audio.artist", "Unknown Artist"),
                "album": normalise("$metadata.audio.album", "Unknown Album"),
            },
        }},
        doc! { "$group": {
            "_id": "$_id.artist",
            "album_count": { "$sum": 1 },
        }},
    ];
    let mut cursor = state
        .db
        .collection::<File>("files")
        .aggregate(pipeline)
        .await
        .map_err(to_subsonic_error)?;
    let mut rows = Vec::new();
    while let Some(doc) = cursor.next().await {
        let doc = doc.map_err(to_subsonic_error)?;
        let name = doc.get_str("_id").unwrap_or("Unknown Artist").to_string();
        let id = subsonic_id_for(state, user.id, SubsonicIdKind::Artist, artist_key(&name)).await?;
        rows.push(ArtistRow {
            id,
            name,
            album_count: get_i64(&doc, "album_count"),
        });
    }
    rows.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(rows)
}

fn group_artists(rows: Vec<ArtistRow>) -> BTreeMap<String, Vec<ArtistRow>> {
    let mut groups: BTreeMap<String, Vec<ArtistRow>> = BTreeMap::new();
    for row in rows {
        let first = row
            .name
            .chars()
            .find(|c| c.is_alphanumeric())
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "#".to_string());
        groups.entry(first).or_default().push(row);
    }
    groups
}

struct AlbumRow {
    id: String,
    name: String,
    artist: String,
    created_at: String,
    json: Value,
    xml: XmlElement,
}

async fn album_rows_for_artist(
    state: &AppState,
    user: &User,
    artist: &str,
    music_folder_id: Option<&str>,
) -> std::result::Result<Vec<AlbumRow>, SubsonicError> {
    album_rows(state, user, music_folder_id, Some(artist)).await
}

async fn album_rows(
    state: &AppState,
    user: &User,
    music_folder_id: Option<&str>,
    artist_filter: Option<&str>,
) -> std::result::Result<Vec<AlbumRow>, SubsonicError> {
    let parent_ids = scoped_parent_ids(state, user, music_folder_id).await?;
    if parent_ids.is_empty() {
        return Ok(Vec::new());
    }
    let normalise = |field: &str, default: &str| {
        doc! {
            "$let": {
                "vars": { "v": { "$ifNull": [field, ""] } },
                "in": { "$cond": [{ "$eq": ["$$v", ""] }, default, "$$v"] },
            }
        }
    };
    let mut match_doc = doc! {
        "parent_id": { "$in": &parent_ids },
        "mime_type": { "$regex": "^audio/" },
        "deleted_at": Bson::Null,
    };
    if let Some(artist) = artist_filter {
        if artist == "Unknown Artist" {
            match_doc.insert(
                "$or",
                vec![
                    doc! { "metadata.audio.artist": Bson::Null },
                    doc! { "metadata.audio.artist": "" },
                ],
            );
        } else {
            match_doc.insert(
                "$or",
                vec![
                    doc! { "metadata.audio.artist": artist },
                    doc! { "metadata.audio.album_artist": artist },
                ],
            );
        }
    }
    let pipeline = vec![
        doc! { "$match": match_doc },
        doc! { "$sort": { "created_at": -1 } },
        doc! { "$group": {
            "_id": {
                "artist": normalise("$metadata.audio.artist", "Unknown Artist"),
                "album": normalise("$metadata.audio.album", "Unknown Album"),
            },
            "song_count": { "$sum": 1 },
            "duration": { "$sum": { "$ifNull": ["$metadata.audio.duration_secs", 0] } },
            "year": { "$first": "$metadata.audio.year" },
            "created_at": { "$first": "$created_at" },
            "cover_candidates": { "$push": { "$cond": [
                { "$eq": ["$metadata.audio.has_cover_art", true] },
                "$_id",
                "$$REMOVE",
            ]}},
        }},
        doc! { "$addFields": { "cover_file_id": { "$arrayElemAt": ["$cover_candidates", 0] } } },
    ];

    let mut cursor = state
        .db
        .collection::<File>("files")
        .aggregate(pipeline)
        .await
        .map_err(to_subsonic_error)?;
    let mut rows = Vec::new();
    while let Some(doc) = cursor.next().await {
        let doc = doc.map_err(to_subsonic_error)?;
        let id_doc = doc
            .get_document("_id")
            .map_err(|_| SubsonicError::generic("Invalid album row"))?;
        let artist = id_doc
            .get_str("artist")
            .unwrap_or("Unknown Artist")
            .to_string();
        let name = id_doc
            .get_str("album")
            .unwrap_or("Unknown Album")
            .to_string();
        let id = subsonic_id_for(
            state,
            user.id,
            SubsonicIdKind::Album,
            album_key(&artist, &name),
        )
        .await?;
        let artist_id =
            subsonic_id_for(state, user.id, SubsonicIdKind::Artist, artist_key(&artist)).await?;
        let song_count = get_i64(&doc, "song_count");
        let duration = get_f64(&doc, "duration").round() as i64;
        let year = doc
            .get_i32("year")
            .ok()
            .or_else(|| doc.get_i64("year").ok().map(|n| n as i32));
        let created_at = doc
            .get_datetime("created_at")
            .map(|dt| dt.to_chrono().to_rfc3339())
            .unwrap_or_else(|_| Utc::now().to_rfc3339());
        let cover_art = doc
            .get_object_id("cover_file_id")
            .ok()
            .map(|oid| oid.to_hex());
        let cover_art_alias = match cover_art {
            Some(file_id) => {
                Some(subsonic_id_for(state, user.id, SubsonicIdKind::Song, file_id).await?)
            }
            None => None,
        };

        let json = json!({
            "id": id,
            "name": name,
            "artist": artist,
            "artistId": &artist_id,
            "songCount": song_count,
            "duration": duration,
            "coverArt": cover_art_alias,
            "year": year,
            "created": created_at,
        });
        let xml = XmlElement::new("album")
            .attr("id", &id)
            .attr("name", &name)
            .attr("artist", &artist)
            .attr("artistId", &artist_id)
            .attr("songCount", song_count)
            .attr("duration", duration)
            .opt_attr("coverArt", cover_art_alias)
            .opt_attr("year", year)
            .attr("created", &created_at);
        rows.push(AlbumRow {
            id,
            name,
            artist,
            created_at,
            json,
            xml,
        });
    }
    rows.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(rows)
}

struct TrackWithMeta {
    file: File,
    audio: AudioMeta,
}

impl TrackWithMeta {
    fn from_file(file: File) -> Self {
        let audio = extract_audio_meta(&file);
        Self { file, audio }
    }
}

async fn album_tracks(
    state: &AppState,
    user: &User,
    artist: &str,
    album: &str,
) -> std::result::Result<Vec<TrackWithMeta>, SubsonicError> {
    let parent_ids = scoped_parent_ids(state, user, None).await?;
    let mut filters = vec![
        doc! { "parent_id": { "$in": &parent_ids } },
        doc! { "mime_type": { "$regex": "^audio/" } },
        doc! { "deleted_at": Bson::Null },
    ];
    if album == "Unknown Album" {
        filters.push(doc! { "$or": [
            { "metadata.audio.album": Bson::Null },
            { "metadata.audio.album": "" },
        ]});
    } else {
        filters.push(doc! { "metadata.audio.album": album });
    }
    if artist == "Unknown Artist" {
        filters.push(doc! { "$or": [
            { "metadata.audio.artist": Bson::Null },
            { "metadata.audio.artist": "" },
        ]});
    } else {
        filters.push(doc! { "$or": [
            { "metadata.audio.artist": artist },
            { "metadata.audio.album_artist": artist },
        ]});
    }
    let mut cursor = state
        .db
        .collection::<File>("files")
        .find(doc! { "$and": filters })
        .await
        .map_err(to_subsonic_error)?;
    let mut tracks = Vec::new();
    while cursor.advance().await.map_err(to_subsonic_error)? {
        tracks.push(TrackWithMeta::from_file(
            cursor.deserialize_current().map_err(to_subsonic_error)?,
        ));
    }
    tracks.sort_by(|a, b| {
        a.audio
            .disc_number
            .unwrap_or(0)
            .cmp(&b.audio.disc_number.unwrap_or(0))
            .then_with(|| {
                a.audio
                    .track_number
                    .unwrap_or(0)
                    .cmp(&b.audio.track_number.unwrap_or(0))
            })
            .then_with(|| a.file.name.to_lowercase().cmp(&b.file.name.to_lowercase()))
    });
    Ok(tracks)
}

async fn artist_tracks(
    state: &AppState,
    user: &User,
    artist: &str,
) -> std::result::Result<Vec<TrackWithMeta>, SubsonicError> {
    let parent_ids = scoped_parent_ids(state, user, None).await?;
    let artist_filter = if artist == "Unknown Artist" {
        doc! { "$or": [
            { "metadata.audio.artist": Bson::Null },
            { "metadata.audio.artist": "" },
        ]}
    } else {
        doc! { "$or": [
            { "metadata.audio.artist": artist },
            { "metadata.audio.album_artist": artist },
        ]}
    };
    let mut cursor = state
        .db
        .collection::<File>("files")
        .find(doc! {
            "$and": [
                { "parent_id": { "$in": &parent_ids } },
                { "mime_type": { "$regex": "^audio/" } },
                { "deleted_at": Bson::Null },
                artist_filter,
            ]
        })
        .await
        .map_err(to_subsonic_error)?;
    let mut tracks = Vec::new();
    while cursor.advance().await.map_err(to_subsonic_error)? {
        tracks.push(TrackWithMeta::from_file(
            cursor.deserialize_current().map_err(to_subsonic_error)?,
        ));
    }
    Ok(tracks)
}

async fn direct_folder_tracks(
    state: &AppState,
    folder_id: ObjectId,
) -> std::result::Result<Vec<TrackWithMeta>, SubsonicError> {
    let options = FindOptions::builder()
        .sort(doc! { "metadata.audio.disc_number": 1, "metadata.audio.track_number": 1, "name": 1 })
        .build();
    let mut cursor = state
        .db
        .collection::<File>("files")
        .find(doc! {
            "parent_id": folder_id,
            "mime_type": { "$regex": "^audio/" },
            "deleted_at": Bson::Null,
        })
        .with_options(options)
        .await
        .map_err(to_subsonic_error)?;
    let mut tracks = Vec::new();
    while cursor.advance().await.map_err(to_subsonic_error)? {
        tracks.push(TrackWithMeta::from_file(
            cursor.deserialize_current().map_err(to_subsonic_error)?,
        ));
    }
    Ok(tracks)
}

async fn search_tracks(
    state: &AppState,
    user: &User,
    music_folder_id: Option<&str>,
    query: &str,
    offset: u64,
    limit: i64,
) -> std::result::Result<Vec<TrackWithMeta>, SubsonicError> {
    let parent_ids = scoped_parent_ids(state, user, music_folder_id).await?;
    if parent_ids.is_empty() {
        return Ok(Vec::new());
    }
    let options = FindOptions::builder()
        .sort(doc! { "metadata.audio.title": 1, "created_at": -1 })
        .skip(offset)
        .limit(limit)
        .build();
    let mut filter = doc! {
        "parent_id": { "$in": parent_ids },
        "mime_type": { "$regex": "^audio/" },
        "deleted_at": Bson::Null,
    };
    if !query.is_empty() {
        let q_rx =
            doc! { "$regex": crate::services::sync_log::escape_regex(query), "$options": "i" };
        filter.insert(
            "$or",
            vec![
                doc! { "metadata.audio.title": &q_rx },
                doc! { "metadata.audio.artist": &q_rx },
                doc! { "metadata.audio.album": &q_rx },
                doc! { "metadata.audio.album_artist": &q_rx },
                doc! { "name": &q_rx },
            ],
        );
    }
    let mut cursor = state
        .db
        .collection::<File>("files")
        .find(filter)
        .with_options(options)
        .await
        .map_err(to_subsonic_error)?;
    let mut tracks = Vec::new();
    while cursor.advance().await.map_err(to_subsonic_error)? {
        tracks.push(TrackWithMeta::from_file(
            cursor.deserialize_current().map_err(to_subsonic_error)?,
        ));
    }
    Ok(tracks)
}

async fn file_for_song_id(
    state: &AppState,
    user: &User,
    id: &str,
) -> std::result::Result<File, SubsonicError> {
    let alias = alias_for_param(state, user.id, id).await?;
    if alias.kind != SubsonicIdKind::Song.as_str() {
        return Err(SubsonicError::not_found("Song"));
    }
    file_for_alias(state, user, &alias).await
}

async fn file_for_alias(
    state: &AppState,
    user: &User,
    alias: &SubsonicId,
) -> std::result::Result<File, SubsonicError> {
    let file_id =
        ObjectId::parse_str(&alias.internal_key).map_err(|_| SubsonicError::not_found("Song"))?;
    let file = state
        .db
        .collection::<File>("files")
        .find_one(doc! { "_id": file_id, "deleted_at": Bson::Null })
        .await
        .map_err(to_subsonic_error)?
        .ok_or_else(|| SubsonicError::not_found("Song"))?;
    if !file.mime_type.starts_with("audio/")
        || !file_is_in_music_library(state, user, &file).await?
    {
        return Err(SubsonicError::not_found("Song"));
    }
    Ok(file)
}

async fn file_is_in_music_library(
    state: &AppState,
    user: &User,
    file: &File,
) -> std::result::Result<bool, SubsonicError> {
    if check_file_access(&state.db, user.id, file.id)
        .await
        .map_err(to_subsonic_error)?
        .can_read()
        == false
    {
        return Ok(false);
    }
    let Some(parent_id) = file.parent_id else {
        return Ok(false);
    };
    let parent_ids = music_included_parent_ids(state, user.id)
        .await
        .map_err(to_subsonic_error)?;
    Ok(parent_ids
        .iter()
        .any(|id| matches!(id, Bson::ObjectId(oid) if *oid == parent_id)))
}

async fn song_entry(
    state: &AppState,
    owner_id: ObjectId,
    file: &File,
    audio: &AudioMeta,
    parent_alias: Option<&str>,
) -> std::result::Result<(Value, XmlElement), SubsonicError> {
    let id = subsonic_id_for(state, owner_id, SubsonicIdKind::Song, file.id.to_hex()).await?;
    let parent = match parent_alias {
        Some(parent) => Some(parent.to_string()),
        None => match file.parent_id {
            Some(parent_id) => Some(
                subsonic_id_for(state, owner_id, SubsonicIdKind::Folder, parent_id.to_hex())
                    .await?,
            ),
            None => None,
        },
    };
    let title = audio
        .title
        .as_deref()
        .filter(|value| !value.is_empty())
        .unwrap_or(&file.name)
        .to_string();
    let artist = audio
        .artist
        .as_deref()
        .filter(|value| !value.is_empty())
        .unwrap_or("Unknown Artist")
        .to_string();
    let album = audio
        .album
        .as_deref()
        .filter(|value| !value.is_empty())
        .unwrap_or("Unknown Album")
        .to_string();
    let suffix = file
        .name
        .rsplit_once('.')
        .map(|(_, suffix)| suffix.to_ascii_lowercase())
        .unwrap_or_default();
    let duration = audio.duration_secs.map(|secs| secs.round() as i64);
    let cover_art = if audio.has_cover_art {
        Some(id.clone())
    } else {
        None
    };
    let album_id = subsonic_id_for(
        state,
        owner_id,
        SubsonicIdKind::Album,
        album_key(&artist, &album),
    )
    .await?;
    let artist_id =
        subsonic_id_for(state, owner_id, SubsonicIdKind::Artist, artist_key(&artist)).await?;

    let json = json!({
        "id": id,
        "parent": parent,
        "title": title,
        "album": album,
        "artist": artist,
        "isDir": false,
        "coverArt": cover_art,
        "created": file.created_at.to_rfc3339(),
        "duration": duration,
        "size": file.size_bytes,
        "suffix": suffix,
        "contentType": file.mime_type,
        "isVideo": false,
        "path": file.storage_path,
        "albumId": album_id,
        "artistId": artist_id,
        "type": "music",
        "track": audio.track_number,
        "discNumber": audio.disc_number,
        "year": audio.year,
        "genre": audio.genre,
    });
    let xml = XmlElement::new("child")
        .attr("id", id)
        .opt_attr("parent", parent)
        .attr("title", title)
        .attr("album", album)
        .attr("artist", artist)
        .attr("isDir", "false")
        .opt_attr("coverArt", cover_art)
        .attr("created", file.created_at.to_rfc3339())
        .opt_attr("duration", duration)
        .attr("size", file.size_bytes)
        .attr("suffix", suffix)
        .attr("contentType", &file.mime_type)
        .attr("isVideo", "false")
        .attr("path", &file.storage_path)
        .attr("albumId", album_id)
        .attr("artistId", artist_id)
        .attr("type", "music")
        .opt_attr("track", audio.track_number)
        .opt_attr("discNumber", audio.disc_number)
        .opt_attr("year", audio.year)
        .opt_attr("genre", audio.genre.clone());
    Ok((json, xml))
}

async fn stream_file(
    state: &AppState,
    user: &User,
    file: &File,
    headers: &HeaderMap,
    attachment: bool,
) -> std::result::Result<Response, SubsonicError> {
    if !file_is_in_music_library(state, user, file).await? {
        return Err(SubsonicError::not_found("Song"));
    }
    let backend = state
        .storage
        .get_backend(file.storage_id)
        .await
        .map_err(to_subsonic_error)?;
    let total = file.size_bytes as u64;
    let disposition_type = if attachment { "attachment" } else { "inline" };
    let content_type = content_type_header(&file.mime_type);
    let content_disposition = content_disposition_header(disposition_type, &file.name);

    if let Some(range_value) = headers.get(header::RANGE) {
        let range_str = range_value
            .to_str()
            .map_err(|_| SubsonicError::generic("Invalid Range header"))?;
        let (start, end) = parse_range_header(range_str, total)
            .ok_or_else(|| SubsonicError::generic("Invalid Range header"))?;
        let length = end - start + 1;
        let reader = backend
            .read_range(&file.storage_path, start, length)
            .await
            .map_err(to_subsonic_error)?;
        let body = Body::from_stream(ReaderStream::new(reader));
        return Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_TYPE, content_type)
            .header(header::CONTENT_DISPOSITION, content_disposition)
            .header(header::CONTENT_LENGTH, length)
            .header(
                header::CONTENT_RANGE,
                format!("bytes {start}-{end}/{total}"),
            )
            .header(header::ACCEPT_RANGES, "bytes")
            .body(body)
            .map_err(|_| SubsonicError::generic("Failed to build stream response"));
    }

    let reader = backend
        .read(&file.storage_path)
        .await
        .map_err(to_subsonic_error)?;
    let body = Body::from_stream(ReaderStream::new(reader));
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_DISPOSITION, content_disposition)
        .header(header::CONTENT_LENGTH, file.size_bytes)
        .header(header::ACCEPT_RANGES, "bytes")
        .body(body)
        .map_err(|_| SubsonicError::generic("Failed to build stream response"))
}

async fn stream_thumbnail(
    state: &AppState,
    file: &File,
) -> std::result::Result<Response, SubsonicError> {
    let done = file.processing_tasks.iter().any(|task| {
        matches!(
            task.task_type,
            TaskType::Thumbnail | TaskType::AudioMetadata
        ) && task.status == ProcessingStatus::Done
    });
    if !done {
        return Err(SubsonicError::not_found("Cover art"));
    }
    let backend = state
        .storage
        .get_backend(file.storage_id)
        .await
        .map_err(to_subsonic_error)?;
    let thumb_path = format!(".thumbs/{}.jpg", file.id.to_hex());
    let reader = backend.read(&thumb_path).await.map_err(to_subsonic_error)?;
    let body = Body::from_stream(ReaderStream::new(reader));
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/jpeg")
        .body(body)
        .map_err(|_| SubsonicError::generic("Failed to build cover art response"))
}

fn content_type_header(mime_type: &str) -> HeaderValue {
    HeaderValue::from_str(mime_type)
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"))
}

fn content_disposition_header(disposition_type: &str, filename: &str) -> HeaderValue {
    let ascii_filename: String = filename
        .chars()
        .map(|ch| match ch {
            '"' | '\\' => '_',
            ch if ch.is_ascii_control() => '_',
            ch if ch.is_ascii() => ch,
            _ => '_',
        })
        .collect();
    let ascii_filename = if ascii_filename.is_empty() {
        "download"
    } else {
        ascii_filename.as_str()
    };
    let encoded_filename = urlencoding::encode(filename);
    let value = format!(
        "{disposition_type}; filename=\"{ascii_filename}\"; filename*=UTF-8''{encoded_filename}"
    );
    HeaderValue::from_str(&value).unwrap_or_else(|_| {
        if disposition_type == "attachment" {
            HeaderValue::from_static("attachment")
        } else {
            HeaderValue::from_static("inline")
        }
    })
}

fn parse_range_header(header: &str, total: u64) -> Option<(u64, u64)> {
    if !header.starts_with("bytes=") {
        return None;
    }
    let range = &header[6..];
    let (start_str, end_str) = range.split_once('-')?;
    if start_str.is_empty() {
        let suffix_len: u64 = end_str.parse().ok()?;
        if suffix_len == 0 {
            return None;
        }
        if suffix_len >= total {
            return Some((0, total.saturating_sub(1)));
        }
        Some((total - suffix_len, total - 1))
    } else {
        let start: u64 = start_str.parse().ok()?;
        if start >= total {
            return None;
        }
        let end = if end_str.is_empty() {
            total - 1
        } else {
            end_str.parse::<u64>().ok()?.min(total - 1)
        };
        (end >= start).then_some((start, end))
    }
}

async fn subsonic_id_for(
    state: &AppState,
    owner_id: ObjectId,
    kind: SubsonicIdKind,
    internal_key: String,
) -> std::result::Result<String, SubsonicError> {
    let coll = state.db.collection::<SubsonicId>("subsonic_ids");
    let kind_str = kind.as_str();
    if let Some(row) = coll
        .find_one(doc! { "owner_id": owner_id, "kind": kind_str, "internal_key": &internal_key })
        .await
        .map_err(to_subsonic_error)?
    {
        return Ok(row.numeric_id.to_string());
    }

    for _ in 0..8 {
        if let Some(row) = coll
            .find_one(
                doc! { "owner_id": owner_id, "kind": kind_str, "internal_key": &internal_key },
            )
            .await
            .map_err(to_subsonic_error)?
        {
            return Ok(row.numeric_id.to_string());
        }
        let max_row = coll
            .find_one(doc! { "owner_id": owner_id })
            .with_options(
                mongodb::options::FindOneOptions::builder()
                    .sort(doc! { "numeric_id": -1 })
                    .build(),
            )
            .await
            .map_err(to_subsonic_error)?;
        let next_id = max_row.map(|row| row.numeric_id + 1).unwrap_or(1);
        let row = SubsonicId::new(owner_id, next_id, kind, internal_key.clone());
        match coll.insert_one(row).await {
            Ok(_) => return Ok(next_id.to_string()),
            Err(err) if is_duplicate_key(&err) => continue,
            Err(err) => return Err(to_subsonic_error(err)),
        }
    }

    Err(SubsonicError::generic("Failed to allocate Subsonic ID"))
}

async fn alias_for_param(
    state: &AppState,
    owner_id: ObjectId,
    id: &str,
) -> std::result::Result<SubsonicId, SubsonicError> {
    let numeric_id = id
        .parse::<i64>()
        .map_err(|_| SubsonicError::not_found("ID"))?;
    state
        .db
        .collection::<SubsonicId>("subsonic_ids")
        .find_one(doc! { "owner_id": owner_id, "numeric_id": numeric_id })
        .await
        .map_err(to_subsonic_error)?
        .ok_or_else(|| SubsonicError::not_found("ID"))
}

fn is_duplicate_key(err: &mongodb::error::Error) -> bool {
    err.to_string().contains("E11000")
}

fn artist_key(name: &str) -> String {
    format!("artist:{}", URL_SAFE_NO_PAD.encode(name.as_bytes()))
}

fn album_key(artist: &str, album: &str) -> String {
    format!(
        "album:{}:{}",
        URL_SAFE_NO_PAD.encode(artist.as_bytes()),
        URL_SAFE_NO_PAD.encode(album.as_bytes())
    )
}

fn parse_artist_key(key: &str) -> Option<String> {
    let encoded = key.strip_prefix("artist:")?;
    String::from_utf8(URL_SAFE_NO_PAD.decode(encoded).ok()?).ok()
}

fn parse_album_key(key: &str) -> Option<(String, String)> {
    let rest = key.strip_prefix("album:")?;
    let (artist, album) = rest.split_once(':')?;
    Some((
        String::from_utf8(URL_SAFE_NO_PAD.decode(artist).ok()?).ok()?,
        String::from_utf8(URL_SAFE_NO_PAD.decode(album).ok()?).ok()?,
    ))
}

fn get_i64(doc: &bson::Document, key: &str) -> i64 {
    doc.get_i32(key)
        .map(|n| n as i64)
        .or_else(|_| doc.get_i64(key))
        .unwrap_or(0)
}

fn get_f64(doc: &bson::Document, key: &str) -> f64 {
    doc.get_f64(key)
        .or_else(|_| doc.get_i32(key).map(|n| n as f64))
        .or_else(|_| doc.get_i64(key).map(|n| n as f64))
        .unwrap_or(0.0)
}

fn to_subsonic_error(error: impl std::fmt::Display) -> SubsonicError {
    SubsonicError::generic(error.to_string())
}

fn ok_response(format: ResponseFormat, payload: Option<SubsonicPayload>) -> Response {
    match format {
        ResponseFormat::Json => {
            let mut root = serde_json::Map::new();
            root.insert("status".to_string(), json!("ok"));
            root.insert("version".to_string(), json!(API_VERSION));
            root.insert("type".to_string(), json!(SERVER_TYPE));
            root.insert(
                "serverVersion".to_string(),
                json!(env!("CARGO_PKG_VERSION")),
            );
            root.insert("openSubsonic".to_string(), json!(OPEN_SUBSONIC));
            if let Some(payload) = payload {
                let mut json_value = payload.json_value;
                remove_json_nulls(&mut json_value);
                root.insert(payload.json_key.to_string(), json_value);
            }
            Json(json!({ "subsonic-response": Value::Object(root) })).into_response()
        }
        ResponseFormat::Xml => {
            let mut xml = root_xml_open("ok");
            if let Some(payload) = payload {
                render_xml_element(&payload.xml, &mut xml);
            }
            xml.push_str("</subsonic-response>");
            xml_response(xml).into_response()
        }
    }
}

fn remove_json_nulls(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.retain(|_, value| !value.is_null());
            for value in map.values_mut() {
                remove_json_nulls(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                remove_json_nulls(value);
            }
        }
        _ => {}
    }
}

fn subsonic_error_response(format: ResponseFormat, err: SubsonicError) -> Response {
    match format {
        ResponseFormat::Json => Json(json!({
            "subsonic-response": {
                "status": "failed",
                "version": API_VERSION,
                "type": SERVER_TYPE,
                "serverVersion": env!("CARGO_PKG_VERSION"),
                "openSubsonic": OPEN_SUBSONIC,
                "error": {
                    "code": err.code,
                    "message": err.message,
                }
            }
        }))
        .into_response(),
        ResponseFormat::Xml => {
            let mut xml = root_xml_open("failed");
            render_xml_element(
                &XmlElement::new("error")
                    .attr("code", err.code)
                    .attr("message", err.message),
                &mut xml,
            );
            xml.push_str("</subsonic-response>");
            xml_response(xml).into_response()
        }
    }
}

fn root_xml_open(status: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><subsonic-response xmlns="http://subsonic.org/restapi" status="{status}" version="{API_VERSION}" type="{SERVER_TYPE}" serverVersion="{}" openSubsonic="{}">"#,
        env!("CARGO_PKG_VERSION"),
        OPEN_SUBSONIC
    )
}

fn xml_response(xml: String) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/xml; charset=utf-8")
        .body(Body::from(xml))
        .unwrap()
}

fn render_xml_element(element: &XmlElement, out: &mut String) {
    out.push('<');
    out.push_str(&element.name);
    for (key, value) in &element.attrs {
        out.push(' ');
        out.push_str(key);
        out.push_str("=\"");
        escape_xml(value, out);
        out.push('"');
    }
    if element.children.is_empty() && element.text.is_none() {
        out.push_str("/>");
        return;
    }
    out.push('>');
    if let Some(text) = &element.text {
        escape_xml(text, out);
    }
    for child in &element.children {
        render_xml_element(child, out);
    }
    out.push_str("</");
    out.push_str(&element.name);
    out.push('>');
}

fn escape_xml(value: &str, out: &mut String) {
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
}
