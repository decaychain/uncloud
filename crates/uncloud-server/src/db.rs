use mongodb::{Client, Database, IndexModel, options::IndexOptions};
use crate::config::DatabaseConfig;
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

    tracing::info!("Database indexes created successfully");
    Ok(())
}
