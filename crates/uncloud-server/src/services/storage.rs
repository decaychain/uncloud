use mongodb::{bson::doc, bson::oid::ObjectId, Collection, Database};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::StorageConfig;
use crate::error::{AppError, Result};
use crate::models::{Storage, StorageBackendConfig};
use crate::storage::{LocalStorage, StorageBackend};

pub struct StorageService {
    db: Database,
    storages_collection: Collection<Storage>,
    backends: Arc<RwLock<HashMap<ObjectId, Arc<dyn StorageBackend>>>>,
    default_path: String,
}

impl StorageService {
    pub async fn new(db: &Database, config: &StorageConfig) -> Result<Self> {
        let service = Self {
            db: db.clone(),
            storages_collection: db.collection("storages"),
            backends: Arc::new(RwLock::new(HashMap::new())),
            default_path: config.default_path.to_string_lossy().to_string(),
        };

        // Initialize storage backends
        service.initialize_backends().await?;

        Ok(service)
    }

    async fn initialize_backends(&self) -> Result<()> {
        let mut cursor = self.storages_collection.find(doc! {}).await?;
        let mut backends = self.backends.write().await;

        while cursor.advance().await? {
            let storage: Storage = cursor.deserialize_current()?;
            if let Ok(backend) = self.create_backend(&storage.config).await {
                backends.insert(storage.id, backend);
            }
        }

        Ok(())
    }

    async fn create_backend(
        &self,
        config: &StorageBackendConfig,
    ) -> Result<Arc<dyn StorageBackend>> {
        match config {
            StorageBackendConfig::Local { path } => {
                let storage = LocalStorage::new(path).await?;
                Ok(Arc::new(storage))
            }
            StorageBackendConfig::S3 { .. } => {
                Err(AppError::Internal("S3 storage not yet implemented".to_string()))
            }
        }
    }

    pub async fn get_or_create_default(&self, admin_id: ObjectId) -> Result<Storage> {
        // Check if default storage exists
        if let Some(storage) = self
            .storages_collection
            .find_one(doc! { "is_default": true })
            .await?
        {
            return Ok(storage);
        }

        // Create default storage
        let storage = Storage::new_local(
            "Default Storage".to_string(),
            self.default_path.clone(),
            admin_id,
            true,
        );

        self.storages_collection.insert_one(&storage).await?;

        // Initialize backend
        let backend = self.create_backend(&storage.config).await?;
        self.backends.write().await.insert(storage.id, backend);

        Ok(storage)
    }

    pub async fn get_storage(&self, id: ObjectId) -> Result<Storage> {
        self.storages_collection
            .find_one(doc! { "_id": id })
            .await?
            .ok_or_else(|| AppError::NotFound("Storage not found".to_string()))
    }

    pub async fn get_default_storage(&self) -> Result<Storage> {
        self.storages_collection
            .find_one(doc! { "is_default": true })
            .await?
            .ok_or_else(|| AppError::NotFound("No default storage configured".to_string()))
    }

    pub async fn list_storages(&self) -> Result<Vec<Storage>> {
        let mut cursor = self.storages_collection.find(doc! {}).await?;
        let mut storages = Vec::new();
        while cursor.advance().await? {
            storages.push(cursor.deserialize_current()?);
        }
        Ok(storages)
    }

    pub async fn create_storage(
        &self,
        name: String,
        config: StorageBackendConfig,
        is_default: bool,
        created_by: ObjectId,
    ) -> Result<Storage> {
        // If this is the new default, unset others
        if is_default {
            self.storages_collection
                .update_many(doc! {}, doc! { "$set": { "is_default": false } })
                .await?;
        }

        let backend_type = match &config {
            StorageBackendConfig::Local { .. } => crate::models::StorageBackendType::Local,
            StorageBackendConfig::S3 { .. } => crate::models::StorageBackendType::S3,
        };

        let storage = Storage {
            id: ObjectId::new(),
            name,
            backend_type,
            config: config.clone(),
            is_default,
            created_by,
            created_at: chrono::Utc::now(),
        };

        // Test creating the backend
        let backend = self.create_backend(&config).await?;

        self.storages_collection.insert_one(&storage).await?;
        self.backends.write().await.insert(storage.id, backend);

        Ok(storage)
    }

    pub async fn delete_storage(&self, id: ObjectId) -> Result<()> {
        // Check if storage has files
        let files_collection = self.db.collection::<mongodb::bson::Document>("files");
        let count = files_collection
            .count_documents(doc! { "storage_id": id })
            .await?;

        if count > 0 {
            return Err(AppError::Conflict(
                "Cannot delete storage with existing files".to_string(),
            ));
        }

        self.storages_collection
            .delete_one(doc! { "_id": id })
            .await?;
        self.backends.write().await.remove(&id);

        Ok(())
    }

    pub async fn get_backend(&self, storage_id: ObjectId) -> Result<Arc<dyn StorageBackend>> {
        let backends = self.backends.read().await;
        backends
            .get(&storage_id)
            .cloned()
            .ok_or_else(|| AppError::NotFound("Storage backend not found".to_string()))
    }

    pub async fn reload_backend(&self, storage_id: ObjectId) -> Result<()> {
        let storage = self.get_storage(storage_id).await?;
        let backend = self.create_backend(&storage.config).await?;
        self.backends.write().await.insert(storage_id, backend);
        Ok(())
    }
}
