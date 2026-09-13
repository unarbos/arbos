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

    pub fn is_stopped(&self) -> bool {
        self.0.cancel.is_cancelled()
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

    /// Every steer waiting, oldest first. A turn boundary takes them all:
    /// taking one per step lost the rest when steers arrived faster than
    /// steps (qa-014).
    pub fn take_steers(&self) -> Vec<String> {
        self.0.steer.lock().unwrap().drain(..).collect()
    }

    /// The user asked for a compaction. Honoured before the next model call.
    pub fn request_compact(&self) {
        self.0.compact.store(true, Ordering::SeqCst);
    }

    pub fn take_compact(&self) -> bool {
        self.0.compact.swap(false, Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_boundary_takes_every_waiting_steer_in_order() {
        let c = TurnControl::new();
        for i in 0..25 {
            c.steer(format!("STEER-{i:02}"));
        }
        let all = c.take_steers();
        assert_eq!(all.len(), 25);
        assert_eq!(all.first().map(String::as_str), Some("STEER-00"));
        assert_eq!(all.last().map(String::as_str), Some("STEER-24"));
        assert!(all.windows(2).all(|w| w[0] < w[1]), "oldest first");
        assert!(c.take_steers().is_empty());
        assert!(!c.steer_pending());
    }
}
