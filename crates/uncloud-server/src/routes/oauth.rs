use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Form, Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{Duration, Utc};
use mongodb::bson::{doc, oid::ObjectId};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

use crate::error::{AppError, Result};
use crate::middleware::AuthUser;
use crate::models::{ApiToken, OAuthAuthorizationCode, OAuthClient};
use crate::AppState;

// =============================================================================
// Constants
// =============================================================================

const SUPPORTED_SCOPES: &[&str] = &["files:read", "files:write", "files:delete"];

const AUTHORIZATION_CODE_TTL: Duration = Duration::minutes(10);
const ACCESS_TOKEN_TTL: Duration = Duration::hours(1);

// =============================================================================
// Helpers
// =============================================================================

fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(&buf)
}

fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

fn issuer_url(headers: &HeaderMap) -> String {
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("http");
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get("host"))
        .and_then(|h| h.to_str().ok())
        .unwrap_or("localhost");
    format!("{}://{}", scheme, host)
}

/// Validates a redirect_uri claim. Public clients can register http://localhost
/// (any port/path, dev convenience) or any https URL. http for non-localhost
/// is rejected.
fn redirect_uri_is_acceptable(uri: &str) -> bool {
    if let Ok(parsed) = url::Url::parse(uri) {
        match parsed.scheme() {
            "https" => true,
            "http" => parsed
                .host_str()
                .map(|h| h == "localhost" || h == "127.0.0.1" || h == "[::1]")
                .unwrap_or(false),
            _ => false,
        }
    } else {
        false
    }
}

fn validate_scope_string(scope: &str) -> std::result::Result<Vec<String>, String> {
    let mut out = Vec::new();
    for s in scope.split_ascii_whitespace() {
        if !SUPPORTED_SCOPES.contains(&s) {
            return Err(format!("unsupported scope: {}", s));
        }
        if !out.contains(&s.to_string()) {
            out.push(s.to_string());
        }
    }
    if out.is_empty() {
        return Err("scope is required".into());
    }
    Ok(out)
}

// =============================================================================
// Discovery
// =============================================================================

#[derive(Serialize)]
pub struct AuthorizationServerMetadata {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: String,
    revocation_endpoint: String,
    response_types_supported: Vec<&'static str>,
    grant_types_supported: Vec<&'static str>,
    code_challenge_methods_supported: Vec<&'static str>,
    token_endpoint_auth_methods_supported: Vec<&'static str>,
    scopes_supported: Vec<&'static str>,
}

pub async fn authorization_server_metadata(
    headers: HeaderMap,
) -> Json<AuthorizationServerMetadata> {
    let issuer = issuer_url(&headers);
    Json(AuthorizationServerMetadata {
        authorization_endpoint: format!("{}/oauth/authorize", issuer),
        token_endpoint: format!("{}/oauth/token", issuer),
        registration_endpoint: format!("{}/oauth/register", issuer),
        revocation_endpoint: format!("{}/oauth/revoke", issuer),
        issuer,
        response_types_supported: vec!["code"],
        grant_types_supported: vec!["authorization_code", "refresh_token"],
        code_challenge_methods_supported: vec!["S256"],
        token_endpoint_auth_methods_supported: vec!["none"],
        scopes_supported: SUPPORTED_SCOPES.to_vec(),
    })
}

#[derive(Serialize)]
pub struct ProtectedResourceMetadata {
    resource: String,
    authorization_servers: Vec<String>,
    scopes_supported: Vec<&'static str>,
    bearer_methods_supported: Vec<&'static str>,
}

pub async fn protected_resource_metadata(headers: HeaderMap) -> Json<ProtectedResourceMetadata> {
    let issuer = issuer_url(&headers);
    Json(ProtectedResourceMetadata {
        resource: issuer.clone(),
        authorization_servers: vec![issuer],
        scopes_supported: SUPPORTED_SCOPES.to_vec(),
        bearer_methods_supported: vec!["header"],
    })
}

// =============================================================================
// Dynamic client registration (RFC 7591)
// =============================================================================

