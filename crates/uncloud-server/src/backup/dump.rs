//! Semantic database dump — BSON Document → portable JSON Lines.
//!
//! The aim is engine-agnosticism: a future SQLite/Postgres-backed Uncloud
//! must be able to ingest these files without any Mongo-specific extended
//! JSON. Each row is one self-contained JSON object on its own line.
//!
//! Type mapping:
//!
//! | BSON               | JSON                                |
//! |--------------------|-------------------------------------|
//! | ObjectId           | 24-char hex string                  |
//! | DateTime           | RFC 3339 string (UTC)               |
//! | Binary             | `{ "$binary": "<base64>" }`         |
//! | Decimal128         | string                              |
//! | Int32 / Int64      | JSON number                         |
//! | Double             | JSON number (NaN / ±∞ → null)       |
//! | Boolean / String   | as-is                               |
//! | Null / Undefined   | null                                |
//! | Document           | recursed                            |
//! | Array              | recursed                            |
//! | RegEx              | `{ "$regex": "...", "$options": "..." }` |
//! | Timestamp          | `{ "$timestamp": { "t": ..., "i": ... } }` |
//! | DbPointer / JavaScriptCode / Symbol / MaxKey / MinKey / DbRef | best-effort string forms; we don't write these in our schema, surfaced only so a stray document doesn't crash the dumper |

use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use bson::{Bson, Document};
use chrono::{SecondsFormat, TimeZone, Utc};
use futures::stream::TryStreamExt;
use mongodb::Database;
use serde_json::{json, Map, Value};
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

/// Convert a BSON Document into a portable JSON Value (always an object).
pub fn document_to_json(doc: Document) -> Value {
    let mut map = Map::with_capacity(doc.len());
    for (key, value) in doc {
        map.insert(key, bson_to_json(value));
    }
    Value::Object(map)
}

/// Convert any BSON value into a portable JSON value.
pub fn bson_to_json(value: Bson) -> Value {
    match value {
        Bson::Null | Bson::Undefined => Value::Null,
        Bson::Boolean(b) => Value::Bool(b),
        Bson::String(s) => Value::String(s),
        Bson::Int32(n) => Value::from(n),
        Bson::Int64(n) => Value::from(n),
        Bson::Double(n) => {
            // NaN / ±∞ are not representable in JSON. Project to null rather
            // than crashing — these never appear in our domain models, but
            // we don't want a stray document in the wild to break a backup.
            if n.is_finite() {
                Value::from(n)
            } else {
                Value::Null
            }
        }
        Bson::ObjectId(oid) => Value::String(oid.to_hex()),
        Bson::DateTime(dt) => {
            // RFC 3339 with millisecond precision, always Z-suffixed.
            let secs = dt.timestamp_millis() / 1_000;
            let nanos = ((dt.timestamp_millis() % 1_000) * 1_000_000) as u32;
            let chrono = Utc
                .timestamp_opt(secs, nanos)
                .single()
                .unwrap_or_else(Utc::now);
            Value::String(chrono.to_rfc3339_opts(SecondsFormat::Millis, true))
        }
        Bson::Binary(bin) => json!({
            "$binary": BASE64.encode(&bin.bytes),
        }),
        Bson::Decimal128(d) => Value::String(d.to_string()),
        Bson::Document(d) => document_to_json(d),
        Bson::Array(arr) => Value::Array(arr.into_iter().map(bson_to_json).collect()),
        Bson::RegularExpression(re) => json!({
            "$regex": re.pattern,
            "$options": re.options,
        }),
        Bson::Timestamp(ts) => json!({
            "$timestamp": { "t": ts.time, "i": ts.increment },
        }),
        // Best-effort fallbacks — none appear in our domain models.
        Bson::JavaScriptCode(s) => json!({ "$code": s }),
        Bson::JavaScriptCodeWithScope(c) => json!({
            "$code": c.code,
            "$scope": document_to_json(c.scope),
        }),
        Bson::Symbol(s) => Value::String(s),
        Bson::DbPointer(_) => Value::Null,
        Bson::MaxKey => json!({ "$maxKey": 1 }),
        Bson::MinKey => json!({ "$minKey": 1 }),
    }
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
    fn primitive_mapping() {
        assert_eq!(bson_to_json(Bson::Null), Value::Null);
        assert_eq!(bson_to_json(Bson::Boolean(true)), Value::Bool(true));
        assert_eq!(bson_to_json(Bson::Int32(7)), Value::from(7));
        assert_eq!(bson_to_json(Bson::Int64(1 << 40)), Value::from(1i64 << 40));
        assert_eq!(bson_to_json(Bson::String("x".into())), Value::String("x".into()));
    }

    #[test]
    fn objectid_to_hex_string() {
        let oid = ObjectId::parse_str("507f1f77bcf86cd799439011").unwrap();
        let v = bson_to_json(Bson::ObjectId(oid));
        assert_eq!(v, Value::String("507f1f77bcf86cd799439011".into()));
    }

    #[test]
    fn datetime_to_rfc3339() {
        let dt = bson::DateTime::from_chrono(
            chrono::DateTime::parse_from_rfc3339("2026-05-01T12:34:56.789Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        let v = bson_to_json(Bson::DateTime(dt));
        assert_eq!(v, Value::String("2026-05-01T12:34:56.789Z".into()));
    }

    #[test]
    fn binary_base64_envelope() {
        let bytes = vec![0x00, 0x01, 0x02, 0xFF];
        let b = bson::Binary {
            subtype: bson::spec::BinarySubtype::Generic,
            bytes,
        };
        let v = bson_to_json(Bson::Binary(b));
        assert_eq!(v, json!({ "$binary": "AAEC/w==" }));
    }

    #[test]
    fn nested_document_roundtrips() {
        let oid = ObjectId::new();
        let doc = bson::doc! {
            "_id": oid,
            "name": "alice",
            "tags": ["a", "b"],
            "meta": { "active": true, "score": 42 },
        };
        let v = document_to_json(doc);
        assert_eq!(v["_id"], Value::String(oid.to_hex()));
        assert_eq!(v["name"], Value::String("alice".into()));
        assert_eq!(v["tags"], json!(["a", "b"]));
        assert_eq!(v["meta"]["active"], Value::Bool(true));
        assert_eq!(v["meta"]["score"], Value::from(42));
    }

    #[test]
    fn nan_and_infinity_become_null() {
        assert_eq!(bson_to_json(Bson::Double(f64::NAN)), Value::Null);
        assert_eq!(bson_to_json(Bson::Double(f64::INFINITY)), Value::Null);
        assert_eq!(bson_to_json(Bson::Double(2.5)), Value::from(2.5));
    }

    #[test]
    fn no_extended_json_envelopes_for_oid_or_date() {
        // Make sure we did not accidentally fall back to mongodb's canonical
        // EJSON form, which would break the engine-agnostic contract.
        let oid = ObjectId::new();
        let dt = bson::DateTime::now();
        let doc = bson::doc! { "id": oid, "at": dt };
        let v = document_to_json(doc);
        let s = serde_json::to_string(&v).unwrap();
        assert!(!s.contains("$oid"), "found EJSON $oid envelope: {s}");
        assert!(!s.contains("$date"), "found EJSON $date envelope: {s}");
    }
}
