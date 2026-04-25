# Uncloud — Project Guide

## Overview

Uncloud is a self-hosted personal cloud storage system. It is a Rust workspace with six crates:

- `crates/uncloud-server` — Axum HTTP server, REST API, MongoDB, local file storage
- `crates/uncloud-web` — Dioxus 0.7 WASM frontend, Tailwind CSS + DaisyUI
- `crates/uncloud-common` — Shared types (API request/response structs, `ServerEvent`, validation), re-exported to both server and web
- `crates/uncloud-client` — Native HTTP client (reqwest with cookie jar) for the desktop app
- `crates/uncloud-sync` — Two-way file sync engine with SQLite journal (used by the desktop app)
- `crates/uncloud-desktop` — Tauri v2 desktop app (tray-only, bundles the Dioxus web frontend)

## Repository Layout

```
Uncloud/
  config.yaml                  ← runtime config (storage paths, auth, DB URI)
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
          auth.rs              ← LoginRequest, RegisterRequest, UserResponse, UserRole
          files.rs             ← FileResponse, UploadInit/Complete, GalleryResponse
          folders.rs           ← FolderResponse, SyncStrategy
          music.rs             ← TrackResponse, MusicArtistResponse, MusicAlbumResponse
          playlists.rs         ← PlaylistResponse, CreatePlaylist, PlaylistTrack
          shares.rs            ← ShareResponse, CreateShareRequest
          events.rs            ← ServerEvent enum
          search.rs            ← SearchResponse, SearchStatus
          versions.rs          ← VersionResponse
          shopping.rs          ← ShoppingList/Item/Category/Shop request/response types
    uncloud-server/
      src/
        main.rs                ← AppState, startup, trash auto-purge task
        config.rs              ← Config struct, loads config.yaml
        db.rs                  ← MongoDB connection + index setup
        error.rs               ← AppError → HTTP status mapping
        routes/                ← Axum handlers
          auth.rs              ← register, login, logout, me, sessions
          files.rs             ← CRUD, upload (simple + chunked), download, thumb, gallery
          folders.rs           ← CRUD, copy, breadcrumb, sync strategy, sync tree
          music.rs             ← tracks, folders, artists, albums
          playlists.rs         ← CRUD, add/remove/reorder tracks
          shares.rs            ← create/list/delete share links, public download
          trash.rs             ← list, restore, permanently delete, empty
          versions.rs          ← list, download, restore versions
          search.rs            ← search files, search status, admin reindex
          storages.rs          ← admin: CRUD storage backends
          users.rs             ← admin: CRUD users
          events.rs            ← SSE stream
          shopping.rs          ← lists/items/categories/shops + list sharing, mark-purchased
        services/
          auth.rs              ← AuthService (sessions, password hashing, user bytes)
          storage.rs           ← StorageService (backend registry, get_or_create_default)
          events.rs            ← EventService (per-user SSE broadcast channels)
          search.rs            ← SearchService (Meilisearch SDK wrapper)
        storage/
          mod.rs               ← StorageBackend trait
          local.rs             ← LocalStorage impl (filesystem)
        models/
          user.rs              ← User, UserRole
          session.rs           ← Session
          file.rs              ← File, FileVersion, ProcessingTask, TaskType, ProcessingStatus
          folder.rs            ← Folder, SyncStrategy
          storage.rs           ← Storage (backend config doc)
          share.rs             ← Share, ShareResourceType
          playlist.rs          ← Playlist, PlaylistTrack
          shopping.rs          ← ShoppingCategory, Shop, ShoppingItem, ShoppingList, ShoppingListItem
        middleware/
          auth.rs              ← AuthUser extractor, auth_middleware, admin_middleware
        processing/
          mod.rs               ← FileProcessor trait, re-exports all processors
          service.rs           ← ProcessingService (enqueue, recover, run_task)
          thumbnail.rs         ← ThumbnailProcessor (image/*)
          audio_metadata.rs    ← AudioMetadataProcessor (audio/*, extracts ID3/Vorbis tags + cover art)
          text_extract.rs      ← TextExtractProcessor (text/*, PDF, DOCX)
          search_index.rs      ← SearchIndexProcessor (indexes into Meilisearch)
    uncloud-web/
      src/
        app.rs                 ← App root, provides AuthState + ThemeState + PlayerState contexts
        router.rs              ← Dioxus Router (all routes under Layout)
        state.rs               ← AuthState, ThemeState, Section, FileBrowserState, ViewMode, PlayerState
        components/
          layout.rs            ← Drawer shell + Navbar (search bar, theme toggle)
          sidebar.rs           ← Section-aware sidebar nav + StorageUsage
          file_browser.rs      ← File listing (grid/list), selection, bulk actions, modals
          file_item.rs         ← Individual file card/row (thumbnail, context menu trigger)
          file_viewer.rs       ← File viewer (image lightbox, text viewer, PDF, audio playback)
          upload.rs            ← Upload zone (hidden input + drag-and-drop)
          context_menu.rs      ← Right-click / long-press dropdown (actions per file type)
          share_dialog.rs      ← Share link modal (password, expiry, download limit)
          lightbox.rs          ← Image lightbox overlay
          search.rs            ← Search results overlay / page
          gallery.rs           ← Gallery page (timeline + album tree + folder inclusion settings)
          music/
            mod.rs             ← Music page shell + tab navigation
            artist_list.rs     ← Artist grid/list
            artist_view.rs     ← Artist detail (albums for an artist)
            album_grid.rs      ← Album grid display
            album_view.rs      ← Album detail (track listing)
            folder_view.rs     ← Browse music by folder
            playlist_view.rs   ← Playlist detail (track list + reorder)
            track_list.rs      ← Reusable track list component
          player.rs            ← Audio player bar (play/pause, skip, queue, progress)
          settings.rs          ← Settings page (admin: storage + users; desktop: sync status)
          setup.rs             ← First-run onboarding (Tauri desktop only)
          trash.rs             ← Trash view (restore / permanently delete)
          version_history.rs   ← Version history panel for a file
          shopping.rs          ← Shopping page: lists, items, categories, shops, share list
          auth/
            login.rs
            register.rs
        hooks/
          api.rs               ← API base URL helpers (seed_api_base for desktop)
          tauri.rs             ← Tauri JS bridge (invoke, get_config)
          use_auth.rs          ← login/logout/register API calls
          use_files.rs         ← fetch/upload/delete/move/copy file API calls
          use_events.rs        ← SSE subscription hook (FnMut handler)
          use_music.rs         ← music track/artist/album API calls
          use_player.rs        ← audio player state management
          use_playlists.rs     ← playlist CRUD API calls
          use_search.rs        ← search API calls
          use_shares.rs        ← share link API calls
          use_shopping.rs      ← shopping lists/items/categories/shops API calls
      assets/
        tailwind.css           ← generated; do not edit by hand
      input.css                ← Tailwind entry point (@tailwind base/components/utilities)
      tailwind.config.js       ← content: ["./src/**/*.rs"], plugin: daisyui
      build.rs                 ← auto-runs npm install + npx tailwindcss at cargo build time
      Dioxus.toml              ← proxy: http://localhost:8080/api/
    uncloud-client/
      src/
        lib.rs                 ← reqwest-based HTTP client with cookie jar
        error.rs               ← client error types
    uncloud-sync/
      src/
        lib.rs                 ← SyncEngine public API
        engine.rs              ← Two-way sync algorithm
        journal.rs             ← SQLite journal (tracks local state)
    uncloud-desktop/
      src/
        main.rs                ← Tauri entry point (env vars, tray icon, event handler)
        lib.rs                 ← App logic (commands, state, window management)
      src-frontend/            ← Bundled Dioxus web build output (copied by build-desktop.sh)
      tauri.conf.json          ← Tauri config (frontendDist: src-frontend, tray icon)
```

