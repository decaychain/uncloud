use axum::{
    extract::{Path, State},
    http::{header::SET_COOKIE, HeaderMap, StatusCode},
    response::IntoResponse,
    Extension, Json,
};
use mongodb::bson::{doc, oid::ObjectId};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::error::{AppError, Result};
use crate::middleware::AuthUser;
use crate::models::{User, UserRole};
use crate::AppState;

const SESSION_COOKIE: &str = "session";

/// Returns `"SameSite=None; Secure"` for Tauri origins (cross-origin WebView),
/// `"SameSite=Lax"` for same-origin browser requests.
/// Android uses `useHttpsScheme: true` so the origin is `https://tauri.localhost`.
fn samesite_for(headers: &HeaderMap) -> &'static str {
    if let Some(origin) = headers.get("Origin").and_then(|v| v.to_str().ok()) {
        if origin.contains("tauri.localhost") || origin.starts_with("tauri://") {
            return "SameSite=None; Secure";
        }
    }
    "SameSite=Lax"
}

fn compute_features_enabled(config: &crate::config::Config, disabled: &[String]) -> Vec<String> {
    let mut enabled = Vec::new();
    if config.features.shopping && !disabled.contains(&"shopping".to_string()) {
        enabled.push("shopping".to_string());
    }
    enabled
}

fn user_to_response(user: &User, config: &crate::config::Config) -> UserResponse {
    UserResponse {
        id: user.id.to_hex(),
        username: user.username.clone(),
        email: user.email.clone(),
        role: user.role,
        quota_bytes: user.quota_bytes,
        used_bytes: user.used_bytes,
        features_enabled: compute_features_enabled(config, &user.disabled_features),
        session_token: None,
    }
}

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub email: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct UserResponse {
    pub id: String,
    pub username: String,
    pub email: String,
    pub role: UserRole,
    pub quota_bytes: Option<i64>,
    pub used_bytes: i64,
    pub features_enabled: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_token: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SessionResponse {
    pub id: String,
    pub user_agent: Option<String>,
    pub ip_address: Option<String>,
    pub created_at: String,
    pub expires_at: String,
}

pub async fn register(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RegisterRequest>,
) -> Result<impl IntoResponse> {
    let user = state
        .auth
        .register(req.username, req.email, req.password)
        .await?;

    Ok((
        StatusCode::CREATED,
        Json(user_to_response(&user, &state.config)),
    ))
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Result<impl IntoResponse> {
    let user_agent = headers
        .get("User-Agent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let ip_address = headers
        .get("X-Forwarded-For")
        .or_else(|| headers.get("X-Real-IP"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or(s).trim().to_string());

    let (user, session) = state
        .auth
        .login(&req.username, &req.password, user_agent, ip_address)
        .await?;

    let origin_dbg = headers.get("Origin").and_then(|v| v.to_str().ok()).unwrap_or("<none>");
    let samesite = samesite_for(&headers);
    tracing::info!("LOGIN: Origin={}, SameSite chosen={}", origin_dbg, samesite);
    let cookie = format!(
        "{}={}; HttpOnly; {}; Path=/; Max-Age={}",
        SESSION_COOKIE,
        session.token,
        samesite,
        state.config.auth.session_duration_hours * 3600
    );
    tracing::info!("LOGIN: Set-Cookie={}", cookie);

    let mut headers = HeaderMap::new();
    headers.insert(SET_COOKIE, cookie.parse().unwrap());

    let mut resp = user_to_response(&user, &state.config);
    resp.session_token = Some(session.token.clone());

    Ok((
        headers,
        Json(resp),
    ))
}

pub async fn logout(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse> {
    if let Some(cookie) = headers.get("Cookie") {
        if let Ok(cookie_str) = cookie.to_str() {
            for part in cookie_str.split(';') {
                let part = part.trim();
                if let Some(token) = part.strip_prefix(&format!("{}=", SESSION_COOKIE)) {
                    state.auth.logout(token).await?;
                }
            }
        }
    }

    let clear_cookie = format!(
        "{}=; HttpOnly; {}; Path=/; Max-Age=0",
        SESSION_COOKIE,
        samesite_for(&headers),
    );

    let mut response_headers = HeaderMap::new();
    response_headers.insert(SET_COOKIE, clear_cookie.parse().unwrap());

    Ok((response_headers, StatusCode::NO_CONTENT))
}

pub async fn me(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<UserResponse>> {
    Ok(Json(user_to_response(&user, &state.config)))
}

pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<SessionResponse>>> {
    let sessions = state.auth.get_user_sessions(user.id).await?;

    Ok(Json(
        sessions
            .into_iter()
            .map(|s| SessionResponse {
                id: s.id.to_hex(),
                user_agent: s.user_agent,
                ip_address: s.ip_address,
                created_at: s.created_at.to_rfc3339(),
                expires_at: s.expires_at.to_rfc3339(),
            })
            .collect(),
    ))
}

pub async fn revoke_session(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let session_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid session ID".to_string()))?;

    state.auth.revoke_session(user.id, session_id).await?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn update_my_features(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<uncloud_common::UpdateFeaturesRequest>,
) -> Result<Json<UserResponse>> {
    let users_coll = state.db.collection::<User>("users");

    if let Some(enabled) = body.shopping {
        if enabled {
            users_coll
                .update_one(
                    doc! { "_id": user.id },
                    doc! { "$pull": { "disabled_features": "shopping" } },
                )
                .await?;
        } else {
            users_coll
                .update_one(
                    doc! { "_id": user.id },
                    doc! { "$addToSet": { "disabled_features": "shopping" } },
                )
                .await?;
        }
    }

    let updated_user = users_coll
        .find_one(doc! { "_id": user.id })
        .await?
        .ok_or(AppError::NotFound("User not found".to_string()))?;

    Ok(Json(user_to_response(&updated_user, &state.config)))
}
