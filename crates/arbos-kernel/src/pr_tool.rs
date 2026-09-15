//! `pr`: Cursor's ManagePullRequest and EditPullRequestLabels, over `gh`.
//!
//! One tool, one `action`: `create` (draft by default, the repository's
//! PR template folded in, local artifact paths in the body uploaded to
//! an `arbos-artifacts` branch and rewritten to URLs the PR can show),
//! `update` (title, body, base, `draft:false` marks ready), `comment`
//! (top-level, a reply with `in_reply_to`, or on a file line with `path`
//! and `line`), `resolve` (the review thread of a comment), `ci` (the
//! checks), `status` (`open` | `closed`), `labels` (add, remove),
//! `template` (read the repository's template). A created PR is recorded
//! in `prs.jsonl` and followed by a `github_pr` and a `github_ci`
//! subscription, as a `gh pr create` from bash is.
//!
//! `gh` is the one dependency: it holds the token, and its refusals are
//! the tool's. Tests point `PATH` at a stand-in.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use arbos_engine::{Access, BoxFuture, Plan, PlanCx, RunCx, Tool, ToolOut};
use serde_json::{Value, json};

use crate::hooks::KernelHooks;

pub struct Pr(pub Arc<KernelHooks>);

/// Where the repository keeps its template, in the order Cursor looks.
const TEMPLATES: &[&str] = &[
    ".github/PULL_REQUEST_TEMPLATE.md",
    ".github/pull_request_template.md",
    "PULL_REQUEST_TEMPLATE.md",
    "pull_request_template.md",
    "docs/PULL_REQUEST_TEMPLATE.md",
];

/// The branch artifacts are pushed to, so a PR body can show them.
pub const ARTIFACTS_BRANCH: &str = "arbos-artifacts";

impl Tool for Pr {
    fn name(&self) -> &'static str {
        "pr"
    }
    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "pr",
                "description": "Pull requests through gh. create (title, body, branch, base; draft unless draft:false; the repo's PR template is folded into the body; local images and files the body links — media/…, docs/… — are uploaded to an arbos-artifacts branch and the links rewritten so the PR shows them). update (pr; title, body, base; draft:false marks it ready). comment (pr, body; in_reply_to a comment id to reply; path + line for a line comment). resolve (pr, comment_id: resolves that review thread). ci (pr: the checks). status (pr, status open|closed). labels (pr; add[], remove[]). template: the repo's PR template. Open, close, and comment only when the user asked; never merge.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": {"type": "string", "enum": ["create", "update", "comment", "resolve", "ci", "status", "labels", "template"]},
                        "pr": {"type": "string", "description": "PR number or URL (all but create/template)."},
                        "title": {"type": "string"},
                        "body": {"type": "string", "description": "create/update: the description; comment: the text."},
                        "branch": {"type": "string", "description": "create: the head branch (default: the checkout's)."},
                        "base": {"type": "string", "description": "create/update: the base branch."},
                        "draft": {"type": "boolean", "description": "create: default true. update: false marks ready."},
                        "in_reply_to": {"type": "string", "description": "comment: a review comment id to reply to."},
                        "path": {"type": "string", "description": "comment: file for a line comment."},
                        "line": {"type": "integer", "description": "comment: the line (end of a range)."},
                        "start_line": {"type": "integer"},
                        "side": {"type": "string", "enum": ["LEFT", "RIGHT"]},
                        "comment_id": {"type": "string", "description": "resolve: the review comment whose thread to resolve."},
                        "status": {"type": "string", "enum": ["open", "closed"]},
                        "add": {"type": "array", "items": {"type": "string"}, "description": "labels to add."},
                        "remove": {"type": "array", "items": {"type": "string"}, "description": "labels to remove."}
                    },
                    "required": ["action"]
                }
            }
        })
    }
    fn plan(&self, _cx: &PlanCx, _args: &Value) -> Result<Plan> {
        // Talks to GitHub and may push a branch: one at a time.
        Ok(Plan::access(Access::exclusive()))
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        let hooks = Arc::clone(&self.0);
        Box::pin(async move {
            let action = str_arg(&args, "action").trim().to_ascii_lowercase();
            let agent = cx.agent.clone();
            let cwd = agent.work_dir(hooks.place.path());
            let place = hooks.place.clone();
            let out = tokio::task::spawn_blocking(move || match action.as_str() {
                "create" | "create_pr" => create(&place, &cwd, &agent.id.to_string(), &args),
                "update" | "update_pr" | "ready" => update(&cwd, &args),
                "comment" | "post_comment" => comment(&cwd, &args),
                "resolve" | "resolve_comment" => resolve(&cwd, &args),
                "ci" | "checks" | "get_ci_status" => ci(&cwd, &args),
                "status" | "set_pr_status" | "close" | "reopen" => status(&cwd, &args),
                "labels" => labels(&cwd, &args),
                "template" => template_op(&cwd),
                other => bail!(
                    "pr: action must be create, update, comment, resolve, ci, status, labels, or template, not {other:?}"
                ),
            })
            .await
            .map_err(|e| anyhow::anyhow!("pr task: {e}"))??;
            // A created PR is followed like one from `gh pr create` in bash.
            if let Some(url) = out.paths.iter().find(|p| p.contains("/pull/")) {
                let opened: Vec<arbos_core::PrRec> = arbos_core::load_prs(&hooks.place)
                    .into_iter()
                    .filter(|p| p.url == *url)
                    .collect();
                if !opened.is_empty() {
                    crate::serve::follow_prs(&hooks, cx.agent.id.as_str(), &opened);
                }
            }
            Ok(out)
        })
    }
}