## Git Workflow

Scope the workflow to the size of the change.

### Large features → feature branch + PR + manual test

New features, significant refactors, schema changes, multi-file behaviour changes:

1. **Create a feature branch**:
   ```bash
   git checkout -b feature/<short-description>
   ```
2. **Commit changes** to that branch.
3. **Push** to origin:
   ```bash
   git push -u origin feature/<short-description>
   ```
4. **Open a PR** with `gh`:
   ```bash
   gh pr create --fill --base main
   ```
5. **Stop — do not merge**. The user manually tests the branch, then merges via GitHub UI or `gh pr merge` once satisfied.

### Small fixes → direct to `main` (or the relevant open branch)

Bug fixes, doc updates, config tweaks, CI adjustments, small maintenance — commit directly on `main`. No branch, no PR. If a feature branch is already open and the fix belongs there, push to it directly.

### Amending an open PR

```bash
git checkout feature/<branch-name>
# make the fix
git add <files> && git commit -m "Fix: ..."
git push                              # updates the PR automatically
git checkout main
```

### Main working directory stays on `main`

The primary checkout must always be on `main`. For large features, work in isolated worktrees so the primary tree's `git status` / builds stay predictable.

> Remote: `https://github.com/decaychain/uncloud.git`.

## CI / GitHub Actions

