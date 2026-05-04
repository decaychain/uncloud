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
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub sync_audit: SyncAuditConfig,
    #[serde(default)]
    pub backup: crate::backup::config::BackupConfig,
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
    /// Legacy single-storage shortcut. When `storages` is empty, a storage
    /// named `"local"` is synthesised from this path. Kept for backward
    /// compatibility with pre-1.x configs.
    #[serde(default)]
    pub default_path: Option<PathBuf>,
    /// Configured storage backends. Each entry has a unique `name` referenced
    /// from MongoDB. At least one must exist (either here or via the legacy
    /// `default_path` shortcut).
    #[serde(default)]
    pub storages: Vec<ConfiguredStorage>,
    /// Name of the storage that receives uploads when no folder-level override
    /// applies. Required when `storages` is non-empty.
    #[serde(default)]
    pub default: Option<String>,
    /// Retry policy applied to S3 and SFTP backends for transient errors
    /// (connection resets, throttling, mid-flight timeouts). The local
    /// backend ignores it — kernel VFS handles transient I/O. Idempotent
    /// ops only; mutating ops that consume an input stream skip retry.
    #[serde(default)]
    pub retry: crate::storage::retry::RetryConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConfiguredStorage {
    pub name: String,
    #[serde(flatten)]
    pub backend: ConfiguredStorageBackend,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ConfiguredStorageBackend {
    Local {
        path: PathBuf,
    },
    S3 {
        endpoint: String,
        bucket: String,
        access_key: String,
        secret_key: String,
        #[serde(default)]
        region: Option<String>,
    },
    Sftp {
        host: String,
        #[serde(default = "default_ssh_port")]
        port: u16,
        username: String,
        /// Password auth. Either `password` or `private_key` must be set.
        #[serde(default)]
        password: Option<String>,
        /// PEM-encoded private key (OpenSSH or PKCS#8). Mutually exclusive
        /// with `password`.
        #[serde(default)]
        private_key: Option<String>,
        /// Optional passphrase for `private_key`.
        #[serde(default)]
        private_key_passphrase: Option<String>,
        /// All SFTP paths are resolved relative to this directory on the host.
        base_path: String,
        /// Optional pinned host public key (e.g. "ssh-ed25519 AAAA..."). When
        /// present, takes precedence over the TOFU store — strict mode.
        #[serde(default)]
        host_key: Option<String>,
        /// `tofu` (default), `skip`, or `strict` (implied when `host_key` set).
        /// `skip` logs a warning at startup; use only on trusted networks.
        #[serde(default)]
        host_key_check: Option<String>,
        /// SSH connection pool size. Each in-flight op uses one connection.
        /// Default 2. Hetzner Storage Box subaccounts cap simultaneous SSH
        /// at ~5; raising past 4 will hit that limit on shared-tenant SFTP.
        #[serde(default)]
        connection_pool_size: Option<u32>,
        /// Cap on simultaneous in-flight ops (semaphore at the backend
        /// layer). Default 4. Independent of pool size — pool=2, ops=4
        /// means 4 ops queue but only 2 hold a connection at once.
        #[serde(default)]
        max_concurrent_ops: Option<u32>,
    },
}

fn default_ssh_port() -> u16 {
    22
}

impl StorageConfig {
    /// Returns the list of `(name, backend)` pairs the server should serve,
    /// along with the resolved default name. Synthesises a single `"local"`
    /// entry from `default_path` when `storages` is empty, so old configs
    /// keep working.
    pub fn resolve(&self) -> Result<ResolvedStorages, String> {
        if self.storages.is_empty() {
            let path = self
                .default_path
                .as_ref()
                .ok_or_else(|| {
                    "storage: configure at least one entry under `storages` or set `default_path`"
                        .to_string()
                })?
                .clone();
            return Ok(ResolvedStorages {
                default: "local".to_string(),
                entries: vec![ConfiguredStorage {
                    name: "local".to_string(),
                    backend: ConfiguredStorageBackend::Local { path },
                }],
            });
        }

        let mut seen = std::collections::HashSet::new();
        for s in &self.storages {
            if s.name.trim().is_empty() {
                return Err("storage: every entry needs a non-empty `name`".to_string());
            }
            if !seen.insert(&s.name) {
                return Err(format!("storage: duplicate name `{}`", s.name));
            }
        }

        let default = self.default.clone().ok_or_else(|| {
            "storage: set `default: <name>` when `storages` is non-empty".to_string()
        })?;
        if !self.storages.iter().any(|s| s.name == default) {
            return Err(format!(
                "storage: `default: {}` does not match any configured storage name",
                default
            ));
        }
        Ok(ResolvedStorages {
            default,
            entries: self.storages.clone(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedStorages {
    pub default: String,
    pub entries: Vec<ConfiguredStorage>,
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
pub struct LoggingConfig {
    /// `tracing_subscriber::EnvFilter` directive string. Falls back to
    /// `default_log_level()` (debug builds → `debug`, release builds → `info`).
    /// The `RUST_LOG` env var, when set, always wins over this.
    #[serde(default = "default_log_level")]
    pub level: String,
}

fn default_log_level() -> String {
    if cfg!(debug_assertions) {
        "uncloud_server=debug,tower_http=debug".to_string()
    } else {
        "uncloud_server=info,tower_http=info".to_string()
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
        }
    }
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

#[derive(Debug, Clone, Deserialize)]
pub struct SyncAuditConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_sync_audit_retention_days")]
    pub retention_days: u32,
    #[serde(default = "default_sync_audit_max_records_per_user")]
    pub max_records_per_user: u32,
}

fn default_sync_audit_retention_days() -> u32 {
    7
}

fn default_sync_audit_max_records_per_user() -> u32 {
    10_000
}

impl Default for SyncAuditConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            retention_days: default_sync_audit_retention_days(),
            max_records_per_user: default_sync_audit_max_records_per_user(),
        }
    }
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
        let expanded = expand_env_vars(&contents);
        let config: Config = serde_yaml::from_str(&expanded)?;
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

/// Replaces `${VAR}` occurrences in a config body with the value of `VAR`
/// from the process environment. Unknown vars expand to empty so the YAML
/// deserialiser fails on missing required fields rather than silently
/// keeping the literal `${VAR}` text.
fn expand_env_vars(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'$' && bytes[i + 1] == b'{' {
            if let Some(end) = input[i + 2..].find('}') {
                let var = &input[i + 2..i + 2 + end];
                if !var.is_empty() && var.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    out.push_str(&std::env::var(var).unwrap_or_default());
                    i += 2 + end + 1;
                    continue;
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
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
                default_path: Some(PathBuf::from("/data/uncloud")),
                storages: Vec::new(),
                default: None,
                retry: Default::default(),
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
            logging: LoggingConfig::default(),
            sync_audit: SyncAuditConfig::default(),
            backup: crate::backup::config::BackupConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_env_vars_substitutes_set_vars() {
        std::env::set_var("UC_TEST_FOO", "abc123");
        let out = expand_env_vars("key: ${UC_TEST_FOO}");
        assert_eq!(out, "key: abc123");
        std::env::remove_var("UC_TEST_FOO");
    }

    #[test]
    fn expand_env_vars_drops_unset_vars() {
        std::env::remove_var("UC_TEST_NOT_SET");
        let out = expand_env_vars("key: ${UC_TEST_NOT_SET}");
        assert_eq!(out, "key: ");
    }

    #[test]
    fn expand_env_vars_leaves_invalid_syntax_alone() {
        // Unclosed `${`, characters outside [A-Za-z0-9_], etc.
        assert_eq!(expand_env_vars("$ {VAR}"), "$ {VAR}");
        assert_eq!(expand_env_vars("${VAR-with-dash}"), "${VAR-with-dash}");
        assert_eq!(expand_env_vars("${"), "${");
    }

    #[test]
    fn resolve_synthesises_local_from_default_path() {
        let cfg = StorageConfig {
            default_path: Some(PathBuf::from("/data")),
            storages: vec![],
            default: None,
            retry: Default::default(),
        };
        let r = cfg.resolve().unwrap();
        assert_eq!(r.default, "local");
        assert_eq!(r.entries.len(), 1);
        assert_eq!(r.entries[0].name, "local");
    }

    #[test]
    fn resolve_requires_default_when_storages_set() {
        let cfg = StorageConfig {
            default_path: None,
            storages: vec![ConfiguredStorage {
                name: "main".into(),
                backend: ConfiguredStorageBackend::Local {
                    path: PathBuf::from("/data"),
                },
            }],
            default: None,
            retry: Default::default(),
        };
        let err = cfg.resolve().unwrap_err();
        assert!(err.contains("default"), "{err}");
    }

    #[test]
    fn resolve_rejects_default_pointing_to_unknown_name() {
        let cfg = StorageConfig {
            default_path: None,
            storages: vec![ConfiguredStorage {
                name: "main".into(),
                backend: ConfiguredStorageBackend::Local {
                    path: PathBuf::from("/data"),
                },
            }],
            default: Some("nope".into()),
            retry: Default::default(),
        };
        let err = cfg.resolve().unwrap_err();
        assert!(err.contains("does not match"), "{err}");
    }

    #[test]
    fn resolve_rejects_duplicate_names() {
        let cfg = StorageConfig {
            default_path: None,
            storages: vec![
                ConfiguredStorage {
                    name: "main".into(),
                    backend: ConfiguredStorageBackend::Local {
                        path: PathBuf::from("/data"),
                    },
                },
                ConfiguredStorage {
                    name: "main".into(),
                    backend: ConfiguredStorageBackend::Local {
                        path: PathBuf::from("/data2"),
                    },
                },
            ],
            default: Some("main".into()),
            retry: Default::default(),
        };
        let err = cfg.resolve().unwrap_err();
        assert!(err.contains("duplicate"), "{err}");
    }

    #[test]
    fn resolve_errors_on_empty_input() {
        let cfg = StorageConfig {
            default_path: None,
            storages: vec![],
            default: None,
            retry: Default::default(),
        };
        assert!(cfg.resolve().is_err());
    }
}
