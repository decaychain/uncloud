//! S3-compatible REST API handlers.
//!
//! Mounted at `/s3`. All routes are authenticated via AWS SigV4.
//! One bucket per user, named after their username.
//! `s3://alice/photos/cat.jpg` maps to alice's file at `photos/cat.jpg`.

use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use mongodb::bson::{self, doc, oid::ObjectId};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::middleware::sigv4::{s3_error_response, S3User};
use crate::models::{File, FileVersion, Folder, UploadChunk, User};
use crate::routes::files::{resolve_storage_path, sanitize_path_component, version_path};
use crate::AppState;

// ---------------------------------------------------------------------------
// XML helpers
// ---------------------------------------------------------------------------

fn xml_response(status: StatusCode, body: String) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/xml".to_string())],
        body,
    )
        .into_response()
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Parse `<Delete><Object><Key>...</Key></Object>...</Delete>` XML using quick-xml.
fn parse_delete_keys(body: &[u8]) -> Vec<String> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_reader(body);
    reader.config_mut().trim_text(true);
    let mut keys = Vec::new();
    let mut inside_key = false;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"Key" => {
                inside_key = true;
            }
            Ok(Event::Text(ref e)) if inside_key => {
                if let Ok(text) = e.unescape() {
                    keys.push(text.to_string());
                }
                inside_key = false;
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == b"Key" => {
                inside_key = false;
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    keys
}

// ---------------------------------------------------------------------------
// Bucket / key extraction from path
// ---------------------------------------------------------------------------

/// Extract bucket and optional key from the URI path under /s3.
/// `/s3` -> (None, None)
/// `/s3/` -> (None, None)
/// `/s3/alice` -> (Some("alice"), None)
/// `/s3/alice/` -> (Some("alice"), None)
/// `/s3/alice/photos/cat.jpg` -> (Some("alice"), Some("photos/cat.jpg"))
fn extract_bucket_key(path: &str) -> (Option<String>, Option<String>) {
    let stripped = path.strip_prefix("/s3").unwrap_or(path);
    let stripped = stripped.strip_prefix('/').unwrap_or(stripped);

    if stripped.is_empty() {
        return (None, None);
    }

    match stripped.split_once('/') {
        None => (Some(stripped.to_string()), None),
        Some((bucket, "")) => (Some(bucket.to_string()), None),
        Some((bucket, key)) => (Some(bucket.to_string()), Some(key.to_string())),
    }
}

/// Validate that the bucket matches the authenticated user's username.
fn validate_bucket(user: &User, bucket: &str) -> Result<(), Response> {
    if user.username != bucket {
        return Err(s3_error_response(
            StatusCode::FORBIDDEN,
            "AccessDenied",
            "Access Denied",
        ));
    }
    Ok(())
}

/// Look up a file by its S3 key (= logical path within the user's storage).
async fn find_file_by_key(
    state: &AppState,
    user: &User,
    key: &str,
) -> Result<Option<File>, Response> {
    // The key maps to the file's storage_path minus the username prefix.
    // storage_path = "{username}/{key}"
    let storage_path = format!(
        "{}/{}",
        sanitize_path_component(&user.username),
        key
    );

    let files_coll = state.db.collection::<File>("files");
    files_coll
        .find_one(doc! {
            "owner_id": user.id,
            "storage_path": &storage_path,
            "deleted_at": bson::Bson::Null,
        })
        .await
        .map_err(|e| {
            s3_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalError",
                &e.to_string(),
            )
        })
}

/// Ensure all ancestor folders exist for a given key, returning the parent_id
/// for the file itself. Creates folders as needed.
async fn ensure_folders(
    state: &AppState,
    user: &User,
    key: &str,
) -> Result<Option<ObjectId>, Response> {
    let parts: Vec<&str> = key.split('/').collect();
    if parts.len() <= 1 {
        // No folders needed — file goes to root
        return Ok(None);
    }

    let folder_parts = &parts[..parts.len() - 1]; // everything except the filename
    let folders_coll = state.db.collection::<Folder>("folders");
    let mut current_parent: Option<ObjectId> = None;

    for folder_name in folder_parts {
        let parent_bson = current_parent
            .map(bson::Bson::ObjectId)
            .unwrap_or(bson::Bson::Null);

        let existing = folders_coll
            .find_one(doc! {
                "owner_id": user.id,
                "parent_id": &parent_bson,
                "name": *folder_name,
                "deleted_at": bson::Bson::Null,
            })
            .await
            .map_err(|e| {
                s3_error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "InternalError",
                    &e.to_string(),
                )
            })?;

        match existing {
            Some(f) => current_parent = Some(f.id),
            None => {
                let new_folder =
                    Folder::new(user.id, current_parent, folder_name.to_string());
                folders_coll.insert_one(&new_folder).await.map_err(|e| {
                    s3_error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "InternalError",
                        &e.to_string(),
                    )
                })?;
                current_parent = Some(new_folder.id);
            }
        }
    }

    Ok(current_parent)
}

/// Get the filename from an S3 key.
fn key_filename(key: &str) -> &str {
    key.rsplit_once('/').map(|(_, f)| f).unwrap_or(key)
}

// ---------------------------------------------------------------------------
// S3 Operations
// ---------------------------------------------------------------------------

