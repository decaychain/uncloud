use bson::serde_helpers::chrono_datetime_as_bson_datetime;
use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    Admin,
    User,
}

impl Default for UserRole {
    fn default() -> Self {
        Self::User
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub username: String,
    pub email: String,
    pub password_hash: String,
    #[serde(default)]
    pub role: UserRole,
    pub quota_bytes: Option<i64>,
    #[serde(default)]
    pub used_bytes: i64,
    #[serde(default)]
    pub disabled_features: Vec<String>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub updated_at: DateTime<Utc>,
}

impl User {
    pub fn new(username: String, email: String, password_hash: String) -> Self {
        let now = Utc::now();
        Self {
            id: ObjectId::new(),
            username,
            email,
            password_hash,
            role: UserRole::User,
            quota_bytes: None,
            used_bytes: 0,
            disabled_features: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }

    pub fn is_admin(&self) -> bool {
        self.role == UserRole::Admin
    }

    pub fn has_quota_space(&self, bytes: i64) -> bool {
        match self.quota_bytes {
            Some(quota) => self.used_bytes + bytes <= quota,
            None => true,
        }
    }
}
