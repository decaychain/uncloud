use bson::serde_helpers::chrono_datetime_as_bson_datetime;
use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthClient {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub client_id: String,
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    pub allowed_scopes: Vec<String>,
    pub dynamically_registered: bool,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
}

impl OAuthClient {
    pub fn new(
        client_id: String,
        client_name: String,
        redirect_uris: Vec<String>,
        allowed_scopes: Vec<String>,
        dynamically_registered: bool,
    ) -> Self {
        Self {
            id: ObjectId::new(),
            client_id,
            client_name,
            redirect_uris,
            allowed_scopes,
            dynamically_registered,
            created_at: Utc::now(),
        }
    }
}
