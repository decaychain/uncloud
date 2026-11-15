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
timestamps alone are not proof of identity. File and folder ids, and with
them shares and version history, survive; nothing is re-uploaded. Known
gaps, which fall back to delete-and-create:

- an empty folder (nothing to recognise it by),
- a folder moved into a directory that is itself new,
- a rename combined with edits to the files inside before the next scan,
- two candidates that match equally well,
- single-file renames, which were always delete-and-upload,
- a file whose checksum is not on record (see below).

**Checksums.** The journal records the SHA-256 of the bytes of every
successful upload and download. Rows written by older builds have none;
they are filled in from the server, which reports each file's checksum in
the sync tree (`checksum_sha256`): while a row still names the server's
current version, the bytes last transferred are the bytes the server
holds, so no local file has to be read. Against a server that predates
the field such rows stay without a checksum and a rename of their folder
falls back to create-and-upload, never a speculative rename.

Local files are hashed in fixed-size chunks (`LocalFs::sha256`), so a
large file costs no more memory than a small one. Results are remembered
per path and mtime for rename matching across successive runs. Copy-move
recovery always hashes both copies afresh before deciding which one may be
overwritten; matching timestamps do not prove that content is unchanged.

**Receiving a rename or move.** Before materializing the new server layout,
the client relocates a tracked folder from its previous local path and rebases
its descendants' journal paths in one transaction. This preserves local edits,
unsynced contents and empty folders, without re-uploading or re-downloading
unchanged files. A move intent is persisted before the filesystem operation
and cleared with the journal transaction, or as soon as a rename fails
(a failed rename moved nothing). Intents that no longer describe the
journal and the server layout are dropped at the start of the next run.

What happens depends on what is found at the old and the new path:

| Old path | New path | Outcome |
|---|---|---|
| present | absent | The directory is renamed. |
| present | the same directory (case-only rename on a case-insensitive filesystem) | Renamed in place; `LocalFs::same_entry` tells this apart from a collision. A backend that cannot change only the case in one step (Android SAF, which would store `Docs` as `Docs (1)` while `docs` exists) refuses with `MoveUnsupported`, and the folder is parked as `<name>.uncloud-moving` and renamed from there. It is never copied onto its own other spelling. |
| present | another tracked folder that is itself about to move | The folder waits for it. Chains resolve in one run in dependency order; a ring (two folders swapping names) is broken by parking one folder as `<name>.uncloud-moving`, with the journal following it. |
| present | anywhere, while an ancestor folder is still waiting to move | The folder waits for the ancestor and travels with it. Its own paths are derived from the ancestor's, so moving it first would put it inside whatever still occupies the ancestor's new name. |
| present | the folder of a server-deleted item | The deletion echo clears it later in the same run and the rename lands on the next one. |
| present | anything else | Error: "a different folder already exists". Nothing is overwritten or merged. |
| absent | present, recorded intent matches | An interrupted move; the journal is completed. |
| absent | present, holds at least one tracked file and every tracked file has the same mtime and checksum | The same rename was made by hand; the directory is adopted. Empty folders provide no identity evidence and need a matching intent. |
| absent | present, anything else | Error, as above. |
| absent | absent | The journal follows; the missing folder is then an ordinary local deletion. |

**Moves the backend cannot do in one step.** A move to another volume on
desktop, or to another directory through Android SAF, fails with
`LocalFsError::MoveUnsupported` and is done by copying: every file is
copied (`LocalFs::copy_file`, streamed), and only then is the original removed.
Each file is read three times: once to copy it, and once on each side for the
SHA-256 comparison made right before the original is removed. That comparison
never relies on mtimes. The destination is listed once up front instead of
being probed per file, which on SAF would list every directory from the tree
root each time.

Nothing is copied unless everything can be. `LocalFs::uncopyable` lists what
the walkers would leave out (symlinks, special files, sync state files) and
fails on a directory it cannot read, and a tracked file that exists but was
not listed stops the move as well. The folder then stays where it is with an
error naming the entry, and no move intent is kept. Without this, a file in an
unreadable directory would be left behind while its journal row followed the
folder, and would then be deleted on the server as missing.

A copy that fails must not leave something behind that later looks like a
file. Three things keep that from needing the user:

- On desktop `NativeFs::copy_file` writes to a staging file beside the
  destination and renames it into place, so a copy that dies half-way never
  leaves a truncated file under the final name. The staging file is removed
  by the sweep described below.
- When `copy_file` returns an error, the engine removes the destination in
  the same breath. What is there at that moment is what the call just wrote
  over a file that was absent or already vouched stale, so nothing can be
  lost. The next run copies the file again.
- When the client was killed in the middle of a copy on a backend that cannot
  replace a file in one step (SAF), the truncated file is still there after
  the restart. Copies are written front to back, so it is the beginning of
  its source and nothing more. A marked destination that is such a strict
  prefix is copied again.

The intent records that a copy is under way and which file is being written.
That marker alone is not permission to overwrite its destination, because the
user may have edited it after the interruption. The marked destination is
checked before another copy can replace the marker. On recovery, a
destination file that differs from its source is settled as follows, using
fresh SHA-256 hashes:

| Destination file | Outcome |
|---|---|
| the file that was being written when the copy stopped, and a strict prefix of its source | A cut-off copy. Copied again. The one thing this can undo is a user truncating exactly that file after the interruption; no content is lost, since the source holds all of it. |
| the file that was being written when the copy stopped, anything else | Error; both copies are kept for manual resolution, because it was edited. An identical copy is reused, and a missing destination is copied again. |
| matches the journal's checksum, source does not | The source was edited after the copy. It is copied again over the stale destination. |
| differs, source matches the journal's checksum | The destination was edited after the copy. It is kept and the source removed. |
| anything else (both edited, or no checksum on record) | Error: "move destination differs from its source". Both are kept for manual resolution. |

