use bson::serde_helpers::chrono_datetime_as_bson_datetime;
use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiToken {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub user_id: ObjectId,
    pub name: String,
    /// SHA-256 hash of the actual bearer token
    pub token_hash: String,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,

    // OAuth-issued tokens populate these. Legacy PATs leave them None and
    // get resolved as full-access bearers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Vec<String>>,
    #[serde(
        default,
        with = "crate::models::opt_dt",
        skip_serializing_if = "Option::is_none"
    )]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token_hash: Option<String>,
}

impl ApiToken {
    pub fn new(user_id: ObjectId, name: String, token_hash: String) -> Self {
        Self {
            id: ObjectId::new(),
            user_id,
            name,
            token_hash,
            created_at: Utc::now(),
            client_id: None,
            scopes: None,
            expires_at: None,
            refresh_token_hash: None,
        }
    }

    pub fn new_oauth(
        user_id: ObjectId,
        client_id: String,
        client_name: String,
        token_hash: String,
        scopes: Vec<String>,
        expires_at: DateTime<Utc>,
        refresh_token_hash: Option<String>,
    ) -> Self {
        Self {
            id: ObjectId::new(),
            user_id,
            name: client_name,
            token_hash,
            created_at: Utc::now(),
            client_id: Some(client_id),
            scopes: Some(scopes),
            expires_at: Some(expires_at),
            refresh_token_hash,
        }
    }

    pub fn is_expired(&self) -> bool {
        match self.expires_at {
            Some(exp) => exp <= Utc::now(),
            None => false,
        }
    }
}