#[derive(Deserialize)]
pub struct RegisterClientRequest {
    pub client_name: Option<String>,
    pub redirect_uris: Vec<String>,
    #[serde(default)]
    pub grant_types: Vec<String>,
    #[serde(default)]
    pub response_types: Vec<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub token_endpoint_auth_method: Option<String>,
}

#[derive(Serialize)]
pub struct RegisterClientResponse {
    pub client_id: String,
    pub client_id_issued_at: i64,
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    pub grant_types: Vec<&'static str>,
    pub response_types: Vec<&'static str>,
    pub token_endpoint_auth_method: &'static str,
    pub scope: String,
}

pub async fn register_client(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RegisterClientRequest>,
) -> Result<(StatusCode, Json<RegisterClientResponse>)> {
    if body.redirect_uris.is_empty() {
        return Err(AppError::BadRequest(
            "redirect_uris must not be empty".into(),
        ));
    }
    for uri in &body.redirect_uris {
        if !redirect_uri_is_acceptable(uri) {
            return Err(AppError::BadRequest(format!(
                "redirect_uri rejected: {}",
                uri
            )));
        }
    }
    if let Some(method) = &body.token_endpoint_auth_method {
        if method != "none" {
            return Err(AppError::BadRequest(
                "only public clients (token_endpoint_auth_method=none) are supported".into(),
            ));
        }
    }

    let allowed_scopes = match body.scope.as_deref() {
        Some(s) if !s.trim().is_empty() => {
            validate_scope_string(s).map_err(AppError::BadRequest)?
        }
        _ => SUPPORTED_SCOPES.iter().map(|s| s.to_string()).collect(),
    };

    let client_id = format!("uc_{}", random_token(16));
    let client_name = body
        .client_name
        .unwrap_or_else(|| "Unnamed OAuth client".into());

    let client = OAuthClient::new(
        client_id.clone(),
        client_name.clone(),
        body.redirect_uris.clone(),
        allowed_scopes.clone(),
        true,
    );
    let coll = state.db.collection::<OAuthClient>("oauth_clients");
    coll.insert_one(&client).await?;

    Ok((
        StatusCode::CREATED,
        Json(RegisterClientResponse {
            client_id_issued_at: client.created_at.timestamp(),
            client_id: client.client_id,
            client_name,
            redirect_uris: client.redirect_uris,
            grant_types: vec!["authorization_code", "refresh_token"],
            response_types: vec!["code"],
            token_endpoint_auth_method: "none",
            scope: allowed_scopes.join(" "),
        }),
    ))
}

// =============================================================================
// Public client lookup (used by the consent UI)
// =============================================================================

#[derive(Deserialize)]
pub struct ClientLookupQuery {
    pub client_id: String,
}

#[derive(Serialize)]
pub struct ClientLookupResponse {
    pub client_id: String,
    pub client_name: String,
    pub allowed_scopes: Vec<String>,
}

pub async fn lookup_client(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ClientLookupQuery>,
) -> Result<Json<ClientLookupResponse>> {
    let coll = state.db.collection::<OAuthClient>("oauth_clients");
    let client = coll
        .find_one(doc! { "client_id": &q.client_id })
        .await?
        .ok_or_else(|| AppError::NotFound("client not found".into()))?;
    Ok(Json(ClientLookupResponse {
        client_id: client.client_id,
        client_name: client.client_name,
        allowed_scopes: client.allowed_scopes,
    }))
}

// =============================================================================
// Authorize — POST consent submit (the GET is rendered by the Dioxus frontend
// at the same path; the frontend reads query params and posts here).
// =============================================================================

#[derive(Deserialize)]
pub struct AuthorizeSubmitRequest {
    pub client_id: String,
    pub redirect_uri: String,
    pub response_type: String,
    pub scope: String,
    pub state: Option<String>,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub decision: String, // "allow" | "deny"
}

#[derive(Serialize)]
pub struct AuthorizeSubmitResponse {
    pub redirect_to: String,
}

