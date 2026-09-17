//! The `.arbos/` repository's commits: one after every turn, one after
//! every mechanical node run, one around a rewind. `.arbos/` is its own
//! nested git repository (`arbos_core::init_arbos_repo`), so the record of
//! an agent's work is git history: `git -C .arbos log`, `show`, `diff`.
//!
//! Commits run on the blocking pool, one at a time (git takes its own lock
//! and two at once make one fail), with a fixed identity so no user config
//! is needed on a server. Quiet when git or the repository is missing.

use std::path::Path;
use std::process::Command;
use std::sync::Mutex;

use anyhow::{Result, bail};
use arbos_core::Place;

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

const IDENTITY: [&str; 4] = ["-c", "user.name=arbos", "-c", "user.email=arbos@localhost"];

/// Whether this place's `.arbos/` is a repository we can commit to.
pub fn enabled(place: &Place) -> bool {
    place.arbos_repo().exists()
}

/// `git add -A && git commit` in `.arbos/`, when there is anything to
/// commit. Blocking. Returns the new commit's short sha, or None when the
/// tree was clean or git is unavailable.
pub fn commit(place: &Place, message: &str) -> Result<Option<String>> {
    if !enabled(place) {
        return Ok(None);
    }
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let arbos = place.arbos();
    let add = Command::new("git")
        .args(IDENTITY)
        .args(["add", "-A", "--", "."])
        .current_dir(&arbos)
        .stdin(std::process::Stdio::null())
        .output()?;
    if !add.status.success() {
        bail!("git add: {}", String::from_utf8_lossy(&add.stderr).trim());
    }
    let staged = Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(&arbos)
        .stdin(std::process::Stdio::null())
        .status()?;
    if staged.success() {
        // Nothing staged: `diff --cached --quiet` exits 0 on no changes.
        return Ok(None);
    }
    let msg: String = message
        .lines()
        .next()
        .unwrap_or("arbos")
        .chars()
        .take(200)
        .collect();
    let out = Command::new("git")
        .args(IDENTITY)
        .args(["commit", "-q", "--no-verify", "-m", &msg])
        .current_dir(&arbos)
        .stdin(std::process::Stdio::null())
        .output()?;
    if !out.status.success() {
        bail!(
            "git commit: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(head(&arbos))
}

/// Commit in the background; a failure is a warning in the kernel log,
/// never a failed turn.
pub fn commit_later(place: &Place, message: String) {
    if !enabled(place) {
        return;
    }
    let place = place.clone();
    tokio::task::spawn_blocking(move || match commit(&place, &message) {
        Ok(Some(sha)) => crate::klog::info("snapshot", None, format!("{sha} {message}")),
        Ok(None) => {}
        Err(e) => crate::klog::warn("snapshot_failed", None, format!("{e:#}")),
    });
}

/// Short sha of HEAD, if any.
pub fn head(arbos: &Path) -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .current_dir(arbos)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `git log --oneline -n N` of the record.
pub fn log(place: &Place, n: usize) -> Result<Vec<String>> {
    if !enabled(place) {
        bail!(".arbos/ is not a git repository yet (start a kernel once)");
    }
    let out = Command::new("git")
        .args([
            "log",
            "--format=%h %ad %s",
            "--date=format:%Y-%m-%d %H:%M:%S",
            "-n",
            &n.to_string(),
        ])
        .current_dir(place.arbos())
        .stdin(std::process::Stdio::null())
        .output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("does not have any commits") {
            return Ok(Vec::new());
        }
        bail!("git log: {}", err.trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect())
}

/// Put the record back to `commit`: every tracked file to that tree,
/// files the target lacks removed (`runtime/` is untracked and stays),
/// then a forward commit "rewind to <sha>" so history stays linear and
/// the pre-rewind state is one commit back. The kernel must be stopped.
pub fn rewind_to(place: &Place, to: &str) -> Result<String> {
    if !enabled(place) {
        bail!(".arbos/ is not a git repository yet");
    }
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let arbos = place.arbos();
    let resolve = Command::new("git")
        .args(["rev-parse", "--verify", &format!("{to}^{{commit}}")])
        .current_dir(&arbos)
        .stdin(std::process::Stdio::null())
        .output()?;
    if !resolve.status.success() {
        bail!("{to}: not a commit in .arbos/ (see `arbos-kernel log`)");
    }
    let target = String::from_utf8_lossy(&resolve.stdout).trim().to_string();
    // Anything uncommitted is committed first, so nothing is lost.
    drop(_one);
    let _ = commit(place, "pre-rewind")?;
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let st = Command::new("git")
        .args(["read-tree", "-u", "--reset", &target])
        .current_dir(&arbos)
        .stdin(std::process::Stdio::null())
        .status()?;
    if !st.success() {
        bail!("git read-tree {target} failed");
    }
    drop(_one);
    let short: String = target.chars().take(12).collect();
    let sha = commit(place, &format!("rewind to {short}"))?;
    Ok(sha.unwrap_or(short))
}
