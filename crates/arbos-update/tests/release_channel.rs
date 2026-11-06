//! Which channel a range of main releases on: `scripts/release/release-channel.sh`.
//!
//! The bug these guard against: a commit range that touches only `crates/arbos-agentd` or
//! `image/` released nothing at all. `mac-change.sh` correctly answered "no Mac path changed",
//! `auto-stable.sh` turned that single answer into `channel=none`, and every job in
//! `.github/workflows/release.yml` is gated on `channel != 'none'` — so an agentd fix produced no
//! tag, no image and no artifact, and sat on main until an unrelated Mac-touching merge happened
//! to carry it into a Mac cut. The 15-minute scheduled backstop runs the same predicate, so it
//! rescued nothing.
//!
//! The Mac question and the agent-image question are two questions. This file holds them apart:
//! a Mac change still cuts `vX.Y.Z` and nothing here may change that, and an agent-image change
//! on its own now cuts `image-v0.1.N`. A range on both is a Mac cut, because the Mac release is
//! the one with a signing and notarization cost that must not be paid twice.
//!
//! Every case builds its own throwaway repository, so none of them changes answer as main moves.
//! That is not a preference: CI checks main out one commit deep, so a case pinned to a real
//! commit could not read it. The live case these fixtures stand in for is #2817, merged as
//! 7988e3427 — two files, both under `crates/arbos-agentd/src`. `mac-change.sh` answers "no Mac
//! path changed" for 7988e3427^1..7988e3427 and `release-channel.sh` answers `channel=agent`,
//! which is the whole of this change in one range anybody can replay in a full checkout.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root is two levels above this crate")
}

/// A throwaway git repository carrying one file from each path set the release predicates read.
struct Checkout {
    dir: PathBuf,
}

