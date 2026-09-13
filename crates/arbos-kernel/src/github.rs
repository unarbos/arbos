//! The GitHub door: follow a pull request and wake the subscriber when it
//! changes — a review, a comment, a check, a merge.
//!
//! `subscribe add repo=owner/name pr=N` writes an entry to
//! `<place>/.arbos/subscriptions.json`. A poller runs `gh pr view` for each
//! entry on a timer, compares with the last snapshot it saved, and on a
//! change appends a `[github]` line to the subscriber's transcript and
//! queues a turn — the same path `say mode=request` takes. `gh` runs with
//! the kernel's environment plus any secret granted through the `secret`
//! tool, so a token never touches the transcript.

use anyhow::{Context, Result, bail};
use arbos_core::{Event, EventKind, Node, append_event, node::DEFAULT_HOPS};
use arbos_engine::{Access, BoxFuture, Plan, PlanCx, RunCx, Tool, ToolOut, typed_schema};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use crate::hooks::KernelHooks;

/// Default poll period. `ARBOS_GITHUB_POLL_S` overrides.
const POLL_S: u64 = 60;
const MIN_POLL_S: u64 = 15;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub state: String,
    pub title: String,
    pub head: String,
    /// `author:STATE` per review, in order.
    pub reviews: Vec<String>,
    pub comments: usize,
    pub last_commenter: String,
    /// check name → conclusion (or status while pending).
    pub checks: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    pub id: u64,
    pub agent: String,
    pub repo: String,
    pub pr: u64,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub last: Option<Snapshot>,
    /// The last error, so it is reported once and not every poll.
    #[serde(default)]
    pub error: Option<String>,
    pub created_ms: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct File {
    #[serde(default)]
    pub next_id: u64,
    #[serde(default)]
    pub subscriptions: Vec<Subscription>,
}

impl File {
    pub fn path(place: &Path) -> PathBuf {
        place.join(".arbos").join("subscriptions.json")
    }

    pub fn load(place: &Path) -> Result<Self> {
        let path = Self::path(place);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Ok(Self::default());
        };
        serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
    }

    pub fn save(&self, place: &Path) -> Result<()> {
        let path = Self::path(place);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}

/// `gh` with the kernel's environment plus granted secrets.
fn gh(args: &[&str]) -> Result<Value> {
    let mut cmd = Command::new("gh");
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .envs(arbos_engine::secrets::store().env());
    let out = cmd
        .output()
        .context("run gh (is the GitHub CLI installed?)")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let line = err
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("failed")
            .trim();
        bail!("gh {}: {line}", args.first().copied().unwrap_or(""));
    }
    Ok(serde_json::from_slice(&out.stdout).context("gh returned no JSON")?)
}

