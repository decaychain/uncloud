# Storage Migration — Offline CLI

## Problem

Uncloud now ships three storage backends (Local, S3, SFTP) and supports per-folder
storage pinning. There is no path today to move a folder's existing files from one
backend to another: changing `Folder.storage_id` only affects *new* uploads. Files
that were already on the old backend stay there until they are deleted and re-uploaded
by hand.

We deliberately punted on online migration when designing the multi-backend feature:
coordinating live writes with a long-running copy is a class of bug we do not want.
Doing it offline — server stopped, no concurrent writes — sidesteps the entire
problem.

## Goal

A CLI subcommand of the existing `uncloud-server` binary that copies all blobs owned
by one storage to another, then atomically flips the `File.storage_id` pointer for
each file. Idempotent, resumable, verifiable.

```
uncloud-server migrate \
    --from <storage-id-or-name> \
    --to   <storage-id-or-name> \
    [--folder <folder-id>]    \
    [--dry-run]               \
    [--verify size|hash|none] \
    [--delete-source]
```

Non-goals:

- Online / live migration. Server must be stopped.
- Migrating between backends of different *types* with format conversion. Blobs are
  copied byte-for-byte; thumbnails and versions ride along unchanged.
- Cross-instance migration (different MongoDB databases). Source and dest live in
  the same Uncloud installation.

## Why a subcommand of the server binary

`uncloud-server` already uses clap with subcommands (`Serve` is the default,
`BootstrapAdmin` exists). Adding `Migrate` reuses:

- Config loading (`config.yaml` → `ConfiguredStorageBackend` factory).
- MongoDB client + connection settings.
- The `StorageBackend` factory and trait — no duplicated plumbing.
- The same Docker image / systemd unit.

A separate `uncloud-migrate` binary would mean re-implementing all of the above for
no real benefit. The subcommand stays in `crates/uncloud-server/src/cli/migrate.rs`.

## Algorithm

The whole design is built around **per-file flip with idempotent retry**. The unit
of progress is one `File` document.

### 1. Acquire a migration lock

Write a row to a new `migration_locks` collection:

```rust
struct MigrationLock {
    id: ObjectId,
    from_storage_id: ObjectId,
    to_storage_id: ObjectId,
    started_at: DateTime<Utc>,
    last_heartbeat: DateTime<Utc>,
    pid: u32,
    hostname: String,
}
```

There is at most one row globally — enforced by a partial unique index. A
heartbeat task refreshes `last_heartbeat` every 30 s. On startup, both
`uncloud-server serve` and `uncloud-server migrate` check this collection:

- `serve` refuses to start if a recent (`< 5 min`) lock row exists.
- `migrate` refuses to start if a recent lock row exists, with a `--force-unlock`
  override for the case where a previous run crashed.

The lock row is deleted on clean exit. A stale row (no heartbeat for 5 min) is
treated as crashed and can be cleared by `--force-unlock`.

### 2. Resolve source and destination

