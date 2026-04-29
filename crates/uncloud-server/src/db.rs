use mongodb::{Client, Database, IndexModel, options::IndexOptions};
use crate::config::{DatabaseConfig, SyncAuditConfig};
use crate::error::{AppError, Result};

pub async fn connect(config: &DatabaseConfig) -> Result<Database> {
    let client = Client::with_uri_str(&config.uri)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to connect to MongoDB: {}", e)))?;

    // Verify connection
    client
        .database("admin")
        .run_command(mongodb::bson::doc! { "ping": 1 })
        .await
        .map_err(|e| AppError::Internal(format!("Failed to ping MongoDB: {}", e)))?;

    let db = client.database(&config.name);

    tracing::info!("Connected to MongoDB database: {}", config.name);

    Ok(db)
}

pub async fn setup_indexes(db: &Database) -> Result<()> {
    // Users indexes
    let users = db.collection::<mongodb::bson::Document>("users");
    users
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "username": 1 })
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;
    // Drop the old non-partial email index if it exists, then create a partial
    // unique index that only covers documents where email is a non-null string.
    // This allows multiple users to have no email (null) without collisions.
    let _ = users.drop_index("email_1").await;
    users
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "email": 1 })
                .options(
                    IndexOptions::builder()
                        .unique(true)
                        .partial_filter_expression(mongodb::bson::doc! {
                            "email": { "$type": "string" }
                        })
                        .build(),
                )
                .build(),
        )
        .await?;

    // Sessions indexes
    let sessions = db.collection::<mongodb::bson::Document>("sessions");
    sessions
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "token": 1 })
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;
    sessions
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "user_id": 1 })
                .build(),
        )
        .await?;
    sessions
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "expires_at": 1 })
                .options(
                    IndexOptions::builder()
                        .expire_after(std::time::Duration::from_secs(0))
                        .build(),
                )
                .build(),
        )
        .await?;

    // Files indexes
    let files = db.collection::<mongodb::bson::Document>("files");
    files
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "owner_id": 1, "parent_id": 1 })
                .build(),
        )
        .await?;
    files
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "storage_id": 1, "storage_path": 1 })
                .build(),
        )
        .await?;
    // Live-file uniqueness — the on-disk layout (`{username}/{chain}/{name}`)
    // assumes one document per logical path. A *partial* index restricts
    // uniqueness to live (non-soft-deleted) rows so trash entries don't
    // block re-using a name. If creation fails, the most likely cause is
    // residual duplicates from before this constraint existed — run
    // `uncloud-server dedupe-files` once before re-starting.
    files
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "owner_id": 1, "parent_id": 1, "name": 1 })
                .options(
                    IndexOptions::builder()
                        .unique(true)
                        .partial_filter_expression(
                            mongodb::bson::doc! { "deleted_at": mongodb::bson::Bson::Null },
                        )
                        .build(),
                )
                .build(),
        )
        .await?;

    // Folders indexes
    let folders = db.collection::<mongodb::bson::Document>("folders");
    folders
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "owner_id": 1, "parent_id": 1, "name": 1 })
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;

    // Shares indexes
    let shares = db.collection::<mongodb::bson::Document>("shares");
    shares
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "token": 1 })
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;
    shares
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "owner_id": 1 })
                .build(),
        )
        .await?;

    // Upload chunks indexes
    let upload_chunks = db.collection::<mongodb::bson::Document>("upload_chunks");
    upload_chunks
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "upload_id": 1 })
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;
    upload_chunks
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "created_at": 1 })
                .options(
                    IndexOptions::builder()
                        .expire_after(std::time::Duration::from_secs(24 * 60 * 60))
                        .build(),
                )
                .build(),
        )
        .await?;

    // Storages indexes
    let storages = db.collection::<mongodb::bson::Document>("storages");
    storages
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "name": 1 })
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;

    // API tokens indexes
    let api_tokens = db.collection::<mongodb::bson::Document>("api_tokens");
    api_tokens
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "token_hash": 1 })
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;
    api_tokens
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "user_id": 1 })
                .build(),
        )
        .await?;

    // S3 credentials indexes
    let s3_creds = db.collection::<mongodb::bson::Document>("s3_credentials");
    s3_creds
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "access_key_id": 1 })
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;
    s3_creds
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "user_id": 1 })
                .build(),
        )
        .await?;

    // SFTP host-key TOFU pin (one row per storage_id).
    let sftp_keys = db.collection::<mongodb::bson::Document>("sftp_host_keys");
    sftp_keys
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "storage_id": 1 })
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;

    // Apps indexes
    let apps = db.collection::<mongodb::bson::Document>("apps");
    apps.create_index(
        IndexModel::builder()
            .keys(mongodb::bson::doc! { "name": 1 })
            .options(IndexOptions::builder().unique(true).build())
            .build(),
    )
    .await?;

    // Webhooks indexes
    let webhooks = db.collection::<mongodb::bson::Document>("webhooks");
    webhooks
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "app_name": 1 })
                .build(),
        )
        .await?;
    webhooks
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "events": 1 })
                .build(),
        )
        .await?;

    // Shopping indexes
    let shopping_items = db.collection::<mongodb::bson::Document>("shopping_items");
    shopping_items
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "owner_id": 1, "name": 1 })
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;

    let shopping_lists = db.collection::<mongodb::bson::Document>("shopping_lists");
    shopping_lists
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "owner_id": 1, "name": 1 })
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;
    shopping_lists
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "shared_with": 1 })
                .build(),
        )
        .await?;

    let shopping_list_items = db.collection::<mongodb::bson::Document>("shopping_list_items");
    shopping_list_items
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "list_id": 1, "item_id": 1 })
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;
    shopping_list_items
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "list_id": 1, "position": 1 })
                .build(),
        )
        .await?;

    // Shopping categories
    let shopping_categories = db.collection::<mongodb::bson::Document>("shopping_categories");
    shopping_categories
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "owner_id": 1, "name": 1 })
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;
    shopping_categories
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "owner_id": 1, "position": 1 })
                .build(),
        )
        .await?;

    // Shops
    let shops = db.collection::<mongodb::bson::Document>("shops");
    shops
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "owner_id": 1, "name": 1 })
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;

    // Invites indexes
    let invites = db.collection::<mongodb::bson::Document>("invites");
    invites
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "token": 1 })
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;
    invites
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "expires_at": 1 })
                .options(
                    IndexOptions::builder()
                        .expire_after(std::time::Duration::from_secs(0))
                        .build(),
                )
                .build(),
        )
        .await?;

    // TOTP challenges indexes (short-lived, auto-expire)
    let totp_challenges = db.collection::<mongodb::bson::Document>("totp_challenges");
    totp_challenges
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "token": 1 })
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;
    totp_challenges
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "expires_at": 1 })
                .options(
                    IndexOptions::builder()
                        .expire_after(std::time::Duration::from_secs(0))
                        .build(),
                )
                .build(),
        )
        .await?;

    // Folder shares indexes
    let folder_shares = db.collection::<mongodb::bson::Document>("folder_shares");
    folder_shares
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "folder_id": 1, "grantee_id": 1 })
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;
    folder_shares
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "grantee_id": 1 })
                .build(),
        )
        .await?;
    folder_shares
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "owner_id": 1 })
                .build(),
        )
        .await?;

    // Task projects indexes
    let task_projects = db.collection::<mongodb::bson::Document>("task_projects");
    task_projects
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "owner_id": 1 })
                .build(),
        )
        .await?;
    task_projects
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "members.user_id": 1 })
                .build(),
        )
        .await?;

    // Task sections indexes
    let task_sections = db.collection::<mongodb::bson::Document>("task_sections");
    task_sections
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "project_id": 1, "position": 1 })
                .build(),
        )
        .await?;

    // Tasks indexes
    let tasks = db.collection::<mongodb::bson::Document>("tasks");
    tasks
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "project_id": 1, "status": 1, "position": 1 })
                .build(),
        )
        .await?;
    tasks
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "project_id": 1, "section_id": 1, "position": 1 })
                .build(),
        )
        .await?;
    tasks
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "parent_task_id": 1 })
                .build(),
        )
        .await?;
    tasks
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "assignee_id": 1, "due_date": 1 })
                .build(),
        )
        .await?;
    tasks
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "due_date": 1, "status": 1 })
                .build(),
        )
        .await?;

    // Task comments indexes
    let task_comments = db.collection::<mongodb::bson::Document>("task_comments");
    task_comments
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "task_id": 1, "created_at": 1 })
                .build(),
        )
        .await?;

    // Task labels indexes
    let task_labels = db.collection::<mongodb::bson::Document>("task_labels");
    task_labels
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "project_id": 1 })
                .build(),
        )
        .await?;

    tracing::info!("Database indexes created successfully");
    Ok(())
}

