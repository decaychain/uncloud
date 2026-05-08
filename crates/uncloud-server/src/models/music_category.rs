use bson::serde_helpers::chrono_datetime_as_bson_datetime;
use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MusicCategory {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub name: String,
    #[serde(default)]
    pub folder_ids: Vec<ObjectId>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub updated_at: DateTime<Utc>,
}

impl MusicCategory {
    pub fn new(owner_id: ObjectId, name: String, folder_ids: Vec<ObjectId>) -> Self {
        let now = Utc::now();
        Self {
            id: ObjectId::new(),
            owner_id,
            name,
            folder_ids,
            created_at: now,
            updated_at: now,
        }
    }
}
