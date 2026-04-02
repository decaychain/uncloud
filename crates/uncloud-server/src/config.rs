use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

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

#[derive(Debug, Clone, Deserialize)]
pub struct AuthConfig {
    pub session_duration_hours: u64,
    pub registration_enabled: bool,
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
        match Self::load("config.yaml") {
            Ok(config) => config,
            Err(e) => {
                eprintln!("FATAL: Failed to load config.yaml: {e}");
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
                registration_enabled: true,
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
