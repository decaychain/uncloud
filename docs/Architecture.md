# Architecture

Repository layout, key conventions, storage design, API route summary, and config reference.

## Repository Layout

```
Uncloud/
  config.yaml                  ← runtime config (storage paths, auth, DB URI, apps, sync_audit)
  config.example.yaml
  build-desktop.sh             ← dx build → copy to src-frontend → cargo build desktop
  Cargo.toml                   ← workspace manifest
  crates/
    uncloud-common/
      src/
        lib.rs                 ← re-exports api::*, client::ApiClient
        client.rs              ← ApiClient (shared HTTP client type)
        validation.rs          ← input validation helpers
        api/                   ← shared request/response types
          auth.rs              ← LoginRequest, RegisterRequest, UserResponse, UserRole,
                                 ServerInfoResponse, RegistrationMode, TOTP types, InviteResponse
          files.rs             ← FileResponse, UploadInit/Complete, GalleryResponse
          folders.rs           ← FolderResponse, SyncStrategy
          folder_shares.rs     ← FolderShareResponse, SharePermission, Create/Update requests
          music.rs             ← TrackResponse, MusicArtistResponse, MusicAlbumResponse
          playlists.rs         ← PlaylistResponse, CreatePlaylist, PlaylistTrack
          shares.rs            ← ShareResponse, CreateShareRequest (public links)
          events.rs            ← ServerEvent enum + event payload structs
          search.rs            ← SearchResponse, SearchStatus
          versions.rs          ← VersionResponse
          shopping.rs          ← ShoppingList/Item/Category/Shop request/response types
          tasks.rs             ← TaskProjectResponse, TaskResponse, TaskLabelResponse,
                                 ProjectMember, ProjectView, RecurrenceRule, etc.
          sync_events.rs       ← SyncEventResponse, SyncOperation, SyncEventSource, SyncClientOs
          preferences.rs       ← UserPreferences, UpdatePreferencesRequest (dashboard_tiles)
          vault_recents.rs     ← RecentVaultEntry, AddRecentVaultRequest
    uncloud-server/
      src/
        main.rs                ← AppState, startup, trash auto-purge, sync-audit prune,
                                 demo cleanup, managed-app supervisor
        config.rs              ← Config struct, RegistrationMode, AppsConfig (managed apps)
        db.rs                  ← MongoDB connection + index setup (incl. sync_events TTL,
                                 s3_credentials, api_tokens, apps, etc.)
        error.rs               ← AppError → HTTP status mapping
        routes/                ← Axum handlers
          auth.rs              ← register/login/logout/me/sessions, change-password,
                                 demo-login, server-info, TOTP setup/enable/disable/verify,
                                 invite validation, update_my_features, update_my_preferences
          files.rs             ← CRUD, upload (simple + chunked), download, thumb, gallery
          folders.rs           ← CRUD, copy, breadcrumb, sync strategy, sync tree
          folder_shares.rs     ← share folder with another user (with permission level)
          music.rs             ← tracks, folders, artists, albums
          playlists.rs         ← CRUD, add/remove/reorder tracks
          shares.rs            ← create/list/delete public share links + public download
          trash.rs             ← list, restore, permanently delete, empty
          versions.rs          ← list, download, restore versions
          search.rs            ← search files, search status, admin reindex
          storages.rs          ← admin: CRUD storage backends + rescan jobs
          users.rs             ← admin: CRUD users + approve/disable/enable/reset-totp/
                                 reset-password/change-role; non-admin: list_usernames
          events.rs            ← SSE stream
          shopping.rs          ← lists/items/categories/shops + list sharing, mark-purchased
          tasks.rs             ← projects, sections, labels (CRUD + member management)
          task_items.rs        ← tasks, subtasks, comments, attachments, schedule, reorder
          tokens.rs            ← scoped Bearer API tokens (create/list/revoke)
          s3.rs                ← S3-compatible API handler (mounted at /s3)
          s3_credentials.rs    ← /api/v1/s3/credentials CRUD
          apps.rs              ← app registration, webhook registration, list_apps,
                                 reverse proxy_handler, deliver_webhooks
          invites.rs           ← admin invite CRUD; public invite validation in auth.rs
          audit.rs             ← internal helpers for emitting sync_events from handlers
          sync_events.rs       ← list sync events for the current user
          vault_recents.rs     ← per-user "recent password vaults" list
          admin_processing.rs  ← admin: rerun all processing tasks
        services/
          auth.rs              ← AuthService (sessions, password hashing, TOTP, invites,
                                 demo accounts, user bytes)
          storage.rs           ← StorageService (config-driven backend registry, parent-chain resolution)
          events.rs            ← EventService (per-user SSE broadcast channels)
          search.rs            ← SearchService (Meilisearch SDK wrapper)
          sharing.rs           ← FolderShare resolution (effective permission for a path)
          sync_log.rs          ← SyncLogService (append events, prune, broadcast)
          rescan.rs            ← RescanService (background storage rescan jobs)
        storage/
          mod.rs               ← StorageBackend trait
          local.rs             ← LocalStorage impl (filesystem)
          s3.rs                ← S3Storage impl (AWS S3 / R2 / B2 / MinIO)
          sftp.rs              ← SftpStorage impl (russh; password or key auth; TOFU host-key pin in MongoDB)
        models/
          user.rs              ← User, UserRole, totp_enabled, disabled_features, status
          session.rs           ← Session
          file.rs              ← File, FileVersion, ProcessingTask, TaskType, ProcessingStatus
          folder.rs            ← Folder, SyncStrategy
          folder_share.rs      ← FolderShare, SharePermissionModel
          storage.rs           ← Storage (backend config doc)
          share.rs             ← Share (public link), ShareResourceType
          playlist.rs          ← Playlist, PlaylistTrack
          shopping.rs          ← ShoppingCategory, Shop, ShoppingItem, ShoppingList,
                                 ShoppingListItem
          task.rs              ← TaskProject, TaskSection, Task, TaskComment, TaskLabel,
                                 ProjectMember, TaskStatus, TaskPriority, RecurrenceRule
          api_token.rs         ← ApiToken (SHA-256 hashed Bearer tokens)
          s3_credential.rs     ← S3Credential (access_key_id + plaintext secret for SigV4)
          app.rs               ← App (registered sidecar app: name, base_url, nav_label, icon)
          webhook.rs           ← Webhook (URL + events + secret per app)
          invite.rs            ← Invite (token, role, expires_at)
          totp_challenge.rs    ← short-lived two-step-login challenge
          user_preferences.rs  ← UserPreferences (dashboard_tiles), VaultRecentsDoc
          sync_event.rs        ← SyncEvent (audit log row, TTL-purged)
        middleware/
          auth.rs              ← AuthUser extractor; cookie + Bearer-token auth_middleware;
                                 admin_middleware
          sigv4.rs             ← AWS SigV4 verifier + S3User extractor + sigv4_middleware
          request_meta.rs      ← parses User-Agent / IP into client metadata for audit log
        processing/
          mod.rs               ← FileProcessor trait, re-exports all processors
          service.rs           ← ProcessingService (enqueue, recover, run_task)
          thumbnail.rs         ← ThumbnailProcessor (image/*)
          audio_metadata.rs    ← AudioMetadataProcessor (audio/*, ID3/Vorbis tags + cover art)
          text_extract.rs      ← TextExtractProcessor (text/*, PDF, DOCX)
          search_index.rs      ← SearchIndexProcessor (Meilisearch)
    uncloud-web/
      src/
        app.rs                 ← App root, provides AuthState + ThemeState + PlayerState contexts
        router.rs              ← Dioxus Router (Home, Dashboard, Folder, Shares, Gallery,
                                 Music, Trash, Passwords, Tasks, Shopping, Settings)
        state.rs               ← AuthState, ThemeState, Section, FileBrowserState, ViewMode,
                                 PlayerState
        components/
          layout.rs            ← Drawer shell + Navbar (search bar, theme toggle)
          sidebar.rs           ← Section-aware sidebar nav + StorageUsage; data-driven
                                 app-platform entries via /api/v1/apps
          activity.rs          ← Sync-activity indicator
          file_browser.rs      ← File listing (grid/list), selection, bulk actions, modals
          file_item.rs         ← Individual file card/row (thumbnail, context menu trigger)
          file_properties.rs   ← File details panel
          file_viewer.rs       ← File viewer (image lightbox, text, PDF, audio, password vault)
          upload.rs            ← Upload zone (hidden input + drag-and-drop)
          context_menu.rs      ← Right-click / long-press dropdown
          share_dialog.rs      ← Public share-link modal (password, expiry, download limit)
          folder_share_dialog.rs ← Share folder with another user (permission level)
          shared_with_me.rs    ← Inbox of folders shared with the current user
          shares_page.rs       ← My public links + folder shares overview
          lightbox.rs          ← Image lightbox overlay
          search.rs            ← Search results overlay / page
          gallery.rs           ← Gallery page (timeline + album tree + folder inclusion)
          dashboard.rs         ← Configurable tile dashboard (default homepage)
          icons.rs             ← Icon component library
          right_drawer.rs      ← Reusable safe-area-aware right drawer
          music/
            mod.rs             ← Music page shell + tab navigation
            artist_list.rs     ← Artist grid/list
            artist_view.rs     ← Artist detail (albums for an artist)
            album_grid.rs      ← Album grid display
            album_view.rs      ← Album detail (track listing)
            folder_view.rs     ← Browse music by folder
            playlist_list.rs   ← Playlist index
            playlist_view.rs   ← Playlist detail (track list + reorder)
            playlist_panel.rs  ← Persistent right-side playlist panel
            track_list.rs      ← Reusable track list component
          player.rs            ← Audio player bar (play/pause, skip, queue, progress)
          tasks/
            mod.rs             ← Tasks page shell
            board_view.rs      ← Kanban board (sections as columns)
            board_card.rs      ← Single task card
            list_view.rs       ← Flat list view
            schedule_view.rs   ← Date-grouped schedule
            task_detail.rs     ← Task drawer (description, comments, attachments, subtasks)
            project_settings.rs ← Per-project settings (members, sections, view)
          settings.rs          ← Settings page (profile, TOTP, S3 keys, sessions, sync,
                                 admin: storages, users, invites, processing rerun)
          setup.rs             ← First-run onboarding (Tauri desktop only)
          trash.rs             ← Trash view (restore / permanently delete)
          version_history.rs   ← Version history panel for a file
          shopping.rs          ← Shopping page: lists, items, categories, shops, share list
          passwords.rs         ← KeePass-format password vaults (browse, open, recents)
          auth/
            login.rs
            register.rs
        hooks/
          api.rs               ← API base URL helpers (seed_api_base for desktop)
          tauri.rs             ← Tauri JS bridge (invoke, get_config, autostart)
          use_auth.rs          ← login/logout/register, TOTP, change-password, server-info
          use_files.rs         ← fetch/upload/delete/move/copy file API calls
          use_events.rs        ← SSE subscription hook (FnMut handler)
          use_music.rs         ← music track/artist/album API calls
          use_player.rs        ← audio player state management
          use_playlists.rs     ← playlist CRUD API calls
          use_search.rs        ← search API calls
          use_shares.rs        ← public share link API calls
          use_folder_shares.rs ← folder-share-with-user API calls
          use_shopping.rs      ← shopping lists/items/categories/shops API calls
          use_tasks.rs         ← projects, sections, tasks, comments, schedule
          use_apps.rs          ← /api/v1/apps for sidebar
          use_s3.rs            ← /api/v1/s3/credentials CRUD
          use_storages.rs      ← admin storages + rescan jobs
          use_processing.rs    ← admin processing rerun
          use_sync_events.rs   ← sync audit log
          use_preferences.rs   ← dashboard_tiles persistence
          media_session.rs     ← Browser MediaSession API (lock-screen / Bluetooth controls)
          native_audio.rs      ← Tauri/Android native-audio bridge for background playback
      assets/
        tailwind.css           ← generated; do not edit by hand
      input.css                ← Tailwind entry point + safe-area utilities
      tailwind.config.js       ← content: ["./src/**/*.rs"], plugin: daisyui
      build.rs                 ← auto-runs npm install + npx tailwindcss at cargo build time
      Dioxus.toml              ← proxy: http://localhost:8080/api/
    uncloud-client/
      src/
        lib.rs                 ← reqwest-based HTTP client with cookie jar
        error.rs               ← client error types
    uncloud-sync/
      src/
        lib.rs                 ← SyncEngine public API + SyncActivity
        engine.rs              ← Two-way sync algorithm + activity broadcast (Idle/Polling/
                                 Transferring) via tokio::sync::watch
        journal.rs             ← SQLite journal (tracks local state)
        fs.rs                  ← LocalFs / NativeFs trait + WalkEntry (testing seam)
    uncloud-desktop/
      src/
        main.rs                ← Tauri entry point (env vars, tray icon, event handler)
        lib.rs                 ← App logic (commands, state, window management, autostart,
                                 sync-on-start, tray activity indicator)
        file_watcher.rs        ← notify-based filesystem watcher with 2s debounce
      src-frontend/            ← Bundled Dioxus web build output (copied by build-desktop.sh)
      icons/                   ← App + tray icons (tray-idle, tray-syncing)
      tauri.conf.json          ← Tauri config (frontendDist: src-frontend, tray icon)
```

