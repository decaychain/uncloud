use bson::serde_helpers::chrono_datetime_as_bson_datetime;
use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

/// Coordinates the offline `uncloud-server backup` subcommand against a
/// concurrently running server, and against migrations. At most one row may
/// exist at a time, enforced by a unique index on `scope` (always `"global"`).
/// On startup the server refuses to run if it sees a row here; backup and
/// migrate refuse to start a new run if one is already present (use
/// `--force-unlock` after a crash).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupLock {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    /// Always `"global"`. Indexed unique → enforces singleton.
    pub scope: String,
    /// Free-form description of the run, surfaced in interlock errors.
    /// Examples: `"create:nas"`, `"restore:b2"`, `"prune:b2"`.
    pub operation: String,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub started_at: DateTime<Utc>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub last_heartbeat: DateTime<Utc>,
    pub pid: u32,
    pub hostname: String,
}

impl BackupLock {
    pub const SCOPE: &'static str = "global";
}