fn str_arg<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or("")
}

fn opt(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// `gh` with arguments, in `cwd`; stdout on success, the stderr as the
/// error otherwise.
fn gh(cwd: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("gh")
        .args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .output()
        .with_context(|| "run gh (is the GitHub CLI installed and signed in?)")?;
    if !out.status.success() {
        bail!(
            "gh {}: {}",
            args.first().copied().unwrap_or(""),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .output()?;
    if !out.status.success() {
        bail!(
            "git {}: {}",
            args.first().copied().unwrap_or(""),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `owner/name` of the repository `cwd` is in, from gh.
fn name_with_owner(cwd: &Path) -> Result<String> {
    let v: Value = serde_json::from_str(&gh(cwd, &["repo", "view", "--json", "nameWithOwner"])?)
        .context("gh repo view")?;
    v.get("nameWithOwner")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("gh repo view said nothing about the repository"))
}

/// The repository's PR template text, if it has one.
fn template(cwd: &Path) -> Option<(PathBuf, String)> {
    let root = git(cwd, &["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .unwrap_or_else(|_| cwd.to_path_buf());
    for name in TEMPLATES {
        let p = root.join(name);
        if let Ok(text) = std::fs::read_to_string(&p) {
            return Some((p, text));
        }
    }
    let dir = root.join(".github").join("PULL_REQUEST_TEMPLATE");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    files.sort();
    files
        .into_iter()
        .next()
        .and_then(|p| std::fs::read_to_string(&p).ok().map(|t| (p, t)))
}

fn template_op(cwd: &Path) -> Result<ToolOut> {
    match template(cwd) {
        Some((p, text)) => Ok(ToolOut::with_paths(
            format!("{}\n{text}", p.display()),
            vec![p.display().to_string()],
        )),
        None => Ok(ToolOut::text("no PR template in this repository")),
    }
}

/// The body with the template folded in when the body does not already
/// carry the template's first heading (Cursor "honours PR templates").
fn with_template(cwd: &Path, body: &str) -> String {
    let Some((_, tpl)) = template(cwd) else {
        return body.to_string();
    };
    let first_heading = tpl
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with('#'))
        .map(|l| l.trim_start_matches('#').trim().to_string());
    match first_heading {
        Some(h) if !h.is_empty() && body.contains(&h) => body.to_string(),
        _ => format!("{}\n\n{}", body.trim_end(), tpl.trim()),
    }
}

/// Local paths the body links (`[x](p)`, `![x](p)`), resolved against the
/// store and the place, that exist.
fn local_links(place: &arbos_core::Place, cwd: &Path, body: &str) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(open) = rest.find("](") {
        let after = &rest[open + 2..];
        let Some(close) = after.find(')') else {
            break;
        };
        let raw = after[..close]
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim();
        rest = &after[close + 1..];
        if raw.is_empty()
            || raw.contains("://")
            || raw.starts_with('#')
            || raw.starts_with("mailto:")
        {
            continue;
        }
        let clean = raw.trim_start_matches("./");
        let candidates = [
            PathBuf::from(raw),
            place.arbos().join(clean),
            place.path.join(clean),
            cwd.join(clean),
        ];
        if let Some(p) = candidates.into_iter().find(|c| c.is_file())
            && !out.iter().any(|(r, _): &(String, PathBuf)| r == raw)
        {
            out.push((raw.to_string(), p));
        }
    }
    out
}

/// Push the linked files to the `arbos-artifacts` branch and return the
/// body with each link rewritten to the raw URL. A repository with no
/// `origin`, or a push that fails, leaves the body as it was and says so.
fn upload_artifacts(
    place: &arbos_core::Place,
    cwd: &Path,
    agent: &str,
    body: &str,
    repo: &str,
) -> (String, Vec<String>, Option<String>) {
    let links = local_links(place, cwd, body);
    if links.is_empty() {
        return (body.to_string(), Vec::new(), None);
    }
    let stamp = arbos_core::now_ms();
    let staging = std::env::temp_dir().join(format!("arbos-artifacts-{agent}-{stamp}"));
    let result = (|| -> Result<Vec<(String, String)>> {
        let _ = git(cwd, &["fetch", "origin", ARTIFACTS_BRANCH]).ok();
        let remote_has = git(
            cwd,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/remotes/origin/{ARTIFACTS_BRANCH}"),
            ],
        )
        .is_ok();
        let _ = git(cwd, &["worktree", "prune"]);
        if remote_has {
            git(
                cwd,
                &[
                    "worktree",
                    "add",
                    "--detach",
                    staging.to_str().unwrap_or_default(),
                    &format!("origin/{ARTIFACTS_BRANCH}"),
                ],
            )?;
        } else {
            git(
                cwd,
                &[
                    "worktree",
                    "add",
                    "--detach",
                    staging.to_str().unwrap_or_default(),
                    "HEAD",
                ],
            )?;
            git(&staging, &["checkout", "-q", "--orphan", ARTIFACTS_BRANCH])?;
            git(&staging, &["rm", "-rfq", "."])
                .or_else(|_| Ok::<String, anyhow::Error>(String::new()))?;
            // A fresh orphan may still have untracked leftovers.
            for e in std::fs::read_dir(&staging).into_iter().flatten().flatten() {
                if e.file_name() != ".git" {
                    let p = e.path();
                    if p.is_dir() {
                        let _ = std::fs::remove_dir_all(&p);
                    } else {
                        let _ = std::fs::remove_file(&p);
                    }
                }
            }
        }
        let mut rewritten = Vec::new();
        for (raw, file) in &links {
            let name = file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "artifact".into());
            let rel = format!("{agent}/{stamp}/{name}");
            let dest = staging.join(&rel);
            std::fs::create_dir_all(dest.parent().expect("parent"))?;
            std::fs::copy(file, &dest)?;
            git(&staging, &["add", &rel])?;
            rewritten.push((
                raw.clone(),
                format!("https://raw.githubusercontent.com/{repo}/{ARTIFACTS_BRANCH}/{rel}"),
            ));
        }
        git(
            &staging,
            &[
                "-c",
                "user.name=arbos",
                "-c",
                "user.email=arbos@localhost",
                "commit",
                "-q",
                "-m",
                &format!("artifacts from {agent}"),
            ],
        )?;
        git(
            &staging,
            &["push", "-q", "origin", &format!("HEAD:{ARTIFACTS_BRANCH}")],
        )?;
        Ok(rewritten)
    })();
    let _ = git(
        cwd,
        &[
            "worktree",
            "remove",
            "--force",
            staging.to_str().unwrap_or_default(),
        ],
    );
    let _ = std::fs::remove_dir_all(&staging);
    match result {
        Ok(rewritten) => {
            let mut out = body.to_string();
            let mut urls = Vec::new();
            for (raw, url) in rewritten {
                out = out.replace(&format!("]({raw})"), &format!("]({url})"));
                urls.push(url);
            }
            (out, urls, None)
        }
        Err(e) => (
            body.to_string(),
            Vec::new(),
            Some(format!(
                "artifacts not uploaded ({e:#}); the body keeps local paths the PR cannot show"
            )),
        ),
    }
}

fn create(place: &arbos_core::Place, cwd: &Path, agent: &str, args: &Value) -> Result<ToolOut> {
    let title =
        opt(args, "title").ok_or_else(|| anyhow::anyhow!("pr create: title is required"))?;
    let body = opt(args, "body").unwrap_or_default();
    // A repository with no remote has nothing to open a PR against; said
    // here in plain words, before gh's "no git remotes" (cold-p5, where
    // a worker then reported an invented PR link).
    if git(cwd, &["remote"])
        .map(|s| s.trim().is_empty())
        .unwrap_or(false)
    {
        let branch = git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
        bail!(
            "pr create: this repository has no remote, so there is no pull request to open. The work is on branch {} in this checkout only — report it as local, do not report a PR (git remote add origin <url> && git push -u origin {} would make one possible).",
            if branch.is_empty() {
                "(unknown)".to_string()
            } else {
                branch.clone()
            },
            if branch.is_empty() {
                "<branch>".to_string()
            } else {
                branch
            }
        );
    }
    let repo = name_with_owner(cwd)?;
    let body = with_template(cwd, &body);
    let (body, uploaded, upload_note) = upload_artifacts(place, cwd, agent, &body, &repo);
    let body_file =
        std::env::temp_dir().join(format!("arbos-pr-body-{agent}-{}.md", arbos_core::now_ms()));
    std::fs::write(&body_file, &body)?;
    let mut gh_args: Vec<String> = vec![
        "pr".into(),
        "create".into(),
        "--title".into(),
        title.clone(),
        "--body-file".into(),
        body_file.display().to_string(),
    ];
    if let Some(b) = opt(args, "branch") {
        gh_args.push("--head".into());
        gh_args.push(b);
    }
    if let Some(b) = opt(args, "base") {
        gh_args.push("--base".into());
        gh_args.push(b);
    }
    let draft = args.get("draft").and_then(Value::as_bool).unwrap_or(true);
    if draft {
        gh_args.push("--draft".into());
    }
    let refs: Vec<&str> = gh_args.iter().map(String::as_str).collect();
    let out = gh(cwd, &refs);
    let _ = std::fs::remove_file(&body_file);
    let out = out?;
    let mut paths = Vec::new();
    let mut lines = vec![format!(
        "Opened {}{}: {out}",
        if draft { "draft " } else { "" },
        title
    )];
    for (url, repo_of, number) in arbos_core::prs::pr_urls(&out) {
        let rec = arbos_core::PrRec {
            ts: arbos_core::now_ms(),
            agent: agent.to_string(),
            url: url.clone(),
            repo: repo_of,
            number,
            branch: opt(args, "branch").unwrap_or_default(),
        };
        let _ = arbos_core::record_pr(place, &rec);
        paths.push(url);
    }
    if !uploaded.is_empty() {
        lines.push(format!(
            "Uploaded {} artifact(s) to {ARTIFACTS_BRANCH}; the body links them by URL.",
            uploaded.len()
        ));
    }
    if let Some(n) = upload_note {
        lines.push(n);
    }
    if paths.is_empty() {
        lines.push("(gh printed no PR URL)".into());
    } else {
        lines.push("It is followed: a review comment or a check change wakes you.".into());
    }
    Ok(ToolOut::with_paths(lines.join("\n"), paths))
}

fn pr_ref(args: &Value) -> Result<String> {
    opt(args, "pr").ok_or_else(|| anyhow::anyhow!("pr: pr (number or URL) is required"))
}

fn update(cwd: &Path, args: &Value) -> Result<ToolOut> {
    let pr = pr_ref(args)?;
    let mut done = Vec::new();
    let mut edit: Vec<String> = vec!["pr".into(), "edit".into(), pr.clone()];
    let mut body_file = None;
    if let Some(t) = opt(args, "title") {
        edit.push("--title".into());
        edit.push(t);
    }
    if let Some(b) = opt(args, "body") {
        let f = std::env::temp_dir().join(format!("arbos-pr-body-{}.md", arbos_core::now_ms()));
        std::fs::write(&f, b)?;
        edit.push("--body-file".into());
        edit.push(f.display().to_string());
        body_file = Some(f);
    }
    if let Some(b) = opt(args, "base") {
        edit.push("--base".into());
        edit.push(b);
    }
    if edit.len() > 3 {
        let refs: Vec<&str> = edit.iter().map(String::as_str).collect();
        let r = gh(cwd, &refs);
        if let Some(f) = body_file {
            let _ = std::fs::remove_file(f);
        }
        r?;
        done.push("edited");
    }
    match args.get("draft").and_then(Value::as_bool) {
        Some(false) => {
            gh(cwd, &["pr", "ready", &pr])?;
            done.push("marked ready for review");
        }
        Some(true) => {
            gh(cwd, &["pr", "ready", "--undo", &pr])?;
            done.push("back to draft");
        }
        None => {}
    }
    if done.is_empty() {
        bail!("pr update: nothing to change (title, body, base, or draft)");
    }
    Ok(ToolOut::text(format!("PR {pr}: {}.", done.join(", "))))
}

fn comment(cwd: &Path, args: &Value) -> Result<ToolOut> {
    let pr = pr_ref(args)?;
    let body = opt(args, "body").ok_or_else(|| anyhow::anyhow!("pr comment: body is required"))?;
    let number = pr_number(&pr)?;
    let repo = name_with_owner(cwd)?;
    if let Some(reply_to) = opt(args, "in_reply_to") {
        let out = gh(
            cwd,
            &[
                "api",
                "--method",
                "POST",
                &format!("repos/{repo}/pulls/{number}/comments/{reply_to}/replies"),
                "-f",
                &format!("body={body}"),
                "--jq",
                ".html_url",
            ],
        )?;
        return Ok(ToolOut::with_paths(
            format!("Replied to comment {reply_to}: {out}"),
            vec![out],
        ));
    }
    if let Some(path) = opt(args, "path") {
        let line = args
            .get("line")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("pr comment: a line comment needs line"))?;
        let head = gh(
            cwd,
            &[
                "pr",
                "view",
                &pr,
                "--json",
                "headRefOid",
                "--jq",
                ".headRefOid",
            ],
        )?;
        let side = opt(args, "side").unwrap_or_else(|| "RIGHT".into());
        let mut api: Vec<String> = vec![
            "api".into(),
            "--method".into(),
            "POST".into(),
            format!("repos/{repo}/pulls/{number}/comments"),
            "-f".into(),
            format!("body={body}"),
            "-f".into(),
            format!("commit_id={head}"),
            "-f".into(),
            format!("path={path}"),
            "-F".into(),
            format!("line={line}"),
            "-f".into(),
            format!("side={side}"),
        ];
        if let Some(start) = args.get("start_line").and_then(Value::as_u64) {
            api.push("-F".into());
            api.push(format!("start_line={start}"));
            api.push("-f".into());
            api.push(format!("start_side={side}"));
        }
        api.push("--jq".into());
        api.push(".html_url".into());
        let refs: Vec<&str> = api.iter().map(String::as_str).collect();
        let out = gh(cwd, &refs)?;
        return Ok(ToolOut::with_paths(
            format!("Commented on {path}:{line}: {out}"),
            vec![out],
        ));
    }
    let out = gh(cwd, &["pr", "comment", &pr, "--body", &body])?;
    Ok(ToolOut::with_paths(format!("Commented: {out}"), vec![out]))
}

fn resolve(cwd: &Path, args: &Value) -> Result<ToolOut> {
    let pr = pr_ref(args)?;
    let comment_id = opt(args, "comment_id")
        .ok_or_else(|| anyhow::anyhow!("pr resolve: comment_id is required"))?;
    let number = pr_number(&pr)?;
    let repo = name_with_owner(cwd)?;
    let (owner, name) = repo
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("repository {repo:?} is not owner/name"))?;
    let query = "query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){pullRequest(number:$number){reviewThreads(first:100){nodes{id isResolved comments(first:50){nodes{databaseId}}}}}}}";
    let out = gh(
        cwd,
        &[
            "api",
            "graphql",
            "-f",
            &format!("query={query}"),
            "-f",
            &format!("owner={owner}"),
            "-f",
            &format!("name={name}"),
            "-F",
            &format!("number={number}"),
        ],
    )?;
    let v: Value = serde_json::from_str(&out).context("graphql reply")?;
    let threads = v
        .pointer("/data/repository/pullRequest/reviewThreads/nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let thread = threads.iter().find(|t| {
        t.pointer("/comments/nodes")
            .and_then(Value::as_array)
            .is_some_and(|cs| {
                cs.iter().any(|c| {
                    c.get("databaseId")
                        .map(|d| d.to_string() == comment_id)
                        .unwrap_or(false)
                })
            })
    });
    let Some(thread) = thread else {
        bail!("pr resolve: no review thread holds comment {comment_id} on PR {number}");
    };
    if thread.get("isResolved").and_then(Value::as_bool) == Some(true) {
        return Ok(ToolOut::text(format!(
            "Thread of comment {comment_id} was already resolved."
        )));
    }
    let thread_id = thread
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("thread has no id"))?;
    let mutation =
        "mutation($id:ID!){resolveReviewThread(input:{threadId:$id}){thread{isResolved}}}";
    gh(
        cwd,
        &[
            "api",
            "graphql",
            "-f",
            &format!("query={mutation}"),
            "-f",
            &format!("id={thread_id}"),
        ],
    )?;
    Ok(ToolOut::text(format!(
        "Resolved the thread of comment {comment_id}."
    )))
}

