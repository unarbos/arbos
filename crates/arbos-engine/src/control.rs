//! Turn control as signals, not polls.
//!
//! One handle per running turn. The kernel holds a clone and flips it from
//! the attach loop; the turn, the provider stream, and every running tool
//! observe it. Stop is a [`CancellationToken`], so a stop lands mid-stream,
//! mid-bash, or mid-question — not at the next loop iteration.

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
pub struct TurnControl(Arc<Inner>);

#[derive(Default)]
struct Inner {
    cancel: CancellationToken,
    steer: Mutex<VecDeque<String>>,
    compact: AtomicBool,
    /// Why the turn was stopped, for the `interrupted` line. None = "stop".
    reason: Mutex<Option<String>>,
}

impl TurnControl {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) -> &CancellationToken {
        &self.0.cancel
    }

    pub fn stop(&self) {
        self.0.cancel.cancel();
    }

    /// Stop with a reason the transcript records, e.g. `kernel stopping`.
    pub fn stop_for(&self, reason: &str) {
        *self.0.reason.lock().unwrap() = Some(reason.to_string());
        self.0.cancel.cancel();
    }

    pub fn is_stopped(&self) -> bool {
        self.0.cancel.is_cancelled()
    }

    /// The `interrupted` detail for this stop: the reason given, else `stop`.
    pub fn stop_reason(&self) -> String {
        self.0
            .reason
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| "stop".into())
    }

    /// The user spoke while the turn was running.
    pub fn steer(&self, text: String) {
        self.0.steer.lock().unwrap().push_back(text);
    }

    pub fn steer_pending(&self) -> bool {
        !self.0.steer.lock().unwrap().is_empty()
    }

    pub fn take_steer(&self) -> Option<String> {
        self.0.steer.lock().unwrap().pop_front()
    }

    /// The user asked for a compaction. Honoured before the next model call.
    pub fn request_compact(&self) {
        self.0.compact.store(true, Ordering::SeqCst);
    }

    pub fn take_compact(&self) -> bool {
        self.0.compact.swap(false, Ordering::SeqCst)
    }
}
