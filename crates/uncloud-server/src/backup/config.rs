//! Backup configuration — `backup` section of `config.yaml`.
//!
//! See `docs/backup.md` for the schema and the secret-resolution rules.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct BackupConfig {
    pub options: BackupOptions,
    pub targets: Vec<BackupTarget>,
}

impl BackupConfig {
    pub fn target(&self, name: &str) -> Option<&BackupTarget> {
        self.targets.iter().find(|t| t.name == name)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BackupOptions {
    pub include_versions: bool,
    pub include_trash: bool,
    pub include_thumbnails: bool,
    /// Local directory rustic uses for chunked uploads, packfile staging, etc.
    /// `None` falls back to the OS temp dir.
    pub staging_dir: Option<PathBuf>,
    /// Cap on simultaneous open `read()` calls against the source storage
    /// backend. Each in-flight reader holds one connection / file handle on
    /// the backend. SFTP servers (Hetzner Storage Box, OpenSSH stock config)
    /// limit concurrent SFTP handles per session and the archiver hits that
    /// cap fast under rayon's full parallelism. `None` falls back to 8 —
    /// conservative enough for shared-tenant SFTP, plenty for S3/local.
    pub max_concurrent_source_reads: Option<usize>,
}

impl Default for BackupOptions {
    fn default() -> Self {
        Self {
            include_versions: true,
            include_trash: false,
            include_thumbnails: false,
            staging_dir: None,
            max_concurrent_source_reads: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct BackupTarget {
    pub name: String,
    /// Restic-format repo URI. Supported shorthands (see `backup::uri`):
    /// * `sftp://user@host:port/path` — URL style, custom port
    /// * `sftp:user@host:/path`       — legacy form, default port 22
    /// * `s3:https://endpoint/bucket` / `s3:bucket/prefix`
    /// * `b2:bucket:prefix`
    /// * `azure:container:prefix`
    ///
    /// Native rustic schemes (`rest:`, `rclone:`, `opendal:`, `local:`)
    /// pass through unchanged. A bare path is treated as local.
    pub repo: String,
    #[serde(flatten)]
    pub password: SecretSource,
    #[serde(default)]
    pub credentials: BackupCredentials,
    #[serde(default)]
    pub retention: Option<RetentionPolicy>,
}

/// One of `password`, `password_file`, `password_env`, `password_command`.
/// Resolved in priority order: file → env → command → inline. Inline is
/// supported but logs a warning at runtime — secrets shouldn't live in
/// `config.yaml`.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct SecretSource {
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub password_file: Option<PathBuf>,
    #[serde(default)]
    pub password_env: Option<String>,
    #[serde(default)]
    pub password_command: Option<String>,
}

impl SecretSource {
    /// Resolve the configured source to a literal password string.
    pub fn resolve(&self) -> Result<String, String> {
        if let Some(path) = &self.password_file {
            let raw = std::fs::read_to_string(path)
                .map_err(|e| format!("backup password_file {path:?}: {e}"))?;
            return Ok(trim_eol(&raw).to_string());
        }
        if let Some(name) = &self.password_env {
            return std::env::var(name)
                .map_err(|_| format!("backup password_env: ${name} is not set"));
        }
        if let Some(cmd) = &self.password_command {
            let output = std::process::Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .output()
                .map_err(|e| format!("backup password_command {cmd:?}: {e}"))?;
            if !output.status.success() {
                return Err(format!(
                    "backup password_command exited {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            let raw = String::from_utf8(output.stdout)
                .map_err(|e| format!("backup password_command output not UTF-8: {e}"))?;
            return Ok(trim_eol(&raw).to_string());
        }
        if let Some(inline) = &self.password {
            return Ok(inline.clone());
        }
        Err("no backup password source configured (one of password / password_file / password_env / password_command is required)".into())
    }

    /// True if the password is configured inline — caller logs a warning
    /// for that case.
    pub fn is_inline(&self) -> bool {
        self.password.is_some()
            && self.password_file.is_none()
            && self.password_env.is_none()
            && self.password_command.is_none()
    }
}

/// Backend-specific credentials passed through to rustic_backend's
/// `BackendOptions::options`. Keys are backend-defined (e.g. `access_key_id`
/// for S3, `b2_account_id` for B2).
///
/// The `_env` and `_file` suffixes opt into indirect resolution. For example:
///
/// ```yaml
/// credentials:
///   access_key_id: "AKIA..."
///   secret_access_key_env: UNCLOUD_S3_SECRET
///   endpoint_file: /etc/uncloud/s3-endpoint
/// ```
///
/// resolves to a flat map `{ access_key_id: "AKIA...", secret_access_key:
/// "<env value>", endpoint: "<file contents>" }`.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, transparent)]
pub struct BackupCredentials(pub BTreeMap<String, String>);

impl BackupCredentials {
    /// Resolve `_env` / `_file` suffixed keys into their literal values.
    pub fn resolve(&self) -> Result<BTreeMap<String, String>, String> {
        let mut out = BTreeMap::new();
        for (key, value) in &self.0 {
            if let Some(stem) = key.strip_suffix("_env") {
                let resolved = std::env::var(value)
                    .map_err(|_| format!("backup credentials.{key}: ${value} is not set"))?;
                out.insert(stem.to_string(), resolved);
            } else if let Some(stem) = key.strip_suffix("_file") {
                let raw = std::fs::read_to_string(value)
                    .map_err(|e| format!("backup credentials.{key}: {e}"))?;
                out.insert(stem.to_string(), trim_eol(&raw).to_string());
            } else {
                out.insert(key.clone(), value.clone());
            }
        }
        Ok(out)
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct RetentionPolicy {
    pub keep_last: Option<u32>,
    pub keep_daily: Option<u32>,
    pub keep_weekly: Option<u32>,
    pub keep_monthly: Option<u32>,
    pub keep_yearly: Option<u32>,
}

impl RetentionPolicy {
    pub fn is_empty(&self) -> bool {
        self.keep_last.is_none()
            && self.keep_daily.is_none()
            && self.keep_weekly.is_none()
            && self.keep_monthly.is_none()
            && self.keep_yearly.is_none()
    }
}

fn trim_eol(s: &str) -> &str {
    s.trim_end_matches(['\r', '\n'])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_target() {
        let yaml = r#"
targets:
  - name: local
    repo: /tmp/uncloud-backup
    password_file: /tmp/key
"#;
        let cfg: BackupConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.targets.len(), 1);
        let t = &cfg.targets[0];
        assert_eq!(t.name, "local");
        assert_eq!(t.repo, "/tmp/uncloud-backup");
        assert_eq!(t.password.password_file.as_ref().unwrap().to_str(), Some("/tmp/key"));
    }

    #[test]
    fn parses_multi_target_with_retention_and_credentials() {
        let yaml = r#"
options:
  include_versions: true
  include_trash: false
targets:
  - name: nas
    repo: "sftp:backup@nas.lan:/srv/backups/uncloud"
    password_file: /etc/uncloud/nas.key
    retention:
      keep_daily: 7
      keep_weekly: 4
  - name: minio
    repo: "s3:http://minio:9000/uncloud-backup"
    password_env: UNCLOUD_BACKUP_PW
    credentials:
      access_key_id: AKIA
      secret_access_key_env: UNCLOUD_S3_SECRET
"#;
        let cfg: BackupConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.targets.len(), 2);
        assert!(cfg.options.include_versions);
        let minio = cfg.target("minio").unwrap();
        assert_eq!(minio.password.password_env.as_deref(), Some("UNCLOUD_BACKUP_PW"));
        assert_eq!(minio.credentials.0.get("access_key_id"), Some(&"AKIA".to_string()));
        let nas = cfg.target("nas").unwrap();
        let r = nas.retention.as_ref().unwrap();
        assert_eq!(r.keep_daily, Some(7));
        assert_eq!(r.keep_weekly, Some(4));
    }

    #[test]
    fn secret_source_resolves_env() {
        let key = "UNCLOUD_BACKUP_TEST_SECRET_RESOLVE";
        // SAFETY: single-threaded test using an isolated env var name.
        unsafe { std::env::set_var(key, "hunter2") };
        let s = SecretSource {
            password_env: Some(key.to_string()),
            ..Default::default()
        };
        assert_eq!(s.resolve().unwrap(), "hunter2");
        unsafe { std::env::remove_var(key) };
    }

    #[test]
    fn credentials_resolve_env_suffix() {
        let key = "UNCLOUD_BACKUP_TEST_S3_SECRET_RESOLVE";
        unsafe { std::env::set_var(key, "shh") };
        let mut map = BTreeMap::new();
        map.insert("access_key_id".to_string(), "AKIA".to_string());
        map.insert("secret_access_key_env".to_string(), key.to_string());
        let creds = BackupCredentials(map);
        let resolved = creds.resolve().unwrap();
        assert_eq!(resolved.get("access_key_id"), Some(&"AKIA".to_string()));
        assert_eq!(resolved.get("secret_access_key"), Some(&"shh".to_string()));
        assert!(resolved.get("secret_access_key_env").is_none());
        unsafe { std::env::remove_var(key) };
    }
}