pub async fn authorize_submit(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<AuthorizeSubmitRequest>,
) -> Result<Json<AuthorizeSubmitResponse>> {
    if body.response_type != "code" {
        return Err(AppError::BadRequest(
            "only response_type=code is supported".into(),
        ));
    }
    if body.code_challenge_method != "S256" {
        return Err(AppError::BadRequest(
            "only code_challenge_method=S256 is supported".into(),
        ));
    }
    if body.code_challenge.len() < 43 || body.code_challenge.len() > 128 {
        return Err(AppError::BadRequest("invalid code_challenge length".into()));
    }

    let scopes = validate_scope_string(&body.scope).map_err(AppError::BadRequest)?;

    let clients = state.db.collection::<OAuthClient>("oauth_clients");
    let client = clients
        .find_one(doc! { "client_id": &body.client_id })
        .await?
        .ok_or_else(|| AppError::BadRequest("unknown client_id".into()))?;

    if !client.redirect_uris.iter().any(|u| u == &body.redirect_uri) {
        return Err(AppError::BadRequest(
            "redirect_uri does not match registration".into(),
        ));
    }
    for s in &scopes {
        if !client.allowed_scopes.iter().any(|a| a == s) {
            return Err(AppError::BadRequest(format!(
                "scope not allowed for this client: {}",
                s
            )));
        }
    }

    if body.decision == "deny" {
        let url = build_redirect(
            &body.redirect_uri,
            &[
                ("error", "access_denied"),
                ("state", body.state.as_deref().unwrap_or("")),
            ],
        );
        return Ok(Json(AuthorizeSubmitResponse { redirect_to: url }));
    }
    if body.decision != "allow" {
        return Err(AppError::BadRequest("invalid decision".into()));
    }

    // Mint authorization code
    let code = random_token(32);
    let code_hash = sha256_hex(&code);
    let row = OAuthAuthorizationCode {
        id: ObjectId::new(),
        code_hash,
        client_id: body.client_id.clone(),
        user_id: user.id,
        scopes: scopes.clone(),
        redirect_uri: body.redirect_uri.clone(),
        code_challenge: body.code_challenge.clone(),
        code_challenge_method: body.code_challenge_method.clone(),
        expires_at: Utc::now() + AUTHORIZATION_CODE_TTL,
        consumed: false,
    };
    state
        .db
        .collection::<OAuthAuthorizationCode>("oauth_authorization_codes")
        .insert_one(&row)
        .await?;

    let url = build_redirect(
        &body.redirect_uri,
        &[
            ("code", &code),
            ("state", body.state.as_deref().unwrap_or("")),
        ],
    );
    Ok(Json(AuthorizeSubmitResponse { redirect_to: url }))
}

fn build_redirect(base: &str, params: &[(&str, &str)]) -> String {
    let mut url = base.to_string();
    let mut first = !url.contains('?');
    for (k, v) in params {
        if v.is_empty() {
            continue;
        }
        url.push(if first { '?' } else { '&' });
        first = false;
        url.push_str(&urlencoding::encode(k));
        url.push('=');
        url.push_str(&urlencoding::encode(v));
    }
    url
}

// =============================================================================
// Token endpoint (form-encoded per RFC 6749)
// =============================================================================

#[derive(Deserialize)]
pub struct TokenRequest {
    pub grant_type: String,
    // authorization_code
    pub code: Option<String>,
    pub redirect_uri: Option<String>,
    pub client_id: Option<String>,
    pub code_verifier: Option<String>,
    // refresh_token
    pub refresh_token: Option<String>,
}

#[derive(Serialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: &'static str,
    pub expires_in: i64,
    pub refresh_token: String,
    pub scope: String,
}

#[derive(Serialize)]
struct OAuthError {
    error: &'static str,
    error_description: String,
}

fn oauth_error(status: StatusCode, code: &'static str, desc: impl Into<String>) -> Response {
    (
        status,
        Json(OAuthError {
            error: code,
            error_description: desc.into(),
        }),
    )
        .into_response()
}

