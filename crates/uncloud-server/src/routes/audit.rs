//! Thin builders for [`SyncEvent`] used by every mutating route handler.
//!
//! Keeps emission sites short and consistent — each route call becomes:
//! ```ignore
//! state.sync_log.record(audit::file_event(
//!     owner_id, SyncOperation::Renamed, file.id,
//!     old_path, Some(new_path), &meta,
//! )).await;
//! ```

use mongodb::bson::oid::ObjectId;
use uncloud_common::{SyncOperation, SyncResourceType};

use crate::middleware::RequestMeta;
use crate::models::SyncEvent;

pub fn file_event(
    owner_id: ObjectId,
    operation: SyncOperation,
    file_id: ObjectId,
    path: impl Into<String>,
    new_path: Option<String>,
    meta: &RequestMeta,
) -> SyncEvent {
    SyncEvent {
        id: ObjectId::new(),
        owner_id,
        timestamp: chrono::Utc::now(),
        operation,
        resource_type: SyncResourceType::File,
        resource_id: Some(file_id),
        path: path.into(),
        new_path,
        source: meta.source,
        client_id: meta.client_id.clone(),
        client_os: meta.client_os,
        affected_count: None,
    }
}

pub fn folder_event(
    owner_id: ObjectId,
    operation: SyncOperation,
    folder_id: ObjectId,
    path: impl Into<String>,
    new_path: Option<String>,
    affected_count: Option<u32>,
    meta: &RequestMeta,
) -> SyncEvent {
    SyncEvent {
        id: ObjectId::new(),
        owner_id,
        timestamp: chrono::Utc::now(),
        operation,
        resource_type: SyncResourceType::Folder,
        resource_id: Some(folder_id),
        path: path.into(),
        new_path,
        source: meta.source,
        client_id: meta.client_id.clone(),
        client_os: meta.client_os,
        affected_count,
    }
}

/// For summary events that cover many resources (empty trash, mass purge) where
/// the individual resource IDs are not useful. `path` is a human-readable label.
pub fn summary_event(
    owner_id: ObjectId,
    operation: SyncOperation,
    resource_type: SyncResourceType,
    path: impl Into<String>,
    affected_count: u32,
    meta: &RequestMeta,
) -> SyncEvent {
    SyncEvent {
        id: ObjectId::new(),
        owner_id,
        timestamp: chrono::Utc::now(),
        operation,
        resource_type,
        resource_id: None,
        path: path.into(),
        new_path: None,
        source: meta.source,
        client_id: meta.client_id.clone(),
        client_os: meta.client_os,
        affected_count: Some(affected_count),
    }
}
