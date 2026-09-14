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

/// What is left in a child's worktree once the child is done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leftover {
    pub path: PathBuf,
    pub branch: String,
    /// Uncommitted changes (modified or untracked paths), from `git status`.
    pub dirty: usize,
    /// Commits on the branch that the place's HEAD does not have.
    pub ahead: usize,
}

/// Look at `id`'s worktree, if the folder is there. None when it is not.
pub fn leftover(place: &Path, id: &str) -> Option<Leftover> {
    let path = Worktree::path_for(place, id);
    if !path.is_dir() {
        return None;
    }
    let branch = branch_of(&path).unwrap_or_else(|| Worktree::branch_for(id));
    let dirty = git(&path, &["status", "--porcelain"])
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().count())
        .unwrap_or(usize::MAX);
    let ahead = git(place, &["rev-list", "--count", &format!("HEAD..{branch}")])
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
        .unwrap_or(usize::MAX);
    Some(Leftover {
        path,
        branch,
        dirty,
        ahead,
    })
}

/// What `remove_if_clean` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Removed {
    /// No worktree folder for this id.
    Nothing,
    /// Uncommitted changes: the folder stays, with how many paths.
    KeptDirty { path: PathBuf, dirty: usize },
    /// The folder is gone. The branch stays when it holds commits the
    /// place does not have (`ahead` > 0); a branch with none goes too.
    Removed {
        branch: String,
        ahead: usize,
        branch_kept: bool,
    },
}

/// Take down a finished child's worktree when nothing would be lost: no
/// uncommitted changes. Commits stay on the branch; an empty branch (no
/// commit beyond the place's HEAD) is deleted with the folder. A dirty
/// tree is left where it is and named, never forced.
pub fn remove_if_clean(place: &Path, id: &str) -> Result<Removed> {
    let Some(left) = leftover(place, id) else {
        return Ok(Removed::Nothing);
    };
    if left.dirty > 0 {
        return Ok(Removed::KeptDirty {
            path: left.path,
            dirty: left.dirty,
        });
    }
    let path_s = left.path.to_string_lossy().into_owned();
    let out = git(place, &["worktree", "remove", &path_s])?;
    if !out.status.success() {
        bail!(
            "git worktree remove {}: {}",
            left.path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let branch_kept = left.ahead > 0;
    if !branch_kept {
        let _ = git(place, &["branch", "-D", &left.branch]);
    }
    Ok(Removed::Removed {
        branch: left.branch,
        ahead: left.ahead,
        branch_kept,
    })
}

/// Every worktree folder under `.arbos/worktrees/`, by id.
pub fn ids(place: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(place.join(".arbos").join(DIR))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
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

#[cfg(test)]
mod cleanup_tests {
    use super::*;

    fn repo(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "arbos-worktree-{tag}-{}-{}",
            std::process::id(),
            arbos_core::now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        for args in [
            vec!["init", "-q"],
            vec![
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "start",
            ],
        ] {
            assert!(git(&dir, &args).unwrap().status.success(), "git {args:?}");
        }
        dir
    }

    fn branch_exists(place: &Path, branch: &str) -> bool {
        git(
            place,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
        .unwrap()
        .status
        .success()
    }

    #[test]
    fn a_clean_empty_worktree_goes_with_its_branch() {
        let place = repo("clean");
        let wt = create(&place, "w1").unwrap();
        assert!(wt.path.is_dir());
        assert_eq!(ids(&place), vec!["w1"]);
        let done = remove_if_clean(&place, "w1").unwrap();
        assert_eq!(
            done,
            Removed::Removed {
                branch: "arbos/w1".into(),
                ahead: 0,
                branch_kept: false
            }
        );
        assert!(!wt.path.exists());
        assert!(!branch_exists(&place, "arbos/w1"));
        assert_eq!(remove_if_clean(&place, "w1").unwrap(), Removed::Nothing);
        let _ = std::fs::remove_dir_all(&place);
    }

    #[test]
    fn commits_keep_the_branch_and_uncommitted_work_keeps_the_folder() {
        let place = repo("kept");
        let wt = create(&place, "w2").unwrap();
        std::fs::write(wt.path.join("a.txt"), "a\n").unwrap();
        // Dirty: nothing is touched.
        let left = leftover(&place, "w2").unwrap();
        assert_eq!((left.dirty, left.ahead), (1, 0));
        assert_eq!(
            remove_if_clean(&place, "w2").unwrap(),
            Removed::KeptDirty {
                path: wt.path.clone(),
                dirty: 1
            }
        );
        assert!(wt.path.is_dir());
        // Committed: the folder goes, the branch and its commit stay.
        for args in [
            vec!["add", "a.txt"],
            vec![
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "-m",
                "work",
            ],
        ] {
            assert!(git(&wt.path, &args).unwrap().status.success());
        }
        assert_eq!(
            remove_if_clean(&place, "w2").unwrap(),
            Removed::Removed {
                branch: "arbos/w2".into(),
                ahead: 1,
                branch_kept: true
            }
        );
        assert!(!wt.path.exists());
        assert!(branch_exists(&place, "arbos/w2"));
        let _ = std::fs::remove_dir_all(&place);
    }
}
