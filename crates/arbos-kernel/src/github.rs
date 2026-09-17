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
    /// check name → the run's page, when known (branch runs). A red
    /// check's line carries it so the subscriber can open the log.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub urls: BTreeMap<String, String>,
}

/// `gh` with the kernel's environment plus the grants the asking agent
/// holds (`env`).
fn gh(args: &[&str], env: &[(String, String)]) -> Result<Value> {
    let mut cmd = Command::new("gh");
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
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
pub fn snapshot(repo: &str, pr: u64, env: &[(String, String)]) -> Result<Snapshot> {
    let v = gh(
        &[
            "pr",
            "view",
            &pr.to_string(),
            "--repo",
            repo,
            "--json",
            "state,title,headRefOid,reviews,comments,statusCheckRollup",
        ],
        env,
    )?;
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
        urls: BTreeMap::new(),
    })
}

/// One look at a branch's workflow runs: the newest run per workflow on
/// the branch's latest commit. `state` is `green`, `red`, or `pending`
/// over those; `head` the commit they ran on.
pub fn branch_snapshot(repo: &str, branch: &str, env: &[(String, String)]) -> Result<Snapshot> {
    let v = gh(
        &[
            "run",
            "list",
            "--repo",
            repo,
            "--branch",
            branch,
            "--limit",
            "30",
            "--json",
            "databaseId,workflowName,name,status,conclusion,headSha,url,createdAt",
        ],
        env,
    )?;
    Ok(runs_snapshot(&v, branch))
}

/// `branch_snapshot` on the JSON `gh run list` returned. Runs come newest
/// first; only those on the newest commit count, one per workflow.
pub fn runs_snapshot(runs: &Value, branch: &str) -> Snapshot {
    let runs = runs.as_array().cloned().unwrap_or_default();
    let str_of = |r: &Value, k: &str| r.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let head = runs
        .first()
        .map(|r| str_of(r, "headSha"))
        .unwrap_or_default();
    let mut checks = BTreeMap::new();
    let mut urls = BTreeMap::new();
    for r in runs.iter().filter(|r| str_of(r, "headSha") == head) {
        let name = {
            let w = str_of(r, "workflowName");
            if w.is_empty() { str_of(r, "name") } else { w }
        };
        if name.is_empty() || checks.contains_key(&name) {
            continue;
        }
        let conclusion = str_of(r, "conclusion");
        let outcome = if conclusion.is_empty() {
            let status = str_of(r, "status");
            if status.is_empty() {
                "pending".to_string()
            } else {
                status
            }
        } else {
            conclusion
        };
        let url = str_of(r, "url");
        if !url.is_empty() {
            urls.insert(name.clone(), url);
        }
        checks.insert(name, outcome);
    }
    let state = if checks.is_empty() {
        "no runs".to_string()
    } else if checks.values().any(|o| {
        matches!(
            o.as_str(),
            "failure" | "timed_out" | "startup_failure" | "action_required"
        )
    }) {
        "red".to_string()
    } else if checks
        .values()
        .all(|o| matches!(o.as_str(), "success" | "skipped" | "neutral"))
    {
        "green".to_string()
    } else {
        "pending".to_string()
    };
    Snapshot {
        state,
        title: branch.to_string(),
        head: head.chars().take(7).collect(),
        reviews: Vec::new(),
        comments: 0,
        last_commenter: String::new(),
        checks,
        urls,
    }
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
        let url = match new.urls.get(name) {
            // A run's page only when there is something to look at.
            Some(u) if !matches!(outcome.as_str(), "success" | "skipped" | "neutral") => {
                format!(" ({u})")
            }
            _ => String::new(),
        };
        match old.checks.get(name) {
            Some(prev) if prev == outcome => {}
            Some(prev) => lines.push(format!("check {name}: {prev} → {outcome}{url}")),
            None => lines.push(format!("check {name}: {outcome}{url}")),
        }
    }
    lines
}

#[cfg(test)]
mod branch_tests {
    use super::*;

    fn runs() -> Value {
        serde_json::json!([
            {"databaseId": 3, "workflowName": "ci", "name": "ci", "status": "completed", "conclusion": "failure", "headSha": "bbbbbbb1", "url": "https://github.com/o/r/actions/runs/3"},
            {"databaseId": 4, "workflowName": "lint", "name": "lint", "status": "in_progress", "conclusion": "", "headSha": "bbbbbbb1", "url": "https://github.com/o/r/actions/runs/4"},
            {"databaseId": 2, "workflowName": "ci", "name": "ci", "status": "completed", "conclusion": "success", "headSha": "aaaaaaa1", "url": "https://github.com/o/r/actions/runs/2"},
            {"databaseId": 1, "workflowName": "lint", "name": "lint", "status": "completed", "conclusion": "success", "headSha": "aaaaaaa1", "url": "https://github.com/o/r/actions/runs/1"}
        ])
    }

