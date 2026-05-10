//! Per-route scope enforcement for OAuth bearers.
//!
//! Routes that mutate state opt in via `.layer(require_files_write())`
//! or `.layer(require_files_delete())`. The auth middleware has already
//! placed `Scopes` in the request extensions; this layer rejects with
//! 403 if the bearer's scopes don't satisfy the requirement. Sessions
//! and legacy PATs (`Scopes(None)`) bypass — the entire web UI is
//! unaffected.
//!
//! Spec note: 403 carries a `WWW-Authenticate: Bearer scope="..."`
//! challenge so a well-behaved OAuth client knows which scope to
//! request a stepped-up token with.

use axum::{
    extract::Request,
    http::{HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

use super::auth::Scopes;

async fn require(scope: &'static str, request: Request, next: Next) -> Response {
    let scopes = request
        .extensions()
        .get::<Scopes>()
        .cloned()
        .unwrap_or_default();
    if scopes.allows(scope) {
        return next.run(request).await;
    }
    let challenge = format!("Bearer scope=\"{}\"", scope);
    (
        StatusCode::FORBIDDEN,
        [
            (
                axum::http::header::WWW_AUTHENTICATE,
                HeaderValue::from_str(&challenge)
                    .unwrap_or_else(|_| HeaderValue::from_static("Bearer")),
            ),
        ],
        format!("Scope `{}` required", scope),
    )
        .into_response()
}

pub async fn require_files_write(request: Request, next: Next) -> Response {
    require("files:write", request, next).await
}

pub async fn require_files_delete(request: Request, next: Next) -> Response {
    require("files:delete", request, next).await
}