A source that changes between its copy and the final comparison fails that
run with "file changed during move"; the next run settles it by the table
above. Once the move is done, a file known to hold what was last synced is
recorded with the mtime it has at the new location, so a backend that cannot
carry mtimes across (SAF) does not make it read as a local edit. A file known
to differ is recorded so that it does read as one, even if the edit kept the
mtime, and is uploaded. The new paths, content-derived mtimes, and removal
of the move intent commit in one journal transaction. If that transaction
fails or the process stops before it commits, the old paths and intent
remain. Recovery rehashes the destination even when the old directory is
already gone, preserving edited-file detection without re-uploading
unchanged files.

To resolve an interrupted copy that was edited, preserve the edited version
outside the moving directories, remove the destination file, and retry sync.
Alternatively, make the destination identical to the source. The error names
both files and says so. Apart from a strict prefix of its source, recovery
never discards a differing in-flight copy on its own.

While a move waits for the user, the verdict is remembered in memory with the
mtimes of the two files it stopped on. Later runs repeat the error without
reading the folder again, until either mtime changes, ten minutes pass, or
the client restarts. The memo only ever repeats an error; a decision to copy
or remove something is always made from fresh hashes.

**Files that never arrived.** An interrupted copy leaves the old directory
complete and the new one partial, and clearing the old one away as a
leftover is the obvious thing to do. The rows of files and subfolders that
never made it across must then not follow the folder, or they would read as
local deletions and take the server copies with them. Whenever a copy-move is
completed from a recorded intent, rows with nothing at the matching place
under the new path are forgotten instead, and the items are downloaded again
like any first sight. This holds when the old directory is gone, when only
its contents are, and when both directories are; in the last case the
folder's own row is forgotten too. The cost is that a file deleted on purpose
while its folder was half-moved comes back once.

When the old directory is gone and the journal update of a finished copy was
lost, the files at the destination are compared with the journal's
checksums. A file with no checksum on record is left to the ordinary mtime
comparison instead of being taken for edited.

**Strategies.** Only strategies that apply server changes locally
(`TwoWay`, `ServerToClient`) follow an incoming rename. Under
`ClientToServer` and `UploadOnly` the local directory keeps its name and
the folder stays linked by id: for as long as the names differ, the folder
and everything beneath it resolve to the journal's path. Switching to a
strategy that takes server changes applies the rename then.

**Mapping changes.** Giving an already-synced folder its own local path
(or changing it) is not a server rename and moves nothing. What the
journal tracked at the old location is forgotten and the folder syncs at
the new one as if seen for the first time. The old directory is left
alone: both default locations and retired custom locations are excluded from
discovery, so they are neither re-created on the server as new folders nor
read as deletions. Retired paths persist in the journal's `sync_config`,
atomically with mapping changes or forgetting detached rows, and survive
restarts. A retired path is released when a folder is mapped to it again
or when nothing exists there any more, so a directory created at it later
is a new folder like any other.

Native downloads and copies use uniquely named, exclusively created
neighboring staging files (`.uncloud-download-<uuid>.uncloud-part`) and
replace the destination only when complete. A download keeps the permission
bits of the file it replaces; a copy carries those of its source. Walkers skip only that internal naming pattern, not ordinary user
files ending in `.uncloud-part`; pre-existing neighbors and symlinks are never
opened as staging files.

A download or copy that is killed or cancelled leaves its staging file behind. Being
invisible to the walkers, it would keep its directory from ever being emptied:
a folder deleted on the server could not be removed locally and would be
created on the server again. `LocalFs::sweep_staging` therefore removes
leftovers under every verified sync base at the start of the first run after
the client starts, and of any run that follows one in which a download or
a copy started. On Android downloads are spooled in the app's cache directory, and
that is what is swept there.

**Conflict copies.** When both sides changed a file, the local version is
copied to `<name> (conflict <date>)` and the server version is downloaded
over the original. If the copy cannot be written the download does not
happen: the local file stays as it is and the failure is reported, to be
retried on the next run.

**Uploads.** Where the backend can state a file's size (`LocalFs::size`,
desktop only), the upload declares it and the request carries a
`Content-Length` instead of chunked transfer encoding. The declared length
is binding: the stream stops there even if the file has grown since, so the
checksum on record is that of the bytes the server received, and a file that
shrank fails the request. The mtime written to the journal is the one read
before the upload began, so a file that changed while it was being sent
still reads as edited on the next scan. Android reports no size, because a
storage provider that misreports one would truncate the upload.

Unreadable paths, genuine collisions and a directory that contains a
folder with its own local path defer the affected subtree with an error
that names the cause, while unrelated sync continues. On Android the
rename is verified: a provider that would store the folder under a
sanitised or suffixed name has the rename undone and reported, so the
journal never points at a path that does not exist.

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
new-file discovery, incoming renames and server-deletion echoes. The
failure is counted in the run's errors every time and written to the
activity log once when it starts, not once per poll. Runs that do abort
(journal or server errors) log their reason the same way.

SAF errors carry no kind, so on Android a failed lookup alone cannot tell
"not there" from "the provider failed". When the quick probe fails,
`AndroidSafFs` consults the directory listings on the way down from the
tree root: a name missing from a listing that succeeded is absent, a name
that is listed but could not be read is an error, and a listing that fails
is an error. Listings are cached for one run (`LocalFs::begin_run` clears
them) and dropped for a directory the engine removes from or renames in.
Deletions are still confirmed against fresh listings, never the cache.

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
