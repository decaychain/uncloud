// Integration tests for the S3 storage backend, run against a MinIO container.
// Marked `#[ignore]` so they only execute when explicitly requested:
//
//     cargo test -p uncloud-server --test storage_s3 -- --ignored
//
// Requires Docker. Not run by CI (which only does `cargo check`).

use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_s3::Client as S3Client;
use testcontainers::core::WaitFor;
use testcontainers::{runners::AsyncRunner, GenericImage, ImageExt};
use tokio::io::AsyncReadExt;
use uncloud_server::storage::{S3Storage, StorageBackend};

const ACCESS_KEY: &str = "minioadmin";
const SECRET_KEY: &str = "minioadmin";
const BUCKET: &str = "test-bucket";

async fn start_minio() -> (
    testcontainers::ContainerAsync<GenericImage>,
    String, // endpoint URL
) {
    let container = GenericImage::new("minio/minio", "latest")
        .with_exposed_port(9000.into())
        .with_wait_for(WaitFor::message_on_stderr("API:"))
        .with_env_var("MINIO_ROOT_USER", ACCESS_KEY)
        .with_env_var("MINIO_ROOT_PASSWORD", SECRET_KEY)
        .with_cmd(vec!["server", "/data"])
        .start()
        .await
        .expect("start minio");
    let port = container
        .get_host_port_ipv4(9000)
        .await
        .expect("minio port");
    let endpoint = format!("http://127.0.0.1:{port}");
    (container, endpoint)
}

async fn make_bucket(endpoint: &str) {
    let creds = Credentials::new(ACCESS_KEY, SECRET_KEY, None, None, "test");
    let conf = aws_sdk_s3::config::Builder::new()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(creds)
        .endpoint_url(endpoint)
        .force_path_style(true)
        .build();
    let client = S3Client::from_conf(conf);
    client
        .create_bucket()
        .bucket(BUCKET)
        .send()
        .await
        .expect("create bucket");
}

async fn make_storage(endpoint: &str) -> S3Storage {
    S3Storage::new(
        endpoint,
        BUCKET,
        ACCESS_KEY,
        SECRET_KEY,
        Some("us-east-1"),
        Default::default(),
    )
    .await
    .expect("S3Storage::new")
}

#[tokio::test]
#[ignore]
async fn write_read_delete_roundtrip() {
    let (_minio, endpoint) = start_minio().await;
    make_bucket(&endpoint).await;
    let s = make_storage(&endpoint).await;

    let data = b"hello, s3 world";
    s.write("blobs/abc.bin", data).await.unwrap();

    assert!(s.exists("blobs/abc.bin").await.unwrap());
    assert!(!s.exists("blobs/missing.bin").await.unwrap());

    let mut reader = s.read("blobs/abc.bin").await.unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, data);

    s.delete("blobs/abc.bin").await.unwrap();
    assert!(!s.exists("blobs/abc.bin").await.unwrap());
}

#[tokio::test]
#[ignore]
async fn read_range_works() {
    let (_minio, endpoint) = start_minio().await;
    make_bucket(&endpoint).await;
    let s = make_storage(&endpoint).await;

    s.write("range.txt", b"Hello, World!").await.unwrap();

    let mut reader = s.read_range("range.txt", 7, 5).await.unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(&buf, b"World");
}

#[tokio::test]
#[ignore]
async fn temp_upload_finalizes() {
    let (_minio, endpoint) = start_minio().await;
    make_bucket(&endpoint).await;
    let s = make_storage(&endpoint).await;

    let temp = s.create_temp().await.unwrap();
    s.append_temp(&temp, b"chunk-1;").await.unwrap();
    s.append_temp(&temp, b"chunk-2;").await.unwrap();
    s.append_temp(&temp, b"chunk-3").await.unwrap();
    s.finalize_temp(&temp, "uploads/final.bin").await.unwrap();

    let mut reader = s.read("uploads/final.bin").await.unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(&buf, b"chunk-1;chunk-2;chunk-3");
}