    #[test]
    fn only_the_newest_commits_runs_count_one_per_workflow() {
        let now = runs_snapshot(&runs(), "main");
        assert_eq!(now.head, "bbbbbbb");
        assert_eq!(now.state, "red");
        assert_eq!(now.checks["ci"], "failure");
        assert_eq!(now.checks["lint"], "in_progress");
        assert_eq!(now.checks.len(), 2);
        // The earlier commit, all green.
        let old_runs: Value = serde_json::json!(runs().as_array().unwrap()[2..].to_vec());
        let old = runs_snapshot(&old_runs, "main");
        assert_eq!(
            (old.state.as_str(), old.head.as_str()),
            ("green", "aaaaaaa")
        );
        let lines = diff(&old, &now);
        assert!(lines.iter().any(|l| l == "state: green → red"), "{lines:?}");
        assert!(
            lines
                .iter()
                .any(|l| l == "new commits: head is now bbbbbbb"),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l == "check ci: success → failure (https://github.com/o/r/actions/runs/3)"),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("check lint: success → in_progress (")),
            "{lines:?}"
        );
        // Green carries no link.
        let back = diff(&now, &old);
        assert!(
            back.iter().any(|l| l == "check ci: failure → success"),
            "{back:?}"
        );
        assert_eq!(
            runs_snapshot(&serde_json::json!([]), "main").state,
            "no runs"
        );
    }
}

/// One pull request as a repository-wide look records it.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PrRow {
    pub title: String,
    /// `OPEN` | `MERGED` | `CLOSED`.
    pub state: String,
    pub head: String,
    pub author: String,
    /// RFC 3339 when merged / closed, else empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub merged_at: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub closed_at: String,
    /// check name → conclusion, for open pull requests.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub checks: BTreeMap<String, String>,
    pub url: String,
}

/// One look at a repository's pull requests (`github_prs`): the newest
/// `PRS_WINDOW` by update, every state, optionally one author's.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ReposSnapshot {
    /// When the look was taken (ms): a merge or close older than the
    /// previous look is history, not news.
    pub at_ms: i64,
    pub prs: BTreeMap<u64, PrRow>,
}

/// How many pull requests one look covers.
pub const PRS_WINDOW: usize = 100;

