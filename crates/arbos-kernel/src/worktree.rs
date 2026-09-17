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
    /// When the base asked for was not used: why, for the spawn result.
    pub note: Option<String>,
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
    create_from(place, id, None)
}

/// `create`, cutting the branch from `base` (a branch, tag, or sha; Cursor's
/// "base branch" on `CreateAgent`) instead of HEAD. `None` is HEAD.
pub fn create_from(place: &Path, id: &str, base: Option<&str>) -> Result<Worktree> {
    let out = git(place, &["rev-parse", "--is-inside-work-tree"])?;
    if !out.status.success() || String::from_utf8_lossy(&out.stdout).trim() != "true" {
        bail!(
            "isolate=worktree needs a git repository at {}; this place is not one",
            place.display()
        );
    }
    let start = base
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .unwrap_or("HEAD");
    let head = git(
        place,
        &["rev-parse", "--short", &format!("{start}^{{commit}}")],
    )?;
    if !head.status.success() {
        if start == "HEAD" {
            bail!(
                "isolate=worktree needs at least one commit in {}; HEAD has none",
                place.display()
            );
        }
        bail!(
            "base {start:?} is not a commit in {}: {}",
            place.display(),
            String::from_utf8_lossy(&head.stderr).trim()
        );
    }
    let mut base = String::from_utf8_lossy(&head.stdout).trim().to_string();
    let mut start = start.to_string();
    let mut note = None;
    // A base that has none, or few, of the checkout's files would hand
    // the worker an empty project (M-111: the seed was committed on a side
    // branch because the guard refuses `main`, the coordinator passed
    // base=main, the worker found nothing and copied files from an older
    // run's folder; JB-5, four runs). A branch that merely lags HEAD by a
    // few commits is a base as asked — parallel work from main is a real
    // want; a base missing most of the tree is not.
    if start != "HEAD"
        && let Some((base_files, head_files)) = tree_sizes(place, &start)
        && head_files > 0
        && base_files * 2 < head_files
    {
        let head_sha = git(place, &["rev-parse", "--short", "HEAD"])
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|| "HEAD".into());
        let on = arbos_core::store::current_branch(place).unwrap_or_else(|| "HEAD".into());
        note = Some(format!(
            "base {start} has {base_files} tracked file(s) where the checkout ({on}) has {head_files}; a worker cut from it would find an empty project, so its branch is cut from HEAD ({head_sha}) instead. If {start} truly is the base you want, commit the project there first."
        ));
        start = "HEAD".into();
        base = head_sha;
    }
    // A folder deleted by hand leaves a stale registration that would make
    // the add fail on the same path.
    let _ = git(place, &["worktree", "prune"]);
    // A re-spawn under a name used earlier in the project finds the old
    // branch (its commits kept when the worker was archived) and, when the
    // old worktree was dirty, the old folder too. Neither is the model's
    // to manage (F-58: the turn ended on "branch-name collision"): the new
    // worker gets the next free `-N` branch and folder, and the old work
    // stays where it was.
    let (path, branch) = free_slot(place, id)?;
    std::fs::create_dir_all(path.parent().expect("worktrees dir"))?;
    let path_s = path.to_string_lossy().into_owned();
    let added = git(place, &["worktree", "add", "-b", &branch, &path_s, &start])?;
    if !added.status.success() {
        let _ = std::fs::remove_dir_all(&path);
        bail!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&added.stderr).trim()
        );
    }
    exclude_arbos(place);
    Ok(Worktree {
        path,
        branch,
        base,
        note,
    })
}

/// Tracked files in `rev` and in HEAD, when git can say.
fn tree_sizes(place: &Path, rev: &str) -> Option<(usize, usize)> {
    let count = |r: &str| -> Option<usize> {
        let out = git(place, &["ls-tree", "-r", "--name-only", r]).ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).lines().count())
    };
    Some((count(rev)?, count("HEAD")?))
}

