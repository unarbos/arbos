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
/// How often a goal's check runs when `every` is absent.
pub const GOAL_DEFAULT_EVERY_MS: u64 = 30 * 60_000;

pub const KINDS: &[&str] = &[
    "timer",
    "shell",
    "github_pr",
    "github_ci",
    "inbox",
    "goal",
    "chat",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Subscription {
    /// 0 in a hand-written file: `read` fills it from the `NNNN-` prefix.
    #[serde(default)]
    pub id: u32,
    /// `timer` | `shell` | `github_pr` | `github_ci` | `inbox` | `goal`.
    /// A `goal` is an objective held until met: `prompt` says what, `cmd`
    /// (optional) is the check that says when — exit 0 closes the goal;
    /// while it fails the agent is woken with the goal and the check's
    /// output, every `every` (default 30m). Without `cmd` the agent is
    /// woken each period until it removes the goal itself.
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
    /// `github_ci` without a PR: the branch whose workflow runs are
    /// watched (`gh run list --branch`). A "keep main green" loop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// `chat`: the channel a door polls (`discord:<id>`, `slack:<id>`, or
    /// the bare id) whose new messages wake this agent (Cursor's Slack
    /// channel subscription).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// `chat`: only messages in this thread (Slack `thread_ts`; a Discord
    /// thread is a channel of its own, so name it as the channel).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    /// `chat`: only messages whose text contains this (case-insensitive).
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "match")]
    pub match_text: Option<String>,
    /// `agent` (an inbox file, a turn), `user` (a line to the user, no
    /// model turn), or `none` (a quiet chore: nothing on success). `user`
    /// and `none` are for `shell`; a failure always wakes the agent.
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
    /// `timer` / `shell`: each firing carries what the last one produced
    /// — the command's output, or the last words of the turn the timer
    /// opened — so a monitor can compare instead of starting over
    /// (Hermes cron `continuity`). Kept in `seen`, capped.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub continuity: bool,
    /// A kernel chore (the weekly `git gc`): fires like any other but stays
    /// out of every user-facing list — the plan strip, the prompt's
    /// standing section, `subscribe list`. `check` still sees it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub internal: bool,
    /// Empty in a hand-written file: `read` fills it from the file's mtime.
    #[serde(default)]
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

/// `list` without kernel chores: what a person or the model should see.
pub fn list_visible(place: &Place, agent: &str) -> Vec<Subscription> {
    list(place, agent)
        .into_iter()
        .filter(|s| !s.internal)
        .collect()
}