pub fn repo_snapshot(
    repo: &str,
    author: Option<&str>,
    env: &[(String, String)],
) -> Result<ReposSnapshot> {
    let mut args: Vec<String> = vec![
        "pr".into(),
        "list".into(),
        "--repo".into(),
        repo.into(),
        "--state".into(),
        "all".into(),
        "--limit".into(),
        PRS_WINDOW.to_string(),
        "--json".into(),
        "number,title,state,headRefOid,author,mergedAt,closedAt,url,statusCheckRollup".into(),
    ];
    if let Some(a) = author.map(str::trim).filter(|a| !a.is_empty()) {
        args.push("--author".into());
        args.push(a.into());
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let v = gh(&refs, env)?;
    Ok(rows_snapshot(&v, arbos_core::now_ms()))
}

/// The snapshot from `gh pr list --json …` output, at `at_ms`.
pub fn rows_snapshot(list: &Value, at_ms: i64) -> ReposSnapshot {
    let mut prs = BTreeMap::new();
    for row in list.as_array().into_iter().flatten() {
        let Some(number) = row.get("number").and_then(Value::as_u64) else {
            continue;
        };
        let s = |k: &str| row.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let mut checks = BTreeMap::new();
        if let Some(rollup) = row.get("statusCheckRollup").and_then(Value::as_array) {
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
        prs.insert(
            number,
            PrRow {
                title: s("title"),
                state: s("state").to_ascii_uppercase(),
                head: s("headRefOid").chars().take(7).collect(),
                author: row
                    .get("author")
                    .and_then(|a| a.get("login"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                merged_at: s("mergedAt"),
                closed_at: s("closedAt"),
                checks,
                url: s("url"),
            },
        );
    }
    ReposSnapshot { at_ms, prs }
}

/// The news between two repository looks, one line each: `opened #N`,
/// `merged #N`, `closed #N`, `new commits on #N`, `check X failed on
/// #N` (and back to green). A pull request that only entered the window
/// because something old about it was touched is not news; a merge or
/// close counts only when it happened after the previous look.
pub fn repo_diff(old: &ReposSnapshot, new: &ReposSnapshot) -> Vec<String> {
    let mut lines = Vec::new();
    let since = old.at_ms;
    let after_prev = |ts: &str| -> bool {
        arbos_core::parse_instant_ms(ts).is_some_and(|t| t >= since - 60_000)
    };
    let label = |n: u64, row: &PrRow| -> String {
        let who = if row.author.is_empty() {
            String::new()
        } else {
            format!(" by {}", row.author)
        };
        format!("#{n} {:?}{who} {}", row.title, row.url)
            .trim_end()
            .to_string()
    };
    for (n, row) in &new.prs {
        match old.prs.get(n) {
            None => match row.state.as_str() {
                "OPEN" => lines.push(format!("opened {}", label(*n, row))),
                "MERGED" if after_prev(&row.merged_at) => {
                    lines.push(format!("merged {}", label(*n, row)))
                }
                "CLOSED" if after_prev(&row.closed_at) => {
                    lines.push(format!("closed {}", label(*n, row)))
                }
                _ => {}
            },
            Some(prev) => {
                if prev.state != row.state {
                    match row.state.as_str() {
                        "MERGED" => lines.push(format!("merged {}", label(*n, row))),
                        "CLOSED" => lines.push(format!("closed {}", label(*n, row))),
                        "OPEN" => lines.push(format!("reopened {}", label(*n, row))),
                        other => lines.push(format!("#{n}: state {} → {other}", prev.state)),
                    }
                } else if row.state == "OPEN" && prev.head != row.head && !row.head.is_empty() {
                    lines.push(format!("new commits on #{n} (head {})", row.head));
                }
                if row.state == "OPEN" {
                    for (name, outcome) in &row.checks {
                        let red = |o: &str| {
                            matches!(
                                o.to_ascii_uppercase().as_str(),
                                "FAILURE" | "ERROR" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED"
                            )
                        };
                        match prev.checks.get(name) {
                            Some(p) if p == outcome => {}
                            Some(p) if red(outcome) => {
                                lines.push(format!("check {name} failed on #{n} ({p} → {outcome})"))
                            }
                            Some(p) if red(p) && outcome.eq_ignore_ascii_case("success") => {
                                lines.push(format!("check {name} green again on #{n}"))
                            }
                            None if red(outcome) => {
                                lines.push(format!("check {name} failed on #{n}"))
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    lines
}

#[cfg(test)]
mod repo_tests {
    use super::{repo_diff, rows_snapshot};
    use serde_json::json;

    fn row(
        n: u64,
        state: &str,
        head: &str,
        merged: &str,
        checks: serde_json::Value,
    ) -> serde_json::Value {
        json!({
            "number": n, "title": format!("PR {n}"), "state": state, "headRefOid": head,
            "author": {"login": "jacob"}, "mergedAt": merged, "closedAt": "",
            "url": format!("https://github.com/o/r/pull/{n}"), "statusCheckRollup": checks
        })
    }

    /// Projects-post gap 2: "follow all my PRs" fires on open, merge, new
    /// commits, and a check going red on any of them — and not on a
    /// pull request merged long before the subscription looked.
    #[test]
    fn the_repository_diff_reports_opens_merges_commits_and_red_checks_only() {
        let t0 = 1_700_000_000_000i64;
        let old = rows_snapshot(
            &json!([
                row(
                    1,
                    "OPEN",
                    "aaaaaaa",
                    "",
                    json!([{"name": "build", "conclusion": "SUCCESS"}])
                ),
                row(2, "OPEN", "bbbbbbb", "", json!([])),
            ]),
            t0,
        );
        let new = rows_snapshot(
            &json!([
                row(
                    1,
                    "OPEN",
                    "aaaaaaa",
                    "",
                    json!([{"name": "build", "conclusion": "FAILURE"}])
                ),
                row(2, "MERGED", "bbbbbbb", "2026-09-15T18:00:00Z", json!([])),
                row(3, "OPEN", "ccccccc", "", json!([])),
                // Merged years ago, touched today (a comment): history.
                row(4, "MERGED", "ddddddd", "2020-01-01T00:00:00Z", json!([])),
            ]),
            t0 + 600_000,
        );
        let lines = repo_diff(&old, &new);
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("check build failed on #1")),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("merged #2 \"PR 2\" by jacob")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.starts_with("opened #3")),
            "{lines:?}"
        );
        assert!(!lines.iter().any(|l| l.contains("#4")), "{lines:?}");
        assert_eq!(lines.len(), 3, "{lines:?}");
        // New commits on an open one.
        let newer = rows_snapshot(
            &json!([row(3, "OPEN", "eeeeeee", "", json!([]))]),
            t0 + 1_200_000,
        );
        let lines = repo_diff(&new, &newer);
        assert_eq!(lines, vec!["new commits on #3 (head eeeeeee)".to_string()]);
    }
}
