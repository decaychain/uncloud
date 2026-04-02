use bson::serde_helpers::chrono_datetime_as_bson_datetime;
use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};
use uncloud_common::{GalleryInclude, MusicInclude, SyncStrategy};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Folder {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub parent_id: Option<ObjectId>,
    pub name: String,
    /// Missing BSON field deserialises to `Inherit` (the default) — no migration needed.
    #[serde(default)]
    pub sync_strategy: SyncStrategy,
    /// Missing BSON field deserialises to `Inherit` (the default) — no migration needed.
    #[serde(default)]
    pub gallery_include: GalleryInclude,
    /// Missing BSON field deserialises to `Inherit` (the default) — no migration needed.
    #[serde(default)]
    pub music_include: MusicInclude,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub updated_at: DateTime<Utc>,
    #[serde(default, with = "super::opt_dt")]
    pub deleted_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_delete_id: Option<String>,
}

impl Folder {
    pub fn new(owner_id: ObjectId, parent_id: Option<ObjectId>, name: String) -> Self {
        let now = Utc::now();
        Self {
            id: ObjectId::new(),
            owner_id,
            parent_id,
            name,
            sync_strategy: SyncStrategy::default(),
            gallery_include: GalleryInclude::default(),
            music_include: MusicInclude::default(),
            created_at: now,
            updated_at: now,
            deleted_at: None,
            batch_delete_id: None,
        }
    }
}
