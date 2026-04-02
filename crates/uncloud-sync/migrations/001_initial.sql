-- Sync journal: one row per server-known file or folder
CREATE TABLE IF NOT EXISTS sync_state (
    server_id         TEXT    NOT NULL,
    item_type         TEXT    NOT NULL CHECK(item_type IN ('file', 'folder')),
    server_path       TEXT    NOT NULL,  -- logical: "Photos/2024/cat.jpg"
    local_path        TEXT    NOT NULL,  -- absolute local filesystem path
    size_bytes        INTEGER,
    checksum          TEXT,              -- sha256 hex, files only
    server_updated_at TEXT    NOT NULL,  -- ISO 8601
    local_mtime       INTEGER,           -- Unix timestamp seconds
    last_synced_at    TEXT    NOT NULL,
    sync_status       TEXT    NOT NULL DEFAULT 'synced',
    -- 'synced' | 'pending_upload' | 'pending_download' | 'conflict'
    PRIMARY KEY (server_id, item_type)
);

-- Per-folder strategy overrides (client-side, overrides server setting)
CREATE TABLE IF NOT EXISTS folder_sync_config (
    folder_id  TEXT PRIMARY KEY,
    strategy   TEXT NOT NULL,   -- SyncStrategy serde name (e.g. "two_way")
    local_path TEXT,            -- optional custom local dir (mobile use case)
    updated_at TEXT NOT NULL
);

-- Key-value store for global config
CREATE TABLE IF NOT EXISTS sync_config (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
-- Keys used: server_url, last_full_sync_at, root_local_path, poll_interval_secs
