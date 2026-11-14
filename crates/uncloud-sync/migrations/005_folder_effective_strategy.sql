-- Effective sync strategy each folder had on the previous run. A folder that
-- moves from a strategy that never pushes deletes (do_not_sync, upload_only,
-- server_to_client) to one that does must not have the local absences that
-- accumulated in the meantime read as deletions.
CREATE TABLE IF NOT EXISTS folder_effective_strategy (
    folder_id TEXT PRIMARY KEY,
    strategy  TEXT NOT NULL
);
