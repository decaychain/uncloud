pub mod api_token;
pub mod app;
pub mod backup_lock;
pub mod file;
pub mod finance;
pub mod folder;
pub mod folder_share;
pub mod invite;
pub mod mail;
pub mod migration_lock;
pub mod music_category;
pub mod oauth_authorization_code;
pub mod oauth_client;
pub mod playlist;
pub mod s3_credential;
pub mod session;
pub mod sftp_host_key;
pub mod share;
pub mod shopping;
pub mod storage;
pub mod subsonic;
pub mod sync_event;
pub mod task;
pub mod totp_challenge;
pub mod user;
pub mod user_preferences;
pub mod webhook;

pub use api_token::*;
pub use app::*;
pub use backup_lock::*;
pub use file::*;
pub use finance::*;
pub use folder::*;
pub use folder_share::*;
pub use invite::*;
pub use mail::*;
pub use migration_lock::*;
pub use music_category::*;
pub use oauth_authorization_code::*;
pub use oauth_client::*;
pub use playlist::*;
pub use s3_credential::*;
pub use session::*;
pub use sftp_host_key::*;
pub use share::*;
pub use shopping::*;
pub use storage::*;
pub use subsonic::*;
pub use sync_event::*;
pub use task::*;
pub use totp_challenge::*;
pub use user::*;
pub use user_preferences::*;
pub use webhook::*;

/// Serde module for `Option<chrono::DateTime<Utc>>` ↔ BSON Date (nullable).
/// Usage: `#[serde(with = "crate::models::opt_dt")]`
pub(crate) mod opt_dt {
    use bson::serde_helpers::chrono_datetime_as_bson_datetime;
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(val: &Option<DateTime<Utc>>, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match val {
            Some(dt) => chrono_datetime_as_bson_datetime::serialize(dt, s),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(d: D) -> Result<Option<DateTime<Utc>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt = Option::<bson::DateTime>::deserialize(d)?;
        Ok(opt.map(|dt| dt.to_chrono()))
    }
}

/// Serde module for `Vec<chrono::DateTime<Utc>>` ↔ BSON array of Dates.
/// `chrono_datetime_as_bson_datetime` is element-only; this module loops it
/// over the vec so each entry is stored as a real BSON Date (queryable,
/// sortable, no string parsing on read).
///
/// Usage: `#[serde(default, with = "crate::models::dt_vec")]`
pub(crate) mod dt_vec {
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(val: &[DateTime<Utc>], s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let bson_dts: Vec<bson::DateTime> = val
            .iter()
            .map(|dt| bson::DateTime::from_chrono(*dt))
            .collect();
        bson_dts.serialize(s)
    }

    pub fn deserialize<'de, D>(d: D) -> Result<Vec<DateTime<Utc>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let vec: Vec<bson::DateTime> = Vec::deserialize(d)?;
        Ok(vec.into_iter().map(|dt| dt.to_chrono()).collect())
    }
}
