-- Local audit log of sync activity. Both push/pull mutations and manual
-- sync run markers land here. Retention is enforced by the engine, not the
-- database — see prune_sync_log().

CREATE TABLE IF NOT EXISTS sync_log (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp     TEXT    NOT NULL,   -- ISO-8601
    operation     TEXT    NOT NULL,   -- Created | Renamed | Moved | Deleted
                                      -- | ContentReplaced | SyncStart | SyncEnd
    direction     TEXT,               -- Up | Down | NULL for meta rows
    resource_type TEXT,               -- File | Folder | NULL for meta rows
    path          TEXT    NOT NULL,
    new_path      TEXT,
    reason        TEXT    NOT NULL,   -- Sync | User | ManualSyncStart | ManualSyncEnd
    note          TEXT
);

CREATE INDEX IF NOT EXISTS idx_sync_log_ts ON sync_log (timestamp DESC);
