use mongodb::{bson::doc, bson::oid::ObjectId, Collection, Database};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::{ConfiguredStorageBackend, StorageConfig};
use crate::error::{AppError, Result};
use crate::models::{Storage, StorageBackendConfig, StorageBackendType};
use crate::storage::{LocalStorage, S3Storage, StorageBackend};

pub struct StorageService {
    db: Database,
    storages_collection: Collection<Storage>,
    backends: Arc<RwLock<HashMap<ObjectId, Arc<dyn StorageBackend>>>>,
    default_storage_id: ObjectId,
    /// Storages by name, captured at startup. Used for `GET /api/storages` and
    /// when looking up the configured default.
    by_name: Arc<RwLock<HashMap<String, ObjectId>>>,
}

impl StorageService {
    pub async fn new(db: &Database, config: &StorageConfig) -> Result<Self> {
        let resolved = config
            .resolve()
            .map_err(|e| AppError::Internal(format!("Invalid storage config: {e}")))?;
        let storages_collection: Collection<Storage> = db.collection("storages");

        // Pull existing rows so we can upsert by `name` and detect orphans.
        let existing: Vec<Storage> = {
            let mut cur = storages_collection.find(doc! {}).await?;
            let mut out = Vec::new();
            while cur.advance().await? {
                out.push(cur.deserialize_current()?);
            }
            out
        };
        let existing_by_name: HashMap<String, Storage> = existing
            .iter()
            .cloned()
            .map(|s| (s.name.clone(), s))
            .collect();
        let configured_names: HashSet<&str> = resolved.entries.iter().map(|e| e.name.as_str()).collect();

        // Reject if removing a storage that still has files referencing it.
        let files_collection = db.collection::<mongodb::bson::Document>("files");
        for storage in existing.iter() {
            if !configured_names.contains(storage.name.as_str()) {
                let count = files_collection
                    .count_documents(doc! { "storage_id": storage.id })
                    .await?;
                if count > 0 {
                    return Err(AppError::Internal(format!(
                        "Storage `{}` was removed from config but still has {count} file(s) on it. \
                         Restore the entry in config.yaml or move/delete those files first.",
                        storage.name
                    )));
                }
                // No files — silently drop the orphan row.
                storages_collection
                    .delete_one(doc! { "_id": storage.id })
                    .await?;
            }
        }

        // Upsert each configured storage and build live backends.
        let mut backends: HashMap<ObjectId, Arc<dyn StorageBackend>> = HashMap::new();
        let mut by_name: HashMap<String, ObjectId> = HashMap::new();
        let mut default_storage_id: Option<ObjectId> = None;

        for entry in &resolved.entries {
            let backend_config: StorageBackendConfig = entry.backend.clone().into();
            let is_default = entry.name == resolved.default;

            let storage = match existing_by_name.get(&entry.name) {
                Some(prev) => {
                    // Update fields if they drifted.
                    let new = Storage {
                        id: prev.id,
                        name: entry.name.clone(),
                        backend_type: match &backend_config {
                            StorageBackendConfig::Local { .. } => StorageBackendType::Local,
                            StorageBackendConfig::S3 { .. } => StorageBackendType::S3,
                            StorageBackendConfig::Sftp { .. } => StorageBackendType::Sftp,
                        },
                        config: backend_config.clone(),
                        is_default,
                        created_by: prev.created_by,
                        created_at: prev.created_at,
                    };
                    storages_collection
                        .replace_one(doc! { "_id": prev.id }, &new)
                        .await?;
                    new
                }
                None => {
                    let new = Storage {
                        id: ObjectId::new(),
                        name: entry.name.clone(),
                        backend_type: match &backend_config {
                            StorageBackendConfig::Local { .. } => StorageBackendType::Local,
                            StorageBackendConfig::S3 { .. } => StorageBackendType::S3,
                            StorageBackendConfig::Sftp { .. } => StorageBackendType::Sftp,
                        },
                        config: backend_config.clone(),
                        is_default,
                        created_by: ObjectId::new(),
                        created_at: chrono::Utc::now(),
                    };
                    storages_collection.insert_one(&new).await?;
                    new
                }
            };

            let backend = create_backend(&backend_config, &config.retry, db, storage.id).await?;
            backends.insert(storage.id, backend);
            by_name.insert(storage.name.clone(), storage.id);
            if is_default {
                default_storage_id = Some(storage.id);
            }
        }

        let default_storage_id = default_storage_id.ok_or_else(|| {
            AppError::Internal("No default storage resolved (config validation should have caught this)".into())
        })?;

        Ok(Self {
            db: db.clone(),
            storages_collection,
            backends: Arc::new(RwLock::new(backends)),
            default_storage_id,
            by_name: Arc::new(RwLock::new(by_name)),
        })
    }

