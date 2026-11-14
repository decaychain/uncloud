// `POST /api/uploads/simple` and `POST /api/files/{id}/content` stream the
// request body into a storage temp file instead of buffering it. These tests
// pin down that the bytes, size and checksum come out exactly as sent, and
// that an upload which fails at any point leaves nothing in the backend's
// temp directory.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::http::StatusCode;
use axum_test::multipart::{MultipartForm, Part};
use common::{BoundTestApp, TestApp};
use mongodb::bson::{doc, oid::ObjectId, Document};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

const MIB: usize = 1024 * 1024;
const BOUNDARY: &str = "uncloud-streaming-test-boundary";

/// Deterministic filler with no short period, so a dropped, duplicated or
/// reordered buffer changes the hash.
fn pattern(len: usize, seed: u32) -> Vec<u8> {
    (0..len)
        .map(|i| {
            (i as u32)
                .wrapping_add(seed)
                .wrapping_mul(2_654_435_761)
                .to_le_bytes()[1]
        })
        .collect()
}

fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

fn temp_files(storage: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(storage.join(".tmp"))
        .expect("read .tmp")
        .map(|e| e.expect("dir entry").path())
        .collect()
}

/// Cleanup after a rejected or cancelled upload runs on a spawned task, so it
/// can land just after the response does.
async fn assert_no_temp_files(storage: &Path) {
    for _ in 0..500 {
        if temp_files(storage).is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("temp files left behind: {:?}", temp_files(storage));
}

async fn wait_for_temp_bytes(storage: &Path, at_least: u64) {
    for _ in 0..500 {
        let staged: u64 = temp_files(storage)
            .iter()
            .filter_map(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .sum();
        if staged >= at_least {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("upload never reached {at_least} staged bytes");
}

async fn file_doc(db: &mongodb::Database, id: &str) -> Document {
    db.collection::<Document>("files")
        .find_one(doc! { "_id": ObjectId::parse_str(id).expect("file id") })
        .await
        .expect("query file")
        .expect("file document")
}

async fn used_bytes(db: &mongodb::Database, username: &str) -> i64 {
    db.collection::<Document>("users")
        .find_one(doc! { "username": username })
        .await
        .expect("query user")
        .expect("user document")
        .get_i64("used_bytes")
        .expect("used_bytes")
}

async fn audit_operations(db: &mongodb::Database, file_id: &str) -> Vec<String> {
    let mut cursor = db
        .collection::<Document>("sync_events")
        .find(doc! { "resource_id": ObjectId::parse_str(file_id).expect("file id") })
        .sort(doc! { "timestamp": 1 })
        .await
        .expect("query sync_events");
    let mut ops = Vec::new();
    while cursor.advance().await.expect("advance") {
        let event: Document = cursor.deserialize_current().expect("event");
        ops.push(event.get_str("operation").expect("operation").to_string());
    }
    ops
}

fn file_form(filename: &str, content: &[u8]) -> MultipartForm {
    MultipartForm::new().add_part(
        "file",
        Part::bytes(content.to_vec())
            .file_name(filename)
            .mime_type("application/octet-stream"),
    )
}

// ── In-process: integrity ────────────────────────────────────────────────────

#[tokio::test]
async fn simple_upload_stores_multi_megabyte_body_intact() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let data = pattern(9 * MIB + 4321, 1);
    let res = app
        .server
        .post("/api/uploads/simple")
        .multipart(file_form("big.bin", &data))
        .await;
    res.assert_status_ok();
    let file: Value = res.json();
    let id = file["id"].as_str().expect("file id");
    assert_eq!(file["size_bytes"], data.len() as i64);

    let stored = file_doc(&app.db, id).await;
    assert_eq!(stored.get_i64("size_bytes").unwrap(), data.len() as i64);
    assert_eq!(
        stored.get_str("checksum_sha256").unwrap(),
        sha256_hex(&data)
    );
    assert_eq!(stored.get_str("storage_path").unwrap(), "alice/big.bin");

    let on_disk = std::fs::read(app.storage_path().join("alice/big.bin")).expect("blob");
    assert_eq!(sha256_hex(&on_disk), sha256_hex(&data));

    let download = app.server.get(&format!("/api/files/{id}/download")).await;
    download.assert_status_ok();
    assert_eq!(download.as_bytes().as_ref(), data.as_slice());

    assert_eq!(used_bytes(&app.db, "alice").await, data.len() as i64);
    assert_eq!(audit_operations(&app.db, id).await, ["created"]);
    assert_no_temp_files(app.storage_path()).await;

    app.cleanup().await;
}

#[tokio::test]
async fn simple_upload_resolves_folder_named_after_the_file_part() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let folder: Value = app
        .server
        .post("/api/folders")
        .json(&serde_json::json!({ "name": "docs" }))
        .await
        .json();
    let folder_id = folder["id"].as_str().expect("folder id");

    let data = pattern(5 * MIB + 17, 2);
    let file = app
        .upload_to_folder("report.bin", &data, "application/octet-stream", folder_id)
        .await;
    let id = file["id"].as_str().expect("file id");
    assert_eq!(file["parent_id"], folder_id);

    let stored = file_doc(&app.db, id).await;
    assert_eq!(
        stored.get_str("storage_path").unwrap(),
        "alice/docs/report.bin"
    );
    assert_eq!(
        stored.get_str("checksum_sha256").unwrap(),
        sha256_hex(&data)
    );
    let on_disk = std::fs::read(app.storage_path().join("alice/docs/report.bin")).expect("blob");
    assert_eq!(on_disk, data);
    assert_no_temp_files(app.storage_path()).await;

    app.cleanup().await;
}

#[tokio::test]
async fn simple_upload_accepts_empty_file() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let res = app
        .server
        .post("/api/uploads/simple")
        .multipart(file_form("empty.txt", b""))
        .await;
    res.assert_status_ok();
    let file: Value = res.json();
    assert_eq!(file["size_bytes"], 0);

    let stored = file_doc(&app.db, file["id"].as_str().unwrap()).await;
    assert_eq!(stored.get_str("checksum_sha256").unwrap(), sha256_hex(b""));
    let on_disk = std::fs::read(app.storage_path().join("alice/empty.txt")).expect("blob");
    assert!(on_disk.is_empty());
    assert_no_temp_files(app.storage_path()).await;

    app.cleanup().await;
}

#[tokio::test]
async fn content_update_stores_multi_megabyte_body_and_archives_previous() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let original = pattern(3 * MIB + 5, 3);
    let file = app
        .upload("doc.bin", &original, "application/octet-stream")
        .await;
    let id = file["id"].as_str().expect("file id").to_string();

    let replacement = pattern(9 * MIB + 777, 4);
    let res = app
        .server
        .post(&format!("/api/files/{id}/content"))
        .multipart(file_form("doc.bin", &replacement))
        .await;
    res.assert_status_ok();
    let updated: Value = res.json();
    assert_eq!(updated["id"], id.as_str());
    assert_eq!(updated["size_bytes"], replacement.len() as i64);

    let stored = file_doc(&app.db, &id).await;
    assert_eq!(
        stored.get_i64("size_bytes").unwrap(),
        replacement.len() as i64
    );
    assert_eq!(
        stored.get_str("checksum_sha256").unwrap(),
        sha256_hex(&replacement)
    );
    assert_eq!(stored.get_str("storage_path").unwrap(), "alice/doc.bin");

    let download = app.server.get(&format!("/api/files/{id}/download")).await;
    download.assert_status_ok();
    assert_eq!(download.as_bytes().as_ref(), replacement.as_slice());

    let versions: Value = app
        .server
        .get(&format!("/api/files/{id}/versions"))
        .await
        .json();
    let versions = versions.as_array().expect("versions array");
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0]["version"], 1);
    assert_eq!(versions[0]["size_bytes"], original.len() as i64);
    assert_eq!(versions[0]["checksum_sha256"], sha256_hex(&original));

    let archived = app
        .server
        .get(&format!(
            "/api/files/{id}/versions/{}",
            versions[0]["id"].as_str().unwrap()
        ))
        .await;
    archived.assert_status_ok();
    assert_eq!(archived.as_bytes().as_ref(), original.as_slice());

    assert_eq!(used_bytes(&app.db, "alice").await, replacement.len() as i64);
    assert_eq!(
        audit_operations(&app.db, &id).await,
        ["created", "content_replaced"]
    );
    assert_no_temp_files(app.storage_path()).await;

    app.cleanup().await;
}

