use bson::serde_helpers::chrono_datetime_as_bson_datetime;
use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

pub use uncloud_common::{SyncClientOs, SyncEventSource, SyncOperation, SyncResourceType};
use uncloud_common::SyncEventResponse;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncEvent {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub timestamp: DateTime<Utc>,
    pub operation: SyncOperation,
    pub resource_type: SyncResourceType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<ObjectId>,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_path: Option<String>,
    pub source: SyncEventSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_os: Option<SyncClientOs>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub affected_count: Option<u32>,
}

impl From<&SyncEvent> for SyncEventResponse {
    fn from(e: &SyncEvent) -> Self {
        SyncEventResponse {
            id: e.id.to_hex(),
            timestamp: e.timestamp,
            operation: e.operation,
            resource_type: e.resource_type,
            resource_id: e.resource_id.map(|id| id.to_hex()),
            path: e.path.clone(),
            new_path: e.new_path.clone(),
            source: e.source,
            client_id: e.client_id.clone(),
            client_os: e.client_os,
            affected_count: e.affected_count,
        }
    }
}