/// Creates / refreshes the `sync_events` collection indexes. The TTL expiry
/// must match `SyncAuditConfig::retention_days`, so this is split out from
/// `setup_indexes` and called separately after the config is loaded.
pub async fn setup_sync_audit_indexes(db: &Database, cfg: &SyncAuditConfig) -> Result<()> {
    let sync_events = db.collection::<mongodb::bson::Document>("sync_events");

    sync_events
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "owner_id": 1, "timestamp": -1 })
                .build(),
        )
        .await?;
    sync_events
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "owner_id": 1, "path": 1 })
                .build(),
        )
        .await?;
    sync_events
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "owner_id": 1, "client_id": 1, "timestamp": -1 })
                .build(),
        )
        .await?;

    // TTL index. The retention may change at runtime via config — MongoDB
    // rejects create_index on an existing TTL with different expireAfterSeconds,
    // so drop-and-recreate if needed.
    let ttl_seconds = (cfg.retention_days as u64) * 86_400;
    let ttl_index_name = "timestamp_ttl";
    let _ = sync_events.drop_index(ttl_index_name).await;
    sync_events
        .create_index(
            IndexModel::builder()
                .keys(mongodb::bson::doc! { "timestamp": 1 })
                .options(
                    IndexOptions::builder()
                        .name(ttl_index_name.to_string())
                        .expire_after(std::time::Duration::from_secs(ttl_seconds))
                        .build(),
                )
                .build(),
        )
        .await?;

    Ok(())
}
