//! How prose written *to* you is set — the reading aids, as opposed to the
//! type size, which is everyone's.
//!
//! A global for the same reason the caret's blink and the base text size are:
//! the transcript paints inside a borrow of the workspace, so it cannot read
//! the workspace's own copy of the answer.

use bezel::gpui::{App, Global};

#[derive(Default)]
struct Reading {
    bionic: bool,
}

impl Global for Reading {}

/// Whether the agent's prose is weighted for bionic reading — the front of
/// each word heavier than the rest, so the eye has somewhere to land.
pub fn bionic(cx: &App) -> bool {
    cx.try_global::<Reading>()
        .is_some_and(|reading| reading.bionic)
}

pub fn set_bionic(on: bool, cx: &mut App) {
    cx.default_global::<Reading>().bionic = on;
}