fn ci(cwd: &Path, args: &Value) -> Result<ToolOut> {
    let pr = pr_ref(args)?;
    let out = Command::new("gh")
        .args(["pr", "checks", &pr])
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .output()
        .context("run gh")?;
    // `gh pr checks` exits 8 when checks are pending and 1 when one failed;
    // the listing is the answer either way.
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if text.is_empty() && !err.is_empty() && !out.status.success() {
        bail!("gh pr checks: {err}");
    }
    let verdict = match out.status.code() {
        Some(0) => "all checks passed",
        Some(8) => "checks still running",
        Some(1) => "a check failed",
        _ => "checks",
    };
    Ok(ToolOut::text(format!("PR {pr}: {verdict}\n{text}")))
}

fn status(cwd: &Path, args: &Value) -> Result<ToolOut> {
    let pr = pr_ref(args)?;
    let want = opt(args, "status")
        .or_else(|| match str_arg(args, "action") {
            "close" => Some("closed".into()),
            "reopen" => Some("open".into()),
            _ => None,
        })
        .ok_or_else(|| anyhow::anyhow!("pr status: status must be open or closed"))?;
    match want.to_ascii_lowercase().as_str() {
        "closed" | "close" => {
            gh(cwd, &["pr", "close", &pr])?;
            Ok(ToolOut::text(format!("PR {pr} closed (not merged).")))
        }
        "open" | "reopen" => {
            gh(cwd, &["pr", "reopen", &pr])?;
            Ok(ToolOut::text(format!("PR {pr} reopened.")))
        }
        other => bail!("pr status: status must be open or closed, not {other:?}"),
    }
}

