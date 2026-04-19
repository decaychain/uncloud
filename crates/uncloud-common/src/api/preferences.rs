use serde::{Deserialize, Serialize};

/// Per-user preferences persisted on the server. Added to `UserResponse` so the
/// client receives them on login / `me` without an extra round-trip.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct UserPreferences {
    /// Ordered list of enabled dashboard tile IDs (e.g. `["files", "gallery", "tasks"]`).
    /// When empty the client applies its own defaults — this lets us evolve defaults
    /// without migrating stored docs.
    #[serde(default)]
    pub dashboard_tiles: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdatePreferencesRequest {
    pub dashboard_tiles: Option<Vec<String>>,
}
