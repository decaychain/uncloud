use bson::serde_helpers::chrono_datetime_as_bson_datetime;
use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShareResourceType {
    File,
    Folder,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Share {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub token: String,
    pub resource_type: ShareResourceType,
    pub resource_id: ObjectId,
    pub owner_id: ObjectId,
    pub password_hash: Option<String>,
    #[serde(with = "crate::models::opt_dt")]
    pub expires_at: Option<DateTime<Utc>>,
    pub download_count: i64,
    pub max_downloads: Option<i64>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
}

impl Share {
    pub fn new(
        token: String,
        resource_type: ShareResourceType,
        resource_id: ObjectId,
        owner_id: ObjectId,
    ) -> Self {
        Self {
            id: ObjectId::new(),
            token,
            resource_type,
            resource_id,
            owner_id,
            password_hash: None,
            expires_at: None,
            download_count: 0,
            max_downloads: None,
            created_at: Utc::now(),
        }
    }

    pub fn is_expired(&self) -> bool {
        if let Some(expires_at) = self.expires_at {
            Utc::now() > expires_at
        } else {
            false
        }
    }

    pub fn has_downloads_remaining(&self) -> bool {
        match self.max_downloads {
            Some(max) => self.download_count < max,
            None => true,
        }
    }

    pub fn is_valid(&self) -> bool {
        !self.is_expired() && self.has_downloads_remaining()
    }
}
