#![allow(dead_code)]

use std::sync::OnceLock;
use std::sync::Arc;

use axum_test::{TestServer, TestServerConfig};
use serde_json::Value;
use tempfile::TempDir;
use testcontainers::{GenericImage, ImageExt, runners::AsyncRunner};
use uncloud_client::Client;

use uncloud_server::{
    AppState,
    config::{
        AppsConfig, AuthConfig, Config, DatabaseConfig, FeaturesConfig, ProcessingConfig,
        SearchConfig, ServerConfig, StorageConfig, UploadConfig, VersioningConfig,
    },
    db,
    processing::ProcessingService,
    routes,
    services::{AuthService, EventService, RescanService, SearchService, StorageService},
};

// Shared MongoDB container — started once per test binary, port stored here.
static MONGO_PORT: OnceLock<u16> = OnceLock::new();

// Limits concurrent databases to avoid exhausting file descriptors in the container.
static DB_SEMAPHORE: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();

fn db_semaphore() -> Arc<tokio::sync::Semaphore> {
    DB_SEMAPHORE
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(4)))
        .clone()
}

fn mongo_port() -> u16 {
    *MONGO_PORT.get_or_init(|| {
        // Must run in a fresh OS thread: `#[tokio::test]` already holds a
        // runtime, and creating a second one on the same thread panics.
        std::thread::spawn(|| {
            let rt = tokio::runtime::Runtime::new().expect("runtime for container start");
            rt.block_on(async {
                use testcontainers::core::WaitFor;
                let container = GenericImage::new("mongo", "8")
                    .with_exposed_port(27017.into())
                    .with_wait_for(WaitFor::message_on_stdout("Waiting for connections"))
                    .with_cmd(vec![
                        "mongod",
                        "--wiredTigerCacheSizeGB",
                        "0.25",
                        "--bind_ip_all",
                    ])
                    .with_ulimit("nofile", 65536, Some(65536))
                    .start()
                    .await
                    .expect("MongoDB container failed to start");
                let port = container
                    .get_host_port_ipv4(27017)
                    .await
                    .expect("failed to get container port");
                // Leak the container so it stays alive for the whole test binary.
                Box::leak(Box::new(container));
                port
            })
        })
        .join()
        .expect("container startup thread")
    })
}

pub struct TestApp {
    pub server: TestServer,
    pub db: mongodb::Database,
    /// Kept alive so the temp directory is not deleted before the test ends.
    _storage: TempDir,
    /// Held until cleanup — limits concurrent databases in the container.
    _db_permit: tokio::sync::OwnedSemaphorePermit,
}

impl TestApp {
    /// Create a TestApp with a custom registration mode.
    pub async fn with_registration(mode: uncloud_server::config::RegistrationMode) -> Self {
        Self::with_config(|config| {
            config.auth.registration = mode;
        })
        .await
    }

    /// Create a TestApp with custom config modifications.
    async fn with_config(customize: impl FnOnce(&mut Config)) -> Self {
        let permit = db_semaphore()
            .acquire_owned()
            .await
            .expect("db semaphore permit");
        let port = mongo_port();
        let db_name = format!("uncloud_test_{}", uuid::Uuid::new_v4().simple());
        let storage_dir = TempDir::new().expect("temp dir");

        let mut config = Config {
            server: ServerConfig {
                host: "127.0.0.1".to_string(),
                port: 0,
            },
            database: DatabaseConfig {
                uri: format!("mongodb://127.0.0.1:{}", port),
                name: db_name,
            },
            storage: StorageConfig {
                default_path: Some(storage_dir.path().to_path_buf()), storages: Vec::new(), default: None,
            },
            auth: AuthConfig {
                session_duration_hours: 1,
                registration: uncloud_server::config::RegistrationMode::Open,
                demo_quota_bytes: 50 * 1024 * 1024,
                demo_ttl_hours: 24,
            },
            uploads: UploadConfig {
                max_chunk_size: 10 * 1024 * 1024,
                max_file_size: 0,
                temp_cleanup_hours: 24,
            },
            processing: ProcessingConfig::default(),
            search: SearchConfig::default(),
            versioning: VersioningConfig::default(),
            apps: AppsConfig::default(),
            features: FeaturesConfig::default(),
            logging: uncloud_server::config::LoggingConfig::default(),
            sync_audit: uncloud_server::config::SyncAuditConfig::default(),
            backup: uncloud_server::backup::config::BackupConfig::default(),
        };

        customize(&mut config);

        let database = db::connect(&config.database)
            .await
            .expect("connect to test MongoDB");
        db::setup_indexes(&database)
            .await
            .expect("setup indexes");

        let auth = AuthService::new(&database, config.auth.clone());
        let storage = StorageService::new(&database, &config.storage)
            .await
            .expect("storage service");
        let events = EventService::new();
        let search = SearchService::new(&config.search)
            .await
            .expect("search service");
        let processing = ProcessingService::new(1, 3);

        let db_handle = database.clone();

        let sync_log = uncloud_server::services::SyncLog::new(&database, events.clone(), config.sync_audit.enabled);
        let state = Arc::new(AppState {
            config,
            db: database,
            auth,
            storage,
            events,
            processing,
            search,
            rescan: RescanService::new(),
            sync_log,
            http_client: reqwest::Client::new(),
        });

        let router = routes::create_router(state);
        let server = TestServer::new_with_config(
            router,
            TestServerConfig {
                save_cookies: true,
                ..Default::default()
            },
        )
        .expect("TestServer");

        Self {
            server,
            db: db_handle,
            _storage: storage_dir,
            _db_permit: permit,
        }
    }

