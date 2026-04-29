use bson::serde_helpers::chrono_datetime_as_bson_datetime;
use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

/// Pinned SSH host public key for a single SFTP storage. Populated on first
/// connect (TOFU) and verified on every subsequent connect. One row per
/// `storage_id` — uniqueness is enforced by an index on `storage_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SftpHostKey {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub storage_id: ObjectId,
    /// Algorithm name, e.g. `ssh-ed25519`, `ssh-rsa`, `ecdsa-sha2-nistp256`.
    pub key_type: String,
    /// Base64-encoded SSH public key blob (no algorithm prefix).
    pub key_blob_base64: String,
    /// SHA-256 fingerprint of the key blob, hex-encoded — handy for diagnostics.
    pub fingerprint_sha256: String,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub first_seen_at: DateTime<Utc>,
}