/// Dispatcher: routes S3 requests based on method, presence of bucket/key, and query params.
pub async fn s3_handler(
    State(state): State<Arc<AppState>>,
    s3_user: S3User,
    request: Request,
) -> Response {
    let method = request.method().clone();
    let uri = request.uri().clone();
    let path = uri.path().to_string();
    let query = uri.query().unwrap_or("").to_string();
    let (bucket, key) = extract_bucket_key(&path);

    match (method.as_str(), bucket.as_deref(), key.as_deref()) {
        // ListBuckets: GET /s3 or GET /s3/
        ("GET", None, None) => list_buckets(&s3_user).await,

        // Bucket-level operations (no key)
        ("GET", Some(bucket), None) if query == "location" || query.starts_with("location&") || query.starts_with("location=") => {
            // GetBucketLocation — validate bucket then return empty LocationConstraint (= us-east-1)
            if bucket != s3_user.username {
                return s3_error_response(StatusCode::FORBIDDEN, "AccessDenied", "Access Denied");
            }
            (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, "application/xml")],
                r#"<?xml version="1.0" encoding="UTF-8"?><LocationConstraint xmlns="http://s3.amazonaws.com/doc/2006-03-01/"></LocationConstraint>"#,
            ).into_response()
        }
        ("GET", Some(bucket), None) => {
            list_objects_v2(state, &s3_user, bucket, &query).await
        }
        ("POST", Some(bucket), None) if query.contains("delete") => {
            delete_objects(state, &s3_user, bucket, request).await
        }

        // HeadBucket: validates bucket exists and is accessible
        ("HEAD", Some(bucket), None) => {
            if bucket != s3_user.username {
                return s3_error_response(StatusCode::FORBIDDEN, "AccessDenied", "Access Denied");
            }
            StatusCode::OK.into_response()
        }

        // Object-level operations
        ("HEAD", Some(bucket), Some(key)) => {
            head_object(state, &s3_user, bucket, key).await
        }
        ("GET", Some(bucket), Some(key)) => {
            get_object(state, &s3_user, bucket, key, request.headers().clone()).await
        }
        ("PUT", Some(bucket), Some(key)) if query.contains("partNumber=") && query.contains("uploadId=") => {
            upload_part(state, &s3_user, bucket, key, &query, request).await
        }
        ("PUT", Some(bucket), Some(key)) => {
            put_object(state, &s3_user, bucket, key, request).await
        }
        ("DELETE", Some(bucket), Some(_key)) if query.contains("uploadId=") => {
            abort_multipart_upload(state, &s3_user, bucket, &query).await
        }
        ("DELETE", Some(bucket), Some(key)) => {
            delete_object(state, &s3_user, bucket, key).await
        }
        ("POST", Some(bucket), Some(key)) if query.contains("uploads") => {
            create_multipart_upload(state, &s3_user, bucket, key).await
        }
        ("POST", Some(bucket), Some(key)) if query.contains("uploadId=") => {
            complete_multipart_upload(state, &s3_user, bucket, key, &query, request).await
        }

        _ => s3_error_response(
            StatusCode::BAD_REQUEST,
            "InvalidRequest",
            "Unsupported S3 operation",
        ),
    }
}

// ---------------------------------------------------------------------------
// ListBuckets
// ---------------------------------------------------------------------------

async fn list_buckets(user: &S3User) -> Response {
    let now = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ");
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<ListAllMyBucketsResult>
  <Owner>
    <ID>{}</ID>
    <DisplayName>{}</DisplayName>
  </Owner>
  <Buckets>
    <Bucket>
      <Name>{}</Name>
      <CreationDate>{}</CreationDate>
    </Bucket>
  </Buckets>
</ListAllMyBucketsResult>"#,
        xml_escape(&user.id.to_hex()),
        xml_escape(&user.username),
        xml_escape(&user.username),
        now,
    );
    xml_response(StatusCode::OK, body)
}

// ---------------------------------------------------------------------------
// ListObjectsV2
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ListQuery {
    prefix: Option<String>,
    delimiter: Option<String>,
    #[serde(rename = "max-keys")]
    max_keys: Option<i64>,
    #[serde(rename = "continuation-token")]
    continuation_token: Option<String>,
}

