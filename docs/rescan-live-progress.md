# Rescan live progress

Two problems with the current rescan UX:

1. **State is lost on navigation.** `rescan_job`, `rescan_starting`, and `rescan_error` are `use_signal`s local to the `AdminSection` component in `settings.rs`. Leaving Settings unmounts the component, drops the signals, and cancels the polling loop. Coming back shows a blank rescan panel even though the job is still running on the server.
2. **2s polling is wasteful and laggy.** `GET /api/admin/rescan-jobs/{id}` is fired in a `loop { TimeoutFuture(2000).await; ... }`. It hits the server on a fixed cadence whether anything changed or not, and updates the UI up to 2s after each flush.

Both collapse into a server-push + app-level state design.

## Step 1 — Lift state into an app-level context

Introduce a `RescanState` signal provided at the app root alongside `AuthState`, `ThemeState`, `PlayerState`:

```rust
// crates/uncloud-web/src/state.rs
#[derive(Clone, Default)]
pub struct RescanState {
    pub job: Option<use_storages::RescanJob>,
    pub error: Option<String>,
    pub starting: bool,
}
```

- `app.rs` calls `use_context_provider(|| Signal::new(RescanState::default()))` once, near `AuthState`.
- `AdminSection` in `settings.rs` reads/writes via `use_context::<Signal<RescanState>>()` instead of its own local signals.
- The spawn that starts a rescan moves out of `AdminSection`; it becomes a top-level "start + own the SSE subscription" helper (or stays inline but writes to the shared signal).

Navigating away no longer drops the state. The rescan panel re-renders with live data the next time Settings is opened.

## Step 2 — Replace polling with SSE

Add two variants to `ServerEvent` (`uncloud-common/src/api/events.rs`) and the corresponding server-side `Event` (`uncloud-server/src/services/events.rs`):

```rust
RescanProgress {
    job_id: String,
    status: String,            // "running"
    processed_entries: u64,
    total_entries: Option<u64>,
    imported_files: u64,
    imported_folders: u64,
    skipped_existing: u64,
    conflicts_count: u32,
},
RescanFinished {
    job_id: String,
    status: String,            // "completed" | "cancelled" | "failed"
    processed_entries: u64,
    imported_files: u64,
    imported_folders: u64,
    skipped_existing: u64,
    conflicts: Vec<RescanConflictData>,
    error: Option<String>,
},
```

Emission in `run_rescan_worker` (`routes/storages.rs`):

- On every counter flush (every `COUNTER_FLUSH_EVERY = 32` entries), call `state.events.emit_rescan_progress(owner_user_id, &*job).await` after writing the job snapshot.
- On terminal transition (post-worker, where `job.status` is set to Completed / Failed / Cancelled and `finished_at` is stamped), emit `RescanFinished` with the final counters + conflicts + error.
- Threading: `rescan_storage` needs the admin's `user_id` (from `AuthUser`) — pass it into `run_rescan_worker` so emissions reach the initiator's per-user SSE channel.

Client wiring:

- Extend the main SSE consumer (already present via `use_events` + the `sse_event` context) — in `AdminSection`'s new hook (or in `app.rs` where we own the context), match `RescanProgress` / `RescanFinished` and write the whole thing into `RescanState`.
- Delete the `loop { TimeoutFuture(2000).await; … }` block. Starting a rescan just writes the initial snapshot into state and returns; the stream carries it from there.

Cadence check: a 100k-entry scan emits ~3k `RescanProgress` events (one per 32 entries). If that feels chatty, coalesce server-side on 500ms: track "last emit at" on the job handle and skip the emit if less than 500ms has elapsed, always emitting the final one.

## Step 3 — Restore on reload via `/active`

New admin route `GET /api/admin/rescan-jobs/active` returns the in-memory job for the admin's storage if any is `Running`, else 204. Covers:

- Hard refresh during a rescan.
- Logging in on a different browser / device while a rescan is in flight.

Frontend: on app mount (or first time an admin-authed session appears), call `get_active_rescan_job` once and hydrate `RescanState`. After that the SSE stream drives updates.

Server registry already has `active_by_storage` — the endpoint is a small lookup:

```rust
pub async fn get_active_rescan_job(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Option<RescanJob>>> {
    let active = state.rescan.active_by_storage.read().await;
    // pick any (we only allow one per storage today, so first/only)
    for (_storage_id, job_id) in active.iter() {
        if let Some(handle) = state.rescan.get(*job_id).await {
            return Ok(Json(Some(handle.job.read().await.clone())));
        }
    }
    Ok(Json(None))
}
```

(`active_by_storage` is currently private — either expose via a `list_active` method on `RescanService` or make it `pub(crate)`.)

## Notes / tradeoffs

- **Per-user SSE.** Events only reach the admin who started the rescan. Single-admin deployments are fine. Multi-admin live-observation would need a broadcast channel or an "admin room" concept — deferred.
- **In-memory registry.** Server restart mid-rescan leaves the client with a dangling job id; `get_active_rescan_job` returns `None` and `RescanState` clears. Acceptable — rescans are idempotent and can be restarted by the user.
- **Backward compat.** `GET /api/admin/rescan-jobs/{id}` and `POST /.../cancel` stay; cancel still works against an in-flight job. The polling endpoint becomes unused by the UI but is useful for debugging and future external tools.
- **Event ordering.** SSE channels are per-user ordered, so `RescanProgress` counters are monotone from the client's perspective. No reconciliation logic needed.

## File checklist

Server:

- [ ] `crates/uncloud-common/src/api/events.rs` — add `RescanProgress` + `RescanFinished` variants.
- [ ] `crates/uncloud-server/src/services/events.rs` — mirror variants + `emit_rescan_progress`, `emit_rescan_finished`.
- [ ] `crates/uncloud-server/src/services/rescan.rs` — `list_active()` helper (or `pub(crate)` access).
- [ ] `crates/uncloud-server/src/routes/storages.rs` — thread `user_id` into `run_rescan_worker`; emit on flush + on terminal; add `get_active_rescan_job`.
- [ ] `crates/uncloud-server/src/routes/mod.rs` — wire `GET /admin/rescan-jobs/active`.

Web:

- [ ] `crates/uncloud-web/src/state.rs` — `RescanState` struct.
- [ ] `crates/uncloud-web/src/app.rs` — provide `Signal<RescanState>` context; on admin session, hydrate via `get_active_rescan_job`.
- [ ] `crates/uncloud-web/src/hooks/use_storages.rs` — add `get_active_rescan_job`.
- [ ] `crates/uncloud-web/src/components/layout.rs` (or wherever the global SSE dispatch lives) — consume `RescanProgress` / `RescanFinished` into `RescanState`.
- [ ] `crates/uncloud-web/src/components/settings.rs` — drop local signals + polling loop; read/write `RescanState`.

## Not in scope

- Multi-admin broadcast.
- Persisting jobs across server restarts.
- Rescan history / log of past jobs.
