//! "Sign in with GitHub" on the first-launch screen: the state behind the pill.
//!
//! Pure, no GPUI, no AppKit: `app.rs` feeds it the clicks, the events from
//! `auth::begin` and the clock; `ui/shell.rs` reads `label()` / `actions()`.
//! Every attempt gets a number so a callback from an attempt the user already
//! cancelled or retried cannot touch the current one.

use crate::sync::user_error::UserError;

/// The soft timeout: after this long with no answer the pill offers Retry /
/// Open in browser. The system sheet (if it did open) stays alive — a slow
/// GitHub login (password + 2FA) is not cut off; its callback still lands.
pub const SOFT_TIMEOUT_MS: u64 = 60_000;

/// Where the attempt stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing in flight: "Sign in with GitHub".
    Idle,
    /// Probing the relay and opening the system sheet.
    Opening,
    /// The sheet is up (`in_browser: false`) or the default browser has the
    /// GitHub page (`true`); the relay's `arbos://auth/callback` is awaited.
    Waiting { in_browser: bool },
    /// The grant code is being exchanged at `POST /auth/token`.
    Finishing,
    /// `SOFT_TIMEOUT_MS` passed with no answer; the sheet may still be alive.
    TimedOut { in_browser: bool },
}

/// What the screen offers besides the pill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Cancel,
    Retry,
    OpenInBrowser,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flow {
    pub step: Step,
    /// Attempt number of the current step; 0 before the first click.
    pub attempt: u64,
    /// `now_ms` when the current attempt started.
    pub started_at: u64,
    pub error: Option<UserError>,
}

impl Default for Flow {
    fn default() -> Self {
        Self {
            step: Step::Idle,
            attempt: 0,
            started_at: 0,
            error: None,
        }
    }
}

impl Flow {
    pub fn busy(&self) -> bool {
        !matches!(self.step, Step::Idle)
    }

    /// Click on "Sign in with GitHub" / Retry: a new attempt, or `None` while
    /// one is already opening, waiting or finishing.
    pub fn start(&mut self, now_ms: u64) -> Option<u64> {
        match self.step {
            Step::Idle | Step::TimedOut { .. } => {
                self.attempt += 1;
                self.step = Step::Opening;
                self.started_at = now_ms;
                self.error = None;
                Some(self.attempt)
            }
            Step::Opening | Step::Waiting { .. } | Step::Finishing => None,
        }
    }

    /// `auth::begin` reports that the sheet (or the browser) is open.
    pub fn opened(&mut self, attempt: u64, in_browser: bool) {
        if attempt != self.attempt {
            return;
        }
        match self.step {
            Step::Opening | Step::Waiting { .. } => self.step = Step::Waiting { in_browser },
            // A late "opened" after the soft timeout keeps the recovery offer
            // but remembers where the page went.
            Step::TimedOut { .. } => self.step = Step::TimedOut { in_browser },
            Step::Idle | Step::Finishing => {}
        }
    }

    /// The callback carried a grant code: exchanging it.
    pub fn grant(&mut self, attempt: u64) -> bool {
        if attempt != self.attempt || matches!(self.step, Step::Idle | Step::Finishing) {
            return false;
        }
        self.step = Step::Finishing;
        true
    }

    /// The attempt failed (relay error, GitHub denied, exchange failed).
    pub fn failed(&mut self, attempt: u64, error: UserError) {
        if attempt != self.attempt {
            return;
        }
        self.step = Step::Idle;
        self.error = Some(error);
    }

    /// The user cancelled (pill Cancel, or the sheet's own Cancel): back to
    /// the start, no red text.
    pub fn cancelled(&mut self, attempt: u64) {
        if attempt != self.attempt {
            return;
        }
        self.step = Step::Idle;
        self.error = None;
    }

