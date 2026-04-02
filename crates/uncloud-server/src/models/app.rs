use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct App {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub name: String,
    pub nav_label: String,
    pub icon: String,
    pub base_url: String,
    #[serde(default)]
    pub enabled_for: Vec<ObjectId>,
    #[serde(with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
}
