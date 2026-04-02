use std::sync::Arc;
use axum::http::{HeaderValue, Method};
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use chrono::Utc;
use mongodb::bson::{self, doc};
use uncloud_server::models::{File, FileVersion, Folder};

use uncloud_server::{
    AppState,
    config::Config,
    db,
    models,
    processing,
    routes,
    services::{AuthService, EventService, SearchService, StorageService},
    supervisor::Supervisor,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "uncloud_server=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("Starting Uncloud server...");

    // Load configuration
    let config = Config::load_or_default();
    tracing::info!("Configuration loaded");

    // Connect to database
    let db = db::connect(&config.database).await?;
    db::setup_indexes(&db).await?;

    // Initialize services
    let auth = AuthService::new(&db, config.auth.clone());
    let storage = StorageService::new(&db, &config.storage).await?;
    let events = EventService::new();
    let processing = processing::ProcessingService::new(
        config.processing.max_concurrency,
        config.processing.max_attempts,
    )
    .register(processing::ThumbnailProcessor {
        size: config.processing.thumbnail_size,
    })
    .register(processing::AudioMetadataProcessor {
        thumbnail_size: config.processing.thumbnail_size,
    })
    .register(processing::TextExtractProcessor)
    .register(processing::SearchIndexProcessor);

    let search = SearchService::new(&config.search)
        .await
        .expect("Failed to initialise SearchService");

    // Ensure default storage exists
    let users_collection = db.collection::<models::User>("users");
    let mut cursor = users_collection
        .find(mongodb::bson::doc! { "role": "admin" })
        .await?;
    if cursor.advance().await? {
        let admin: models::User = cursor.deserialize_current()?;
        let _ = storage.get_or_create_default(admin.id).await;
    }

    let state = Arc::new(AppState {
        config: config.clone(),
        db,
        auth,
        storage,
        events,
        processing,
        search,
        http_client: reqwest::Client::new(),
    });

    state.processing.recover(state.clone()).await;

    // Spawn trash auto-purge background task
    {
        let retention_days = config.versioning.trash_retention_days;
        if retention_days > 0 {
            let state = state.clone();
            tokio::spawn(async move {
                let interval = std::time::Duration::from_secs(3600); // check every hour
                loop {
                    tokio::time::sleep(interval).await;
                    if let Err(e) = purge_expired_trash(&state, retention_days).await {
                        tracing::error!("Trash auto-purge error: {}", e);
                    }
                }
            });
        }
    }

    // Build router with API routes
    let api_router = routes::create_router(state.clone());

    // Serve static files from the frontend build directory
    let static_dir = std::env::var("STATIC_DIR")
        .unwrap_or_else(|_| "target/dx/uncloud-web/release/web/public".to_string());
    let index_path = format!("{}/index.html", static_dir);
    let serve_dir = ServeDir::new(&static_dir)
        .not_found_service(ServeFile::new(&index_path));

    let cors = CorsLayer::new()
        .allow_origin([
            "tauri://localhost".parse::<HeaderValue>().unwrap(),
            "https://tauri.localhost".parse::<HeaderValue>().unwrap(),
            "http://tauri.localhost".parse::<HeaderValue>().unwrap(),
        ])
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE, Method::PATCH])
        .allow_headers([CONTENT_TYPE, AUTHORIZATION])
        .allow_credentials(true);

    let app = api_router
        .fallback_service(serve_dir)
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    tracing::info!("Server listening on {}", addr);

    // Build supervisor for managed apps
    let supervisor = Supervisor::new(state.clone());
    let app_shutdown = supervisor.shutdown.clone();

    // Launch managed apps after a short delay (let the server bind first)
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        supervisor.start_all().await;
    });

    // Graceful shutdown: Ctrl-C cancels the supervisor AND stops Axum.
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            tokio::signal::ctrl_c().await.ok();
            tracing::info!("Shutdown signal received, stopping managed apps...");
            app_shutdown.cancel();
            // Give managed apps a moment to exit before the server drops connections.
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        })
        .await?;

    Ok(())
}

/// Purge trashed files and folders older than `retention_days`.
async fn purge_expired_trash(
    state: &AppState,
    retention_days: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let cutoff = Utc::now() - chrono::Duration::days(retention_days as i64);
    let cutoff_bson = bson::DateTime::from_chrono(cutoff);

    let files_coll = state.db.collection::<File>("files");
    let folders_coll = state.db.collection::<Folder>("folders");
    let versions_coll = state.db.collection::<FileVersion>("file_versions");

    // Purge expired trashed files
    let mut cursor = files_coll
        .find(doc! { "deleted_at": { "$ne": bson::Bson::Null, "$lt": cutoff_bson } })
        .await?;

    let mut total_size = 0i64;
    let mut file_ids = Vec::new();
    let mut user_sizes: std::collections::HashMap<mongodb::bson::oid::ObjectId, i64> =
        std::collections::HashMap::new();

    while cursor.advance().await? {
        let file: File = cursor.deserialize_current()?;
        if let Ok(backend) = state.storage.get_backend(file.storage_id).await {
            if let Some(ref tp) = file.trash_path {
                let _ = backend.delete(tp).await;
            }
        }
        *user_sizes.entry(file.owner_id).or_default() += file.size_bytes;
        total_size += file.size_bytes;
        file_ids.push(file.id);

        // Delete versions
        versions_coll
            .delete_many(doc! { "file_id": file.id })
            .await?;
    }

    if !file_ids.is_empty() {
        let bson_ids: Vec<bson::Bson> = file_ids
            .iter()
            .map(|id| bson::Bson::ObjectId(*id))
            .collect();
        files_coll
            .delete_many(doc! { "_id": { "$in": bson_ids } })
            .await?;
    }

    // Update quotas per user
    for (user_id, size) in &user_sizes {
        let _ = state.auth.update_user_bytes(*user_id, -*size).await;
    }

    // Purge expired trashed folders
    folders_coll
        .delete_many(doc! { "deleted_at": { "$ne": bson::Bson::Null, "$lt": cutoff_bson } })
        .await?;

    if total_size > 0 {
        tracing::info!(
            "Trash auto-purge: removed {} file(s), freed {} bytes",
            file_ids.len(),
            total_size
        );
    }

    Ok(())
}