impl Subscription {
    pub fn every_ms(&self) -> Option<u64> {
        match &self.every {
            Some(e) => parse_duration_ms(e),
            None => match self.kind.as_str() {
                "github_pr" | "github_ci" => Some(GITHUB_DEFAULT_EVERY_MS),
                "goal" => Some(GOAL_DEFAULT_EVERY_MS),
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

    /// Whole periods this subscription has missed beyond the one that is
    /// due now: 0 when it is on time or has no period.
    pub fn missed_periods(&self, now_ms: i64) -> u64 {
        let (Some(due), Some(every)) = (self.next_due_ms(), self.every_ms()) else {
            return 0;
        };
        if every == 0 || due > now_ms {
            return 0;
        }
        ((now_ms - due) as u64 / every).min(1_000_000)
    }

    /// After a pause or a long stop: the next firing is one period from
    /// `now`, not a pile of overdue ones. `last_fired` is left alone.
    pub fn resume_at(&mut self, now_ms: i64) {
        if let Some(every) = self.every_ms()
            && !self.once
        {
            self.next_due = Some(crate::inbox::rfc3339(align_at(
                now_ms + every as i64,
                self.at.as_deref(),
            )));
        }
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
        } else if let (Some(r), Some(b)) = (&self.repo, &self.branch) {
            format!("{r}@{b}")
        } else if let Some(p) = &self.path {
            p.clone()
        } else if let Some(c) = &self.channel {
            match &self.thread {
                Some(t) => format!("{c} thread {t}"),
                None => c.clone(),
            }
        } else {
            self.kind.clone()
        };
        crate::text::clip(&text, 60)
    }

    /// `chat`: does a message in `channel_tag` (`discord:<id>`) with this
    /// thread and text fall under this subscription?
    pub fn matches_chat(&self, channel_tag: &str, thread: Option<&str>, text: &str) -> bool {
        if self.kind != "chat" || self.paused {
            return false;
        }
        let Some(want) = self.channel.as_deref().map(str::trim) else {
            return false;
        };
        let bare = channel_tag
            .split_once(':')
            .map(|(_, id)| id)
            .unwrap_or(channel_tag);
        if want != channel_tag && want != bare {
            return false;
        }
        if let Some(t) = self.thread.as_deref().filter(|t| !t.trim().is_empty())
            && thread != Some(t.trim())
        {
            return false;
        }
        if let Some(m) = self.match_text.as_deref().filter(|m| !m.trim().is_empty())
            && !text.to_lowercase().contains(&m.trim().to_lowercase())
        {
            return false;
        }
        true
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
                    "chat" => match &self.thread {
                        Some(t) => format!("on each message in thread {t}"),
                        None => "on each message".to_string(),
                    },
                    "github_pr" | "github_ci" => format!(
                        "every {}{next}",
                        human_ms(self.every_ms().unwrap_or(GITHUB_DEFAULT_EVERY_MS))
                    ),
                    "goal" => format!(
                        "until met · checked every {}{next}",
                        human_ms(self.every_ms().unwrap_or(GOAL_DEFAULT_EVERY_MS))
                    ),
                    _ => String::new(),
                },
            },
        }
    }

    /// Read the shape the model meant where it is unambiguous, instead of
    /// refusing (qa-019 `kind="default"`, qa-031 `host="local"`, qa-035
    /// `kind=timer` with a `cmd`). Returns what was changed, in words for
    /// the tool result; nothing when the call was already as written.
    pub fn coerce(&mut self) -> Vec<String> {
        let mut notes = Vec::new();
        let has_cmd = self.cmd.as_deref().is_some_and(|c| !c.trim().is_empty());
        if matches!(self.kind.as_str(), "timer" | "default" | "") && has_cmd {
            // A command on a schedule is a shell subscription: the kernel
            // runs it, no model turn, the output goes where deliver_to says.
            notes.push(format!(
                "kind = {} with a cmd runs as kind = shell (the kernel runs the command; deliver_to = user posts its output)",
                if self.kind.is_empty() { "(none)".to_string() } else { self.kind.clone() }
            ));
            self.kind = "shell".into();
        } else if matches!(self.kind.as_str(), "default" | "") {
            self.kind = "timer".into();
        }
        if self.kind == "shell"
            && self.deliver_to == "user"
            && self
                .notify
                .as_deref()
                .is_some_and(|n| !n.contains("{output}"))
        {
            // A notify without the output would post the label alone; the
            // reading goes after it.
            let n = self.notify.take().unwrap_or_default();
            self.notify = Some(format!("{}: {{output}}", n.trim_end_matches(':').trim()));
            notes.push("notify had no {output}; the command's output is appended to it".into());
        }
        notes
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
            "github_pr" => {
                if self.repo.as_deref().unwrap_or("").trim().is_empty() || self.pr.is_none() {
                    bail!("github_pr needs repo (owner/name) and pr");
                }
            }
            "github_ci" => {
                let has_branch = self.branch.as_deref().is_some_and(|b| !b.trim().is_empty());
                if self.repo.as_deref().unwrap_or("").trim().is_empty()
                    || (self.pr.is_none() && !has_branch)
                {
                    bail!("github_ci needs repo (owner/name) and pr or branch");
                }
            }
            "goal" => {
                if self.prompt.trim().is_empty() {
                    bail!("goal needs prompt: what is to be true when it is met");
                }
                if self.deliver_to != "agent" {
                    bail!("goal delivers to the agent; deliver_to must be agent");
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
            "chat" => {
                if self.channel.as_deref().unwrap_or("").trim().is_empty() {
                    bail!("chat needs channel: a channel a door in doors.toml polls");
                }
                if self.every.is_some() || self.next_due.is_some() {
                    bail!("chat has no schedule: the door's messages fire it");
                }
            }
            _ => {}
        }
        if self.continuity && !matches!(self.kind.as_str(), "timer" | "shell") {
            bail!("continuity is for timer and shell subscriptions");
        }
        if !matches!(self.deliver_to.as_str(), "agent" | "user" | "none") {
            bail!("deliver_to must be agent, user, or none");
        }
        if self.deliver_to != "agent" && self.kind != "shell" {
            bail!(
                "deliver_to = {} is for shell (a command with no model turn); a {} wakes the agent",
                self.deliver_to,
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

/// Every subscription of `agent`, by id. A file that does not read is
/// skipped here; `list_with_errors` says which and why (the watcher logs
/// it once, `check` reports it).
pub fn list(place: &Place, agent: &str) -> Vec<Subscription> {
    list_with_errors(place, agent).0
}

/// The readable subscriptions and, per unreadable file, its name and the
/// error.
pub fn list_with_errors(place: &Place, agent: &str) -> (Vec<Subscription>, Vec<(String, String)>) {
    let Ok(rd) = std::fs::read_dir(dir(place, agent)) else {
        return (Vec::new(), Vec::new());
    };
    let mut out = Vec::new();
    let mut errors = Vec::new();
    let mut paths: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .filter(|p| {
            !p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        })
        .collect();
    paths.sort();
    for p in paths {
        match read(&p) {
            Ok(sub) => out.push(sub),
            Err(e) => errors.push((
                p.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                format!("{e:#}"),
            )),
        }
    }
    out.sort_by_key(|s| s.id);
    (out, errors)
}

/// Read one file. A hand-written file may leave out what the kernel
/// usually writes: `id` comes from the `NNNN-` file prefix, `created`
/// from the file's mtime, and `next_due` is now (so it fires on the next
/// scan) — file-authored subscriptions are the point of the folder. What
/// the file cannot do without (a kind, a command for `shell`, a period)
/// still fails, with the reason.
pub fn read(path: &Path) -> Result<Subscription> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut sub: Subscription =
        toml::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if sub.id == 0 {
        sub.id = name
            .split('-')
            .next()
            .and_then(|n| n.trim_end_matches(".toml").parse::<u32>().ok())
            .filter(|n| *n > 0)
            .with_context(|| {
                format!("{name}: no id in the file and no NNNN- prefix in the name")
            })?;
    }
    if sub.created.trim().is_empty() {
        let ms = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or_else(crate::now_ms);
        sub.created = crate::inbox::rfc3339(ms);
    }
    if sub.next_due.is_none() && !sub.paused && (sub.every_ms().is_some() || sub.kind == "inbox") {
        sub.next_due = Some(crate::inbox::rfc3339(align_at(
            crate::now_ms(),
            sub.at.as_deref(),
        )));
    }
    // A hand-written file gets the same reading as a tool call.
    sub.coerce();
    sub.validate().with_context(|| format!("{name}"))?;
    Ok(sub)
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
    sub.id = next_id(place, agent);
    if sub.created.is_empty() {
        sub.created = crate::inbox::rfc3339(now);
    }
    // Event kinds have no schedule: a door's message fires a `chat`.
    if sub.next_due.is_none() && sub.kind != "chat" {
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
            // A goal's first check runs now: the agent starts on it at
            // once instead of after the first period.
            None if sub.kind == "goal" => now,
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
    sub.coerce();
    sub.validate()?;
    save(place, agent, &sub)?;
    Ok(sub)
}

/// One past the highest id in the folder — by file prefix as well as by
/// content, so a file that does not read (or has no `id` yet) is never
/// overwritten by the next `add`.
fn next_id(place: &Place, agent: &str) -> u32 {
    let by_name = std::fs::read_dir(dir(place, agent))
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    name.split('-')
                        .next()?
                        .trim_end_matches(".toml")
                        .parse::<u32>()
                        .ok()
                })
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0);
    let by_content = list(place, agent).iter().map(|s| s.id).max().unwrap_or(0);
    by_name.max(by_content) + 1
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

            branch: None,

            channel: None,

            thread: None,

            match_text: None,
            deliver_to: "agent".into(),
            notify: None,
            expires: None,
            paused: false,
            continuity: false,
            internal: false,
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
        // A notify without the output slot is read as "label: {output}"
        // rather than refused (qa-035).
        shell.notify = Some("now".into());
        let saved = add(&p, "root", shell.clone(), None).unwrap();
        assert_eq!(saved.notify.as_deref(), Some("now: {output}"));
        shell.notify = Some("now: {output}".into());
        assert!(add(&p, "root", shell, None).is_ok());
    }

    #[test]
    fn a_hand_written_file_gets_id_created_and_next_due_filled_in() {
        let p = place("hand");
        let d = dir(&p, "root");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("0007-tick.toml"),
            "kind = \"shell\"\ncmd = \"echo tick\"\nevery = \"30s\"\nprompt = \"tick ran\"\n",
        )
        .unwrap();
        let subs = list(&p, "root");
        assert_eq!(subs.len(), 1, "{subs:?}");
        assert_eq!(subs[0].id, 7);
        assert!(!subs[0].created.is_empty());
        assert!(subs[0].is_due(crate::now_ms() + 1), "due on the next scan");
        // What it cannot do without still fails, and the error is kept.
        std::fs::write(
            d.join("0008-bad.toml"),
            "kind = \"shell\"\nevery = \"30s\"\n",
        )
        .unwrap();
        std::fs::write(
            d.join("noid.toml"),
            "kind = \"timer\"\nevery = \"1h\"\nprompt = \"x\"\n",
        )
        .unwrap();
        let (ok, errors) = list_with_errors(&p, "root");
        assert_eq!(ok.len(), 1);
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(
            errors
                .iter()
                .any(|(n, e)| n == "0008-bad.toml" && e.contains("shell needs cmd")),
            "{errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|(n, e)| n == "noid.toml" && e.contains("no id")),
            "{errors:?}"
        );
        // The next add never lands on an unreadable file's number.
        let added = add(&p, "root", timer("later", Some("1h")), None).unwrap();
        assert_eq!(added.id, 9, "{added:?}");
        assert!(d.join("0008-bad.toml").exists());
        assert_eq!(
            std::fs::read_to_string(d.join("0008-bad.toml")).unwrap(),
            "kind = \"shell\"\nevery = \"30s\"\n"
        );
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

#[cfg(test)]
mod coerce_tests {
    use super::*;

    /// qa-035: the kickoff coordinator's call, as it sent it.
    #[test]
    fn a_timer_with_a_cmd_is_read_as_a_shell_subscription() {
        let mut sub = Subscription {
            id: 0,
            kind: "timer".into(),
            prompt: "record the QA loop result in notes.md".into(),
            every: Some("1h".into()),
            at: None,
            once: false,
            cmd: Some("python3 toy-repo/hello.py".into()),
            path: None,
            repo: None,
            pr: None,
            branch: None,
            channel: None,
            thread: None,
            match_text: None,
            deliver_to: "user".into(),
            notify: Some("QA loop result: {output}".into()),
            expires: None,
            paused: false,
            continuity: false,
            internal: false,
            created: String::new(),
            next_due: None,
            last_fired: None,
            last: String::new(),
            error: None,
            seen: None,
        };
        assert!(sub.validate().is_err(), "as sent, it is refused");
        let notes = sub.coerce();
        assert_eq!(sub.kind, "shell");
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(
            notes[0].contains("kind = timer with a cmd runs as kind = shell"),
            "{notes:?}"
        );
        sub.validate().unwrap();
        assert_eq!(sub.notify.as_deref(), Some("QA loop result: {output}"));
        // A notify without the output slot gets it; a plain timer stays one.
        sub.notify = Some("QA loop result".into());
        let notes = sub.coerce();
        assert_eq!(sub.notify.as_deref(), Some("QA loop result: {output}"));
        assert_eq!(notes.len(), 1, "{notes:?}");
        let mut plain = sub.clone();
        plain.kind = "timer".into();
        plain.cmd = None;
        plain.deliver_to = "agent".into();
        assert!(plain.coerce().is_empty());
        assert_eq!(plain.kind, "timer");
    }
}

#[cfg(test)]
mod branch_tests {
    use super::*;

    #[test]
    fn github_ci_takes_a_branch_in_place_of_a_pr() {
        let mut sub = Subscription {
            id: 0,
            kind: "github_ci".into(),
            prompt: String::new(),
            every: None,
            at: None,
            once: false,
            cmd: None,
            path: None,
            repo: Some("o/r".into()),
            pr: None,
            branch: Some("main".into()),
            channel: None,
            thread: None,
            match_text: None,
            deliver_to: "agent".into(),
            notify: None,
            expires: None,
            paused: false,
            continuity: false,
            internal: false,
            created: String::new(),
            next_due: None,
            last_fired: None,
            last: String::new(),
            error: None,
            seen: None,
        };
        sub.validate().unwrap();
        assert_eq!(sub.label(), "o/r@main");
        sub.branch = None;
        assert!(sub.validate().is_err());
        sub.kind = "github_pr".into();
        sub.branch = Some("main".into());
        assert!(sub.validate().is_err(), "github_pr still needs a pr");
        // The line round-trips through TOML.
        sub.kind = "github_ci".into();
        let text = toml::to_string(&sub).unwrap();
        assert!(text.contains("branch = \"main\""), "{text}");
        let back: Subscription = toml::from_str(&text).unwrap();
        assert_eq!(back.branch.as_deref(), Some("main"));
    }
}
