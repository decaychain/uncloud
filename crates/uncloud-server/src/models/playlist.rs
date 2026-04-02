use bson::serde_helpers::chrono_datetime_as_bson_datetime;
use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistTrack {
    pub file_id: ObjectId,
    pub position: u32,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub added_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playlist {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub tracks: Vec<PlaylistTrack>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub updated_at: DateTime<Utc>,
}

impl Playlist {
    pub fn new(owner_id: ObjectId, name: String) -> Self {
        let now = Utc::now();
        Self {
            id: ObjectId::new(),
            owner_id,
            name,
            description: None,
            tracks: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }
}
