-- One row per directory the user has pointed sync at: the global desktop root
-- and every Android per-folder SAF override get their own row. The companion
-- `.uncloud-root.json` sentinel inside that directory carries the matching
-- `base_id` so the engine can detect "wrong volume mounted" or "user pointed
-- sync at a fresh folder" separately from genuine local deletions.
CREATE TABLE IF NOT EXISTS sync_bases (
    base_id     TEXT PRIMARY KEY,    -- UUID minted on first successful sync
    local_path  TEXT NOT NULL UNIQUE,
    instance_id TEXT NOT NULL,       -- shared across every base on this client install
    created_at  TEXT NOT NULL
);

-- ISO 8601 timestamp marking when the engine first noticed this row's local
-- file was missing. Phase 6.5 only commits the delete to the server on the
-- *second* scan that still sees it missing — anything else (file reappears,
-- watcher event, server side updates, strategy change) clears the column.
ALTER TABLE sync_state ADD COLUMN delete_pending_since TEXT NULL;
