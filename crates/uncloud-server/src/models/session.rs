use bson::serde_helpers::chrono_datetime_as_bson_datetime;
use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub token: String,
    pub user_id: ObjectId,
    pub user_agent: Option<String>,
    pub ip_address: Option<String>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub expires_at: DateTime<Utc>,
}

impl Session {
    pub fn new(
        token: String,
        user_id: ObjectId,
        duration_hours: u64,
        user_agent: Option<String>,
        ip_address: Option<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: ObjectId::new(),
            token,
            user_id,
            user_agent,
            ip_address,
            created_at: now,
            expires_at: now + chrono::Duration::hours(duration_hours as i64),
        }
    }

    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }
}