/// The upload handlers drive the temp API through `UploadStaging`. Run the
/// same sequence against MinIO: a multi-megabyte body in small frames
/// replacing an existing object.
#[tokio::test]
#[ignore]
async fn staged_upload_replaces_object() {
    use mongodb::bson::oid::ObjectId;
    use sha2::{Digest, Sha256};
    use std::sync::Arc;
    use uncloud_server::storage::UploadStaging;

    let (_minio, endpoint) = start_minio().await;
    make_bucket(&endpoint).await;
    let storage: Arc<dyn StorageBackend> = Arc::new(make_storage(&endpoint).await);
    let storage_id = ObjectId::new();

    storage
        .write("uploads/doc.bin", b"old content")
        .await
        .unwrap();

    let data: Vec<u8> = (0..9 * 1024 * 1024 + 321u32)
        .map(|i| i.wrapping_mul(2_654_435_761).to_le_bytes()[1])
        .collect();
    let mut staging = UploadStaging::create(storage_id, storage.clone())
        .await
        .unwrap();
    for frame in data.chunks(64 * 1024) {
        staging.write(frame).await.unwrap();
    }
    let staged = staging.finish().await.unwrap();
    assert_eq!(staged.size_bytes(), data.len() as i64);
    assert_eq!(staged.checksum_sha256(), hex::encode(Sha256::digest(&data)));
    staged
        .commit(storage_id, &storage, "uploads/doc.bin")
        .await
        .unwrap();
    assert_eq!(storage.read_all("uploads/doc.bin").await.unwrap(), data);
}

#[tokio::test]
#[ignore]
async fn temp_can_be_read_back_before_finalize() {
    let (_minio, endpoint) = start_minio().await;
    make_bucket(&endpoint).await;
    let s = make_storage(&endpoint).await;

    let temp = s.create_temp().await.unwrap();
    s.append_temp(&temp, b"chunk-1;").await.unwrap();
    s.append_temp(&temp, b"chunk-2").await.unwrap();

    let mut reader = s.read_temp(&temp).await.unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    drop(reader);
    assert_eq!(&buf, b"chunk-1;chunk-2");

    s.abort_temp(&temp).await.unwrap();
    assert!(s.read_temp(&temp).await.is_err());
}

#[tokio::test]
#[ignore]
async fn rename_and_archive() {
    let (_minio, endpoint) = start_minio().await;
    make_bucket(&endpoint).await;
    let s = make_storage(&endpoint).await;

    s.write("a.txt", b"alpha").await.unwrap();
    s.rename("a.txt", "moved/b.txt").await.unwrap();
    assert!(!s.exists("a.txt").await.unwrap());
    assert!(s.exists("moved/b.txt").await.unwrap());

    s.archive_version("moved/b.txt", "versions/b.v1.txt")
        .await
        .unwrap();
    assert!(s.exists("moved/b.txt").await.unwrap()); // original kept
    assert!(s.exists("versions/b.v1.txt").await.unwrap());
}

#[tokio::test]
#[ignore]
async fn scan_lists_all_keys() {
    let (_minio, endpoint) = start_minio().await;
    make_bucket(&endpoint).await;
    let s = make_storage(&endpoint).await;

    s.write("dir1/a.txt", b"a").await.unwrap();
    s.write("dir1/b.txt", b"bb").await.unwrap();
    s.write("dir2/c.txt", b"ccc").await.unwrap();

    let entries = s.scan("").await.unwrap();
    let mut keys: Vec<_> = entries.iter().map(|e| e.path.clone()).collect();
    keys.sort();
    assert_eq!(keys, vec!["dir1/a.txt", "dir1/b.txt", "dir2/c.txt"]);

    let dir1 = s.scan("dir1").await.unwrap();
    assert_eq!(dir1.len(), 2);

    let total_size: u64 = s.scan("").await.unwrap().iter().map(|e| e.size_bytes).sum();
    assert_eq!(total_size, 1 + 2 + 3);
}

#[tokio::test]
#[ignore]
async fn missing_bucket_fails_fast() {
    let (_minio, endpoint) = start_minio().await;
    // do NOT create the bucket
    let res = S3Storage::new(
        &endpoint,
        "no-such-bucket",
        ACCESS_KEY,
        SECRET_KEY,
        None,
        Default::default(),
    )
    .await;
    assert!(res.is_err(), "expected new() to fail with no bucket");
}
