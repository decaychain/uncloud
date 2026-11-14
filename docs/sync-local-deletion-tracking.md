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
- **A later scan** that still finds the file missing, *and* runs at least
  the confirmation delay after the marker was set, pushes `DELETE
  /api/files/{id}` (the server soft-deletes to Trash) and removes the
  journal row.

The confirmation delay (`DEFAULT_DELETE_CONFIRM_DELAY`, 30 seconds) exists
because "two scans" alone is not a waiting period: the filesystem watcher
fires a sync two seconds after any activity in the root, so a directory
that a build tool, a branch switch or a slow move is still replacing could
be seen missing twice within seconds. A marker that cannot be parsed, or
that lies in the future after a clock change, restarts the wait.

In between, these things cancel the pending state:

| Trigger                                          | Why it cancels                                              |
| ------------------------------------------------ | ----------------------------------------------------------- |
| File reappears locally                           | User restored it; treat as local-newer on the next compare. |
| Server holds a version newer than the journal    | Server change wins; the item is restored (see below).       |
| Watcher fires a Create/Modify event for the path | Tripwire — path was touched, can't trust the absence.       |
| Effective strategy stops permitting deletes      | Strategy gating below.                                      |

The watcher is **only** allowed to cancel, never to commit. Filesystem
watchers are notoriously unreliable (lost events under load, missing
on network mounts, inotify limits) so we don't trust them as the
authority for deletes — but a fired event is positive evidence that
the path is alive, which is enough to drop a pending state.

Right before a delete is pushed, two more checks run:

- **Authoritative absence.** `LocalFs::confirm_absent` must positively
  report that nothing is at the path. On desktop it uses
  `symlink_metadata`, so a dangling symlink (target on an unmounted
  volume) is not a deletion. On Android, where SAF errors carry no kind
  and the quick `is_file` / `is_dir` probes report provider failures as
  "absent", it lists each directory from the tree root down: a name
  missing from a listing that succeeded is absence, a listing that fails
  is an error.
- **Fresh server state.** The tree fetched at the start of the run may be
  minutes old by the time deletes are pushed. The engine fetches it again
  and requires the doomed file, or every item in the doomed folder's
  subtree, to be unchanged. Otherwise the delete is postponed and the next
  scan re-evaluates against the new state. The remaining window is the
  round trip between that check and the `DELETE`; closing it fully would
  need a precondition on the server endpoint.

### Strategy gating

| Strategy          | Tracked item missing locally                                   |
| ----------------- | -------------------------------------------------------------- |
| `TwoWay`          | two-phase delete on the server                                 |
| `ClientToServer`  | two-phase delete on the server                                 |
| `UploadOnly`      | left alone: never deleted, never re-downloaded                 |
| `ServerToClient`  | re-downloaded on the next scan                                 |
| `DoNotSync`       | ignored                                                        |

Removing local copies under a strategy that never pushes deletes is
routine (freeing space under `DoNotSync` or `UploadOnly`), and the journal
rows stay behind. To keep those absences from turning into deletions when
the strategy later changes, the engine records every folder's effective
strategy (`folder_effective_strategy` table). When a folder moves from a
non-delete-pushing strategy to a delete-pushing one, the journal rows of
its missing items are dropped, so they are treated as first-sight: under
`TwoWay` they download again, under `ClientToServer` they are left on the
server untouched.

### Folders

A tracked folder whose directory and entire tracked subtree are gone is
handled as one item. Subtrees are resolved deepest-first, and only the
topmost vanished folder is acted on; server ancestry decides "topmost",
not local path length (a per-folder path override can put a child
elsewhere). A folder is never handled this way while its subtree holds
anything this client has not seen in its current form (an untracked or
server-changed item), anything mapped elsewhere that still exists, or any
descendant whose strategy forbids pushing deletes. Its eligible contents
are then deleted item by item and the folder stays on the server.

**Rename and move.** Before a vanished folder is marked for deletion, the
engine looks for a local directory that has no server folder yet, sits
directly inside a known folder or sync root, and holds every file the
journal tracked in the vanished folder at the same relative path with the
same mtime. If exactly one such pairing exists, the folder was renamed or
moved: the engine sends `PUT /api/folders/{id}` with the new name and/or
parent and repoints the journal rows. Matching requires the stored SHA-256
checksum of every tracked file as well as its relative path and mtime;
timestamps alone are not proof of identity. Checksums are recorded from the
actual bytes of successful uploads and downloads, using the existing journal
column. An older row without a checksum falls back to ordinary creation and
upload, never a speculative rename. File and folder ids, and with them
shares and version history, survive; nothing is re-uploaded. Known gaps,
which fall back to delete-and-create:

