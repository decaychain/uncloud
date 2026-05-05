//! Recursive filesystem watcher that triggers a sync when the user
//! changes files inside the configured root, debounced so a bulk
//! operation (archive extraction, recursive paste) only kicks one sync.
//!
//! Desktop-only — Android uses SAF and doesn't expose inotify-style
//! events; mobile sync still runs from the poll loop and resume hook.

#![cfg(desktop)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
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

/// Single message between the OS-side notify callback and the debouncer
/// task. Carries enough context for the debouncer to decide which paths
/// (if any) should cancel pending deletes before the post-debounce sync
/// fires.
struct WatcherSignal {
    paths: Vec<PathBuf>,
    /// `true` for Create/Modify events — those signal a path coming back
    /// to life and should cancel a journal-side pending delete. Removes
    /// and metadata-only events are not enough on their own.
    resurrects: bool,
}

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
    // Each event carries the paths it touched plus a flag for whether
    // the kind is a "create or modify" (which cancels a pending delete
    // on Phase 6a's two-phase tracker). Removes don't cancel — the
    // engine treats scan-time absence as authoritative, and we don't
    // want a delete event to invalidate its own pending state.
    let (tx, mut rx) = mpsc::unbounded_channel::<WatcherSignal>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| match res {
        Ok(event) => {
            let resurrects = matches!(
                event.kind,
                EventKind::Create(_) | EventKind::Modify(_)
            );
            let _ = tx.send(WatcherSignal {
                paths: event.paths,
                resurrects,
            });
        }
        Err(e) => warn!("file watcher: {}", e),
    })?;
    watcher.watch(root, RecursiveMode::Recursive)?;
    info!("file watcher: watching {}", root.display());

    async_runtime::spawn(async move {
        loop {
            // Block until the first event in a quiet period.
            let Some(first) = rx.recv().await else { break };
            let mut resurrected: Vec<PathBuf> = if first.resurrects {
                first.paths.clone()
            } else {
                Vec::new()
            };
            // Drain further events; reset the timer each time we see
            // one. When `DEBOUNCE` of silence elapses, fire.
            loop {
                match tokio::time::timeout(DEBOUNCE, rx.recv()).await {
                    Ok(Some(sig)) => {
                        if sig.resurrects {
                            resurrected.extend(sig.paths);
                        }
                        continue;
                    }
                    Ok(None) => return, // channel closed
                    Err(_) => break,    // debounce window ended
                }
            }

            // Cancel pending deletes for any path that came back during
            // this debounce window. Done before spawn_sync so the about-
            // to-run sync sees the cleared journal and skips Phase 6a's
            // commit step for those paths.
            if !resurrected.is_empty() {
                let engine_snap = engine.read().await.clone();
                if let Some(eng) = engine_snap {
                    let mut seen: std::collections::HashSet<PathBuf> =
                        std::collections::HashSet::new();
                    for p in resurrected {
                        if !seen.insert(p.clone()) {
                            continue;
                        }
                        if let Some(s) = p.to_str() {
                            let _ = eng.cancel_pending_delete_for_path(s).await;
                        }
                    }
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
