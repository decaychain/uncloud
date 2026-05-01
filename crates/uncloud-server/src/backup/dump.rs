//! Semantic database dump — BSON Document → canonical Extended JSON Lines.
//!
//! Each row is one self-contained JSON object on its own line. We use
//! canonical [Extended JSON](https://www.mongodb.com/docs/manual/reference/mongodb-extended-json/)
//! (EJSON) — `{"$oid": "..."}` for ObjectIds, `{"$date": ...}` for
//! DateTimes, etc. — because it round-trips losslessly through
//! `bson::Bson::try_from(serde_json::Value)`, which is exactly what `restore`
//! needs to invert the dump. A future SQLite/Postgres-backed Uncloud reads
//! the same files; the EJSON wrappers are an inert per-cell hint and easy
//! to strip if a target wants plain types.
//!
//! Skipping the in-house transform also means we delegate every BSON edge
//! case (Decimal128, Binary subtypes, Timestamp, RegEx) to the bson crate
//! rather than re-implementing them.

use std::path::Path;

use bson::{Bson, Document};
use futures::stream::TryStreamExt;
use mongodb::Database;
use serde_json::{json, Value};
use tokio::fs::File;
use tokio::io::{AsyncWriteExt, BufWriter};

use crate::error::{AppError, Result};

/// Collections written to a snapshot's `/database/` directory. Anything not
/// on this list is intentionally not backed up — see `docs/backup.md` for the
/// rationale (sessions, TTL'd transients, lock collections).
pub const COLLECTION_ALLOWLIST: &[&str] = &[
    "users",
    "folders",
    "files",
    "file_versions",
    "storages",
    "shares",
    "folder_shares",
    "api_tokens",
    "s3_credentials",
    "sftp_host_keys",
    "apps",
    "webhooks",
    "sync_events",
    "invites",
    "user_preferences",
    "playlists",
    "shopping_lists",
    "shopping_items",
    "shopping_list_items",
    "shopping_categories",
    "shops",
    "task_projects",
    "task_sections",
    "tasks",
    "task_comments",
    "task_labels",
];

/// Top-level schema version stamped into `/database/manifest.json`.
/// Bump on any incompatible model change.
pub const SCHEMA_VERSION: u32 = 1;

/// Convert a BSON Document into a canonical-EJSON JSON Value.
pub fn document_to_json(doc: Document) -> Value {
    Bson::Document(doc).into_canonical_extjson()
}

/// Convert any BSON value into a canonical-EJSON JSON value.
pub fn bson_to_json(value: Bson) -> Value {
    value.into_canonical_extjson()
}

/// Inverse of [`document_to_json`]. Parses a canonical-EJSON JSON object
/// back into a BSON `Document`, used by `backup restore`.
pub fn json_to_document(value: Value) -> Result<Document> {
    let map = match value {
        Value::Object(m) => m,
        other => {
            return Err(AppError::Internal(format!(
                "expected JSON object, got {:?}",
                other
            )));
        }
    };
    Document::try_from(map)
        .map_err(|e| AppError::Internal(format!("EJSON → BSON document conversion failed: {e}")))
}

/// Stream every row in `collection` as JSON Lines into `writer`.
/// Returns the number of rows written.
pub async fn dump_collection<W>(
    db: &Database,
    collection: &str,
    writer: &mut W,
) -> Result<usize>
where
    W: AsyncWriteExt + Unpin,
{
    let coll = db.collection::<Document>(collection);
    let mut cursor = coll
        .find(bson::doc! {})
        .await
        .map_err(|e| AppError::Internal(format!("dump {collection}: cursor failed: {e}")))?;

    let mut rows = 0usize;
    while let Some(doc) = cursor
        .try_next()
        .await
        .map_err(|e| AppError::Internal(format!("dump {collection}: cursor read: {e}")))?
    {
        let value = document_to_json(doc);
        let line = serde_json::to_string(&value)
            .map_err(|e| AppError::Internal(format!("dump {collection}: serialise row: {e}")))?;
        writer
            .write_all(line.as_bytes())
            .await
            .map_err(|e| AppError::Internal(format!("dump {collection}: write: {e}")))?;
        writer
            .write_all(b"\n")
            .await
            .map_err(|e| AppError::Internal(format!("dump {collection}: write: {e}")))?;
        rows += 1;
    }
    Ok(rows)
}