impl Checkout {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "arbos-release-channel-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let checkout = Checkout { dir };
        checkout.git(&["init", "-q"]);
        checkout
    }

    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(&self.dir)
            .args(args)
            // Nothing from the machine's own git config, and a fixed identity: the test must read
            // the same on a developer's laptop as on the runner.
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "release test")
            .env("GIT_AUTHOR_EMAIL", "release@arbos.life")
            .env("GIT_COMMITTER_NAME", "release test")
            .env("GIT_COMMITTER_EMAIL", "release@arbos.life")
            .output()
            .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn write(&self, rel: &str, body: &str) {
        let path = self.dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn commit(&self, message: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }

    /// A tree with a file on every path set, and the sha both release bases start at.
    fn base_commit(&self, lock: &str) -> String {
        self.write("Cargo.lock", lock);
        self.write("crates/arbos-mac/src/app.rs", "fn main() {}\n");
        self.write("crates/arbos-core/src/lib.rs", "pub fn core() {}\n");
        self.write("crates/arbos-agentd/src/main.rs", "fn main() {}\n");
        self.write("crates/arbos-relay/src/lib.rs", "pub fn relay() {}\n");
        self.write("image/Dockerfile", "FROM debian:trixie-slim\n");
        self.write("scripts/bundle-mac.sh", "#!/usr/bin/env bash\n");
        self.commit("base")
    }

    /// The decision, as the `key=value` lines the script prints.
    fn channel(&self, mac_base: &str, image_base: &str, target: &str) -> HashMap<String, String> {
        let out = Command::new(repo_root().join("scripts/release/release-channel.sh"))
            .current_dir(&self.dir)
            .args([mac_base, image_base, target])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("release-channel.sh is executable and runs");
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(
            out.status.success(),
            "release-channel.sh reports, it does not refuse; it exited {:?}: {stdout} {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
        stdout
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }
}

impl Drop for Checkout {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fixture(pair: &str, side: &str) -> String {
    let path = repo_root()
        .join("scripts/release/testdata")
        .join(format!("{pair}-{side}.lock"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn lock() -> String {
    fixture("lock-version-only", "before")
}

// ── the gap this file was written for ───────────────────────────────────────────────────────────

#[test]
fn an_agentd_only_range_cuts_the_agent_image() {
    let checkout = Checkout::new("agentd-only");
    let base = checkout.base_commit(&lock());
    checkout.write(
        "crates/arbos-agentd/src/main.rs",
        "fn main() { supervise(); }\n",
    );
    let target = checkout.commit("fix: agentd restarts a dead workspace");

    let plan = checkout.channel(&base, &base, &target);
    assert_eq!(
        plan["channel"], "agent",
        "an agentd fix must cut its own release rather than wait for a Mac merge: {plan:?}"
    );
    assert_eq!(plan["mac"], "no", "an agentd fix is not a Mac release");
    assert_eq!(plan["agent"], "yes");
    assert!(
        plan["agent_why"].contains("crates/arbos-agentd/src/main.rs"),
        "the reason names the file that moved: {}",
        plan["agent_why"]
    );
}

#[test]
fn an_image_only_range_cuts_the_agent_image() {
    let checkout = Checkout::new("image-only");
    let base = checkout.base_commit(&lock());
    checkout.write("image/Dockerfile", "FROM debian:trixie-slim\nRUN true\n");
    let target = checkout.commit("fix: the sandbox image pins the browser build");

    let plan = checkout.channel(&base, &base, &target);
    assert_eq!(
        plan["channel"], "agent",
        "the image directory builds the image, so a change to it must rebuild it: {plan:?}"
    );
}

#[test]
fn an_agentd_fix_already_behind_the_image_tag_cuts_nothing() {
    // The same fix, read a second time after its image shipped. Without this the schedule would
    // cut a new image every fifteen minutes for a range that is already out.
    let checkout = Checkout::new("agentd-released");
    let base = checkout.base_commit(&lock());
    checkout.write(
        "crates/arbos-agentd/src/main.rs",
        "fn main() { supervise(); }\n",
    );
    let target = checkout.commit("fix: agentd restarts a dead workspace");

    let plan = checkout.channel(&base, &target, &target);
    assert_eq!(plan["channel"], "none", "{plan:?}");
    assert_eq!(plan["agent"], "no");
}

#[test]
fn a_checkout_with_no_image_tag_cuts_no_agent_image() {
    // The `image-v0.1.x` namespace is written by whichever side gets there first, and before the
    // first one exists there is no baseline to measure a range against. Guessing one would cut an
    // image for the whole history of the repository, so the answer is "no", said out loud.
    let checkout = Checkout::new("no-image-tag");
    let base = checkout.base_commit(&lock());
    checkout.write(
        "crates/arbos-agentd/src/main.rs",
        "fn main() { supervise(); }\n",
    );
    let target = checkout.commit("fix: agentd restarts a dead workspace");

    let plan = checkout.channel(&base, "", &target);
    assert_eq!(plan["channel"], "none", "{plan:?}");
    assert!(
        plan["agent_why"].contains("no image-v"),
        "it says which baseline is missing: {}",
        plan["agent_why"]
    );
}

// ── the Mac side, which none of the above may disturb ───────────────────────────────────────────

#[test]
fn a_mac_fix_still_cuts_a_stable_release() {
    let checkout = Checkout::new("mac-fix");
    let base = checkout.base_commit(&lock());
    checkout.write("crates/arbos-mac/src/app.rs", "fn main() { paint(); }\n");
    let target = checkout.commit("fix: the Mac window paints again");

    let plan = checkout.channel(&base, &base, &target);
    assert_eq!(plan["channel"], "stable", "{plan:?}");
    assert_eq!(plan["mac"], "yes");
}

#[test]
fn a_range_on_both_path_sets_is_one_mac_cut() {
    // crates/arbos-core is on both lists, and so is any merge batch that moves the Mac app and
    // agentd together. Signing and notarizing costs about fourteen minutes, and the Mac cut
    // already rebuilds and pushes the image, so the two must not both fire.
    let checkout = Checkout::new("both");
    let base = checkout.base_commit(&lock());
    checkout.write(
        "crates/arbos-core/src/lib.rs",
        "pub fn core() { fixed(); }\n",
    );
    let target = checkout.commit("fix: a core change both the app and agentd link");

    let plan = checkout.channel(&base, &base, &target);
    assert_eq!(plan["channel"], "stable", "{plan:?}");
    assert_eq!(plan["mac"], "yes");
    assert_eq!(
        plan["agent"], "yes",
        "the agent side still reports what it saw, so the log says why it did not cut"
    );
}

#[test]
fn a_relay_only_range_cuts_nothing_and_says_so_twice() {
    // The case that must stay a non-release, and the reason design call 3 exists: `channel=none`
    // used to look the same whether it was a healthy skip or the agentd hole. Both predicates now
    // report, so the log distinguishes them.
    let checkout = Checkout::new("relay-only");
    let base = checkout.base_commit(&lock());
    checkout.write(
        "crates/arbos-relay/src/lib.rs",
        "pub fn relay() { fixed(); }\n",
    );
    let target = checkout.commit("fix: the relay keeps its websocket");

    let plan = checkout.channel(&base, &base, &target);
    assert_eq!(plan["channel"], "none", "{plan:?}");
    assert_eq!(plan["mac_why"], "no Mac path changed");
    assert_eq!(plan["agent_why"], "no agent image path changed");
}

#[test]
fn a_carried_forward_version_bump_cuts_nothing_on_either_side() {
    // Cargo.lock is on both path lists, and a release bump fast-forwards onto main, so every
    // later commit carries its version lines. A plain path diff would cut both artifacts for
    // twelve lines of our own version numbers; both sides read the lockfile instead.
    let checkout = Checkout::new("version-bump");
    let base = checkout.base_commit(&fixture("lock-version-only", "before"));
    checkout.write("Cargo.lock", &fixture("lock-version-only", "after"));
    let target = checkout.commit("release: 0.6.367 version bump carried forward");

    let plan = checkout.channel(&base, &base, &target);
    assert_eq!(plan["channel"], "none", "{plan:?}");
    assert_eq!(
        plan["mac_why"],
        "Cargo.lock moved only our own crates' version numbers"
    );
    assert_eq!(
        plan["agent_why"],
        "Cargo.lock moved only our own crates' version numbers"
    );
}

#[test]
fn a_dependency_change_reaches_both_artifacts() {
    let checkout = Checkout::new("dependency");
    let base = checkout.base_commit(&fixture("lock-new-dependency", "before"));
    checkout.write("Cargo.lock", &fixture("lock-new-dependency", "after"));
    let target = checkout.commit("two objc2-av packages arrive");

    let plan = checkout.channel(&base, &base, &target);
    assert_eq!(plan["channel"], "stable", "{plan:?}");
    assert_eq!(
        plan["agent"], "yes",
        "a dependency change is in the image too"
    );
}

// ── the path list is the image's closure, not a guess at it ─────────────────────────────────────

#[test]
fn the_agent_path_list_watches_every_crate_the_image_links() {
    // The same property `release-gate.py` holds the Mac list to. `crates/arbos-agentd` is copied
    // into the image as a binary, so every workspace crate it links is in the image, and a fix in
    // one of them that the list does not watch would ship to boxes with no image cut for it.
    let paths = Command::new(repo_root().join("scripts/release/agent-paths.sh"))
        .output()
        .expect("agent-paths.sh is executable and runs");
    let watched: Vec<String> = String::from_utf8_lossy(&paths.stdout)
        .split_whitespace()
        .map(str::to_string)
        .collect();

    let manifest =
        std::fs::read_to_string(repo_root().join("crates/arbos-agentd/Cargo.toml")).unwrap();
    let linked: Vec<&str> = manifest
        .lines()
        .take_while(|line| !line.starts_with("[dev-dependencies]"))
        .filter_map(|line| line.split_once(" = "))
        .map(|(name, _)| name.trim())
        .filter(|name| name.starts_with("arbos-"))
        .collect();
    assert!(
        !linked.is_empty(),
        "arbos-agentd's manifest names the workspace crates it links"
    );
    for crate_name in linked {
        assert!(
            watched.iter().any(|p| p == &format!("crates/{crate_name}")),
            "the image links {crate_name}, which scripts/release/agent-paths.sh does not watch, \
             so a fix in it would reach boxes with no image cut for it"
        );
    }
    for required in ["crates/arbos-agentd", "image", "harness", "Cargo.lock"] {
        assert!(
            watched.iter().any(|p| p == required),
            "{required} builds the image and must be watched"
        );
    }
}

#[test]
fn the_agent_path_list_covers_everything_the_deploy_worker_rolls_for() {
    // Two sides read "did the agent image change?": this list decides whether release.yml cuts an
    // `image-v0.1.N`, and `paths_of agentd` in scripts/deploy/arbos-deploy-worker decides whether
    // the relay host rolls one. A path the worker rolls for and this list does not watch is a fix
    // that reaches boxes with no release cut for it — the Mac-list hole of 2026-11-04, one
    // artifact over. `harness` was exactly that: image/Dockerfile bind-mounts
    // `harness/arbos-pi-ext` rather than naming it in a COPY, so reading the Dockerfile for COPY
    // lines misses it.
    //
    // A superset, not an equality. This list also watches Cargo.lock, which the worker's does
    // not, so a dependency bump cuts an image here and does not roll one there.
    let worker =
        std::fs::read_to_string(repo_root().join("scripts/deploy/arbos-deploy-worker")).unwrap();
    let line = worker
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("agentd) echo \""))
        .expect("arbos-deploy-worker's paths_of still has an `agentd) echo \"…\"` arm");
    let rolled: Vec<&str> = line
        .trim_start_matches("agentd) echo \"")
        .split('"')
        .next()
        .expect("the arm's path list is quoted")
        .split_whitespace()
        .collect();
    assert!(
        rolled.len() >= 3,
        "the worker's agentd path list parsed as {rolled:?}, which is not a path list"
    );

    let paths = Command::new(repo_root().join("scripts/release/agent-paths.sh"))
        .output()
        .expect("agent-paths.sh is executable and runs");
    let watched: Vec<String> = String::from_utf8_lossy(&paths.stdout)
        .split_whitespace()
        .map(str::to_string)
        .collect();
    for path in rolled {
        assert!(
            watched.iter().any(|p| p == path),
            "scripts/deploy/arbos-deploy-worker rolls a new image when {path} changes, and \
             scripts/release/agent-paths.sh does not watch it, so that change would reach boxes \
             with no image release cut for it"
        );
    }
}
