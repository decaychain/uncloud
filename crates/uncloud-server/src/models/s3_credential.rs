use bson::serde_helpers::chrono_datetime_as_bson_datetime;
use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3Credential {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub user_id: ObjectId,
    pub access_key_id: String,
    /// The raw secret_access_key — stored in plaintext because AWS SigV4 verification
    /// requires the raw secret to derive HMAC signing keys. This is the same approach
    /// used by AWS, MinIO, Garage, and other S3-compatible services.
    pub secret_access_key: String,
    pub label: String,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
}

impl S3Credential {
    pub fn new(
        user_id: ObjectId,
        access_key_id: String,
        secret_access_key: String,
        label: String,
    ) -> Self {
        Self {
            id: ObjectId::new(),
            user_id,
            access_key_id,
            secret_access_key,
            label,
            created_at: Utc::now(),
        }
    }
}
