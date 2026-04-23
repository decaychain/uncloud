pub mod user;
pub mod session;
pub mod file;
pub mod folder;
pub mod storage;
pub mod share;
pub mod playlist;
pub mod api_token;
pub mod s3_credential;
pub mod app;
pub mod webhook;
pub mod shopping;
pub mod invite;
pub mod totp_challenge;
pub mod folder_share;
pub mod user_preferences;
pub mod task;
pub mod sync_event;

pub use folder_share::*;
pub use user::*;
pub use session::*;
pub use file::*;
pub use folder::*;
pub use storage::*;
pub use share::*;
pub use playlist::*;
pub use api_token::*;
pub use s3_credential::*;
pub use app::*;
pub use webhook::*;
pub use shopping::*;
pub use invite::*;
pub use totp_challenge::*;
pub use user_preferences::*;
pub use task::*;
pub use sync_event::*;

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
