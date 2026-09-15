//! What the general section shows beside the version: the commit this binary
//! was built from.
//!
//! Asked here rather than at runtime because the app that ships has no
//! repository to ask — a bundle in `/Applications` is a binary and an icon —
//! so the answer is compiled in or it does not exist.

use std::process::Command;

fn main() {
    println!("cargo::rustc-env=ARBOS_COMMIT={}", commit());
    println!("cargo::rustc-env=ARBOS_BUILD={}", build());
    println!("cargo::rustc-env=ARBOS_KERNEL_VERSION={}", kernel_version());
    println!("cargo::rerun-if-changed=../crates/arbos-kernel/Cargo.toml");
    // Without this, a packager that passes a different number gets the number
    // from the last build, which is exactly the drift this exists to stop.
    println!("cargo::rerun-if-env-changed=ARBOS_BUILD");
    // Cargo has no reason of its own to look at git, so without these the
    // stamp is whichever commit was checked out the last time something else
    // forced a rebuild. `--git-path` resolves them through the repository
    // itself, which is what keeps this right inside a worktree, where `.git`
    // is a file pointing somewhere else.
    for file in ["HEAD", "refs"] {
        if let Some(path) = git(&["rev-parse", "--git-path", file]) {
            println!("cargo::rerun-if-changed={path}");
        }
    }
}

/// `1a2b3c4`, and `1a2b3c4-dirty` where the tree has been edited since — the
/// suffix git's own `describe` uses, because a build with changes in it is not
/// the commit it names.
///
/// `unknown` where there is no repository at all: a build from a tarball —
/// `cargo install`, a vendored tree — is still a build, so this says what it
/// does not know rather than failing.
fn commit() -> String {
    let Some(short) = git(&["rev-parse", "--short=7", "HEAD"]) else {
        return "unknown".into();
    };
    match git(&["status", "--porcelain"]).is_none_or(|tree| tree.is_empty()) {
        true => short,
        false => format!("{short}-dirty"),
    }
}

/// Commits on the branch: the half of the version that moves between two
/// builds of one `0.2.0`, and what the update feed orders on. A build that
/// does not know it can never be told it is behind.
///
/// `ARBOS_BUILD` in the environment wins, and whoever is packaging sets it —
/// `desktop/Makefile` and the Linux payload action both do. That is the point.
/// Counting commits here *and* in the Makefile is two answers to one question,
/// and they drifted: a bundle went out whose `CFBundleVersion` said 870 while
/// the binary inside said 879. Cargo caches this script's output, so the two
/// are not even computed at the same moment. Now the packager decides once and
/// tells everybody, and `rerun-if-env-changed` below makes a different answer
/// force a re-stamp instead of being served from the cache.
///
/// The git count remains for a plain `cargo build`, which has no packager.
/// `0` where there is no repository either, which sorts under every build that
/// had one.
fn build() -> String {
    if let Ok(given) = std::env::var("ARBOS_BUILD")
        && given.trim().parse::<u64>().is_ok()
    {
        return given.trim().to_owned();
    }
    git(&["rev-list", "--count", "HEAD"]).unwrap_or_else(|| "0".into())
}

/// The kernel this build was cut beside: the `version` line of
/// `crates/arbos-kernel/Cargo.toml`, read by hand so the build script owes
/// no dependency. `unknown` outside the repository, where there is no
/// sibling crate to read.
fn kernel_version() -> String {
    let Ok(text) = std::fs::read_to_string("../crates/arbos-kernel/Cargo.toml") else {
        return "unknown".into();
    };
    text.lines()
        .find_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == "version").then(|| value.trim().trim_matches('"').to_owned())
        })
        .unwrap_or_else(|| "unknown".into())
}

/// One git command, or nothing at all: every caller here has an answer for a
/// machine with no git on it.
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8(out.stdout).ok()?.trim().to_owned())
}
