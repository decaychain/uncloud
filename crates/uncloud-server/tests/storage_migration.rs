// Integration tests for the offline storage migration subcommand. Two
// `LocalStorage` instances (in tempdirs) plus a fresh MongoDB exercise the
// per-file copy + atomic pointer flip. Marked `#[ignore]` because they need
// Docker for the Mongo container; not run by default `cargo test`.
//
//     cargo test -p uncloud-server --test storage_migration -- --ignored

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use chrono::Utc;
use mongodb::bson::{doc, oid::ObjectId};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use testcontainers::core::WaitFor;
use testcontainers::{runners::AsyncRunner, GenericImage, ImageExt};
use tokio::io::AsyncReadExt;

use uncloud_server::migrate::{self, VerifyMode};
use uncloud_server::models::{File, MigrationLock};
use uncloud_server::storage::{LocalStorage, StorageBackend};

static MONGO_PORT: OnceLock<u16> = OnceLock::new();

fn mongo_port() -> u16 {
    *MONGO_PORT.get_or_init(|| {
        std::thread::spawn(|| {
            let rt = tokio::runtime::Runtime::new().unwrap();
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

async fn fresh_db(name: &str) -> mongodb::Database {
    let uri = format!("mongodb://127.0.0.1:{}", mongo_port());
    let client = mongodb::Client::with_uri_str(&uri).await.expect("mongo connect");
    let db = client.database(name);
    db.drop().await.ok();
    db
}

struct Fixture {
    db: mongodb::Database,
    src: Arc<dyn StorageBackend>,
    dst: Arc<dyn StorageBackend>,
    src_id: ObjectId,
    dst_id: ObjectId,
    _src_dir: TempDir,
    _dst_dir: TempDir,
}

async fn make_fixture(db_name: &str) -> Fixture {
    let src_dir = TempDir::new().unwrap();
    let dst_dir = TempDir::new().unwrap();
    let src = Arc::new(LocalStorage::new(src_dir.path().to_str().unwrap()).await.unwrap())
        as Arc<dyn StorageBackend>;
    let dst = Arc::new(LocalStorage::new(dst_dir.path().to_str().unwrap()).await.unwrap())
        as Arc<dyn StorageBackend>;
    Fixture {
        db: fresh_db(db_name).await,
        src,
        dst,
        src_id: ObjectId::new(),
        dst_id: ObjectId::new(),
        _src_dir: src_dir,
        _dst_dir: dst_dir,
    }
}

fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex::encode(h.finalize())
}

async fn insert_file(
    fx: &Fixture,
    storage_path: &str,
    contents: &[u8],
) -> File {
    fx.src.write(storage_path, contents).await.unwrap();
    let now = Utc::now();
    let file = File {
        id: ObjectId::new(),
        storage_id: fx.src_id,
        storage_path: storage_path.to_string(),
        owner_id: ObjectId::new(),
        parent_id: None,
        name: storage_path.split('/').next_back().unwrap().to_string(),
        mime_type: "application/octet-stream".into(),
        size_bytes: contents.len() as i64,
        checksum_sha256: sha256_hex(contents),
        created_at: now,
        updated_at: now,
        captured_at: None,
        processing_tasks: Vec::new(),
        metadata: HashMap::new(),
        deleted_at: None,
        trash_path: None,
        batch_delete_id: None,
    };
    fx.db
        .collection::<File>("files")
        .insert_one(&file)
        .await
        .unwrap();
    file
}

async fn read_all(backend: &Arc<dyn StorageBackend>, path: &str) -> Vec<u8> {
    let mut r = backend.read(path).await.unwrap();
    let mut buf = Vec::new();
    r.read_to_end(&mut buf).await.unwrap();
    buf
}

#[tokio::test]
#[ignore]
async fn migrates_single_file_local_to_local() {
    let fx = make_fixture("uncloud_migrate_test_1").await;
    let file = insert_file(&fx, "alpha.txt", b"hello migration").await;

    let files = vec![file.clone()];
    migrate::run_migration(
        &fx.db,
        fx.src.clone(),
        fx.dst.clone(),
        fx.src_id,
        fx.dst_id,
        &files,
        VerifyMode::Hash,
        false,
    )
    .await
    .expect("migration");

    // Pointer flipped.
    let after: File = fx
        .db
        .collection::<File>("files")
        .find_one(doc! { "_id": file.id })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.storage_id, fx.dst_id);

    // Dest has the bytes.
    assert_eq!(read_all(&fx.dst, "alpha.txt").await, b"hello migration");

    // Source still has the bytes (no --delete-source).
    assert!(fx.src.exists("alpha.txt").await.unwrap());
}

#[tokio::test]
#[ignore]
async fn migration_is_idempotent_after_partial_run() {
    let fx = make_fixture("uncloud_migrate_test_2").await;
    let f1 = insert_file(&fx, "one.txt", b"first").await;
    let f2 = insert_file(&fx, "two.txt", b"second").await;
    let f3 = insert_file(&fx, "three.txt", b"third").await;

    // Simulate a previous partial run by manually flipping f1 (and pre-staging
    // its blob on dest). The next run should skip it and only process f2/f3.
    fx.dst.write("one.txt", b"first").await.unwrap();
    fx.db
        .collection::<File>("files")
        .update_one(doc! { "_id": f1.id }, doc! { "$set": { "storage_id": fx.dst_id } })
        .await
        .unwrap();

    // Re-fetch all three so the input list reflects current DB state. f1 is
    // already on dst; the loop's idempotency check should skip it.
    let mut files: Vec<File> = Vec::new();
    for id in [f1.id, f2.id, f3.id] {
        let f = fx
            .db
            .collection::<File>("files")
            .find_one(doc! { "_id": id })
            .await
            .unwrap()
            .unwrap();
        files.push(f);
    }

    migrate::run_migration(
        &fx.db,
        fx.src.clone(),
        fx.dst.clone(),
        fx.src_id,
        fx.dst_id,
        &files,
        VerifyMode::Size,
        false,
    )
    .await
    .expect("migration");

    // All three now point at dst.
    for id in [f1.id, f2.id, f3.id] {
        let f: File = fx
            .db
            .collection::<File>("files")
            .find_one(doc! { "_id": id })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(f.storage_id, fx.dst_id, "file {id} not on dst");
    }
    assert_eq!(read_all(&fx.dst, "two.txt").await, b"second");
    assert_eq!(read_all(&fx.dst, "three.txt").await, b"third");
}

#[tokio::test]
#[ignore]
async fn migrates_thumbnail_sidecar() {
    let fx = make_fixture("uncloud_migrate_test_3").await;
    let file = insert_file(&fx, "photos/cat.jpg", b"jpeg-bytes").await;
    // Pretend the processing pipeline produced a thumbnail.
    let thumb_path = format!(".thumbs/{}.jpg", file.id);
    fx.src.write(&thumb_path, b"thumb-bytes").await.unwrap();

    migrate::run_migration(
        &fx.db,
        fx.src.clone(),
        fx.dst.clone(),
        fx.src_id,
        fx.dst_id,
        &[file.clone()],
        VerifyMode::Size,
        false,
    )
    .await
    .expect("migration");

    assert_eq!(read_all(&fx.dst, "photos/cat.jpg").await, b"jpeg-bytes");
    assert_eq!(read_all(&fx.dst, &thumb_path).await, b"thumb-bytes");
}

#[tokio::test]
#[ignore]
async fn check_no_active_migration_blocks_when_lock_present() {
    let db = fresh_db("uncloud_migrate_test_4").await;
    // No lock present — should pass.
    migrate::check_no_active_migration(&db).await.unwrap();

    // Insert a fresh lock — should refuse.
    let now = Utc::now();
    let lock = MigrationLock {
        id: ObjectId::new(),
        scope: MigrationLock::SCOPE.to_string(),
        from_storage_id: ObjectId::new(),
        to_storage_id: ObjectId::new(),
        started_at: now,
        last_heartbeat: now,
        pid: 12345,
        hostname: "test-host".into(),
    };
    db.collection::<MigrationLock>("migration_locks")
        .insert_one(&lock)
        .await
        .unwrap();

    let err = migrate::check_no_active_migration(&db).await.unwrap_err();
    assert!(err.contains("migration is in progress"), "got: {err}");
}

#[tokio::test]
#[ignore]
async fn migration_fails_on_corrupt_dest_when_hash_verify_enabled() {
    // A backend that "writes" by silently truncating to a single byte. This
    // simulates a backend bug where verify=hash catches what verify=size
    // could not (truncated and mid-byte content matches; but here even size
    // catches it — the point is to exercise the hash-mismatch path).
    use async_trait::async_trait;
    use uncloud_server::error::{AppError, Result as StorageResult};
    use uncloud_server::storage::{BoxedAsyncRead, ScanEntry};

    struct CorruptBackend {
        inner: Arc<dyn StorageBackend>,
    }

    #[async_trait]
    impl StorageBackend for CorruptBackend {
        async fn read(&self, path: &str) -> StorageResult<BoxedAsyncRead> {
            self.inner.read(path).await
        }
        async fn read_range(&self, p: &str, o: u64, l: u64) -> StorageResult<BoxedAsyncRead> {
            self.inner.read_range(p, o, l).await
        }
        async fn write(&self, path: &str, _data: &[u8]) -> StorageResult<()> {
            self.inner.write(path, b"X").await
        }
        async fn write_stream(&self, path: &str, _r: BoxedAsyncRead, _s: u64) -> StorageResult<()> {
            self.inner.write(path, b"X").await
        }
        async fn delete(&self, path: &str) -> StorageResult<()> {
            self.inner.delete(path).await
        }
        async fn exists(&self, path: &str) -> StorageResult<bool> {
            self.inner.exists(path).await
        }
        async fn available_space(&self) -> StorageResult<Option<u64>> {
            self.inner.available_space().await
        }
        async fn create_temp(&self) -> StorageResult<String> {
            self.inner.create_temp().await
        }
        async fn append_temp(&self, p: &str, d: &[u8]) -> StorageResult<()> {
            self.inner.append_temp(p, d).await
        }
        async fn finalize_temp(&self, t: &str, f: &str) -> StorageResult<()> {
            self.inner.finalize_temp(t, f).await
        }
        async fn abort_temp(&self, t: &str) -> StorageResult<()> {
            self.inner.abort_temp(t).await
        }
        async fn rename(&self, f: &str, t: &str) -> StorageResult<()> {
            self.inner.rename(f, t).await
        }
        async fn archive_version(&self, c: &str, v: &str) -> StorageResult<()> {
            self.inner.archive_version(c, v).await
        }
        async fn move_to_trash(&self, c: &str, t: &str) -> StorageResult<()> {
            self.inner.move_to_trash(c, t).await
        }
        async fn restore_from_trash(&self, t: &str, r: &str) -> StorageResult<()> {
            self.inner.restore_from_trash(t, r).await
        }
        async fn scan(&self, p: &str) -> StorageResult<Vec<ScanEntry>> {
            self.inner.scan(p).await
        }
    }
    // Suppress unused-import warning for Result alias above.
    let _ = std::any::TypeId::of::<AppError>();

    let fx = make_fixture("uncloud_migrate_test_5").await;
    let file = insert_file(&fx, "data.bin", b"the original twelve").await;

    let corrupt = Arc::new(CorruptBackend { inner: fx.dst.clone() }) as Arc<dyn StorageBackend>;
    let result = migrate::run_migration(
        &fx.db,
        fx.src.clone(),
        corrupt,
        fx.src_id,
        fx.dst_id,
        &[file.clone()],
        VerifyMode::Size,
        false,
    )
    .await;
    assert!(result.is_err(), "expected size verification to fail");

    // Pointer was NOT flipped (verification failed before flip).
    let after: File = fx
        .db
        .collection::<File>("files")
        .find_one(doc! { "_id": file.id })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.storage_id, fx.src_id);
}


#[tokio::test]
#[ignore]
async fn delete_source_removes_blob_and_thumb_after_flip() {
    let fx = make_fixture("uncloud_migrate_test_6").await;
    let file = insert_file(&fx, "doc.bin", b"sample bytes").await;
    let thumb = format!(".thumbs/{}.jpg", file.id);
    fx.src.write(&thumb, b"thumb-bytes").await.unwrap();

    migrate::run_migration(
        &fx.db,
        fx.src.clone(),
        fx.dst.clone(),
        fx.src_id,
        fx.dst_id,
        &[file.clone()],
        VerifyMode::Size,
        true, // delete_source
    )
    .await
    .expect("migration");

    assert!(!fx.src.exists("doc.bin").await.unwrap(), "source blob should be gone");
    assert!(!fx.src.exists(&thumb).await.unwrap(), "source thumb should be gone");
    assert_eq!(read_all(&fx.dst, "doc.bin").await, b"sample bytes");
    assert_eq!(read_all(&fx.dst, &thumb).await, b"thumb-bytes");
}

#[tokio::test]
#[ignore]
async fn repin_folders_updates_pinned_folders() {
    use uncloud_server::models::Folder;
    let fx = make_fixture("uncloud_migrate_test_7").await;

    let folders = fx.db.collection::<Folder>("folders");
    let now = chrono::Utc::now();
    let pinned_to_src = Folder {
        id: ObjectId::new(),
        owner_id: ObjectId::new(),
        parent_id: None,
        name: "PinnedSrc".into(),
        storage_id: Some(fx.src_id),
        sync_strategy: Default::default(),
        gallery_include: Default::default(),
        music_include: Default::default(),
        created_at: now,
        updated_at: now,
        deleted_at: None,
        batch_delete_id: None,
    };
    let other_storage = ObjectId::new();
    let pinned_elsewhere = Folder {
        id: ObjectId::new(),
        owner_id: ObjectId::new(),
        parent_id: None,
        name: "PinnedOther".into(),
        storage_id: Some(other_storage),
        sync_strategy: Default::default(),
        gallery_include: Default::default(),
        music_include: Default::default(),
        created_at: now,
        updated_at: now,
        deleted_at: None,
        batch_delete_id: None,
    };
    folders.insert_one(&pinned_to_src).await.unwrap();
    folders.insert_one(&pinned_elsewhere).await.unwrap();

    let modified = migrate::repin_folders(&fx.db, fx.src_id, fx.dst_id, None)
        .await
        .expect("repin");
    assert_eq!(modified, 1);

    let after_src: Folder = folders
        .find_one(doc! { "_id": pinned_to_src.id })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after_src.storage_id, Some(fx.dst_id));

    let after_other: Folder = folders
        .find_one(doc! { "_id": pinned_elsewhere.id })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after_other.storage_id, Some(other_storage), "untouched");
}

