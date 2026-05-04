// Integration tests for the SFTP storage backend, run against an
// `atmoz/sftp` container. Marked `#[ignore]` so they only run when explicitly
// requested:
//
//     cargo test -p uncloud-server --test storage_sftp -- --ignored
//
// Requires Docker. Not run by CI (which only does `cargo check`).

use mongodb::bson::oid::ObjectId;
use std::sync::OnceLock;
use testcontainers::core::WaitFor;
use testcontainers::{runners::AsyncRunner, GenericImage, ImageExt};
use tokio::io::AsyncReadExt;
use uncloud_server::models::StorageBackendConfig;
use uncloud_server::storage::{SftpStorage, StorageBackend};

const USERNAME: &str = "uncloud";
const PASSWORD: &str = "uncloud-test";
/// atmoz/sftp default chroots logins to `/home/{user}`. We give it a
/// writable subdirectory so the backend can create `.tmp/`, etc. The path
/// is relative — leading slash would refer to the chroot root, which the
/// connecting user cannot write to.
const HOST_BASE_PATH: &str = "upload";

static MONGO_PORT: OnceLock<u16> = OnceLock::new();

/// Spin up a single MongoDB container per test binary, just to satisfy the
/// SftpStorage constructor's TOFU host-key write. (We don't actually care
/// about the data — each test uses a fresh database name.) The container
/// handle is intentionally leaked so it lives for the whole test binary's
/// lifetime rather than being dropped at the end of the init thread.
fn mongo_port() -> u16 {
    *MONGO_PORT.get_or_init(|| {
        std::thread::spawn(|| {
            let rt = tokio::runtime::Runtime::new().unwrap();
            // Leak the runtime AND the container so neither gets dropped.
            let rt: &'static tokio::runtime::Runtime = Box::leak(Box::new(rt));
            rt.block_on(async {
                let container = GenericImage::new("mongo", "7")
                    .with_exposed_port(27017.into())
                    .with_wait_for(WaitFor::message_on_stdout("Waiting for connections"))
                    .with_cmd(vec!["mongod", "--wiredTigerCacheSizeGB", "0.25"])
                    .start()
                    .await
                    .expect("start mongo");
                let port = container.get_host_port_ipv4(27017).await.expect("mongo port");
                Box::leak(Box::new(container));
                port
            })
        })
        .join()
        .expect("mongo thread")
    })
}

async fn mongo_db(name: &str) -> mongodb::Database {
    let uri = format!("mongodb://127.0.0.1:{}", mongo_port());
    let client = mongodb::Client::with_uri_str(&uri).await.expect("mongo connect");
    client.database(name)
}

async fn start_sftp_password() -> (testcontainers::ContainerAsync<GenericImage>, u16) {
    let container = GenericImage::new("atmoz/sftp", "alpine")
        .with_exposed_port(22.into())
        .with_wait_for(WaitFor::message_on_stderr("Server listening on"))
        // atmoz/sftp argument format:  user:pass:uid:gid:dirs
        // The trailing `upload` makes atmoz create that directory inside the
        // chroot and chown it to the user — otherwise the user has nowhere
        // writable since the chroot root itself is owned by root.
        .with_cmd(vec!["uncloud:uncloud-test:1001:1001:upload"])
        .start()
        .await
        .expect("start sftp");
    let port = container.get_host_port_ipv4(22).await.expect("sftp port");
    (container, port)
}

fn make_password_config(port: u16) -> StorageBackendConfig {
    StorageBackendConfig::Sftp {
        host: "127.0.0.1".into(),
        port,
        username: USERNAME.into(),
        password: Some(PASSWORD.into()),
        private_key: None,
        private_key_passphrase: None,
        base_path: HOST_BASE_PATH.into(),
        host_key: None,
        host_key_check: Some("skip".into()), // tofu in DB also works, skip is simpler in tests
        connection_pool_size: None,
        max_concurrent_ops: None,
    }
}

