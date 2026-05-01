//! Backup subsystem — `uncloud-server backup {init,create,list,check,prune,restore}`.
//!
//! Writes deduplicated, encrypted snapshots to a Restic-format repository
//! via `rustic_core`. See `docs/backup.md` for the design.

pub mod config;
pub mod create;
pub mod dump;
pub mod lock;
pub mod repo;
pub mod source;

/// `backup create` arguments. Built by the clap layer in `main.rs` and
/// passed to `run_create`.
#[derive(Debug, Clone)]
pub struct CreateArgs {
    /// Restrict to this target. `None` runs sequentially against every
    /// configured target.
    pub target: Option<String>,
    pub dry_run: bool,
    pub tag: Option<String>,
    pub force_unlock: bool,
}

#[derive(Debug, Clone)]
pub struct RestoreArgs {
    pub target: String,
    /// Snapshot id, or the literal string `"latest"`.
    pub snapshot: String,
    /// Override the destination's `is_default: true` storage when matching
    /// unmapped storages from the snapshot. `None` falls back to whatever the
    /// destination flags as default.
    pub default_storage: Option<String>,
    pub conflict_policy: ConflictPolicy,
    /// Required confirmation for `conflict_policy = overwrite`.
    pub yes_i_know_this_is_destructive: bool,
    pub dry_run: bool,
    /// Acknowledge the storage-remap plan and proceed.
    pub yes: bool,
    pub force_unlock: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictPolicy {
    Abort,
    Overwrite,
}

impl std::str::FromStr for ConflictPolicy {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "abort" => Ok(ConflictPolicy::Abort),
            "overwrite" => Ok(ConflictPolicy::Overwrite),
            other => Err(format!(
                "invalid --conflict-policy value {other:?} (expected `abort` or `overwrite`)"
            )),
        }
    }
}

// ── Entry points ───────────────────────────────────────────────────────────
//
// Each `run_*` is a thin wrapper that loads config and dispatches to the
// implementation. Implementations land in subsequent commits; for now the
// wrappers report "not yet implemented" so the CLI surface is reachable.

pub async fn run_init(target_name: String) -> Result<(), Box<dyn std::error::Error>> {
    init_logging();
    let config = crate::config::Config::load_or_default();
    let target = config
        .backup
        .target(&target_name)
        .ok_or_else(|| format!("backup target {target_name:?} is not configured"))?;
    let password = target.password.resolve()?;
    if target.password.is_inline() {
        tracing::warn!(
            "target {:?}: password is inline in config.yaml — prefer password_file / password_env / password_command",
            target.name
        );
    }

    println!("Initialising repository for target {:?} at {}", target.name, target.repo);
    let target = target.clone();
    let _repo = tokio::task::spawn_blocking(move || repo::init(&target, &password)).await??;
    println!("Repository initialised successfully.");
    println!("Save the password somewhere safe — losing it makes the repository permanently unrecoverable.");
    Ok(())
}

pub async fn run_create(args: CreateArgs) -> Result<(), Box<dyn std::error::Error>> {
    create::run(args).await
}

pub async fn run_list(target: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("backup list --target {target:?}: not yet implemented");
    Ok(())
}

pub async fn run_check(
    target: Option<String>,
    read_data: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("backup check --target {target:?} --read-data={read_data}: not yet implemented");
    Ok(())
}

pub async fn run_prune(
    target: Option<String>,
    dry_run: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("backup prune --target {target:?} --dry-run={dry_run}: not yet implemented");
    Ok(())
}

pub async fn run_restore(args: RestoreArgs) -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("backup restore {args:?}: not yet implemented");
    Ok(())
}

fn init_logging() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .try_init()
        .ok();
}
