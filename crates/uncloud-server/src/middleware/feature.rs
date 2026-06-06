use std::sync::Arc;

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::middleware::AuthUser;
use crate::AppState;

fn user_feature_enabled(state: &AppState, user: &AuthUser, feature: &str) -> bool {
    state.config.features.is_enabled(feature)
        && !user
            .disabled_features
            .iter()
            .any(|disabled| disabled == feature)
}

pub async fn require_tasks_feature(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    request: Request,
    next: Next,
) -> Response {
    if !user_feature_enabled(&state, &user, crate::config::FEATURE_TASKS) {
        return crate::error::AppError::Forbidden("Access denied".into()).into_response();
    }
    next.run(request).await
}

pub async fn require_music_feature(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    request: Request,
    next: Next,
) -> Response {
    if !user_feature_enabled(&state, &user, crate::config::FEATURE_MUSIC) {
        return crate::error::AppError::Forbidden("Access denied".into()).into_response();
    }
    next.run(request).await
}
