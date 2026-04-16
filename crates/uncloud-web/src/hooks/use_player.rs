use dioxus::prelude::{ReadableExt, Signal, WritableExt};
use uncloud_common::TrackResponse;
use crate::state::PlayerState;

/// Replace the play queue and start playing at `start_index`. Preserves the
/// user's shuffle and repeat preferences. If shuffle is on, the incoming
/// tracks are reshuffled so playback starts at `start_index` and the rest are
/// randomised.
pub fn play_queue(mut player: Signal<PlayerState>, tracks: Vec<TrackResponse>, start_index: usize) {
    let (shuffle, repeat) = {
        let current = player.peek();
        (current.shuffle, current.repeat)
    };

    let mut next = PlayerState {
        queue: tracks,
        current_index: start_index,
        playing: true,
        shuffle: false,
        repeat,
        original_queue: None,
    };

    if shuffle {
        // toggle_shuffle handles the reshuffle-with-current-first logic.
        next.toggle_shuffle();
    }

    player.set(next);
}
