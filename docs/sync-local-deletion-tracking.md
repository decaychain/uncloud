# Sync — Local Deletion Tracking

Branch: `sync-local-deletion-tracking` (branched off `main`).

## Motivation

Before this change the sync engine had no concept of "the user deleted a
file locally." A row that was once synced but no longer existed on disk
got silently re-downloaded on the next scan, so `rm` against the sync
root was effectively undone. The relevant code lives in `engine.rs`
Phase 5 — the `.filter(|_| local_exists)` line that discarded every
journal entry whose local file was gone, falling through to the
`None → download` branch.

The fix is more subtle than flipping the table: a naïve "missing
locally → push delete on the server" reads benign in isolation but is
catastrophic when the volume is unmounted, the journal DB is copied
between machines, the user wipes their root, or the watcher is lying
about the state of the world. The whole journal would be interpreted
as one big delete and pushed up.

This document describes the design that replaces the buggy branch.

## Design

Three independent guard rails, layered:

### 1. `.uncloud-root.json` sentinel

Every directory the user pointed sync at gets a sentinel file at its
root the first time the engine successfully syncs it. Content:

```json
{
  "base_id": "<uuid>",
  "instance_id": "<uuid>",
  "local_path": "/home/alice/Uncloud",
  "created_at": "2026-05-06T12:34:56Z"
}
```

The `base_id` is mirrored into a new `sync_bases` row in the journal,
keyed by `local_path`. On every sync run the engine reads the sentinel
and compares its `base_id` to the journal's row:

| Disk           | Journal       | Outcome                                                                  |
| -------------- | ------------- | ------------------------------------------------------------------------ |
| Match          | Match         | `Verified` — proceed.                                                    |
| Missing        | Has row       | `Missing` — abort. Volume unmounted, user wiped, or sentinel deleted.    |
| Wrong base id  | Has row       | `Mismatch` — abort. Different base mounted at the same path.             |
| Has stale file | No row        | `Mismatch` — abort. Refuses to adopt another install's leftover.         |
| Missing        | No row        | `Minted` — first sync of a fresh base, write the file and the row.       |

A failure does **not** auto-recover. It surfaces a structured error
the desktop UI shows to the user, who is expected to fix the
underlying state (reattach the volume, clear the base via Settings,
…) and retry. Recreating silently would mask the problems we built
this guard for.

The sentinel is excluded from `walk()` and `walk_dirs()` so it never
participates in sync (no "what is this `.uncloud-root.json` doing on
my server?").

### 2. Out-of-base journal pruning

After sentinel verification the engine walks `sync_state` and drops
any row whose `local_path` doesn't fall under one of the verified
bases. This catches:

- Stale rows left over from a previous root path before the user
  reconfigured the desktop's sync folder.
- Rows from a journal DB copied between machines (different roots).
- Rows from a per-folder `local_path` override that has since been
  cleared.

Pruning is cheap to be wrong in: dropping a coherent row just makes
Phase 4 redo a compare-and-sync, never loses data.

### 3. Two-phase deletion

For rows that *do* sit inside a verified base but whose local file is
missing, the engine uses a two-phase commit:

- **First scan** sets `delete_pending_since` (an ISO 8601 timestamp)
  on the journal row and does nothing else for that file.
- **Second scan** that still finds the file missing pushes `DELETE
  /api/files/{id}` (the server soft-deletes to Trash) and removes the
  journal row.

In between, four things can cancel the pending state:

| Trigger                                          | Why it cancels                                              |
| ------------------------------------------------ | ----------------------------------------------------------- |
| File reappears locally                           | User restored it; treat as local-newer on the next compare. |
| Server's `updated_at` advanced past the marker   | Server change wins (user's intent is older than the edit).  |
| Watcher fires a Create/Modify event for the path | Tripwire — path was touched, can't trust the absence.       |
| Effective strategy stops permitting deletes      | Strategy gating below.                                      |

The watcher is **only** allowed to cancel, never to commit. Filesystem
watchers are notoriously unreliable (lost events under load, missing
on network mounts, inotify limits) so we don't trust them as the
authority for deletes — but a fired event is positive evidence that
the path is alive, which is enough to drop a pending state.

### Strategy gating

| Strategy          | First scan: mark pending? | Second scan: push delete? |
| ----------------- | ------------------------- | ------------------------- |
| `TwoWay`          | yes                       | yes                       |
| `ClientToServer`  | yes                       | yes                       |
| `UploadOnly`      | no — drop pending if set, never push | n/a            |
| `ServerToClient`  | no — Phase 5 keeps re-downloading the file | n/a       |
| `DoNotSync`       | n/a                       | n/a                       |

### Folder collapse

When every file the journal had under a folder is missing locally
*and* the folder's own directory is gone, Phase 6a pushes a single
`DELETE /api/folders/{id}` instead of N file-deletes. The server
cascades the trash (sets `deleted_at` on every descendant inside one
`batch_delete_id`), so the user sees one entry in Trash and one row in
the audit log. The two-phase commit applies to the folder row the
same way it does to file rows.

## Migration

Existing users on builds before this change have:

- a populated `sync_state` table,
- no `sync_bases` row,
- no `.uncloud-root.json` on disk.

The first sync after upgrade lands in the `Minted` branch of the
sentinel logic — fresh base row + sentinel file written. To absorb
journals that drifted out of coherence with disk under the old
behaviour (the bug), the engine also runs a one-shot **reconcile**
pass on freshly-minted bases: any journal row whose local file is
missing is silently dropped before Phase 4 runs. Coherent rows pass
through untouched. Incoherent rows are then re-discovered as fresh
downloads on this same scan.

After that one reconcile, the user is in steady state: every base has
a sentinel + journal row, and the deletion tracker takes over.

## Costs and known tradeoffs

- **Two-scan latency.** Deletes don't reach Trash until the second
  sync after the user removes the file. With the default 60s poll
  that's typically 1–3 minutes. Worth flagging in user-facing
  documentation.
- **Wipe-then-restore needs a reattach.** Users who used to wipe
  their local root expecting "next sync re-downloads from server"
  now hit a sentinel error and have to explicitly clear the base
  (planned UI action) before the next sync re-mints. This is by
  design — the abort prevents the catastrophic mass-delete failure
  mode, and the price is one extra click in a rare workflow.
- **No threshold protection.** A user who legitimately deletes
  every single file in a base will see the sentinel still verify
  (the sentinel itself remains) and Phase 6a pushes deletes for
  all of them across two scans. The two-phase commit is the only
  brake. We could add an N% threshold guard later if the workflow
  proves common enough to warrant the extra UX.

## File layout

| Concern                               | Where                                              |
| ------------------------------------- | -------------------------------------------------- |
| Schema                                | `crates/uncloud-sync/migrations/004_local_deletion_tracking.sql` |
| Sentinel module                       | `crates/uncloud-sync/src/sentinel.rs`              |
| Journal API additions                 | `crates/uncloud-sync/src/journal.rs`               |
| Engine integration (verify, prune, reconcile, Phase 6a) | `crates/uncloud-sync/src/engine.rs`     |
| Watcher cancel hook                   | `crates/uncloud-desktop/src/file_watcher.rs`       |
| Tests                                 | `crates/uncloud-sync/src/{sentinel,journal}.rs` (unit), `crates/uncloud-sync/tests/local_deletion_tracking.rs` (integration) |