async fn list_objects_v2(
    state: Arc<AppState>,
    user: &S3User,
    bucket: &str,
    raw_query: &str,
) -> Response {
    if let Err(resp) = validate_bucket(user, bucket) {
        return resp;
    }

    // Parse query params manually (more flexible than axum Query for S3-style params)
    let params: std::collections::HashMap<String, String> = raw_query
        .split('&')
        .filter(|s| !s.is_empty())
        .filter_map(|pair| {
            let mut it = pair.splitn(2, '=');
            Some((
                urlencoding::decode(it.next()?).ok()?.to_string(),
                urlencoding::decode(it.next().unwrap_or("")).ok()?.to_string(),
            ))
        })
        .collect();

    let prefix = params.get("prefix").cloned().unwrap_or_default();
    let delimiter = params.get("delimiter").cloned();
    let max_keys = params
        .get("max-keys")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(1000)
        .min(1000);

    let username_prefix = format!("{}/", sanitize_path_component(&user.username));

    // Query all non-deleted files for this user
    let files_coll = state.db.collection::<File>("files");
    let mut filter = doc! {
        "owner_id": user.id,
        "deleted_at": bson::Bson::Null,
    };

    // If prefix is set, filter by storage_path prefix
    if !prefix.is_empty() {
        let full_prefix = format!("{}{}", username_prefix, prefix);
        filter.insert(
            "storage_path",
            doc! { "$regex": format!("^{}", regex_escape(&full_prefix)) },
        );
    }

    let mut cursor = files_coll.find(filter).await.map_err(|e| {
        s3_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "InternalError",
            &e.to_string(),
        )
    });

    let cursor = match cursor {
        Ok(c) => c,
        Err(resp) => return resp,
    };

    let mut all_files: Vec<File> = Vec::new();
    let mut cursor = cursor;
    while let Ok(true) = cursor.advance().await {
        if let Ok(f) = cursor.deserialize_current() {
            all_files.push(f);
        }
    }

    // Build the key for each file by stripping the username prefix
    let mut objects = Vec::new();
    let mut common_prefixes: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();

    for file in &all_files {
        let key = file
            .storage_path
            .strip_prefix(&username_prefix)
            .unwrap_or(&file.storage_path);

        if let Some(ref delim) = delimiter {
            // Check if there's a delimiter after the prefix
            let after_prefix = key.strip_prefix(&prefix).unwrap_or(key);
            if let Some(pos) = after_prefix.find(delim.as_str()) {
                // This is a "directory" — add as common prefix
                let cp = format!("{}{}{}", prefix, &after_prefix[..pos], delim);
                common_prefixes.insert(cp);
                continue;
            }
        }

        objects.push((key.to_string(), file));
    }

    // Truncate to max_keys
    let is_truncated = objects.len() as i64 > max_keys;
    objects.truncate(max_keys as usize);

    let mut xml = String::from(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    xml.push_str("\n<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">");
    xml.push_str(&format!("\n  <Name>{}</Name>", xml_escape(bucket)));
    xml.push_str(&format!("\n  <Prefix>{}</Prefix>", xml_escape(&prefix)));
    xml.push_str(&format!("\n  <MaxKeys>{}</MaxKeys>", max_keys));
    xml.push_str(&format!(
        "\n  <IsTruncated>{}</IsTruncated>",
        is_truncated
    ));
    if let Some(ref delim) = delimiter {
        xml.push_str(&format!("\n  <Delimiter>{}</Delimiter>", xml_escape(delim)));
    }
    xml.push_str(&format!("\n  <KeyCount>{}</KeyCount>", objects.len()));

    for (key, file) in &objects {
        let last_modified = file.updated_at.format("%Y-%m-%dT%H:%M:%S%.3fZ");
        xml.push_str("\n  <Contents>");
        xml.push_str(&format!("\n    <Key>{}</Key>", xml_escape(key)));
        xml.push_str(&format!("\n    <LastModified>{}</LastModified>", last_modified));
        xml.push_str(&format!(
            "\n    <ETag>\"{}\"</ETag>",
            xml_escape(&file.checksum_sha256)
        ));
        xml.push_str(&format!("\n    <Size>{}</Size>", file.size_bytes));
        xml.push_str("\n    <StorageClass>STANDARD</StorageClass>");
        xml.push_str("\n  </Contents>");
    }

    for cp in &common_prefixes {
        xml.push_str("\n  <CommonPrefixes>");
        xml.push_str(&format!("\n    <Prefix>{}</Prefix>", xml_escape(cp)));
        xml.push_str("\n  </CommonPrefixes>");
    }

    xml.push_str("\n</ListBucketResult>");

    xml_response(StatusCode::OK, xml)
}

fn regex_escape(s: &str) -> String {
    let special = [
        '.', '^', '$', '*', '+', '?', '(', ')', '[', ']', '{', '}', '|', '\\',
    ];
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        if special.contains(&c) {
            result.push('\\');
        }
        result.push(c);
    }
    result
}

// ---------------------------------------------------------------------------
// HeadObject
// ---------------------------------------------------------------------------

async fn head_object(
    state: Arc<AppState>,
    user: &S3User,
    bucket: &str,
    key: &str,
) -> Response {
    if let Err(resp) = validate_bucket(user, bucket) {
        return resp;
    }

    let file = match find_file_by_key(&state, user, key).await {
        Ok(Some(f)) => f,
        Ok(None) => {
            return s3_error_response(StatusCode::NOT_FOUND, "NoSuchKey", "The specified key does not exist")
        }
        Err(resp) => return resp,
    };

    let last_modified = file
        .updated_at
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string();

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, &file.mime_type)
        .header(header::CONTENT_LENGTH, file.size_bytes)
        .header(header::LAST_MODIFIED, &last_modified)
        .header("ETag", format!("\"{}\"", file.checksum_sha256))
        .body(Body::empty())
        .unwrap()
}

// ---------------------------------------------------------------------------
// GetObject
// ---------------------------------------------------------------------------

