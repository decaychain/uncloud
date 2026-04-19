use bson::serde_helpers::chrono_datetime_as_bson_datetime;
use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

pub use uncloud_common::{UserPreferences, UserStatus};

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
    #[serde(default)]
    pub email: Option<String>,
    pub password_hash: String,
    #[serde(default)]
    pub role: UserRole,
    #[serde(default)]
    pub status: UserStatus,
    pub quota_bytes: Option<i64>,
    #[serde(default)]
    pub used_bytes: i64,
    #[serde(default)]
    pub disabled_features: Vec<String>,
    /// Base32-encoded TOTP secret (set during setup, cleared on disable).
    #[serde(default)]
    pub totp_secret: Option<String>,
    #[serde(default)]
    pub totp_enabled: bool,
    /// Argon2-hashed one-time recovery codes.
    #[serde(default)]
    pub recovery_codes: Vec<String>,
    /// True for ephemeral demo accounts (auto-purged).
    #[serde(default)]
    pub demo: bool,
    /// Per-user UI preferences (dashboard tiles, etc.).
    #[serde(default)]
    pub preferences: UserPreferences,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub updated_at: DateTime<Utc>,
}

impl User {
    pub fn new(username: String, email: Option<String>, password_hash: String) -> Self {
        let now = Utc::now();
        Self {
            id: ObjectId::new(),
            username,
            email,
            password_hash,
            role: UserRole::User,
            status: UserStatus::Active,
            quota_bytes: None,
            used_bytes: 0,
            disabled_features: Vec::new(),
            totp_secret: None,
            totp_enabled: false,
            recovery_codes: Vec::new(),
            demo: false,
            preferences: UserPreferences::default(),
            created_at: now,
            updated_at: now,
        }
    }

    pub fn is_admin(&self) -> bool {
        self.role == UserRole::Admin
    }

    pub fn is_active(&self) -> bool {
        self.status == UserStatus::Active
    }

    pub fn has_quota_space(&self, bytes: i64) -> bool {
        match self.quota_bytes {
            Some(quota) => self.used_bytes + bytes <= quota,
            None => true,
        }
    }
}