Workflows in `.github/workflows/` are triggered manually or on release tags — never on every push/PR:

| Workflow | Auto trigger | Manual trigger |
|---|---|---|
| `ci.yml` | — | `workflow_dispatch` |
| `release-server.yml` | push tag `v*` | `workflow_dispatch` |
| `release-desktop.yml` | push tag `v*` | `workflow_dispatch` |
| `release-android.yml` | push tag `v*` | `workflow_dispatch` |

Run a workflow manually from the Actions tab or:
```bash
gh workflow run ci.yml                         # on default branch
gh workflow run ci.yml --ref feature/foo       # on a specific branch
```

This keeps the feedback loop tight locally and avoids long CI queues on every commit. Verify builds locally before pushing.

---

## Dev Workflow

```bash
# Backend
cargo run -p uncloud-server

# Server CLI subcommands (each maps to a one-off task; default is `serve`):
cargo run -p uncloud-server -- bootstrap-admin --username alice
cargo run -p uncloud-server -- dedupe-files --dry-run    # see Storage Design → Constraint

# Frontend (Tailwind is rebuilt automatically by build.rs on cargo build,
# but for watch mode during active UI work run both):
cd crates/uncloud-web
npx tailwindcss -i input.css -o assets/tailwind.css --watch   # Terminal 1
dx serve                                                        # Terminal 2

# Desktop (requires webkit2gtk4.1-devel, libsoup3-devel on Fedora)
./build-desktop.sh   # dx build → copy to src-frontend → cargo build desktop
cargo run -p uncloud-desktop
```

## Key Conventions

