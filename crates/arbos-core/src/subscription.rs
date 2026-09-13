//! `agents/<id>/subscriptions/NNNN-slug.toml`: the only scheduler.
//!
//! A subscription is a standing request to be woken when something happens:
//! a clock tick (`timer`), a command's result on a period (`shell`), a
//! pull request or its checks changing (`github_pr`, `github_ci`), or a new
//! file in a folder (`inbox`). Firing writes one inbox file to the agent —
//! or, for `deliver_to = "user"`, one line to the user with no model turn.
//! There is no other clock (Cursor's model, decided 2026-09-13).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::Place;

/// Shortest period a subscription may have.
pub const MIN_EVERY_MS: u64 = 30_000;
/// Poll period for the GitHub kinds when `every` is absent.
pub const GITHUB_DEFAULT_EVERY_MS: u64 = 60_000;

pub const KINDS: &[&str] = &["timer", "shell", "github_pr", "github_ci", "inbox"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Subscription {
    pub id: u32,
    /// `timer` | `shell` | `github_pr` | `github_ci` | `inbox`.
    pub kind: String,
    /// What the agent is told when it fires. For `shell`, the output and
    /// exit follow it. Empty is allowed for the GitHub kinds (the diff is
    /// the message).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prompt: String,
    /// Period, e.g. "1h". Absent: one-shot (`timer` with `next_due`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every: Option<String>,
    /// "09:00" or ":15": the next due lands on that wall-clock moment
    /// (UTC for now; see the design's `timezone` note).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// Remove after the first firing.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub once: bool,
    /// `shell`: the command the kernel runs as a job, no model turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmd: Option<String>,
    /// `inbox`: the folder watched for new files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// `github_*`: `owner/name` and the PR number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<u64>,
    /// `agent` (an inbox file, a turn) or `user` (a line to the user, no
    /// model turn; `shell` only, failures still wake the agent).
    #[serde(default = "default_deliver_to")]
    pub deliver_to: String,
    /// `deliver_to = "user"`: the line sent, with `{output}` replaced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify: Option<String>,
    /// RFC 3339: the subscription is removed after this instant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub paused: bool,
    pub created: String,
    /// RFC 3339: when the kernel next looks. The kernel writes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_due: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_fired: Option<String>,
    /// One line about the last firing (exit code, output head, "no change").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last: String,
    /// The last error, so it is said once and not every poll.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// `github_*` / `inbox`: what was seen last, as JSON, for the diff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen: Option<String>,
}

fn default_deliver_to() -> String {
    "agent".into()
}

impl Subscription {
    pub fn every_ms(&self) -> Option<u64> {
        match &self.every {
            Some(e) => parse_duration_ms(e),
            None => match self.kind.as_str() {
                "github_pr" | "github_ci" => Some(GITHUB_DEFAULT_EVERY_MS),
                _ => None,
            },
        }
    }

    pub fn next_due_ms(&self) -> Option<i64> {
        self.next_due.as_deref().and_then(crate::parse_instant_ms)
    }

    pub fn expires_ms(&self) -> Option<i64> {
        self.expires.as_deref().and_then(crate::parse_instant_ms)
    }

    pub fn is_due(&self, now_ms: i64) -> bool {
        !self.paused && self.next_due_ms().is_some_and(|d| d <= now_ms)
    }

    pub fn expired(&self, now_ms: i64) -> bool {
        self.expires_ms().is_some_and(|e| e <= now_ms)
    }

    /// Where the next look lands after firing at `now_ms`. None: one-shot,
    /// done.
    pub fn schedule_next(&mut self, now_ms: i64) {
        let next = match self.every_ms() {
            Some(every) if !self.once => Some(align_at(now_ms + every as i64, self.at.as_deref())),
            _ => None,
        };
        self.next_due = next.map(crate::inbox::rfc3339);
        self.last_fired = Some(crate::inbox::rfc3339(now_ms));
    }

    /// The file name: `0003-nightly-tests.toml`.
    pub fn file_name(&self) -> String {
        format!("{:04}-{}.toml", self.id, slug(&self.label()))
    }

    /// A short label: the prompt's head, the command, or the PR.
    pub fn label(&self) -> String {
        let text = if !self.prompt.is_empty() {
            self.prompt.clone()
        } else if let Some(c) = &self.cmd {
            c.clone()
        } else if let (Some(r), Some(n)) = (&self.repo, self.pr) {
            format!("{r}#{n}")
        } else if let Some(p) = &self.path {
            p.clone()
        } else {
            self.kind.clone()
        };
        crate::text::clip(&text, 60)
    }

    /// `every 1h · next 15:04`, `once at 15:04`, `paused`, `watching …/inbox`.
    pub fn when_line(&self) -> String {
        if self.paused {
            return "paused".into();
        }
        let next = self
            .next_due_ms()
            .map(|d| format!(" · next {}", clock(d)))
            .unwrap_or_default();
        match (&self.every, self.once) {
            (Some(e), false) => format!("every {e}{next}"),
            _ => match self.next_due_ms() {
                Some(d) => format!("once at {}", clock(d)),
                None => match self.kind.as_str() {
                    "github_pr" | "github_ci" => format!(
                        "every {}{next}",
                        human_ms(self.every_ms().unwrap_or(GITHUB_DEFAULT_EVERY_MS))
                    ),
                    _ => String::new(),
                },
            },
        }
    }

    /// Refuse what the watcher could not run.
    pub fn validate(&self) -> Result<()> {
        if !KINDS.contains(&self.kind.as_str()) {
            bail!("kind must be one of {}", KINDS.join(", "));
        }
        if let Some(e) = &self.every {
            let ms = parse_duration_ms(e)
                .with_context(|| format!("every {e:?} is not a duration (30m, 2h, 1d)"))?;
            if ms < MIN_EVERY_MS {
                bail!(
                    "every {e:?} is under the minimum of {}s",
                    MIN_EVERY_MS / 1000
                );
            }
        }
        match self.kind.as_str() {
            "timer" => {
                if self.every.is_none() && self.next_due.is_none() {
                    bail!("a timer needs every (recurring) or after/at (one-shot)");
                }
                if self.prompt.trim().is_empty() {
                    bail!("a timer needs a prompt: what to do when it fires");
                }
            }
            "shell" => {
                if self.cmd.as_deref().unwrap_or("").trim().is_empty() {
                    bail!("shell needs cmd");
                }
                if self.every.is_none() && self.next_due.is_none() {
                    bail!("shell needs every (or after for one run)");
                }
                if self.deliver_to == "user"
                    && let Some(n) = &self.notify
                    && !n.contains("{output}")
                {
                    bail!("notify must contain {{output}}, or the reading never reaches the user");
                }
            }
            "github_pr" | "github_ci" => {
                if self.repo.as_deref().unwrap_or("").trim().is_empty() || self.pr.is_none() {
                    bail!("{} needs repo (owner/name) and pr", self.kind);
                }
            }
            "inbox" => {
                if self.path.as_deref().unwrap_or("").trim().is_empty() {
                    bail!("inbox needs path: the folder to watch");
                }
                if self.every.is_none() {
                    bail!("inbox needs every: how often to look");
                }
            }
            _ => {}
        }
        if self.deliver_to != "agent" && self.deliver_to != "user" {
            bail!("deliver_to must be agent or user");
        }
        if self.deliver_to == "user" && self.kind != "shell" {
            bail!(
                "deliver_to = user is for shell (a reading with no model turn); a {} wakes the agent",
                self.kind
            );
        }
        Ok(())
    }
}

/// `at = "09:00"` moves a due instant to the next such wall-clock moment
/// at or after it; `":15"` to the next quarter past the hour. UTC.
pub fn align_at(due_ms: i64, at: Option<&str>) -> i64 {
    let Some(at) = at else {
        return due_ms;
    };
    let at = at.trim();
    let day_ms = 86_400_000i64;
    let hour_ms = 3_600_000i64;
    if let Some(min) = at.strip_prefix(':') {
        let Ok(m) = min.parse::<i64>() else {
            return due_ms;
        };
        let base = due_ms - due_ms.rem_euclid(hour_ms);
        let target = base + m * 60_000;
        return if target >= due_ms {
            target
        } else {
            target + hour_ms
        };
    }
    let Some((h, m)) = at.split_once(':') else {
        return due_ms;
    };
    let (Ok(h), Ok(m)) = (h.parse::<i64>(), m.parse::<i64>()) else {
        return due_ms;
    };
    let base = due_ms - due_ms.rem_euclid(day_ms);
    let target = base + h * hour_ms + m * 60_000;
    if target >= due_ms {
        target
    } else {
        target + day_ms
    }
}

pub fn dir(place: &Place, agent: &str) -> PathBuf {
    place.agent_dir(agent).join("subscriptions")
}

/// Every subscription of `agent`, by id.
pub fn list(place: &Place, agent: &str) -> Vec<Subscription> {
    let Ok(rd) = std::fs::read_dir(dir(place, agent)) else {
        return Vec::new();
    };
    let mut out: Vec<Subscription> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .filter_map(|p| read(&p).ok())
        .collect();
    out.sort_by_key(|s| s.id);
    out
}

pub fn read(path: &Path) -> Result<Subscription> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

/// The file for `sub`, whatever slug an earlier save gave it.
pub fn path_of(place: &Place, agent: &str, id: u32) -> Option<PathBuf> {
    let prefix = format!("{id:04}-");
    std::fs::read_dir(dir(place, agent))
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&prefix) && n.ends_with(".toml"))
        })
}

