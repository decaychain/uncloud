use axum::{
    extract::{Path, State},
    http::{header::SET_COOKIE, HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use mongodb::bson::{doc, oid::ObjectId};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::error::{AppError, Result};
use crate::middleware::AuthUser;
use crate::models::{User, UserRole};
use crate::services::auth::LoginOutcome;
use crate::AppState;

const SESSION_COOKIE: &str = "session";

/// Returns `"SameSite=None; Secure"` for Tauri origins (cross-origin WebView),
/// `"SameSite=Lax"` for same-origin browser requests.
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
        status: user.status,
        quota_bytes: user.quota_bytes,
        used_bytes: user.used_bytes,
        totp_enabled: user.totp_enabled,
        features_enabled: compute_features_enabled(config, &user.disabled_features),
        preferences: user.preferences.clone(),
        session_token: None,
    }
}

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    #[serde(default)]
    pub email: Option<String>,
    pub password: String,
    #[serde(default)]
    pub invite_token: Option<String>,
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
    pub email: Option<String>,
    pub role: UserRole,
    pub status: uncloud_common::UserStatus,
    pub quota_bytes: Option<i64>,
    pub used_bytes: i64,
    pub totp_enabled: bool,
    pub features_enabled: Vec<String>,
    pub preferences: uncloud_common::UserPreferences,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_token: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub user: Option<UserResponse>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub totp_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub totp_token: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SessionResponse {
    pub id: String,
    pub user_agent: Option<String>,
    pub ip_address: Option<String>,
    pub created_at: String,
    pub expires_at: String,
}

// ── Public: server-info ──────────────────────────────────────────────────────

pub async fn server_info(
    State(state): State<Arc<AppState>>,
) -> Json<uncloud_common::ServerInfoResponse> {
    Json(uncloud_common::ServerInfoResponse {
        registration_mode: state.config.auth.registration,
        version: env!("CARGO_PKG_VERSION").to_string(),
        name: None,
    })
}

// ── Public: register ─────────────────────────────────────────────────────────

