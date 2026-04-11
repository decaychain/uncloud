# Client Sync Overhaul — Implementation Plan

Branch: `feature/client-sync-overhaul` (branched off `main` after PR #8 merged).

## Motivation

The current sync engine walks a single `root_local_path` and ignores per-folder
`local_path` entries in the journal. This means:

- Android was given a placeholder `sync-root/` directory, which violated the
  "nothing syncs by default on mobile" intent and littered app-private
  storage with files.
- Per-folder local paths picked in the Folder Settings modal are stored but
  never consulted by the engine, so users see the UI accept a path and then
  nothing actually syncs there.
- The client has no way to override the server's default strategy for a
  specific folder on one device while leaving others alone.

## Design

### Server (unchanged)

Every folder has a `sync_strategy` field — this is the **server-side default**
that applies to all clients. Root is implicitly `TwoWay`; descendants default
to `Inherit`. The server never stores local paths, because paths are
client-specific.

### Client journal (changed)

Schema becomes:

```sql
CREATE TABLE folder_sync_config (
    folder_id    TEXT PRIMARY KEY,
    strategy     TEXT NULL,    -- NULL = use server default
    local_path   TEXT NULL,    -- NULL = inherit from ancestor
    updated_at   TEXT NOT NULL
);
```

Strategy and path are **independently nullable** and **independently writable**.
Existing rows with `"inherit"` are migrated to NULL.

### Resolution rules (run in the sync engine for each folder)

**Strategy resolution:**

1. If the client has an override for this folder, use it.
2. Otherwise walk up the client-override chain. If any ancestor has an
   explicit override, use it.
3. Otherwise use the server's `effective_strategy` for this folder (which
   itself walks the server parent chain with server-side defaults).
4. Otherwise fall back to the client-root default:
   - Desktop: `TwoWay`
   - Mobile: `DoNotSync`

**Local path resolution:**

1. If the folder has an explicit `local_path`, use it.
2. Otherwise find the nearest ancestor with a `local_path` set and join with
   the relative subpath walked from that ancestor.
3. If no ancestor has a path and the resolved strategy would sync this
   subtree, report an error and skip that subtree. The sync engine does
   **not** create a fallback path.

### UI (FolderSettingsModal, Sync tab)

Two clearly-separated sections:

**Server default (applies to all clients)** — visible everywhere (web + Tauri):

- Strategy dropdown: `Inherit from parent` / `TwoWay` / `ClientToServer` /
  `ServerToClient` / `UploadOnly` / `DoNotSync`.
- "Effective" line showing the server-resolved result and its source.
- Saved via existing `PUT /api/folders/{id}`.

**This device** — Tauri only:

- Strategy dropdown: `Use server default` (NULL) / `TwoWay` / … / `DoNotSync`.
- "Effective on this device" line showing the layered resolution.
- Local folder row (only when effective strategy syncs): shows explicit path,
  inherited ancestor path ("Inherits /Music"), or "Not set" with a warning
  when no ancestor has a path.
- Saved via the two new Tauri commands, independently.

### Android SAF strategy

We commit to the **walk-into-SAF-subtree** approach: a single picked
`content://` tree URI at an ancestor folder lets the engine walk into
subdirectories using `tauri-plugin-android-fs` child-document APIs. If that
turns out to be unreliable, the fallback is "explicit SAF pick required per
synced subfolder" — we decide during task #18 based on what the plugin
actually supports.

## Task breakdown

Seven tasks, dependencies in parentheses:

1. **Journal migration** — sqlx migration: nullable `strategy`, migrate
   `"inherit"` → NULL. New `get_folder_sync_config` returning
   `(Option<strategy>, Option<local_path>)`. Independent write methods.

2. **Remove Android placeholder root** *(depends on 1)* — drop `sync-root/`
   creation in `login`/auto-login. `SyncEngine` tolerates an unset root.
   Desktop setup still requires a root path; Android doesn't.

3. **Split Tauri commands** *(depends on 1)* — replace `set_folder_strategy`
   with:
   - `set_folder_local_strategy(folder_id, Option<strategy>)`
   - `set_folder_local_path(folder_id, Option<path>)`
   - `get_folder_effective_config(folder_id) -> { client_strategy, effective_strategy, base_path, base_source }`
   Update web hooks.

4. **`LocalFs` trait** *(depends on 1, 2)* — abstract `std::fs`/`tokio::fs`
   behind a trait (`list_dir`, `read`, `write`, `mkdir`, `remove`, `mtime`).
   `NativeFs` for desktop; `AndroidSafFs` walking child documents under a
   tree URI for Android. Engine parameterized over the trait.

5. **Engine rewrite** *(depends on 4)* — replace flat `walk_local(root)` with
   a recursive walk of the server folder tree. For each folder resolve
   `(strategy, base_path)` via the rules above and sync each subtree against
   its own base. Subtrees without a resolvable base path are skipped with a
   reported error.

6. **Rebuild modal Sync tab** *(depends on 3)* — two-section UI as described
   above. Independent save handlers for server default vs. device override
   vs. local folder.

7. **End-to-end validation** *(depends on 5, 6)* — cargo build full
   workspace; build and install Android APK on `emulator-5554`; manually
   verify:
   - Fresh Android install shows no sync at all.
   - Picking a SAF folder + strategy on a subfolder persists across modal
     reopens.
   - Sync Now transfers files into the picked SAF tree (including walking
     into subdirectories if that path is chosen).
   - Desktop setup still prompts for a root path and syncs normally.
   - Server-default changes propagate to clients that haven't overridden.
   - Clearing a local override falls back correctly.

## Non-goals for this PR

- Reconciling files currently in `/data/data/de.lunarstream.uncloud/sync-root/`
  on existing installs. Those were downloaded under the old design; users
  can either leave them, delete them manually, or move them into a newly
  picked SAF directory. No migration tooling.
- Conflict UI improvements. Engine conflict semantics stay as they are.
- Server-side sync strategy inheritance changes.