#[tokio::test]
#[ignore]
async fn write_read_delete_roundtrip_password_auth() {
    let (_sftp, port) = start_sftp_password().await;
    let db = mongo_db("uncloud_sftp_test_1").await;
    let storage = SftpStorage::new(&make_password_config(port), Default::default(), db, ObjectId::new())
        .await
        .expect("connect");

    let data = b"hello, sftp world";
    storage.write("blobs/abc.bin", data).await.unwrap();

    assert!(storage.exists("blobs/abc.bin").await.unwrap());
    assert!(!storage.exists("blobs/missing.bin").await.unwrap());

    let mut reader = storage.read("blobs/abc.bin").await.unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, data);

    storage.delete("blobs/abc.bin").await.unwrap();
    assert!(!storage.exists("blobs/abc.bin").await.unwrap());
}

#[tokio::test]
#[ignore]
async fn read_range_works() {
    let (_sftp, port) = start_sftp_password().await;
    let db = mongo_db("uncloud_sftp_test_2").await;
    let storage = SftpStorage::new(&make_password_config(port), Default::default(), db, ObjectId::new())
        .await
        .unwrap();

    storage.write("range.txt", b"Hello, World!").await.unwrap();
    let mut reader = storage.read_range("range.txt", 7, 5).await.unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(&buf, b"World");
}

#[tokio::test]
#[ignore]
async fn temp_upload_finalizes() {
    let (_sftp, port) = start_sftp_password().await;
    let db = mongo_db("uncloud_sftp_test_3").await;
    let storage = SftpStorage::new(&make_password_config(port), Default::default(), db, ObjectId::new())
        .await
        .unwrap();

    let temp = storage.create_temp().await.unwrap();
    storage.append_temp(&temp, b"chunk-1;").await.unwrap();
    storage.append_temp(&temp, b"chunk-2;").await.unwrap();
    storage.append_temp(&temp, b"chunk-3").await.unwrap();
    storage.finalize_temp(&temp, "uploads/final.bin").await.unwrap();

    let mut reader = storage.read("uploads/final.bin").await.unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(&buf, b"chunk-1;chunk-2;chunk-3");
}

#[tokio::test]
#[ignore]
async fn rename_and_archive() {
    let (_sftp, port) = start_sftp_password().await;
    let db = mongo_db("uncloud_sftp_test_4").await;
    let storage = SftpStorage::new(&make_password_config(port), Default::default(), db, ObjectId::new())
        .await
        .unwrap();

    storage.write("a.txt", b"alpha").await.unwrap();
    storage.rename("a.txt", "moved/b.txt").await.unwrap();
    assert!(!storage.exists("a.txt").await.unwrap());
    assert!(storage.exists("moved/b.txt").await.unwrap());

    storage
        .archive_version("moved/b.txt", "versions/b.v1.txt")
        .await
        .unwrap();
    assert!(storage.exists("moved/b.txt").await.unwrap());
    assert!(storage.exists("versions/b.v1.txt").await.unwrap());
}

#[tokio::test]
#[ignore]
async fn scan_lists_all_keys() {
    let (_sftp, port) = start_sftp_password().await;
    let db = mongo_db("uncloud_sftp_test_5").await;
    let storage = SftpStorage::new(&make_password_config(port), Default::default(), db, ObjectId::new())
        .await
        .unwrap();

    storage.write("dir1/a.txt", b"a").await.unwrap();
    storage.write("dir1/b.txt", b"bb").await.unwrap();
    storage.write("dir2/c.txt", b"ccc").await.unwrap();

    let entries = storage.scan("").await.unwrap();
    let mut files: Vec<_> = entries
        .iter()
        .filter(|e| !e.is_dir)
        .map(|e| e.path.clone())
        .collect();
    files.sort();
    assert_eq!(files, vec!["dir1/a.txt", "dir1/b.txt", "dir2/c.txt"]);
}

