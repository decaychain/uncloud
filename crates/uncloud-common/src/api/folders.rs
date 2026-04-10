use serde::{Deserialize, Serialize};

use super::files::FileResponse;

/// Per-folder gallery inclusion setting, inherited down the tree.
/// Root default is `Exclude` (opt-in).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GalleryInclude {
    /// Use parent's setting; root default is `Exclude`.
    #[default]
    Inherit,
    /// Images in this folder and subfolders appear in the Gallery.
    Include,
    /// Excluded from the Gallery.
    Exclude,
}

/// Per-folder music library inclusion setting, inherited down the tree.
/// Root default is `Exclude` (opt-in).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MusicInclude {
    #[default]
    Inherit,
    Include,
    Exclude,
}

/// Per-folder sync strategy stored server-side and inherited down the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SyncStrategy {
    /// Use parent's strategy; root default is `TwoWay`.
    #[default]
    Inherit,
    /// Changes and deletions flow both directions.
    TwoWay,
    /// Local mirrors to server including deletions.
    ClientToServer,
    /// Read-only local copy; server is authoritative.
    ServerToClient,
    /// Upload new/modified; local deletions don't touch server (phone gallery mode).
    UploadOnly,
    /// Excluded from sync entirely.
    DoNotSync,
}

/// Implemented by every folder setting that can be inherited from a parent folder.
/// The enums themselves are unchanged — no serde or API surface changes.
pub trait InheritableSetting: Copy + PartialEq + Default {
    /// True when this value means "use the parent's setting".
    fn is_inherit(&self) -> bool;
    /// The value used when the root folder has `Inherit` (system default).
    fn root_default() -> Self;
    /// For binary Include/Exclude settings: `Some(true)` = include,
    /// `Some(false)` = exclude, `None` = inherit.
    /// Non-binary settings (e.g. `SyncStrategy`) leave this as the default `None`.
    fn as_include_flag(&self) -> Option<bool> {
        None
    }
}

impl InheritableSetting for SyncStrategy {
    fn is_inherit(&self) -> bool { *self == Self::Inherit }
    fn root_default() -> Self { Self::DoNotSync }
}

impl InheritableSetting for GalleryInclude {
    fn is_inherit(&self) -> bool { *self == Self::Inherit }
    fn root_default() -> Self { Self::Exclude }
    fn as_include_flag(&self) -> Option<bool> {
        match self {
            Self::Include => Some(true),
            Self::Exclude => Some(false),
            Self::Inherit => None,
        }
    }
}

impl InheritableSetting for MusicInclude {
    fn is_inherit(&self) -> bool { *self == Self::Inherit }
    fn root_default() -> Self { Self::Exclude }
    fn as_include_flag(&self) -> Option<bool> {
        match self {
            Self::Include => Some(true),
            Self::Exclude => Some(false),
            Self::Inherit => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderResponse {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// The strategy set directly on this folder.
    pub sync_strategy: SyncStrategy,
    /// The resolved strategy (walking up the tree; root `Inherit` → `TwoWay`).
    pub effective_strategy: SyncStrategy,
    /// The gallery inclusion setting on this folder.
    #[serde(default)]
    pub gallery_include: GalleryInclude,
    /// The resolved gallery inclusion (walking up the tree; root `Inherit` → `Exclude`).
    #[serde(default)]
    pub effective_gallery_include: GalleryInclude,
    /// The music library inclusion setting on this folder.
    #[serde(default)]
    pub music_include: MusicInclude,
    /// The resolved music inclusion (walking up the tree; root `Inherit` → `Exclude`).
    #[serde(default)]
    pub effective_music_include: MusicInclude,
    /// Username of the owner when this folder is being viewed via a share.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_by: Option<String>,
    /// Number of users this folder is shared with (only set for the owner).
    #[serde(default)]
    pub shared_with_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateFolderRequest {
    pub name: String,
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateFolderRequest {
    pub name: Option<String>,
    pub parent_id: Option<String>,
    pub sync_strategy: Option<SyncStrategy>,
    pub gallery_include: Option<GalleryInclude>,
    pub music_include: Option<MusicInclude>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopyFolderRequest {
    /// Destination folder ID. None = same parent as source; empty string = root.
    pub parent_id: Option<String>,
    /// New folder name. None = "Copy of {original}".
    pub name: Option<String>,
}

/// Response for `GET /api/folders/{id}/effective-strategy`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectiveStrategyResponse {
    pub strategy: SyncStrategy,
    /// ID of the folder where the strategy is explicitly set; `None` = system default.
    pub source_folder_id: Option<String>,
}

/// Flat tree of all files and folders under a root, used by the sync engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncTreeResponse {
    pub files: Vec<FileResponse>,
    pub folders: Vec<FolderResponse>,
}
