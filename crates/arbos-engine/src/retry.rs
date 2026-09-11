//! Riding through provider failures.
//!
//! For each model in `[primary, fallbacks…]`: retry a transient failure up to
//! `max_attempts` times with exponential backoff and equal jitter, honouring a
//! server `Retry-After`; then move to the next model for the rest of the
//! turn. A permanent failure (bad key, bad request) stops at once — it would
//! fail identically everywhere. A failure after assistant text has already
//! reached the window is terminal too: a retry would append a second answer
//! under the first. Thinking-only output does not count; nothing durable was
//! shown.
//!
//! A server hint longer than `max_server_delay` is not waited on silently:
//! with a fallback available we switch, otherwise we fail with the hint in the
//! message so the user knows why.

use std::time::Duration;

use crate::provider::{FailKind, ProviderError};

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Calls per model, including the first.
    pub max_attempts: u32,
    pub base: Duration,
    pub max_backoff: Duration,
    /// Longest server-requested wait we will honour.
    pub max_server_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            base: Duration::from_secs(1),
            max_backoff: Duration::from_secs(30),
            max_server_delay: Duration::from_secs(60),
        }
    }
}

/// What to do after a failed attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Same model, after a wait.
    Retry,
    /// Next model now, without more attempts on this one.
    Fallback,
    /// Stop. Nothing will help.
    Fail,
}

pub fn verdict(
    e: &ProviderError,
    attempt: u32,
    policy: &RetryPolicy,
    more_models: bool,
) -> Verdict {
    if e.visible {
        return Verdict::Fail;
    }
    let transient = match e.should_retry {
        Some(explicit) => explicit,
        None => match (e.kind, e.status) {
            (FailKind::Transport | FailKind::Idle, _) => true,
            (FailKind::Stream, _) => false,
            (FailKind::Status, Some(s)) => matches!(s, 408 | 409 | 425 | 429) || s >= 500,
            (FailKind::Status, None) => true,
        },
    };
    // A model the endpoint does not serve: no point retrying it.
    if e.status == Some(404) {
        return if more_models {
            Verdict::Fallback
        } else {
            Verdict::Fail
        };
    }
    if !transient {
        return Verdict::Fail;
    }
    if let Some(hint) = e.retry_after {
        if hint > policy.max_server_delay {
            return if more_models {
                Verdict::Fallback
            } else {
                Verdict::Fail
            };
        }
    }
    if attempt >= policy.max_attempts {
        return if more_models {
            Verdict::Fallback
        } else {
            Verdict::Fail
        };
    }
    Verdict::Retry
}

/// How long to wait before attempt `attempt + 1`. With a server hint: the
/// hint plus up to 25% so sessions sharing one hint desync. Otherwise
/// exponential from `base`, capped, with equal jitter (half fixed, half random).
pub fn delay(policy: &RetryPolicy, attempt: u32, hint: Option<Duration>) -> Duration {
    if let Some(h) = hint {
        let spread = h.as_millis() as u64 / 4;
        return h + Duration::from_millis(fastrand::u64(0..=spread));
    }
    let mut d = policy.base;
    for _ in 1..attempt.max(1) {
        if d >= policy.max_backoff {
            break;
        }
        d *= 2;
    }
    let d = d.min(policy.max_backoff);
    let half = d.as_millis() as u64 / 2;
    Duration::from_millis(half + fastrand::u64(0..=half))
}

/// The model in use for this turn. A switch is sticky until the turn ends;
/// the next turn tries the primary again.
#[derive(Debug, Clone)]
pub struct Models {
    list: Vec<String>,
    current: usize,
}

impl Models {
    pub fn new(primary: String, fallbacks: &[String]) -> Self {
        let mut list = vec![primary];
        for f in fallbacks {
            if !f.trim().is_empty() && !list.contains(f) {
                list.push(f.trim().to_string());
            }
        }
        Self { list, current: 0 }
    }

    pub fn current(&self) -> &str {
        &self.list[self.current]
    }

    pub fn has_next(&self) -> bool {
        self.current + 1 < self.list.len()
    }

    /// Advance. Returns the new model.
    pub fn next(&mut self) -> Option<&str> {
        if !self.has_next() {
            return None;
        }
        self.current += 1;
        Some(self.current())
    }
}

pub fn human(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s < 10.0 {
        format!("{s:.1}s")
    } else {
        format!("{}s", s.round() as u64)
    }
}
