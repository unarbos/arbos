//! Helpers the model's own tests share.
//!
//! `feedback.rs` grew its own copy of this before there was anywhere to put
//! one; when someone next touches that file, its `Scratch` belongs here.

use std::path::{Path, PathBuf};

/// A throwaway folder that clears up after itself, so a test owes no
/// dependency for one directory.
pub struct Scratch(PathBuf);

impl Scratch {
    /// A counter as well as the clock: tests run in parallel, and two that
    /// asked in the same millisecond shared a folder, so one test's cleanup
    /// deleted another's files. That passes alone and fails in the suite,
    /// which is the shape of every flake.
    pub fn new(name: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let at = std::env::temp_dir().join(format!(
            "arbos-model-test-{}-{name}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&at).unwrap();
        Self(at)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
