use bson::serde_helpers::chrono_datetime_as_bson_datetime;
use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentVault {
    pub file_id: ObjectId,
    pub file_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_path: Option<String>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub last_opened_at: DateTime<Utc>,
}

/// MongoDB document stored in the `user_preferences` collection, keyed by `user_id`.
/// Holds the per-user "recent vaults" list for the passwords feature.
/// Distinct from `uncloud_common::UserPreferences`, which is the embedded
/// UI-preferences struct on the `User` document itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultRecentsDoc {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub user_id: ObjectId,
    #[serde(default)]
    pub recent_vaults: Vec<RecentVault>,
}
