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

    /// The live `kernel.json`, wherever this kernel or an older one put it.
    pub fn kernel_json_read(&self) -> PathBuf {
        let new = self.kernel_json();
        if new.exists() {
            new
        } else {
            self.legacy_kernel_json()
        }
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
