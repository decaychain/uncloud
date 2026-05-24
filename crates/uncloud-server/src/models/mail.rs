use bson::serde_helpers::chrono_datetime_as_bson_datetime;
use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};
use uncloud_common::MailSecurity;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailServerConfig {
    pub host: String,
    pub port: u16,
    pub security: MailSecurity,
    pub username: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedMailCredential {
    pub version: u8,
    pub algorithm: String,
    pub nonce: String,
    pub ciphertext: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailAccount {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub display_name: String,
    pub email_address: String,
    pub imap: MailServerConfig,
    pub smtp: MailServerConfig,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub sync_enabled: bool,
    #[serde(default)]
    pub credential_configured: bool,
    #[serde(default)]
    pub credential: Option<EncryptedMailCredential>,
    #[serde(default, with = "crate::models::opt_dt")]
    pub last_sync_at: Option<DateTime<Utc>>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailIdentity {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub account_id: ObjectId,
    pub display_name: String,
    pub email_address: String,
    #[serde(default)]
    pub reply_to: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default)]
    pub is_default: bool,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailFolder {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub account_id: ObjectId,
    pub path: String,
    pub name: String,
    #[serde(default)]
    pub delimiter: Option<String>,
    #[serde(default)]
    pub parent_path: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default = "default_true")]
    pub selectable: bool,
    #[serde(default)]
    pub attributes: Vec<String>,
    #[serde(default)]
    pub uid_validity: Option<u32>,
    #[serde(default)]
    pub uid_next: Option<u32>,
    #[serde(default)]
    pub exists: Option<u32>,
    #[serde(default)]
    pub unseen: Option<u32>,
    #[serde(default)]
    pub highest_synced_uid: Option<u32>,
    #[serde(default)]
    pub lowest_synced_uid: Option<u32>,
    #[serde(default, with = "crate::models::opt_dt")]
    pub last_sync_started_at: Option<DateTime<Utc>>,
    #[serde(default, with = "crate::models::opt_dt")]
    pub last_sync_finished_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_sync_error: Option<String>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailAddress {
    #[serde(default)]
    pub name: Option<String>,
    pub address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailMessage {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub account_id: ObjectId,
    pub folder_id: ObjectId,
    pub folder_path: String,
    pub uid: u32,
    #[serde(default)]
    pub message_id: Option<String>,
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub in_reply_to: Option<String>,
    #[serde(default)]
    pub references: Vec<String>,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub from: Vec<MailAddress>,
    #[serde(default)]
    pub to: Vec<MailAddress>,
    #[serde(default)]
    pub cc: Vec<MailAddress>,
    #[serde(default)]
    pub bcc: Vec<MailAddress>,
    #[serde(default, with = "crate::models::opt_dt")]
    pub date: Option<DateTime<Utc>>,
    #[serde(default, with = "crate::models::opt_dt")]
    pub internal_date: Option<DateTime<Utc>>,
    #[serde(default)]
    pub flags: Vec<String>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    #[serde(default)]
    pub has_attachments: bool,
    #[serde(default)]
    pub snippet: Option<String>,
    #[serde(default)]
    pub raw_storage_path: Option<String>,
    #[serde(default)]
    pub text_storage_path: Option<String>,
    #[serde(default)]
    pub html_storage_path: Option<String>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailAttachment {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub account_id: ObjectId,
    pub message_id: ObjectId,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub content_id: Option<String>,
    #[serde(default)]
    pub disposition: Option<String>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    #[serde(default)]
    pub storage_path: Option<String>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
}

fn default_true() -> bool {
    true
}