    pub async fn new() -> Self {
        Self::with_config(|_| {}).await
    }

    /// Drop the test database to free MongoDB memory.
    /// Call this at the end of each test.
    pub async fn cleanup(&self) {
        self.db.drop().await.ok();
    }

    /// Register a new user and return the parsed response body.
    pub async fn register(&self, username: &str, email: &str, password: &str) -> Value {
        self.server
            .post("/api/auth/register")
            .json(&serde_json::json!({
                "username": username,
                "email": email,
                "password": password
            }))
            .await
            .json()
    }

    /// Log in. On success the session cookie is stored in the TestServer cookie jar.
    pub async fn login(&self, username: &str, password: &str) -> Value {
        self.server
            .post("/api/auth/login")
            .json(&serde_json::json!({
                "username": username,
                "password": password
            }))
            .await
            .json()
    }

    /// Register then immediately log in. Returns the user response body.
    pub async fn register_and_login(&self, username: &str) -> Value {
        let email = format!("{}@example.com", username);
        self.register(username, &email, "password123!").await;
        self.login(username, "password123!").await
    }

    /// Upload a file and return the FileResponse JSON.
    pub async fn upload(&self, filename: &str, content: &[u8], mime: &str) -> Value {
        use axum_test::multipart::{MultipartForm, Part};
        let form = MultipartForm::new()
            .add_part("file", Part::bytes(content.to_vec()).file_name(filename).mime_type(mime));
        self.server
            .post("/api/uploads/simple")
            .multipart(form)
            .await
            .json()
    }

    /// Upload a file into a specific folder and return the FileResponse JSON.
    pub async fn upload_to_folder(&self, filename: &str, content: &[u8], mime: &str, parent_id: &str) -> Value {
        use axum_test::multipart::{MultipartForm, Part};
        let form = MultipartForm::new()
            .add_part("file", Part::bytes(content.to_vec()).file_name(filename).mime_type(mime))
            .add_part("parent_id", Part::text(parent_id.to_string()));
        self.server
            .post("/api/uploads/simple")
            .multipart(form)
            .await
            .json()
    }

    /// Create an admin user directly in the DB, then log in via the API.
    /// Works regardless of registration mode.
    pub async fn create_admin_and_login(&self, username: &str, password: &str) -> Value {
        use uncloud_server::services::AuthService;

        // Hash password using the same argon2 logic as the server
        let auth_config = uncloud_server::config::AuthConfig {
            session_duration_hours: 1,
            registration: uncloud_server::config::RegistrationMode::Open,
            demo_quota_bytes: 0,
            demo_ttl_hours: 24,
        };
        let temp_auth = AuthService::new(&self.db, auth_config);
        let hash = temp_auth.hash_password(password).expect("hash password");

        // Insert admin user directly
        let collection = self.db.collection::<mongodb::bson::Document>("users");
        let now = mongodb::bson::DateTime::now();
        collection
            .insert_one(mongodb::bson::doc! {
                "username": username,
                "email": mongodb::bson::Bson::Null,
                "password_hash": hash,
                "role": "admin",
                "status": "active",
                "quota_bytes": mongodb::bson::Bson::Null,
                "used_bytes": 0_i64,
                "totp_enabled": false,
                "totp_secret": mongodb::bson::Bson::Null,
                "recovery_codes": [],
                "demo": false,
                "disabled_features": [],
                "created_at": now,
                "updated_at": now,
            })
            .await
            .expect("insert admin user");

        self.login(username, password).await
    }
}