#[tokio::test]
#[ignore]
async fn private_key_auth_works() {
    // Hardcoded ed25519 keypair, generated by `ssh-keygen -t ed25519 -N ""`.
    // Test-only — the SFTP container is throwaway, no security implications.
    let public_openssh = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOXhtPVwOuAYnbOA8ecJ8JYEK0lLKMCvfpDe1a37V4o3 uncloud-test";
    let private_pem = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAACDl4bT1cDrgGJ2zgPHnCfCWBCtJSyjAr36Q3tWt+1eKNwAAAJBhHhC1YR4Q\ntQAAAAtzc2gtZWQyNTUxOQAAACDl4bT1cDrgGJ2zgPHnCfCWBCtJSyjAr36Q3tWt+1eKNw\nAAAEBdQw2K0bugcEaDnTS5XsicWSim+KMp+5eoWhXD+lEvj+XhtPVwOuAYnbOA8ecJ8JYE\nK0lLKMCvfpDe1a37V4o3AAAADHVuY2xvdWQtdGVzdAE=\n-----END OPENSSH PRIVATE KEY-----\n";

    // Stage the authorized key file the way atmoz/sftp expects: a sidecar
    // file mounted at /home/{user}/.ssh/keys/auth-key.pub.
    let tmp = tempfile::tempdir().unwrap();
    let key_dir = tmp.path().join("keys");
    std::fs::create_dir_all(&key_dir).unwrap();
    std::fs::write(key_dir.join("auth-key.pub"), public_openssh.as_bytes()).unwrap();

    use testcontainers::core::Mount;
    let mount = Mount::bind_mount(
        key_dir.to_str().unwrap().to_string(),
        format!("/home/{USERNAME}/.ssh/keys"),
    );
    let container = GenericImage::new("atmoz/sftp", "alpine")
        .with_exposed_port(22.into())
        .with_wait_for(WaitFor::message_on_stderr("Server listening on"))
        .with_mount(mount)
        .with_cmd(vec!["uncloud::1001:1001:upload"])
        .start()
        .await
        .expect("start sftp");
    let port = container.get_host_port_ipv4(22).await.expect("sftp port");

    let cfg = StorageBackendConfig::Sftp {
        host: "127.0.0.1".into(),
        port,
        username: USERNAME.into(),
        password: None,
        private_key: Some(private_pem.to_string()),
        private_key_passphrase: None,
        base_path: HOST_BASE_PATH.into(),
        host_key: None,
        host_key_check: Some("skip".into()),
        connection_pool_size: None,
        max_concurrent_ops: None,
    };
    let db = mongo_db("uncloud_sftp_test_6").await;
    let storage = SftpStorage::new(&cfg, Default::default(), db, ObjectId::new())
        .await
        .expect("key auth connect");

    storage.write("keyauth.txt", b"signed in via key").await.unwrap();
    let mut reader = storage.read("keyauth.txt").await.unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(&buf, b"signed in via key");
}

#[tokio::test]
#[ignore]
async fn tofu_pins_host_key_and_subsequent_connect_validates() {
    let (_sftp, port) = start_sftp_password().await;
    let db = mongo_db("uncloud_sftp_test_7").await;
    let storage_id = ObjectId::new();

    let mut cfg = make_password_config(port);
    if let StorageBackendConfig::Sftp { host_key_check, .. } = &mut cfg {
        *host_key_check = Some("tofu".into());
    }

    // First connect: writes the row.
    let _first = SftpStorage::new(&cfg, Default::default(), db.clone(), storage_id).await.unwrap();
    let pins = db
        .collection::<mongodb::bson::Document>("sftp_host_keys")
        .count_documents(mongodb::bson::doc! { "storage_id": storage_id })
        .await
        .unwrap();
    assert_eq!(pins, 1, "first connect should pin one row");

    // Second connect: row already exists, must succeed against the same key.
    let _second = SftpStorage::new(&cfg, Default::default(), db, storage_id).await.unwrap();
}