pub async fn token_endpoint(
    State(state): State<Arc<AppState>>,
    Form(req): Form<TokenRequest>,
) -> Response {
    match req.grant_type.as_str() {
        "authorization_code" => match exchange_code(&state, req).await {
            Ok(resp) => Json(resp).into_response(),
            Err(e) => e,
        },
        "refresh_token" => match exchange_refresh(&state, req).await {
            Ok(resp) => Json(resp).into_response(),
            Err(e) => e,
        },
        _ => oauth_error(
            StatusCode::BAD_REQUEST,
            "unsupported_grant_type",
            "grant_type must be authorization_code or refresh_token",
        ),
    }
}

async fn exchange_code(
    state: &AppState,
    req: TokenRequest,
) -> std::result::Result<TokenResponse, Response> {
    let code = req
        .code
        .as_deref()
        .ok_or_else(|| oauth_error(StatusCode::BAD_REQUEST, "invalid_request", "missing code"))?;
    let client_id = req.client_id.as_deref().ok_or_else(|| {
        oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "missing client_id",
        )
    })?;
    let redirect_uri = req.redirect_uri.as_deref().ok_or_else(|| {
        oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "missing redirect_uri",
        )
    })?;
    let verifier = req.code_verifier.as_deref().ok_or_else(|| {
        oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "missing code_verifier",
        )
    })?;

    let codes = state
        .db
        .collection::<OAuthAuthorizationCode>("oauth_authorization_codes");
    let code_hash = sha256_hex(code);
    let row = codes
        .find_one(doc! { "code_hash": &code_hash })
        .await
        .map_err(|_| {
            oauth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "code lookup failed",
            )
        })?
        .ok_or_else(|| oauth_error(StatusCode::BAD_REQUEST, "invalid_grant", "unknown code"))?;

    if row.consumed {
        return Err(oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "code already used",
        ));
    }
    if row.expires_at <= Utc::now() {
        return Err(oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "code expired",
        ));
    }
    if row.client_id != client_id {
        return Err(oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "client_id mismatch",
        ));
    }
    if row.redirect_uri != redirect_uri {
        return Err(oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "redirect_uri mismatch",
        ));
    }

    // Verify PKCE
    let verifier_hash = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    if verifier_hash != row.code_challenge {
        return Err(oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "code_verifier mismatch",
        ));
    }

    // Mark consumed (best-effort — TTL index will eventually drop the row).
    let _ = codes
        .update_one(
            doc! { "_id": row.id },
            doc! { "$set": { "consumed": true } },
        )
        .await;

    // Look up client name for the token's `name` field.
    let clients = state.db.collection::<OAuthClient>("oauth_clients");
    let client_name = clients
        .find_one(doc! { "client_id": &row.client_id })
        .await
        .ok()
        .flatten()
        .map(|c| c.client_name)
        .unwrap_or_else(|| row.client_id.clone());

    issue_token_pair(state, row.user_id, row.client_id, client_name, row.scopes)
        .await
        .map_err(|_| {
            oauth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "token issuance failed",
            )
        })
}

async fn exchange_refresh(
    state: &AppState,
    req: TokenRequest,
) -> std::result::Result<TokenResponse, Response> {
    let refresh = req.refresh_token.as_deref().ok_or_else(|| {
        oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "missing refresh_token",
        )
    })?;
    let client_id = req.client_id.as_deref().ok_or_else(|| {
        oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "missing client_id",
        )
    })?;

    let refresh_hash = sha256_hex(refresh);
    let api_tokens = state.db.collection::<ApiToken>("api_tokens");
    let row = api_tokens
        .find_one(doc! { "refresh_token_hash": &refresh_hash })
        .await
        .map_err(|_| {
            oauth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "refresh lookup failed",
            )
        })?
        .ok_or_else(|| {
            oauth_error(
                StatusCode::BAD_REQUEST,
                "invalid_grant",
                "unknown refresh_token",
            )
        })?;

    if row.client_id.as_deref() != Some(client_id) {
        return Err(oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "client_id mismatch",
        ));
    }

    // Rotate: delete the old token row, issue a fresh pair.
    let _ = api_tokens.delete_one(doc! { "_id": row.id }).await;

    let scopes = row.scopes.unwrap_or_default();
    issue_token_pair(state, row.user_id, row.client_id.unwrap(), row.name, scopes)
        .await
        .map_err(|_| {
            oauth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "token issuance failed",
            )
        })
}

