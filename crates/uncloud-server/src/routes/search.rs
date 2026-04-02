use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::error::{AppError, Result};
use crate::middleware::AuthUser;
use crate::models::file::TaskType;
use crate::AppState;

#[derive(Serialize)]
pub struct SearchStatus {
    pub enabled: bool,
}

pub async fn search_status(
    State(state): State<Arc<AppState>>,
    _user: AuthUser,
) -> Json<SearchStatus> {
    Json(SearchStatus { enabled: state.search.is_enabled() })
}

#[derive(Deserialize)]
pub struct SearchParams {
    pub q: String,
    pub limit: Option<usize>,
}

/// Admin-only: strip stale search_index tasks and re-queue all files.
/// Needed when search was previously disabled and files were incorrectly
/// marked as Done without actually being indexed.
pub async fn reindex(
    State(state): State<Arc<AppState>>,
    _user: AuthUser,
) -> StatusCode {
    if !state.search.is_enabled() {
        return StatusCode::SERVICE_UNAVAILABLE;
    }
    let state_clone = state.clone();
    tokio::spawn(async move {
        state_clone
            .processing
            .reindex_task_type(&TaskType::SearchIndex, state_clone.clone())
            .await;
    });
    StatusCode::ACCEPTED
}

pub async fn search_files(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(params): Query<SearchParams>,
) -> Result<Json<Vec<uncloud_common::SearchHit>>> {
    if params.q.trim().is_empty() {
        return Ok(Json(vec![]));
    }
    if !state.search.is_enabled() {
        return Ok(Json(vec![]));
    }
    let limit = params.limit.unwrap_or(20).min(100);
    let hits = state
        .search
        .search(user.id, &params.q, limit)
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(hits))
}
