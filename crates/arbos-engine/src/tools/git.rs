use anyhow::Result;
use serde_json::Value;
use std::path::Path;
use std::process::Command;

use super::ToolOut;
use crate::access::Access;
use crate::tool::{BoxFuture, Plan, PlanCx, RunCx, Tool, blocking, simple_schema};

pub struct Changes;
pub struct Undo;

impl Tool for Changes {
    fn name(&self) -> &'static str {
        "changes"
    }
    fn schema(&self) -> Value {
        simple_schema("changes", "Show the git checkpoint for this turn.", &[])
    }
    fn plan(&self, cx: &PlanCx, _args: &Value) -> Result<Plan> {
        Ok(Plan::access(Access::read_path(cx.cwd)))
    }
    fn run(&self, cx: RunCx, _args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        blocking(move || changes(&cx.cwd))
    }
}

impl Tool for Undo {
    fn name(&self) -> &'static str {
        "undo"
    }
    fn schema(&self) -> Value {
        simple_schema("undo", "Restore the git checkpoint.", &[])
    }
    fn plan(&self, _cx: &PlanCx, _args: &Value) -> Result<Plan> {
        Ok(Plan::access(Access::exclusive()))
    }
    fn run(&self, cx: RunCx, _args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        blocking(move || undo(&cx.cwd))
    }
}

const TAG: &str = "arbos-checkpoint";

pub fn snapshot(cwd: &Path) -> Result<()> {
    if !cwd.join(".git").exists() {
        return Ok(());
    }
    // HEAD only. `git add -A` + stash on a large place blocked the first
    // token and staged thousands of files. Undo still uses this sha.
    if let Ok(out) = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(cwd)
        .output()
    {
        let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !sha.is_empty() {
            let _ = std::fs::write(cwd.join(".arbos").join("checkpoint"), format!("{sha}\n"));
        }
    }
    Ok(())
}

pub fn changes(cwd: &Path) -> Result<ToolOut> {
    let out = Command::new("git")
        .args(["status", "--short"])
        .current_dir(cwd)
        .output()?;
    let status = String::from_utf8_lossy(&out.stdout).into_owned();
    // On a branch of its own, the first line says where the work stands
    // against the base: the moment an agent looks at its changes is the
    // moment to notice nothing is committed yet.
    let mut body = branch_line(cwd, &status).unwrap_or_default();
    body.push_str(&status);
    let diff = Command::new("git")
        .args(["diff", "HEAD"])
        .current_dir(cwd)
        .output()?;
    body.push_str(&String::from_utf8_lossy(&diff.stdout));
    if body.trim().is_empty() {
        body = "(no changes)\n".into();
    }
    Ok(ToolOut::text(body))
}

/// "branch `fix/x`: 2 uncommitted files, 0 commits ahead of main" when
/// `cwd` is on a branch other than the base. None on the base itself, on a
/// detached HEAD, or outside a repository.
fn branch_line(cwd: &Path, status: &str) -> Option<String> {
    let git = |args: &[&str]| -> Option<String> {
        let out = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"])?;
    if branch.is_empty() || branch == "HEAD" {
        return None;
    }
    let base = base_branch(cwd, &git);
    if branch == base {
        return None;
    }
    let dirty = status.lines().filter(|l| !l.trim().is_empty()).count();
    let ahead = git(&["rev-list", "--count", &format!("{base}..HEAD")]).unwrap_or_default();
    let ahead_text = if ahead.is_empty() {
        format!("(no `{base}` branch to compare with)")
    } else {
        format!(
            "{ahead} commit{} ahead of {base}",
            if ahead == "1" { "" } else { "s" }
        )
    };
    Some(format!(
        "branch `{branch}`: {dirty} uncommitted file{}, {ahead_text}\n",
        if dirty == 1 { "" } else { "s" }
    ))
}

/// The branch work is measured against: `base` in `.arbos/git.toml` when the
/// place (found upward from `cwd`) configures one, else `main`, else
/// `master` when only that exists.
fn base_branch(cwd: &Path, git: &dyn Fn(&[&str]) -> Option<String>) -> String {
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        if let Ok(text) = std::fs::read_to_string(d.join(".arbos").join("git.toml")) {
            let base = text
                .lines()
                .filter_map(|l| l.split_once('='))
                .find(|(k, _)| k.trim() == "base")
                .map(|(_, v)| v.trim().trim_matches('"').to_string())
                .unwrap_or_default();
            if !base.is_empty() {
                return base;
            }
            break;
        }
        dir = d.parent();
    }
    if git(&["rev-parse", "--verify", "--quiet", "refs/heads/main"]).is_some() {
        return "main".into();
    }
    if git(&["rev-parse", "--verify", "--quiet", "refs/heads/master"]).is_some() {
        return "master".into();
    }
    "main".into()
}

pub fn undo(cwd: &Path) -> Result<ToolOut> {
    let mark = cwd.join(".arbos").join("checkpoint");
    if let Ok(sha) = std::fs::read_to_string(&mark) {
        let sha = sha.trim();
        if !sha.is_empty() {
            let st = Command::new("git")
                .args(["reset", "--hard", sha])
                .current_dir(cwd)
                .status()?;
            if st.success() {
                // `.arbos/` holds the agent's own state (transcripts, lock,
                // kernel.json) and is often untracked; it is never the
                // turn's work, so it must survive the clean.
                let _ = Command::new("git")
                    .args(["clean", "-fd", "-e", ".arbos", "-e", ".arbos/**"])
                    .current_dir(cwd)
                    .status();
                return Ok(ToolOut::text(format!("restored {sha}")));
            }
        }
    }
    let st = Command::new("git")
        .args(["stash", "list"])
        .current_dir(cwd)
        .output()?;
    let list = String::from_utf8_lossy(&st.stdout);
    if let Some(line) = list.lines().find(|l| l.contains(TAG)) {
        let name = line.split(':').next().unwrap_or("stash@{0}");
        let _ = Command::new("git")
            .args(["stash", "pop", name])
            .current_dir(cwd)
            .status();
        return Ok(ToolOut::text("restored stash checkpoint"));
    }
    Ok(ToolOut::text("no checkpoint"))
}
