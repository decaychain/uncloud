//! Thin wrapper around `rustic_core::Repository` — turns our config-driven
//! `BackupTarget` into either a freshly initialised or an already-opened
//! repository handle.
//!
//! Kept narrow on purpose so the `rustic_core` API surface is reachable from
//! exactly one file. The fork is pre-1.0 and likely to churn; centralising
//! the bridge here means a version bump touches one module instead of many.

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
    let credentials = target.credentials.resolve().map_err(AppError::Internal)?;

    let mut be_opts = BackendOptions::default();
    be_opts.repository = Some(target.repo.clone());
    be_opts.options = credentials.into_iter().collect();

    be_opts.to_backends().map_err(|e| {
        AppError::Storage(format!(
            "backup target {:?}: failed to construct rustic backend: {e}",
            target.name
        ))
    })
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
