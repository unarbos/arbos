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

/// One turn's starting point, for `arbos-kernel rewind`: the transcript
/// line the turn began on, HEAD, and a commit holding the working tree as
/// it was, untracked files included (None when the tree matched HEAD).
/// The commit is kept alive by `refs/arbos/cp/<agent>/<line>`. One JSON
/// line per turn in `<agent dir>/checkpoints.jsonl`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Checkpoint {
    pub line: u64,
    pub ts: i64,
    pub head: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<String>,
}

/// Record where a turn starts: the plain HEAD mark `undo` uses, plus a
/// checkpoint of the working tree for `rewind`. Runs on the blocking pool.
pub fn snapshot_turn(cwd: &Path, agent_dir: &Path, agent: &str, line: u64) -> Result<()> {
    snapshot(cwd)?;
    if !cwd.join(".git").exists() {
        return Ok(());
    }
    let head = git_out(cwd, &["rev-parse", "HEAD"]).unwrap_or_default();
    if head.is_empty() {
        return Ok(());
    }
    let work = work_commit(cwd, &head);
    if let Some(w) = &work {
        let safe: String = agent
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let _ = Command::new("git")
            .args(["update-ref", &format!("refs/arbos/cp/{safe}/{line}"), w])
            .current_dir(cwd)
            .status();
    }
    let cp = Checkpoint {
        line,
        ts: arbos_core::now_ms(),
        head,
        work,
    };
    let path = agent_dir.join("checkpoints.jsonl");
    let mut text = serde_json::to_string(&cp)?;
    text.push('\n');
    use std::io::Write;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(text.as_bytes())?;
    Ok(())
}