// ── BoundTestApp ──────────────────────────────────────────────────────────────
//
// Like TestApp but binds to a real TCP port so that `uncloud_client::Client`
// (which uses reqwest) can talk to the server over HTTP.

pub struct BoundTestApp {
    pub base_url: String,
    _storage: TempDir,
    /// Dropping this sender triggers graceful shutdown of the background server.
    _shutdown: tokio::sync::oneshot::Sender<()>,
}

impl BoundTestApp {
    pub async fn new() -> Self {
        let port = mongo_port();
        let db_name = format!("uncloud_test_{}", uuid::Uuid::new_v4().simple());
        let storage_dir = TempDir::new().expect("temp dir");

        let config = Config {
            server: ServerConfig {
                host: "127.0.0.1".to_string(),
                port: 0,
            },
            database: DatabaseConfig {
                uri: format!("mongodb://127.0.0.1:{}", port),
                name: db_name,
            },
            storage: StorageConfig {
                default_path: Some(storage_dir.path().to_path_buf()), storages: Vec::new(), default: None,
            },
            auth: AuthConfig {
                session_duration_hours: 1,
                registration: uncloud_server::config::RegistrationMode::Open,
                demo_quota_bytes: 50 * 1024 * 1024,
                demo_ttl_hours: 24,
            },
            uploads: UploadConfig {
                max_chunk_size: 10 * 1024 * 1024,
                max_file_size: 0,
                temp_cleanup_hours: 24,
            },
            processing: ProcessingConfig::default(),
            search: SearchConfig::default(),
            versioning: VersioningConfig::default(),
            apps: AppsConfig::default(),
            features: FeaturesConfig::default(),
            logging: uncloud_server::config::LoggingConfig::default(),
            sync_audit: uncloud_server::config::SyncAuditConfig::default(),
            backup: uncloud_server::backup::config::BackupConfig::default(),
        };

        let database = db::connect(&config.database)
            .await
            .expect("connect to test MongoDB");
        db::setup_indexes(&database)
            .await
            .expect("setup indexes");

        let auth = AuthService::new(&database, config.auth.clone());
        let storage = StorageService::new(&database, &config.storage)
            .await
            .expect("storage service");
        let events = EventService::new();
        let search = SearchService::new(&config.search)
            .await
            .expect("search service");
        let processing = ProcessingService::new(1, 3);

        let sync_log = uncloud_server::services::SyncLog::new(&database, events.clone(), config.sync_audit.enabled);
        let state = Arc::new(AppState {
            config,
            db: database,
            auth,
            storage,
            events,
            processing,
            search,
            rescan: RescanService::new(),
            sync_log,
            http_client: reqwest::Client::new(),
        });

        let router = uncloud_server::routes::create_router(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind TCP listener");
        let bound_port = listener.local_addr().expect("local_addr").port();

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async { shutdown_rx.await.ok(); })
                .await
                .ok();
        });

        Self {
            base_url: format!("http://127.0.0.1:{}", bound_port),
            _storage: storage_dir,
            _shutdown: shutdown_tx,
        }
    }

    /// Create a fresh, unauthenticated client for this server.
    pub fn client(&self) -> Arc<Client> {
        Arc::new(Client::new(&self.base_url))
    }

    /// Register a new user (via raw HTTP) and return a logged-in client.
    pub async fn setup_user(&self, username: &str) -> Arc<Client> {
        let http = reqwest::Client::new();
        http.post(format!("{}/api/auth/register", self.base_url))
            .json(&serde_json::json!({
                "username": username,
                "email": format!("{}@example.com", username),
                "password": "password123!"
            }))
            .send()
            .await
            .expect("register request");

        self.login_client(username, "password123!").await
    }

    /// Create a fresh client and log in as an existing user.
    pub async fn login_client(&self, username: &str, password: &str) -> Arc<Client> {
        let client = Client::new(&self.base_url);
        client.login(username, password).await.expect("login");
        Arc::new(client)
    }

    /// Create a `SyncEngine` backed by a fresh temp directory.
    /// Returns `(engine, sync_dir)` — caller must keep `sync_dir` alive.
    pub async fn new_sync_engine(
        &self,
        client: Arc<Client>,
    ) -> (uncloud_sync::SyncEngine, TempDir) {
        let sync_dir = TempDir::new().expect("sync dir");
        let db_path = sync_dir.path().join(".uncloud-sync.db");
        let engine = uncloud_sync::SyncEngine::new(
            &db_path,
            client,
            Some(sync_dir.path().to_string_lossy().into_owned()),
        )
        .await
        .expect("SyncEngine::new");
        (engine, sync_dir)
    }
}
