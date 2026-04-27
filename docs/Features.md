# Implemented Features

Detailed feature inventory. For the route table see [Architecture.md](Architecture.md#api-route-summary); for what's still planned see [Roadmap.md](Roadmap.md).

## File Management

- **Upload**: drag-and-drop zone (shown only when folder is empty) + toolbar Upload button; hidden `<input>` always present for the button; supports simple upload and chunked upload for large files; `POST /api/uploads/init`, `POST /api/uploads/{id}/chunk`, `POST /api/uploads/{id}/complete`; also `POST /api/uploads/simple` for small files
- **Download**: `GET /api/files/{id}/download` with range request support
- **Update content**: `POST /api/files/{id}/content` replaces file content (creates a new version)
- **Rename**: file and folder, via context menu -> modal
- **Move**: file and folder (single and bulk), via context menu or selection toolbar -> folder-picker dialog; recursive storage-path sync on folder move
- **Copy**: file (single and bulk) and folder (single and bulk, recursive); `POST /api/files/{id}/copy`, `POST /api/folders/{id}/copy`
- **Delete**: soft delete to trash; file and folder (single and bulk); single uses a DaisyUI confirmation modal; bulk uses a separate confirmation modal; folder delete is recursive
- **Conflict resolution**: 409 responses during move/copy trigger an inline rename prompt inside the move/copy dialog; `suggest_name()` generates "foo (1).txt" style alternatives
- **Multi-select**: checkbox on each item; bulk Move, Copy, Delete from selection toolbar
- **Context menu**: right-click (desktop) or three-dot button (always visible on mobile, hover-revealed on desktop); fixed-position to escape `overflow: hidden` containers; viewport-clamped to stay on screen
- **View modes**: grid and list, persisted to localStorage
- **Breadcrumb navigation**: clickable chain built from folder ancestry; `GET /api/folders/{id}/breadcrumb`
- **File properties**: side panel (`file_properties.rs`) shows metadata, hash, processing status, version count

## Version History

Every re-upload of an existing file archives the previous blob and creates a version record.

- **DB**: `file_versions` collection stores `{ file_id, version, storage_path, size_bytes, checksum, created_at }`; `files` doc has a `version: i32` field that increments
- **API**: `GET /api/files/{id}/versions` (list), `GET /api/files/{file_id}/versions/{version_id}` (download), `POST /api/files/{file_id}/versions/{version_id}/restore` (promote to current)
- **Frontend**: `version_history.rs` component shows a timeline of versions with timestamp, size, and Restore button

## Trash

Deletes are soft. Files and folders go to trash and can be recovered. Auto-purge runs as a background task.

- **DB**: `deleted_at: Option<DateTime<Utc>>` and `trash_path: Option<String>` on `File` and `Folder` docs; all normal queries filter `deleted_at: null`
- **API**: `GET /api/trash` (list), `POST /api/trash/{id}/restore`, `DELETE /api/trash/{id}` (permanent), `DELETE /api/trash` (empty all)
- **Frontend**: `trash.rs` component accessible from the sidebar; shows deleted files with original path and deletion date; Restore / Permanently Delete actions
- **Auto-purge**: background `tokio::spawn` task in `main.rs` checks hourly and purges trashed files older than `versioning.trash_retention_days`; also cleans up associated versions and updates user byte quotas
- **Config**: `versioning.trash_retention_days` (default 30, 0 = never auto-purge), `versioning.max_versions` (default 50)

## Post-Upload Processing Pipeline

Extensible background processing triggered after each upload. All processors implement the `FileProcessor` trait and are registered with `ProcessingService` at startup.

- **`FileProcessor` trait**: `task_type() -> TaskType`, `applies_to(&File) -> bool`, `process(&File, Arc<AppState>) -> Result<(), String>`
- **`ProcessingService`**: `register()` adds processors, `enqueue()` spawns applicable tasks (bounded by semaphore), `recover()` retries pending/failed tasks on startup (also backfills files that predate the pipeline)
- **DB**: `processing_tasks: Vec<ProcessingTask>` embedded array on each `File` document; each task has `task_type`, `status` (Pending/Done/Error), `attempts`, `error`, `queued_at`, `completed_at`
- **SSE**: `ServerEvent::ProcessingCompleted { file_id, task_type, success }` emitted on task completion; frontend re-renders thumbnails, etc.
- **Admin "rerun all"**: `POST /api/admin/processing/rerun` re-enqueues every applicable file across all processors (useful after bug fixes)
- **Config**: `processing.max_concurrency` (default 4), `processing.thumbnail_size` (default 320px), `processing.max_attempts` (default 3)

### Processors

| Processor | Applies to | Output |
|---|---|---|
| `ThumbnailProcessor` | `image/*` | `.thumbs/{file_id}.jpg` via `image` crate |
| `AudioMetadataProcessor` | `audio/*` | Extracts ID3/Vorbis tags (artist, album, track, year) + embedded cover art as thumbnail |
| `TextExtractProcessor` | `text/*`, `application/pdf` | `content_text` field on file doc |
| `SearchIndexProcessor` | all files (when search enabled) | Meilisearch document upsert |

- **Thumbnail API**: `GET /api/files/{id}/thumb` -> `200` + JPEG if ready, `202` if pending, `404` if not applicable

## Full-Text Search (Meilisearch)

Full-text search powered by Meilisearch. Runs as a separate process alongside the server.

- **`SearchService`** in `services/search.rs`: wraps `meilisearch-sdk` client; `index_file()`, `delete_file()`, `search()` filtered by `owner_id`
- **API**: `GET /api/search?q=<query>` (returns matching files for current user), `GET /api/search/status` (index health), `POST /api/search/reindex` (admin: rebuild full index)
- **Frontend**: search bar in the Navbar; `search.rs` component displays results; `use_search.rs` hook
- **Text extraction**: `TextExtractProcessor` extracts content from text files and PDFs; `SearchIndexProcessor` upserts into Meilisearch
- **Config**: `search.enabled` (default false), `search.url` (default `http://localhost:7700`), `search.api_key` (optional)
- **Meilisearch index** is a derivative of MongoDB data and can be fully rebuilt via the admin reindex endpoint

## Gallery

Photo gallery with timeline view and album organization.

- **API**: `GET /api/gallery` (all images for user, sorted by date), `GET /api/gallery/albums` (folder tree of image-containing folders)
- **Frontend**: `gallery.rs` component with timeline view and album tree; clicking an album navigates to `GalleryAlbum { id }` route; images use lightbox overlay (`lightbox.rs`)
- **Routes**: `/gallery`, `/gallery/album/:id`

## Music + Playlists

Music library with artist/album aggregation, folder browsing, playlist management, and in-browser playback.

- **Audio metadata**: `AudioMetadataProcessor` extracts ID3/Vorbis tags on upload and stores artist, album, track name, year, genre, duration, track number on the file document; also extracts embedded cover art as thumbnail
- **Server API**:
  - `GET /api/music/tracks` — all audio files for user
  - `GET /api/music/folders` — folders containing audio files
  - `GET /api/music/artists` — aggregated artist list
  - `GET /api/music/artists/{name}/albums` — albums for an artist
  - `GET /api/music/albums/{artist}/{album}/tracks` — tracks in an album
  - `GET /api/playlists` / `POST /api/playlists` — list/create playlists
  - `GET /api/playlists/{id}` / `PUT /api/playlists/{id}` / `DELETE /api/playlists/{id}` — CRUD
  - `POST /api/playlists/{id}/tracks` — add tracks
  - `DELETE /api/playlists/{id}/tracks` — remove tracks
  - `PUT /api/playlists/{id}/tracks/reorder` — reorder tracks
- **DB**: `Playlist` model with `tracks: Vec<PlaylistTrack>` (each has `file_id` and `position`)
- **Frontend**:
  - Music page with tabs: Artists, Albums, Folders, Playlists
  - Sub-pages: `/music/artist/:name`, `/music/album/:artist/:album`, `/music/folder/:id`, `/music/playlist/:id`
  - Components in `components/music/`: `artist_list`, `artist_view`, `album_grid`, `album_view`, `folder_view`, `playlist_list`, `playlist_view`, `playlist_panel` (persistent right-side), `track_list`
  - `player.rs`: persistent audio player bar with play/pause, skip, queue display, progress
  - `PlayerState` context with queue, current index, playing state
  - **Lock-screen / OS controls**: `hooks/media_session.rs` integrates with the browser MediaSession API; `hooks/native_audio.rs` is the Tauri/Android bridge for background playback
  - Hooks: `use_music.rs`, `use_player.rs`, `use_playlists.rs`

## File Viewer

In-browser file viewing for multiple content types.

- **`file_viewer.rs`**: image lightbox (via `lightbox.rs`), plain text viewer, PDF viewer, audio playback, password-vault opener (KeePass `.kdbx`)
- Accessed from file item click or context menu

## Public Share Links

Share files and folders via public links with optional access controls.

- **DB**: `Share` model with `token`, `resource_type` (File/Folder), `resource_id`, `owner_id`, `password_hash`, `expires_at`, `download_count`, `max_downloads`
- **Public routes**: `GET /api/public/{token}` (get share info), `GET /api/public/{token}/download` (download), `POST /api/public/{token}/verify` (password check)
- **Authenticated routes**: `GET /api/shares`, `POST /api/shares`, `DELETE /api/shares/{id}`
- **Frontend**: `share_dialog.rs` modal (password, expiry, download limit); `/share/:token` public route; `use_shares.rs` hook; `shares_page.rs` aggregates outgoing public links + folder shares.

## Folder Shares (with Uncloud users)

Share a folder directly with another Uncloud user (in addition to anonymous public links).

- **DB**: `folder_shares` collection; `FolderShare` model with `folder_id`, `owner_id`, `shared_with_user_id`, `permission` (`SharePermissionModel::Read | Write`), `created_at`
- **Authorisation**: `services/sharing.rs` walks up the folder ancestry to resolve the effective permission for a path — a parent share grants access to all descendants
- **API**:
  - `POST /api/folder-shares` — create
  - `GET /api/folder-shares/by-me` — folders I've shared with others
  - `GET /api/folder-shares/with-me` — folders shared with me ("inbox")
  - `GET /api/folder-shares/folder/{id}` — shares for a specific folder I own
  - `PUT /api/folder-shares/{id}` — change permission
  - `DELETE /api/folder-shares/{id}` — revoke
- **SSE**: `FolderShared` / `FolderShareRevoked` events fire to both sides so UIs update live
- **Frontend**: `folder_share_dialog.rs` (per-folder share dialog), `shared_with_me.rs` (inbox), `shares_page.rs` (overview)

## Two-Factor Auth (TOTP)

Time-based one-time-password 2FA, opt-in per user.

- **DB**: `User.totp_enabled: bool` + `User.totp_secret: Option<String>` (encrypted at rest); `totp_challenges` collection holds short-lived two-step-login tokens
- **Setup flow**: `POST /api/auth/totp/setup` returns `{ otpauth_uri, secret }` for the QR code; `POST /api/auth/totp/enable` with a verifying code activates it; `POST /api/auth/totp/disable` removes it (requires a code)
- **Login flow**: `POST /api/auth/login` returns `LoginResponse { totp_required: true, totp_token: <challenge> }` if TOTP is enabled; client then `POST /api/auth/totp/verify` with `{ totp_token, code }` and gets a session
- **Admin override**: `POST /api/admin/users/{id}/reset-totp` clears `totp_secret` (lockout recovery)
- **Frontend**: TOTP setup/disable in `settings.rs` Profile tab; two-step login form in `auth/login.rs`

## User Invites & Registration Modes

Configurable registration policy + invite-only sign-up.

- **Modes** (`auth.registration` in config.yaml):
  - `open` — anyone can register
  - `approval` — anyone can register; account is `pending` until an admin approves it
  - `invite_only` — only users with an invite token can register
  - `disabled` — no public registration (admin can still create users)
  - `demo` — adds a "Try Demo" button that mints ephemeral accounts (auto-purged after `auth.demo_ttl_hours`)
- **DB**: `invites` collection with `{ token, comment, role, expires_at, used, used_by }`
- **API**:
  - `GET /api/auth/server-info` — public; returns registration mode (used by login UI)
  - `GET /api/auth/invite/{token}` — public; validates an invite token before sign-up
  - `POST /api/auth/demo` — public; creates an ephemeral demo account (when mode == `demo`)
  - `GET/POST /api/admin/invites`, `DELETE /api/admin/invites/{id}` — admin invite CRUD
  - `POST /api/admin/users/{id}/approve` — admin approval (when mode == `approval`)
- **Frontend**: `/invite/:token` route validates and prefills register form; admin invite UI in `settings.rs` admin tab; user lifecycle controls (approve / disable / enable / change role / reset password / reset TOTP) in admin Users tab

## Versioned API + Scoped Bearer Tokens

Stable surface for sidecar apps and scripts.

- Every authenticated route is mounted under both `/api/...` and `/api/v1/...`. Use `/api/v1/` for any external consumer; the unversioned form is the legacy/internal surface.
- **Scoped Bearer tokens**: users mint personal API tokens for scripts, CLI tools, or sidecar apps.
  - **DB**: `api_tokens` collection; `ApiToken` model stores SHA-256 hash only (raw token shown once on creation)
  - **API**: `POST /api/v1/auth/tokens` (create — returns raw token), `GET /api/v1/auth/tokens` (list metadata), `DELETE /api/v1/auth/tokens/{id}` (revoke)
  - **Auth**: `auth_middleware` accepts `Authorization: Bearer <token>` in addition to session cookies

## App Platform (sidecar HTTP services)

Sidecar processes register themselves with Uncloud and get auth, file storage, real-time events, and a sidebar slot for free.

- **Architecture**: each app is an independent HTTP server bound to localhost. Uncloud reverse-proxies `/apps/{name}/*` to its `base_url` so the browser sees a single origin (cookies and Bearer tokens flow without CORS).
- **Registration**: `POST /api/v1/apps/register` (public, secret-protected via `apps.registration_secret` in config) is called by an app on startup; payload `{ name, nav_label, icon, base_url, secret }`. Server upserts an `App` doc and returns `{ id, name, db, db_uri }` so apps can also share a Mongo database namespace.
- **Webhook delivery**: `POST /api/v1/apps/webhooks` registers a URL + event subscription + signing secret. `apps::deliver_webhooks` fans out events with HMAC signatures and exponential-backoff retries.
- **Sidebar nav**: `GET /api/v1/apps` returns apps enabled for the current user; `sidebar.rs` consumes this via `use_apps::list_apps()` and renders dynamic entries below the static sections.
- **Managed apps**: `apps.managed[]` in config lists local sidecar processes the server should supervise. `main.rs` spawns each one with the configured env (port, registration secret, Uncloud URL) and a restart policy (`always` / `on-failure` / `never`) with backoff. Useful for shipping the server + apps as a single unit (e.g. systemd or Docker Compose).
- **Models**: `App { name, nav_label, icon, base_url, enabled_for: Vec<ObjectId> }`, `Webhook { app_id, url, events, secret_hash }`
- **Routes**:
  - Public v1: `POST /api/v1/apps/register`, `POST /api/v1/apps/webhooks`
  - Auth v1: `GET /api/v1/apps`
  - Admin v1: `DELETE /api/v1/admin/apps/{name}`, `DELETE /api/v1/admin/apps/webhooks/{id}`
  - Auth (outside `/api`): `ANY /apps/{name}/*` (reverse proxy)

## S3-Compatible API

Standard S3 tools (`s5cmd`, `rclone`, `aws-cli`, Cyberduck) work against Uncloud without any custom client.

- **Mounted at**: `/s3` (path-style URLs only). One bucket per user, named after the user's username.
- **Auth**: `middleware/sigv4.rs` implements AWS Signature V4 verification — parses `Authorization: AWS4-HMAC-SHA256`, reconstructs the canonical request and string-to-sign, derives the signing key via four HMAC-SHA256 rounds, and constant-time-compares signatures. Looks up the secret in the `s3_credentials` collection by `access_key_id`.
- **Credentials API** (cookie/Bearer auth, v1-only):
  - `POST /api/v1/s3/credentials` — generate keypair (returns secret once)
  - `GET /api/v1/s3/credentials` — list access keys for current user
  - `DELETE /api/v1/s3/credentials/{id}` — revoke
- **Operations supported** (`routes/s3.rs`):

| Operation | HTTP |
|---|---|
| `ListBuckets` | `GET /` |
| `ListObjectsV2` | `GET /{bucket}?list-type=2[&prefix=][&delimiter=]` |
| `HeadObject` | `HEAD /{bucket}/{key}` |
| `GetObject` | `GET /{bucket}/{key}` (+ range requests) |
| `PutObject` | `PUT /{bucket}/{key}` |
| `DeleteObject` | `DELETE /{bucket}/{key}` |
| `DeleteObjects` | `POST /{bucket}?delete` (batch, XML body) |
| `CreateMultipartUpload` | `POST /{bucket}/{key}?uploads` |
| `UploadPart` | `PUT /{bucket}/{key}?partNumber=N&uploadId=X` |
| `CompleteMultipartUpload` | `POST /{bucket}/{key}?uploadId=X` |
| `AbortMultipartUpload` | `DELETE /{bucket}/{key}?uploadId=X` |

- **XML responses** via `quick-xml` + `serde`; mirrors the official S3 wire format.
- **Settings UI**: "S3 Access Keys" panel in `settings.rs` (lines ~348–487) — list keys, generate new (one-time secret display + clipboard copy), revoke. Sample `s5cmd` invocation:
  ```bash
  s5cmd --endpoint-url http://localhost:8080/s3 \
        --credentials-file ~/.aws/uncloud \
        ls s3://alice/
  ```
- **Storage layer is reused as-is**: the S3 API is purely a new HTTP surface over the same `StorageBackend`/file model.

## Tasks / Projects

Todoist-style task manager with projects, sections, subtasks, comments, attachments, and shared projects.

- **Models** (`models/task.rs`):
  - `TaskProject { name, color, view: ProjectView (List/Board/Schedule), members: Vec<ProjectMember> }`
  - `ProjectMember { user_id, permission: ProjectPermission (Owner/Editor/Viewer) }`
  - `TaskSection { project_id, name, position }` — Kanban columns
  - `Task { project_id, section_id, parent_task_id, title, description, status (Open/InProgress/Done/Archived), priority (Low/Medium/High/Urgent), due_at, recurrence, assignee_id, labels: Vec<String>, attachments: Vec<ObjectId>, position }`
  - `TaskComment { task_id, author_id, body, created_at }`
  - `TaskLabel { project_id, name, color }`
- **Server API** (full): projects CRUD + members, sections CRUD + reorder, labels CRUD, tasks CRUD + reorder, subtasks (`POST /tasks/{id}/subtasks`), promote subtask to top-level, status update, comments CRUD, file attachments (link/unlink existing files), schedule (`GET /tasks/schedule` — date-grouped upcoming tasks), assigned-to-me feed.
- **Frontend** (`components/tasks/`):
  - `mod.rs` — page shell + project list
  - `board_view.rs` — Kanban: sections as columns, drag-drop tasks between columns
  - `list_view.rs` — flat list with grouping
  - `schedule_view.rs` — calendar / agenda view
  - `task_detail.rs` — drawer with description, comments, subtasks, attachments, labels chip-input + create-inline picker
  - `project_settings.rs` — members, sections, labels CRUD (8-colour palette, rename/recolour/delete with cascade)
- **Routes**: `/tasks`, `/tasks/project/:id`
- **Labels**: project-scoped (`TaskLabel { project_id, name, color }`); tasks store `labels: Vec<String>` of label *names*, with server-side cascade on rename/delete. UI renders coloured name chips on both board cards and list rows (max 2 + overflow on list rows); label colours are looked up from the project's catalogue with a stable grey fallback. Filter strip (`LabelFilterBar` in `tasks/mod.rs`) at the top of board/list views narrows visible tasks (OR semantics).
- **Live updates**: every mutation (task, section, label CRUD + reorders + status changes) emits `ServerEvent::TaskChanged` to the project's owner and members. `BoardView` / `ListView` refetch tasks; `TasksProjectPage` refetches the label catalogue; `ScheduleView` refetches the schedule on any TaskChanged. The open `TaskDetail` re-fetches via `refresh_key` when the changed `task_id` matches.

## Passwords (KeePass Vaults)

Open KeePass-format password databases (`.kdbx`) directly in the browser.

- The vault file lives in normal Uncloud storage like any other file; opening it decrypts in the browser only — the server never sees the master key.
- **Recent vaults**: per-user list of last-opened vaults, persisted server-side so it follows you across devices. Backed by `vault_recents` (a key on `user_preferences` documents).
  - `GET /api/vault-recents`, `POST /api/vault-recents` (capped at 10, dedupes by file_id, MRU-ordered), `DELETE /api/vault-recents/{file_id}`
- **Frontend**: `components/passwords.rs` (route `/passwords`) lists recents and opens a vault into a viewer rendered via `file_viewer.rs`.

## Dashboard

Configurable home page with selectable tiles.

- **DB**: `UserPreferences.dashboard_tiles: Vec<String>` (ordered list of tile IDs)
- **API**: `PUT /api/v1/auth/me/preferences` `{ dashboard_tiles: [...] }`
- **Frontend**: `components/dashboard.rs` (route `/dashboard`) renders tiles like recent files, gallery, tasks summary, etc. `all_tile_ids()` enumerates available tiles; users pick which to show and in what order.

## Sync Audit Log

Per-user record of change-inducing operations (uploads, renames, moves, deletes, restores) for cross-device coherence and forensics.

- **DB**: `sync_events` collection with TTL index (`config.sync_audit.retention_days`) + per-user cap (`max_records_per_user`, pruned by background task)
- **Recording**: change-inducing handlers call `routes/audit.rs` helpers (`file_event`, `folder_event`, `summary_event`) which write to `services/sync_log::SyncLogService`
- **Source classification**: `SyncEventSource` distinguishes `UserWeb` / `UserDesktop` / `UserMobile` / `Sync` / `Admin` / `Public` / `System`; `SyncClientOs` carried alongside (`Linux`/`Windows`/`Macos`/`Android`/`Ios`/`Unknown`)
- **API**: `GET /api/sync-events?limit=&before=` returns `SyncEventListResponse { events, has_more }`
- **SSE**: `ServerEvent::SyncEventAppended { event }` broadcasts new rows to other tabs/devices live
- **Frontend**: `use_sync_events.rs` hook; consumed by Settings → Activity (and used by `uncloud-sync` to short-circuit redundant work)
- **Config**: `sync_audit.enabled` (default true), `sync_audit.retention_days` (default 7), `sync_audit.max_records_per_user` (default 10000)

## Admin

Admin panel for storage and user management. Guarded by `admin_middleware`.

- **Storage management**: `GET/POST /api/admin/storages`, `PUT/DELETE /api/admin/storages/{id}`
- **Storage rescan**: `POST /api/admin/storages/{id}/rescan` kicks off a background job that imports any orphan filesystem content into MongoDB; progress streams via `ServerEvent::RescanProgress` / `RescanFinished`. `GET /api/admin/rescan-jobs/active`, `GET /api/admin/rescan-jobs/{id}`, `POST /api/admin/rescan-jobs/{id}/cancel`. Service in `services/rescan.rs`.
- **User management**: `GET/POST /api/admin/users`, `PUT/DELETE /api/admin/users/{id}` plus lifecycle ops: `POST .../approve`, `POST .../disable`, `POST .../enable`, `POST .../reset-totp`, `POST .../reset-password`, `POST .../role`
- **Invite management**: `GET/POST /api/admin/invites`, `DELETE /api/admin/invites/{id}`
- **Processing rerun**: `POST /api/admin/processing/rerun` re-enqueues every applicable file across all processors
- **Frontend**: admin sections in `settings.rs` (only visible when `AuthState.is_admin()`)
- **User model**: `UserRole` enum (Admin/User), `quota_bytes: Option<i64>`, `used_bytes: i64`, `status` (Active/Pending/Disabled), `disabled_features: Vec<String>`

## File Sync (Folder-Level Strategy)

Per-folder sync strategy configuration for the desktop sync engine.

- **`SyncStrategy` enum** in `uncloud-common`: Inherit, TwoWay, ClientToServer, ServerToClient, UploadOnly, DoNotSync
- **API**: `GET /api/folders/{id}/effective-strategy` (resolves inheritance up the parent chain), `GET /api/sync/tree` (full folder tree with strategies)
- **Frontend**: right-click folder -> "Sync settings..." opens a strategy dropdown modal
- **Activity broadcast**: `SyncEngine::activity()` returns a `tokio::sync::watch::Receiver<SyncActivity>` (Idle/Polling/Transferring) consumed by the desktop tray icon to show transfer indication without blinking on quiet polls.

## Desktop App (Tauri v2)

Tray-only desktop application that bundles the Dioxus web frontend and provides local file sync.

- **Architecture**: Tauri v2 with `frontendDist: src-frontend` serving the bundled Dioxus build
- **Tray menu**: Open Uncloud / Sync Now / Quit
- **First-run**: `/setup` route for server URL configuration; seeds `api_base` via Tauri invoke
- **Sync**: uses `uncloud-client` for HTTP and `uncloud-sync` for two-way sync with SQLite journal. Fires one immediate sync after engine init at every entry point (ensure_engine / login / auto-login) so users see server state without waiting for the 60s poll tick.
- **Tray indicator**: subscribes to `SyncEngine::activity()`; swaps `tray-idle.png` ↔ `tray-syncing.png` only on `Transferring` (download/upload), not on quiet `Polling` ticks.
- **Filesystem watcher**: `desktop/src/file_watcher.rs` uses the `notify` crate (recursive, 2s debounce) to react to local edits between poll ticks. Self-induced events from sync writes are tolerated — the engine has its own no-op detection.
- **Autostart**: `tauri-plugin-autostart` registers the app at OS login; default-on the first time (gated by an `autostart_decided` sentinel in the app config dir). Toggle in Settings → Sync.
- **CORS**: server allows `tauri://localhost` and `https://tauri.localhost` with credentials
- **Build**: `./build-desktop.sh` at workspace root (dx build -> copy to src-frontend -> cargo build)
- **Linux deps**: `webkit2gtk4.1-devel`, `libsoup3-devel`, `libappindicator-gtk3-devel`

## Real-Time Events (SSE)

`EventService` broadcasts `ServerEvent` variants to per-user channels. Frontend subscribes at `/api/events`.

Variants:
- `FileCreated` / `FileUpdated` / `FileDeleted` / `FileRestored`
- `FolderCreated` / `FolderUpdated` / `FolderDeleted`
- `FolderShared` / `FolderShareRevoked`
- `UploadProgress { upload_id, progress }`
- `ProcessingCompleted { file_id, task_type, success }`
- `RescanProgress { ... }` / `RescanFinished { ... }`
- `SyncEventAppended { event }`
- `TaskChanged { project_id, task_id }` — fans out to project owner + members on any task / section / label CRUD; `task_id` is `Some` for single-task changes, `None` for bulk (reorder, label/section CRUD)

`FileBrowser` and other views refresh on relevant events; thumbnail updates trigger image re-render via a per-file version counter (`thumb_vers: HashMap<String, u32>`). `BoardView` / `ListView` / `ScheduleView` / `TasksProjectPage` subscribe to `TaskChanged` and refetch their data (tasks, sections, label catalogue, schedule) live.

## Shopping Lists

A lightweight shopping-list companion app that shares the Uncloud session. Users maintain a per-user catalogue of items (with categories and shops) and group them into shareable lists; items on a list can be checked off as purchased, and a list can be shared with other Uncloud users.

- **Model** (MongoDB collections):
  - `shopping_categories` — `ShoppingCategory { owner_id, name, position }` (ordered labels used to tag items and shops)
  - `shopping_shops` — `Shop { owner_id, name, categories: Vec<String> }` (user's stores; each shop claims a set of categories so items auto-associate)
  - `shopping_items` — `ShoppingItem { owner_id, name, categories, shop_ids, notes }` (the catalogue; re-used across lists)
  - `shopping_lists` — `ShoppingList { owner_id, name, shared_with: Vec<ObjectId> }`
  - `shopping_list_items` — `ShoppingListItem { list_id, item_id, checked, recurring, quantity, position }` (join row; `recurring` items stay on the list when "remove purchased" is invoked)
- **Access control**: `list_access_filter` lets either the owner or any user in `shared_with` read/mutate a list's rows; catalogue (categories/shops/items) is always owner-scoped.
- **Feature flag**: `require_shopping` gate at the top of every handler checks `config.features.shopping` (server-wide) AND `user.disabled_features` (per-user opt-out via `PUT /api/auth/me/features` `{ "shopping": false }`). Disabled users get 404.
- **Frontend**: `components/shopping.rs` provides `ShoppingPage` (list index + shops/categories/items management) and `ShoppingListView` (per-list view with check/uncheck, inline item inlet, drag-to-reorder, "remove purchased" bulk action); routes `/shopping` and `/shopping/list/:id`.
- **Marking as purchased**: `PATCH /api/shopping/lists/{id}/items/{item_id}` with `{ "checked": true }`; bulk cleanup via `POST /api/shopping/lists/{id}/remove-purchased` which deletes all checked non-recurring rows.