// ── In-process: rejected uploads leave nothing behind ────────────────────────

#[tokio::test]
async fn quota_exceeded_upload_leaves_no_temp_and_no_file() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;
    app.db
        .collection::<Document>("users")
        .update_one(
            doc! { "username": "alice" },
            doc! { "$set": { "quota_bytes": MIB as i64 } },
        )
        .await
        .expect("set quota");

    let res = app
        .server
        .post("/api/uploads/simple")
        .multipart(file_form("too-big.bin", &pattern(5 * MIB, 5)))
        .await;
    res.assert_status(StatusCode::FORBIDDEN);

    assert_no_temp_files(app.storage_path()).await;
    assert!(!app.storage_path().join("alice/too-big.bin").exists());
    let listing: Value = app.server.get("/api/files").await.json();
    assert!(listing.as_array().expect("files array").is_empty());
    assert_eq!(used_bytes(&app.db, "alice").await, 0);

    app.cleanup().await;
}

#[tokio::test]
async fn duplicate_name_upload_leaves_no_temp_and_keeps_existing_content() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let first = pattern(2 * MIB, 6);
    app.upload("doc.bin", &first, "application/octet-stream")
        .await;

    let res = app
        .server
        .post("/api/uploads/simple")
        .multipart(file_form("doc.bin", &pattern(5 * MIB, 7)))
        .await;
    res.assert_status(StatusCode::CONFLICT);

    assert_no_temp_files(app.storage_path()).await;
    let on_disk = std::fs::read(app.storage_path().join("alice/doc.bin")).expect("blob");
    assert_eq!(on_disk, first);
    assert_eq!(used_bytes(&app.db, "alice").await, first.len() as i64);

    app.cleanup().await;
}

