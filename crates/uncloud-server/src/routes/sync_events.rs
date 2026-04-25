use std::sync::Arc;

use axum::{
    extract::{Query, State},
    Json,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use uncloud_common::{SyncEventListResponse, SyncEventResponse, SyncEventSource};

use crate::AppState;
use crate::error::Result;
use crate::middleware::AuthUser;
use crate::services::sync_log::SyncEventFilter;

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub client: Option<String>,
    /// Comma-separated list; repeating the query param also accumulates.
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub before: Option<DateTime<Utc>>,
    #[serde(default)]
    pub limit: Option<u32>,
}

pub async fn list_sync_events(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(q): Query<ListQuery>,
) -> Result<Json<SyncEventListResponse>> {
    let sources = q.source.as_deref().map(parse_sources).filter(|v: &Vec<SyncEventSource>| !v.is_empty());

    let filter = SyncEventFilter {
        q: q.q,
        client: q.client,
        source: sources,
        before: q.before,
        limit: q.limit.unwrap_or(0),
    };

    let (events, has_more) = state.sync_log.list(user.id, filter).await?;
    let body = SyncEventListResponse {
        events: events.iter().map(SyncEventResponse::from).collect(),
        has_more,
    };
    Ok(Json(body))
}

fn parse_sources(raw: &str) -> Vec<SyncEventSource> {
    raw.split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .filter_map(|s| match s.to_ascii_lowercase().as_str() {
            "sync" => Some(SyncEventSource::Sync),
            "user" | "user_web" | "web" => Some(SyncEventSource::UserWeb),
            "user_desktop" | "desktop" => Some(SyncEventSource::UserDesktop),
            "user_mobile" | "mobile" => Some(SyncEventSource::UserMobile),
            "admin" => Some(SyncEventSource::Admin),
            "public" => Some(SyncEventSource::Public),
            "system" => Some(SyncEventSource::System),
            _ => None,
        })
        .collect()
}
