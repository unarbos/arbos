use std::path::{Path, PathBuf};

/// A directory the kernel serves. Remote places are the same folder on a host;
/// the window tunnels. The kernel only ever sees a local path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub path: PathBuf,
}

impl Place {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn arbos(&self) -> PathBuf {
        self.path.join(".arbos")
    }

    pub fn agents_dir(&self) -> PathBuf {
        self.arbos().join("agents")
    }

    pub fn archive_dir(&self) -> PathBuf {
        self.arbos().join("archive")
    }

    /// Process facts and caches: what describes a running kernel, not the
    /// project. Never committed to the `.arbos/` repository, and safe to
    /// delete when no kernel runs. (Phase 1 of the file-system design.)
    pub fn runtime_dir(&self) -> PathBuf {
        self.arbos().join("runtime")
    }

    pub fn kernel_json(&self) -> PathBuf {
        self.runtime_dir().join("kernel.json")
    }

    /// Where kernels before the `runtime/` split wrote `kernel.json`; the
    /// kernel still writes a copy there for one release, so older windows
    /// and phones find it, and readers fall back to it.
    pub fn legacy_kernel_json(&self) -> PathBuf {
        self.arbos().join("kernel.json")
    }

    /// The `kernel.json` that describes the kernel actually running here.
    ///
    /// Both files can exist at once and disagree, because they have
    /// different writers. A kernel from before the `runtime/` split writes
    /// only the legacy path; a kernel after it writes both. So a place
    /// that has ever been served by a newer kernel keeps a `runtime/`
    /// file for ever, and if an older kernel then serves that place —
    /// after a rollback, or because a supervisor restarted the
    /// `.previous` binary — the newer file stays behind, naming a process
    /// that is gone.
    ///
    /// Preferring the newer path made every reader believe it. Measured
    /// on the disposable target on 2026-09-17: a live kernel on
    /// `67d066eb48f0` had written the legacy file with its own pid, while
    /// `runtime/kernel.json` still named a dead pid on `cbbe9922d6a2` —
    /// so the sweep and `update --place` reported the newer build for a
    /// place serving older code. That is the lie this whole area exists
    /// to prevent, arriving by a different route than `subnet120` did.
    ///
    /// So the question asked is not "which path is newer" but "which of
    /// these names a process that is still there". When neither does, the
    /// newer path wins as before, because then the honest answer is that
    /// nothing is running and either file says so.
    pub fn kernel_json_read(&self) -> PathBuf {
        let new = self.kernel_json();
        let legacy = self.legacy_kernel_json();
        if names_a_live_pid(&new) {
            return new;
        }
        if names_a_live_pid(&legacy) {
            return legacy;
        }
        if new.exists() { new } else { legacy }
    }

    pub fn focus_path(&self) -> PathBuf {
        self.runtime_dir().join("focus")
    }

    pub fn user_md(&self) -> PathBuf {
        self.arbos().join("user.md")
    }

    pub fn lock_path(&self) -> PathBuf {
        self.runtime_dir().join("lock")
    }

    /// Where kernels before the `runtime/` split took the place lock. A
    /// kernel takes both, so an old build and a new one contend for the
    /// same place and one of them loses honestly — with only the new
    /// path, two kernels of two builds served one store at once (the
    /// update worker's proof, 2026-09-17).
    pub fn legacy_lock_path(&self) -> PathBuf {
        self.arbos().join("lock")
    }

    /// The lock files a holder writes its pid into, legacy first: the one
    /// an old kernel reads and the one a new kernel reads.
    pub fn lock_paths(&self) -> [PathBuf; 2] {
        [self.legacy_lock_path(), self.lock_path()]
    }

    /// The `.arbos/` folder's own git repository, when bootstrap made one.
    pub fn arbos_repo(&self) -> PathBuf {
        self.arbos().join(".git")
    }

    pub fn hooks_dir(&self) -> PathBuf {
        self.arbos().join("hooks")
    }

    pub fn agent_dir(&self, id: &str) -> PathBuf {
        self.agents_dir().join(id)
    }

