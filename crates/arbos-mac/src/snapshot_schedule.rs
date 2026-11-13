//! Catch-up paints use an 80 ms trailing edge; live changes still paint immediately
//! (#1405). Time is supplied by the room task so paused Tokio clocks keep working.

use std::time::{Duration, Instant};

const WINDOW: Duration = Duration::from_millis(80);

#[derive(Default)]
pub(crate) struct SnapshotSchedule {
    deadline: Option<Instant>,
}

impl SnapshotSchedule {
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    pub(crate) fn list_changed(&mut self, now: Instant, caught_up: bool) -> bool {
        if caught_up {
            self.deadline = None;
            true
        } else {
            self.deadline.get_or_insert_with(|| now + WINDOW);
            false
        }
    }

    pub(crate) fn timer_fired(&mut self, caught_up: bool) -> bool {
        self.deadline.take().is_some() && !caught_up
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_change_has_no_pending_paint() {
        let mut schedule = SnapshotSchedule::default();
        assert_eq!(schedule.deadline(), None);
        assert!(!schedule.timer_fired(false));
    }

    #[test]
    fn catch_up_bursts_keep_the_first_trailing_edge() {
        let start = Instant::now();
        let mut schedule = SnapshotSchedule::default();
        for ms in [0, 10, 50] {
            assert!(!schedule.list_changed(start + Duration::from_millis(ms), false));
            assert_eq!(schedule.deadline(), Some(start + Duration::from_millis(80)));
        }
        assert!(schedule.timer_fired(false));
        assert_eq!(schedule.deadline(), None);
        assert!(!schedule.list_changed(start + Duration::from_millis(120), false));
        assert_eq!(
            schedule.deadline(),
            Some(start + Duration::from_millis(200))
        );
        assert!(schedule.timer_fired(false));
    }

    #[test]
    fn live_changes_each_paint_immediately() {
        let start = Instant::now();
        let mut schedule = SnapshotSchedule::default();
        for ms in [0, 10, 50, 120] {
            assert!(schedule.list_changed(start + Duration::from_millis(ms), true));
            assert_eq!(schedule.deadline(), None);
        }
    }

    #[test]
    fn live_change_clears_the_pending_catch_up_paint() {
        let start = Instant::now();
        let mut schedule = SnapshotSchedule::default();
        assert!(!schedule.list_changed(start, false));
        assert!(schedule.list_changed(start + Duration::from_millis(50), true));
        assert_eq!(schedule.deadline(), None);
        assert!(!schedule.timer_fired(false));
    }

    #[test]
    fn link_live_paint_suppresses_a_duplicate_timer_paint() {
        let mut schedule = SnapshotSchedule::default();
        assert!(!schedule.list_changed(Instant::now(), false));
        assert!(!schedule.timer_fired(true));
        assert_eq!(schedule.deadline(), None);
        assert!(!schedule.timer_fired(false));
    }

    #[test]
    fn queued_change_does_not_postpone_an_already_due_timer() {
        let start = Instant::now();
        let mut schedule = SnapshotSchedule::default();
        assert!(!schedule.list_changed(start, false));
        assert!(!schedule.list_changed(start + Duration::from_millis(100), false));
        assert_eq!(schedule.deadline(), Some(start + Duration::from_millis(80)));
        assert!(schedule.timer_fired(false));
    }

    #[test]
    fn two_thousand_backfilled_changes_stay_coalesced() {
        let start = Instant::now();
        let mut schedule = SnapshotSchedule::default();
        let mut paints = Vec::new();
        for ms in 0..2_000 {
            let now = start + Duration::from_millis(ms);
            if let Some(deadline) = schedule.deadline().filter(|deadline| *deadline <= now) {
                assert!(schedule.timer_fired(false));
                paints.push(deadline.duration_since(start).as_millis());
            }
            assert!(!schedule.list_changed(now, false));
        }
        let deadline = schedule.deadline().unwrap();
        assert!(schedule.timer_fired(false));
        paints.push(deadline.duration_since(start).as_millis());
        assert_eq!(paints.len(), 25);
        assert_eq!(paints.first(), Some(&80));
        assert_eq!(paints.last(), Some(&2_000));
    }
}
