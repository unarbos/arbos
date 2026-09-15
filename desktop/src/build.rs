//! What this build is, in one line, so whoever runs it can say which one:
//! `layout 1a2b3c4 · kernel 0.2.0`. The commit is stamped by `build.rs`
//! from git at compile time (a shipped bundle has no repository to ask);
//! the kernel version is the sibling crate's, read the same way.

/// The commit this binary was built from, `-dirty` when the tree had edits.
pub const COMMIT: &str = env!("ARBOS_COMMIT");

/// The `arbos-kernel` crate version beside this build.
pub const KERNEL_VERSION: &str = env!("ARBOS_KERNEL_VERSION");

/// Commits on the branch this was built from — see `build.rs`. The half of
/// the version that moves between two builds of one `0.2.0`, and so the half
/// the update feed is ordered on.
pub const BUILD: &str = env!("ARBOS_BUILD");

/// `layout <commit> · kernel <version>`.
pub fn badge() -> String {
    format!("layout {COMMIT} · kernel {KERNEL_VERSION}")
}

/// What this build is, in the pair the update feed compares on: the marketing
/// version from `Cargo.toml` and the commit count behind it.
///
/// This is the one place the running app says which build it is, and every
/// "is there something newer" question is asked against it.
pub fn version() -> arbos_update::Version {
    arbos_update::Version::parse(env!("CARGO_PKG_VERSION"))
        .unwrap_or_else(|_| arbos_update::Version::new(0, 0, 0, 0))
        .with_build(BUILD.parse().unwrap_or(0))
}

/// `0.2.0 (1877)` — the version as a person reads it, which is what the bar
/// shows when there is nothing to update to.
pub fn version_label() -> String {
    version().human()
}