async fn get_object(
    state: Arc<AppState>,
    user: &S3User,
    bucket: &str,
    key: &str,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = validate_bucket(user, bucket) {
        return resp;
    }

    let file = match find_file_by_key(&state, user, key).await {
        Ok(Some(f)) => f,
        Ok(None) => {
            return s3_error_response(StatusCode::NOT_FOUND, "NoSuchKey", "The specified key does not exist")
        }
        Err(resp) => return resp,
    };

    let backend = match state.storage.get_backend(file.storage_id).await {
        Ok(b) => b,
        Err(e) => {
            return s3_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalError",
                &e.to_string(),
            )
        }
    };

    let total = file.size_bytes as u64;
    let last_modified = file
        .updated_at
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string();
    let etag = format!("\"{}\"", file.checksum_sha256);

    // Check for Range header
    if let Some(range_value) = headers.get(header::RANGE) {
        if let Some(range_str) = range_value.to_str().ok() {
            if let Some((start, end)) = parse_range(range_str, total) {
                let length = end - start + 1;
                let reader = match backend
                    .read_range(&file.storage_path, start, length)
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        return s3_error_response(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "InternalError",
                            &e.to_string(),
                        )
                    }
                };
                let stream = ReaderStream::new(reader);
                let body = Body::from_stream(stream);

                return Response::builder()
                    .status(StatusCode::PARTIAL_CONTENT)
                    .header(header::CONTENT_TYPE, &file.mime_type)
                    .header(header::CONTENT_LENGTH, length)
                    .header(
                        header::CONTENT_RANGE,
                        format!("bytes {}-{}/{}", start, end, total),
                    )
                    .header(header::ACCEPT_RANGES, "bytes")
                    .header(header::LAST_MODIFIED, &last_modified)
                    .header("ETag", &etag)
                    .body(body)
                    .unwrap();
            }
        }
    }

    // Full object
    let reader = match backend.read(&file.storage_path).await {
        Ok(r) => r,
        Err(e) => {
            return s3_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalError",
                &e.to_string(),
            )
        }
    };
    let stream = ReaderStream::new(reader);
    let body = Body::from_stream(stream);

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, &file.mime_type)
        .header(header::CONTENT_LENGTH, total)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::LAST_MODIFIED, &last_modified)
        .header("ETag", &etag)
        .body(body)
        .unwrap()
}