#[tokio::test]
async fn upload_to_missing_folder_leaves_no_temp() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let form = file_form("lost.bin", &pattern(5 * MIB, 8))
        .add_part("parent_id", Part::text(ObjectId::new().to_hex()));
    let res = app.server.post("/api/uploads/simple").multipart(form).await;
    res.assert_status(StatusCode::NOT_FOUND);

    assert_no_temp_files(app.storage_path()).await;
    assert_eq!(used_bytes(&app.db, "alice").await, 0);

    app.cleanup().await;
}

#[tokio::test]
async fn upload_without_file_part_is_rejected() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let form = MultipartForm::new().add_part("parent_id", Part::text(String::new()));
    let res = app.server.post("/api/uploads/simple").multipart(form).await;
    res.assert_status(StatusCode::BAD_REQUEST);
    assert_no_temp_files(app.storage_path()).await;

    app.cleanup().await;
}

#[tokio::test]
async fn empty_content_update_is_rejected_and_keeps_existing_content() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let original = pattern(MIB, 9);
    let file = app
        .upload("doc.bin", &original, "application/octet-stream")
        .await;
    let id = file["id"].as_str().expect("file id");

    let res = app
        .server
        .post(&format!("/api/files/{id}/content"))
        .multipart(file_form("doc.bin", b""))
        .await;
    res.assert_status(StatusCode::BAD_REQUEST);

    assert_no_temp_files(app.storage_path()).await;
    let on_disk = std::fs::read(app.storage_path().join("alice/doc.bin")).expect("blob");
    assert_eq!(on_disk, original);
    let versions: Value = app
        .server
        .get(&format!("/api/files/{id}/versions"))
        .await
        .json();
    assert!(versions.as_array().expect("versions array").is_empty());
    assert_eq!(audit_operations(&app.db, id).await, ["created"]);

    app.cleanup().await;
}

// ── In-process: folder pinned to a non-default storage ───────────────────────