- an empty folder (nothing to recognise it by),
- a folder moved into a directory that is itself new,
- a rename combined with edits to the files inside before the next scan,
- two candidates that match equally well,
- single-file renames, which were always delete-and-upload.

**Receiving a rename or move.** Before materializing the new server layout,
the client relocates a tracked folder from its previous local path and rebases
its descendants' journal paths in one transaction. This preserves local edits,
unsynced contents and empty folders, without re-uploading or re-downloading
unchanged files. A move intent is persisted before the filesystem operation;
after interruption, an already-moved directory is only adopted when that intent
matches. Completing the journal transaction also clears the intent. An unrelated
existing destination is never overwritten or adopted.

Unreadable paths, destination collisions, configured sync-base boundaries and
unsupported moves defer the affected subtree with an error, while unrelated
sync continues. Desktop moves use the filesystem rename operation (cross-volume
moves may fail). Android SAF supports renames within one parent directory;
cross-directory moves are safely deferred because the provider wrapper has no
directory-move primitive.

**Delete.** Otherwise the folder goes through the same two-phase commit
as a file and is removed with a single `DELETE /api/folders/{id}`. The
server cascades the trash (sets `deleted_at` on every descendant inside
one `batch_delete_id`), so the user sees one entry in Trash and one row in
the audit log.

### Server change versus local delete

Conflicts are resolved per item: a file or folder the server changed since
the journal last matched it wins over its local deletion and comes back.
Nothing else does. If a folder was deleted locally and one file inside it
changed on the server, that file is restored together with the
directories leading to it; the folder's unchanged files and subfolders are
still deleted on the server once confirmed. Restores are written to the
activity log with the note "Local deletion cancelled: changed on the
server".

A restore is recorded as `pending_download` on the file's journal row
before the download starts and cleared when it succeeds. Until then the
file is never a deletion candidate, its folders are kept, and the retry
overwrites whatever a failed attempt left on disk instead of comparing a
half-written file against the journal as a local edit. This holds across
restarts.

A file that can never be downloaded (an invalid name on this platform,
say) therefore keeps its own folders alive and is retried each scan, but
deletions around it proceed normally.

### Unreadable paths

A failed probe is not evidence of absence, and it is not a reason to stop
syncing everything else. A path whose state cannot be read (permission
denied, I/O error) is skipped for the run: nothing under it is
overwritten, uploaded or deleted, and a folder containing it is never
removed as a whole. Directory probes run before descendant mutations, and a
failed path blocks all paths beneath it for the rest of the run, including
new-file discovery, incoming renames and server-deletion echoes. The failure is counted in the run's errors every time
and written to the activity log once when it starts, not once per poll.
Runs that do abort (journal or server errors) log their reason the same
way.

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

- **Confirmation latency.** Deletes don't reach Trash until a scan
  that runs at least 30 seconds after the one that first noticed the
  absence. With the default 60s poll that's typically 1–2 minutes.
  Worth flagging in user-facing documentation.
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
| Schema                                | `crates/uncloud-sync/migrations/004_local_deletion_tracking.sql`, `005_folder_effective_strategy.sql` |
| Sentinel module                       | `crates/uncloud-sync/src/sentinel.rs`              |
| Journal API additions                 | `crates/uncloud-sync/src/journal.rs`               |
| Engine integration (verify, prune, reconcile, Phase 6a) | `crates/uncloud-sync/src/engine.rs`     |
| Watcher cancel hook                   | `crates/uncloud-desktop/src/file_watcher.rs`       |
| Android absence confirmation          | `crates/uncloud-desktop/src/android_fs.rs`         |
| Tests                                 | `crates/uncloud-sync/src/{sentinel,journal}.rs` (unit), `crates/uncloud-sync/tests/{local_deletion_tracking,client_creates_local}.rs` (integration), `crates/uncloud-server/tests/sync_engine.rs` (against a real server) |
