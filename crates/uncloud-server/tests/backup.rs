//! Integration tests for the backup pipeline. Marked `#[ignore]` because
//! they need a Mongo container — not run by default `cargo test`.
//!
//!     cargo test -p uncloud-server --test backup -- --ignored
//!
//! v1 covers the load-bearing dump → restore round-trip at the document
//! level (real Mongo cursor → EJSON NDJSON → BSON Document → comparison).
//! End-to-end CLI smoke (init / create / list / check / restore --dry-run)
//! is verified manually on an empty database — see the PR description.

use std::sync::OnceLock;

use mongodb::bson::{doc, oid::ObjectId, Document};
use testcontainers::core::WaitFor;
use testcontainers::{runners::AsyncRunner, GenericImage, ImageExt};

use uncloud_server::backup::dump;

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
                let port = container
                    .get_host_port_ipv4(27017)
                    .await
                    .expect("mongo port");
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
    let client = mongodb::Client::with_uri_str(&uri).await.expect("connect");
    let db = client.database(name);
    db.drop().await.ok();
    db
}

/// Round-trip a real BSON document through `dump_collection` and
/// `json_to_document` and assert structural equivalence. This is the most
/// load-bearing test for restore correctness — if EJSON round-trip is
/// lossless on our domain types, restore can rely on it.
#[tokio::test]
#[ignore]
async fn dump_roundtrips_real_documents_via_ejson() {
    let db = fresh_db("uncloud_backup_dump_roundtrip").await;

    let doc1 = doc! {
        "_id": ObjectId::new(),
        "username": "alice",
        "created_at": bson::DateTime::from_millis(1_730_000_000_000),
        "tags": ["a", "b", "c"],
        "metadata": {
            "active": true,
            "score": 42i64,
            "pi": 3.14,
        },
    };
    let doc2 = doc! {
        "_id": ObjectId::new(),
        "username": "bob",
        "created_at": bson::DateTime::from_millis(1_731_500_000_500),
        "tags": [],
        "metadata": { "active": false, "score": -7i64 },
    };
    let coll = db.collection::<Document>("users");
    coll.insert_many([&doc1, &doc2]).await.unwrap();

    let mut buf: Vec<u8> = Vec::new();
    let rows = dump::dump_collection(&db, "users", &mut buf).await.unwrap();
    assert_eq!(rows, 2);

    let text = String::from_utf8(buf).expect("ndjson is utf-8");
    let parsed: Vec<Document> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            dump::json_to_document(v).unwrap()
        })
        .collect();
    assert_eq!(parsed.len(), 2);

    // Order-insensitive comparison — Mongo's insert order is not guaranteed
    // across cursor reads.
    let by_id = |d: &Document| d.get_object_id("_id").unwrap();
    let originals: std::collections::HashMap<ObjectId, &Document> =
        [&doc1, &doc2].iter().map(|d| (by_id(d), *d)).collect();
    for got in &parsed {
        let id = by_id(got);
        let original = originals
            .get(&id)
            .unwrap_or_else(|| panic!("got unexpected id {id}"));
        assert_eq!(got, *original, "round-trip mismatch on id {id}");
    }
}

/// Verify the manifest written alongside the dumped collections has the
/// fields restore relies on (schema_version, per-collection counts).
#[tokio::test]
#[ignore]
async fn dump_all_writes_manifest_with_counts() {
    let db = fresh_db("uncloud_backup_manifest").await;
    db.collection::<Document>("users")
        .insert_one(doc! { "_id": ObjectId::new(), "username": "carol" })
        .await
        .unwrap();
    db.collection::<Document>("folders")
        .insert_many([
            doc! { "_id": ObjectId::new(), "owner_id": ObjectId::new(), "name": "a" },
            doc! { "_id": ObjectId::new(), "owner_id": ObjectId::new(), "name": "b" },
        ])
        .await
        .unwrap();

    let dir = tempfile::TempDir::new().unwrap();
    let counts = dump::dump_all(&db, dir.path()).await.unwrap();

    // Verify manifest.json present with expected shape.
    let manifest_bytes = tokio::fs::read(dir.path().join("manifest.json")).await.unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
    assert_eq!(manifest["schema_version"], dump::SCHEMA_VERSION);
    let collections = manifest["collections"].as_array().unwrap();
    assert!(collections.iter().any(|c| c["name"] == "users" && c["rows"] == 1));
    assert!(collections.iter().any(|c| c["name"] == "folders" && c["rows"] == 2));

    // And a per-collection jsonl exists for at least the seeded ones.
    let users_path = dir.path().join("users.jsonl");
    assert!(tokio::fs::metadata(&users_path).await.is_ok());
    let users_bytes = tokio::fs::read(&users_path).await.unwrap();
    let users_text = String::from_utf8(users_bytes).unwrap();
    let lines: Vec<&str> = users_text.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 1);

    // And the returned counts match.
    let users_count = counts.iter().find(|(n, _)| n == "users").unwrap().1;
    assert_eq!(users_count, 1);
    let folders_count = counts.iter().find(|(n, _)| n == "folders").unwrap().1;
    assert_eq!(folders_count, 2);
}