/// One look at a pull request.
pub fn snapshot(repo: &str, pr: u64) -> Result<Snapshot> {
    let v = gh(&[
        "pr",
        "view",
        &pr.to_string(),
        "--repo",
        repo,
        "--json",
        "state,title,headRefOid,reviews,comments,statusCheckRollup",
    ])?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let reviews = v
        .get("reviews")
        .and_then(Value::as_array)
        .map(|rs| {
            rs.iter()
                .map(|r| {
                    format!(
                        "{}:{}",
                        r.get("author")
                            .and_then(|a| a.get("login"))
                            .and_then(Value::as_str)
                            .unwrap_or("?"),
                        r.get("state").and_then(Value::as_str).unwrap_or("?")
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let comments_v = v.get("comments").and_then(Value::as_array);
    let comments = comments_v.map(|c| c.len()).unwrap_or(0);
    let last_commenter = comments_v
        .and_then(|c| c.last())
        .and_then(|c| c.get("author"))
        .and_then(|a| a.get("login"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut checks = BTreeMap::new();
    if let Some(rollup) = v.get("statusCheckRollup").and_then(Value::as_array) {
        for c in rollup {
            let name = c
                .get("name")
                .or_else(|| c.get("context"))
                .and_then(Value::as_str)
                .unwrap_or("check")
                .to_string();
            let outcome = c
                .get("conclusion")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .or_else(|| c.get("state").and_then(Value::as_str))
                .or_else(|| c.get("status").and_then(Value::as_str))
                .unwrap_or("PENDING")
                .to_string();
            checks.insert(name, outcome);
        }
    }
    Ok(Snapshot {
        state: s("state"),
        title: s("title"),
        head: s("headRefOid").chars().take(7).collect(),
        reviews,
        comments,
        last_commenter,
        checks,
    })
}

/// What changed between two looks, as lines for the subscriber. Empty when
/// nothing did.
pub fn diff(old: &Snapshot, new: &Snapshot) -> Vec<String> {
    let mut lines = Vec::new();
    if old.state != new.state {
        lines.push(format!("state: {} → {}", old.state, new.state));
    }
    if old.title != new.title {
        lines.push(format!("title: {:?} → {:?}", old.title, new.title));
    }
    if old.head != new.head && !new.head.is_empty() {
        lines.push(format!("new commits: head is now {}", new.head));
    }
    if new.reviews.len() > old.reviews.len() {
        for r in &new.reviews[old.reviews.len()..] {
            lines.push(format!("review: {}", r.replace(':', " ")));
        }
    }
    if new.comments > old.comments {
        lines.push(format!(
            "{} new comment{}{}",
            new.comments - old.comments,
            if new.comments - old.comments == 1 {
                ""
            } else {
                "s"
            },
            if new.last_commenter.is_empty() {
                String::new()
            } else {
                format!(", last by {}", new.last_commenter)
            }
        ));
    }
    for (name, outcome) in &new.checks {
        match old.checks.get(name) {
            Some(prev) if prev == outcome => {}
            Some(prev) => lines.push(format!("check {name}: {prev} → {outcome}")),
            None => lines.push(format!("check {name}: {outcome}")),
        }
    }
    lines
}

/// The poller. Runs for the life of the kernel; sleeps when there is
/// nothing to follow.
pub fn spawn_poller(hooks: Arc<KernelHooks>) {
    let period = std::env::var("ARBOS_GITHUB_POLL_S")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(POLL_S)
        .max(MIN_POLL_S);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(period));
        loop {
            tick.tick().await;
            let hooks = Arc::clone(&hooks);
            let _ = tokio::task::spawn_blocking(move || poll_once(&hooks)).await;
        }
    });
}

/// One pass over every subscription. Errors are recorded once per
/// subscription and cleared when a poll succeeds again.
pub fn poll_once(hooks: &KernelHooks) -> usize {
    let place = hooks.place.path().to_path_buf();
    let mut file = match File::load(&place) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("github: {e:#}");
            return 0;
        }
    };
    if file.subscriptions.is_empty() {
        return 0;
    }
    let mut woke = 0;
    let mut dirty = false;
    for sub in &mut file.subscriptions {
        match snapshot(&sub.repo, sub.pr) {
            Ok(now) => {
                if sub.error.take().is_some() {
                    dirty = true;
                }
                let lines = match &sub.last {
                    Some(prev) => diff(prev, &now),
                    // First look: remember, say nothing.
                    None => Vec::new(),
                };
                if sub.last.as_ref() != Some(&now) {
                    sub.last = Some(now.clone());
                    dirty = true;
                }
                if !lines.is_empty() {
                    let text = format!(
                        "{}#{} ({}): {}{}",
                        sub.repo,
                        sub.pr,
                        now.title,
                        lines.join("; "),
                        if sub.note.is_empty() {
                            String::new()
                        } else {
                            format!(". You asked: {}", sub.note)
                        }
                    );
                    if let Err(e) = deliver(hooks, &sub.agent, &text) {
                        eprintln!("github: deliver to {}: {e:#}", sub.agent);
                    } else {
                        woke += 1;
                    }
                }
            }
            Err(e) => {
                let msg = format!("{e:#}");
                if sub.error.as_deref() != Some(&msg) {
                    eprintln!("github: {}#{}: {msg}", sub.repo, sub.pr);
                    let text = format!(
                        "{}#{}: the subscription cannot be checked: {msg}. It stays until you remove it (subscribe remove {}).",
                        sub.repo, sub.pr, sub.id
                    );
                    let _ = deliver(hooks, &sub.agent, &text);
                    sub.error = Some(msg);
                    dirty = true;
                }
            }
        }
    }
    if dirty {
        if let Err(e) = file.save(&place) {
            eprintln!("github: save subscriptions: {e:#}");
        }
    }
    woke
}

/// A `[github]` line on the subscriber's transcript and a turn for it —
/// what `say mode=request` from an agent named github would do.
fn deliver(hooks: &KernelHooks, agent: &str, text: &str) -> Result<()> {
    append_event(
        &hooks.layout(agent).transcript(),
        &Event::new(EventKind::Say {
            from: "github".into(),
            text: text.to_string(),
        }),
    )?;
    let mut n = Node::inbox(text, "agent:github");
    n.hops = DEFAULT_HOPS;
    hooks.inbox(agent, n)?;
    Ok(())
}

pub struct Subscribe(pub Arc<KernelHooks>);

impl Tool for Subscribe {
    fn name(&self) -> &'static str {
        "subscribe"
    }
    fn schema(&self) -> Value {
        typed_schema(
            "subscribe",
            "Follow a GitHub pull request: you are woken with a [github] message when it changes — a review, a comment, a check result, new commits, a merge or close. add needs repo (owner/name) and pr (number); note says what you intend to do when it changes and is repeated in each message. list shows your subscriptions; remove takes an id. Checks run every minute with the GitHub CLI (gh), using a GH_TOKEN granted with secret use when the kernel has none.",
            &[
                ("action", "add (default), list, or remove.", false, "string"),
                ("repo", "owner/name.", false, "string"),
                ("pr", "Pull request number.", false, "integer"),
                (
                    "note",
                    "What to do when it changes, e.g. \"fix CI if it fails; tell the user when merged\".",
                    false,
                    "string",
                ),
                ("id", "For remove.", false, "integer"),
            ],
        )
    }
    fn plan(&self, _cx: &PlanCx, _args: &Value) -> Result<Plan> {
        Ok(Plan::access(Access::none()))
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        let hooks = Arc::clone(&self.0);
        Box::pin(async move {
            let action = args
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("add")
                .trim()
                .to_ascii_lowercase();
            let repo = args.get("repo").and_then(Value::as_str).map(|s| {
                s.trim()
                    .trim_start_matches("https://github.com/")
                    .trim_end_matches('/')
                    .to_string()
            });
            let pr = args.get("pr").and_then(Value::as_u64);
            let note = args
                .get("note")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            let id = args.get("id").and_then(Value::as_u64);
            let agent = cx.agent.id.to_string();
            let place = hooks.place.path().to_path_buf();
            let text = tokio::task::spawn_blocking(move || -> Result<String> {
                let mut file = File::load(&place)?;
                match action.as_str() {
                    "add" => {
                        let repo = repo.filter(|r| r.matches('/').count() == 1)
                            .ok_or_else(|| anyhow::anyhow!("subscribe add needs repo as owner/name"))?;
                        let pr = pr.ok_or_else(|| anyhow::anyhow!("subscribe add needs pr (a number)"))?;
                        if let Some(existing) = file
                            .subscriptions
                            .iter()
                            .find(|s| s.agent == agent && s.repo == repo && s.pr == pr)
                        {
                            return Ok(format!(
                                "Already following {repo}#{pr} (subscription {}).",
                                existing.id
                            ));
                        }
                        // Look once now: proves gh can see it, and sets the
                        // baseline so the first poll reports only changes.
                        let now = snapshot(&repo, pr).with_context(|| {
                            format!("cannot read {repo}#{pr}; is gh authenticated (GH_TOKEN via secret use), and does the PR exist?")
                        })?;
                        file.next_id += 1;
                        let sid = file.next_id;
                        let summary = format!(
                            "{} · {} · {} review{} · {} comment{} · checks: {}",
                            now.state,
                            now.title,
                            now.reviews.len(),
                            if now.reviews.len() == 1 { "" } else { "s" },
                            now.comments,
                            if now.comments == 1 { "" } else { "s" },
                            if now.checks.is_empty() {
                                "none".to_string()
                            } else {
                                now.checks
                                    .iter()
                                    .map(|(k, v)| format!("{k}={v}"))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            }
                        );
                        file.subscriptions.push(Subscription {
                            id: sid,
                            agent: agent.clone(),
                            repo: repo.clone(),
                            pr,
                            note,
                            last: Some(now),
                            error: None,
                            created_ms: arbos_core::now_ms(),
                        });
                        file.save(&place)?;
                        Ok(format!(
                            "Following {repo}#{pr} as subscription {sid}. Now: {summary}. You will be woken with a [github] message when it changes; end your turn and wait for it rather than polling."
                        ))
                    }
                    "list" => {
                        let mine: Vec<String> = file
                            .subscriptions
                            .iter()
                            .filter(|s| s.agent == agent)
                            .map(|s| {
                                format!(
                                    "{}: {}#{}{}{}",
                                    s.id,
                                    s.repo,
                                    s.pr,
                                    s.last
                                        .as_ref()
                                        .map(|l| format!(" — {} ({})", l.title, l.state))
                                        .unwrap_or_default(),
                                    s.error
                                        .as_ref()
                                        .map(|e| format!(" — last check failed: {e}"))
                                        .unwrap_or_default()
                                )
                            })
                            .collect();
                        Ok(if mine.is_empty() {
                            "No subscriptions.".into()
                        } else {
                            mine.join("\n")
                        })
                    }
                    "remove" => {
                        let id = id.ok_or_else(|| anyhow::anyhow!("subscribe remove needs id"))?;
                        let before = file.subscriptions.len();
                        file.subscriptions
                            .retain(|s| !(s.id == id && s.agent == agent));
                        if file.subscriptions.len() == before {
                            bail!("no subscription {id} of yours");
                        }
                        file.save(&place)?;
                        Ok(format!("Removed subscription {id}."))
                    }
                    other => bail!("subscribe: action must be add, list, or remove, not {other:?}"),
                }
            })
            .await
            .map_err(|e| anyhow::anyhow!("subscribe task: {e}"))??;
            Ok(ToolOut::text(text))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_names_what_changed() {
        let mut a = Snapshot {
            state: "OPEN".into(),
            title: "t".into(),
            head: "abc1234".into(),
            reviews: vec!["ann:COMMENTED".into()],
            comments: 1,
            last_commenter: "ann".into(),
            checks: BTreeMap::from([("build".to_string(), "PENDING".to_string())]),
        };
        let b = Snapshot {
            state: "MERGED".into(),
            head: "def5678".into(),
            reviews: vec!["ann:COMMENTED".into(), "bob:APPROVED".into()],
            comments: 3,
            last_commenter: "bob".into(),
            checks: BTreeMap::from([
                ("build".to_string(), "SUCCESS".to_string()),
                ("lint".to_string(), "FAILURE".to_string()),
            ]),
            ..a.clone()
        };
        let lines = diff(&a, &b);
        assert_eq!(
            lines,
            vec![
                "state: OPEN → MERGED",
                "new commits: head is now def5678",
                "review: bob APPROVED",
                "2 new comments, last by bob",
                "check build: PENDING → SUCCESS",
                "check lint: FAILURE",
            ]
        );
        a = b.clone();
        assert!(diff(&a, &b).is_empty());
    }
}
