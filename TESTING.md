# Uncloud — Testing Guide

## Strategy

API integration tests run against a real MongoDB instance managed by Docker via the
`testcontainers` crate. Each test function gets a fresh database (UUID-named); the
Docker container itself is started once per test binary and reused across tests in
that run.

File storage uses a `TempDir` per `TestApp` instance, so every test runs against its
own isolated directory.

The processing pipeline is disabled in tests (no processors registered) to prevent
background thumbnail tasks from racing test assertions.

## Running Tests

```bash
# Requires Docker to be running
cargo test -p uncloud-server
```

Docker must be accessible without `sudo` (user should be in the `docker` group).

## Test Structure

```
crates/uncloud-server/
  src/
    lib.rs                ← library crate; exports AppState + all modules
    main.rs               ← binary entry point only
  tests/
    common/
      mod.rs              ← TestApp helper (container, TempDir, TestServer)
    auth.rs               ← Authentication tests (login/logout/sessions)
    auth_e2e.rs           ← Extended auth: registration modes, password change, admin user lifecycle
    files.rs              ← File upload/download/CRUD + duplicate-name constraints
    folders.rs            ← Folder management tests
    sync.rs               ← Sync strategy tests (effective_strategy, sync/tree)
    sync_engine.rs        ← End-to-end two-way sync algorithm tests (uncloud-sync against a real server)
    storage_migration.rs  ← Offline `migrate` subcommand: idempotency, hash verification, version archives, cleanup
    storage_s3.rs         ← S3Storage backend against a MinIO container
    storage_sftp.rs       ← SftpStorage backend (password + private-key auth, TOFU host-key pinning)
    backup.rs             ← Backup dump → restore round-trip (marked `#[ignore]` — needs Mongo)
