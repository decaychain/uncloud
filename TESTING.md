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
    upload_streaming.rs   ← Streamed single-request uploads: integrity, temp cleanup on failure, client disconnect
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

### Streaming uploads (`tests/upload_streaming.rs`)

`POST /api/uploads/simple` and `POST /api/files/{id}/content` stream the body into a storage temp file. Every test except the ignored memory one ends by asserting the backend's `.tmp/` directory is empty.

| Test | Behaviour |
|---|---|
| `simple_upload_stores_multi_megabyte_body_intact` | 9 MiB upload: size, SHA-256, blob on disk, download, `used_bytes`, audit event |
| `simple_upload_resolves_folder_named_after_the_file_part` | `parent_id` sent after `file` still lands under the folder |
| `simple_upload_accepts_empty_file` | zero-byte upload still creates a file |
| `content_update_stores_multi_megabyte_body_and_archives_previous` | 9 MiB replacement: new size/checksum, previous content downloadable as version 1 |
| `quota_exceeded_upload_leaves_no_temp_and_no_file` | 403; no temp, no blob, no record, `used_bytes` unchanged |
| `duplicate_name_upload_leaves_no_temp_and_keeps_existing_content` | 409; existing blob untouched |
| `upload_to_missing_folder_leaves_no_temp` | 404 |
| `upload_without_file_part_is_rejected` | 400 |
| `empty_content_update_is_rejected_and_keeps_existing_content` | 400; no version created |
| `upload_into_folder_pinned_to_another_storage_lands_there` | two local storages; body staged on the default, committed to the pinned one (and directly when `parent_id` comes first) |
| `chunked_simple_upload_without_content_length_arrives_intact` | real HTTP, chunked transfer encoding, 11 MiB in ~48 KiB frames |
| `chunked_content_update_without_content_length_arrives_intact` | same for content replacement |
| `disconnect_during_simple_upload_leaves_no_temp_and_no_file` | client abandons the request after 6 MiB; temp removed |
| `disconnect_during_content_update_keeps_existing_content` | same for content replacement; current content and version list unchanged |
| `large_upload_does_not_grow_resident_memory` (`#[ignore]`, Linux) | 256 MiB upload must grow peak RSS by less than 48 MiB |

The ignored memory test writes 256 MiB to the temp dir; run it with:

```bash
cargo test -p uncloud-server --test upload_streaming -- --ignored large_upload --nocapture
```

`storage::staging` also has unit tests (`cargo test -p uncloud-server --lib staging`) for the hash/size bookkeeping across append-buffer flushes, replace-on-commit, cross-storage commit, and temp removal on drop.

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

### Storage migration (`tests/storage_migration.rs`)

Covers the offline `uncloud-server migrate` subcommand. Asserts idempotency, hash-verify failure handling, version-archive co-migration, source delete after the atomic pointer flip, repin of pinned folders, and orphan cleanup (incl. version blobs).

### S3 backend (`tests/storage_s3.rs`)

Runs `S3Storage` against a `MinIO` testcontainer. Covers write/read/delete round-trip, range reads, the temp-upload finalize sequence, reading a temp back before finalize, rename+archive_version, key scanning, and fast-fail behaviour against a missing bucket.

### SFTP backend (`tests/storage_sftp.rs`)

Runs `SftpStorage` against an `atmoz/sftp` testcontainer. Covers password auth and private-key auth, range reads, temp-upload finalize (including finalizing over an existing file), reading a temp back before finalize, rename+archive, key scanning, and the TOFU host-key pin: first connect records the key in `sftp_host_keys`; subsequent connects validate against the pinned key.

### Backup (`tests/backup.rs`, `#[ignore]`)

Document-level round-trip via the BSON → portable-JSON helper (`dump_roundtrips_real_documents_via_ejson`) and the per-collection `manifest.json` writer (`dump_all_writes_manifest_with_counts`). Marked `#[ignore]` because they spin up a Mongo container; run with:

```bash
cargo test -p uncloud-server --test backup -- --ignored
```

End-to-end CLI smoke (`init` / `create` / `list` / `check` / `restore --dry-run`) is verified manually on an empty database.