    pub fn default_storage_id(&self) -> ObjectId {
        self.default_storage_id
    }

    pub async fn get_storage(&self, id: ObjectId) -> Result<Storage> {
        self.storages_collection
            .find_one(doc! { "_id": id })
            .await?
            .ok_or_else(|| AppError::NotFound("Storage not found".to_string()))
    }

    pub async fn get_default_storage(&self) -> Result<Storage> {
        self.get_storage(self.default_storage_id).await
    }

    pub async fn list_storages(&self) -> Result<Vec<Storage>> {
        let mut cursor = self.storages_collection.find(doc! {}).await?;
        let mut storages = Vec::new();
        while cursor.advance().await? {
            storages.push(cursor.deserialize_current()?);
        }
        Ok(storages)
    }

    pub async fn storage_id_by_name(&self, name: &str) -> Option<ObjectId> {
        self.by_name.read().await.get(name).copied()
    }

    /// Walks up the parent chain from `parent_id`, returning the closest
    /// ancestor's pinned `storage_id` if any. Falls back to the configured
    /// default when no ancestor pins one (or when uploading at root).
    pub async fn resolve_storage_for_parent(
        &self,
        parent_id: Option<ObjectId>,
    ) -> Result<ObjectId> {
        let mut current = parent_id;
        let folders = self.db.collection::<crate::models::Folder>("folders");
        // Bound the walk to avoid pathological loops if a parent_id chain
        // ever becomes self-referential — folders are at most a few dozen
        // deep in practice.
        for _ in 0..256 {
            let Some(id) = current else { break };
            let Some(folder) = folders.find_one(doc! { "_id": id }).await? else {
                break;
            };
            if let Some(sid) = folder.storage_id {
                return Ok(sid);
            }
            current = folder.parent_id;
        }
        Ok(self.default_storage_id)
    }

    pub async fn get_backend(&self, storage_id: ObjectId) -> Result<Arc<dyn StorageBackend>> {
        let backends = self.backends.read().await;
        backends
            .get(&storage_id)
            .cloned()
            .ok_or_else(|| AppError::NotFound("Storage backend not found".to_string()))
    }
}

impl From<ConfiguredStorageBackend> for StorageBackendConfig {
    fn from(b: ConfiguredStorageBackend) -> Self {
        match b {
            ConfiguredStorageBackend::Local { path } => StorageBackendConfig::Local {
                path: path.to_string_lossy().into_owned(),
            },
            ConfiguredStorageBackend::S3 {
                endpoint,
                bucket,
                access_key,
                secret_key,
                region,
            } => StorageBackendConfig::S3 {
                endpoint,
                bucket,
                access_key,
                secret_key,
                region,
            },
            ConfiguredStorageBackend::Sftp {
                host,
                port,
                username,
                password,
                private_key,
                private_key_passphrase,
                base_path,
                host_key,
                host_key_check,
            } => StorageBackendConfig::Sftp {
                host,
                port,
                username,
                password,
                private_key,
                private_key_passphrase,
                base_path,
                host_key,
                host_key_check,
            },
        }
    }
}

async fn create_backend(
    config: &StorageBackendConfig,
    retry: &crate::storage::retry::RetryConfig,
    db: &Database,
    storage_id: ObjectId,
) -> Result<Arc<dyn StorageBackend>> {
    match config {
        StorageBackendConfig::Local { path } => {
            let storage = LocalStorage::new(path).await?;
            Ok(Arc::new(storage))
        }
        StorageBackendConfig::S3 {
            endpoint,
            bucket,
            access_key,
            secret_key,
            region,
        } => {
            let storage = S3Storage::new(
                endpoint,
                bucket,
                access_key,
                secret_key,
                region.as_deref(),
                retry.clone(),
            )
            .await?;
            Ok(Arc::new(storage))
        }
        StorageBackendConfig::Sftp { .. } => {
            let storage = crate::storage::SftpStorage::new(
                config,
                retry.clone(),
                db.clone(),
                storage_id,
            )
            .await?;
            Ok(Arc::new(storage))
        }
    }
}
