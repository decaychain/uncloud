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
    /// Find and clean duplicate live file documents.
    ///
    /// Pre-2026-04-25 the unique index on `(owner_id, parent_id, name)`
    /// was missing for live files, so any sync-engine bug that retried an
    /// upload could leak a second File document with the same logical
    /// path. Run this once against the affected DB to merge duplicates
    /// into a single survivor (newest `updated_at`) and re-point the
    /// duplicates' `file_versions` rows at it. Then start the server —
    /// `setup_indexes` will install the partial unique index that
    /// prevents new duplicates from showing up.
    DedupeFiles {
        /// Print the plan without executing it.
        #[arg(long)]
        dry_run: bool,
    },
    /// Move every blob owned by one storage backend to another, atomically
    /// flipping `File.storage_id` for each file. The server must be stopped
    /// while this runs — it sets a database lock that the server checks on
    /// startup. See `docs/storage-migration.md` for the full design.
    Migrate {
        /// Source storage — ObjectId or `Storage.name`.
        #[arg(long)]
        from: String,
        /// Destination storage — ObjectId or `Storage.name`.
        #[arg(long)]
        to: String,
        /// Restrict migration to descendants of this folder ObjectId.
        #[arg(long)]
        folder: Option<String>,
        /// Print the planned work and exit without copying anything.
        #[arg(long)]
        dry_run: bool,
        /// Verification mode: `none`, `size` (default — cheap), or `hash`
        /// (re-reads the dest blob and compares SHA-256 to the value stored
        /// on the File document).
        #[arg(long, default_value = "size")]
        verify: String,
        /// Clear a stale lock left by a previously crashed migration.
        #[arg(long)]
        force_unlock: bool,
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
        Some(Command::DedupeFiles { dry_run }) => dedupe_files(dry_run).await,
        Some(Command::Migrate {
            from,
            to,
            folder,
            dry_run,
            verify,
            force_unlock,
        }) => {
            let verify = verify.parse::<uncloud_server::migrate::VerifyMode>()?;
            uncloud_server::migrate::run(uncloud_server::migrate::MigrateArgs {
                from,
                to,
                folder,
                dry_run,
                verify,
                force_unlock,
            })
            .await
        }
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


// ── dedupe-files ────────────────────────────────────────────────────────────

async fn dedupe_files(dry_run: bool) -> Result<(), Box<dyn std::error::Error>> {
    use mongodb::bson::oid::ObjectId;
    use serde::Deserialize;

    let config = Config::load_or_default();
    // Connect WITHOUT setup_indexes — the partial unique index would fail
    // to create against a populated collection that still has duplicates.
    let db = db::connect(&config.database).await?;

    let files = db.collection::<File>("files");
    let versions = db.collection::<FileVersion>("file_versions");

    // Group every live file by (owner_id, parent_id, name).
    #[derive(Debug, Deserialize)]
    struct Group {
        #[serde(rename = "_id")]
        key: GroupKey,
        ids: Vec<ObjectId>,
        updated_at: Vec<bson::DateTime>,
        names: Vec<String>,
        count: i64,
    }
    #[derive(Debug, Deserialize)]
    struct GroupKey {
        owner_id: ObjectId,
        parent_id: Option<ObjectId>,
        name: String,
    }

    let pipeline = vec![
        doc! { "$match": { "deleted_at": bson::Bson::Null } },
        doc! { "$group": {
            "_id": { "owner_id": "$owner_id", "parent_id": "$parent_id", "name": "$name" },
            "count": { "$sum": 1 },
            "ids": { "$push": "$_id" },
            "updated_at": { "$push": "$updated_at" },
            "names": { "$push": "$name" },
        }},
        doc! { "$match": { "count": { "$gt": 1 } } },
        doc! { "$sort": { "count": -1 } },
    ];

    let mut cursor = files.aggregate(pipeline).await?;
    let mut groups: Vec<Group> = Vec::new();
    use futures::TryStreamExt;
    while let Some(doc) = cursor.try_next().await? {
        groups.push(bson::from_document(doc)?);
    }

    if groups.is_empty() {
        println!("No duplicate live files found — nothing to do.");
        return Ok(());
    }

    println!("Found {} duplicate group(s):", groups.len());
    let mut total_excess = 0u64;
    let mut total_versions_repointed = 0u64;
    let mut total_files_deleted = 0u64;

    for g in &groups {
        // Survivor: index of the row with the latest `updated_at`. The
        // engine's journal most likely already references this one
        // (it's the row whose `updated_at` was most recently bumped by
        // a successful upload).
        let mut survivor_idx = 0usize;
        for i in 1..g.ids.len() {
            if g.updated_at[i] > g.updated_at[survivor_idx] {
                survivor_idx = i;
            }
        }
        let survivor_id = g.ids[survivor_idx];
        let losers: Vec<ObjectId> = g
            .ids
            .iter()
            .enumerate()
            .filter_map(|(i, id)| if i == survivor_idx { None } else { Some(*id) })
            .collect();

        // Count file_versions belonging to the losers — we'll re-point
        // them at the survivor.
        let losers_filter = doc! { "file_id": { "$in": losers.iter().copied().collect::<Vec<_>>() } };
        let version_count = versions.count_documents(losers_filter.clone()).await?;

        let parent_label = g
            .key
            .parent_id
            .map(|p| p.to_hex())
            .unwrap_or_else(|| "<root>".to_string());
        println!(
            "  {} (parent={}, count={}): keep {}, drop {} — {} version(s) to re-point",
            g.key.name,
            parent_label,
            g.count,
            survivor_id,
            losers.iter().map(|id| id.to_hex()).collect::<Vec<_>>().join(","),
            version_count,
        );

        if !dry_run {
            // Re-point file_versions of the losers to the survivor so the
            // version chain stays intact.
            if version_count > 0 {
                versions
                    .update_many(
                        losers_filter,
                        doc! { "$set": { "file_id": survivor_id } },
                    )
                    .await?;
            }
            // Hard-delete the loser File documents. Their on-disk bytes
            // live at the same `storage_path` as the survivor so the
            // file is unaffected.
            files
                .delete_many(
                    doc! { "_id": { "$in": losers.iter().copied().collect::<Vec<_>>() } },
                )
                .await?;
        }

        total_excess += losers.len() as u64;
        total_versions_repointed += version_count;
        total_files_deleted += losers.len() as u64;
    }

    println!();
    if dry_run {
        println!(
            "Dry run — would remove {} duplicate row(s) across {} group(s) \
             and re-point {} version(s).",
            total_excess,
            groups.len(),
            total_versions_repointed,
        );
        println!("Re-run without --dry-run to apply.");
    } else {
        println!(
            "Done. Removed {} duplicate row(s) across {} group(s); \
             re-pointed {} version(s).",
            total_files_deleted,
            groups.len(),
            total_versions_repointed,
        );
        println!("Now restart the server — setup_indexes will install the partial unique index.");
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

    // Refuse to start if a migration is in progress. A stale lock blocks too —
    // user must run `uncloud-server migrate --force-unlock` to clear it.
    if let Err(msg) = uncloud_server::migrate::check_no_active_migration(&db).await {
        return Err(format!("Refusing to start: {msg}").into());
    }

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