fn parse_range(range: &str, total: u64) -> Option<(u64, u64)> {
    let range = range.strip_prefix("bytes=")?;
    let range = range.split(',').next()?.trim();
    let (start_str, end_str) = range.split_once('-')?;

    if start_str.is_empty() {
        let suffix_len: u64 = end_str.parse().ok()?;
        if suffix_len == 0 || suffix_len > total {
            return None;
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
        if end < start {
            return None;
        }
        Some((start, end))
    }
}

// ---------------------------------------------------------------------------
// PutObject
// ---------------------------------------------------------------------------

async fn put_object(
    state: Arc<AppState>,
    user: &S3User,
    bucket: &str,
    key: &str,
    request: Request,
) -> Response {
    if let Err(resp) = validate_bucket(user, bucket) {
        return resp;
    }

    // Determine size limit: use configured max_file_size if non-zero, else 5 GB default
    let max_body_size = if state.config.uploads.max_file_size > 0 {
        state.config.uploads.max_file_size as usize
    } else {
        5 * 1024 * 1024 * 1024 // 5 GB
    };

    // Get or create storage
    let storage = match state.storage.get_default_storage().await {
        Ok(s) => s,
        Err(e) => {
            return s3_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalError",
                &e.to_string(),
            )
        }
    };

    let backend = match state.storage.get_backend(storage.id).await {
        Ok(b) => b,
        Err(e) => {
            return s3_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalError",
                &e.to_string(),
            )
        }
    };

    let filename = key_filename(key);
    let parent_id = match ensure_folders(&state, user, key).await {
        Ok(pid) => pid,
        Err(resp) => return resp,
    };

    let storage_path =
        match resolve_storage_path(&state.db, user.id, &user.username, parent_id, filename).await {
            Ok(p) => p,
            Err(e) => {
                return s3_error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "InternalError",
                    &e.to_string(),
                )
            }
        };

    // Stream body through a temp file to avoid buffering in memory
    let temp_path = match backend.create_temp().await {
        Ok(p) => p,
        Err(e) => {
            return s3_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalError",
                &e.to_string(),
            )
        }
    };

    let mut hasher = Sha256::new();
    let mut size: i64 = 0;
    let mut body = request.into_body();

    use http_body_util::BodyExt;
    loop {
        match body.frame().await {
            Some(Ok(frame)) => {
                if let Some(data) = frame.data_ref() {
                    size += data.len() as i64;
                    if size as usize > max_body_size {
                        let _ = backend.abort_temp(&temp_path).await;
                        return s3_error_response(
                            StatusCode::BAD_REQUEST,
                            "EntityTooLarge",
                            "Request body exceeds maximum allowed size",
                        );
                    }
                    hasher.update(data);
                    if let Err(e) = backend.append_temp(&temp_path, data).await {
                        let _ = backend.abort_temp(&temp_path).await;
                        return s3_error_response(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "InternalError",
                            &e.to_string(),
                        );
                    }
                }
            }
            Some(Err(e)) => {
                let _ = backend.abort_temp(&temp_path).await;
                return s3_error_response(
                    StatusCode::BAD_REQUEST,
                    "IncompleteBody",
                    &e.to_string(),
                );
            }
            None => break,
        }
    }

    // Check quota
    if !user.has_quota_space(size) {
        let _ = backend.abort_temp(&temp_path).await;
        return s3_error_response(
            StatusCode::FORBIDDEN,
            "AccessDenied",
            "Storage quota exceeded",
        );
    }

    let checksum = hex::encode(hasher.finalize());

    // Check if file already exists (overwrite)
    let files_coll = state.db.collection::<File>("files");
    let existing = files_coll
        .find_one(doc! {
            "owner_id": user.id,
            "storage_path": &storage_path,
            "deleted_at": bson::Bson::Null,
        })
        .await;

    match existing {
        Ok(Some(existing_file)) => {
            // Archive the current blob as a version before overwriting
            let versions_coll = state.db.collection::<FileVersion>("file_versions");
            let version_number = versions_coll
                .count_documents(doc! { "file_id": existing_file.id })
                .await
                .unwrap_or(0) as i32
                + 1;

            let ver_path = version_path(&existing_file.storage_path);
            if let Err(e) = backend.archive_version(&existing_file.storage_path, &ver_path).await {
                return s3_error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "InternalError",
                    &format!("Failed to archive version: {}", e),
                );
            }

            let file_version = FileVersion::new(
                existing_file.id,
                version_number,
                ver_path,
                existing_file.size_bytes,
                existing_file.checksum_sha256.clone(),
            );
            let _ = versions_coll.insert_one(&file_version).await;

            // Finalize temp file to the storage path (overwrites existing)
            if let Err(e) = backend.finalize_temp(&temp_path, &storage_path).await {
                return s3_error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "InternalError",
                    &e.to_string(),
                );
            }

            let size_delta = size - existing_file.size_bytes;
            let now = Utc::now();
            let mime_type = mime_guess::from_path(filename)
                .first_or_octet_stream()
                .to_string();

            let _ = files_coll
                .update_one(
                    doc! { "_id": existing_file.id },
                    doc! { "$set": {
                        "size_bytes": size,
                        "checksum_sha256": &checksum,
                        "mime_type": &mime_type,
                        "updated_at": bson::DateTime::from_chrono(now),
                        "processing_tasks": [],
                    },
                    "$inc": { "version": 1 }},
                )
                .await;

            if size_delta != 0 {
                let _ = state.auth.update_user_bytes(user.id, size_delta).await;
            }

            // Remove stale thumbnail and trigger reprocessing
            let _ = backend.delete(&format!(".thumbs/{}.jpg", existing_file.id.to_hex())).await;
            state.events.emit_file_created(user.id, &existing_file).await;

            let etag = format!("\"{}\"", checksum);
            Response::builder()
                .status(StatusCode::OK)
                .header("ETag", etag)
                .body(Body::empty())
                .unwrap()
        }
        _ => {
            // New file — finalize temp to final storage path
            if let Err(e) = backend.finalize_temp(&temp_path, &storage_path).await {
                return s3_error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "InternalError",
                    &e.to_string(),
                );
            }

            let mime_type = mime_guess::from_path(filename)
                .first_or_octet_stream()
                .to_string();

            let file = File::new(
                storage.id,
                storage_path,
                user.id,
                parent_id,
                filename.to_string(),
                mime_type,
                size,
                checksum.clone(),
            );

            if let Err(e) = files_coll.insert_one(&file).await {
                return s3_error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "InternalError",
                    &e.to_string(),
                );
            }

            let _ = state.auth.update_user_bytes(user.id, size).await;
            state.events.emit_file_created(user.id, &file).await;
            state.processing.enqueue(&file, state.clone()).await;

            let etag = format!("\"{}\"", checksum);
            Response::builder()
                .status(StatusCode::OK)
                .header("ETag", etag)
                .body(Body::empty())
                .unwrap()
        }
    }
}

// ---------------------------------------------------------------------------
// DeleteObject
// ---------------------------------------------------------------------------

async fn delete_object(
    state: Arc<AppState>,
    user: &S3User,
    bucket: &str,
    key: &str,
) -> Response {
    if let Err(resp) = validate_bucket(user, bucket) {
        return resp;
    }

    let file = match find_file_by_key(&state, user, key).await {
        Ok(Some(f)) => f,
        Ok(None) => {
            // S3 returns 204 even for non-existent keys
            return Response::builder()
                .status(StatusCode::NO_CONTENT)
                .body(Body::empty())
                .unwrap();
        }
        Err(resp) => return resp,
    };

    // Soft-delete (same as the normal API)
    let backend = match state.storage.get_backend(file.storage_id).await {
        Ok(b) => b,
        Err(e) => {
            return s3_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalError",
                &e.to_string(),
            )
        }
    };

    let tp = crate::routes::files::trash_path(&file.storage_path);
    let _ = backend.move_to_trash(&file.storage_path, &tp).await;
    let _ = backend
        .delete(&format!(".thumbs/{}.jpg", file.id.to_hex()))
        .await;

    let now = Utc::now();
    let batch_id = Uuid::new_v4().to_string();
    let files_coll = state.db.collection::<File>("files");
    let _ = files_coll
        .update_one(
            doc! { "_id": file.id, "owner_id": user.id },
            doc! { "$set": {
                "deleted_at": bson::DateTime::from_chrono(now),
                "trash_path": &tp,
                "batch_delete_id": &batch_id,
            }},
        )
        .await;

    let _ = state.auth.update_user_bytes(user.id, -file.size_bytes).await;
    state.events.emit_file_deleted(user.id, file.id).await;

    Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body(Body::empty())
        .unwrap()
}