---

## Key Conventions

- **Error handling**: `AppError` in `error.rs` maps to HTTP status codes. `AppError::NotFound` -> 404, `AppError::BadRequest` -> 400, etc.
- **Auth**: cookie-based sessions via `AuthUser` extractor; `Authorization: Bearer <token>` is also accepted (used by the desktop app and by sidecar app integrations). Public routes: `/api/auth/*` (login, register, totp/verify, invite/validate, server-info, demo) and `/api/public/*` (public share links). All other API routes require auth.
- **Versioned API**: every authenticated route is mounted under both `/api/...` and `/api/v1/...`. The unversioned form is the legacy/internal surface; `/api/v1/` is the long-term stability contract for sidecar apps. A small set of routes are **v1-only**: `auth/tokens`, `s3/credentials`, `apps`, `auth/me/features`, `auth/me/preferences`. Public-only-on-v1: `apps/register`, `apps/webhooks`.
- **Admin routes**: guarded by `admin_middleware` (`user.role == Admin`). Mounted under `/api/admin/*` and `/api/v1/admin/*`.
- **App reverse proxy**: requests to `/apps/{name}/*` are authenticated via cookies and proxied to the registered app's `base_url`. This gives sidecar apps single-origin auth for free.
- **S3-compatible API**: `/s3` and `/s3/{*rest}` are auth'd by `sigv4_middleware` (NOT cookies/Bearer); the middleware extracts an `S3User` analogous to `AuthUser`. Mounted in parallel with `/api`.
- **Storage**: `StorageService` holds an in-memory map of `ObjectId -> Arc<dyn StorageBackend>`, populated at startup by upserting the storages declared in `config.yaml`. The configured `default` storage receives uploads at root; folders may pin themselves to a different storage via `Folder.storage_id`, and `StorageService::resolve_storage_for_parent` walks the parent chain at upload time to find the closest pinned ancestor (falling back to the default). The admin REST endpoints under `/api/admin/storages` are read-only — `POST`/`PUT`/`DELETE` return 405 with a "configure via config.yaml" message. Removing a storage from config that still has files referencing it is rejected at startup.
- **Events**: SSE stream at `/api/events` for real-time updates. `EventService` broadcasts to per-user channels. Frontend subscribes via `use_events` hook (accepts `FnMut`, uses `Rc<RefCell<F>>`). The `ServerEvent` enum covers file/folder CRUD, processing, folder-share lifecycle, rescan progress, sync-audit appends, and task-project changes (`TaskChanged` fans out to project owner + members).
- **Audit log**: change-inducing handlers call `audit::file_event` / `audit::folder_event` to append a `SyncEvent` row (capped + TTL-purged); the row is also broadcast as `ServerEvent::SyncEventAppended` so other devices for the same user see it live.
- **Assets**: Dioxus requires `asset!()` macro for any file to appear in the build output. CSS is loaded via `document::Stylesheet { href: TAILWIND }` in `app.rs`.
- **DaisyUI themes**: `light` and `dark` only. Theme state lives in `Signal<ThemeState>` context, written to `data-theme` on the root div.
- **Datetime storage**: All model fields use `chrono::DateTime<Utc>` annotated with `#[serde(with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]` (via the `bson` crate's `chrono-0_4` feature). `Option<DateTime<Utc>>` fields use a custom `models::opt_dt` serde module. This keeps domain models free of MongoDB-specific types while storing proper BSON Date objects on disk.
- **Mobile safe-area / Android system bars**: the Android app runs edge-to-edge — web content paints under the status bar (top) and gesture-nav bar (bottom). Any `fixed inset-0`, `fixed top-0`, or `fixed bottom-0` overlay (drawers, modals, full-screen panels, lightboxes) must add `env(safe-area-inset-top)` / `env(safe-area-inset-bottom)` to its top/bottom padding. Use `pt-safe` / `pb-safe` utilities (defined in `input.css`) when no existing padding needs preserving, or inline `style: "padding-top: calc(<base> + env(safe-area-inset-top))"` to add to existing padding. Prefer the shared `RightDrawer` component (`components/right_drawer.rs`) for right-side panels — it already handles this correctly.

---

## Storage Design

### On-Disk Layout

Files are stored at their **logical path** so the filesystem mirrors the user's folder structure. This means data is recoverable by browsing the disk even if MongoDB is completely lost.

```
/data/uncloud/
  alice/
    photos/
      vacation/
        cat.jpg                        <- current version
        dog.jpg
    documents/
      report.pdf
    .uncloud/
      versions/
        photos/vacation/cat.jpg/
          2024-01-15T103000Z            <- previous version blobs
          2024-03-10T092215Z
      trash/
        2024-03-15T142200Z/
          photos/vacation/old-photo.jpg <- deleted files at their original paths
  bob/
    ...
```

- **Current file** lives at `{username}/{logical/path/filename}`
- **Previous versions** live at `{username}/.uncloud/versions/{logical/path/filename}/{iso-timestamp}`
- **Deleted files** live at `{username}/.uncloud/trash/{deleted-at-iso}/` mirroring original path
- **Thumbnails** live at `.thumbs/{file_id}.jpg` inside the storage backend
- The `.uncloud/` directory is hidden from the file browser UI but fully navigable on disk

### StorageBackend Trait

```rust
#[async_trait]
pub trait StorageBackend: Send + Sync {
    async fn read(&self, path: &str) -> Result<BoxedAsyncRead>;
    async fn read_range(&self, path: &str, offset: u64, length: u64) -> Result<BoxedAsyncRead>;
    async fn write(&self, path: &str, data: &[u8]) -> Result<()>;
    async fn write_stream(&self, path: &str, reader: BoxedAsyncRead, size: u64) -> Result<()>;
    async fn delete(&self, path: &str) -> Result<()>;
    async fn exists(&self, path: &str) -> Result<bool>;
    async fn available_space(&self) -> Result<Option<u64>>;
    async fn create_temp(&self) -> Result<String>;
    async fn append_temp(&self, temp_path: &str, data: &[u8]) -> Result<()>;
    async fn finalize_temp(&self, temp_path: &str, final_path: &str) -> Result<()>;
    async fn abort_temp(&self, temp_path: &str) -> Result<()>;
    async fn rename(&self, from: &str, to: &str) -> Result<()>;
    async fn archive_version(&self, current: &str, version: &str) -> Result<()>;
    async fn move_to_trash(&self, current: &str, trash: &str) -> Result<()>;
    async fn restore_from_trash(&self, trash: &str, restore: &str) -> Result<()>;
}
```

Three backends ship today:

- `LocalStorage` (filesystem) — files live under `data/<username>/<folder-chain>/<name>`.
- `S3Storage` (any S3-compatible service: AWS S3, Cloudflare R2, Backblaze B2, MinIO, Wasabi) — files live as objects keyed by their relative path under a single bucket. Built on `aws-sdk-s3` with `force_path_style(true)` for self-hosted compat. Multipart uploads are not used; chunked uploads buffer to a local staging file then `PutObject` on finalize, so the per-file ceiling is the S3 single-object limit (5 GB on AWS).
- `SftpStorage` (any SSH-accessible host: VPS, NAS, dedicated SFTP service) — files live under a configured `base_path` on the remote host. Built on `russh` + `russh-sftp` with a single long-lived SSH session that is recreated on demand. Supports both password and PEM-encoded private-key authentication (passphrase optional). Host-key verification defaults to **TOFU**: the server's public key is recorded in MongoDB `sftp_host_keys` (keyed by `storage_id`) on first connect and verified on every subsequent connect. Pin explicitly with `host_key:` for strict mode, or set `host_key_check: skip` to disable on trusted networks.

Multiple `Storage` records can coexist (each file's `storage_id` decides where its blob lives). The lineup is configured in `config.yaml`; admin REST endpoints under `/api/admin/storages` are read-only.

### Constraint

`(owner_id, parent_id, name)` is the logical identity for a live file — the on-disk layout `{username}/{chain}/{name}` would be ambiguous otherwise. Two layers enforce it:

1. **Handler-level pre-flight check.** `simple_upload`, `complete_upload`, and `copy_file` all call `check_name_conflict` (in `routes/files.rs`) before writing storage / inserting the row, returning **409 Conflict** on collision. The check filters by `deleted_at: null`, so a soft-deleted (trashed) file does not block reusing its name.
2. **MongoDB partial unique index.** `db::setup_indexes` installs a unique index on `files (owner_id, parent_id, name)` with a partial filter `{ deleted_at: null }`. Defence in depth — even if a future handler skips the explicit check, the DB rejects the insert.

If the unique index ever fails to create at server startup against an existing dataset, the most likely cause is residual duplicates from before this constraint existed. Run the one-off cleanup tool first:

```bash
uncloud-server dedupe-files --dry-run   # preview
uncloud-server dedupe-files             # apply
```

It groups live files by `(owner_id, parent_id, name)`, picks the document with the latest `updated_at` as survivor, re-points all `file_versions` of the losers at the survivor, and hard-deletes the losers. Their on-disk bytes live at the same `storage_path` so no filesystem cleanup is needed.

Coverage: `crates/uncloud-server/tests/files.rs` exercises both handler paths (`simple_upload_rejects_duplicate_name`, `complete_upload_rejects_duplicate_name`) plus the partial-filter behaviour (`upload_after_trash_succeeds_partial_filter`).

### Consistency note

There is no two-phase commit across the filesystem and MongoDB. If the disk operation succeeds but the DB update fails (or vice versa), a periodic repair scan reconciles them. In practice this is rare and acceptable for a personal cloud.

---

## API Route Summary

All authenticated routes are mounted under both `/api/...` and `/api/v1/...`. The table below uses the `/api/` form for brevity. **v1-only** routes are explicitly noted.

### Public (no auth)
| Route | Method | Purpose |
|---|---|---|
| `/health` | GET | Health check |
| `/api/auth/server-info` | GET | Public server info (registration mode, version) |
| `/api/auth/register` | POST | User registration |
| `/api/auth/login` | POST | Login (returns session cookie or two-step challenge) |
| `/api/auth/demo` | POST | Create ephemeral demo account (when `registration: demo`) |
| `/api/auth/totp/verify` | POST | Complete two-step login with TOTP code |
| `/api/auth/invite/{token}` | GET | Validate an invite token |
| `/api/public/{token}` | GET | Get public share info |
| `/api/public/{token}/download` | GET | Download shared file |
| `/api/public/{token}/verify` | POST | Verify share password |
| `/api/v1/apps/register` | POST | **v1-only** — sidecar app registration (secret-protected) |
| `/api/v1/apps/webhooks` | POST | **v1-only** — register a webhook (secret-protected) |

### Authenticated
| Route | Method | Purpose |
|---|---|---|
| `/api/auth/logout` | POST | Logout |
| `/api/auth/me` | GET | Current user info |
| `/api/auth/sessions` | GET | List active sessions |
| `/api/auth/sessions/{id}` | DELETE | Revoke session |
| `/api/auth/change-password` | POST | Change own password |
| `/api/auth/totp/setup` | POST | Begin TOTP setup (returns otpauth_uri) |
| `/api/auth/totp/enable` | POST | Activate TOTP after verifying a code |
| `/api/auth/totp/disable` | POST | Deactivate TOTP (requires a code) |
| `/api/v1/auth/me/features` | PUT | **v1-only** — toggle per-user feature opt-outs |
| `/api/v1/auth/me/preferences` | PUT | **v1-only** — update dashboard tiles |
| `/api/v1/auth/tokens` | GET/POST | **v1-only** — list/create scoped Bearer API tokens |
| `/api/v1/auth/tokens/{id}` | DELETE | **v1-only** — revoke a Bearer token |
| `/api/files` | GET | List files (query: `parent_id`) |
| `/api/files/{id}` | GET/PUT/DELETE | Get/update/delete file |
| `/api/files/{id}/download` | GET | Download file (range support) |
| `/api/files/{id}/copy` | POST | Copy file |
| `/api/files/{id}/thumb` | GET | Get thumbnail |
| `/api/files/{id}/content` | POST | Replace file content |
| `/api/files/{id}/versions` | GET | List versions |
| `/api/files/{fid}/versions/{vid}` | GET | Download version |
| `/api/files/{fid}/versions/{vid}/restore` | POST | Restore version |
| `/api/uploads/init` | POST | Init chunked upload |
| `/api/uploads/simple` | POST | Simple single-request upload |
| `/api/uploads/{id}/chunk` | POST | Upload chunk |
| `/api/uploads/{id}/complete` | POST | Complete chunked upload |
| `/api/uploads/{id}` | DELETE | Cancel upload |
| `/api/folders` | GET/POST | List/create folders |
| `/api/folders/{id}` | GET/PUT/DELETE | Get/update/delete folder |
| `/api/folders/{id}/copy` | POST | Copy folder (recursive) |
| `/api/folders/{id}/breadcrumb` | GET | Folder breadcrumb chain |
| `/api/folders/{id}/effective-strategy` | GET | Resolved sync strategy |
| `/api/sync/tree` | GET | Full sync tree |
| `/api/folder-shares` | POST | Share folder with another user |
| `/api/folder-shares/by-me` | GET | List folders I've shared |
| `/api/folder-shares/with-me` | GET | List folders shared with me |
| `/api/folder-shares/folder/{id}` | GET | List shares for a specific folder |
| `/api/folder-shares/{id}` | PUT/DELETE | Update permission / revoke share |
| `/api/gallery` | GET | Gallery images (timeline) |
| `/api/gallery/albums` | GET | Gallery album tree |
| `/api/music/tracks` | GET | All audio tracks |
| `/api/music/folders` | GET | Folders with audio |
| `/api/music/artists` | GET | Artist list |
| `/api/music/artists/{name}/albums` | GET | Albums by artist |
| `/api/music/albums/{artist}/{album}/tracks` | GET | Tracks in album |
| `/api/playlists` | GET/POST | List/create playlists |
| `/api/playlists/{id}` | GET/PUT/DELETE | CRUD playlist |
| `/api/playlists/{id}/tracks` | POST/DELETE | Add/remove tracks |
| `/api/playlists/{id}/tracks/reorder` | PUT | Reorder tracks |
| `/api/users/names` | GET | List usernames (used by share-with-user pickers) |
| `/api/shares` | GET/POST | List/create public share links |
| `/api/shares/{id}` | DELETE | Delete share |
| `/api/trash` | GET/DELETE | List trash / empty all |
| `/api/trash/{id}/restore` | POST | Restore from trash |
| `/api/trash/{id}` | DELETE | Permanently delete |
| `/api/search` | GET | Search files (`?q=`) |
| `/api/search/status` | GET | Search index status |
| `/api/search/reindex` | POST | Admin: rebuild search index |
| `/api/events` | GET | SSE event stream |
| `/api/sync-events` | GET | Sync audit log (paged) |
| `/api/vault-recents` | GET/POST | List/add recent password vault |
| `/api/vault-recents/{file_id}` | DELETE | Remove from recents |
| `/api/shopping/categories` | GET/POST | List/create shopping categories |
| `/api/shopping/categories/{id}` | PUT/DELETE | Update/delete category |
| `/api/shopping/categories/{id}/position` | PUT | Reorder category |
| `/api/shopping/shops` | GET/POST | List/create shops |
| `/api/shopping/shops/{id}` | PUT/DELETE | Update/delete shop |
| `/api/shopping/items` | GET/POST | List/create catalogue items |
| `/api/shopping/items/{id}` | PUT/DELETE | Update/delete catalogue item |
| `/api/shopping/lists` | GET/POST | List/create shopping lists |
| `/api/shopping/lists/{id}` | PUT/DELETE | Rename/delete list |
| `/api/shopping/lists/{id}/items` | GET/POST | Get/add items on a list |
| `/api/shopping/lists/{id}/items/{item_id}` | PATCH/DELETE | Toggle checked/quantity/recurring, or remove from list |
| `/api/shopping/lists/{id}/items/{item_id}/position` | PUT | Reorder list item |
| `/api/shopping/lists/{id}/remove-purchased` | POST | Delete all checked non-recurring items |
| `/api/shopping/lists/{id}/share` | POST | Share list with a user |
| `/api/shopping/lists/{id}/share/{user_id}` | DELETE | Revoke share |
| `/api/tasks/projects` | GET/POST | List/create projects |
| `/api/tasks/projects/{id}` | GET/PUT/DELETE | CRUD project |
| `/api/tasks/projects/{id}/members` | POST | Add project member |
| `/api/tasks/projects/{id}/members/{user_id}` | PUT/DELETE | Change permission / remove member |
| `/api/tasks/projects/{id}/sections` | GET/POST | List/create sections |
| `/api/tasks/projects/{id}/sections/reorder` | PUT | Reorder sections |
| `/api/tasks/projects/{id}/labels` | GET/POST | List/create labels |
| `/api/tasks/projects/{id}/tasks` | GET/POST | List/create tasks |
| `/api/tasks/projects/{id}/tasks/reorder` | PUT | Reorder tasks |
| `/api/tasks/sections/{id}` | PUT/DELETE | Update/delete section |
| `/api/tasks/labels/{id}` | PUT/DELETE | Update/delete label |
| `/api/tasks/{id}` | GET/PUT/DELETE | CRUD task |
| `/api/tasks/{id}/status` | PUT | Update task status |
| `/api/tasks/{id}/subtasks` | POST | Create subtask |
| `/api/tasks/{id}/promote` | POST | Promote subtask to top-level |
| `/api/tasks/{id}/attachments` | POST | Attach files |
| `/api/tasks/{id}/attachments/{file_id}` | DELETE | Detach file |
| `/api/tasks/{id}/comments` | GET/POST | List/create comments |
| `/api/tasks/comments/{id}` | PUT/DELETE | Update/delete comment |
| `/api/tasks/schedule` | GET | Date-grouped upcoming tasks |
| `/api/tasks/assigned-to-me` | GET | Cross-project task feed |
| `/api/v1/apps` | GET | **v1-only** — list apps enabled for current user |
| `/api/v1/s3/credentials` | GET/POST | **v1-only** — list/create S3 access keys |
| `/api/v1/s3/credentials/{id}` | DELETE | **v1-only** — revoke S3 access key |

### Admin (requires admin role)
| Route | Method | Purpose |
|---|---|---|
| `/api/admin/storages` | GET/POST | List/create storage backends |
| `/api/admin/storages/{id}` | PUT/DELETE | Update/delete storage backend |
| `/api/admin/storages/{id}/rescan` | POST | Start rescan job |
| `/api/admin/rescan-jobs/active` | GET | Currently-running rescan job |
| `/api/admin/rescan-jobs/{id}` | GET | Rescan job status |
| `/api/admin/rescan-jobs/{id}/cancel` | POST | Cancel rescan job |
| `/api/admin/users` | GET/POST | List/create users |
| `/api/admin/users/{id}` | PUT/DELETE | Update/delete user |
| `/api/admin/users/{id}/approve` | POST | Approve pending user |
| `/api/admin/users/{id}/disable` | POST | Disable user account |
| `/api/admin/users/{id}/enable` | POST | Re-enable user account |
| `/api/admin/users/{id}/reset-totp` | POST | Clear TOTP secret (lockout recovery) |
| `/api/admin/users/{id}/reset-password` | POST | Set new password |
| `/api/admin/users/{id}/role` | POST | Change user role |
| `/api/admin/invites` | GET/POST | List/create invites |
| `/api/admin/invites/{id}` | DELETE | Delete invite |
| `/api/admin/processing/rerun` | POST | Re-enqueue all applicable processing tasks |
| `/api/v1/admin/apps/{name}` | DELETE | **v1-only** — delete a registered app |
| `/api/v1/admin/apps/webhooks/{id}` | DELETE | **v1-only** — delete a webhook |

### S3-Compatible (SigV4 auth)
| Route | Method | Purpose |
|---|---|---|
| `/s3` | GET | ListBuckets |
| `/s3/{*rest}` | ANY | All other S3 operations (see Features → S3-Compatible API) |

### App Reverse Proxy (cookie/Bearer auth)
| Route | Method | Purpose |
|---|---|---|
| `/apps/{name}` | ANY | Proxy to registered app |
| `/apps/{name}/{*path}` | ANY | Proxy to registered app sub-paths |

---

## `config.yaml` Reference

```yaml
server:
  host: "0.0.0.0"
  port: 8080

database:
  uri: "mongodb://localhost:27017"
  name: "uncloud"

storage:
  default: main                    # name of the default storage
  storages:
    - name: main
      type: local
      path: /data/uncloud
    # - name: cold
    #   type: s3
    #   endpoint: https://s3.us-west-002.backblazeb2.com
    #   bucket: my-uncloud-cold
    #   access_key: ${B2_KEY_ID}    # ${VAR} expanded from env
    #   secret_key: ${B2_APP_KEY}
    #   region: us-west-002
  # Legacy fallback: when `storages` is empty, a single "local" entry is
  # synthesised from this path. Old configs keep working unchanged.
  # default_path: "/data/uncloud"

auth:
  session_duration_hours: 168      # 7 days
  # registration mode: open | approval | invite_only | disabled | demo
  registration: open
  # demo_quota_bytes: 52428800     # 50MB quota for demo accounts
  # demo_ttl_hours: 24             # auto-purge demo accounts after N hours

uploads:
  max_chunk_size: 10485760         # 10MB
  max_file_size: 0                 # 0 = unlimited
  temp_cleanup_hours: 24

processing:
  max_concurrency: 4               # semaphore limit across all processors
  thumbnail_size: 320              # max px on longest edge
  max_attempts: 3                  # give up after N failures per task

search:
  enabled: false                   # set true + run Meilisearch to enable
  url: "http://localhost:7700"
  api_key: null                    # optional Meilisearch API key

versioning:
  max_versions: 50                 # per file; oldest pruned beyond this
  trash_retention_days: 30         # auto-purge trash after N days (0 = never)

apps:
  # registration_secret: "change-me"   # required to enable sidecar app registration
  managed: []                          # list of locally-supervised sidecar processes
    # Example managed app:
    # - name: shopping
    #   command: cargo run -p uncloud-shopping
    #   working_dir: /path/to/uncloud   # optional
    #   env:
    #     SHOPPING_REGISTRATION_SECRET: change-me
    #     SHOPPING_PORT: "8082"
    #     SHOPPING_UNCLOUD_URL: http://localhost:8080
    #   restart: on-failure             # always | on-failure | never
    #   restart_max_attempts: 5         # 0 = unlimited
    #   restart_backoff_secs: 2         # initial backoff, doubles each attempt (max 60s)

sync_audit:
  enabled: true                    # master switch for the audit log
  retention_days: 7                # MongoDB TTL for sync_event rows
  max_records_per_user: 10000      # hard cap; older rows pruned past this

features:
  shopping: true                   # server-wide shopping feature toggle (per-user opt-out
                                   # via User.disabled_features)

logging:
  # tracing_subscriber EnvFilter directive. RUST_LOG env var, when set, always overrides.
  # Default: `uncloud_server=info,tower_http=info` in release, debug in debug builds.
  level: "uncloud_server=info,tower_http=info"
```

All config sections have sensible `Default` implementations, so existing `config.yaml` files remain valid when new fields are added. The legacy `auth.registration_enabled: bool` field is still accepted and mapped to `open` / `disabled`.
