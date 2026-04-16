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

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum RepeatMode {
    #[default]
    Off,
    All,
    One,
}

impl RepeatMode {
    pub fn cycle(self) -> Self {
        match self {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        }
    }
}

#[derive(Clone, PartialEq, Default)]
pub struct PlayerState {
    pub queue: Vec<TrackResponse>,
    pub current_index: usize,
    pub playing: bool,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    /// When shuffle is on, holds the pre-shuffle queue so we can restore order
    /// when shuffle is turned off.
    pub original_queue: Option<Vec<TrackResponse>>,
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

    /// With repeat All or shuffle (and queue > 1), there's always a "next".
    pub fn has_next(&self) -> bool {
        if self.queue.is_empty() {
            return false;
        }
        if self.repeat == RepeatMode::All || self.shuffle && self.queue.len() > 1 {
            return true;
        }
        self.current_index + 1 < self.queue.len()
    }

    /// With repeat All or shuffle, "previous" is always available if queue > 1.
    pub fn has_prev(&self) -> bool {
        if self.queue.is_empty() {
            return false;
        }
        if self.repeat == RepeatMode::All || self.shuffle && self.queue.len() > 1 {
            return true;
        }
        self.current_index > 0
    }

    /// Picks the next track's index based on shuffle/repeat state.
    /// Used by both auto-advance (on track end) and the skip-forward button.
    /// `respect_repeat_one` is true only for auto-advance; skipping always moves on.
    pub fn next_index(&self, respect_repeat_one: bool) -> Option<usize> {
        if self.queue.is_empty() {
            return None;
        }
        if respect_repeat_one && self.repeat == RepeatMode::One {
            return Some(self.current_index);
        }
        if self.shuffle && self.queue.len() > 1 {
            return Some(pick_shuffle_index(self.queue.len(), self.current_index));
        }
        if self.current_index + 1 < self.queue.len() {
            return Some(self.current_index + 1);
        }
        if self.repeat == RepeatMode::All {
            return Some(0);
        }
        None
    }

    /// Picks the previous track's index. Mirrors `next_index` semantics.
    pub fn prev_index(&self) -> Option<usize> {
        if self.queue.is_empty() {
            return None;
        }
        if self.shuffle && self.queue.len() > 1 {
            return Some(pick_shuffle_index(self.queue.len(), self.current_index));
        }
        if self.current_index > 0 {
            return Some(self.current_index - 1);
        }
        if self.repeat == RepeatMode::All {
            return Some(self.queue.len().saturating_sub(1));
        }
        None
    }

    /// Toggle shuffle. Preserves the current track's position at index 0 of
    /// the reshuffled queue so the currently-playing song isn't interrupted.
    pub fn toggle_shuffle(&mut self) {
        if self.shuffle {
            // Turn OFF: restore the original order, move current_index to match
            // the currently playing track.
            if let Some(original) = self.original_queue.take() {
                let current_id = self
                    .queue
                    .get(self.current_index)
                    .map(|t| t.file.id.clone());
                self.queue = original;
                if let Some(cid) = current_id {
                    if let Some(pos) = self.queue.iter().position(|t| t.file.id == cid) {
                        self.current_index = pos;
                    }
                }
            }
            self.shuffle = false;
        } else {
            // Turn ON: save current order, shuffle the queue with current track first.
            if self.queue.len() <= 1 {
                self.shuffle = true;
                return;
            }
            self.original_queue = Some(self.queue.clone());
            let current = self.queue.remove(self.current_index);
            fisher_yates_shuffle(&mut self.queue);
            self.queue.insert(0, current);
            self.current_index = 0;
            self.shuffle = true;
        }
    }
}

/// Random index in [0, len) that is not `current`. Caller must ensure `len > 1`.
fn pick_shuffle_index(len: usize, current: usize) -> usize {
    let mut attempt = 0u32;
    loop {
        let r = (js_sys::Math::random() * len as f64).floor() as usize;
        let r = r.min(len - 1);
        if r != current || attempt > 8 {
            return r;
        }
        attempt += 1;
    }
}

fn fisher_yates_shuffle<T>(v: &mut [T]) {
    let len = v.len();
    if len < 2 {
        return;
    }
    for i in (1..len).rev() {
        let j = (js_sys::Math::random() * (i as f64 + 1.0)).floor() as usize;
        let j = j.min(i);
        v.swap(i, j);
    }
}
