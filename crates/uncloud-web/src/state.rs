use serde::{Deserialize, Serialize};
use uncloud_common::{TrackResponse, UserResponse};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Section {
    Files,
    Gallery,
    Music,
    Shopping,
    Passwords,
    Settings,
}

#[derive(Debug, Clone, Copy)]
pub struct ThemeState {
    pub dark: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthState {
    pub user: Option<UserResponse>,
    pub loading: bool,
}

impl AuthState {
    pub fn is_authenticated(&self) -> bool {
        self.user.is_some()
    }

    pub fn username(&self) -> Option<&str> {
        self.user.as_ref().map(|u| u.username.as_str())
    }

    pub fn is_admin(&self) -> bool {
        self.user
            .as_ref()
            .map(|u| u.role == uncloud_common::UserRole::Admin)
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone, Default)]
pub struct FileBrowserState {
    pub current_folder: Option<String>,
    pub selected_items: Vec<String>,
    pub view_mode: ViewMode,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum ViewMode {
    #[default]
    Grid,
    List,
}

#[derive(Clone, PartialEq, Default)]
pub struct PlayerState {
    pub queue: Vec<TrackResponse>,
    pub current_index: usize,
    pub playing: bool,
}

/// Set before navigation to make the target folder scroll-and-highlight a specific item.
/// FileBrowser reads and clears this after applying the highlight.
#[derive(Clone, Default, PartialEq)]
pub struct HighlightTarget {
    pub file_id: Option<String>,
}

/// Set before navigating to /passwords to auto-show an unlock card for a specific vault file.
/// The passwords page reads and clears this on mount.
#[derive(Clone, Default, PartialEq)]
pub struct VaultOpenTarget {
    pub file_id: Option<String>,
    pub file_name: Option<String>,
}

impl PlayerState {
    pub fn current_track(&self) -> Option<&TrackResponse> {
        self.queue.get(self.current_index)
    }
    pub fn has_prev(&self) -> bool {
        self.current_index > 0
    }
    pub fn has_next(&self) -> bool {
        self.current_index + 1 < self.queue.len()
    }
}
