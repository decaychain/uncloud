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
    /// Pin this folder's contents to a specific storage. `None` (the default)
    /// means inherit from the closest ancestor that pins a storage; the root
    /// falls back to the configured default storage. Set at create time only —
    /// changing it on an existing folder is unsupported (would imply migration).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_id: Option<ObjectId>,
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
        Self::new_with_storage(owner_id, parent_id, name, None)
    }

    pub fn new_with_storage(
        owner_id: ObjectId,
        parent_id: Option<ObjectId>,
        name: String,
        storage_id: Option<ObjectId>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: ObjectId::new(),
            owner_id,
            parent_id,
            name,
            storage_id,
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
