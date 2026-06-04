use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use axum_extra::extract::CookieJar;
use sha2::{Digest, Sha256};
use std::sync::Arc;

use crate::models::{ApiToken, User, UserRole};
use crate::AppState;

const SESSION_COOKIE: &str = "session";

#[derive(Clone)]
pub struct AuthUser(pub User);

impl std::ops::Deref for AuthUser {
    type Target = User;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Scopes carried by an OAuth-issued bearer. `None` means "no scope filter"
/// (session cookie or legacy PAT) — full access. `Some(vec)` means the
/// request is gated to those scopes; routes opt in to checking via the
/// `Scopes` extractor.
#[derive(Clone, Default)]
pub struct Scopes(pub Option<Vec<String>>);

impl Scopes {
    pub fn allows(&self, required: &str) -> bool {
        match &self.0 {
            None => true,
            Some(s) => s.iter().any(|sc| sc == required),
        }
    }

    /// Reject the request if the bearer's scopes don't include `required`.
    /// Sessions and legacy PATs (`Scopes(None)`) always pass — only OAuth
    /// bearers ever fail this check, and they only fail if the requested
    /// scope wasn't granted.
    pub fn require(&self, required: &str) -> Result<(), crate::error::AppError> {
        if self.allows(required) {
            Ok(())
        } else {
            Err(crate::error::AppError::Forbidden(format!(
                "Scope `{}` required",
                required
            )))
        }
    }
}

/// Try to resolve a bearer token: first as a session token, then as an API
/// token. Returns the user and any OAuth scopes attached to the token (None
/// for sessions and legacy PATs).
async fn resolve_bearer_token(
    state: &AppState,
    token: &str,
) -> Option<(User, Option<Vec<String>>)> {
    // 1. Try session lookup — sessions never carry scopes.
    if let Ok((user, _session)) = state.auth.validate_session(token).await {
        return Some((user, None));
    }

    // 2. Try API token lookup (hash the bearer value and match)
    let hash = hex::encode(Sha256::digest(token.as_bytes()));
    let api_tokens = state.db.collection::<ApiToken>("api_tokens");
    if let Ok(Some(api_token)) = api_tokens
        .find_one(mongodb::bson::doc! { "token_hash": &hash })
        .await
    {
        if api_token.is_expired() {
            return None;
        }
        if let Ok(Some(user)) = state.auth.get_user_by_id(api_token.user_id).await {
            return Some((user, api_token.scopes));
        }
    }

    None
}

pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    mut request: Request,
    next: Next,
) -> Response {
    // Try cookie first
    let cookie_val = jar.get(SESSION_COOKIE).map(|c| c.value().to_string());
    let bearer_val = request
        .headers()
        .get("Authorization")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string());
    let origin_val = request
        .headers()
        .get("Origin")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string());
    tracing::debug!(
        "AUTH: cookie={:?}, bearer={:?}, origin={:?}, uri={}",
        cookie_val,
        bearer_val,
        origin_val,
        request.uri()
    );

    if let Some(cookie) = jar.get(SESSION_COOKIE) {
        if let Ok((user, _session)) = state.auth.validate_session(cookie.value()).await {
            request.extensions_mut().insert(AuthUser(user));
            request.extensions_mut().insert(Scopes(None));
            return next.run(request).await;
        }
    }

    // Try Authorization: Bearer <token>
    if let Some(bearer) = request
        .headers()
        .get("Authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
    {
        if let Some((user, scopes)) = resolve_bearer_token(&state, bearer).await {
            request.extensions_mut().insert(AuthUser(user));
            request.extensions_mut().insert(Scopes(scopes));
            return next.run(request).await;
        }
    }

    // Try ?token= query parameter (for EventSource / SSE which cannot set headers)
    if let Some(query) = request.uri().query() {
        for param in query.split('&') {
            if let Some(token) = param.strip_prefix("token=") {
                if let Some((user, scopes)) = resolve_bearer_token(&state, token).await {
                    request.extensions_mut().insert(AuthUser(user));
                    request.extensions_mut().insert(Scopes(scopes));
                    return next.run(request).await;
                }
            }
        }
    }

    (StatusCode::UNAUTHORIZED, "Authentication required").into_response()
}

pub async fn optional_auth_middleware(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    mut request: Request,
    next: Next,
) -> Response {
    if let Some(cookie) = jar.get(SESSION_COOKIE) {
        if let Ok((user, _session)) = state.auth.validate_session(cookie.value()).await {
            request.extensions_mut().insert(AuthUser(user));
            request.extensions_mut().insert(Scopes(None));
            return next.run(request).await;
        }
    }

    if let Some(bearer) = request
        .headers()
        .get("Authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
    {
        if let Some((user, scopes)) = resolve_bearer_token(&state, bearer).await {
            request.extensions_mut().insert(AuthUser(user));
            request.extensions_mut().insert(Scopes(scopes));
        }
    }

    next.run(request).await
}

pub async fn admin_middleware(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    mut request: Request,
    next: Next,
) -> Response {
    // Try cookie first
    let mut resolved_user: Option<User> = None;

    if let Some(cookie) = jar.get(SESSION_COOKIE) {
        if let Ok((user, _session)) = state.auth.validate_session(cookie.value()).await {
            resolved_user = Some(user);
        }
    }

    if resolved_user.is_none() {
        if let Some(bearer) = request
            .headers()
            .get("Authorization")
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "))
        {
            resolved_user = resolve_bearer_token(&state, bearer).await.map(|(u, _)| u);
        }
    }

    match resolved_user {
        Some(user) if user.role == UserRole::Admin => {
            request.extensions_mut().insert(AuthUser(user));
            next.run(request).await
        }
        Some(_) => (StatusCode::FORBIDDEN, "Admin access required").into_response(),
        None => (StatusCode::UNAUTHORIZED, "Authentication required").into_response(),
    }
}

// Extractor for getting the authenticated user
impl<S> axum::extract::FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<AuthUser>()
            .cloned()
            .ok_or((StatusCode::UNAUTHORIZED, "Not authenticated"))
    }
}

impl<S> axum::extract::FromRequestParts<S> for Scopes
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(parts
            .extensions
            .get::<Scopes>()
            .cloned()
            .unwrap_or_default())
    }
}
