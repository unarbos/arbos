//! What this build is, in one line, so whoever runs it can say which one:
//! `layout 1a2b3c4 · kernel 0.2.0`. The commit is stamped by `build.rs`
//! from git at compile time (a shipped bundle has no repository to ask);
//! the kernel version is the sibling crate's, read the same way.

/// The commit this binary was built from, `-dirty` when the tree had edits.
pub const COMMIT: &str = env!("ARBOS_COMMIT");

/// The `arbos-kernel` crate version beside this build.
pub const KERNEL_VERSION: &str = env!("ARBOS_KERNEL_VERSION");

/// `layout <commit> · kernel <version>`.
pub fn badge() -> String {
    format!("layout {COMMIT} · kernel {KERNEL_VERSION}")
}
