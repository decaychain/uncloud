use bson::serde_helpers::chrono_datetime_as_bson_datetime;
use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthAuthorizationCode {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub code_hash: String,
    pub client_id: String,
    pub user_id: ObjectId,
    pub scopes: Vec<String>,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub expires_at: DateTime<Utc>,
    #[serde(default)]
    pub consumed: bool,
}
