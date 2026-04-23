use std::sync::Arc;
use axum::http::{HeaderValue, Method};
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::routing::get;
use clap::{Parser, Subcommand};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use chrono::Utc;
use mongodb::bson::{self, doc};
use uncloud_server::models::{File, FileVersion, Folder, UserRole, UserStatus};

use uncloud_server::{
    AppState,
    config::Config,
    db,
    models,
    processing,
    routes,
    services::{AuthService, EventService, RescanService, SearchService, StorageService},
    supervisor::Supervisor,
};

// ── CLI definition ──────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "uncloud-server", about = "Uncloud — self-hosted personal cloud")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Start the server (default)
    Serve,
    /// Create the first admin user
    BootstrapAdmin {
        /// Admin username
        #[arg(long)]
        username: String,
        /// Admin password (generated if omitted)
        #[arg(long)]
        password: Option<String>,
        /// Admin email (optional)
        #[arg(long)]
        email: Option<String>,
    },
}

// ── Entry point ─────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        None | Some(Command::Serve) => run_server().await,
        Some(Command::BootstrapAdmin {
            username,
            password,
            email,
        }) => bootstrap_admin(username, password, email).await,
    }
}

// ── bootstrap-admin ─────────────────────────────────────────────────────────

async fn bootstrap_admin(
    username: String,
    password: Option<String>,
    email: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load_or_default();
    let db = db::connect(&config.database).await?;
    db::setup_indexes(&db).await?;

    let auth = AuthService::new(&db, config.auth.clone());

    // Check if user already exists
    let users = db.collection::<models::User>("users");
    if let Some(existing) = users.find_one(doc! { "username": &username }).await? {
        if existing.role == UserRole::Admin {
            println!("User '{}' already exists and is an admin.", username);
            return Ok(());
        }
        eprintln!(
            "Error: user '{}' already exists with role 'user'. \
             Promote them manually or choose a different username.",
            username
        );
        std::process::exit(1);
    }

    // Resolve or generate password
    let password_was_generated = password.is_none();
    let password = match password {
        Some(p) => p,
        None => {
            use rand::RngCore;
            let mut bytes = [0u8; 16];
            rand::thread_rng().fill_bytes(&mut bytes);
            hex::encode(bytes)
        }
    };

    if password.len() < 8 {
        eprintln!("Error: password must be at least 8 characters.");
        std::process::exit(1);
    }

    let password_hash = auth.hash_password(&password)?;
    let mut user = models::User::new(username.clone(), email.clone(), password_hash);
    user.role = UserRole::Admin;
    user.status = UserStatus::Active;

    users.insert_one(&user).await?;

    println!("Admin user created successfully.");
    println!("  Username: {}", username);
    if let Some(ref e) = email {
        println!("  Email:    {}", e);
    }
    if password_was_generated {
        println!("  Password: {}", password);
        println!();
        println!("  (Save this password — it will not be shown again.)");
    }

    Ok(())
}

// ── Server ──────────────────────────────────────────────────────────────────

async fn run_server() -> Result<(), Box<dyn std::error::Error>> {
    // Load configuration first so the logging filter honours `logging.level`.
    // `RUST_LOG` still wins when set; config supplies the default otherwise.
    let config = Config::load_or_default();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| config.logging.level.clone().into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("Starting Uncloud server...");
    tracing::info!("Configuration loaded");

    // Connect to database
    let db = db::connect(&config.database).await?;
    db::setup_indexes(&db).await?;
    db::setup_sync_audit_indexes(&db, &config.sync_audit).await?;

    // Initialize services
    let auth = AuthService::new(&db, config.auth.clone());
    let storage = StorageService::new(&db, &config.storage).await?;
    let events = EventService::new();
    let sync_log = uncloud_server::services::SyncLog::new(&db, events.clone(), config.sync_audit.enabled);
    let processing = processing::ProcessingService::new(
        config.processing.max_concurrency,
        config.processing.max_attempts,
    )
    .register(processing::ThumbnailProcessor {
        size: config.processing.thumbnail_size,
        max_pixels: config.processing.thumbnail_max_pixels,
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
        rescan: RescanService::new(),
        sync_log,
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

    // Spawn demo account purge background task
    if config.auth.registration == uncloud_server::config::RegistrationMode::Demo {
        let state = state.clone();
        tokio::spawn(async move {
            let interval = std::time::Duration::from_secs(3600); // check every hour
            loop {
                tokio::time::sleep(interval).await;
                match state.auth.purge_demo_accounts(&state.db).await {
                    Ok(0) => {}
                    Ok(n) => tracing::info!("Purged {} expired demo accounts", n),
                    Err(e) => tracing::error!("Demo account purge error: {}", e),
                }
            }
        });
    }

    // Build router with API routes
    let api_router = routes::create_router(state.clone());

    // Embedded frontend (assets baked into binary in release builds)
    let frontend = axum::Router::new()
        .route("/{*path}", get(uncloud_server::frontend::static_handler))
        .fallback(get(uncloud_server::frontend::index_handler));

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
        .fallback_service(frontend)
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
    // Force exit after a short grace period so long-lived SSE connections don't block shutdown.
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            tokio::signal::ctrl_c().await.ok();
            tracing::info!("Shutdown signal received, stopping managed apps...");
            app_shutdown.cancel();
            // Give managed apps a moment to exit, then force-exit so SSE connections don't hang.
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            tracing::info!("Forcing exit.");
            std::process::exit(0);
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