/// A commit whose tree is the working tree as it stands — tracked
/// changes and untracked files alike, ignored files and `.arbos/` left
/// out — parented on HEAD so `read-tree` can bring it all back. Built
/// through a scratch index copied from the real one (so the add is
/// incremental) and never touching the real index or the branch. `None`
/// when the tree equals HEAD's.
fn work_commit(cwd: &Path, head: &str) -> Option<String> {
    let index = git_out(cwd, &["rev-parse", "--git-path", "index"])?;
    let index = cwd.join(index);
    let scratch = cwd
        .join(".arbos")
        .join(format!("index-scratch-{}", std::process::id()));
    let _ = std::fs::create_dir_all(cwd.join(".arbos"));
    if index.exists() {
        std::fs::copy(&index, &scratch).ok()?;
    }
    let run = |args: &[&str]| -> Option<String> {
        let out = Command::new("git")
            .args(args)
            .env("GIT_INDEX_FILE", &scratch)
            .current_dir(cwd)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let result = (|| {
        run(&["add", "-A", "--", "."])?;
        // `.arbos/` is the agent's own state, never part of the project's
        // checkpoint; it is dropped from the scratch index when not ignored.
        let _ = run(&[
            "rm",
            "-r",
            "-q",
            "--cached",
            "--ignore-unmatch",
            "--",
            ".arbos",
        ]);
        let tree = run(&["write-tree"])?;
        let head_tree = git_out(cwd, &["rev-parse", &format!("{head}^{{tree}}")])?;
        if tree == head_tree {
            return None;
        }
        git_out(
            cwd,
            &["commit-tree", &tree, "-p", head, "-m", "arbos checkpoint"],
        )
    })();
    let _ = std::fs::remove_file(&scratch);
    result
}

fn git_out(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Every checkpoint of an agent, oldest first.
pub fn checkpoints(agent_dir: &Path) -> Vec<Checkpoint> {
    std::fs::read_to_string(agent_dir.join("checkpoints.jsonl"))
        .map(|t| {
            t.lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Put the working tree back to a checkpoint: HEAD to its commit, tracked
/// files to the saved tree (or to HEAD when the tree was clean), untracked
/// files from after it removed — never `.arbos/`.
pub fn restore(cwd: &Path, cp: &Checkpoint) -> Result<String> {
    // The agent's own state must not be part of what comes back: a
    // `.arbos/` tracked by the project repo would be reset to an old
    // transcript under a running kernel. Its own repo (the F design's
    // Phase 1) is the real fix; until then, refuse.
    if git_out(cwd, &["ls-files", "--", ".arbos"]).is_some_and(|l| !l.is_empty()) {
        anyhow::bail!(
            ".arbos/ is tracked by the project repository; run `git rm -r --cached .arbos` (and add .arbos to .gitignore) before rewinding files"
        );
    }
    // Order matters: HEAD back first, then everything untracked that the
    // later turns added goes (never `.arbos/`), then the checkpoint's tree
    // — tracked changes and the untracked files of that moment — comes
    // back, and the index returns to HEAD so it all shows as it did.
    let st = Command::new("git")
        .args(["reset", "--hard", &cp.head])
        .current_dir(cwd)
        .status()?;
    if !st.success() {
        anyhow::bail!("git reset --hard {} failed", cp.head);
    }
    let _ = Command::new("git")
        .args(["clean", "-fd", "-e", ".arbos", "-e", ".arbos/**"])
        .current_dir(cwd)
        .status();
    if let Some(work) = &cp.work {
        let st = Command::new("git")
            .args(["read-tree", "-u", "--reset", work])
            .current_dir(cwd)
            .status()?;
        if !st.success() {
            anyhow::bail!("git read-tree {work} failed");
        }
        let _ = Command::new("git")
            .args(["reset", "-q"])
            .current_dir(cwd)
            .status();
    }
    Ok(match &cp.work {
        Some(w) => format!(
            "{} + working tree {}",
            &cp.head[..cp.head.len().min(12)],
            &w[..w.len().min(12)]
        ),
        None => cp.head[..cp.head.len().min(12)].to_string(),
    })
}

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
            let runtime = cwd.join(".arbos").join("runtime");
            let _ = std::fs::create_dir_all(&runtime);
            let _ = std::fs::write(runtime.join("checkpoint"), format!("{sha}\n"));
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
    if let Some(note) = test_files_note(&status) {
        body.push_str(&note);
    }
    Ok(ToolOut::text(body))
}

/// A line naming the existing test files the working tree changes, so the
/// agent sees when it is editing the spec (SWE-bench: two of four losses
/// were a rewritten or loosened test). New test files are not the point.
fn test_files_note(status: &str) -> Option<String> {
    let touched: Vec<&str> = status
        .lines()
        .filter(|l| l.len() > 3 && !l.starts_with("??") && !l.starts_with('A'))
        .map(|l| l[3..].trim())
        .filter(|p| is_test_path(p))
        .collect();
    if touched.is_empty() {
        return None;
    }
    Some(format!(
        "\nNote: {} existing test file(s) changed: {}. Existing tests are the spec — if you altered an assertion, say why in your reply and the commit message, or put it back and add a new test instead.\n",
        touched.len(),
        touched.join(", ")
    ))
}

pub(crate) fn is_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    lower
        .split('/')
        .any(|seg| seg == "tests" || seg == "test" || seg == "__tests__" || seg == "spec")
        || name.starts_with("test_")
        || name.ends_with("_test.py")
        || name.ends_with("_test.go")
        || name.ends_with("_test.rs")
        || name.ends_with(".test.ts")
        || name.ends_with(".test.js")
        || name.ends_with(".test.tsx")
        || name.ends_with(".spec.ts")
        || name.ends_with(".spec.js")
        || name.ends_with("_spec.rb")
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
    let mark = cwd.join(".arbos").join("runtime").join("checkpoint");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_note_names_changed_existing_tests_only() {
        let status = " M src/lib.rs\n M tests/test_csv.py\n?? tests/test_new.py\nA  tests/test_added.py\n M pkg/foo_test.go\n";
        let note = test_files_note(status).unwrap();
        assert!(note.contains("2 existing test file(s)"), "{note}");
        assert!(note.contains("tests/test_csv.py") && note.contains("pkg/foo_test.go"));
        assert!(!note.contains("test_new.py") && !note.contains("test_added.py"));
        assert!(test_files_note(" M src/lib.rs\n").is_none());
    }
}
