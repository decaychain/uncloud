use bson::serde_helpers::chrono_datetime_as_bson_datetime;
use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedSubsonicCredential {
    pub version: u8,
    pub algorithm: String,
    pub nonce: String,
    pub ciphertext: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubsonicCredential {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub label: String,
    pub credential: EncryptedSubsonicCredential,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(default, with = "crate::models::opt_dt")]
    pub last_used_at: Option<DateTime<Utc>>,
}

impl SubsonicCredential {
    pub fn new(owner_id: ObjectId, label: String, credential: EncryptedSubsonicCredential) -> Self {
        Self {
            id: ObjectId::new(),
            owner_id,
            label,
            credential,
            created_at: Utc::now(),
            last_used_at: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubsonicIdKind {
    Folder,
    Song,
    Artist,
    Album,
    Playlist,
}

impl SubsonicIdKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Folder => "folder",
            Self::Song => "song",
            Self::Artist => "artist",
            Self::Album => "album",
            Self::Playlist => "playlist",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubsonicId {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub numeric_id: i64,
    pub kind: String,
    pub internal_key: String,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
}

impl SubsonicId {
    pub fn new(
        owner_id: ObjectId,
        numeric_id: i64,
        kind: SubsonicIdKind,
        internal_key: String,
    ) -> Self {
        Self {
            id: ObjectId::new(),
            owner_id,
            numeric_id,
            kind: kind.as_str().to_string(),
            internal_key,
            created_at: Utc::now(),
        }
    }
}