`--from` / `--to` accept either an `ObjectId` (from `Storage._id`) or the
`Storage.name` field, whichever is friendlier. Refuse if they resolve to the same
storage. Refuse if either resolves to a storage that does not exist or is not
healthy (we already have a `verify_storage_health` helper from PR #28).

### 3. Enumerate work

Build the candidate set of `File` documents:

```js
{ storage_id: <from>, ...optional folder filter... }
```

If `--folder <id>` is passed, expand it to the set of all descendant folder ids
(BFS over `Folder.parent_id`) and filter `File.folder_id` to that set. This is
the same code path used by recursive folder copy, so we factor it out into a
shared `services::folders::descendant_ids` helper.

Print the planned work up front:

```
Migrating from "Local" → "MinIO":
  Files:    3,481
  Bytes:    27.4 GiB
  Versions: 412
  Trash:    81
Run with --dry-run to preview, or remove it to proceed.
```

### 4. Per-file copy + flip

For each `File`:

1. **Skip if already on dest.** `file.storage_id == to` → nothing to do.
   This is the property that makes the algorithm resumable: re-running after a
   crash just picks up where it left off.
2. **Copy the blob.** `from.read(file.path).pipe(to.write_stream(file.path, file.size))`.
   Wrap the source reader in a SHA-256 hashing tee so we get the source hash
   for free.
3. **Verify dest.** `to.exists(file.path) && to.size(file.path) == file.size`.
   If `--verify=hash`, additionally hash the dest blob and compare. Default is
   size-only — hashes the source while it streams (cheap) but trusts the dest
   write (no second read).
4. **Migrate sidecars.** For each of `.thumbs/{file_id}.jpg` and any version
   archive paths (see §5), repeat steps 2–3.
5. **Atomically flip the pointer.** Single MongoDB update:
   ```js
   files.updateOne(
       { _id: file._id, storage_id: <from> },
       { $set: { storage_id: <to> } }
   )
   ```
   The `storage_id: <from>` predicate makes this safe against concurrent
   modification (which shouldn't exist while the lock is held, but belt and
   braces).
6. **(Optional) Delete from source.** Only if `--delete-source` was passed.
   Disabled by default — see §7.
7. **Emit progress.** Files and bytes counters, throughput, ETA.

If any step fails, log the file id and continue. At the end, print the failure
list and exit non-zero. The user re-runs to retry.

### 5. Versions and trash

`File` has `version: i32` (current version number) and `deleted_at` / `trash_path`
(soft-delete state). The migration must cover:

- **Active blob** at `file.path` — handled above.
- **Thumbnail** at `.thumbs/{file_id}.jpg` if it exists on source.
- **Versions.** When version history lands (planned), older versions are stored
  at a deterministic archive path. The migration walks `file.versions[]` (or
  whatever the eventual schema looks like) and copies each archived blob.
  *Designed in now so we don't have to retrofit later.*
- **Trash entries.** Files where `deleted_at` is set live at `trash_path` instead
  of `path`. Use `trash_path` as the source/dest key.

Thumbnails are recoverable — if migration drops one, the processing pipeline
will rebuild it on next access. We could exploit that to avoid migrating thumbs
at all (smaller migration set, slightly slower first-access UX). I'd default to
copying them anyway; they are small relative to the source blob and the latency
on first access matters more than the migration time.

### 6. Folder pin update

`Folder.storage_id` only governs *new* uploads. Migration does not need to touch
folder pins — but it's a natural follow-up. Add a `--repin-folders` flag that,
after a successful migration, sets `Folder.storage_id = <to>` on every folder
that previously pinned `<from>` (and was within the `--folder` scope, if any).
This keeps future uploads on the new storage instead of trickling back to the
old one.

### 7. Source cleanup

`--delete-source` deletes the source blob (and thumb / versions) immediately
after each successful pointer flip. Off by default. Three reasons:

- If post-migration smoke tests fail, you want the source intact so you can
  flip pointers back.
- Disk-space reclamation is rarely the urgent thing — usually you migrate
  *to* a new backend, not because you're out of space on the old one.
- The default-safe path is "two copies briefly, then explicit cleanup."

For the explicit cleanup, ship a separate `uncloud-server migrate-cleanup
--storage <id>` that walks `<storage>` and deletes any blob whose owning
`File.storage_id` no longer points to it. This is conceptually a different
operation (verify + sweep) and is cleaner as a separate subcommand.

### 8. Server startup interlock

`uncloud-server serve` reads the `migration_locks` collection on startup. If a
non-stale row exists, it refuses to start with a clear error message:

```
Refusing to start: a storage migration is in progress
  from: Local (5f3...)
  to:   MinIO (5f4...)
  started: 2026-05-01T14:22:00Z
  pid: 12345

Wait for it to finish, or run `uncloud-server migrate --force-unlock` to clear
the lock if the previous run crashed.
```

This protects against the worst failure mode: server starts up alongside a
running migration and accepts uploads to the *old* storage, which then get
silently orphaned when the migration concludes.

## Verification modes

| Mode      | Source side       | Dest side             | Use case                   |
|-----------|-------------------|-----------------------|----------------------------|
| `none`    | none              | none                  | Quick test runs only       |
| `size`    | size from DB      | size from `exists`    | **Default.** Cheap.        |
| `hash`    | SHA-256 streaming | SHA-256 of dest blob  | High-value migrations      |

`hash` doubles I/O on the dest side (write, then read back). Worth it for one-shot
migrations of irreplaceable data; skip it for bulk moves where size + the
backend's own write acks are good enough.

## Failure modes and recovery

| Failure                      | Behaviour                                              |
|------------------------------|--------------------------------------------------------|
| Source blob missing          | Log file id, mark failed, continue. Surface in summary. |
| Dest disk full               | Mark failed, continue. Subsequent files will also fail; user fixes capacity then re-runs. |
| Dest write succeeds, verify fails | Log, mark failed, do *not* flip pointer. Next run re-copies (overwrites partial). |
| Pointer flip fails (e.g. Mongo down) | Log, abort. Next run re-copies (idempotent) and retries flip. |
| Crash mid-file               | Lock heartbeat goes stale → next run sees stale lock → user `--force-unlock`s and re-runs. Idempotent skip handles already-flipped files. |
| Server started during migration | Server's lock check refuses to start. |
| Migration started while server is running | Migration's lock check refuses (because server *should* be holding a different lock — or, more realistically, because the user ran them on different machines). Document clearly: stop the server before migrating. |

## Open questions

1. **How do we surface progress?** The doc above says "print to stdout" but the
   admin UI could plausibly show migration progress too. I'd start CLI-only and
   only build a UI if there's demand.
2. **Should `migrate-cleanup` be the same subcommand with a flag?** I lean
   separate subcommand because the operations have different blast radius
   (cleanup deletes data; migrate creates copies).
3. **Atomicity within a folder?** Right now the algorithm is per-file. If a
   migration is interrupted mid-folder, the folder ends up split across two
   storages. This is fine — the system already supports that — but it means
   "show me which folder's files are on which storage" is a real UX question.
   Probably solved by a "primary storage" column on the folder properties dialog
   that reports majority storage of contained files.

## Implementation outline

A rough cut, for sequencing rather than estimation:

1. **Lock model + server interlock.** New `migration_locks` collection, partial
   unique index, server startup check, heartbeat task. ~150 lines.
2. **`migrate` clap subcommand + arg parsing.** Wire up to existing CLI. Resolve
   `--from` / `--to` to storage ids. ~100 lines.
3. **Core copy loop** for active blobs: enumerate, copy, verify, flip pointer,
   progress. ~250 lines.
4. **Sidecar migration**: thumbnails first, versions and trash slot in once
   their schemas are stable. ~100 lines.
5. **`--delete-source`** path + the separate `migrate-cleanup` subcommand.
   ~150 lines.
6. **`--repin-folders`** flag. ~50 lines.

Tests:

- Unit tests on the copy loop with two `LocalStorage` instances pointed at
  tempdirs. Easy and fast.
- Integration test with `LocalStorage → S3` against a MinIO testcontainer
  (already wired up for `storage_s3.rs`).
- Integration test for resume-after-crash: start a migration, kill the process
  partway, re-run, assert all files end up correctly flipped.
- Integration test for the server-startup interlock.

## Risks

- **Verification false negatives.** Size matches but content doesn't. Mitigated
  by `--verify=hash` for high-value migrations and by streaming source hashing
  in the default path (we *know* the source hash even when not verifying dest).
- **Disk fills up mid-migration.** Source and dest both contain the data
  briefly. Document the ~2x peak storage requirement. `--delete-source` flips
  this trade-off (1x peak, no rollback).
- **MongoDB transaction isn't actually atomic across files.** Each `updateOne`
  is its own write. If you need "either every file is on dest or every file is
  on source," you can't have it. The system already tolerates split state, so
  this is fine — but worth being explicit.
