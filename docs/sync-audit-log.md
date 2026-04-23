# Sync Audit Log

A per-user log of change-inducing operations, held on both the server and each
sync client, browsable and updated live.

## Goals

- Every mutating operation leaves an audit record with **who / what / where / when**.
- Server log is **searchable** (partial path, client ID, source) and **retention-limited**.
- Client log is **local-only**, **not searchable**, and records both directions plus
  manual sync start/end markers.
- Updates are **live** — no polling. The existing SSE infrastructure carries events.
- Logging a write never blocks or fails the user's operation: if the log insert
  errors we warn-log and move on.

## Non-goals

- No file content, diffs, or byte counts (version history already handles content).
- No cross-client aggregation on the client side — each client only sees its own log.
- No log of read-only operations (list, download, view, thumbnail).

---

## Server

### Schema

New MongoDB collection `sync_events`:

```rust
// crates/uncloud-server/src/models/sync_event.rs
pub struct SyncEvent {
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub timestamp: DateTime<Utc>,

    pub operation: SyncOperation,     // Created | Renamed | Moved | Deleted
                                      // | Restored | PermanentlyDeleted
                                      // | ContentReplaced | Copied
    pub resource_type: ResourceType,  // File | Folder
    pub resource_id: Option<ObjectId>, // set pre-hard-delete, else may be null
    pub path: String,                  // logical path at the moment of the event
    pub new_path: Option<String>,      // for Renamed / Moved / Copied

    pub source: EventSource,           // UserWeb | UserDesktop | UserMobile
                                       // | Sync | Admin | Public | System
    pub client_id: Option<String>,     // hostname / device name; None for web
    pub client_os: Option<ClientOs>,   // Linux | Windows | Macos | Android | Ios

    pub affected_count: Option<u32>,   // for recursive ops (copy/delete folder)
}
```

### Indexes

```javascript
db.sync_events.createIndex({ owner_id: 1, timestamp: -1 })
db.sync_events.createIndex({ owner_id: 1, path: 1 })         // prefix/regex search
db.sync_events.createIndex({ owner_id: 1, client_id: 1, timestamp: -1 })
db.sync_events.createIndex(
    { timestamp: 1 },
    { expireAfterSeconds: 604800 }                            // 7 days
)
```

The TTL index handles time-based retention without a cron task. A second
lightweight prune task (hourly) enforces the per-user record cap.

### Source / client detection

Sync client (and eventually the mobile app) send three headers on every mutating
request:

| Header | Values | Purpose |
|---|---|---|
| `X-Uncloud-Source` | `sync` / `user` / `admin` | Why the write is happening |
| `X-Uncloud-Client` | free-form string | Hostname or device label |
| `X-Uncloud-Os` | `linux` / `windows` / `macos` / `android` / `ios` | OS of the originator |

Server middleware extracts these into a `RequestMeta` struct attached as an
Axum request extension.

Defaults when headers are absent:

| Auth type | `source` fallback | Reasoning |
|---|---|---|
| Cookie (browser session) | `UserWeb` | Standard web UI |
| Bearer token | `UserDesktop` | Currently only the desktop/mobile apps use bearer tokens |
| Admin route | `Admin` (overrides header) | Always mark admin-originated changes clearly |

`client_id` / `client_os` simply remain `None` when headers are missing.

### Service

```
crates/uncloud-server/src/services/sync_log.rs
    pub struct SyncLog { db: Database, events: EventService }
    impl SyncLog {
        pub async fn record(&self, event: SyncEvent) -> Result<()>
        pub async fn list(&self, owner_id, filter: Filter) -> Result<Vec<SyncEvent>>
        pub async fn prune_overflow(&self, owner_id, keep: u32) -> Result<()>
    }
```

`record()` inserts the document, then calls
`events.emit_sync_event_appended(owner_id, &event).await`. It is always called
after the mutation has succeeded — never as middleware — so a failed operation
does not produce a phantom audit record.

### Emission sites