    /// Where `spawn isolate=worktree` puts a child's checkout: one folder
    /// per child under here. A cwd inside it is that child's whole world.
    pub fn worktrees_dir(&self) -> PathBuf {
        self.arbos().join("worktrees")
    }
}

/// Whether a `kernel.json` describes a process that is still there.
///
/// Absent, unreadable, or naming a pid nothing answers for all count as
/// no: the caller is choosing between two records and wants the one that
/// is about a live kernel, not the one that parses.
fn names_a_live_pid(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    let Some(pid) = json.get("pid").and_then(|p| p.as_i64()) else {
        return false;
    };
    // Guard the pids that do not mean one process: 0 is this process's
    // whole group and a negative pid is a group too, so signalling either
    // would answer a question nobody asked — and `kill(0, 0)` always
    // succeeds, which would make `"pid": 0` read as live for ever.
    if pid <= 0 || pid > i64::from(i32::MAX) {
        return false;
    }
    // SAFETY: signal 0 asks whether the pid could be signalled and sends
    // nothing.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(test)]
mod kernel_json_tests {
    use super::*;

    fn place_with(runtime: Option<i64>, legacy: Option<i64>) -> (tempfile::TempDir, Place) {
        let dir = tempfile::tempdir().unwrap();
        let place = Place::new(dir.path().to_path_buf());
        std::fs::create_dir_all(place.runtime_dir()).unwrap();
        let write = |path: PathBuf, pid: i64| {
            std::fs::write(path, format!(r#"{{"pid": {pid}, "url": "tcp://x"}}"#)).unwrap();
        };
        if let Some(pid) = runtime {
            write(place.kernel_json(), pid);
        }
        if let Some(pid) = legacy {
            write(place.legacy_kernel_json(), pid);
        }
        (dir, place)
    }

    /// A pid nothing can be running under, so `kill -0` says no.
    const DEAD: i64 = 0x7FFF_FFFE;
    fn alive() -> i64 {
        i64::from(std::process::id())
    }

    #[test]
    fn a_live_legacy_record_beats_a_dead_runtime_one() {
        // The state measured on the target: an older kernel serving a
        // place a newer one had served, so the newer file is left over.
        let (_dir, place) = place_with(Some(DEAD), Some(alive()));
        assert_eq!(place.kernel_json_read(), place.legacy_kernel_json());
    }

    #[test]
    fn the_runtime_record_is_still_preferred_when_it_is_the_live_one() {
        let (_dir, place) = place_with(Some(alive()), Some(DEAD));
        assert_eq!(place.kernel_json_read(), place.kernel_json());
    }

    #[test]
    fn both_live_reads_as_the_runtime_one() {
        // The ordinary case: a current kernel writes both.
        let (_dir, place) = place_with(Some(alive()), Some(alive()));
        assert_eq!(place.kernel_json_read(), place.kernel_json());
    }

    #[test]
    fn neither_live_keeps_the_old_answer() {
        // Nothing is running, and both files say so. The newer path wins
        // as it always did, so "no kernel here" reads the same as before.
        let (_dir, place) = place_with(Some(DEAD), Some(DEAD));
        assert_eq!(place.kernel_json_read(), place.kernel_json());
    }

    #[test]
    fn only_a_legacy_file_is_found_whether_or_not_it_is_live() {
        let (_dir, place) = place_with(None, Some(alive()));
        assert_eq!(place.kernel_json_read(), place.legacy_kernel_json());
        let (_dir, place) = place_with(None, Some(DEAD));
        assert_eq!(place.kernel_json_read(), place.legacy_kernel_json());
    }

    #[test]
    fn a_pid_of_zero_is_not_a_live_process() {
        let (_dir, place) = place_with(Some(0), Some(alive()));
        assert_eq!(place.kernel_json_read(), place.legacy_kernel_json());
    }

    #[test]
    fn rubbish_in_a_file_is_not_a_live_process() {
        let (_dir, place) = place_with(None, Some(alive()));
        std::fs::write(place.kernel_json(), "not json at all").unwrap();
        assert_eq!(place.kernel_json_read(), place.legacy_kernel_json());
    }
}