    /// Clock tick: flips Opening / Waiting to TimedOut once `SOFT_TIMEOUT_MS`
    /// has passed. Returns true when the step changed.
    pub fn tick(&mut self, attempt: u64, now_ms: u64) -> bool {
        if attempt != self.attempt || now_ms.saturating_sub(self.started_at) < SOFT_TIMEOUT_MS {
            return false;
        }
        match self.step {
            Step::Opening => {
                self.step = Step::TimedOut { in_browser: false };
                true
            }
            Step::Waiting { in_browser } => {
                self.step = Step::TimedOut { in_browser };
                true
            }
            Step::Idle | Step::Finishing | Step::TimedOut { .. } => false,
        }
    }

    /// "Open in browser" on the pill: the current attempt continues in the
    /// default browser (the sheet, if any, is cancelled by the caller).
    pub fn continue_in_browser(&mut self, now_ms: u64) -> Option<u64> {
        match self.step {
            Step::Idle | Step::Finishing => None,
            Step::Opening | Step::Waiting { .. } | Step::TimedOut { .. } => {
                self.attempt += 1;
                self.step = Step::Waiting { in_browser: true };
                self.started_at = now_ms;
                self.error = None;
                Some(self.attempt)
            }
        }
    }

    /// Pill copy.
    pub fn label(&self) -> &'static str {
        match self.step {
            Step::Idle => "Sign in with GitHub",
            Step::Opening => "Opening GitHub sign-in…",
            Step::Waiting { in_browser: false } => "Waiting for GitHub…",
            Step::Waiting { in_browser: true } => "Finish signing in in your browser…",
            Step::Finishing => "Finishing sign-in…",
            Step::TimedOut { .. } => "GitHub did not answer yet.",
        }
    }

    /// One line under the pill (not an error).
    pub fn hint(&self) -> Option<&'static str> {
        match self.step {
            Step::Waiting { in_browser: true } => {
                Some("The GitHub page opened in your browser. Come back here when it is done.")
            }
            Step::TimedOut { in_browser: false } => {
                Some("No GitHub window? Try again, or sign in from your browser. If GitHub is open, just finish there.")
            }
            Step::TimedOut { in_browser: true } => {
                Some("Not back from the browser yet. Try again, or reopen the GitHub page.")
            }
            Step::Idle | Step::Opening | Step::Waiting { in_browser: false } | Step::Finishing => {
                None
            }
        }
    }

    /// Buttons next to the pill.
    pub fn actions(&self) -> &'static [Action] {
        match self.step {
            Step::Idle | Step::Finishing => &[],
            Step::Opening | Step::Waiting { .. } => &[Action::Cancel],
            Step::TimedOut { .. } => &[Action::Retry, Action::OpenInBrowser],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_opens_then_waits_then_finishes() {
        let mut f = Flow::default();
        assert_eq!(f.label(), "Sign in with GitHub");
        let a = f.start(1_000).unwrap();
        assert_eq!(a, 1);
        assert_eq!(f.step, Step::Opening);
        assert_eq!(f.label(), "Opening GitHub sign-in…");
        assert_eq!(f.actions(), &[Action::Cancel]);
        f.opened(a, false);
        assert_eq!(f.label(), "Waiting for GitHub…");
        assert!(f.grant(a));
        assert_eq!(f.label(), "Finishing sign-in…");
        assert!(f.actions().is_empty());
    }

    #[test]
    fn second_click_while_busy_is_ignored() {
        let mut f = Flow::default();
        assert!(f.start(0).is_some());
        assert!(f.start(10).is_none());
        f.opened(1, false);
        assert!(f.start(20).is_none());
        assert_eq!(f.attempt, 1);
    }

    #[test]
    fn soft_timeout_offers_retry_and_browser_but_keeps_the_callback_alive() {
        let mut f = Flow::default();
        let a = f.start(0).unwrap();
        f.opened(a, false);
        assert!(!f.tick(a, SOFT_TIMEOUT_MS - 1));
        assert_eq!(f.step, Step::Waiting { in_browser: false });
        assert!(f.tick(a, SOFT_TIMEOUT_MS));
        assert_eq!(f.step, Step::TimedOut { in_browser: false });
        assert_eq!(f.label(), "GitHub did not answer yet.");
        assert_eq!(f.actions(), &[Action::Retry, Action::OpenInBrowser]);
        // The sheet was slow, not dead: its grant still lands.
        assert!(f.grant(a));
        assert_eq!(f.step, Step::Finishing);
    }

    #[test]
    fn timeout_while_still_opening() {
        let mut f = Flow::default();
        let a = f.start(0).unwrap();
        assert!(f.tick(a, SOFT_TIMEOUT_MS + 5));
        assert_eq!(f.step, Step::TimedOut { in_browser: false });
        assert!(f.hint().unwrap().contains("No GitHub window"));
    }

    #[test]
    fn retry_after_timeout_is_a_new_attempt_and_old_events_are_ignored() {
        let mut f = Flow::default();
        let a1 = f.start(0).unwrap();
        f.tick(a1, SOFT_TIMEOUT_MS);
        let a2 = f.start(SOFT_TIMEOUT_MS + 1).unwrap();
        assert_eq!(a2, 2);
        assert_eq!(f.step, Step::Opening);
        // Stale attempt: cancelled / failed / grant / opened all no-ops.
        f.cancelled(a1);
        f.failed(a1, UserError::from_text("sign in", "old"));
        f.opened(a1, true);
        assert!(!f.grant(a1));
        assert_eq!(f.step, Step::Opening);
        assert!(f.error.is_none());
        // The old timer firing does not flip the new attempt either.
        assert!(!f.tick(a1, SOFT_TIMEOUT_MS * 3));
        assert_eq!(f.step, Step::Opening);
    }

    #[test]
    fn cancel_returns_to_idle_without_error() {
        let mut f = Flow::default();
        let a = f.start(0).unwrap();
        f.opened(a, false);
        f.cancelled(a);
        assert_eq!(f.step, Step::Idle);
        assert!(f.error.is_none());
        assert!(!f.busy());
    }

    #[test]
    fn failure_returns_to_idle_with_the_message() {
        let mut f = Flow::default();
        let a = f.start(0).unwrap();
        f.failed(a, UserError::from_text("sign in", "relay answered 503"));
        assert_eq!(f.step, Step::Idle);
        assert_eq!(
            f.error.as_ref().map(|e| e.raw.as_str()),
            Some("relay answered 503")
        );
        // A new click clears it.
        f.start(1).unwrap();
        assert!(f.error.is_none());
    }

    #[test]
    fn open_in_browser_continues_in_the_browser() {
        let mut f = Flow::default();
        let a = f.start(0).unwrap();
        f.tick(a, SOFT_TIMEOUT_MS);
        let b = f.continue_in_browser(SOFT_TIMEOUT_MS).unwrap();
        assert_eq!(b, 2);
        assert_eq!(f.started_at, SOFT_TIMEOUT_MS);
        assert_eq!(f.step, Step::Waiting { in_browser: true });
        assert_eq!(f.label(), "Finish signing in in your browser…");
        assert_eq!(f.actions(), &[Action::Cancel]);
        // The old sheet's cancel (from session.cancel()) is a stale event.
        f.cancelled(a);
        assert_eq!(f.step, Step::Waiting { in_browser: true });
        // The browser leg times out with its own copy.
        assert!(f.tick(b, SOFT_TIMEOUT_MS * 2));
        assert_eq!(f.step, Step::TimedOut { in_browser: true });
        assert!(f.hint().unwrap().contains("Not back from the browser"));
    }

    #[test]
    fn no_window_at_all_reports_the_browser_fallback() {
        let mut f = Flow::default();
        let a = f.start(0).unwrap();
        f.opened(a, true);
        assert_eq!(f.label(), "Finish signing in in your browser…");
        assert!(f.hint().unwrap().contains("opened in your browser"));
    }

    #[test]
    fn continue_in_browser_is_a_no_op_when_idle_or_finishing() {
        let mut f = Flow::default();
        assert!(f.continue_in_browser(0).is_none());
        let a = f.start(0).unwrap();
        f.grant(a);
        assert!(f.continue_in_browser(1).is_none());
        assert_eq!(f.step, Step::Finishing);
    }
}
