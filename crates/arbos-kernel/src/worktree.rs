//! A checkout of the place's repository for one child agent.
//!
//! `spawn isolate=worktree` gives the child `<place>/.arbos/worktrees/<id>/`
//! on branch `arbos/<id>`, cut from the parent's `HEAD`. The child edits,
//! builds, and commits there; the parent's tree is untouched. Under
//! `.arbos/` so the file tools, which stay inside the place, still reach it.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const DIR: &str = "worktrees";
pub const BRANCH_PREFIX: &str = "arbos/";

#[derive(Debug, Clone)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    /// Short sha the branch was cut from.
    pub base: String,
}

impl Worktree {
    /// Where `id`'s worktree would be, for a given place.
    pub fn path_for(place: &Path, id: &str) -> PathBuf {
        place.join(".arbos").join(DIR).join(id)
    }

    pub fn branch_for(id: &str) -> String {
        format!("{BRANCH_PREFIX}{id}")
    }

    /// The one-line removal the parent runs when the branch is merged or
    /// abandoned.
    pub fn removal(&self) -> String {
        format!(
            "git worktree remove --force {} && git branch -D {}",
            self.path.display(),
            self.branch
        )
    }
}

/// Is `cwd` one of these? Only the location says so; the branch is read
/// from git when a prompt needs it.
pub fn is_worktree(place: &Path, cwd: &Path) -> bool {
    cwd.starts_with(place.join(".arbos").join(DIR))
}

/// Current branch name in `dir`, for the child's prompt.
pub fn branch_of(dir: &Path) -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(dir)
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !s.is_empty()).then_some(s)
}

/// Whether `place` is inside a git work tree.
pub fn is_repo(place: &Path) -> bool {
    git(place, &["rev-parse", "--is-inside-work-tree"])
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "true")
        .unwrap_or(false)
}

/// Make the worktree. Fails, leaving nothing behind, when the place is not
/// a repository, has no commit yet, or the branch or folder already exists.
pub fn create(place: &Path, id: &str) -> Result<Worktree> {
    let out = git(place, &["rev-parse", "--is-inside-work-tree"])?;
    if !out.status.success() || String::from_utf8_lossy(&out.stdout).trim() != "true" {
        bail!(
            "isolate=worktree needs a git repository at {}; this place is not one",
            place.display()
        );
    }
    let head = git(place, &["rev-parse", "--short", "HEAD"])?;
    if !head.status.success() {
        bail!(
            "isolate=worktree needs at least one commit in {}; HEAD has none",
            place.display()
        );
    }
    let base = String::from_utf8_lossy(&head.stdout).trim().to_string();
    let path = Worktree::path_for(place, id);
    let branch = Worktree::branch_for(id);
    if path.exists() {
        bail!("worktree folder {} already exists", path.display());
    }
    let exists = git(
        place,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )?;
    if exists.status.success() {
        bail!("branch {branch} already exists; remove it or pick another brief");
    }
    // A folder deleted by hand leaves a stale registration that would make
    // the add fail on the same path.
    let _ = git(place, &["worktree", "prune"]);
    std::fs::create_dir_all(path.parent().expect("worktrees dir"))?;
    let path_s = path.to_string_lossy().into_owned();
    let added = git(place, &["worktree", "add", "-b", &branch, &path_s, "HEAD"])?;
    if !added.status.success() {
        let _ = std::fs::remove_dir_all(&path);
        bail!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&added.stderr).trim()
        );
    }
    exclude_arbos(place);
    Ok(Worktree { path, branch, base })
}

/// `.arbos/` is usually in the project's `.gitignore`. When it is not, the
/// worktree would show up as an untracked folder in the parent's status;
/// `.git/info/exclude` hides it locally without editing a tracked file.
fn exclude_arbos(place: &Path) {
    let ignored = git(place, &["check-ignore", "-q", ".arbos"])
        .map(|o| o.status.success())
        .unwrap_or(false);
    if ignored {
        return;
    }
    let Ok(common) = git(place, &["rev-parse", "--git-common-dir"]) else {
        return;
    };
    let dir = String::from_utf8_lossy(&common.stdout).trim().to_string();
    let git_dir = if Path::new(&dir).is_absolute() {
        PathBuf::from(dir)
    } else {
        place.join(dir)
    };
    let exclude = git_dir.join("info").join("exclude");
    let current = std::fs::read_to_string(&exclude).unwrap_or_default();
    if current
        .lines()
        .any(|l| l.trim() == "/.arbos/" || l.trim() == ".arbos/")
    {
        return;
    }
    if std::fs::create_dir_all(exclude.parent().expect("info dir")).is_ok() {
        let mut text = current;
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("/.arbos/\n");
        let _ = std::fs::write(&exclude, text);
    }
}

fn git(dir: &Path, args: &[&str]) -> Result<std::process::Output> {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .with_context(|| format!("run git {} in {}", args.join(" "), dir.display()))
}