| Handler | Event(s) |
|---|---|
| `POST /api/uploads/simple` / `/complete` | `Created` |
| `POST /api/files/{id}/content` | `ContentReplaced` |
| `POST /api/files/{fid}/versions/{vid}/restore` | `ContentReplaced` (with note in `new_path` pointing to version id? — see open questions) |
| `PUT /api/files/{id}` | `Renamed` or `Moved` depending on diff |
| `POST /api/files/{id}/copy` | `Copied` |
| `DELETE /api/files/{id}` | `Deleted` |
| `POST /api/folders` | `Created` |
| `PUT /api/folders/{id}` | `Renamed` or `Moved` |
| `POST /api/folders/{id}/copy` | `Copied` (with `affected_count`) |
| `DELETE /api/folders/{id}` | `Deleted` (with `affected_count`) |
| `POST /api/trash/{id}/restore` | `Restored` |
| `DELETE /api/trash/{id}` | `PermanentlyDeleted` |
| `DELETE /api/trash` (empty) | one `PermanentlyDeleted` per resource, or one summary event (TBD — see open questions) |

Every call site uses the same helper:

```rust
state.sync_log.record(SyncEvent {
    owner_id: user.id,
    operation: SyncOperation::Renamed,
    resource_type: ResourceType::File,
    resource_id: Some(file.id),
    path: old_path,
    new_path: Some(new_path),
    source: meta.source,
    client_id: meta.client_id.clone(),
    client_os: meta.client_os,
    affected_count: None,
    timestamp: Utc::now(),
    id: ObjectId::new(),
}).await.ok();
```

Errors are swallowed with a `tracing::warn!` — the audit log must never break a
real operation.

### API

```
GET /api/sync-events
  ?q=<partial path>                (optional, matched against path & new_path)
  ?client=<partial client id>      (optional)
  ?source=sync|user|admin          (optional, multi-valued via repeat)
  ?before=<ISO-8601 timestamp>     (for pagination / "load more")
  ?limit=<int, default 100, max 500>
  -> 200 { events: [...], has_more: bool }
```

Admin-only variant, later:

```
GET /api/admin/sync-events?user_id=...&<same filters>
```

### SSE

Add to `ServerEvent` (`uncloud-common/src/api/events.rs`) and
`services::events::Event`:

```rust
SyncEventAppended { event: SyncEventResponse },
```

Emitted by `SyncLog::record()` on the owner's channel.

### Config

```yaml
sync_audit:
  enabled: true
  retention_days: 7           # TTL index TTL
  max_records_per_user: 10000 # hourly prune keeps newest N
```

All optional; `SyncAuditConfig` implements `Default`.

---

## Client (desktop / mobile sync engine)

### Schema

New SQLite table in the existing journal DB (`uncloud-sync/src/journal.rs`):

```sql
CREATE TABLE IF NOT EXISTS sync_log (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp   TEXT    NOT NULL,            -- ISO-8601
    operation   TEXT    NOT NULL,            -- Created | Renamed | Moved | Deleted
                                             -- | ContentReplaced | SyncStart | SyncEnd
    direction   TEXT,                        -- Up | Down | NULL for meta
    resource_type TEXT,                      -- File | Folder | NULL for meta
    path        TEXT    NOT NULL,            -- or a summary string for meta events
    new_path    TEXT,
    reason      TEXT    NOT NULL,            -- Sync | User | ManualSyncStart | ManualSyncEnd
    note        TEXT                         -- free-form detail (e.g. "42 files, 3 errors")
);
CREATE INDEX IF NOT EXISTS idx_sync_log_ts ON sync_log (timestamp DESC);
```

### Emission

Two call sites on the engine side:

- `SyncEngine::apply_remote_change(entry)` → logs one row with `direction=Down`
  and `reason=Sync` per non-trivial mutation (skip no-ops and reads).
- `SyncEngine::push_local_change(entry)` → logs `direction=Up, reason=Sync`.

Bracket every run of `SyncEngine::run_sync()` with a pair of meta rows:

```
SyncStart — path="manual" / "scheduled", note=""
... operations ...
SyncEnd   — path="manual",
            note="42 up, 3 down, 0 conflicts, 2.1s"
```

When the user triggers a sync from the tray, the wrapper sets `reason =
ManualSyncStart` / `ManualSyncEnd` so it reads differently in the UI.

### Retention

- Time-based: a `DELETE FROM sync_log WHERE timestamp < ?` at the end of each
  sync run, with the same 7-day default the server uses.
- Count-based: optional — if the table grows past `max_records` (default 10_000),
  delete oldest to fit.

Settings exposed via `syncaudit.retention_days` in the desktop config TOML.

### Hooking client → server headers

