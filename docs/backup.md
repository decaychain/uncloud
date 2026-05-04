# Backup — Offline CLI to Restic-compatible Repos

## Problem

There is no first-class way to back up an Uncloud instance today. The data lives
in two places:

- File contents, scattered across one or more storage backends (Local / S3 /
  SFTP), each with its own native backup story but no unified one.
- MongoDB documents that describe what the files are, who owns them, where they
  pin, version history, sync events, share links, app data, and so on.

Native per-backend backups (e.g. Borg the Local directory, replicate the S3
bucket) miss the database half. A `mongodump` covers the database but not the
blobs. A backup of just the storage backends is meaningless without the
metadata; a backup of just the database is a list of dangling pointers. We
need both, in one snapshot, with one command, to one or more remote targets.

## Goal

A `backup` subcommand of the `uncloud-server` binary that produces a single
deduplicated, encrypted snapshot containing:

1. A **semantic dump of the database** as portable NDJSON — not BSON / Mongo
   wire format. Engine-agnostic, so a future non-Mongo Uncloud can ingest it.
2. **All file blobs** owned by every configured storage backend, organised by
   logical (user-facing) path.

The snapshot lives in a [Restic](https://restic.net/)-format repository written
via [`rustic_core`](https://docs.rs/rustic_core/) — the Rust library that powers
`rustic-rs` and produces repositories byte-compatible with the official
`restic` CLI. That gives us:

- Native dedup + encryption with no subprocess dependency.
- Multiple backend types out of the box: SFTP-over-SSH, S3 / B2 / R2 / Azure /
  GCS, REST server (with append-only mode), local, and rclone for everything
  else.
- Restore via either our own subcommand or the upstream `restic` / `rustic`
  binaries, on any machine, even without Uncloud installed.

```
uncloud-server backup init    --target <name>
uncloud-server backup create [--target <name>] [--dry-run] [--tag <tag>]
uncloud-server backup list   [--target <name>]
uncloud-server backup check  [--target <name>]
uncloud-server backup prune  [--target <name>]
uncloud-server backup restore --target <name> --snapshot <id-or-latest> \
    [--default-storage <name>] [--conflict-policy abort|overwrite] [--dry-run]
```

Restore is **in-place**: it writes the database state back into MongoDB and the
blobs back into their matching storage backends. There is no "extract to a
path" mode — that doesn't model multi-backend installations (you'd be
downloading every byte from S3 to local disk just to copy it back to S3
afterwards). Storage backends are matched **by name** between snapshot and
destination; unmatched storages fall back to the destination's configured
default. Detailed algorithm in the [Restore](#restore) section.

Non-goals (for v1):

- **Selective restore** (single file by id, single user, single folder
  subtree). The full-snapshot path is enough to ship; selective restore is a
  follow-up that can build on the same engine.
- **Scheduled / cron-driven backups.** Manual CLI-triggered runs only. Adding a
  scheduler is a small follow-up once the create path is solid.
- **Cross-instance restore consistency guarantees.** Restoring an Uncloud A
  snapshot into Uncloud B is best-effort — same Mongo schema version is the
  contract; cross-installation user / share semantics are operator
  responsibility.

## Why a subcommand of the server binary

Same reasoning as `migrate`: the server binary already loads `config.yaml`,
connects to MongoDB, and constructs the `StorageBackend` factory. A separate
binary would re-implement all of that. The subcommand lives in
`crates/uncloud-server/src/backup/` (a module, since `rustic_core` integration
is heavier than `migrate.rs`).

## Snapshot layout

A single Restic snapshot per `backup create` invocation, structured logically
inside the repo. Restic dedups across paths, so naming is presentation-only —
there is no storage cost to organising things for human-friendly restore.

```
/manifest.json                          # snapshot-level metadata (see below)
/database/
    manifest.json                       # DB schema version + per-collection counts
    users.jsonl
    folders.jsonl
    files.jsonl
    file_versions.jsonl
    storages.jsonl
    shares.jsonl
    folder_shares.jsonl
    api_tokens.jsonl
    s3_credentials.jsonl
    sftp_host_keys.jsonl
    apps.jsonl
    webhooks.jsonl
    sync_events.jsonl
    invites.jsonl
    user_preferences.jsonl
    playlists.jsonl
    shopping_lists.jsonl
    shopping_items.jsonl
    shopping_list_items.jsonl
    shopping_categories.jsonl
    shops.jsonl
    task_projects.jsonl
    task_sections.jsonl
    tasks.jsonl
    task_comments.jsonl
    task_labels.jsonl
/files/<owner_username>/<virtual path>      # logical tree as users see it
/versions/<file_id>/<version_id>             # if include_versions (default on)
/trash/<file_id>                             # if include_trash (default off)
```

Snapshot-level `manifest.json`:

```json
{
  "schema_version": 1,
  "uncloud_version": "0.x.y",
  "host": "uncloud-prod-01",
  "started_at": "2026-05-01T14:22:00Z",
  "completed_at": "2026-05-01T14:38:11Z",
  "config_hash": "sha256:...",
  "options": {
    "include_versions": true,
    "include_trash": false,
    "include_thumbnails": false
  },
  "stats": {
    "files": 12345,
    "bytes": 29476139008,
    "errors": 0
  },
  "tags": ["host:uncloud-prod-01", "app:uncloud", "uncloud:0.1.0"]
}
```

Restic snapshot tags mirror the `tags` field plus any user `--tag`. They are
the primary input to `prune`'s retention filter.

### `uncloud:complete` tag — verified-clean marker

A snapshot is tagged `uncloud:complete` only after the run finished and
we confirmed **zero source-read failures** — open-side or mid-stream.
The tag is added post-flight, not optimistically: if the process is
interrupted between `backup_with_source` returning and the tag step,
the snapshot is left untagged, which is the right state ("we didn't get
to verify, so we can't claim it's complete"). Untagged means "not
verifiably complete," which covers:

- runs where some files genuinely couldn't be read (storage 5xx, missing
  versions, permission issues),
- runs that crashed or were killed before reaching the post-flight check,
- snapshots created before this feature shipped.

**Side effect**: snapshots are content-addressed, so adding a tag means
writing a new snapshot file with a new id and deleting the original.
The id printed during `backup_with_source` (in rustic's logs) refers
to the pre-tag, now-deleted snapshot; the on-disk id after a clean run
is different. The `backup create` summary intentionally omits the id
in the clean case and points users at `backup list` for the current
id. Partial runs don't have this wrinkle — no retag, no id change.

## Semantic database dump

NDJSON per collection — one JSON object per line. Lossless but engine-neutral.

### Type mapping

Our domain models use BSON-specific serde adapters
(`chrono_datetime_as_bson_datetime`, `models::opt_dt`) that emit canonical EJSON
when fed through `serde_json` — wrong for a portable dump. The dumper
therefore reads BSON `Document`s straight from the cursor and runs them through
a shared `bson_to_portable_json(Bson) -> serde_json::Value` helper that maps:

| BSON type                | JSON representation              |
|--------------------------|----------------------------------|
| `ObjectId`               | 24-char hex string               |
| `DateTime`               | RFC 3339 string (UTC)            |
| `Binary`                 | `{ "$binary": "<base64>" }`      |
| `Decimal128`             | string                           |
| `Int32` / `Int64`        | JSON number                      |
| `Document` / `Array`     | recursed                         |
| `Null`                   | `null`                           |
| `Boolean` / `String` / `Double` | as-is                     |

No `{"$oid": ...}`, no `{"$date": {"$numberLong": ...}}`. A future SQLite- or
Postgres-backed Uncloud reads the same files.

### Collection allowlist (explicit, not auto-discovery)

Anything new that needs backing up must be added here consciously.

**Include** — all persistent business data:

```
users, folders, files, file_versions, storages, shares, folder_shares,
api_tokens, s3_credentials, sftp_host_keys, apps, webhooks, sync_events,
invites, user_preferences, playlists, shopping_lists, shopping_items,
shopping_list_items, shopping_categories, shops, task_projects,
task_sections, tasks, task_comments, task_labels
```

**Skip**:

| Collection             | Reason                                              |
|------------------------|-----------------------------------------------------|
| `sessions`             | Force re-login after restore (snapshot replay risk) |
| `totp_challenges`      | TTL'd; ephemeral                                    |
| `upload_chunks`        | TTL'd; ephemeral resumable-upload state             |
| `migration_locks`      | Transient lock state                                |
| `backup_locks`         | Transient lock state                                |

### Schema versioning and restore contract

`/database/manifest.json` carries a top-level `schema_version` plus per-collection
`schema_version` fields. `backup restore` (eventually `backup restore --apply`)
refuses snapshots with a `schema_version` newer than it knows how to ingest, and
either supports older versions via a thin migration step or emits a clear error.
v1 has `schema_version = 1`.

### Indexes are not backed up

Indexes are derived data. Restore reapplies them via the existing
`db::setup_indexes` and `db::setup_sync_audit_indexes`. Documented in the
restore section below.

### Consistency

Online runs produce a *per-collection-consistent* dump, not a *cross-collection-
consistent* one. A folder/file create or delete during the dump window can leave
the snapshot with, say, a `files` row whose `parent_id` doesn't exist in
`folders`. Acceptable for v1 — the file-blob walker has the same eventual-
consistency envelope, and the storage-migration design already accepts this
class of drift. Documented; not engineered around.

A future `consistency: snapshot` config flag could opt into Mongo's snapshot
read concern (replica-set-only) for cross-collection consistency. Out of scope
here.

## File blob walk

The blob walker streams each `File`'s bytes straight from its storage
backend into rustic — no full-dataset local staging, regardless of where the
data lives.

For each `File` document on the include list (i.e. not skipped — currently no
file is, but `--folder` scoping or per-user filters could land later):

1. Resolve to its storage backend via `file.storage_id`.
2. Open `backend.read(file.storage_path)` (or `file.trash_path` for soft-deleted
   files when `include_trash`).
3. Stream the bytes into rustic at logical path
   `/files/<owner_username>/<full virtual path>`.
4. If `include_versions`, also stream each entry from
   `file_versions` at `/versions/<file_id>/<version_id>`.
5. Read errors are logged, the file is skipped, and the run prints a
   final WARNING summary listing the failure count. (Tagging the snapshot
   itself with `partial` is a follow-up — rustic's snapshot mutation API
   can grow it later. For now the surface is "snapshot exists; run output
   tells you how complete it is.")

Streaming relies on a small patch to `rustic_core` we maintain on a
[fork](https://github.com/decaychain/rustic_core/tree/uncloud/backup-with-source):
a single new `Repository::backup_with_source` method that takes any
`&impl ReadSource` and bypasses the stock `LocalSource` walk over filesystem
paths. Without that, the public `Repository::backup` API forces every
backed-up byte through a local path, which doesn't fit the multi-backend
model. The patch is small and self-contained; we'll upstream it once the
shape stabilises.

Rustic chunks and dedups as bytes flow in, so re-running a backup against the
same target is incremental even though our walker re-streams every byte every
time. The wire cost is the chunks that *changed*, not the full corpus.

Thumbnails (`.thumbs/<file_id>.jpg`) are skipped by default — they are
derivable, the processing pipeline regenerates them on demand, and including
them roughly doubles small-file metadata churn. Toggle with
`include_thumbnails: true` if a particular target wants them anyway.

## Configuration

```yaml
backup:
  options:
    include_versions: true
    include_trash: false
    include_thumbnails: false
    staging_dir: /var/lib/uncloud/backup-staging   # default: $TMPDIR
    # Cap simultaneous open `read()` calls against the source storage
    # backend. Each in-flight reader holds one connection / file handle.
    # SFTP servers (Hetzner Storage Box, OpenSSH stock config) limit
    # concurrent SFTP handles per session and rayon's full archiver
    # parallelism hits that cap fast. Default 8 — conservative for
    # shared-tenant SFTP, plenty for S3 / local.
    # max_concurrent_source_reads: 8

  targets:
    - name: nas
      # SFTP supports two URI forms:
      #   sftp://[user@]host[:port][/path]   — URL style, custom port
      #   sftp:[user@]host:/path             — legacy, default port 22
      # Auth is key-based (OpenDAL's SFTP service has no password
      # field — it wraps the system `ssh` binary with `BatchMode=yes`,
      # which forbids any interactive prompt). Point `credentials.key`
      # at a *private* key file; the public half lives on the server in
      # `~/.ssh/authorized_keys`. Mode must be 0400 or 0600 (no group
      # / other access) and the file must be readable by whichever
      # user runs the backup — uncloud now pre-checks both and errors
      # clearly if either is wrong, so you don't end up debugging an
      # opaque "connection request: timeout" from the openssh wrapper.
      repo: "sftp://backup@nas.lan:2222/srv/backups/uncloud"
      # password sources for the snapshot ENCRYPTION key (Restic
      # crypto, separate from the SSH key) — first present wins
      password_file: /etc/uncloud/backup-nas.key
      # password_env: UNCLOUD_BACKUP_NAS_PW
      # password_command: "pass show uncloud/backup-nas"
      credentials:
        key: /etc/uncloud/keys/backup-nas
        # user: backup                     # overrides URI's user@ part
        # known_hosts_strategy: strict     # strict | accept-new | accept-unknown
        # pool_size: 4                     # bb8 SSH pool size (default 64).
        # Lower for shared-tenant SFTP (Hetzner Storage Box ~= 5
        # simultaneous connections per subaccount). Available because
        # we patch OpenDAL's hardcoded max_size(64) — see workspace
        # Cargo.toml [patch.crates-io].
      retention:
        keep_last: 5
        keep_daily: 7
        keep_weekly: 4
        keep_monthly: 12

    - name: b2
      repo: "b2:bucket-name:uncloud"
      password_env: UNCLOUD_BACKUP_B2_PW
      credentials:
        b2_account_id: "..."
        b2_account_key_env: B2_KEY    # indirect; resolved at run time

    - name: minio
      repo: "s3:http://minio:9000/uncloud-backup"
      password_file: /etc/uncloud/backup.key
      credentials:
        access_key_id: "..."
        secret_access_key_env: UNCLOUD_BACKUP_S3_SECRET
```

Repo URIs use Restic's familiar scheme (`sftp:`, `s3:`, `b2:`, `azure:`,
`gs:`, `rest:`, `rclone:`, plain path = local). `rustic_backend::BackendOptions::repository(...)`
parses these directly.

`backup create` with no `--target` runs sequentially against every configured
target. `--parallel` is a future opt-in flag; not v1.

### Secret handling

Each repo password and each backend credential resolves from one of:

- `*_file: /path/to/secret` — file contents (trimmed)
- `*_env: VAR_NAME` — environment variable
- `*_command: "shell command"` — stdout (trimmed)
- inline (e.g. `password: "..."`) — strongly discouraged but supported for
  dev / test

Plaintext secrets in `config.yaml` log a warning at startup.

## Lock & interlock

A new `backup_locks` collection mirrors `migration_locks`:

```rust
struct BackupLock {
    id: ObjectId,
    scope: String,        // always "global"
    target: String,       // which target this run is writing to
    started_at: DateTime<Utc>,
    last_heartbeat: DateTime<Utc>,
    pid: u32,
    hostname: String,
}
```

Singleton-by-scope unique index on `scope`. Heartbeat every 30 s. Stale
(`last_heartbeat` older than 5 min) treated as crashed and clearable via
`--force-unlock`.

Cross-feature interlock:

- `backup create` refuses if a `migration_locks` row exists.
- `migrate` refuses if a `backup_locks` row exists.
- `backup create` against multiple targets uses **one** lock for the whole run,
  not per-target — concurrent fan-out is opt-in only and not v1.

The server is **not** required to be stopped (unlike `migrate`). Running while
the server is up is the explicit design — otherwise scheduled backups will
never get adopted. Drift during the run is accepted (see "Consistency" above).

## Verification & integrity

- `backup check --target=N` runs `rustic_core`'s `check` operation — verifies
  pack integrity and (with `--read-data`) re-reads every chunk to detect
  bitrot. Slow but thorough.
- The end-of-`create` summary reports total bytes, dedup ratio (data added vs
  data referenced), files processed, and per-collection row counts.
- Snapshots are tagged `partial` if any file failed to read; the operator can
  decide whether to re-run or accept the partial snapshot.

## Restore

```
uncloud-server backup restore \
    --target <name> \
    --snapshot <id-or-latest> \
    [--default-storage <name>] \
    [--conflict-policy abort|overwrite] \
    [--dry-run]
```

In-place restore writes the snapshot's database state back into MongoDB and its
file blobs back into their matching storage backends, on the same Uncloud
installation that the operator is running the command from. Two realistic
deployment shapes are explicitly supported:

- **Restore on top of an existing installation** to roll back to a prior
  point in time. Most or all storage names line up between the snapshot and
  the destination. Conflict policy is required because data already exists.
- **Restore onto a fresh installation** after a total loss. Storage names may
  partially line up (operator restored `config.yaml` from another backup) or
  not at all (operator built a clean install with new storages). Anything
  unmatched falls back to the destination's default storage.

Restore is **offline** — it requires the server to be stopped. The same lock
collection (`backup_locks`) and startup interlock used by `backup create` and
`migrate` apply.

### Algorithm

1. **Acquire lock**, refuse if a `migration_lock` or different `backup_lock`
   exists. Same singleton-by-scope pattern as elsewhere.

2. **Open the repo**, fetch snapshot manifest, validate `schema_version`.
   Refuse if the snapshot's schema is newer than this binary understands.

3. **Build the storage remap.** Read `/database/storages.jsonl` from the
   snapshot — for matching purposes only, never inserted. For each snapshot
   storage:
   - Look up by `name` in the destination's current `storages` collection
     (which is bootstrapped from `config.yaml` on startup, so name-matching
     is operator-friendly).
   - Match → record `snapshot_storage_id → destination_storage_id`.
   - No match → fall back to either `--default-storage <name>` if passed, or
     the storage flagged `is_default: true` in the destination.
   - If neither a match nor a default exists, abort with a clear error.

   Print the full remap plan up front, e.g.:
   ```
   Storage mapping:
     "Local"  →  "Local"      (matched)
     "MinIO"  →  "MinIO"      (matched)
     "Backup" →  "Local"      (default, no match)

   Files affected by remap: 412 / 12,345
   ```
   Require explicit `--yes` (or `--dry-run` to preview only) before
   proceeding.

4. **Choose conflict policy.** Two modes for v1:
   - `abort` (default) — refuse if any document or blob would collide. Forces
     the operator to start from a known-empty destination or pick `overwrite`
     consciously.
   - `overwrite` — clobber existing documents and blobs. Requires
     `--yes-i-know-this-is-destructive` (extra confirmation flag, separate
     from `--yes`).

   `skip` (insert only what's missing) is a follow-up; not v1.

5. **Restore database collections** in dependency-friendly order (users →
   storages-already-skipped → folders → files → file_versions → shares →
   folder_shares → the rest). For each row from `/database/<collection>.jsonl`:
   - Apply the remap to any `storage_id` field on the document
     (`File.storage_id`, `Folder.storage_id`).
   - For `sftp_host_keys`, remap `storage_id`; drop entries whose source
     storage didn't match any destination (the destination will TOFU-pin its
     own keys on first use).
   - For `storages` itself: skip — destination's own rows are authoritative.
   - All other ObjectIds (user ids, folder ids, file ids, etc.) are
     preserved as-is. Internal references stay consistent because the whole
     snapshot is a self-contained graph.
   - Insert (or replace, depending on conflict policy).

6. **Restore file blobs.** For each newly-inserted (or overwritten) `File`:
   - Resolve the destination backend via the remapped `storage_id`.
   - Stream the blob from the snapshot's `/files/<owner>/<virtual path>`
     into `backend.write_stream(file.storage_path, ..., file.size)`.
   - For each `file_versions` row, stream from `/versions/<file_id>/<version_id>`
     into the appropriate version archive path.
   - Failures are logged; the file id is added to a per-run failure
     report. The restore continues to maximise recovered data — the
     operator decides whether to retry the failed subset or accept partial.

7. **Indexes.** No restore-specific work — `setup_indexes` runs on next
   `serve`. Document this so the operator knows nothing more is needed
   before starting the server.

### What gets remapped vs preserved

| Field / collection                | Remapped?                                       |
|-----------------------------------|-------------------------------------------------|
| `storages` rows                   | **Skipped entirely** — destination keeps its own |
| `File.storage_id`                 | Remapped via name-matching (with default fallback) |
| `Folder.storage_id` (pin)         | Remapped via name-matching (with default fallback) |
| `sftp_host_keys.storage_id`       | Remapped; entries with no match dropped         |
| All other ObjectIds (`_id`, `owner_id`, `parent_id`, `user_id`, ...) | Preserved as-is |
| Indexes                           | Re-applied from `setup_indexes` on next startup |

### Selective restore (deferred)

A future `backup restore --file <id>` and/or `--user <id>` adds the ability
to recover a subset without doing full DR. Same engine, narrower scope: the
remap + blob-stream stage runs only over the filtered set, the DB stage is
either skipped (file-only recovery) or filtered (user-only recovery). Out
of scope for v1, but the v1 code is structured so the filter is a parameter,
not a rewrite.

## Failure modes

| Scenario                          | Behaviour                                              |
|-----------------------------------|--------------------------------------------------------|
| Repo unreachable mid-run          | rustic chunks commit atomically; snapshot is finalised only at the end. Failed run leaves no snapshot in `list`. Re-run resumes via dedup — only changed chunks re-uploaded. |
| Single file unreadable            | Log file id, skip, continue. Run prints a final WARNING summary with the failure count; snapshot exists but is partial. |
| Mongo cursor failure mid-dump     | Abort whole run, release lock, no snapshot finalised.   |
| Crash mid-run                     | Lock heartbeat goes stale → next run sees stale lock → user `--force-unlock`s and re-runs. No partial snapshots to clean up (they were never finalised). |
| Out-of-disk on `staging_dir`      | DB dump and per-blob temp files stream through staging; rustic chunks them as they arrive. Document the few-MB ceiling — peak usage is roughly one in-flight blob plus the DB dump. |
| Concurrent migration              | Refuses with clear error. Same the other way round. |
| Repo password wrong               | Refuses with clear error before any work happens. |
| Restore: snapshot schema newer than binary | Refuses up front with clear error.            |
| Restore: storage name unmatched, no default | Refuses up front with full mapping report.   |
| Restore: blob write failure mid-restore | Log file id, continue, surface in summary. Operator retries failed subset. |
| Restore: DB conflict in `abort` mode | Refuses before touching anything. Mapping report includes the colliding ids. |

## Implementation outline

Rough sequencing, not estimation:

1. **Lock model + interlock.** New `BackupLock` model, partial unique index,
   migrate/backup mutual refusal, heartbeat task. Mirrors the `MigrationLock`
   work in PR #30 nearly line-for-line. ~150 lines.
2. **Config schema.** `BackupConfig` with targets, options, secret resolution
   helpers shared with the existing storage-credentials path. ~200 lines.
3. **`backup` clap subcommand group** (`init`, `create`, `list`, `check`,
   `prune`, `restore`). Argument parsing, target resolution, password
   resolution. ~150 lines.
4. **`bson_to_portable_json` helper + per-collection dumper.** Single transform
   function plus a registry of `(collection_name, dump_fn)` tuples. ~250 lines
   including tests on fixtures.
5. **Rustic integration**: open / init repo, push files via the
   `rustic_core::Repository::backup_streams` (or equivalent — exact API name TBD
   while reading the crate docs). Snapshot manifest writer. ~250 lines.
6. **File-blob walker**: cursor over `files` collection, resolve storage
   backend per file, stream into rustic. ~200 lines.
7. **`backup list` / `check` / `prune`** wrappers around `rustic_core`
   operations. ~100 lines.
8. **In-place restore.** Storage remap (name-match → destination default),
   conflict-policy enforcement, dependency-ordered DB reinsertion with
   `storage_id` rewriting on the way through, blob streaming back into the
   matched backends. The biggest single chunk. ~400 lines.

Tests:

- Unit tests on `bson_to_portable_json` against a hand-rolled fixture
  covering every BSON type our models produce. Fast, runs in CI.
- Integration test: `LocalStorage` source → local rustic repo → list →
  restore → verify both DB JSONL and file bytes round-trip. Uses
  testcontainers Mongo.
- Integration test: fault-inject a file-read failure mid-run; assert
  snapshot tagged `partial` and the run still completes.
- Integration test: `--force-unlock` clears a stale lock; create succeeds
  on retry.
- Manual test against MinIO (S3 backend in rustic) — out of CI but on the
  test plan.

## Open questions / deferrals

1. **Selective restore.** `--file <id>`, `--user <id>`, `--folder <id>`. The
   v1 engine is structured to make this a filter parameter, but the UX,
   conflict semantics, and dependency-walk for partial restores deserve their
   own design pass. Defer.
2. **`skip` conflict policy** (insert only what's missing on the destination).
   Useful for filling gaps after partial corruption. v1 ships `abort` and
   `overwrite` only.
3. **Scheduling.** Cron-style or systemd-timer-driven runs. A small wrapper
   over `backup create`; defer until v1 is exercised in practice.
4. **Encryption-key rotation.** Restic supports adding/removing keys on a repo;
   we don't expose it in v1. Operators use the `rustic` / `restic` CLI directly
   if they need it.
5. **App data outside the main DB.** The App Platform stores app-specific data
   in the main collections; apps that allocate their own MongoDB collections
   would need to be added to the allowlist. Document the contract: "an app
   that wants to be backed up must use the main collections or register its
   own under the backup allowlist."
6. **Sessions.** Currently skipped on security grounds. If post-restore re-login
   becomes operationally painful (e.g. for the desktop apps with stored
   tokens), reconsider — but the right answer there is probably a separate
   "trusted device" mechanism, not session resurrection.

## Risks

- **`rustic_core` is pre-1.0.** The crate explicitly says its API is subject
  to change. We pin a specific version, and we wrap rustic calls in a thin
  `backup::repo` module so an API churn affects one file, not the whole
  feature.
- **Restic format != Borg.** Operators expecting drop-in Borg compatibility
  will be disappointed. Documented up front.
- **Encrypted repo passwords are write-only.** Lose the password, lose the
  data. The config doc must shout this; the `backup init` UX should print a
  big "save this password somewhere safe" warning.
- **Per-collection dump consistency drift.** A foreign-key-equivalent reference
  in one collection may point at a row that no longer exists in another by the
  time it's dumped. Documented; v1 accepts it.
