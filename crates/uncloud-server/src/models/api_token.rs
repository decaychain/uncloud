use bson::serde_helpers::chrono_datetime_as_bson_datetime;
use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiToken {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub user_id: ObjectId,
    pub name: String,
    /// SHA-256 hash of the actual bearer token
    pub token_hash: String,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
}

impl ApiToken {
    pub fn new(user_id: ObjectId, name: String, token_hash: String) -> Self {
        Self {
            id: ObjectId::new(),
            user_id,
            name,
            token_hash,
            created_at: Utc::now(),
        }
    }
}