/// The first `(folder, branch)` pair free for `id`: `arbos/<id>` and
/// `.arbos/worktrees/<id>` when neither exists, else `-2`, `-3`, … up to
/// a limit. Both names carry the same suffix so a worktree and its
/// branch read as one.
fn free_slot(place: &Path, id: &str) -> Result<(PathBuf, String)> {
    for n in 1..=50u32 {
        let slot = if n == 1 {
            id.to_string()
        } else {
            format!("{id}-{n}")
        };
        let path = Worktree::path_for(place, &slot);
        let branch = Worktree::branch_for(&slot);
        if path.exists() {
            continue;
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
            continue;
        }
        return Ok((path, branch));
    }
    bail!(
        "fifty worktrees and branches already exist for {id} under {}; remove some (git worktree list; git branch -D arbos/{id}-N)",
        place.display()
    )
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

/// The folder `id`'s worktree lives in: the agent's recorded `cwd` when
/// it is one of ours (a re-spawn may sit in `<id>-2`), else the plain
/// `.arbos/worktrees/<id>`. The agent is looked for live, then archived.
pub fn folder_for(place: &Path, id: &str) -> PathBuf {
    let plain = Worktree::path_for(place, id);
    let dirs = [
        place.join(".arbos").join("agents").join(id),
        place.join(".arbos").join("archive").join("agents").join(id),
    ];
    for dir in dirs {
        if let Ok(agent) = arbos_core::Agent::load(&dir)
            && let Some(cwd) = agent.cwd.as_deref()
            && is_worktree(place, cwd)
        {
            return cwd.to_path_buf();
        }
    }
    plain
}

/// Look at `id`'s worktree, if the folder is there. None when it is not.
pub fn leftover(place: &Path, id: &str) -> Option<Leftover> {
    let path = folder_for(place, id);
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
    // Confirmed: a hand's exclude lines are added to, never lost to a
    // failed read (arbos_core::record).
    let current = match arbos_core::record::read_text(&exclude).confirmed() {
        Ok(text) => text.unwrap_or_default(),
        Err(e) => {
            crate::klog::warn("exclude_unread", None, format!("{e:#}"));
            return;
        }
    };
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

    #[test]
    fn a_worktree_is_cut_from_the_base_it_is_given() {
        let dir = repo("base");
        for args in [
            vec!["branch", "-q", "release"],
            vec![
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "later on main",
            ],
        ] {
            assert!(git(&dir, &args).unwrap().status.success(), "git {args:?}");
        }
        let release = git(&dir, &["rev-parse", "--short", "release"]).unwrap();
        let release = String::from_utf8_lossy(&release.stdout).trim().to_string();
        let wt = create_from(&dir, "w-base", Some("release")).unwrap();
        assert_eq!(wt.base, release, "the branch starts at the base, not HEAD");
        let head = git(&wt.path, &["rev-parse", "--short", "HEAD"]).unwrap();
        assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), release);
        let err = create_from(&dir, "w-nope", Some("no-such-branch")).unwrap_err();
        assert!(err.to_string().contains("is not a commit"), "{err}");
        assert!(!Worktree::path_for(&dir, "w-nope").exists());
        assert!(
            wt.note.is_none(),
            "a base with the same tree is taken as asked"
        );
    }

    /// M-111 / JB-5: `main` holds an empty start commit; the project was
    /// committed on a side branch (the guard refuses `main`); the
    /// coordinator passes base=main. The worker must not find an empty
    /// project: the branch is cut from HEAD, and the result says why.
    #[test]
    fn a_base_missing_most_of_the_tree_is_replaced_by_head_with_a_note() {
        let dir = repo("seedbase");
        assert!(
            git(&dir, &["checkout", "-q", "-b", "fix/J1_initial_setup"])
                .unwrap()
                .status
                .success()
        );
        for f in ["mathlib.py", "tests/test_math.py", "README.md"] {
            let p = dir.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, "x\n").unwrap();
        }
        for args in [
            vec!["add", "."],
            vec![
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "-m",
                "seed",
            ],
        ] {
            assert!(git(&dir, &args).unwrap().status.success(), "git {args:?}");
        }
        let main = git(&dir, &["rev-parse", "--abbrev-ref", "main"]).unwrap();
        let main_name = if main.status.success() {
            "main"
        } else {
            "master"
        };
        let wt = create_from(&dir, "w-seed", Some(main_name)).unwrap();
        let head = git(&dir, &["rev-parse", "--short", "HEAD"]).unwrap();
        let head = String::from_utf8_lossy(&head.stdout).trim().to_string();
        assert_eq!(wt.base, head, "cut from HEAD, not the empty base");
        assert!(
            wt.path.join("mathlib.py").exists() && wt.path.join("tests/test_math.py").exists(),
            "the worker sees the project"
        );
        let note = wt.note.as_deref().expect("the result says why");
        assert!(
            note.contains(&format!("base {main_name} has 0 tracked file(s)")),
            "{note}"
        );
        assert!(note.contains("(fix/J1_initial_setup) has 3"), "{note}");
        assert!(note.contains("cut from HEAD"), "{note}");
        let _ = std::fs::remove_dir_all(&dir);
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

    /// F-58: a re-spawn under a name used earlier in the project met
    /// "branch arbos/test-fix already exists" and the turn ended with no
    /// code changed. The new worker takes the next free `-N` branch and
    /// folder; the old branch keeps its commits; a dirty old folder is
    /// left alone; and the archive step finds the new folder through the
    /// agent's recorded cwd.
    #[test]
    fn a_respawn_takes_the_next_free_branch_and_folder() {
        let dir = repo("respawn");
        let first = create(&dir, "test-fix").unwrap();
        assert_eq!(first.branch, "arbos/test-fix");
        // The first worker committed and was archived: folder gone, branch kept.
        for args in [vec![
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "fix",
        ]] {
            assert!(git(&first.path, &args).unwrap().status.success());
        }
        let done = remove_if_clean(&dir, "test-fix").unwrap();
        assert!(
            matches!(
                done,
                Removed::Removed {
                    branch_kept: true,
                    ..
                }
            ),
            "{done:?}"
        );
        assert!(branch_exists(&dir, "arbos/test-fix"));
        // The re-spawn: no collision, the -2 slot.
        let second = create(&dir, "test-fix").unwrap();
        assert_eq!(second.branch, "arbos/test-fix-2");
        assert!(
            second.path.ends_with("test-fix-2"),
            "{}",
            second.path.display()
        );
        assert!(second.path.is_dir());
        // A third while the second's folder is still there (dirty or not): -3.
        let third = create(&dir, "test-fix").unwrap();
        assert_eq!(third.branch, "arbos/test-fix-3");
        // The archive step resolves the folder from the agent's cwd.
        let place = arbos_core::Place::new(&dir);
        let mut a = arbos_core::Agent::root("test-fix");
        a.cwd = Some(third.path.clone());
        a.save(&place.agent_dir("test-fix")).unwrap();
        assert_eq!(folder_for(&dir, "test-fix"), third.path);
        assert_eq!(
            remove_if_clean(&dir, "test-fix").unwrap(),
            Removed::Removed {
                branch: "arbos/test-fix-3".into(),
                ahead: 0,
                branch_kept: false
            }
        );
        assert!(!third.path.exists());
        assert!(
            second.path.exists(),
            "the other worker's folder is untouched"
        );
    }
}
