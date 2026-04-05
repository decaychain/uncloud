use bson::serde_helpers::chrono_datetime_as_bson_datetime;
use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

use super::UserRole;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invite {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub token: String,
    pub created_by: ObjectId,
    #[serde(default)]
    pub comment: Option<String>,
    /// Legacy field — kept for backward compat with existing DB docs.
    #[serde(default)]
    pub email: Option<String>,
    pub role: Option<UserRole>,
    #[serde(default, with = "super::opt_dt")]
    pub expires_at: Option<DateTime<Utc>>,
    pub used_by: Option<ObjectId>,
    #[serde(default, with = "super::opt_dt")]
    pub used_at: Option<DateTime<Utc>>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
}

impl Invite {
    pub fn new(
        token: String,
        created_by: ObjectId,
        comment: Option<String>,
        role: Option<UserRole>,
        expires_at: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            id: ObjectId::new(),
            token,
            created_by,
            comment,
            email: None,
            role,
            expires_at,
            used_by: None,
            used_at: None,
            created_at: Utc::now(),
        }
    }

    pub fn is_valid(&self) -> bool {
        if self.used_by.is_some() {
            return false;
        }
        if let Some(exp) = self.expires_at {
            if Utc::now() > exp {
                return false;
            }
        }
        true
    }
}