The sync client already uses `uncloud-client`'s reqwest wrapper. Add a
permanent request extension on the `ApiClient` constructor:

```rust
ApiClient::new(base_url)
    .with_source(Source::Sync)
    .with_client_id(hostname::get()?)
    .with_os(std::env::consts::OS);
```

Every request then includes the `X-Uncloud-*` headers. The web frontend does
nothing — absence of the headers correctly maps to `UserWeb`.

Android device name: initial cut uses `android.os.Build.MODEL` via a tiny
Tauri plugin; if the plugin isn't ready in time, fall back to
`"android-device"` + a short random suffix stored in app data.

---

## Frontend

### Web (and desktop which embeds web)

New sidebar entry under **Settings** → **Activity** (or standalone
`/activity` route — TBD, see open questions).

Layout:

```
┌──────────────────────────────────────────────────────────┐
│ [search path]  [client filter]  [source ▾]  [clear]      │
├──────────────────────────────────────────────────────────┤
│ 14:32  Renamed   photos/cat.jpg → kitten.jpg   Sync (laptop, Linux)
│ 14:30  Deleted   old-report.pdf                UserWeb
│ 14:29  Created   reports/q1.pdf                Sync (laptop, Linux)
│ ...                                                      │
│ [Load more]                                              │
└──────────────────────────────────────────────────────────┘
```

- Fetches `/api/sync-events` with filters.
- Subscribes via `use_events` to `SyncEventAppended` and prepends matching rows.
- "Load more" paginates via `?before=<oldest-shown-timestamp>`.

### Desktop-only tab

In the desktop Settings page (`components/settings.rs`), add an "Activity"
section with two toggles: **Server** (uses the same web API above) and **This
Device** (reads local SQLite via a new Tauri command `get_local_sync_log`).

The local view is identical visually but pulls from `sync_log.db` and isn't
filter-heavy — just a time-ordered list with a "clear" button.

Live updates on desktop: after every insert into `sync_log`, Tauri emits a
`sync-log-updated` event; the frontend refreshes the current page of rows.

---

## Implementation phases

Phased so each phase ships something visible.

### Phase 1 — Server foundation (no UI)

- `models/sync_event.rs`, `services/sync_log.rs`.
- Indexes + TTL migration on startup.
- `RequestMeta` extractor middleware.
- Emission wired into every mutating route.
- `GET /api/sync-events` endpoint + filters.
- Feature flag: `config.sync_audit.enabled`.

Tested via curl / integration tests before any UI lands.

### Phase 2 — Live updates

- `ServerEvent::SyncEventAppended`.
- Emission from `SyncLog::record()`.

### Phase 3 — Web UI

- New route / settings tab + components.
- `use_sync_log` hook.
- Live-append on SSE.

### Phase 4 — Client emission

- Sync client sets `X-Uncloud-*` headers.
- Local SQLite `sync_log` table + retention.
- `record_local()` helper wired into the engine's mutation paths.
- Meta rows around `run_sync()`.

### Phase 5 — Desktop UI + admin view

- Desktop "This Device" panel with local log.
- Admin `/api/admin/sync-events`.
- Count-based prune background task (if TTL isn't enough in practice).

---

## Open questions

1. **Folder copy / recursive delete:** one summary event (with
   `affected_count: N`) or N granular events? Summary is far nicer to browse
   and cheaper to store; keep the per-resource detail discoverable via version
   history. **Proposal: one summary event.**
2. **Empty-trash:** same question. **Proposal: one summary event with
   `affected_count`.**
3. **Where does "Activity" live?** Settings sub-tab is easier to build; a
   top-level sidebar entry is more visible. **Proposal: settings sub-tab for
   MVP, promote later if users use it.**
4. **Version restore:** model as `ContentReplaced` (with optional
   `source_version_id` in a future column), or invent a `VersionRestored`
   variant? **Proposal: `ContentReplaced` — simpler taxonomy.**
5. **Public-share downloads:** not mutating, but users may want to see "share
   link X was used 40 times". That's a different product question; keep it out
   of MVP.

---

## Out of scope

- CSV / JSON export of the log.
- Fuzzy or regex search beyond substring `LIKE` / MongoDB regex.
- Aggregated timeline views ("which files churned the most this week").
- Log compaction / archival to cold storage.
- Per-operation severity levels.

Each of these is a follow-up once the basic log proves useful.
