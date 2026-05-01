//! `backup list`, `check`, `prune` — thin wrappers over `rustic_core`.
//!
//! These don't touch the database, the storage backends, or our own state;
//! they just open the configured target's repository and forward to rustic.
//! No `backup_lock` is taken: the operations are idempotent and concurrent
//! reads are safe. (We may revisit if `prune` proves contention-prone.)

use rustic_core::{CheckOptions, KeepOptions, PruneOptions};

use crate::backup::config::{BackupConfig, BackupTarget, RetentionPolicy};
use crate::backup::repo;
use crate::config::Config;

pub async fn run_list(target_filter: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    crate::backup::init_logging();
    let config = Config::load_or_default();
    for target in pick_targets(&config.backup, target_filter.as_deref())? {
        list_one(&target).await?;
    }
    Ok(())
}

async fn list_one(target: &BackupTarget) -> Result<(), Box<dyn std::error::Error>> {
    println!("── Target {:?} ({}) ──────────────────────────", target.name, target.repo);
    let password = target.password.resolve()?;
    let target = target.clone();
    let snapshots = tokio::task::spawn_blocking(move || -> Result<_, String> {
        let repo = repo::open(&target, &password).map_err(|e| e.to_string())?;
        repo.get_all_snapshots()
            .map_err(|e| format!("rustic list failed: {e}"))
    })
    .await??;

    if snapshots.is_empty() {
        println!("(no snapshots)");
        return Ok(());
    }
    println!(
        "{:<10} {:<25} {:<24} {:>10} {}",
        "ID", "TIME", "HOST", "SIZE", "TAGS"
    );
    let mut sorted = snapshots;
    sorted.sort_by_key(|s| s.time.clone());
    for s in &sorted {
        let id = s.id.to_hex().to_string();
        let id_short = &id[..id.len().min(8)];
        let time = s.time.to_string();
        let host = s.hostname.clone();
        let size = s
            .summary
            .as_ref()
            .map(|sm| sm.total_bytes_processed)
            .unwrap_or(0);
        let tags = s
            .tags
            .iter()
            .map(|t| t.to_string())
            .collect::<Vec<_>>()
            .join(",");
        println!("{id_short:<10} {time:<25} {host:<24} {:>10} {tags}", human_bytes(size));
    }
    Ok(())
}

pub async fn run_check(
    target_filter: Option<String>,
    read_data: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    crate::backup::init_logging();
    let config = Config::load_or_default();
    let mut had_failure = false;
    for target in pick_targets(&config.backup, target_filter.as_deref())? {
        if let Err(e) = check_one(&target, read_data).await {
            eprintln!("Target {:?} check failed: {e}", target.name);
            had_failure = true;
        }
    }
    if had_failure {
        Err("One or more targets failed integrity check.".into())
    } else {
        Ok(())
    }
}

async fn check_one(target: &BackupTarget, read_data: bool) -> Result<(), Box<dyn std::error::Error>> {
    println!("── Target {:?} ({}) ──────────────────────────", target.name, target.repo);
    let password = target.password.resolve()?;
    let target_clone = target.clone();
    tokio::task::spawn_blocking(move || -> Result<_, String> {
        let repo = repo::open(&target_clone, &password).map_err(|e| e.to_string())?;
        let mut opts = CheckOptions::default();
        opts.read_data = read_data;
        repo.check(opts)
            .map_err(|e| format!("rustic check failed: {e}"))
    })
    .await??;
    println!("OK");
    Ok(())
}

pub async fn run_prune(
    target_filter: Option<String>,
    dry_run: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    crate::backup::init_logging();
    let config = Config::load_or_default();
    for target in pick_targets(&config.backup, target_filter.as_deref())? {
        prune_one(&target, dry_run).await?;
    }
    Ok(())
}

async fn prune_one(target: &BackupTarget, dry_run: bool) -> Result<(), Box<dyn std::error::Error>> {
    println!("── Target {:?} ({}) ──────────────────────────", target.name, target.repo);
    let Some(retention) = target.retention.as_ref() else {
        println!("No retention policy configured for this target — skipping.");
        return Ok(());
    };
    if retention.is_empty() {
        println!("Retention policy is empty (no `keep_*` fields set) — skipping.");
        return Ok(());
    }
    let keep = build_keep_options(retention);

    let password = target.password.resolve()?;
    let target_clone = target.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<(usize, usize), String> {
        let repo = repo::open(&target_clone, &password).map_err(|e| e.to_string())?;
        let snapshots = repo
            .get_all_snapshots()
            .map_err(|e| format!("rustic list failed: {e}"))?;
        let total = snapshots.len();
        let now = rustic_zoned_now();
        let plan = keep
            .apply(snapshots, &now)
            .map_err(|e| format!("retention apply failed: {e}"))?;
        let to_forget: Vec<_> = plan
            .iter()
            .filter(|s| !s.keep)
            .map(|s| s.snapshot.id)
            .collect();
        let kept = total - to_forget.len();

        if dry_run {
            println!(
                "(dry run) {} snapshot(s), would keep {}, forget {}",
                total,
                kept,
                to_forget.len()
            );
            for s in &plan {
                let mark = if s.keep { "keep" } else { "drop" };
                let id = s.snapshot.id.to_hex().to_string();
                let id_short = &id[..id.len().min(8)];
                println!(
                    "  {mark} {id_short:<10} {} {}",
                    s.snapshot.time,
                    s.reasons.join(",")
                );
            }
            return Ok((kept, to_forget.len()));
        }

        if !to_forget.is_empty() {
            repo.delete_snapshots(&to_forget)
                .map_err(|e| format!("delete_snapshots failed: {e}"))?;
        }
        let prune_opts = PruneOptions::default();
        let prune_plan = repo
            .prune_plan(&prune_opts)
            .map_err(|e| format!("prune_plan failed: {e}"))?;
        repo.prune(&prune_opts, prune_plan)
            .map_err(|e| format!("prune failed: {e}"))?;
        Ok((kept, to_forget.len()))
    })
    .await??;
    if !dry_run {
        println!("Forgot {} snapshot(s), kept {}.", result.1, result.0);
    }
    Ok(())
}

fn pick_targets(
    cfg: &BackupConfig,
    explicit: Option<&str>,
) -> Result<Vec<BackupTarget>, Box<dyn std::error::Error>> {
    if let Some(name) = explicit {
        Ok(vec![cfg
            .target(name)
            .ok_or_else(|| format!("backup target {name:?} is not configured"))?
            .clone()])
    } else {
        Ok(cfg.targets.clone())
    }
}

fn build_keep_options(r: &RetentionPolicy) -> KeepOptions {
    let mut k = KeepOptions::default();
    if let Some(n) = r.keep_last { k.keep_last = Some(n as i32); }
    if let Some(n) = r.keep_daily { k.keep_daily = Some(n as i32); }
    if let Some(n) = r.keep_weekly { k.keep_weekly = Some(n as i32); }
    if let Some(n) = r.keep_monthly { k.keep_monthly = Some(n as i32); }
    if let Some(n) = r.keep_yearly { k.keep_yearly = Some(n as i32); }
    k
}

/// `KeepOptions::apply` takes a `jiff::Zoned`. We don't depend on jiff
/// directly; pull the version it pins via rustic's transitive use.
fn rustic_zoned_now() -> jiff::Zoned {
    jiff::Zoned::now()
}

fn human_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} {}", UNITS[0])
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}
