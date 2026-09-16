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
            (FailKind::Transport | FailKind::Idle | FailKind::Silent, _) => true,
            (FailKind::Stream, _) => false,
            (FailKind::Status, Some(s)) => matches!(s, 408 | 409 | 425 | 429) || s >= 500,
            (FailKind::Status, None) => true,
        },
    };
    // Nothing came back at all: another model answers now rather than a
    // second wait of the same length; alone, one more try.
    if e.kind == FailKind::Silent {
        return if more_models {
            Verdict::Fallback
        } else if attempt < 2 {
            Verdict::Retry
        } else {
            Verdict::Fail
        };
    }
    // A model the endpoint does not serve: no point retrying it.
    if e.status == Some(404) {
        return if more_models {
            Verdict::Fallback
        } else {
            Verdict::Fail
        };
    }
    if !transient {
        // A bad key or an empty account: the same everywhere.
        if matches!(e.status, Some(401) | Some(402)) {
            return Verdict::Fail;
        }
        // 403 is a refusal of this request, and on a router that is
        // usually one provider family ("this user has been blocked" for
        // every `openai/*` while `anthropic/*` answers): another model
        // may well be allowed. A key refused everywhere fails at the end
        // of the list, a few fast calls later.
        if e.status == Some(403) {
            return if more_models {
                Verdict::Fallback
            } else {
                Verdict::Fail
            };
        }
        // A provider's own failure — an error frame mid-stream, a 400 the
        // model's backend produced ("Corrupted thought signature") — is
        // this model's, not the request's. Another model is another
        // request; if it fails the same way, that is the end.
        return if more_models {
            Verdict::Fallback
        } else {
            Verdict::Fail
        };
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

/// Fallbacks a turn gets on OpenRouter when config.toml names none: one
/// key, every model, so a second opinion costs nothing to set up. One
/// per provider family; `openrouter_fallbacks` orders them so the
/// primary's own family comes first and a family the key cannot call
/// (an account blocked for `openai/*` returns 403 on every one of them,
/// 2026-09-16) is only ever one of several.
pub const OPENROUTER_FALLBACKS: &[&str] = &[
    "anthropic/claude-opus-5",
    "google/gemini-3.8-flash",
    "openai/gpt-5.6-terra",
];

/// The provider family of an OpenRouter model id: `anthropic` of
/// `anthropic/claude-opus-5`. Empty for an id with no slash.
pub fn family(model: &str) -> &str {
    model.split_once('/').map(|(f, _)| f).unwrap_or("")
}

/// The default fallbacks for `primary`, in the order to try them: the
/// primary's own family first (the same account access, the same
/// tokenizer habits), then the rest in the constant's order, the primary
/// itself left out.
pub fn openrouter_fallbacks(primary: &str) -> Vec<String> {
    let fam = family(primary);
    let mut out: Vec<String> = Vec::new();
    for m in OPENROUTER_FALLBACKS {
        if *m != primary && !fam.is_empty() && family(m) == fam {
            out.push(m.to_string());
        }
    }
    for m in OPENROUTER_FALLBACKS {
        if *m != primary && !out.iter().any(|x| x == m) {
            out.push(m.to_string());
        }
    }
    out
}

impl Models {
    pub fn new(primary: String, fallbacks: &[String]) -> Self {
        let mut list = vec![primary];
        for f in fallbacks {
            let f = f.trim();
            if !f.is_empty() && f != "none" && !list.iter().any(|x| x == f) {
                list.push(f.to_string());
            }
        }
        Self { list, current: 0 }
    }

    /// `new`, plus the OpenRouter defaults when `fallbacks` is empty and
    /// `base` is openrouter.ai. `fallback_models = ["none"]` forbids them.
    pub fn with_defaults(primary: String, fallbacks: &[String], base: &str) -> Self {
        if fallbacks.is_empty() && base.contains("openrouter.ai") {
            let defaults = openrouter_fallbacks(&primary);
            return Self::new(primary, &defaults);
        }
        Self::new(primary, fallbacks)
    }

    pub fn current(&self) -> &str {
        &self.list[self.current]
    }

    /// Drop every fallback (never the primary) that `blocked` says the
    /// key cannot call, so a refused family is not tried turn after turn.
    pub fn drop_blocked(&mut self, base: &str) {
        let primary = self.list[0].clone();
        self.list
            .retain(|m| *m == primary || !crate::blocked::is_blocked(base, m));
        self.current = self.current.min(self.list.len() - 1);
    }

    /// The primary and every fallback, in order.
    pub fn all(&self) -> &[String] {
        &self.list
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

#[cfg(test)]
mod fallback_tests {
    use super::*;

    fn err(status: u16) -> ProviderError {
        ProviderError {
            kind: FailKind::Status,
            status: Some(status),
            message: String::new(),
            retry_after: None,
            should_retry: None,
            visible: false,
            partial: String::new(),
        }
    }

    /// 2026-09-16: an OpenRouter key blocked for `openai/*` (403 on every
    /// one, `anthropic/*` and `google/*` fine) turned a transient failure
    /// of the primary into a hard stop, because the first default fallback
    /// was `openai/gpt-5.6-terra` and 403 ended the turn.
    #[test]
    fn a_403_falls_through_to_the_next_model_and_a_bad_key_does_not() {
        let policy = RetryPolicy::default();
        assert_eq!(verdict(&err(403), 1, &policy, true), Verdict::Fallback);
        assert_eq!(verdict(&err(403), 1, &policy, false), Verdict::Fail);
        assert_eq!(verdict(&err(401), 1, &policy, true), Verdict::Fail);
        assert_eq!(verdict(&err(402), 1, &policy, true), Verdict::Fail);
    }

    #[test]
    fn default_fallbacks_start_with_the_primarys_own_family() {
        assert_eq!(
            openrouter_fallbacks("anthropic/claude-fable-5.1"),
            vec![
                "anthropic/claude-opus-5",
                "google/gemini-3.8-flash",
                "openai/gpt-5.6-terra"
            ]
        );
        // The primary is never its own fallback; the rest keep their order.
        assert_eq!(
            openrouter_fallbacks("openai/gpt-5.6-terra"),
            vec!["anthropic/claude-opus-5", "google/gemini-3.8-flash"]
        );
        assert_eq!(
            openrouter_fallbacks("google/gemini-2.5-flash"),
            vec![
                "google/gemini-3.8-flash",
                "anthropic/claude-opus-5",
                "openai/gpt-5.6-terra"
            ]
        );
        // No family: the constant's order, OpenAI last.
        assert_eq!(
            openrouter_fallbacks("mercury"),
            OPENROUTER_FALLBACKS
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        );
        let m = Models::with_defaults(
            "inception/mercury-2.5".into(),
            &[],
            "https://openrouter.ai/api/v1",
        );
        assert_eq!(m.all()[1], "anthropic/claude-opus-5");
        assert_eq!(m.all().last().unwrap(), "openai/gpt-5.6-terra");
    }
}