// ---------------------------------------------------------------------------
// DeleteObjects (batch)
// ---------------------------------------------------------------------------

async fn delete_objects(
    state: Arc<AppState>,
    user: &S3User,
    bucket: &str,
    request: Request,
) -> Response {
    if let Err(resp) = validate_bucket(user, bucket) {
        return resp;
    }

    let body_bytes = match axum::body::to_bytes(request.into_body(), 10 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => {
            return s3_error_response(
                StatusCode::BAD_REQUEST,
                "MalformedXML",
                &e.to_string(),
            )
        }
    };

    // Parse XML to extract <Key> elements using quick-xml
    let keys = parse_delete_keys(&body_bytes);

    let mut deleted_xml = String::new();
    let mut error_xml = String::new();

    for key in &keys {
        let file = match find_file_by_key(&state, user, key).await {
            Ok(Some(f)) => f,
            Ok(None) => {
                // S3 considers non-existent keys as successfully deleted
                deleted_xml.push_str(&format!(
                    "\n  <Deleted>\n    <Key>{}</Key>\n  </Deleted>",
                    xml_escape(key)
                ));
                continue;
            }
            Err(_) => {
                error_xml.push_str(&format!(
                    "\n  <Error>\n    <Key>{}</Key>\n    <Code>InternalError</Code>\n    <Message>Internal error</Message>\n  </Error>",
                    xml_escape(key)
                ));
                continue;
            }
        };

        // Soft-delete
        let backend = match state.storage.get_backend(file.storage_id).await {
            Ok(b) => b,
            Err(_) => {
                error_xml.push_str(&format!(
                    "\n  <Error>\n    <Key>{}</Key>\n    <Code>InternalError</Code>\n    <Message>Storage error</Message>\n  </Error>",
                    xml_escape(key)
                ));
                continue;
            }
        };

        let tp = crate::routes::files::trash_path(&file.storage_path);
        let _ = backend.move_to_trash(&file.storage_path, &tp).await;

        let now = Utc::now();
        let batch_id = Uuid::new_v4().to_string();
        let files_coll = state.db.collection::<File>("files");
        let _ = files_coll
            .update_one(
                doc! { "_id": file.id, "owner_id": user.id },
                doc! { "$set": {
                    "deleted_at": bson::DateTime::from_chrono(now),
                    "trash_path": &tp,
                    "batch_delete_id": &batch_id,
                }},
            )
            .await;

        let _ = state.auth.update_user_bytes(user.id, -file.size_bytes).await;
        state.events.emit_file_deleted(user.id, file.id).await;

        deleted_xml.push_str(&format!(
            "\n  <Deleted>\n    <Key>{}</Key>\n  </Deleted>",
            xml_escape(key)
        ));
    }

    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<DeleteResult>{}{}</DeleteResult>"#,
        deleted_xml, error_xml
    );

    xml_response(StatusCode::OK, body)
}

// ---------------------------------------------------------------------------
// CreateMultipartUpload
// ---------------------------------------------------------------------------

async fn create_multipart_upload(
    state: Arc<AppState>,
    user: &S3User,
    bucket: &str,
    key: &str,
) -> Response {
    if let Err(resp) = validate_bucket(user, bucket) {
        return resp;
    }

    // Get or create storage
    let storage = match state.storage.get_default_storage().await {
        Ok(s) => s,
        Err(e) => {
            return s3_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalError",
                &e.to_string(),
            )
        }
    };

    let backend = match state.storage.get_backend(storage.id).await {
        Ok(b) => b,
        Err(e) => {
            return s3_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalError",
                &e.to_string(),
            )
        }
    };

    let temp_path = match backend.create_temp().await {
        Ok(p) => p,
        Err(e) => {
            return s3_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalError",
                &e.to_string(),
            )
        }
    };

    let upload_id = Uuid::new_v4().to_string();
    let filename = key_filename(key).to_string();
    let parent_id = match ensure_folders(&state, user, key).await {
        Ok(pid) => pid,
        Err(resp) => return resp,
    };

    // We store total_size=0 initially, chunk_size=0 — S3 multipart doesn't declare size upfront
    let upload = UploadChunk::new(
        upload_id.clone(),
        user.id,
        filename,
        parent_id,
        storage.id,
        0, // total_size unknown
        0, // chunk_size not fixed
        temp_path,
    );

    let collection = state.db.collection::<UploadChunk>("upload_chunks");
    if let Err(e) = collection.insert_one(&upload).await {
        return s3_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "InternalError",
            &e.to_string(),
        );
    }

    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<InitiateMultipartUploadResult>
  <Bucket>{}</Bucket>
  <Key>{}</Key>
  <UploadId>{}</UploadId>
</InitiateMultipartUploadResult>"#,
        xml_escape(bucket),
        xml_escape(key),
        xml_escape(&upload_id),
    );

    xml_response(StatusCode::OK, body)
}

// ---------------------------------------------------------------------------
// UploadPart
// ---------------------------------------------------------------------------