#[tokio::test]
async fn upload_into_folder_pinned_to_another_storage_lands_there() {
    use uncloud_server::config::{ConfiguredStorage, ConfiguredStorageBackend};

    let main_dir = tempfile::tempdir().expect("main dir");
    let pinned_dir = tempfile::tempdir().expect("pinned dir");
    let (main_path, pinned_path) = (
        main_dir.path().to_path_buf(),
        pinned_dir.path().to_path_buf(),
    );

    let app = TestApp::with_config(|config| {
        config.storage.default_path = None;
        config.storage.default = Some("main".to_string());
        config.storage.storages = vec![
            ConfiguredStorage {
                name: "main".to_string(),
                backend: ConfiguredStorageBackend::Local {
                    path: main_path.clone(),
                },
            },
            ConfiguredStorage {
                name: "pinned".to_string(),
                backend: ConfiguredStorageBackend::Local {
                    path: pinned_path.clone(),
                },
            },
        ];
    })
    .await;
    app.register_and_login("alice").await;

    let pinned_storage_id = app
        .db
        .collection::<Document>("storages")
        .find_one(doc! { "name": "pinned" })
        .await
        .expect("query storages")
        .expect("pinned storage")
        .get_object_id("_id")
        .expect("storage id");

    let folder: Value = app
        .server
        .post("/api/folders")
        .json(&serde_json::json!({ "name": "archive" }))
        .await
        .json();
    let folder_id = folder["id"].as_str().expect("folder id");
    app.db
        .collection::<Document>("folders")
        .update_one(
            doc! { "_id": ObjectId::parse_str(folder_id).unwrap() },
            doc! { "$set": { "storage_id": pinned_storage_id } },
        )
        .await
        .expect("pin folder");

    // `file` precedes `parent_id`, so the body is staged on the default
    // storage before the pin is known.
    let data = pattern(6 * MIB + 3, 10);
    let file = app
        .upload_to_folder("cold.bin", &data, "application/octet-stream", folder_id)
        .await;
    let id = file["id"].as_str().expect("file id");

    let stored = file_doc(&app.db, id).await;
    assert_eq!(
        stored.get_object_id("storage_id").unwrap(),
        pinned_storage_id
    );
    assert_eq!(
        stored.get_str("checksum_sha256").unwrap(),
        sha256_hex(&data)
    );

    let on_disk = std::fs::read(pinned_path.join("alice/archive/cold.bin")).expect("pinned blob");
    assert_eq!(on_disk, data);
    assert!(!main_path.join("alice/archive/cold.bin").exists());

    let download = app.server.get(&format!("/api/files/{id}/download")).await;
    download.assert_status_ok();
    assert_eq!(download.as_bytes().as_ref(), data.as_slice());

    // With `parent_id` first the pin is known before the body starts, so the
    // upload is staged on the pinned storage and never touches the default.
    let direct = pattern(5 * MIB + 1, 19);
    let form = MultipartForm::new()
        .add_part("parent_id", Part::text(folder_id.to_string()))
        .add_part(
            "file",
            Part::bytes(direct.clone())
                .file_name("warm.bin")
                .mime_type("application/octet-stream"),
        );
    let res = app.server.post("/api/uploads/simple").multipart(form).await;
    res.assert_status_ok();
    let warm: Value = res.json();
    let warm_doc = file_doc(&app.db, warm["id"].as_str().expect("file id")).await;
    assert_eq!(
        warm_doc.get_object_id("storage_id").unwrap(),
        pinned_storage_id
    );
    let on_disk = std::fs::read(pinned_path.join("alice/archive/warm.bin")).expect("pinned blob");
    assert_eq!(on_disk, direct);
    assert!(!main_path.join("alice/archive/warm.bin").exists());

    assert_no_temp_files(&main_path).await;
    assert_no_temp_files(&pinned_path).await;

    app.cleanup().await;
}

// ── Real HTTP: chunked transfer encoding, no Content-Length ──────────────────

fn file_part_head(filename: &str) -> Vec<u8> {
    format!(
        "--{BOUNDARY}\r\n\
         Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n\
         Content-Type: application/octet-stream\r\n\r\n"
    )
    .into_bytes()
}

fn text_part(name: &str, value: &str) -> Vec<u8> {
    format!(
        "\r\n--{BOUNDARY}\r\n\
         Content-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}"
    )
    .into_bytes()
}

fn closing_boundary() -> Vec<u8> {
    format!("\r\n--{BOUNDARY}--\r\n").into_bytes()
}

