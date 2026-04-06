use axum::{
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use mongodb::bson::{doc, oid::ObjectId};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

use crate::error::{AppError, Result};
use crate::middleware::AuthUser;
use crate::models::{App, Webhook};
use crate::AppState;

// ---------------------------------------------------------------------------
// Webhook event name constants
// ---------------------------------------------------------------------------

pub const EVENT_FILE_CREATED: &str = "file.created";
pub const EVENT_FILE_UPDATED: &str = "file.updated";
pub const EVENT_FILE_DELETED: &str = "file.deleted";

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct RegisterAppRequest {
    pub name: String,
    pub nav_label: String,
    pub icon: String,
    pub base_url: String,
    pub secret: String,
}

#[derive(Serialize)]
pub struct RegisterAppResponse {
    pub id: String,
    pub name: String,
    pub db: String,
    pub db_uri: String,
}

#[derive(Serialize)]
pub struct AppListEntry {
    pub id: String,
    pub name: String,
    pub nav_label: String,
    pub icon: String,
}

#[derive(Deserialize)]
pub struct RegisterWebhookRequest {
    pub app_name: String,
    pub url: String,
    pub events: Vec<String>,
    pub secret: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn validate_base_url(url: &str) -> Result<()> {
    let parsed = url.parse::<reqwest::Url>()
        .map_err(|_| AppError::BadRequest("base_url must be a valid URL".to_string()))?;
    let host = parsed.host_str().unwrap_or("");
    if host != "localhost" && host != "127.0.0.1" && host != "::1" {
        return Err(AppError::BadRequest(
            "base_url must target localhost (127.0.0.1 or ::1) — remote app hosts are not supported".to_string()
        ));
    }
    Ok(())
}

fn verify_secret(state: &AppState, secret: &str) -> Result<()> {
    match &state.config.apps.registration_secret {
        None => Err(AppError::Internal(
            "App registration is disabled (no registration_secret configured)".to_string(),
        )),
        Some(expected) if expected == secret => Ok(()),
        Some(_) => Err(AppError::Forbidden("Access denied".into())),
    }
}

// ---------------------------------------------------------------------------
// POST /api/v1/apps/register  (public — secret-protected)
// ---------------------------------------------------------------------------

pub async fn register_app(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RegisterAppRequest>,
) -> Result<Json<RegisterAppResponse>> {
    verify_secret(&state, &body.secret)?;

    if body.name.is_empty() || body.nav_label.is_empty() || body.base_url.is_empty() {
        return Err(AppError::BadRequest(
            "name, nav_label, and base_url are required".to_string(),
        ));
    }

    validate_base_url(&body.base_url)?;

    let apps_coll = state.db.collection::<App>("apps");

    // Upsert by name
    let filter = doc! { "name": &body.name };
    let existing = apps_coll.find_one(filter.clone()).await?;

    let app_id = if let Some(existing) = existing {
        // Update existing app
        apps_coll
            .update_one(
                filter,
                doc! { "$set": {
                    "nav_label": &body.nav_label,
                    "icon": &body.icon,
                    "base_url": &body.base_url,
                }},
            )
            .await?;
        existing.id
    } else {
        // Insert new app
        let app = App {
            id: ObjectId::new(),
            name: body.name.clone(),
            nav_label: body.nav_label.clone(),
            icon: body.icon.clone(),
            base_url: body.base_url.clone(),
            enabled_for: Vec::new(),
            created_at: Utc::now(),
        };
        apps_coll.insert_one(&app).await?;
        app.id
    };

    // Provision a dedicated MongoDB database for the app
    let app_db_name = format!("uncloud_app_{}", &body.name);
    let app_db = state.db.client().database(&app_db_name);
    // MongoDB creates databases lazily; create a placeholder collection
    let _ = app_db.create_collection("_meta").await;

    let db_uri = format!(
        "{}/{}",
        state.config.database.uri.trim_end_matches('/'),
        &app_db_name
    );

    Ok(Json(RegisterAppResponse {
        id: app_id.to_hex(),
        name: body.name,
        db: app_db_name,
        db_uri,
    }))
}

// ---------------------------------------------------------------------------
// GET /api/v1/apps  (authenticated)
// ---------------------------------------------------------------------------

pub async fn list_apps(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<AppListEntry>>> {
    let apps_coll = state.db.collection::<App>("apps");

    // Apps where enabled_for is empty (all users) or contains this user
    let filter = doc! {
        "$or": [
            { "enabled_for": { "$size": 0 } },
            { "enabled_for": user.id },
        ]
    };

    let mut cursor = apps_coll.find(filter).await?;
    let mut entries = Vec::new();

    while cursor.advance().await? {
        let app: App = cursor.deserialize_current()?;
        entries.push(AppListEntry {
            id: app.id.to_hex(),
            name: app.name,
            nav_label: app.nav_label,
            icon: app.icon,
        });
    }

    Ok(Json(entries))
}

// ---------------------------------------------------------------------------
// DELETE /api/v1/apps/:name  (admin only)
// ---------------------------------------------------------------------------

pub async fn delete_app(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<StatusCode> {
    let apps_coll = state.db.collection::<App>("apps");
    let result = apps_coll.delete_one(doc! { "name": &name }).await?;

    if result.deleted_count == 0 {
        return Err(AppError::NotFound("App not found".to_string()));
    }

    // Also clean up webhooks for this app
    let webhooks_coll = state.db.collection::<Webhook>("webhooks");
    let _ = webhooks_coll
        .delete_many(doc! { "app_name": &name })
        .await;

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// POST /api/v1/apps/webhooks  (public — secret-protected)
// ---------------------------------------------------------------------------

pub async fn register_webhook(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RegisterWebhookRequest>,
) -> Result<Json<serde_json::Value>> {
    verify_secret(&state, &body.secret)?;

    if body.url.is_empty() || body.events.is_empty() {
        return Err(AppError::BadRequest(
            "url and events are required".to_string(),
        ));
    }

    validate_base_url(&body.url)?;

    // Verify the app exists
    let apps_coll = state.db.collection::<App>("apps");
    let _app = apps_coll
        .find_one(doc! { "name": &body.app_name })
        .await?
        .ok_or_else(|| AppError::NotFound("App not found".to_string()))?;

    let webhook = Webhook {
        id: ObjectId::new(),
        app_name: body.app_name,
        url: body.url,
        events: body.events,
        created_at: Utc::now(),
    };

    let webhooks_coll = state.db.collection::<Webhook>("webhooks");
    webhooks_coll.insert_one(&webhook).await?;

    Ok(Json(json!({
        "id": webhook.id.to_hex(),
        "app_name": webhook.app_name,
        "url": webhook.url,
        "events": webhook.events,
    })))
}

// ---------------------------------------------------------------------------
// DELETE /api/v1/apps/webhooks/:id  (admin only)
// ---------------------------------------------------------------------------

pub async fn delete_webhook(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let oid = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid webhook ID".to_string()))?;

    let webhooks_coll = state.db.collection::<Webhook>("webhooks");
    let result = webhooks_coll.delete_one(doc! { "_id": oid }).await?;

    if result.deleted_count == 0 {
        return Err(AppError::NotFound("Webhook not found".to_string()));
    }

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Reverse proxy: /apps/{name}/{*path}
// ---------------------------------------------------------------------------

pub async fn proxy_handler(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    headers: HeaderMap,
    request: Request,
) -> Result<Response> {
    // Derive app name and upstream path directly from the URI so that this
    // handler works for all three registered routes:
    //   /apps/{name}          → upstream /
    //   /apps/{name}/         → upstream /
    //   /apps/{name}/{*path}  → upstream /{path}
    let full_path = request.uri().path().to_string();
    let without_prefix = full_path.strip_prefix("/apps/").unwrap_or(&full_path);
    let (name, upstream_path) = match without_prefix.find('/') {
        Some(pos) => {
            let n = &without_prefix[..pos];
            let p = &without_prefix[pos..]; // includes leading '/'
            (n.to_string(), if p == "/" { "/".to_string() } else { p.to_string() })
        }
        None => (without_prefix.to_string(), "/".to_string()),
    };

    let apps_coll = state.db.collection::<App>("apps");
    let app = apps_coll
        .find_one(doc! { "name": &name })
        .await?
        .ok_or_else(|| AppError::NotFound(format!("App '{}' not found or not registered", name)))?;

    // Check user is allowed
    if !app.enabled_for.is_empty() && !app.enabled_for.contains(&user.id) {
        return Err(AppError::Forbidden("Access denied".into()));
    }

    let upstream = format!("{}{}", app.base_url.trim_end_matches('/'), upstream_path);

    // Append query string if present
    let upstream = if let Some(query) = request.uri().query() {
        format!("{}?{}", upstream, query)
    } else {
        upstream
    };

    let client = state.http_client.clone();
    let method = request.method().clone();

    let mut req_builder = client.request(
        reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap(),
        &upstream,
    );

    // Forward select headers
    if let Some(ct) = headers.get("content-type") {
        req_builder = req_builder.header("content-type", ct.to_str().unwrap_or(""));
    }
    if let Some(accept) = headers.get("accept") {
        req_builder = req_builder.header("accept", accept.to_str().unwrap_or(""));
    }
    // Inject user identity header
    req_builder = req_builder.header("X-Uncloud-User-Id", user.id.to_hex());

    // Forward body
    // TODO: make body size limit configurable (config.apps.max_proxy_body_bytes)
    // TODO: stream body through to reqwest instead of buffering for large uploads
    let body_bytes = axum::body::to_bytes(request.into_body(), 50 * 1024 * 1024)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to read request body: {}", e)))?;

    if !body_bytes.is_empty() {
        req_builder = req_builder.body(body_bytes.to_vec());
    }

    let upstream_resp = req_builder
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("Upstream request failed: {}", e)))?;

    let status = StatusCode::from_u16(upstream_resp.status().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

    let mut response_headers = HeaderMap::new();
    if let Some(ct) = upstream_resp.headers().get("content-type") {
        response_headers.insert("content-type", ct.clone());
    }
    if let Some(cl) = upstream_resp.headers().get("content-length") {
        response_headers.insert("content-length", cl.clone());
    }
    if let Some(cc) = upstream_resp.headers().get("cache-control") {
        response_headers.insert("cache-control", cc.clone());
    }

    let response_body = upstream_resp
        .bytes()
        .await
        .map_err(|e| AppError::Internal(format!("Failed to read upstream response: {}", e)))?;

    Ok((status, response_headers, response_body.to_vec()).into_response())
}

// ---------------------------------------------------------------------------
// Webhook delivery (called from file CRUD handlers)
// ---------------------------------------------------------------------------

pub async fn deliver_webhooks(state: &AppState, event_name: &str, payload: serde_json::Value) {
    let client = state.http_client.clone();
    let webhooks_coll = state.db.collection::<Webhook>("webhooks");

    let filter = doc! { "events": event_name };
    let mut cursor = match webhooks_coll.find(filter).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("Failed to query webhooks: {}", e);
            return;
        }
    };

    while let Ok(true) = cursor.advance().await {
        let webhook: Webhook = match cursor.deserialize_current() {
            Ok(w) => w,
            Err(_) => continue,
        };
        let url = webhook.url.clone();
        let body = payload.clone();
        let event = event_name.to_string();
        let client = client.clone();

        tokio::spawn(async move {
            // TODO: add X-Webhook-Signature: sha256=<hmac> header for payload verification (V2)
            for attempt in 0..3u32 {
                match client
                    .post(&url)
                    .json(&serde_json::json!({
                        "event": event,
                        "data": body,
                    }))
                    .send()
                    .await
                {
                    Ok(r) if r.status().is_success() => break,
                    Ok(r) => {
                        tracing::warn!(
                            "Webhook {} returned status {} (attempt {})",
                            url,
                            r.status(),
                            attempt + 1
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Webhook {} failed (attempt {}): {}",
                            url,
                            attempt + 1,
                            e
                        );
                    }
                }
                if attempt < 2 {
                    tokio::time::sleep(std::time::Duration::from_secs(2u64.pow(attempt))).await;
                }
            }
        });
    }
}
