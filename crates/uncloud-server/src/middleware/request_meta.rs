//! Extracts client-identifying headers set by sync clients and desktop/mobile
//! apps, and attaches a [`RequestMeta`] to the request extensions so audit-log
//! emission sites can record *why* a mutation happened.
//!
//! Headers (all optional):
//!
//! - `X-Uncloud-Source`: `sync` | `user` | `admin`
//! - `X-Uncloud-Client`: free-form device / hostname label
//! - `X-Uncloud-Os`:     `linux` | `windows` | `macos` | `android` | `ios`
//!
//! When the source header is absent the middleware infers it from auth type:
//! a cookie session → `UserWeb`, a bearer token → `UserDesktop`.
//!
//! Admin routes override any declared source with `Admin` (set by
//! [`with_admin_source`] which the admin-route stack mounts explicitly).

use axum::{extract::Request, middleware::Next, response::Response};
use axum_extra::extract::CookieJar;
use uncloud_common::{SyncClientOs, SyncEventSource};

const SESSION_COOKIE: &str = "session";

#[derive(Debug, Clone)]
pub struct RequestMeta {
    pub source: SyncEventSource,
    pub client_id: Option<String>,
    pub client_os: Option<SyncClientOs>,
}

impl Default for RequestMeta {
    fn default() -> Self {
        Self {
            source: SyncEventSource::UserWeb,
            client_id: None,
            client_os: None,
        }
    }
}

fn parse_source(val: &str) -> Option<SyncEventSource> {
    match val.trim().to_ascii_lowercase().as_str() {
        "sync" => Some(SyncEventSource::Sync),
        "user" | "user_web" | "web" => Some(SyncEventSource::UserWeb),
        "user_desktop" | "desktop" => Some(SyncEventSource::UserDesktop),
        "user_mobile" | "mobile" => Some(SyncEventSource::UserMobile),
        "admin" => Some(SyncEventSource::Admin),
        "public" => Some(SyncEventSource::Public),
        "system" => Some(SyncEventSource::System),
        _ => None,
    }
}

fn parse_os(val: &str) -> Option<SyncClientOs> {
    match val.trim().to_ascii_lowercase().as_str() {
        "linux" => Some(SyncClientOs::Linux),
        "windows" | "win" => Some(SyncClientOs::Windows),
        "macos" | "mac" | "darwin" => Some(SyncClientOs::Macos),
        "android" => Some(SyncClientOs::Android),
        "ios" => Some(SyncClientOs::Ios),
        _ => Some(SyncClientOs::Unknown),
    }
}

/// Middleware: reads X-Uncloud-* headers, falls back to auth-type based defaults,
/// and stashes the result as a request extension.
pub async fn request_meta_middleware(
    jar: CookieJar,
    mut request: Request,
    next: Next,
) -> Response {
    let headers = request.headers();
    let explicit_source = headers
        .get("x-uncloud-source")
        .and_then(|h| h.to_str().ok())
        .and_then(parse_source);
    let client_id = headers
        .get("x-uncloud-client")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let client_os = headers
        .get("x-uncloud-os")
        .and_then(|h| h.to_str().ok())
        .and_then(parse_os);

    let source = explicit_source.unwrap_or_else(|| infer_source(&jar, request.headers()));

    request.extensions_mut().insert(RequestMeta {
        source,
        client_id,
        client_os,
    });
    next.run(request).await
}

/// Middleware applied on top of admin routes: forces `source = Admin` so an
/// admin-originated mutation is unambiguously attributed regardless of what
/// the client declared.
pub async fn admin_meta_middleware(mut request: Request, next: Next) -> Response {
    let (client_id, client_os) = request
        .extensions()
        .get::<RequestMeta>()
        .map(|m| (m.client_id.clone(), m.client_os))
        .unwrap_or((None, None));
    request.extensions_mut().insert(RequestMeta {
        source: SyncEventSource::Admin,
        client_id,
        client_os,
    });
    next.run(request).await
}

fn infer_source(jar: &CookieJar, headers: &axum::http::HeaderMap) -> SyncEventSource {
    if jar.get(SESSION_COOKIE).is_some() {
        return SyncEventSource::UserWeb;
    }
    if headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .map(|h| h.starts_with("Bearer "))
        .unwrap_or(false)
    {
        return SyncEventSource::UserDesktop;
    }
    SyncEventSource::UserWeb
}

/// Extractor: pulls the [`RequestMeta`] from request extensions, falling back
/// to a default rather than erroring — the audit log must degrade gracefully.
impl<S> axum::extract::FromRequestParts<S> for RequestMeta
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
            .get::<RequestMeta>()
            .cloned()
            .unwrap_or_default())
    }
}
