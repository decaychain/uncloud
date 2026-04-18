use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

pub use uncloud_common::RegistrationMode;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub storage: StorageConfig,
    pub auth: AuthConfig,
    pub uploads: UploadConfig,
    #[serde(default)]
    pub processing: ProcessingConfig,
    #[serde(default)]
    pub search: SearchConfig,
    #[serde(default)]
    pub versioning: VersioningConfig,
    #[serde(default)]
    pub apps: AppsConfig,
    #[serde(default)]
    pub features: FeaturesConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    pub uri: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    pub default_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub session_duration_hours: u64,
    pub registration: RegistrationMode,
    pub demo_quota_bytes: i64,
    pub demo_ttl_hours: u64,
}

/// Raw deserialization target that accepts both old (`registration_enabled: bool`)
/// and new (`registration: mode`) config fields.
#[derive(Deserialize)]
struct AuthConfigRaw {
    session_duration_hours: u64,
    #[serde(default)]
    registration: Option<RegistrationMode>,
    #[serde(default)]
    registration_enabled: Option<bool>,
    #[serde(default = "default_demo_quota")]
    demo_quota_bytes: i64,
    #[serde(default = "default_demo_ttl_hours")]
    demo_ttl_hours: u64,
}

impl<'de> serde::Deserialize<'de> for AuthConfig {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = AuthConfigRaw::deserialize(deserializer)?;
        let registration = match (raw.registration, raw.registration_enabled) {
            (Some(mode), _) => mode,
            (None, Some(true)) => RegistrationMode::Open,
            (None, Some(false)) => RegistrationMode::Disabled,
            (None, None) => RegistrationMode::Open,
        };
        Ok(AuthConfig {
            session_duration_hours: raw.session_duration_hours,
            registration,
            demo_quota_bytes: raw.demo_quota_bytes,
            demo_ttl_hours: raw.demo_ttl_hours,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct UploadConfig {
    pub max_chunk_size: u64,
    pub max_file_size: u64,
    pub temp_cleanup_hours: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProcessingConfig {
    pub max_concurrency: usize,
    pub thumbnail_size: u32,
    pub max_attempts: u32,
    /// Maximum input pixel count (width × height) the thumbnail processor will
    /// accept. Raise this to support higher-resolution cameras at the cost of
    /// peak memory use during decode.
    #[serde(default = "default_thumbnail_max_pixels")]
    pub thumbnail_max_pixels: u64,
}

fn default_thumbnail_max_pixels() -> u64 {
    // 200 megapixels — fits 50MP phone bursts and common 100MP cameras.
    200_000_000
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_search_url")]
    pub url: String,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VersioningConfig {
    #[serde(default = "default_max_versions")]
    pub max_versions: u32,
    #[serde(default = "default_trash_retention_days")]
    pub trash_retention_days: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AppsConfig {
    pub registration_secret: Option<String>,
    #[serde(default)]
    pub managed: Vec<ManagedApp>,
}

impl Default for AppsConfig {
    fn default() -> Self {
        Self {
            registration_secret: None,
            managed: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum RestartPolicy {
    Always,
    OnFailure,
    Never,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        RestartPolicy::OnFailure
    }
}

/// Accepts either a plain string (`"cargo run -p foo"`) or a list (`["./my-app", "--port", "8082"]`).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum CommandSpec {
    String(String),
    List(Vec<String>),
}

impl CommandSpec {
    /// Returns `(program, args)`.
    pub fn parts(&self) -> (String, Vec<String>) {
        match self {
            CommandSpec::String(s) => {
                let mut parts = s.split_whitespace().map(str::to_string);
                let prog = parts.next().unwrap_or_default();
                (prog, parts.collect())
            }
            CommandSpec::List(v) if v.is_empty() => (String::new(), vec![]),
            CommandSpec::List(v) => (v[0].clone(), v[1..].to_vec()),
        }
    }

    pub fn display(&self) -> String {
        match self {
            CommandSpec::String(s) => s.clone(),
            CommandSpec::List(v) => v.join(" "),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ManagedApp {
    pub name: String,
    pub command: CommandSpec,
    pub working_dir: Option<PathBuf>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub restart: RestartPolicy,
    #[serde(default)]
    pub restart_max_attempts: u32,
    #[serde(default = "default_backoff_secs")]
    pub restart_backoff_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FeaturesConfig {
    #[serde(default = "default_true")]
    pub shopping: bool,
}

fn default_true() -> bool {
    true
}

impl Default for FeaturesConfig {
    fn default() -> Self {
        Self { shopping: true }
    }
}

fn default_demo_quota() -> i64 {
    50 * 1024 * 1024 // 50MB
}

fn default_demo_ttl_hours() -> u64 {
    24
}

fn default_backoff_secs() -> u64 {
    2
}

fn default_max_versions() -> u32 {
    50
}

fn default_trash_retention_days() -> u32 {
    30
}

impl Default for VersioningConfig {
    fn default() -> Self {
        Self {
            max_versions: default_max_versions(),
            trash_retention_days: default_trash_retention_days(),
        }
    }
}

fn default_search_url() -> String {
    "http://localhost:7700".to_string()
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: default_search_url(),
            api_key: None,
        }
    }
}

impl Default for ProcessingConfig {
    fn default() -> Self {
        Self {
            max_concurrency: 4,
            thumbnail_size: 320,
            max_attempts: 3,
            thumbnail_max_pixels: default_thumbnail_max_pixels(),
        }
    }
}

impl Config {
    pub fn load(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let contents = std::fs::read_to_string(path)?;
        let config: Config = serde_yaml::from_str(&contents)?;
        Ok(config)
    }

    pub fn load_or_default() -> Self {
        let path = std::env::var("CONFIG_PATH").unwrap_or_else(|_| "config.yaml".to_string());
        match Self::load(&path) {
            Ok(config) => config,
            Err(e) => {
                eprintln!("FATAL: Failed to load {path}: {e}");
                std::process::exit(1);
            }
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                host: "0.0.0.0".to_string(),
                port: 8080,
            },
            database: DatabaseConfig {
                uri: "mongodb://localhost:27017".to_string(),
                name: "uncloud".to_string(),
            },
            storage: StorageConfig {
                default_path: PathBuf::from("/data/uncloud"),
            },
            auth: AuthConfig {
                session_duration_hours: 168,
                registration: RegistrationMode::Open,
                demo_quota_bytes: default_demo_quota(),
                demo_ttl_hours: default_demo_ttl_hours(),
            },
            uploads: UploadConfig {
                max_chunk_size: 10 * 1024 * 1024, // 10MB
                max_file_size: 0,                  // unlimited
                temp_cleanup_hours: 24,
            },
            processing: ProcessingConfig::default(),
            search: SearchConfig::default(),
            versioning: VersioningConfig::default(),
            apps: AppsConfig::default(),
            features: FeaturesConfig::default(),
        }
    }
}