async fn issue_token_pair(
    state: &AppState,
    user_id: ObjectId,
    client_id: String,
    client_name: String,
    scopes: Vec<String>,
) -> std::result::Result<TokenResponse, AppError> {
    let access = random_token(32);
    let refresh = random_token(32);
    let access_hash = sha256_hex(&access);
    let refresh_hash = sha256_hex(&refresh);
    let expires_at = Utc::now() + ACCESS_TOKEN_TTL;

    let scope_str = scopes.join(" ");
    let row = ApiToken::new_oauth(
        user_id,
        client_id,
        client_name,
        access_hash,
        scopes,
        expires_at,
        Some(refresh_hash),
    );
    state
        .db
        .collection::<ApiToken>("api_tokens")
        .insert_one(&row)
        .await?;

    Ok(TokenResponse {
        access_token: access,
        token_type: "Bearer",
        expires_in: ACCESS_TOKEN_TTL.num_seconds(),
        refresh_token: refresh,
        scope: scope_str,
    })
}

// =============================================================================
// Revocation (RFC 7009)
// =============================================================================

#[derive(Deserialize)]
pub struct RevokeRequest {
    pub token: String,
    #[serde(default)]
    pub token_type_hint: Option<String>,
}

pub async fn revoke_endpoint(
    State(state): State<Arc<AppState>>,
    Form(body): Form<RevokeRequest>,
) -> StatusCode {
    let hash = sha256_hex(&body.token);
    let api_tokens = state.db.collection::<ApiToken>("api_tokens");

    // Try as access token first, then as refresh.
    let _ = api_tokens
        .delete_one(doc! { "$or": [
            { "token_hash": &hash },
            { "refresh_token_hash": &hash },
        ]})
        .await;
    // RFC 7009: always 200 OK regardless of whether the token existed.
    StatusCode::OK
}

// =============================================================================
// Connected-apps management (authenticated)
// =============================================================================

#[derive(Serialize)]
pub struct ConnectedApp {
    pub client_id: String,
    pub client_name: String,
    pub scopes: Vec<String>,
    pub last_issued_at: String,
    pub token_count: u64,
}

pub async fn list_connected_apps(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<ConnectedApp>>> {
    let api_tokens = state.db.collection::<ApiToken>("api_tokens");
    let mut cursor = api_tokens
        .find(doc! { "user_id": user.id, "client_id": { "$ne": null } })
        .await?;

    use std::collections::HashMap;
    let mut by_client: HashMap<String, ConnectedApp> = HashMap::new();
    while cursor.advance().await? {
        let t: ApiToken = cursor.deserialize_current()?;
        let client_id = match &t.client_id {
            Some(id) => id.clone(),
            None => continue,
        };
        let entry = by_client.entry(client_id.clone()).or_insert(ConnectedApp {
            client_id: client_id.clone(),
            client_name: t.name.clone(),
            scopes: t.scopes.clone().unwrap_or_default(),
            last_issued_at: t.created_at.to_rfc3339(),
            token_count: 0,
        });
        entry.token_count += 1;
        if t.created_at.to_rfc3339() > entry.last_issued_at {
            entry.last_issued_at = t.created_at.to_rfc3339();
        }
        // Union the scopes seen across this client's tokens.
        if let Some(scopes) = &t.scopes {
            for s in scopes {
                if !entry.scopes.contains(s) {
                    entry.scopes.push(s.clone());
                }
            }
        }
    }

    let mut out: Vec<ConnectedApp> = by_client.into_values().collect();
    out.sort_by(|a, b| b.last_issued_at.cmp(&a.last_issued_at));
    Ok(Json(out))
}

pub async fn revoke_connected_app(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(client_id): Path<String>,
) -> Result<StatusCode> {
    let api_tokens = state.db.collection::<ApiToken>("api_tokens");
    api_tokens
        .delete_many(doc! { "user_id": user.id, "client_id": &client_id })
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
