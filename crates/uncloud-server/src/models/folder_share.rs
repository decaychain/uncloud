use bson::serde_helpers::chrono_datetime_as_bson_datetime;
use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};
use uncloud_common::{GalleryInclude, MusicInclude};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharePermissionModel {
    ReadOnly,
    ReadWrite,
    Admin,
}

impl From<uncloud_common::SharePermission> for SharePermissionModel {
    fn from(p: uncloud_common::SharePermission) -> Self {
        match p {
            uncloud_common::SharePermission::ReadOnly => Self::ReadOnly,
            uncloud_common::SharePermission::ReadWrite => Self::ReadWrite,
            uncloud_common::SharePermission::Admin => Self::Admin,
        }
    }
}

impl From<SharePermissionModel> for uncloud_common::SharePermission {
    fn from(p: SharePermissionModel) -> Self {
        match p {
            SharePermissionModel::ReadOnly => Self::ReadOnly,
            SharePermissionModel::ReadWrite => Self::ReadWrite,
            SharePermissionModel::Admin => Self::Admin,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderShare {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub folder_id: ObjectId,
    pub owner_id: ObjectId,
    pub grantee_id: ObjectId,
    pub permission: SharePermissionModel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount_parent_id: Option<ObjectId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount_name: Option<String>,
    #[serde(default)]
    pub music_include: MusicInclude,
    #[serde(default)]
    pub gallery_include: GalleryInclude,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub updated_at: DateTime<Utc>,
}
