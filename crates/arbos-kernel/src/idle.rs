//! `serve --until-idle`: run until every agent is idle and no plan node is
//! due within the horizon, then exit 0 — the fixture runner's mode from
//! `docs/filesystem-state-design.md`, and a cron job's ("do one pass").
//!
//! Idle means: no turn running, no mechanical node or condition in flight,
//! no pending node that is fireable now or falls due within `--horizon`
//! (default 1h), and no question waiting on a human. A waiting question
//! is idle in the sense that nothing will happen, but the run did not
//! finish: exit 3, the code `arbos-kernel run` uses for the same state.
//!
//! Set through the environment (`ARBOS_UNTIL_IDLE=1`, `ARBOS_HORIZON=1h`)
//! so a kernel that `run` starts inherits the choice like `ARBOS_NOW`.

use std::sync::Arc;
use std::time::Instant;

use arbos_core::{list_agents, subscription};

use crate::hooks::KernelHooks;

pub const UNTIL_IDLE_ENV: &str = "ARBOS_UNTIL_IDLE";
pub const HORIZON_ENV: &str = "ARBOS_HORIZON";
const DEFAULT_HORIZON_MS: i64 = 3_600_000;
/// Consecutive quiet checks (one per second) before the kernel believes it.
const QUIET_CHECKS: u32 = 2;
/// Nothing counts before the first scan and its wakes had a moment to land.
const MIN_UPTIME_MS: u128 = 1_500;

pub const EXIT_IDLE: i32 = 0;
pub const EXIT_WAITING: i32 = 3;

pub struct UntilIdle {
    horizon_ms: i64,
    quiet: u32,
    started: Instant,
}

#[derive(Debug)]
pub enum Verdict {
    /// Something is running or due soon; the reason, for the log.
    Busy(String),
    Idle,
    /// Idle, but these agents wait on a human.
    Waiting(Vec<String>),
}

impl UntilIdle {
    pub fn from_env() -> Option<Self> {
        let on = std::env::var(UNTIL_IDLE_ENV)
            .map(|v| matches!(v.trim(), "1" | "true" | "yes"))
            .unwrap_or(false);
        if !on {
            return None;
        }
        let horizon_ms = std::env::var(HORIZON_ENV)
            .ok()
            .and_then(|h| subscription::parse_duration_ms(&h))
            .map(|ms| ms as i64)
            .unwrap_or(DEFAULT_HORIZON_MS);
        Some(Self {
            horizon_ms,
            quiet: 0,
            started: Instant::now(),
        })
    }

    pub fn horizon_ms(&self) -> i64 {
        self.horizon_ms
    }

    /// One check, once a second. `Some(code)` when it is time to exit.
    pub fn poll(&mut self, hooks: &Arc<KernelHooks>) -> Option<i32> {
        if self.started.elapsed().as_millis() < MIN_UPTIME_MS {
            return None;
        }
        match verdict(hooks, self.horizon_ms) {
            Verdict::Busy(_) => {
                self.quiet = 0;
                None
            }
            Verdict::Idle => {
                self.quiet += 1;
                (self.quiet >= QUIET_CHECKS).then_some(EXIT_IDLE)
            }
            Verdict::Waiting(_) => {
                self.quiet += 1;
                (self.quiet >= QUIET_CHECKS).then_some(EXIT_WAITING)
            }
        }
    }
}

/// What the kernel is up to right now, as `--until-idle` sees it.
pub fn verdict(hooks: &Arc<KernelHooks>, horizon_ms: i64) -> Verdict {
    let running: Vec<String> = hooks.running.lock().unwrap().iter().cloned().collect();
    if !running.is_empty() {
        return Verdict::Busy(format!("turns running: {}", running.join(", ")));
    }
    if crate::subs::busy() {
        return Verdict::Busy("subscription runs in flight".into());
    }
    let now = arbos_core::now_ms();
    for agent in list_agents(&hooks.place).unwrap_or_default() {
        if agent.paused {
            continue;
        }
        let id = agent.id.as_str();
        if arbos_core::inbox::list(&hooks.place, id).iter().any(|f| f.msg.wake) {
            return Verdict::Busy(format!("{id}: a message waits for a turn"));
        }
        if let Some(sub) = subscription::list(&hooks.place, id).into_iter().find(|s| {
            !s.paused && s.next_due_ms().is_some_and(|due| due <= now + horizon_ms)
        }) {
            let due = sub.next_due_ms().unwrap_or(now);
            return Verdict::Busy(format!(
                "{id}: subscription #{} due in {}s",
                sub.id,
                (due - now).max(0) / 1000
            ));
        }
    }
    let waiting: Vec<String> = hooks.asks.lock().unwrap().keys().cloned().collect();
    if !waiting.is_empty() {
        return Verdict::Waiting(waiting);
    }
    Verdict::Idle
}
