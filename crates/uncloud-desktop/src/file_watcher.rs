//! Recursive filesystem watcher that triggers a sync when the user
//! changes files inside the configured root, debounced so a bulk
//! operation (archive extraction, recursive paste) only kicks one sync.
//!
//! Desktop-only — Android uses SAF and doesn't expose inotify-style
//! events; mobile sync still runs from the poll loop and resume hook.

#![cfg(desktop)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use tauri::{async_runtime, AppHandle};
use tokio::sync::mpsc;
use tokio::sync::{Mutex, RwLock};
use tracing::{error, info, warn};
use uncloud_sync::SyncEngine;

use crate::{spawn_sync, SyncPhase, SyncStats};

/// How long to wait after the last filesystem event before firing a sync.
/// Bulk operations send hundreds of events in quick succession; this
/// window collapses them into one trigger.
const DEBOUNCE: Duration = Duration::from_secs(2);

/// Spawn a recursive watcher on `root`, debounced. Each batch of events
/// (separated by [`DEBOUNCE`] of silence) fires one [`spawn_sync`] call.
///
/// The watcher and its debounce task are both moved into a detached
/// tokio task; if `root` becomes invalid (folder deleted) the watcher
/// errors and the task exits — the next manual sync / poll tick will
/// re-establish state on its own.
pub fn start(
    app: AppHandle,
    root: &Path,
    engine: Arc<RwLock<Option<Arc<SyncEngine>>>>,
    phase: Arc<Mutex<SyncPhase>>,
    stats: Arc<Mutex<SyncStats>>,
    run_lock: Arc<Mutex<()>>,
) -> notify::Result<RecommendedWatcher> {
    let (tx, mut rx) = mpsc::unbounded_channel::<()>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| match res {
        Ok(_event) => {
            // We don't care which paths or kinds — any change in the
            // tree warrants a sync. Drop send errors quietly: a closed
            // channel means the debouncer task exited and we should
            // stop emitting.
            let _ = tx.send(());
        }
        Err(e) => warn!("file watcher: {}", e),
    })?;
    watcher.watch(root, RecursiveMode::Recursive)?;
    info!("file watcher: watching {}", root.display());

    async_runtime::spawn(async move {
        loop {
            // Block until the first event in a quiet period.
            if rx.recv().await.is_none() {
                break;
            }
            // Drain further events; reset the timer each time we see
            // one. When `DEBOUNCE` of silence elapses, fire.
            loop {
                match tokio::time::timeout(DEBOUNCE, rx.recv()).await {
                    Ok(Some(_)) => continue,
                    Ok(None) => return, // channel closed
                    Err(_) => break,    // debounce window ended
                }
            }
            spawn_sync(
                app.clone(),
                engine.clone(),
                phase.clone(),
                stats.clone(),
                run_lock.clone(),
            );
        }
    });

    Ok(watcher)
}

/// Best-effort wrapper around [`start`] that logs errors and returns
/// nothing. Used by the call sites in `lib.rs` that don't want to
/// thread a `notify::Result` back up.
pub fn start_or_log(
    app: AppHandle,
    root: &Path,
    engine: Arc<RwLock<Option<Arc<SyncEngine>>>>,
    phase: Arc<Mutex<SyncPhase>>,
    stats: Arc<Mutex<SyncStats>>,
    run_lock: Arc<Mutex<()>>,
) -> Option<RecommendedWatcher> {
    match start(app, root, engine, phase, stats, run_lock) {
        Ok(w) => Some(w),
        Err(e) => {
            error!("file watcher start failed for {}: {}", root.display(), e);
            None
        }
    }
}
