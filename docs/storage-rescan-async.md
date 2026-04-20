# Storage Rescan — Async Job + Progress

## Problem

The admin storage rescan endpoint (`POST /api/admin/storages/{id}/rescan`, PR #14) runs the
entire scan inline in the HTTP handler. On any real storage this blows past the reverse-proxy
/ browser timeout: the backend keeps working, but the client gives up, the UI shows nothing
useful, and there is no way to observe progress or resume.

Root causes, in order of impact:

1. **Handler is synchronous end-to-end.** No response is sent until every entry has been
   scanned, hashed, and imported. A multi-GB library easily exceeds 30–60 s.
2. **SHA-256 hashing is sequential and in-handler.** For each candidate import the file is
   streamed through `hash_file` before the DB insert. Large files dominate wall-clock time.
3. **One DB round-trip per entry.** `find_live_folder` / `find_live_file` issue a separate
   MongoDB query for each scan entry. N entries -> N queries.
4. **No client progress.** Even if the request completed, the user stares at a spinner the
   whole time and gets a single summary blob at the end.

## Goal

Turn rescan into a background job with live progress. The HTTP POST should return
immediately with a job id. The UI should show a running progress bar and a final summary,
and survive page reloads.

## Design

### Server

1. **Job model.** New `rescan_jobs` collection (or in-memory map keyed by `ObjectId`, persisted
   only if we decide we want crash-resume):

    ```rust
    struct RescanJob {
        id: ObjectId,
        storage_id: ObjectId,
        started_by: ObjectId,      // admin user_id
        started_at: DateTime<Utc>,
        finished_at: Option<DateTime<Utc>>,
        status: RescanStatus,       // Running | Completed | Failed | Cancelled
        // progress
        total_entries: Option<u64>,       // filled in after the scan phase
        processed_entries: u64,
        imported_files: u64,
        imported_folders: u64,
        skipped: u64,
        conflicts: Vec<RescanConflict>,   // capped to N (e.g. 500) to bound memory
        error: Option<String>,
    }
    ```

    Start in-memory only. Promote to Mongo if we want jobs to survive a restart — can be a
    follow-up.

2. **New routes:**
    - `POST /api/admin/storages/{id}/rescan` — enqueue a job, return `{ job_id }` (202).
      Reject with 409 if a job is already running for that storage.
    - `GET /api/admin/rescan-jobs/{id}` — fetch status snapshot.
    - `GET /api/admin/rescan-jobs` — list recent jobs (for the storage admin panel).
    - `POST /api/admin/rescan-jobs/{id}/cancel` — cooperative cancel.

3. **Worker.** `tokio::spawn` a task that owns the job. The task:
    - runs `backend.scan(prefix)` first; sets `total_entries` once the scan completes so the
      UI can show a real percentage.
    - processes entries in batches (e.g. 100 at a time), and for each batch:
        - **batch the folder/file lookups**: single `find` with `{ storage_path: { $in: [...] } }`
          per batch instead of N round trips. Keep the in-memory path->id maps.
        - hashes files with a bounded-parallelism pool (e.g. `max_concurrency` from
          `processing` config) using `tokio::task::spawn_blocking` or an async reader pool —
          hashing a multi-GB file on the handler thread is the main latency driver today.
        - writes progress counters on the shared `Arc<Mutex<RescanJob>>` after each batch.
    - emits SSE `ServerEvent::RescanProgress { job_id, processed, total, imported }` every
      N entries or every few hundred ms (throttled) so the UI updates live.
    - finishes with `ServerEvent::RescanCompleted { job_id, summary }`.

4. **Cancellation.** The worker checks an `AtomicBool` (or a `tokio_util::sync::CancellationToken`)
   between batches. Cancel sets status to `Cancelled` and stops cleanly.

5. **Event bus plumbing.**
    - Add the two variants to `uncloud_common::ServerEvent` and
      `uncloud_server::services::events::Event`.
    - Broadcast to the admin user who started the job (the existing per-user channel works).

### Frontend

1. **Hook (`use_storages.rs`):**
    - `start_rescan(storage_id)` now returns `{ job_id }` and no longer blocks.
    - `get_rescan_job(job_id)` for the initial snapshot on page load.
    - Subscribe to `RescanProgress` / `RescanCompleted` via `use_events` and update a signal.

2. **Admin settings panel (`components/settings.rs`):**
    - Store the current job in a signal; when present, render a progress bar (`progress` class)
      with `processed / total` and live counts (imported files / folders, skipped, conflicts).
    - Replace the current "Rescan" button with:
        - idle -> "Rescan" (starts job)
        - running -> progress bar + "Cancel" button
        - finished -> summary (reuse current alert layout) + "Rescan again" button
    - Persist the active job id in localStorage so a page reload rehydrates the progress view.

3. **Conflicts list**: cap at 50 in the UI with "and N more…" and a "Download full list" button
   that hits `GET /api/admin/rescan-jobs/{id}` for the full conflict array. Today the list is
   rendered unbounded.

## Ordering

1. `ScanEntry` / `scan(prefix)` already exist — no changes needed.
2. Add `RescanJob` struct, routes, and the in-memory job registry. POST returns 202 with
   `{ job_id }`. Worker runs but doesn't yet emit progress — return status via polling.
3. Add SSE `RescanProgress` / `RescanCompleted` and throttle emission.
4. Batch the folder/file lookups (the biggest easy win for speed).
5. Parallelise hashing with a bounded pool.
6. Wire up the frontend progress bar + cancel button + localStorage rehydration.
7. Cap the conflicts list in the UI and expose the full list via the job endpoint.

Steps 4 and 5 can ship independently and dramatically reduce total job time; they don't
depend on the SSE plumbing.

## Out of scope (possible later)

- Persisting `rescan_jobs` to MongoDB so jobs survive a restart.
- Audit-log entry for each rescan (flagged in the PR #14 review, still pending).
- Integration test for the admin rescan endpoint (flagged in the PR #14 review, still
  pending).
- Rescanning non-default storages from the UI — the endpoint already supports it; the
  settings panel only surfaces the default storage today.