fn labels(cwd: &Path, args: &Value) -> Result<ToolOut> {
    let pr = pr_ref(args)?;
    let list = |key: &str| -> Vec<String> {
        args.get(key)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    let add = list("add");
    let remove = list("remove");
    if add.is_empty() && remove.is_empty() {
        bail!("pr labels: add or remove at least one label");
    }
    let mut a: Vec<String> = vec!["pr".into(), "edit".into(), pr.clone()];
    if !add.is_empty() {
        a.push("--add-label".into());
        a.push(add.join(","));
    }
    if !remove.is_empty() {
        a.push("--remove-label".into());
        a.push(remove.join(","));
    }
    let refs: Vec<&str> = a.iter().map(String::as_str).collect();
    gh(cwd, &refs)?;
    Ok(ToolOut::text(format!(
        "PR {pr}: labels{}{}.",
        if add.is_empty() {
            String::new()
        } else {
            format!(" +{}", add.join(", "))
        },
        if remove.is_empty() {
            String::new()
        } else {
            format!(" -{}", remove.join(", "))
        }
    )))
}

/// The number in `123`, `#123`, or a PR URL.
fn pr_number(pr: &str) -> Result<u64> {
    let tail = pr.trim().trim_start_matches('#');
    let digits: String = tail
        .rsplit('/')
        .next()
        .unwrap_or(tail)
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits
        .parse()
        .with_context(|| format!("pr: {pr:?} is not a PR number or URL"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_numbers_come_from_numbers_hashes_and_urls() {
        assert_eq!(pr_number("42").unwrap(), 42);
        assert_eq!(pr_number("#42").unwrap(), 42);
        assert_eq!(pr_number("https://github.com/o/r/pull/42").unwrap(), 42);
        assert!(pr_number("main").is_err());
    }

    #[test]
    fn the_template_is_folded_in_once() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert!(git(root, &["init", "-q"]).is_ok());
        std::fs::create_dir_all(root.join(".github")).unwrap();
        std::fs::write(
            root.join(".github/PULL_REQUEST_TEMPLATE.md"),
            "## Summary\n\n## Test plan\n",
        )
        .unwrap();
        let folded = with_template(root, "Adds the gate.");
        assert!(
            folded.starts_with("Adds the gate.\n\n## Summary"),
            "{folded}"
        );
        let kept = with_template(root, "## Summary\nAdds the gate.\n## Test plan\nran it");
        assert!(!kept.contains("## Summary\n\n## Test plan"), "{kept}");
    }

    #[test]
    fn only_existing_local_files_count_as_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        let place = arbos_core::Place::new(dir.path());
        std::fs::create_dir_all(dir.path().join(".arbos/media/layout")).unwrap();
        std::fs::write(dir.path().join(".arbos/media/layout/a.png"), b"png").unwrap();
        let body = "![panel](media/layout/a.png) and [gone](media/layout/b.png) and [web](https://x/y) and [doc](.arbos/media/layout/a.png)";
        let links = local_links(&place, dir.path(), body);
        let raws: Vec<&str> = links.iter().map(|(r, _)| r.as_str()).collect();
        assert_eq!(
            raws,
            vec!["media/layout/a.png", ".arbos/media/layout/a.png"]
        );
    }
}
