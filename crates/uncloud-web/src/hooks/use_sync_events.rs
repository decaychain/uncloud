use chrono::{DateTime, Utc};
use uncloud_common::{SyncEventListResponse, SyncEventSource};

use super::api;

#[derive(Debug, Clone, Default)]
pub struct SyncEventsFilter {
    pub q: String,
    pub client: String,
    pub sources: Vec<SyncEventSource>,
    pub before: Option<DateTime<Utc>>,
    pub limit: u32,
}

pub fn source_code(s: SyncEventSource) -> &'static str {
    match s {
        SyncEventSource::UserWeb => "user_web",
        SyncEventSource::UserDesktop => "user_desktop",
        SyncEventSource::UserMobile => "user_mobile",
        SyncEventSource::Sync => "sync",
        SyncEventSource::Admin => "admin",
        SyncEventSource::Public => "public",
        SyncEventSource::System => "system",
    }
}

pub async fn list_sync_events(filter: SyncEventsFilter) -> Result<SyncEventListResponse, String> {
    let mut parts = Vec::<String>::new();

    if !filter.q.is_empty() {
        parts.push(format!(
            "q={}",
            js_sys::encode_uri_component(&filter.q)
        ));
    }
    if !filter.client.is_empty() {
        parts.push(format!(
            "client={}",
            js_sys::encode_uri_component(&filter.client)
        ));
    }
    if !filter.sources.is_empty() {
        let csv = filter
            .sources
            .iter()
            .copied()
            .map(source_code)
            .collect::<Vec<_>>()
            .join(",");
        parts.push(format!(
            "source={}",
            js_sys::encode_uri_component(&csv)
        ));
    }
    if let Some(before) = filter.before {
        parts.push(format!(
            "before={}",
            js_sys::encode_uri_component(&before.to_rfc3339())
        ));
    }
    if filter.limit > 0 {
        parts.push(format!("limit={}", filter.limit));
    }

    let query = if parts.is_empty() {
        String::new()
    } else {
        format!("?{}", parts.join("&"))
    };
    let url = format!("{}/sync-events{}", api::api_url(""), query);

    let response = api::get_raw(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<SyncEventListResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(format!(
            "Failed to load activity log ({})",
            response.status()
        ))
    }
}
