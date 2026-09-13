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

    pub fn kernel_json(&self) -> PathBuf {
        self.arbos().join("kernel.json")
    }

    pub fn focus_path(&self) -> PathBuf {
        self.arbos().join("focus")
    }

    pub fn user_md(&self) -> PathBuf {
        self.arbos().join("user.md")
    }

    pub fn lock_path(&self) -> PathBuf {
        self.arbos().join("lock")
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