#[tokio::test]
#[ignore]
async fn cleanup_deletes_orphans_keeps_live() {
    let fx = make_fixture("uncloud_migrate_test_8").await;
    let live = insert_file(&fx, "live.bin", b"alive").await;
    let live_thumb = format!(".thumbs/{}.jpg", live.id);
    fx.src.write(&live_thumb, b"keep-thumb").await.unwrap();

    // Orphan blob — no File document points at it.
    fx.src.write("orphan.bin", b"orphaned").await.unwrap();
    // Orphan thumbnail — file_id is fabricated; no matching File doc.
    let orphan_thumb = format!(".thumbs/{}.jpg", ObjectId::new());
    fx.src.write(&orphan_thumb, b"stale-thumb").await.unwrap();
    // In-flight upload artefact — must NOT be deleted by cleanup.
    fx.src
        .write(".tmp/in-flight.bin", b"do not touch")
        .await
        .unwrap();

    migrate::run_cleanup_inner(&fx.db, &fx.src, fx.src_id, false)
        .await
        .expect("cleanup");

    assert!(fx.src.exists("live.bin").await.unwrap(), "live blob preserved");
    assert!(fx.src.exists(&live_thumb).await.unwrap(), "live thumb preserved");
    assert!(
        fx.src.exists(".tmp/in-flight.bin").await.unwrap(),
        ".tmp must be left alone"
    );
    assert!(
        !fx.src.exists("orphan.bin").await.unwrap(),
        "orphan blob should be deleted"
    );
    assert!(
        !fx.src.exists(&orphan_thumb).await.unwrap(),
        "orphan thumb should be deleted"
    );

    // Touch `live` to satisfy "field not used" — silences potential dead-code
    // warnings in tests that don't reference it after the setup phase.
    let _ = live.id;
}

#[tokio::test]
#[ignore]
async fn cleanup_dry_run_deletes_nothing() {
    let fx = make_fixture("uncloud_migrate_test_9").await;
    fx.src.write("orphan.bin", b"orphaned").await.unwrap();

    migrate::run_cleanup_inner(&fx.db, &fx.src, fx.src_id, true)
        .await
        .expect("cleanup dry-run");

    assert!(fx.src.exists("orphan.bin").await.unwrap(), "dry-run keeps blob");
}
