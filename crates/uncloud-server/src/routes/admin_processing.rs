use axum::{extract::State, http::StatusCode};
use std::sync::Arc;

use crate::middleware::AuthUser;
use crate::AppState;

/// Admin-only: clear processing state from every file and re-queue the full
/// pipeline. Used after fixing a bug or raising `thumbnail_max_pixels` so
/// previously-failed files get another shot.
pub async fn rerun_all(
    State(state): State<Arc<AppState>>,
    _user: AuthUser,
) -> StatusCode {
    let state_clone = state.clone();
    tokio::spawn(async move {
        state_clone.processing.rerun_all(state_clone.clone()).await;
    });
    StatusCode::ACCEPTED
}