- **Error handling**: `AppError` in `error.rs` maps to HTTP status codes. `AppError::NotFound` -> 404, `AppError::BadRequest` -> 400, etc.
- **Auth**: cookie-based sessions via `AuthUser` extractor in middleware. All API routes require auth except `/api/auth/*` and `/api/public/*`. Bearer token auth is also accepted (for the desktop app).
- **Admin routes**: guarded by `admin_middleware` which checks `user.role == Admin`. Mounted under `/api/admin/*`.
- **Storage**: `StorageService` holds an in-memory map of `ObjectId -> Arc<dyn StorageBackend>`. Default storage is auto-provisioned on first upload via `get_or_create_default(user_id)`.
- **Events**: SSE stream at `/api/events` for real-time file updates. `EventService` broadcasts to per-user channels. Frontend subscribes via `use_events` hook (accepts `FnMut`, uses `Rc<RefCell<F>>`).
- **Assets**: Dioxus requires `asset!()` macro for any file to appear in the build output. CSS is loaded via `document::Stylesheet { href: TAILWIND }` in `app.rs`.
- **DaisyUI themes**: `light` and `dark` only. Theme state lives in `Signal<ThemeState>` context, written to `data-theme` on the root div.
- **Datetime storage**: All model fields use `chrono::DateTime<Utc>` annotated with `#[serde(with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]` (via the `bson` crate's `chrono-0_4` feature). `Option<DateTime<Utc>>` fields use a custom `models::opt_dt` serde module. This keeps domain models free of MongoDB-specific types while storing proper BSON Date objects on disk.
- **Mobile safe-area / Android system bars**: the Android app runs edge-to-edge — web content paints under the status bar (top) and gesture-nav bar (bottom). Any `fixed inset-0`, `fixed top-0`, or `fixed bottom-0` overlay (drawers, modals, full-screen panels, lightboxes) must add `env(safe-area-inset-top)` / `env(safe-area-inset-bottom)` to its top/bottom padding so content doesn't sit under the system bars. Use `pt-safe` / `pb-safe` utilities (defined in `input.css`) when no existing padding needs preserving, or inline `style: "padding-top: calc(<base> + env(safe-area-inset-top))"` to add to existing padding. Prefer the shared `RightDrawer` component (`components/right_drawer.rs`) for right-side panels — it already handles this correctly.

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

Currently only `LocalStorage` (filesystem) is implemented.

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

## Implemented Features

### File Management

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

### Version History

Every re-upload of an existing file archives the previous blob and creates a version record.

- **DB**: `file_versions` collection stores `{ file_id, version, storage_path, size_bytes, checksum, created_at }`; `files` doc has a `version: i32` field that increments
- **API**: `GET /api/files/{id}/versions` (list), `GET /api/files/{file_id}/versions/{version_id}` (download), `POST /api/files/{file_id}/versions/{version_id}/restore` (promote to current)
- **Frontend**: `version_history.rs` component shows a timeline of versions with timestamp, size, and Restore button

### Trash

Deletes are soft. Files and folders go to trash and can be recovered. Auto-purge runs as a background task.

- **DB**: `deleted_at: Option<DateTime<Utc>>` and `trash_path: Option<String>` on `File` and `Folder` docs; all normal queries filter `deleted_at: null`
- **API**: `GET /api/trash` (list), `POST /api/trash/{id}/restore`, `DELETE /api/trash/{id}` (permanent), `DELETE /api/trash` (empty all)
- **Frontend**: `trash.rs` component accessible from the sidebar; shows deleted files with original path and deletion date; Restore / Permanently Delete actions
- **Auto-purge**: background `tokio::spawn` task in `main.rs` checks hourly and purges trashed files older than `versioning.trash_retention_days`; also cleans up associated versions and updates user byte quotas
- **Config**: `versioning.trash_retention_days` (default 30, 0 = never auto-purge), `versioning.max_versions` (default 50)

### Post-Upload Processing Pipeline

Extensible background processing triggered after each upload. All processors implement the `FileProcessor` trait and are registered with `ProcessingService` at startup.

- **`FileProcessor` trait**: `task_type() -> TaskType`, `applies_to(&File) -> bool`, `process(&File, Arc<AppState>) -> Result<(), String>`
- **`ProcessingService`**: `register()` adds processors, `enqueue()` spawns applicable tasks (bounded by semaphore), `recover()` retries pending/failed tasks on startup (also backfills files that predate the pipeline)
- **DB**: `processing_tasks: Vec<ProcessingTask>` embedded array on each `File` document; each task has `task_type`, `status` (Pending/Done/Error), `attempts`, `error`, `queued_at`, `completed_at`
- **SSE**: `ServerEvent::ProcessingCompleted { file_id, task_type, success }` emitted on task completion; frontend re-renders thumbnails, etc.
- **Config**: `processing.max_concurrency` (default 4), `processing.thumbnail_size` (default 320px), `processing.max_attempts` (default 3)

#### Processors

| Processor | Applies to | Output |
|---|---|---|
| `ThumbnailProcessor` | `image/*` | `.thumbs/{file_id}.jpg` via `image` crate |
| `AudioMetadataProcessor` | `audio/*` | Extracts ID3/Vorbis tags (artist, album, track, year) + embedded cover art as thumbnail |
| `TextExtractProcessor` | `text/*`, `application/pdf` | `content_text` field on file doc |
| `SearchIndexProcessor` | all files (when search enabled) | Meilisearch document upsert |

- **Thumbnail API**: `GET /api/files/{id}/thumb` -> `200` + JPEG if ready, `202` if pending, `404` if not applicable

### Full-Text Search (Meilisearch)

Full-text search powered by Meilisearch. Runs as a separate process alongside the server.

- **`SearchService`** in `services/search.rs`: wraps `meilisearch-sdk` client; `index_file()`, `delete_file()`, `search()` filtered by `owner_id`
- **API**: `GET /api/search?q=<query>` (returns matching files for current user), `GET /api/search/status` (index health), `POST /api/search/reindex` (admin: rebuild full index)
- **Frontend**: search bar in the Navbar; `search.rs` component displays results; `use_search.rs` hook
- **Text extraction**: `TextExtractProcessor` extracts content from text files and PDFs; `SearchIndexProcessor` upserts into Meilisearch
- **Config**: `search.enabled` (default false), `search.url` (default `http://localhost:7700`), `search.api_key` (optional)
- **Meilisearch index** is a derivative of MongoDB data and can be fully rebuilt via the admin reindex endpoint

### Gallery

Photo gallery with timeline view and album organization.

- **API**: `GET /api/gallery` (all images for user, sorted by date), `GET /api/gallery/albums` (folder tree of image-containing folders)
- **Frontend**: `gallery.rs` component with timeline view and album tree; clicking an album navigates to `GalleryAlbum { id }` route; images use lightbox overlay (`lightbox.rs`)
- **Routes**: `/gallery`, `/gallery/album/:id`

### Music + Playlists

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
  - Components in `components/music/`: `artist_list`, `artist_view`, `album_grid`, `album_view`, `folder_view`, `playlist_view`, `track_list`
  - `player.rs`: persistent audio player bar with play/pause, skip, queue display, progress
  - `PlayerState` context with queue, current index, playing state
  - Hooks: `use_music.rs`, `use_player.rs`, `use_playlists.rs`

### File Viewer

In-browser file viewing for multiple content types.

- **`file_viewer.rs`**: image lightbox (via `lightbox.rs`), plain text viewer, PDF viewer, audio playback
- Accessed from file item click or context menu

### Sharing

Share files and folders via public links with optional access controls.

- **DB**: `Share` model with `token`, `resource_type` (File/Folder), `resource_id`, `owner_id`, `password_hash`, `expires_at`, `download_count`, `max_downloads`
- **Auth routes (public)**: `GET /api/public/{token}` (get share info), `GET /api/public/{token}/download` (download), `POST /api/public/{token}/verify` (password check)
- **Authenticated routes**: `GET /api/shares`, `POST /api/shares`, `DELETE /api/shares/{id}`
- **Frontend**: `share_dialog.rs` modal (password, expiry, download limit); `/share/:token` public route; `use_shares.rs` hook

### Admin

Admin panel for storage and user management. Guarded by `admin_middleware`.

- **Storage management**: `GET/POST /api/admin/storages`, `PUT/DELETE /api/admin/storages/{id}`
- **User management**: `GET/POST /api/admin/users`, `PUT/DELETE /api/admin/users/{id}` (role, quota, etc.)
- **Frontend**: admin sections in `settings.rs` component (only visible when `AuthState.is_admin()`)
- **User model**: `UserRole` enum (Admin/User), `quota_bytes: Option<i64>`, `used_bytes: i64`

### File Sync (Folder-Level Strategy)

Per-folder sync strategy configuration for the desktop sync engine.

- **`SyncStrategy` enum** in `uncloud-common`: Inherit, TwoWay, ClientToServer, ServerToClient, UploadOnly, DoNotSync
- **API**: `GET /api/folders/{id}/effective-strategy` (resolves inheritance up the parent chain), `GET /api/sync/tree` (full folder tree with strategies)
- **Frontend**: right-click folder -> "Sync settings..." opens a strategy dropdown modal

### Desktop App (Tauri v2)

Tray-only desktop application that bundles the Dioxus web frontend and provides local file sync.

- **Architecture**: Tauri v2 with `frontendDist: src-frontend` serving the bundled Dioxus build
- **Tray menu**: Open Uncloud / Sync Now / Quit
- **First-run**: `/setup` route for server URL configuration; seeds `api_base` via Tauri invoke
- **Sync**: uses `uncloud-client` for HTTP and `uncloud-sync` for two-way sync with SQLite journal
- **CORS**: server allows `tauri://localhost` and `https://tauri.localhost` with credentials
- **Build**: `./build-desktop.sh` at workspace root (dx build -> copy to src-frontend -> cargo build)
- **Linux deps**: `webkit2gtk4.1-devel`, `libsoup3-devel`, `libappindicator-gtk3-devel`

### Real-Time Events (SSE)

`EventService` broadcasts `ServerEvent` variants to per-user channels. Frontend subscribes at `/api/events`.

Events include: file/folder CRUD notifications, `ProcessingCompleted`, and other state changes. `FileBrowser` refreshes on relevant events; thumbnail updates trigger image re-render.

### Shopping Lists

A lightweight shopping-list companion app that shares the Uncloud session. Users maintain a per-user catalogue of items (with categories and shops) and group them into shareable lists; items on a list can be checked off as purchased, and a list can be shared with other Uncloud users.

- **Model** (MongoDB collections):
  - `shopping_categories` — `ShoppingCategory { owner_id, name, position }` (ordered labels used to tag items and shops)
  - `shopping_shops` — `Shop { owner_id, name, categories: Vec<String> }` (user's stores; each shop claims a set of categories so items auto-associate)
  - `shopping_items` — `ShoppingItem { owner_id, name, categories, shop_ids, notes }` (the catalogue; re-used across lists)
  - `shopping_lists` — `ShoppingList { owner_id, name, shared_with: Vec<ObjectId> }`
  - `shopping_list_items` — `ShoppingListItem { list_id, item_id, checked, recurring, quantity, position }` (join row; `recurring` items stay on the list when "remove purchased" is invoked)
- **Access control**: `list_access_filter` lets either the owner or any user in `shared_with` read/mutate a list's rows; catalogue (categories/shops/items) is always owner-scoped.
- **Feature flag**: `require_shopping` gate at the top of every handler checks `config.features.shopping` (server-wide) AND `user.disabled_features` (per-user opt-out via `PUT /api/auth/me` `{ "shopping": false }`). Disabled users get 404.
- **Frontend**: `components/shopping.rs` provides `ShoppingPage` (list index + shops/categories/items management) and `ShoppingListView` (per-list view with check/uncheck, inline item inlet, drag-to-reorder, "remove purchased" bulk action); routes `/shopping` and `/shopping/list/:id`.
- **Marking as purchased**: `PATCH /api/shopping/lists/{id}/items/{item_id}` with `{ "checked": true }`; bulk cleanup via `POST /api/shopping/lists/{id}/remove-purchased` which deletes all checked non-recurring rows.

---

## API Route Summary

### Public (no auth)
| Route | Method | Purpose |
|---|---|---|
| `/health` | GET | Health check |
| `/api/auth/register` | POST | User registration |
| `/api/auth/login` | POST | Login (returns session cookie) |
| `/api/public/{token}` | GET | Get public share info |
| `/api/public/{token}/download` | GET | Download shared file |
| `/api/public/{token}/verify` | POST | Verify share password |

### Authenticated
| Route | Method | Purpose |
|---|---|---|
| `/api/auth/logout` | POST | Logout |
| `/api/auth/me` | GET | Current user info |
| `/api/auth/sessions` | GET | List active sessions |
| `/api/auth/sessions/{id}` | DELETE | Revoke session |
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
| `/api/shares` | GET/POST | List/create shares |
| `/api/shares/{id}` | DELETE | Delete share |
| `/api/trash` | GET/DELETE | List trash / empty all |
| `/api/trash/{id}/restore` | POST | Restore from trash |
| `/api/trash/{id}` | DELETE | Permanently delete |
| `/api/search` | GET | Search files (`?q=`) |
| `/api/search/status` | GET | Search index status |
| `/api/search/reindex` | POST | Admin: rebuild search index |
| `/api/events` | GET | SSE event stream |
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
| `/api/shopping/lists/{id}/remove-purchased` | POST | Delete all checked non-recurring items from a list |
| `/api/shopping/lists/{id}/share` | POST | Share list with a user |
| `/api/shopping/lists/{id}/share/{user_id}` | DELETE | Revoke share |

### Admin (requires admin role)
| Route | Method | Purpose |
|---|---|---|
| `/api/admin/storages` | GET/POST | List/create storage backends |
| `/api/admin/storages/{id}` | PUT/DELETE | Update/delete storage backend |
| `/api/admin/users` | GET/POST | List/create users |
| `/api/admin/users/{id}` | PUT/DELETE | Update/delete user |

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
  default_path: "/data/uncloud"

auth:
  session_duration_hours: 168      # 7 days
  registration_enabled: true

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

features:
  shopping: true                   # server-wide shopping-list feature toggle; per-user opt-out stored on User.disabled_features

logging:
  # `tracing_subscriber::EnvFilter` directive. `RUST_LOG` env var, when set,
  # always overrides this value. Defaults to `uncloud_server=info,tower_http=info`
  # in release builds, `uncloud_server=debug,tower_http=debug` in debug builds.
  level: "uncloud_server=info,tower_http=info"
```

All config sections have sensible `Default` implementations, so existing `config.yaml` files remain valid when new fields are added.

---

## Planned Features

### App Platform (not yet implemented)

**Decision: sidecar HTTP services** (not WASM plugins)

Each app is an independent process that registers itself with Uncloud at startup. Uncloud acts as the platform: auth provider, reverse proxy, file storage, event bus. Apps can be written in any language.

**Prerequisites (changes to the core server):**

1. **Versioned API** — prefix all routes with `/api/v1/`. Apps need a stability guarantee; the current unversioned routes are internal-only.

2. **Scoped bearer tokens** — current auth is cookie-sessions only. Apps need `Authorization: Bearer <token>` with a permission scope (e.g. `files:read`, `files:write`, `profile:read`). Add a `tokens` collection and a `POST /api/v1/auth/tokens` endpoint.

3. **App registry** — `apps` MongoDB collection storing `{ id, name, icon_url, base_url, nav_label, enabled_for: [user_ids] }`. New routes:
   - `POST /api/v1/apps/register` — called by an app on startup
   - `GET /api/v1/apps` — returns apps enabled for the current user (consumed by the frontend sidebar)

4. **Reverse proxy layer** — Uncloud proxies `/apps/<name>/` -> `http://localhost:<app_port>/`. Single origin for the browser; cookies and auth work without CORS. `tower-http` reverse proxy or `hyper` client in a catch-all handler.

5. **Webhook delivery** — `POST /api/v1/apps/webhooks` to register a URL; Uncloud calls it on `file.created`, `file.deleted`, `file.updated` events. Retry with exponential backoff on failure.

6. **Frontend nav** — sidebar nav items are data-driven from `GET /api/v1/apps` rather than hardcoded. An app entry is just a link to `/apps/<name>/`.

**Operational model:**
```
uncloud-server   :8080   <- main server + proxy
uncloud-notes    :8081   <- example app (registers on startup, proxied at /apps/notes/)
uncloud-calendar :8082   <- example app
```

All behind a single domain/port. Docker Compose or systemd units manage the processes independently.

**What an app gets for free:**
- Auth (redirect to Uncloud login, get back a scoped token)
- File storage (read/write user files via the existing file API)
- Real-time events (subscribe to webhooks or the SSE stream)
- A slot in the sidebar nav

**What an app brings itself:**
- Its own frontend (served as static files or a web app at its registered base URL)
- Its own data store if needed (SQLite, Postgres, etc.)
- Its own business logic

---

### S3-Compatible API (not yet implemented)

Expose an S3-compatible REST API so that standard S3 tools (`s5cmd`, `rclone`, `aws-cli`, Cyberduck, etc.) work against Uncloud without any custom client.

**Reference implementation:** [Garage](https://git.deuxfleurs.fr/Deuxfleurs/garage) — a distributed object store written in Rust with full S3 compatibility. Useful as a reference for SigV4 and XML response shapes.

**Required operations for `s5cmd` compatibility:**

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

Multipart upload maps directly onto the existing `UploadChunk` model.

**Implementation plan:**

1. **AWS Signature V4 verifier** — the main piece of work. No Rust crate handles server-side SigV4 verification; implement it in `crates/uncloud-server/src/middleware/sigv4.rs`:
   - Parse `Authorization: AWS4-HMAC-SHA256 Credential=..., SignedHeaders=..., Signature=...`
   - Reconstruct canonical request (method + path + sorted query params + lowercased signed headers + payload hash)
   - Reconstruct string-to-sign (algorithm + date + credential scope + canonical request hash)
   - Derive signing key via four nested HMAC-SHA256 rounds (date -> region -> service -> `aws4_request`)
   - Compare computed signature against the one in the header
   - `hmac` and `sha2` are already in Cargo.toml
   - ~150 lines; fiddly (canonical query string encoding, header normalisation) but fully specified

2. **S3 credentials** — new `s3_credentials` MongoDB collection: `{ id, user_id, access_key_id, secret_access_key, label, created_at }`. New API routes (behind normal cookie auth):
   - `POST /api/v1/s3/credentials` — generate a keypair, return secret once
   - `GET /api/v1/s3/credentials` — list access keys for current user
   - `DELETE /api/v1/s3/credentials/:id` — revoke

3. **Bucket model** — one bucket per user, named after their username. S3 key maps to file path. `s3://alice/photos/cat.jpg` -> alice's file at `photos/cat.jpg`. Path-style URLs only initially (`/{bucket}/{key}`); virtual-hosted style (`bucket.host`) requires wildcard DNS and can come later.

4. **XML response layer** — S3 speaks XML, not JSON. Add `quick-xml` + `serde` to Cargo.toml. Define response structs for `ListBucketsResult`, `ListObjectsV2Result`, `DeleteResult`, `InitiateMultipartUploadResult`, `CompleteMultipartUploadResult`, `Error`, etc.

5. **Route handlers** — new module `crates/uncloud-server/src/routes/s3.rs`, mounted at `/s3` (path-style). SigV4 middleware extracts the `access_key_id`, looks up the secret, verifies the signature, and injects an `S3User` extractor analogous to `AuthUser`.

6. **Settings UI** — new panel in the Settings page: "S3 Access Keys". List existing keys, generate new key (show secret once with copy button), revoke keys. Credentials are configured in s5cmd like:
   ```bash
   s5cmd --endpoint-url http://localhost:8080/s3 \
         --credentials-file ~/.aws/uncloud \
         ls s3://alice/
   ```

**What stays unchanged:** the underlying `StorageBackend` and file model are reused as-is. The S3 API is purely a new HTTP surface over the same storage layer.
