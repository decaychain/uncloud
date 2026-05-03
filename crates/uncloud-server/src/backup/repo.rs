//! Thin wrapper around `rustic_core::Repository` — turns our config-driven
//! `BackupTarget` into either a freshly initialised or an already-opened
//! repository handle.
//!
//! Kept narrow on purpose so the `rustic_core` API surface is reachable from
//! exactly one file. The fork is pre-1.0 and likely to churn; centralising
//! the bridge here means a version bump touches one module instead of many.

use std::path::Path;

use rustic_backend::BackendOptions;
use rustic_core::{
    ConfigOptions, Credentials, KeyOptions, OpenStatus, Repository, RepositoryOptions,
};

use crate::backup::config::BackupTarget;
use crate::error::{AppError, Result};

/// An opened repository ready for snapshot operations. Wraps the
/// `rustic_core` handle in our error type so callers don't need to know
/// about `RusticResult`.
pub type RepoHandle = Repository<OpenStatus>;

/// Resolve a `BackupTarget` to a `RepositoryBackends`, expanding password and
/// credentials sources at run time. Used by both `open` and `init`.
fn build_backends(target: &BackupTarget) -> Result<rustic_core::RepositoryBackends> {
    let user_credentials = target.credentials.resolve().map_err(AppError::Internal)?;
    let expanded = crate::backup::uri::expand(&target.repo);

    // User-supplied `credentials:` always wins over synthesised values
    // (matching shorthand → opendal expansion).
    let mut options = expanded.options;
    options.extend(user_credentials);

    // Pre-flight check the SFTP key file before handing off to OpenDAL.
    // Without this, an unreadable / world-readable / missing key just
    // makes ssh hang on auth (BatchMode is on) and surfaces as an opaque
    // "connection request: timeout" some seconds later.
    if expanded.uri == "opendal:sftp" {
        if let Some(key_path) = options.get("key") {
            validate_ssh_key_file(&target.name, key_path)?;
        }
    }

    let mut be_opts = BackendOptions::default();
    be_opts.repository = Some(expanded.uri);
    be_opts.options = options;

    be_opts.to_backends().map_err(|e| {
        AppError::Storage(format!(
            "backup target {:?}: failed to construct rustic backend: {e}",
            target.name
        ))
    })
}

/// Validate an SSH private key file before letting OpenDAL hand it to
/// `ssh`. Checks: the path exists and is a regular file; the running
/// process can read it; on Unix the mode bits forbid group/other access
/// (mirrors what OpenSSH itself enforces — `ssh` rejects keys with
/// `mode & 0o077 != 0` and the resulting auth failure under
/// `BatchMode=yes` looks like a hang).
fn validate_ssh_key_file(target_name: &str, key_path: &str) -> Result<()> {
    let path = Path::new(key_path);
    let meta = std::fs::metadata(path).map_err(|e| {
        AppError::Internal(format!(
            "backup target {target_name:?}: SSH key file {key_path:?}: {e}",
        ))
    })?;
    if !meta.is_file() {
        return Err(AppError::Internal(format!(
            "backup target {target_name:?}: SSH key {key_path:?} is not a regular file",
        )));
    }
    // Open-for-read so a permissions issue surfaces with the actual OS error.
    std::fs::File::open(path).map_err(|e| {
        AppError::Internal(format!(
            "backup target {target_name:?}: SSH key {key_path:?} not readable by current user: {e}",
        ))
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode();
        if mode & 0o077 != 0 {
            return Err(AppError::Internal(format!(
                "backup target {target_name:?}: SSH key {key_path:?} has mode {:o} \
                 (group/other readable). OpenSSH refuses keys with permissions \
                 wider than 0600. Run: chmod 600 {key_path:?}",
                mode & 0o777,
            )));
        }
    }

    Ok(())
}

fn build_repo_options(_target: &BackupTarget) -> RepositoryOptions {
    // Nothing to pull from the target yet — `RepositoryOptions` is mostly
    // about cache, warm-up commands, and progress wiring, none of which we
    // expose in v1. Stays a function so future config plumbing has a
    // single seam.
    RepositoryOptions::default()
}

/// Open an existing repository for the given target. Errors if the repo
/// does not exist (use [`init`] first) or the password is wrong.
pub fn open(target: &BackupTarget, password: &str) -> Result<RepoHandle> {
    let backends = build_backends(target)?;
    let repo_opts = build_repo_options(target);
    let repo = Repository::new(&repo_opts, &backends).map_err(|e| {
        AppError::Storage(format!(
            "backup target {:?}: failed to construct repository: {e}",
            target.name
        ))
    })?;
    let creds = Credentials::password(password);
    repo.open(&creds).map_err(|e| {
        AppError::Storage(format!(
            "backup target {:?}: failed to open repository: {e}",
            target.name
        ))
    })
}

/// Initialise a fresh repository at the target's location. Errors if the
/// repo already exists; the caller surfaces this as a friendlier message.
pub fn init(target: &BackupTarget, password: &str) -> Result<RepoHandle> {
    let backends = build_backends(target)?;
    let repo_opts = build_repo_options(target);
    let repo = Repository::new(&repo_opts, &backends).map_err(|e| {
        AppError::Storage(format!(
            "backup target {:?}: failed to construct repository: {e}",
            target.name
        ))
    })?;
    let creds = Credentials::password(password);
    let key_opts = KeyOptions::default();
    let config_opts = ConfigOptions::default();
    repo.init(&creds, &key_opts, &config_opts).map_err(|e| {
        AppError::Storage(format!(
            "backup target {:?}: failed to initialise repository: {e}",
            target.name
        ))
    })
}
