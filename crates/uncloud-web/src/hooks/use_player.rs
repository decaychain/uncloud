use dioxus::prelude::{Signal, WritableExt};
use uncloud_common::TrackResponse;
use crate::state::PlayerState;

pub fn play_queue(mut player: Signal<PlayerState>, tracks: Vec<TrackResponse>, start_index: usize) {
    player.set(PlayerState {
        queue: tracks,
        current_index: start_index,
        playing: true,
    });
}