/// Write whole-or-absent. The slug is fixed at the first save.
pub fn save(place: &Place, agent: &str, sub: &Subscription) -> Result<PathBuf> {
    let dir = dir(place, agent);
    std::fs::create_dir_all(&dir)?;
    let path = path_of(place, agent, sub.id).unwrap_or_else(|| dir.join(sub.file_name()));
    let text = toml::to_string_pretty(sub).context("serialise subscription")?;
    let tmp = dir.join(format!(".{}.{}.tmp", sub.id, std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Add `sub` with the next free id; `next_due` is set from `after`,
/// `every`, and `at` when the caller left it empty.
pub fn add(
    place: &Place,
    agent: &str,
    mut sub: Subscription,
    after: Option<&str>,
) -> Result<Subscription> {
    let now = crate::now_ms();
    let existing = list(place, agent);
    sub.id = existing.iter().map(|s| s.id).max().unwrap_or(0) + 1;
    if sub.created.is_empty() {
        sub.created = crate::inbox::rfc3339(now);
    }
    if sub.next_due.is_none() {
        if after.is_none()
            && sub.every_ms().is_none()
            && matches!(sub.kind.as_str(), "timer" | "shell")
        {
            bail!("{} needs every (recurring) or after (one-shot)", sub.kind);
        }
        let first = match after {
            Some(a) => {
                let ms = parse_duration_ms(a)
                    .with_context(|| format!("after {a:?} is not a duration"))?;
                now + ms as i64
            }
            None => match sub.every_ms() {
                Some(e) => now + e as i64,
                None => now,
            },
        };
        sub.next_due = Some(crate::inbox::rfc3339(align_at(first, sub.at.as_deref())));
        if after.is_some() && sub.every.is_none() {
            sub.once = true;
        }
    }
    sub.validate()?;
    save(place, agent, &sub)?;
    Ok(sub)
}

pub fn remove(place: &Place, agent: &str, id: u32) -> Result<bool> {
    match path_of(place, agent, id) {
        Some(p) => {
            std::fs::remove_file(&p)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

pub fn get(place: &Place, agent: &str, id: u32) -> Option<Subscription> {
    path_of(place, agent, id).and_then(|p| read(&p).ok())
}

/// Parse `30m`, `2h`, `90s`, `1d`, or plain seconds, into milliseconds.
pub fn parse_duration_ms(s: &str) -> Option<u64> {
    let s = s.trim().to_ascii_lowercase();
    if s.is_empty() {
        return None;
    }
    let (num, unit) = match s.find(|c: char| c.is_ascii_alphabetic()) {
        Some(i) => s.split_at(i),
        None => (s.as_str(), "s"),
    };
    let n: f64 = num.trim().parse().ok()?;
    if n <= 0.0 {
        return None;
    }
    let mult = match unit.trim() {
        "ms" => 1.0,
        "s" | "sec" | "secs" => 1_000.0,
        "m" | "min" | "mins" => 60_000.0,
        "h" | "hr" | "hrs" => 3_600_000.0,
        "d" | "day" | "days" => 86_400_000.0,
        _ => return None,
    };
    Some((n * mult) as u64)
}

pub fn human_ms(ms: u64) -> String {
    let s = ms / 1000;
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86_400)
    }
}

/// `HH:MM` UTC for a unix millisecond stamp.
pub fn clock(ms: i64) -> String {
    let day = ms.div_euclid(1000).rem_euclid(86_400);
    format!("{:02}:{:02}", day / 3600, (day % 3600) / 60)
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.chars().take(40) {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "sub".into() } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(tag: &str) -> Place {
        let dir = std::env::temp_dir().join(format!("arbos-subs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".arbos/agents/root")).unwrap();
        Place::new(dir)
    }

    fn timer(prompt: &str, every: Option<&str>) -> Subscription {
        Subscription {
            id: 0,
            kind: "timer".into(),
            prompt: prompt.into(),
            every: every.map(str::to_string),
            at: None,
            once: false,
            cmd: None,
            path: None,
            repo: None,
            pr: None,
            deliver_to: "agent".into(),
            notify: None,
            expires: None,
            paused: false,
            created: String::new(),
            next_due: None,
            last_fired: None,
            last: String::new(),
            error: None,
            seen: None,
        }
    }

    #[test]
    fn a_recurring_timer_is_due_after_its_period_and_reschedules() {
        let p = place("timer");
        let now = crate::now_ms();
        let sub = add(&p, "root", timer("run tests", Some("1h")), None).unwrap();
        assert_eq!(sub.id, 1);
        assert!(!sub.is_due(now));
        assert!(sub.is_due(now + 3_600_001));
        let mut fired = sub.clone();
        fired.schedule_next(now + 3_600_001);
        assert!(fired.next_due_ms().unwrap() > now + 7_000_000);
        let listed = list(&p, "root");
        assert_eq!(listed.len(), 1);
        assert!(
            path_of(&p, "root", 1)
                .unwrap()
                .to_string_lossy()
                .ends_with("0001-run-tests.toml")
        );
    }

    #[test]
    fn after_makes_a_one_shot_that_ends_after_firing() {
        let p = place("once");
        let now = crate::now_ms();
        let mut sub = add(&p, "root", timer("remind me", None), Some("30m")).unwrap();
        assert!(sub.once);
        assert!(sub.is_due(now + 1_800_001) && !sub.is_due(now));
        sub.schedule_next(now + 1_800_001);
        assert!(sub.next_due.is_none());
    }

    #[test]
    fn validation_refuses_what_the_watcher_cannot_run() {
        let p = place("bad");
        assert!(add(&p, "root", timer("", Some("1h")), None).is_err());
        assert!(add(&p, "root", timer("x", Some("5s")), None).is_err());
        assert!(add(&p, "root", timer("x", None), None).is_err());
        let mut shell = timer("", Some("1h"));
        shell.kind = "shell".into();
        assert!(add(&p, "root", shell.clone(), None).is_err());
        shell.cmd = Some("date".into());
        shell.deliver_to = "user".into();
        shell.notify = Some("now".into());
        assert!(add(&p, "root", shell.clone(), None).is_err());
        shell.notify = Some("now: {output}".into());
        assert!(add(&p, "root", shell, None).is_ok());
    }

    #[test]
    fn at_aligns_to_the_next_wall_clock_moment() {
        let day = 86_400_000;
        let t = day * 100 + 10 * 3_600_000; // 10:00
        assert_eq!(align_at(t, Some("09:00")), day * 101 + 9 * 3_600_000);
        assert_eq!(
            align_at(t, Some("11:30")),
            day * 100 + 11 * 3_600_000 + 30 * 60_000
        );
        assert_eq!(
            align_at(t + 20 * 60_000, Some(":15")),
            t + 3_600_000 + 15 * 60_000
        );
        assert_eq!(align_at(t, None), t);
    }
}
