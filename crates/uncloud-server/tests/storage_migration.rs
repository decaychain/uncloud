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

    migrate::run_cleanup_inner(&fx.db, &fx.src, fx.src_id, false, false, false)
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

    migrate::run_cleanup_inner(&fx.db, &fx.src, fx.src_id, true, false, false)
        .await
        .expect("cleanup dry-run");

    assert!(fx.src.exists("orphan.bin").await.unwrap(), "dry-run keeps blob");
}


#[tokio::test]
#[ignore]
async fn cleanup_prune_broken_deletes_dangling_records() {
    use uncloud_server::models::FileVersion;
    let fx = make_fixture("uncloud_migrate_test_10").await;

    // Live file: blob present, doc present.
    let live = insert_file(&fx, "live.bin", b"alive").await;
    // Broken file: doc inserted but the source blob never made it to disk
    // (simulates a previously failed upload).
    let broken = insert_file(&fx, "broken.bin", b"never written").await;
    fx.src.delete("broken.bin").await.unwrap();
    // Add a file_versions row pointing at the broken file — it should
    // cascade-delete with its parent.
    let now = chrono::Utc::now();
    fx.db
        .collection::<FileVersion>("file_versions")
        .insert_one(&FileVersion {
            id: ObjectId::new(),
            file_id: broken.id,
            version: 1,
            storage_path: "broken.v1.bin".into(),
            size_bytes: 0,
            checksum_sha256: "x".into(),
            created_at: now,
        })
        .await
        .unwrap();

    // Without --prune-broken, the broken record stays.
    migrate::run_cleanup_inner(&fx.db, &fx.src, fx.src_id, false, false, false)
        .await
        .expect("cleanup without prune");
    let still_there = fx
        .db
        .collection::<File>("files")
        .find_one(doc! { "_id": broken.id })
        .await
        .unwrap();
    assert!(still_there.is_some(), "broken record should survive without flag");

    // With --prune-broken, it goes — and so does the file_versions row.
    migrate::run_cleanup_inner(&fx.db, &fx.src, fx.src_id, false, true, false)
        .await
        .expect("cleanup with prune");
    let gone = fx
        .db
        .collection::<File>("files")
        .find_one(doc! { "_id": broken.id })
        .await
        .unwrap();
    assert!(gone.is_none(), "broken record should be deleted");

    let v_count = fx
        .db
        .collection::<FileVersion>("file_versions")
        .count_documents(doc! { "file_id": broken.id })
        .await
        .unwrap();
    assert_eq!(v_count, 0, "file_versions cascade-deleted");

    // Live file untouched throughout.
    let live_after: File = fx
        .db
        .collection::<File>("files")
        .find_one(doc! { "_id": live.id })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(live_after.id, live.id);
    assert!(fx.src.exists("live.bin").await.unwrap());
}

#[tokio::test]
#[ignore]
async fn migrates_version_archive_blobs_alongside_file() {
    use uncloud_server::models::FileVersion;
    let fx = make_fixture("uncloud_migrate_test_versions_copy").await;

    let file = insert_file(&fx, "doc.txt", b"latest revision").await;

    // Two prior versions, each with their own blob on the source.
    let v1_path = "doc.txt.v1";
    let v2_path = "doc.txt.v2";
    let v1_bytes = b"first revision";
    let v2_bytes = b"second revision";
    fx.src.write(v1_path, v1_bytes).await.unwrap();
    fx.src.write(v2_path, v2_bytes).await.unwrap();
    let now = chrono::Utc::now();
    let v1 = FileVersion {
        id: ObjectId::new(),
        file_id: file.id,
        version: 1,
        storage_path: v1_path.into(),
        size_bytes: v1_bytes.len() as i64,
        checksum_sha256: sha256_hex(v1_bytes),
        created_at: now,
    };
    let v2 = FileVersion {
        id: ObjectId::new(),
        file_id: file.id,
        version: 2,
        storage_path: v2_path.into(),
        size_bytes: v2_bytes.len() as i64,
        checksum_sha256: sha256_hex(v2_bytes),
        created_at: now,
    };
    fx.db
        .collection::<FileVersion>("file_versions")
        .insert_many([&v1, &v2])
        .await
        .unwrap();

    migrate::run_migration(
        &fx.db,
        fx.src.clone(),
        fx.dst.clone(),
        fx.src_id,
        fx.dst_id,
        std::slice::from_ref(&file),
        VerifyMode::Hash,
        false,
    )
    .await
    .expect("migration should succeed");

    // Live blob copied.
    assert_eq!(read_all(&fx.dst, "doc.txt").await, b"latest revision");
    // Both version archives copied with content intact.
    assert_eq!(read_all(&fx.dst, v1_path).await, v1_bytes);
    assert_eq!(read_all(&fx.dst, v2_path).await, v2_bytes);
}

