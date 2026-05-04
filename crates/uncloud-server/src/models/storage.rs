use bson::serde_helpers::chrono_datetime_as_bson_datetime;
use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageBackendType {
    Local,
    S3,
    Sftp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum StorageBackendConfig {
    Local { path: String },
    S3 {
        endpoint: String,
        bucket: String,
        access_key: String,
        secret_key: String,
        region: Option<String>,
    },
    Sftp {
        host: String,
        port: u16,
        username: String,
        #[serde(default)]
        password: Option<String>,
        #[serde(default)]
        private_key: Option<String>,
        #[serde(default)]
        private_key_passphrase: Option<String>,
        base_path: String,
        #[serde(default)]
        host_key: Option<String>,
        #[serde(default)]
        host_key_check: Option<String>,
        /// Number of SSH connections kept warm in the pool. Each
        /// in-flight op uses one; the pool size caps how many ops can
        /// run truly in parallel against the server. Defaults to 2.
        /// Hetzner Storage Box subaccounts cap simultaneous SSH
        /// connections at ~5; raise with care.
        #[serde(default)]
        connection_pool_size: Option<u32>,
        /// Cap on simultaneous in-flight ops (queues at the storage
        /// backend layer). Independent of `connection_pool_size`:
        /// `pool=2, ops=4` means up to 4 ops queue but only 2 hold a
        /// connection at any moment. Defaults to 4.
        #[serde(default)]
        max_concurrent_ops: Option<u32>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Storage {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub name: String,
    pub backend_type: StorageBackendType,
    pub config: StorageBackendConfig,
    pub is_default: bool,
    pub created_by: ObjectId,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
}

impl Storage {
    pub fn new_local(name: String, path: String, created_by: ObjectId, is_default: bool) -> Self {
        Self {
            id: ObjectId::new(),
            name,
            backend_type: StorageBackendType::Local,
            config: StorageBackendConfig::Local { path },
            is_default,
            created_by,
            created_at: Utc::now(),
        }
    }
}