type BodySender = mpsc::Sender<std::io::Result<Vec<u8>>>;

struct HttpUser {
    http: reqwest::Client,
    base_url: String,
    cookie: String,
}

impl HttpUser {
    async fn register(app: &BoundTestApp, username: &str) -> Self {
        let http = reqwest::Client::new();
        http.post(format!("{}/api/auth/register", app.base_url))
            .json(&serde_json::json!({
                "username": username,
                "email": format!("{username}@example.com"),
                "password": "password123!"
            }))
            .send()
            .await
            .expect("register");
        let login = http
            .post(format!("{}/api/auth/login", app.base_url))
            .json(&serde_json::json!({ "username": username, "password": "password123!" }))
            .send()
            .await
            .expect("login");
        assert!(login.status().is_success(), "login failed");
        let cookie = login
            .headers()
            .get(reqwest::header::SET_COOKIE)
            .expect("session cookie")
            .to_str()
            .expect("cookie text")
            .split(';')
            .next()
            .expect("cookie pair")
            .to_string();
        Self {
            http,
            base_url: app.base_url.clone(),
            cookie,
        }
    }

    /// Start a multipart POST whose body is fed through the returned sender.
    /// reqwest cannot know the length of a stream, so this goes out with
    /// chunked transfer encoding. Sending an `Err` makes the client abandon
    /// the request mid-body, the way a crashed or disconnected client would.
    fn start_multipart(
        &self,
        path: &str,
    ) -> (
        BodySender,
        tokio::task::JoinHandle<reqwest::Result<reqwest::Response>>,
    ) {
        let (tx, rx) = mpsc::channel(4);
        let request = self
            .http
            .post(format!("{}{}", self.base_url, path))
            .header(reqwest::header::COOKIE, &self.cookie)
            .header(
                reqwest::header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={BOUNDARY}"),
            )
            .body(reqwest::Body::wrap_stream(ReceiverStream::new(rx)));
        (tx, tokio::spawn(request.send()))
    }

    /// Stream a complete multipart upload in small frames and return the
    /// response status and JSON body.
    async fn upload_streamed(
        &self,
        path: &str,
        filename: &str,
        data: &[u8],
        parent_id: Option<&str>,
    ) -> (reqwest::StatusCode, Value) {
        let (tx, response) = self.start_multipart(path);
        tx.send(Ok(file_part_head(filename))).await.expect("head");
        for frame in data.chunks(48 * 1024 + 7) {
            tx.send(Ok(frame.to_vec())).await.expect("frame");
        }
        if let Some(parent_id) = parent_id {
            tx.send(Ok(text_part("parent_id", parent_id)))
                .await
                .expect("parent_id");
        }
        tx.send(Ok(closing_boundary())).await.expect("closing");
        drop(tx);

        let response = response.await.expect("join").expect("response");
        let status = response.status();
        (status, response.json().await.expect("json body"))
    }

    async fn get_json(&self, path: &str) -> Value {
        self.http
            .get(format!("{}{}", self.base_url, path))
            .header(reqwest::header::COOKIE, &self.cookie)
            .send()
            .await
            .expect("get")
            .json()
            .await
            .expect("json body")
    }

    async fn download(&self, file_id: &str) -> Vec<u8> {
        self.http
            .get(format!("{}/api/files/{file_id}/download", self.base_url))
            .header(reqwest::header::COOKIE, &self.cookie)
            .send()
            .await
            .expect("download")
            .bytes()
            .await
            .expect("download body")
            .to_vec()
    }
}