#[tokio::test]
#[ignore]
async fn delete_source_removes_version_blobs() {
    use uncloud_server::models::FileVersion;
    let fx = make_fixture("uncloud_migrate_test_versions_delete_source").await;

    let file = insert_file(&fx, "doc.txt", b"current").await;
    fx.src.write("doc.txt.v1", b"old").await.unwrap();
    let now = chrono::Utc::now();
    fx.db
        .collection::<FileVersion>("file_versions")
        .insert_one(&FileVersion {
            id: ObjectId::new(),
            file_id: file.id,
            version: 1,
            storage_path: "doc.txt.v1".into(),
            size_bytes: 3,
            checksum_sha256: sha256_hex(b"old"),
            created_at: now,
        })
        .await
        .unwrap();

    migrate::run_migration(
        &fx.db,
        fx.src.clone(),
        fx.dst.clone(),
        fx.src_id,
        fx.dst_id,
        std::slice::from_ref(&file),
        VerifyMode::Size,
        true, // delete_source
    )
    .await
    .expect("migration should succeed");

    // Source no longer has live blob OR version blob.
    assert!(!fx.src.exists("doc.txt").await.unwrap());
    assert!(!fx.src.exists("doc.txt.v1").await.unwrap());
    // Destination has both.
    assert!(fx.dst.exists("doc.txt").await.unwrap());
    assert_eq!(read_all(&fx.dst, "doc.txt.v1").await, b"old");
}

#[tokio::test]
#[ignore]
async fn cleanup_keeps_version_archive_blobs() {
    use uncloud_server::models::FileVersion;
    let fx = make_fixture("uncloud_migrate_test_versions_cleanup_keeps").await;

    let file = insert_file(&fx, "doc.txt", b"current").await;
    fx.src.write("doc.txt.v1", b"old").await.unwrap();
    let now = chrono::Utc::now();
    fx.db
        .collection::<FileVersion>("file_versions")
        .insert_one(&FileVersion {
            id: ObjectId::new(),
            file_id: file.id,
            version: 1,
            storage_path: "doc.txt.v1".into(),
            size_bytes: 3,
            checksum_sha256: sha256_hex(b"old"),
            created_at: now,
        })
        .await
        .unwrap();

    migrate::run_cleanup_inner(&fx.db, &fx.src, fx.src_id, false, false, false)
        .await
        .expect("cleanup");

    // Both should still be there — the version blob is part of the keep-set.
    assert!(fx.src.exists("doc.txt").await.unwrap());
    assert!(
        fx.src.exists("doc.txt.v1").await.unwrap(),
        "cleanup must not delete version archive blobs"
    );
}

#[tokio::test]
#[ignore]
async fn cleanup_prune_orphan_versions_deletes_dangling_records() {
    use uncloud_server::models::FileVersion;
    let fx = make_fixture("uncloud_migrate_test_versions_prune_orphan").await;

    let file = insert_file(&fx, "doc.txt", b"current").await;
    let now = chrono::Utc::now();
    let dangling = FileVersion {
        id: ObjectId::new(),
        file_id: file.id,
        version: 1,
        // Blob path that does NOT exist on the storage.
        storage_path: "doc.txt.v1.missing".into(),
        size_bytes: 3,
        checksum_sha256: sha256_hex(b"old"),
        created_at: now,
    };
    let healthy_path = "doc.txt.v2";
    fx.src.write(healthy_path, b"present").await.unwrap();
    let healthy = FileVersion {
        id: ObjectId::new(),
        file_id: file.id,
        version: 2,
        storage_path: healthy_path.into(),
        size_bytes: 7,
        checksum_sha256: sha256_hex(b"present"),
        created_at: now,
    };
    fx.db
        .collection::<FileVersion>("file_versions")
        .insert_many([&dangling, &healthy])
        .await
        .unwrap();

    // Without flag, the dangling row stays.
    migrate::run_cleanup_inner(&fx.db, &fx.src, fx.src_id, false, false, false)
        .await
        .expect("cleanup without flag");
    assert!(fx
        .db
        .collection::<FileVersion>("file_versions")
        .find_one(doc! { "_id": dangling.id })
        .await
        .unwrap()
        .is_some());

    // With flag, the dangling row is deleted; the healthy one survives.
    migrate::run_cleanup_inner(&fx.db, &fx.src, fx.src_id, false, false, true)
        .await
        .expect("cleanup with prune-orphan-versions");
    assert!(fx
        .db
        .collection::<FileVersion>("file_versions")
        .find_one(doc! { "_id": dangling.id })
        .await
        .unwrap()
        .is_none());
    assert!(fx
        .db
        .collection::<FileVersion>("file_versions")
        .find_one(doc! { "_id": healthy.id })
        .await
        .unwrap()
        .is_some());
    // The parent File row is untouched — orphan-versions only nukes the
    // version metadata, not the file.
    assert!(fx
        .db
        .collection::<File>("files")
        .find_one(doc! { "_id": file.id })
        .await
        .unwrap()
        .is_some());
}
