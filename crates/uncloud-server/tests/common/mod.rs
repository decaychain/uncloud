#![allow(dead_code)]

use std::sync::OnceLock;
use std::sync::Arc;

use axum_test::{TestServer, TestServerConfig};
use serde_json::Value;
use tempfile::TempDir;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::mongo::Mongo;
use uncloud_client::Client;

use uncloud_server::{
    AppState,
    config::{AuthConfig, Config, DatabaseConfig, ProcessingConfig, ServerConfig, StorageConfig, UploadConfig},
    db,
    processing::ProcessingService,
    routes,
    services::{AuthService, EventService, StorageService},
};

// Shared MongoDB container — started once per test binary, port stored here.
static MONGO_PORT: OnceLock<u16> = OnceLock::new();

fn mongo_port() -> u16 {
    *MONGO_PORT.get_or_init(|| {
        // Must run in a fresh OS thread: `#[tokio::test]` already holds a
        // runtime, and creating a second one on the same thread panics.
        std::thread::spawn(|| {
            let rt = tokio::runtime::Runtime::new().expect("runtime for container start");
            rt.block_on(async {
                let container = Mongo::default()
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
}

impl TestApp {
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
                default_path: storage_dir.path().to_path_buf(),
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
        // No processors registered — avoids background tasks in tests.
        let processing = ProcessingService::new(1, 3);

        let db_handle = database.clone();

        let state = Arc::new(AppState {
            config,
            db: database,
            auth,
            storage,
            events,
            processing,
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
        }
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

        let config = uncloud_server::config::Config {
            server: uncloud_server::config::ServerConfig {
                host: "127.0.0.1".to_string(),
                port: 0,
            },
            database: uncloud_server::config::DatabaseConfig {
                uri: format!("mongodb://127.0.0.1:{}", port),
                name: db_name,
            },
            storage: uncloud_server::config::StorageConfig {
                default_path: storage_dir.path().to_path_buf(),
            },
            auth: uncloud_server::config::AuthConfig {
                session_duration_hours: 1,
                registration: uncloud_server::config::RegistrationMode::Open,
                demo_quota_bytes: 50 * 1024 * 1024,
                demo_ttl_hours: 24,
            },
            uploads: uncloud_server::config::UploadConfig {
                max_chunk_size: 10 * 1024 * 1024,
                max_file_size: 0,
                temp_cleanup_hours: 24,
            },
            processing: uncloud_server::config::ProcessingConfig::default(),
        };

        let database = uncloud_server::db::connect(&config.database)
            .await
            .expect("connect to test MongoDB");
        uncloud_server::db::setup_indexes(&database)
            .await
            .expect("setup indexes");

        let auth = uncloud_server::services::AuthService::new(&database, config.auth.clone());
        let storage = uncloud_server::services::StorageService::new(&database, &config.storage)
            .await
            .expect("storage service");
        let events = uncloud_server::services::EventService::new();
        let processing = uncloud_server::processing::ProcessingService::new(1, 3);

        let state = Arc::new(uncloud_server::AppState {
            config,
            db: database,
            auth,
            storage,
            events,
            processing,
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
            sync_dir.path().to_path_buf(),
        )
        .await
        .expect("SyncEngine::new");
        (engine, sync_dir)
    }
}