pub async fn register(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<RegisterRequest>,
) -> Result<impl IntoResponse> {
    let user = state
        .auth
        .register(req.username, req.email, req.password, req.invite_token)
        .await?;

    let mut resp = user_to_response(&user, &state.config);

    if user.status == uncloud_common::UserStatus::Pending {
        return Ok((StatusCode::ACCEPTED, HeaderMap::new(), Json(resp)));
    }

    // Active user — create a session so they don't have to log in separately
    let user_agent = headers
        .get("User-Agent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let ip_address = headers
        .get("X-Forwarded-For")
        .or_else(|| headers.get("X-Real-IP"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or(s).trim().to_string());

    let session = state
        .auth
        .create_session(user.id, user_agent, ip_address)
        .await?;

    let samesite = samesite_for(&headers);
    let cookie = format!(
        "{}={}; HttpOnly; {}; Path=/; Max-Age={}",
        SESSION_COOKIE,
        session.token,
        samesite,
        state.config.auth.session_duration_hours * 3600
    );

    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(SET_COOKIE, cookie.parse().unwrap());

    resp.session_token = Some(session.token);

    Ok((StatusCode::CREATED, resp_headers, Json(resp)))
}

// ── Public: login ────────────────────────────────────────────────────────────

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

    let outcome = state
        .auth
        .login(&req.username, &req.password, user_agent, ip_address)
        .await?;

    match outcome {
        LoginOutcome::TotpRequired { totp_token } => {
            let resp = LoginResponse {
                user: None,
                totp_required: true,
                totp_token: Some(totp_token),
            };
            Ok((HeaderMap::new(), Json(resp)))
        }
        LoginOutcome::Success(user, session) => {
            let samesite = samesite_for(&headers);
            let cookie = format!(
                "{}={}; HttpOnly; {}; Path=/; Max-Age={}",
                SESSION_COOKIE,
                session.token,
                samesite,
                state.config.auth.session_duration_hours * 3600
            );

            let mut resp_headers = HeaderMap::new();
            resp_headers.insert(SET_COOKIE, cookie.parse().unwrap());

            let mut user_resp = user_to_response(&user, &state.config);
            user_resp.session_token = Some(session.token.clone());

            let resp = LoginResponse {
                user: Some(user_resp),
                totp_required: false,
                totp_token: None,
            };

            Ok((resp_headers, Json(resp)))
        }
    }
}

// ── Public: demo login ───────────────────────────────────────────────────────

pub async fn demo_login(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
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

    let (user, session) = state.auth.demo_login(user_agent, ip_address).await?;

    let samesite = samesite_for(&headers);
    let cookie = format!(
        "{}={}; HttpOnly; {}; Path=/; Max-Age={}",
        SESSION_COOKIE,
        session.token,
        samesite,
        state.config.auth.demo_ttl_hours * 3600
    );

    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(SET_COOKIE, cookie.parse().unwrap());

    let mut user_resp = user_to_response(&user, &state.config);
    user_resp.session_token = Some(session.token.clone());

    let resp = LoginResponse {
        user: Some(user_resp),
        totp_required: false,
        totp_token: None,
    };

    Ok((resp_headers, Json(resp)))
}

// ── Public: TOTP verify (complete two-step login) ────────────────────────────

pub async fn totp_verify(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<uncloud_common::TotpVerifyRequest>,
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
        .verify_totp_login(&req.totp_token, &req.code, user_agent, ip_address)
        .await?;

    let samesite = samesite_for(&headers);
    let cookie = format!(
        "{}={}; HttpOnly; {}; Path=/; Max-Age={}",
        SESSION_COOKIE,
        session.token,
        samesite,
        state.config.auth.session_duration_hours * 3600
    );

    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(SET_COOKIE, cookie.parse().unwrap());

    let mut user_resp = user_to_response(&user, &state.config);
    user_resp.session_token = Some(session.token.clone());

    let resp = LoginResponse {
        user: Some(user_resp),
        totp_required: false,
        totp_token: None,
    };

    Ok((resp_headers, Json(resp)))
}

// ── Public: validate invite ──────────────────────────────────��───────────────

pub async fn validate_invite(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Json<uncloud_common::InviteInfoResponse>> {
    let invite = state.auth.get_invite_info(&token).await?;
    match invite {
        Some(inv) if inv.is_valid() => Ok(Json(uncloud_common::InviteInfoResponse {
            valid: true,
        })),
        _ => Ok(Json(uncloud_common::InviteInfoResponse {
            valid: false,
        })),
    }
}

// ── Authenticated: session management ────────────────────────────────────────

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

// ── Authenticated: change password ───────────────────────────────────────────

pub async fn change_password(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(req): Json<uncloud_common::ChangePasswordRequest>,
) -> Result<StatusCode> {
    state
        .auth
        .change_password(user.id, &req.current_password, &req.new_password)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Authenticated: TOTP management ───────────────────────────────────────────

pub async fn totp_setup(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<uncloud_common::TotpSetupResponse>> {
    let data = state.auth.setup_totp(user.id).await?;
    Ok(Json(uncloud_common::TotpSetupResponse {
        secret: data.secret,
        otpauth_uri: data.otpauth_uri,
        qr_svg: data.qr_svg,
        recovery_codes: data.recovery_codes,
    }))
}

pub async fn totp_enable(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(req): Json<uncloud_common::TotpEnableRequest>,
) -> Result<StatusCode> {
    state.auth.enable_totp(user.id, &req.code).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn totp_disable(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(req): Json<uncloud_common::TotpDisableRequest>,
) -> Result<StatusCode> {
    state.auth.disable_totp(user.id, &req.code).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Authenticated: feature toggles ───────────────────────────────────────────

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

// ── Authenticated: UI preferences ────────────────────────────────────────────

pub async fn update_my_preferences(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<uncloud_common::UpdatePreferencesRequest>,
) -> Result<Json<UserResponse>> {
    let users_coll = state.db.collection::<User>("users");

    let mut update = mongodb::bson::Document::new();
    if let Some(tiles) = body.dashboard_tiles {
        update.insert("preferences.dashboard_tiles", tiles);
    }

    if !update.is_empty() {
        users_coll
            .update_one(doc! { "_id": user.id }, doc! { "$set": update })
            .await?;
    }

    let updated_user = users_coll
        .find_one(doc! { "_id": user.id })
        .await?
        .ok_or(AppError::NotFound("User not found".to_string()))?;

    Ok(Json(user_to_response(&updated_user, &state.config)))
}
