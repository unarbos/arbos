//! GitHub through `gh`: one look at a pull request (`snapshot`) and the
//! human lines that describe what changed between two looks (`diff`). The
//! watcher's `github_pr` / `github_ci` subscriptions call these; `gh` runs
//! with the kernel's environment plus any secret granted through the
//! `secret` tool, so a token never touches the transcript.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::process::Command;

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