/// Dump every allowlisted collection into `<dir>/<collection>.jsonl` and
/// write `<dir>/manifest.json`. Returns per-collection row counts so callers
/// can surface them in summaries / manifests.
pub async fn dump_all(
    db: &Database,
    dir: &Path,
) -> Result<Vec<(String, usize)>> {
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| AppError::Internal(format!("create dump dir {dir:?}: {e}")))?;

    let mut counts = Vec::with_capacity(COLLECTION_ALLOWLIST.len());
    for &name in COLLECTION_ALLOWLIST {
        let path = dir.join(format!("{name}.jsonl"));
        let file = File::create(&path)
            .await
            .map_err(|e| AppError::Internal(format!("open {path:?}: {e}")))?;
        let mut writer = BufWriter::new(file);
        let rows = dump_collection(db, name, &mut writer).await?;
        writer
            .flush()
            .await
            .map_err(|e| AppError::Internal(format!("flush {path:?}: {e}")))?;
        counts.push((name.to_string(), rows));
    }

    write_manifest(dir, &counts).await?;
    Ok(counts)
}

async fn write_manifest(dir: &Path, counts: &[(String, usize)]) -> Result<()> {
    let collections: Vec<Value> = counts
        .iter()
        .map(|(name, rows)| {
            json!({
                "name": name,
                "rows": rows,
                "schema_version": SCHEMA_VERSION,
            })
        })
        .collect();
    let manifest = json!({
        "schema_version": SCHEMA_VERSION,
        "collections": collections,
    });
    let path = dir.join("manifest.json");
    let body = serde_json::to_vec_pretty(&manifest)
        .map_err(|e| AppError::Internal(format!("serialise manifest: {e}")))?;
    tokio::fs::write(&path, body)
        .await
        .map_err(|e| AppError::Internal(format!("write manifest {path:?}: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bson::oid::ObjectId;

    #[test]
    fn objectid_roundtrips_via_ejson() {
        let oid = ObjectId::parse_str("507f1f77bcf86cd799439011").unwrap();
        let doc = bson::doc! { "_id": oid, "name": "alice" };
        let json = document_to_json(doc.clone());
        let back = json_to_document(json).unwrap();
        assert_eq!(back.get_object_id("_id").unwrap(), oid);
        assert_eq!(back.get_str("name").unwrap(), "alice");
    }

    #[test]
    fn datetime_roundtrips_via_ejson() {
        let dt = bson::DateTime::from_millis(1_730_000_000_000);
        let doc = bson::doc! { "at": dt };
        let json = document_to_json(doc);
        let back = json_to_document(json).unwrap();
        assert_eq!(back.get_datetime("at").unwrap(), &dt);
    }

    #[test]
    fn nested_document_roundtrips() {
        let oid = ObjectId::new();
        let doc = bson::doc! {
            "_id": oid,
            "name": "alice",
            "tags": ["a", "b"],
            "meta": { "active": true, "score": 42i64 },
        };
        let json = document_to_json(doc.clone());
        let back = json_to_document(json).unwrap();
        assert_eq!(back.get_object_id("_id").unwrap(), oid);
        assert_eq!(back.get_str("name").unwrap(), "alice");
        let tags = back.get_array("tags").unwrap();
        assert_eq!(tags.len(), 2);
        let meta = back.get_document("meta").unwrap();
        assert!(meta.get_bool("active").unwrap());
        assert_eq!(meta.get_i64("score").unwrap(), 42);
    }

    #[test]
    fn binary_roundtrips() {
        let b = bson::Binary {
            subtype: bson::spec::BinarySubtype::Generic,
            bytes: vec![0x00, 0x01, 0x02, 0xFF],
        };
        let doc = bson::doc! { "blob": Bson::Binary(b.clone()) };
        let json = document_to_json(doc);
        let back = json_to_document(json).unwrap();
        let restored = back.get_binary_generic("blob").unwrap();
        assert_eq!(restored, &b.bytes);
    }
}
