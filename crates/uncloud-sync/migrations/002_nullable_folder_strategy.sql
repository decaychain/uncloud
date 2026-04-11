-- Make folder_sync_config.strategy nullable (NULL = "use server default")
-- and migrate existing "inherit" rows to NULL. strategy and local_path are
-- now independently nullable and independently writable.
--
-- SQLite can't drop NOT NULL in place, so rebuild the table.

CREATE TABLE folder_sync_config_new (
    folder_id  TEXT PRIMARY KEY,
    strategy   TEXT,                 -- NULL = use server default
    local_path TEXT,                 -- NULL = inherit from ancestor
    updated_at TEXT NOT NULL
);

INSERT INTO folder_sync_config_new (folder_id, strategy, local_path, updated_at)
SELECT folder_id,
       CASE WHEN strategy = 'inherit' THEN NULL ELSE strategy END,
       local_path,
       updated_at
  FROM folder_sync_config;

DROP TABLE folder_sync_config;
ALTER TABLE folder_sync_config_new RENAME TO folder_sync_config;