#[tokio::test]
async fn chunked_simple_upload_without_content_length_arrives_intact() {
    let app = BoundTestApp::new().await;
    let alice = HttpUser::register(&app, "alice").await;

    let folder: Value = alice
        .http
        .post(format!("{}/api/folders", app.base_url))
        .header(reqwest::header::COOKIE, &alice.cookie)
        .json(&serde_json::json!({ "name": "inbox" }))
        .send()
        .await
        .expect("create folder")
        .json()
        .await
        .expect("folder json");
    let folder_id = folder["id"].as_str().expect("folder id");

    let data = pattern(11 * MIB + 1234, 11);
    let (status, file) = alice
        .upload_streamed("/api/uploads/simple", "stream.bin", &data, Some(folder_id))
        .await;
    assert_eq!(status, reqwest::StatusCode::OK, "{file}");
    let id = file["id"].as_str().expect("file id");
    assert_eq!(file["size_bytes"], data.len() as i64);
    assert_eq!(file["parent_id"], folder_id);

    let stored = file_doc(&app.db, id).await;
    assert_eq!(stored.get_i64("size_bytes").unwrap(), data.len() as i64);
    assert_eq!(
        stored.get_str("checksum_sha256").unwrap(),
        sha256_hex(&data)
    );
    let on_disk = std::fs::read(app.storage_path().join("alice/inbox/stream.bin")).expect("blob");
    assert_eq!(sha256_hex(&on_disk), sha256_hex(&data));
    assert_eq!(alice.download(id).await, data);
    assert_no_temp_files(app.storage_path()).await;

    app.db.drop().await.ok();
}

#[tokio::test]
async fn chunked_content_update_without_content_length_arrives_intact() {
    let app = BoundTestApp::new().await;
    let alice = HttpUser::register(&app, "alice").await;

    let original = pattern(2 * MIB + 1, 12);
    let (status, file) = alice
        .upload_streamed("/api/uploads/simple", "doc.bin", &original, None)
        .await;
    assert_eq!(status, reqwest::StatusCode::OK, "{file}");
    let id = file["id"].as_str().expect("file id").to_string();

    let replacement = pattern(10 * MIB + 99, 13);
    let (status, updated) = alice
        .upload_streamed(
            &format!("/api/files/{id}/content"),
            "doc.bin",
            &replacement,
            None,
        )
        .await;
    assert_eq!(status, reqwest::StatusCode::OK, "{updated}");
    assert_eq!(updated["size_bytes"], replacement.len() as i64);

    let stored = file_doc(&app.db, &id).await;
    assert_eq!(
        stored.get_i64("size_bytes").unwrap(),
        replacement.len() as i64
    );
    assert_eq!(
        stored.get_str("checksum_sha256").unwrap(),
        sha256_hex(&replacement)
    );
    assert_eq!(alice.download(&id).await, replacement);

    let versions = alice.get_json(&format!("/api/files/{id}/versions")).await;
    let versions = versions.as_array().expect("versions array");
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0]["checksum_sha256"], sha256_hex(&original));
    assert_no_temp_files(app.storage_path()).await;

    app.db.drop().await.ok();
}

// ── Real HTTP: client goes away mid-body ─────────────────────────────────────

#[tokio::test]
async fn disconnect_during_simple_upload_leaves_no_temp_and_no_file() {
    let app = BoundTestApp::new().await;
    let alice = HttpUser::register(&app, "alice").await;

    let (tx, response) = alice.start_multipart("/api/uploads/simple");
    tx.send(Ok(file_part_head("partial.bin"))).await.unwrap();
    for frame in pattern(6 * MIB, 14).chunks(64 * 1024) {
        tx.send(Ok(frame.to_vec())).await.unwrap();
    }
    // Only cut the connection once the server has demonstrably written part
    // of the body to a temp, so the cleanup under test has something to do.
    wait_for_temp_bytes(app.storage_path(), 4 * MIB as u64).await;
    assert_eq!(temp_files(app.storage_path()).len(), 1);

    tx.send(Err(std::io::Error::other("client went away")))
        .await
        .unwrap();
    drop(tx);
    assert!(response.await.expect("join").is_err());

    assert_no_temp_files(app.storage_path()).await;
    assert!(!app.storage_path().join("alice/partial.bin").exists());
    let listing = alice.get_json("/api/files").await;
    assert!(listing.as_array().expect("files array").is_empty());
    assert_eq!(used_bytes(&app.db, "alice").await, 0);

    app.db.drop().await.ok();
}

