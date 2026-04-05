use bson::serde_helpers::chrono_datetime_as_bson_datetime;
use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

/// Short-lived challenge token issued after password verification when TOTP is enabled.
/// The client must call POST /api/auth/totp/verify with the token and a TOTP code
/// to complete login and receive a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TotpChallenge {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub token: String,
    pub user_id: ObjectId,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub expires_at: DateTime<Utc>,
}

impl TotpChallenge {
    pub fn new(token: String, user_id: ObjectId) -> Self {
        let now = Utc::now();
        Self {
            id: ObjectId::new(),
            token,
            user_id,
            created_at: now,
            expires_at: now + chrono::Duration::minutes(5),
        }
    }

    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }
}
