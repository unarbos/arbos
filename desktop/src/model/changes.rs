//! What the project's working tree holds that is not committed — Cursor's
//! `Changes +6 −1` pill over the composer and its "2 Files Changed" card
//! under the answer. Read from git, off the UI thread, every few seconds
//! while a local project is in front; a project that is not a repository
//! has none.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// Relative to the repository root, as git prints it.
    pub path: String,
    pub add: u32,
    pub del: u32,
    /// Not yet tracked by git: every line counts as added.
    pub new: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitChanges {
    pub files: Vec<FileChange>,
    /// Commits on this branch not yet on its upstream — Cursor's "Push"
    /// pill once the tree is clean. Zero when there is no upstream.
    pub ahead: u32,
}

/// Untracked paths nobody means as a change: caches, virtual
/// environments, build output, the app's own store.
fn junk(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    path.split('/').any(|part| {
        matches!(
            part,
            "__pycache__" | ".venv" | "venv" | "node_modules" | "target" | ".arbos" | ".git" | ".mypy_cache" | ".pytest_cache" | "dist" | "build"
        )
    }) || name.ends_with(".pyc")
        || name == ".DS_Store"
}

impl GitChanges {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn add(&self) -> u32 {
        self.files.iter().map(|f| f.add).sum()
    }

    pub fn del(&self) -> u32 {
        self.files.iter().map(|f| f.del).sum()
    }

    /// Uncommitted changes under `root`: tracked files against HEAD, then
    /// untracked ones. `None` when `root` is not inside a git repository
    /// (or git is not there).
    pub fn read(root: &Path) -> Option<GitChanges> {
        let numstat = git(root, &["diff", "--numstat", "HEAD", "--"])?;
        let mut files: Vec<FileChange> = numstat
            .lines()
            .filter_map(|line| {
                let mut parts = line.splitn(3, '\t');
                let add = parts.next()?.trim();
                let del = parts.next()?.trim();
                let path = parts.next()?.trim().to_string();
                // Binary files print "-"; count them as one change each way.
                Some(FileChange {
                    path,
                    add: add.parse().unwrap_or(if add == "-" { 1 } else { 0 }),
                    del: del.parse().unwrap_or(if del == "-" { 1 } else { 0 }),
                    new: false,
                })
            })
            .collect();
        if let Some(untracked) = git(root, &["ls-files", "--others", "--exclude-standard"]) {
            for path in untracked.lines().map(str::trim).filter(|p| !p.is_empty()) {
                if junk(path) || root.join(path).is_symlink() {
                    continue;
                }
                let lines = std::fs::read_to_string(root.join(path))
                    .map(|text| text.lines().count() as u32)
                    .unwrap_or(1);
                files.push(FileChange {
                    path: path.to_string(),
                    add: lines,
                    del: 0,
                    new: true,
                });
            }
        }
        let ahead = git(root, &["rev-list", "--count", "@{u}..HEAD"])
            .and_then(|out| out.trim().parse().ok())
            .unwrap_or(0);
        Some(GitChanges { files, ahead })
    }

    /// The diff of one file (or the whole tree when `path` is `None`) as
    /// text, for the review view. An untracked file shows whole.
    pub fn diff_text(root: &Path, path: Option<&str>) -> String {
        let mut args = vec!["diff", "HEAD", "--"];
        if let Some(path) = path {
            args.push(path);
        }
        let mut out = git(root, &args).unwrap_or_default();
        let untracked = git(root, &["ls-files", "--others", "--exclude-standard"]).unwrap_or_default();
        for file in untracked.lines().map(str::trim).filter(|p| !p.is_empty()) {
            if path.is_some_and(|wanted| wanted != file) || junk(file) || root.join(file).is_symlink() {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(root.join(file)) {
                out.push_str(&format!("diff --git a/{file} b/{file}\nnew file\n--- /dev/null\n+++ b/{file}\n"));
                for line in text.lines() {
                    out.push('+');
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
        out
    }

    /// Where the review view's diff is written, under the project's own
    /// desktop folder so it is never mistaken for the project's files.
    pub fn review_path(root: &Path, path: Option<&str>) -> PathBuf {
        let name = match path {
            Some(p) => format!("{}.diff", p.replace('/', "__")),
            None => "changes.diff".to_string(),
        };
        root.join(".arbos").join("desktop").join("review").join(name)
    }
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}