```

## Dependencies (dev)

| Crate | Purpose |
|---|---|
| `testcontainers` | Start/stop Docker containers from Rust |
| `testcontainers-modules` | Pre-built MongoDB container |
| `axum-test` | In-process TestServer with cookie jar |
| `tempfile` | Isolated storage directories per test |

## Coverage

### Authentication (`tests/auth.rs`)

| Test | Route | Expected |
|---|---|---|
| `register_success` | `POST /api/auth/register` | 201 + user fields |
| `register_duplicate_username` | `POST /api/auth/register` | 409 |
| `register_duplicate_email` | `POST /api/auth/register` | 409 |
| `register_short_password` | `POST /api/auth/register` | 400 |
| `login_success` | `POST /api/auth/login` | 200 + session cookie |
| `login_wrong_password` | `POST /api/auth/login` | 401 |
| `login_unknown_user` | `POST /api/auth/login` | 401 |
| `me_authenticated` | `GET /api/auth/me` | 200 + current user |
| `me_unauthenticated` | `GET /api/auth/me` | 401 |
| `logout_clears_session` | `POST /api/auth/logout` | 204; /me → 401 after |
| `session_list` | `GET /api/auth/sessions` | includes current session |
| `revoke_session` | `DELETE /api/auth/sessions/{id}` | 204; /me → 401 after |

### Files (`tests/files.rs`)

| Test | Route | Expected |
|---|---|---|
| `upload_creates_file` | `POST /api/uploads/simple` | 201; appears in listing |
| `download_returns_content` | `GET /api/files/{id}/download` | bytes match upload |
| `rename_file` | `PUT /api/files/{id}` | new name in listing |
| `delete_file` | `DELETE /api/files/{id}` | 204; absent from listing |
| `user_isolation` | `GET /api/files/{id}` | user B gets 404 for user A's file |
| `auth_required` | all file routes | 401 without session cookie |

### Folders (`tests/folders.rs`)

| Test | Route | Expected |
|---|---|---|
| `create_folder` | `POST /api/folders` | 201; appears in listing |
| `folder_breadcrumb` | `GET /api/folders/{id}/breadcrumb` | correct ancestor chain |
| `nested_folders` | `POST /api/folders` twice | parent/child relationship |
| `delete_folder` | `DELETE /api/folders/{id}` | 204; absent from listing |

### Sync (`tests/sync.rs`)

| Test | Route | Expected |
|---|---|---|
| `default_effective_strategy` | `GET /api/folders/{id}` | `effective_strategy` = TwoWay |
| `explicit_strategy_roundtrip` | `PUT /api/folders/{id}` | stored strategy returned |
| `child_inherits_parent_strategy` | `GET /api/folders/{child_id}` | inherits parent's value |
| `effective_strategy_route` | `GET /api/folders/{id}/effective-strategy` | resolves correctly |
| `sync_tree_structure` | `GET /api/sync/tree` | all user folders present |

### Auth (extended, `tests/auth_e2e.rs`)

Covers registration modes (`open` / `disabled` / `approval` / `invite_only`), the `pending` user lifecycle, login by email, change-password, and admin-side user management (list/create/reset-password/change-role). Exercises the `auth.registration` config switch and the admin endpoints under `/api/admin/users/...`.

### Sync engine (`tests/sync_engine.rs`)

End-to-end coverage of the two-way sync algorithm in `uncloud-sync` against a real `TestApp` server.

| Test | Behaviour |
|---|---|
| `upload_local_file_appears_on_server` | local create → server upload |
| `modify_local_file_updates_server` | local edit → server new version |
| `server_file_downloads_to_local` | server upload → local mirror |
| `server_file_in_inherit_folder_downloads_to_local` | inherited TwoWay strategy still pulls |
| `server_file_in_nested_inherit_folders_downloads` | inheritance walks the chain |
| `server_delete_removes_local_file` | server delete → local removal |
| `conflict_creates_copy` | both sides edited → conflict copy retains both |
| `do_not_sync_folder_skips_download` | DoNotSync prevents pulls |
| `server_to_client_folder_blocks_local_upload` | one-way strategy enforced |
| `idempotent_sync` | second run is a no-op |
| `two_clients_share_files` | bidirectional propagation between two engines |
| `local_folder_rename_renames_server_folder` | local rename → server rename in place, ids kept, nothing re-uploaded |
| `large_file_round_trips_between_clients` | a multi-megabyte file is uploaded and downloaded as a stream and arrives intact |
| `sync_tree_reports_content_checksums` | the tree carries each file's SHA-256, used to fill in journal rows that predate checksums |
| `local_folder_rename_and_move_reach_second_client` | incoming rename/move relocates nested contents and empty folders without recreating the old remote name |
| `local_folder_delete_removes_server_folder_once_confirmed` | local folder delete → one cascading server delete after confirmation |

The engine's deletion, restore and rename rules are covered in more depth by `crates/uncloud-sync/tests/` (`local_deletion_tracking.rs`, `client_creates_local.rs`), which run against an in-process fake server and need no containers: `cargo test -p uncloud-sync`.

Regression coverage includes interrupted copy moves with edits at either location or both, copies cut off by an error or a kill being redone while an edited one waits for manual resolution, files an interrupted copy never carried being downloaded again instead of deleted on the server, a stuck move not being re-read on every run, a recovered move leaving files without a recorded checksum alone, same-mtime edits after a failed recovery, restart after a failed move-completion transaction, folders a copy cannot carry in full (symlinks, files the walk did not list), source edits that preserve mtimes, case-only renames on a backend that needs a temporary name, swapped folders with subfolders, rejection of unrelated destinations for empty folders, repeated custom-path remapping across restarts and the release of retired paths, leftover download files in a folder the server deleted, a conflict copy that cannot be written, and uploads that declare their length. Filesystem tests check staging-file isolation and sweeping, a copy replacing its destination in one step, permission bits surviving a download, and what a copy would leave behind; journal tests check atomic retirement of old mapping paths and atomic completion of paths, mtimes, and move intents.

### Storage migration (`tests/storage_migration.rs`)

Covers the offline `uncloud-server migrate` subcommand. Asserts idempotency, hash-verify failure handling, version-archive co-migration, source delete after the atomic pointer flip, repin of pinned folders, and orphan cleanup (incl. version blobs).

### S3 backend (`tests/storage_s3.rs`)

Runs `S3Storage` against a `MinIO` testcontainer. Covers write/read/delete round-trip, range reads, the temp-upload finalize sequence, rename+archive_version, key scanning, and fast-fail behaviour against a missing bucket.

### SFTP backend (`tests/storage_sftp.rs`)

Runs `SftpStorage` against a `linuxserver/openssh-server` testcontainer. Covers password auth and private-key auth, range reads, temp-upload finalize, rename+archive, key scanning, and the TOFU host-key pin: first connect records the key in `sftp_host_keys`; subsequent connects validate against the pinned key.

### Backup (`tests/backup.rs`, `#[ignore]`)

Document-level round-trip via the BSON → portable-JSON helper (`dump_roundtrips_real_documents_via_ejson`) and the per-collection `manifest.json` writer (`dump_all_writes_manifest_with_counts`). Marked `#[ignore]` because they spin up a Mongo container; run with:

```bash
cargo test -p uncloud-server --test backup -- --ignored
```

End-to-end CLI smoke (`init` / `create` / `list` / `check` / `restore --dry-run`) is verified manually on an empty database.