async fn upload_part(
    state: Arc<AppState>,
    user: &S3User,
    bucket: &str,
    _key: &str,
    raw_query: &str,
    request: Request,
) -> Response {
    if let Err(resp) = validate_bucket(user, bucket) {
        return resp;
    }

    let params: std::collections::HashMap<String, String> = raw_query
        .split('&')
        .filter(|s| !s.is_empty())
        .filter_map(|pair| {
            let mut it = pair.splitn(2, '=');
            Some((it.next()?.to_string(), it.next().unwrap_or("").to_string()))
        })
        .collect();

    let upload_id = match params.get("uploadId") {
        Some(id) => id.clone(),
        None => {
            return s3_error_response(
                StatusCode::BAD_REQUEST,
                "InvalidArgument",
                "Missing uploadId",
            )
        }
    };

    let part_number: i32 = match params.get("partNumber").and_then(|s| s.parse().ok()) {
        Some(n) => n,
        None => {
            return s3_error_response(
                StatusCode::BAD_REQUEST,
                "InvalidArgument",
                "Missing or invalid partNumber",
            )
        }
    };

    let collection = state.db.collection::<UploadChunk>("upload_chunks");
    let upload = match collection
        .find_one(doc! { "upload_id": &upload_id, "user_id": user.id })
        .await
    {
        Ok(Some(u)) => u,
        _ => {
            return s3_error_response(
                StatusCode::NOT_FOUND,
                "NoSuchUpload",
                "The specified upload does not exist",
            )
        }
    };

    let max_part_size = state.config.uploads.max_chunk_size as usize;
    let body_bytes = match axum::body::to_bytes(request.into_body(), max_part_size).await {
        Ok(b) => b,
        Err(e) => {
            return s3_error_response(
                StatusCode::BAD_REQUEST,
                "EntityTooLarge",
                &format!("Part exceeds maximum size of {} bytes: {}", max_part_size, e),
            )
        }
    };

    let backend = match state.storage.get_backend(upload.storage_id).await {
        Ok(b) => b,
        Err(e) => {
            return s3_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalError",
                &e.to_string(),
            )
        }
    };

    if let Err(e) = backend.append_temp(&upload.temp_path, &body_bytes).await {
        return s3_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "InternalError",
            &e.to_string(),
        );
    }

    // Track part number and accumulated size
    let part_size = body_bytes.len() as i64;
    let _ = collection
        .update_one(
            doc! { "upload_id": &upload_id },
            doc! {
                "$push": { "chunks_received": part_number },
                "$inc": { "total_size": part_size },
            },
        )
        .await;

    let etag = format!("\"{}\"", hex::encode(Sha256::digest(&body_bytes)));

    Response::builder()
        .status(StatusCode::OK)
        .header("ETag", etag)
        .body(Body::empty())
        .unwrap()
}

// ---------------------------------------------------------------------------
// CompleteMultipartUpload
// ---------------------------------------------------------------------------