#[tokio::test]
async fn disconnect_during_content_update_keeps_existing_content() {
    let app = BoundTestApp::new().await;
    let alice = HttpUser::register(&app, "alice").await;

    let original = pattern(MIB + 11, 15);
    let (status, file) = alice
        .upload_streamed("/api/uploads/simple", "doc.bin", &original, None)
        .await;
    assert_eq!(status, reqwest::StatusCode::OK, "{file}");
    let id = file["id"].as_str().expect("file id").to_string();
    assert_no_temp_files(app.storage_path()).await;

    let (tx, response) = alice.start_multipart(&format!("/api/files/{id}/content"));
    tx.send(Ok(file_part_head("doc.bin"))).await.unwrap();
    for frame in pattern(6 * MIB, 16).chunks(64 * 1024) {
        tx.send(Ok(frame.to_vec())).await.unwrap();
    }
    wait_for_temp_bytes(app.storage_path(), 4 * MIB as u64).await;

    tx.send(Err(std::io::Error::other("client went away")))
        .await
        .unwrap();
    drop(tx);
    assert!(response.await.expect("join").is_err());

    assert_no_temp_files(app.storage_path()).await;
    let on_disk = std::fs::read(app.storage_path().join("alice/doc.bin")).expect("blob");
    assert_eq!(on_disk, original);
    let stored = file_doc(&app.db, &id).await;
    assert_eq!(
        stored.get_str("checksum_sha256").unwrap(),
        sha256_hex(&original)
    );
    let versions = alice.get_json(&format!("/api/files/{id}/versions")).await;
    assert!(versions.as_array().expect("versions array").is_empty());
    assert_eq!(audit_operations(&app.db, &id).await, ["created"]);

    app.db.drop().await.ok();
}

// ── Real HTTP: memory stays flat for a large body ────────────────────────────

#[cfg(target_os = "linux")]
fn peak_rss_bytes() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").expect("proc status");
    let kb: u64 = status
        .lines()
        .find_map(|l| l.strip_prefix("VmHWM:"))
        .expect("VmHWM")
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse()
        .expect("VmHWM value");
    kb * 1024
}

/// Uploads a body far larger than the staging buffer, generated on the fly so
/// the test client does not hold it either, and checks the process high-water
/// mark barely moves. Ignored by default: it writes 256 MiB to the temp dir
/// and a resident-memory bound is too environment-sensitive for every run.
///
///     cargo test -p uncloud-server --test upload_streaming -- --ignored large_upload
#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore]
async fn large_upload_does_not_grow_resident_memory() {
    const TOTAL: usize = 256 * MIB;
    const FRAME: usize = 64 * 1024;
    const ALLOWED_GROWTH: u64 = 48 * MIB as u64;

    let app = BoundTestApp::new().await;
    let alice = HttpUser::register(&app, "alice").await;

    // Warm up allocator arenas, connection pools and the staging buffer so
    // the measurement only sees what scales with the body.
    let (status, _) = alice
        .upload_streamed(
            "/api/uploads/simple",
            "warmup.bin",
            &pattern(8 * MIB, 17),
            None,
        )
        .await;
    assert_eq!(status, reqwest::StatusCode::OK);

    let before = peak_rss_bytes();

    let frame = pattern(FRAME, 18);
    let mut hasher = Sha256::new();
    let (tx, response) = alice.start_multipart("/api/uploads/simple");
    tx.send(Ok(file_part_head("huge.bin"))).await.unwrap();
    for _ in 0..TOTAL / FRAME {
        hasher.update(&frame);
        tx.send(Ok(frame.clone())).await.unwrap();
    }
    tx.send(Ok(closing_boundary())).await.unwrap();
    drop(tx);
    let response = response.await.expect("join").expect("response");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let file: Value = response.json().await.expect("json body");

    let growth = peak_rss_bytes().saturating_sub(before);
    println!(
        "peak RSS grew by {:.1} MiB for a {} MiB upload",
        growth as f64 / MIB as f64,
        TOTAL / MIB
    );

    let stored = file_doc(&app.db, file["id"].as_str().unwrap()).await;
    assert_eq!(stored.get_i64("size_bytes").unwrap(), TOTAL as i64);
    assert_eq!(
        stored.get_str("checksum_sha256").unwrap(),
        hex::encode(hasher.finalize())
    );
    assert!(
        growth < ALLOWED_GROWTH,
        "peak RSS grew by {growth} bytes for a {TOTAL} byte upload"
    );

    app.db.drop().await.ok();
}
