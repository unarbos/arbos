//! What a tool call touches, declared before it runs.
//!
//! Two calls may run at the same time only when neither writes a resource
//! the other reads or writes. Read/read never conflicts. A path covers its
//! whole subtree: a grep over `src/` conflicts with an edit to `src/a.rs`.
//! `exclusive` marks a call whose footprint cannot be bounded (an opaque
//! shell command, undo, a question to the user); it conflicts with everything.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resource {
    /// Absolute path. Covers its subtree.
    Path(PathBuf),
    /// The shared browser session. Browser calls order among themselves.
    Browser,
    /// A file or folder in another node's store, by address
    /// (`arbos://<machine>/<project>/<path>`). Covers its subtree, like a
    /// path; a write to one is a write for plan mode and readonly agents.
    Store(String),
}

impl Resource {
    fn covers(&self, other: &Resource) -> bool {
        match (self, other) {
            (Resource::Path(a), Resource::Path(b)) => b.starts_with(a),
            (Resource::Browser, Resource::Browser) => true,
            (Resource::Store(a), Resource::Store(b)) => b.starts_with(a.trim_end_matches('/')),
            _ => false,
        }
    }

    fn overlaps(&self, other: &Resource) -> bool {
        self.covers(other) || other.covers(self)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Access {
    pub reads: Vec<Resource>,
    pub writes: Vec<Resource>,
    /// Must run alone.
    pub exclusive: bool,
}

impl Access {
    /// Touches nothing in the place. Runs alongside anything.
    pub fn none() -> Self {
        Self::default()
    }

    pub fn exclusive() -> Self {
        Self {
            exclusive: true,
            ..Self::default()
        }
    }

    pub fn reads(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            reads: paths.into_iter().map(Resource::Path).collect(),
            ..Self::default()
        }
    }

    pub fn writes(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            writes: paths.into_iter().map(Resource::Path).collect(),
            ..Self::default()
        }
    }

    pub fn read_path(path: &Path) -> Self {
        Self::reads([path.to_path_buf()])
    }

    pub fn write_path(path: &Path) -> Self {
        Self::writes([path.to_path_buf()])
    }

    /// A read of another node's store by address.
    pub fn read_store(address: impl Into<String>) -> Self {
        Self {
            reads: vec![Resource::Store(address.into())],
            ..Self::default()
        }
    }

    /// A write into another node's store by address.
    pub fn write_store(address: impl Into<String>) -> Self {
        Self {
            writes: vec![Resource::Store(address.into())],
            ..Self::default()
        }
    }

    pub fn write_resource(r: Resource) -> Self {
        Self {
            writes: vec![r],
            ..Self::default()
        }
    }

    /// No writes and not exclusive. Safe to run early and beside any read.
    pub fn is_readonly(&self) -> bool {
        self.writes.is_empty() && !self.exclusive
    }

    /// True when the two calls must not overlap in time.
    pub fn conflicts(&self, other: &Access) -> bool {
        if self.exclusive || other.exclusive {
            return true;
        }
        overlaps(&self.writes, &other.writes)
            || overlaps(&self.writes, &other.reads)
            || overlaps(&other.writes, &self.reads)
    }
}

fn overlaps(x: &[Resource], y: &[Resource]) -> bool {
    x.iter().any(|a| y.iter().any(|b| a.overlaps(b)))
}