async fn complete_multipart_upload(
    state: Arc<AppState>,
    user: &S3User,
    bucket: &str,
    key: &str,
    raw_query: &str,
    request: Request,
) -> Response {
    if let Err(resp) = validate_bucket(user, bucket) {
        return resp;
    }

    let params: std::collections::HashMap<String, String> = raw_query
        .split('&')
        .filter(|s| !s.is_empty())
        .filter_map(|pair| {
            let mut it = pair.splitn(2, '=');
            Some((it.next()?.to_string(), it.next().unwrap_or("").to_string()))
        })
        .collect();

    let upload_id = match params.get("uploadId") {
        Some(id) => id.clone(),
        None => {
            return s3_error_response(
                StatusCode::BAD_REQUEST,
                "InvalidArgument",
                "Missing uploadId",
            )
        }
    };

    // Consume the request body (CompleteMultipartUpload XML — we don't need it for our impl)
    let _ = axum::body::to_bytes(request.into_body(), 10 * 1024 * 1024).await;

    let collection = state.db.collection::<UploadChunk>("upload_chunks");
    let upload = match collection
        .find_one(doc! { "upload_id": &upload_id, "user_id": user.id })
        .await
    {
        Ok(Some(u)) => u,
        _ => {
            return s3_error_response(
                StatusCode::NOT_FOUND,
                "NoSuchUpload",
                "The specified upload does not exist",
            )
        }
    };

    if upload.chunks_received.is_empty() {
        return s3_error_response(
            StatusCode::BAD_REQUEST,
            "MalformedXML",
            "No parts uploaded",
        );
    }

    let backend = match state.storage.get_backend(upload.storage_id).await {
        Ok(b) => b,
        Err(e) => {
            return s3_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalError",
                &e.to_string(),
            )
        }
    };

    let filename = key_filename(key);
    let storage_path = match resolve_storage_path(
        &state.db,
        user.id,
        &user.username,
        upload.parent_id,
        filename,
    )
    .await
    {
        Ok(p) => p,
        Err(e) => {
            return s3_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalError",
                &e.to_string(),
            )
        }
    };

    // Finalize the temp file
    if let Err(e) = backend
        .finalize_temp(&upload.temp_path, &storage_path)
        .await
    {
        return s3_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "InternalError",
            &e.to_string(),
        );
    }

    // Calculate checksum
    let reader = match backend.read(&storage_path).await {
        Ok(r) => r,
        Err(e) => {
            return s3_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalError",
                &e.to_string(),
            )
        }
    };
    let mut hasher = Sha256::new();
    let mut reader = tokio::io::BufReader::new(reader);
    let mut buf = [0u8; 8192];
    let mut total_size: i64 = 0;
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                hasher.update(&buf[..n]);
                total_size += n as i64;
            }
            Err(e) => {
                return s3_error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "InternalError",
                    &e.to_string(),
                )
            }
        }
    }
    let checksum = hex::encode(hasher.finalize());

    // Check quota
    if !user.has_quota_space(total_size) {
        let _ = backend.delete(&storage_path).await;
        return s3_error_response(
            StatusCode::FORBIDDEN,
            "AccessDenied",
            "Storage quota exceeded",
        );
    }

    let mime_type = mime_guess::from_path(filename)
        .first_or_octet_stream()
        .to_string();

    let storage = match state.storage.get_default_storage().await {
        Ok(s) => s,
        Err(e) => {
            return s3_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalError",
                &e.to_string(),
            )
        }
    };

    let files_coll = state.db.collection::<File>("files");

    // Check if a file already exists at this path (overwrite case)
    let existing = files_coll
        .find_one(doc! {
            "owner_id": user.id,
            "parent_id": upload.parent_id.map(bson::Bson::ObjectId).unwrap_or(bson::Bson::Null),
            "name": filename,
            "deleted_at": bson::Bson::Null,
        })
        .await;

    match existing {
        Ok(Some(existing_file)) => {
            // Archive the old version before overwriting
            let versions_coll = state.db.collection::<FileVersion>("file_versions");
            let version_number = versions_coll
                .count_documents(doc! { "file_id": existing_file.id })
                .await
                .unwrap_or(0) as i32
                + 1;

            let ver_path = version_path(&existing_file.storage_path);
            let _ = backend
                .archive_version(&existing_file.storage_path, &ver_path)
                .await;

            let file_version = FileVersion::new(
                existing_file.id,
                version_number,
                ver_path,
                existing_file.size_bytes,
                existing_file.checksum_sha256.clone(),
            );
            let _ = versions_coll.insert_one(&file_version).await;

            let size_delta = total_size - existing_file.size_bytes;
            let now = Utc::now();

            let _ = files_coll
                .update_one(
                    doc! { "_id": existing_file.id },
                    doc! { "$set": {
                        "size_bytes": total_size,
                        "checksum_sha256": &checksum,
                        "mime_type": &mime_type,
                        "storage_path": &storage_path,
                        "updated_at": bson::DateTime::from_chrono(now),
                        "processing_tasks": [],
                    },
                    "$inc": { "version": 1 }},
                )
                .await;

            if size_delta != 0 {
                let _ = state.auth.update_user_bytes(user.id, size_delta).await;
            }

            let _ = backend
                .delete(&format!(".thumbs/{}.jpg", existing_file.id.to_hex()))
                .await;
            state.events.emit_file_created(user.id, &existing_file).await;
        }
        _ => {
            // New file
            let file = File::new(
                storage.id,
                storage_path,
                user.id,
                upload.parent_id,
                filename.to_string(),
                mime_type,
                total_size,
                checksum.clone(),
            );

            if let Err(e) = files_coll.insert_one(&file).await {
                return s3_error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "InternalError",
                    &e.to_string(),
                );
            }

            let _ = state.auth.update_user_bytes(user.id, total_size).await;
            state.events.emit_file_created(user.id, &file).await;
            state.processing.enqueue(&file, state.clone()).await;
        }
    }

    // Clean up upload record
    let _ = collection
        .delete_one(doc! { "upload_id": &upload_id })
        .await;

    let etag = format!("\"{}\"", checksum);
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<CompleteMultipartUploadResult>
  <Bucket>{}</Bucket>
  <Key>{}</Key>
  <ETag>{}</ETag>
</CompleteMultipartUploadResult>"#,
        xml_escape(bucket),
        xml_escape(key),
        xml_escape(&etag),
    );

    xml_response(StatusCode::OK, body)
}

// ---------------------------------------------------------------------------
// AbortMultipartUpload
// ---------------------------------------------------------------------------

async fn abort_multipart_upload(
    state: Arc<AppState>,
    user: &S3User,
    _bucket: &str,
    raw_query: &str,
) -> Response {
    let params: std::collections::HashMap<String, String> = raw_query
        .split('&')
        .filter(|s| !s.is_empty())
        .filter_map(|pair| {
            let mut it = pair.splitn(2, '=');
            Some((it.next()?.to_string(), it.next().unwrap_or("").to_string()))
        })
        .collect();

    let upload_id = match params.get("uploadId") {
        Some(id) => id.clone(),
        None => {
            return s3_error_response(
                StatusCode::BAD_REQUEST,
                "InvalidArgument",
                "Missing uploadId",
            )
        }
    };

    let collection = state.db.collection::<UploadChunk>("upload_chunks");
    let upload = match collection
        .find_one(doc! { "upload_id": &upload_id, "user_id": user.id })
        .await
    {
        Ok(Some(u)) => u,
        _ => {
            return s3_error_response(
                StatusCode::NOT_FOUND,
                "NoSuchUpload",
                "The specified upload does not exist",
            )
        }
    };

    let backend = match state.storage.get_backend(upload.storage_id).await {
        Ok(b) => b,
        Err(e) => {
            return s3_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalError",
                &e.to_string(),
            )
        }
    };

    let _ = backend.abort_temp(&upload.temp_path).await;
    let _ = collection
        .delete_one(doc! { "upload_id": &upload_id })
        .await;

    Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body(Body::empty())
        .unwrap()
}
