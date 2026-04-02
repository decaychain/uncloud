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
    lib.rs          ← library crate; exports AppState + all modules
    main.rs         ← binary entry point only
  tests/
    common/
      mod.rs        ← TestApp helper (container, TempDir, TestServer)
    auth.rs         ← Authentication tests
    files.rs        ← File upload/download/CRUD tests
    folders.rs      ← Folder management tests
    sync.rs         ← Sync strategy tests
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
