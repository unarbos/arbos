//! Which merge cuts a Mac release: `scripts/release/mac-change.sh` and the Cargo.lock reader it
//! leans on, `scripts/release/cargo-lock-change.py`.
//!
//! The bug these guard against: a release bump commit writes the new version into Cargo.toml and
//! Cargo.lock and then fast-forwards onto main, so every commit after it carries the bump's
//! version lines in Cargo.lock. Cargo.lock is one of the paths the Mac app is built from, so the
//! old `git diff --quiet -- $MAC_PATHS` check saw a difference after every merge and cut a Mac
//! release every time. v0.6.368 is the proof: a relay-only fix whose entire Mac-path difference
//! was twelve lines of `version = "0.6.366"` becoming `version = "0.6.367"`, released at the cost
//! of a full 32-minute Mac cycle.
//!
//! The fix must hold a sharp line. Our own crates' version numbers moving is not a Mac change.
//! Anything else in Cargo.lock is — a dependency arriving or leaving, a third-party version or
//! checksum moving, a dependency list changing. Letting one of those through would stop a genuine
//! dependency change from reaching Mac users, which is worse than the wasted builds.
//!
//! The lockfiles under `scripts/release/testdata` are real Cargo.lock content from this
//! repository, taken verbatim at the commits named in each file's header and cut down to the
//! package blocks the test needs. Where a case needs something this repository's history does not
//! contain — nobody has ever changed a third-party version in place here — the test edits one
//! line of that real content and says so.

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root is two levels above this crate")
}

fn fixture(pair: &str, side: &str) -> String {
    let path = repo_root()
        .join("scripts/release/testdata")
        .join(format!("{pair}-{side}.lock"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "arbos-release-test-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `true` when the two lockfiles differ in something other than our own crates' version numbers,
/// i.e. when the Mac app must be built and released.
fn is_dependency_change(before: &str, after: &str) -> bool {
    let dir = tempdir("lock");
    std::fs::write(dir.join("before.lock"), before).unwrap();
    std::fs::write(dir.join("after.lock"), after).unwrap();
    let out = Command::new("python3")
        .arg(repo_root().join("scripts/release/cargo-lock-change.py"))
        .arg(dir.join("before.lock"))
        .arg(dir.join("after.lock"))
        .output()
        .expect("python3 runs cargo-lock-change.py");
    let _ = std::fs::remove_dir_all(&dir);
    match out.status.code() {
        Some(0) => false,
        Some(1) => true,
        other => panic!(
            "cargo-lock-change.py exited {other:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        ),
    }
}

/// A throwaway git repository with Mac and non-Mac sources, for the whole decision end to end.
/// CI checks out main one commit deep, so the test cannot read real history and builds its own
/// out of the real lockfiles instead.
struct Checkout {
    dir: PathBuf,
}

impl Checkout {
    fn new(tag: &str) -> Self {
        let checkout = Checkout { dir: tempdir(tag) };
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

    /// The decision itself: `(a release is needed, the one-line reason)`.
    fn mac_change(&self, base: &str, target: &str) -> (bool, String) {
        let out = Command::new(repo_root().join("scripts/release/mac-change.sh"))
            .current_dir(&self.dir)
            .args([base, target])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("mac-change.sh is executable and runs");
        let reason = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let needed = match out.status.code() {
            Some(0) => false,
            Some(1) => true,
            other => panic!(
                "mac-change.sh exited {other:?}: {reason} {}",
                String::from_utf8_lossy(&out.stderr)
            ),
        };
        (needed, reason)
    }
}

impl Drop for Checkout {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Lay down a base commit with a lockfile, Mac sources and relay sources, and return its sha.
fn base_commit(checkout: &Checkout, lock: &str) -> String {
    checkout.write("Cargo.lock", lock);
    checkout.write("crates/arbos-mac/src/app.rs", "fn main() {}\n");
    checkout.write("crates/arbos-core/src/lib.rs", "pub fn core() {}\n");
    checkout.write("crates/arbos-relay/src/lib.rs", "pub fn relay() {}\n");
    checkout.write("scripts/bundle-mac.sh", "#!/usr/bin/env bash\n");
    checkout.commit("base")
}

// ── the five cases the release pipeline turns on ────────────────────────────────────────────────

#[test]
fn version_only_lock_change_does_not_release() {
    let checkout = Checkout::new("version-only");
    let base = base_commit(&checkout, &fixture("lock-version-only", "before"));
    checkout.write("Cargo.lock", &fixture("lock-version-only", "after"));
    let target = checkout.commit("release: 0.6.367 version bump carried forward");

    let (needed, reason) = checkout.mac_change(&base, &target);
    assert!(
        !needed,
        "v0.6.368's lock diff is twelve of our own version lines and nothing else: {reason}"
    );
    assert_eq!(
        reason,
        "Cargo.lock moved only our own crates' version numbers"
    );
}

#[test]
fn a_real_dependency_change_releases() {
    let checkout = Checkout::new("dependency");
    let base = base_commit(&checkout, &fixture("lock-new-dependency", "before"));
    checkout.write("Cargo.lock", &fixture("lock-new-dependency", "after"));
    let target = checkout.commit("objc2-av-foundation and objc2-av-kit arrive");

    let (needed, reason) = checkout.mac_change(&base, &target);
    assert!(needed, "two new packages must reach Mac users: {reason}");
    assert_eq!(reason, "Cargo.lock changed a dependency");
}

#[test]
fn a_dependency_change_mixed_with_version_lines_releases() {
    let checkout = Checkout::new("mixed");
    let base = base_commit(&checkout, &fixture("lock-mixed", "before"));
    checkout.write("Cargo.lock", &fixture("lock-mixed", "after"));
    let target = checkout.commit("0.6.330 -> 0.6.331 and two objc2-av packages");

    let (needed, reason) = checkout.mac_change(&base, &target);
    assert!(
        needed,
        "the version lines must not hide the two new packages beside them: {reason}"
    );
    assert_eq!(reason, "Cargo.lock changed a dependency");
}

#[test]
fn mac_code_without_a_lock_change_releases() {
    let checkout = Checkout::new("mac-code");
    let lock = fixture("lock-version-only", "before");
    let base = base_commit(&checkout, &lock);
    checkout.write("crates/arbos-mac/src/app.rs", "fn main() { paint(); }\n");
    let target = checkout.commit("fix: the Mac window paints again");

    let (needed, reason) = checkout.mac_change(&base, &target);
    assert!(needed, "a Mac fix always ships: {reason}");
    assert_eq!(reason, "Mac sources changed: crates/arbos-mac/src/app.rs");
}

#[test]
fn a_relay_fix_carrying_the_version_bump_does_not_release() {
    let checkout = Checkout::new("relay-only");
    let base = base_commit(&checkout, &fixture("lock-version-only", "before"));
    // Exactly v0.6.368: a fix outside the Mac paths, and a lockfile that moved only because the
    // release bump before it fast-forwarded onto main.
    checkout.write(
        "crates/arbos-relay/src/lib.rs",
        "pub fn relay() { fixed(); }\n",
    );
    checkout.write("Cargo.lock", &fixture("lock-version-only", "after"));
    let target = checkout.commit("fix(feedback): relay event attachments");

    let (needed, reason) = checkout.mac_change(&base, &target);
    assert!(!needed, "no Mac code, no Mac release: {reason}");
    assert_eq!(
        reason,
        "Cargo.lock moved only our own crates' version numbers"
    );
}

// ── the boundary, checked line by line ──────────────────────────────────────────────────────────

#[test]
fn an_unchanged_tree_does_not_release() {
    let checkout = Checkout::new("unchanged");
    let base = base_commit(&checkout, &fixture("lock-version-only", "before"));
    checkout.write("docs/notes.md", "nothing the Mac app is built from\n");
    let target = checkout.commit("docs only");

    assert_eq!(
        checkout.mac_change(&base, &target),
        (false, "no Mac path changed".to_string())
    );
}

#[test]
fn a_dependency_that_leaves_releases() {
    // The same real pair read backwards: a commit that drops objc2-av-foundation and objc2-av-kit,
    // which is what a revert of the inline-video fix would have written.
    assert!(is_dependency_change(
        &fixture("lock-new-dependency", "after"),
        &fixture("lock-new-dependency", "before")
    ));
}

#[test]
fn a_third_party_version_releases() {
    // Nobody in this repository's history has yet moved a third-party version in place, so this
    // one line of real content is edited: archery 1.2.3 -> 1.2.4, the way `cargo update` would.
    let before = fixture("lock-version-only", "after");
    let after = before.replacen("version = \"1.2.3\"", "version = \"1.2.4\"", 1);
    assert_ne!(before, after, "archery 1.2.3 is in the fixture");
    assert!(
        is_dependency_change(&before, &after),
        "a third-party version is not ours to ignore"
    );
}

#[test]
fn a_changed_checksum_releases() {
    // Same idea: the same crate version, re-published with different bytes. One real checksum line
    // is edited; everything else is the real lockfile.
    let before = fixture("lock-version-only", "after");
    let old = "checksum = \"3bc62ac97cc33321f50863d514c3bc38a453947a8f9e781137e47c7401020aed\"";
    assert!(before.contains(old), "arbitrary 1.5.0's checksum is there");
    let after = before.replacen(
        old,
        "checksum = \"0000000000000000000000000000000000000000000000000000000000000000\"",
        1,
    );
    assert!(is_dependency_change(&before, &after));
}

#[test]
fn a_dependency_added_to_one_of_our_own_crates_releases() {
    // arbos-core gains a dependency while its version moves. The version line is ignorable; the
    // new entry in its dependency list is not.
    let before = fixture("lock-version-only", "before");
    let after = fixture("lock-version-only", "after").replacen(
        "name = \"arbos-core\"\nversion = \"0.6.367\"\ndependencies = [\n",
        "name = \"arbos-core\"\nversion = \"0.6.367\"\ndependencies = [\n \"archery\",\n",
        1,
    );
    assert!(
        after.contains(" \"archery\","),
        "the dependency list of arbos-core is shaped as expected"
    );
    assert!(is_dependency_change(&before, &after));
}

#[test]
fn the_lockfile_format_version_is_not_one_of_ours() {
    // `version = 4` above the first [[package]] is the lockfile format, not a crate. It sits in a
    // part of the file with no `source` line anywhere near it, so a reader that looked only for
    // "a version line with no source beside it" would wrongly ignore it.
    let before = fixture("lock-version-only", "before");
    let after = before.replacen("\nversion = 4\n", "\nversion = 5\n", 1);
    assert_ne!(before, after, "the format version is on its own line");
    assert!(is_dependency_change(&before, &after));
}

#[test]
fn our_own_crates_are_the_ones_with_no_source() {
    // What the whole rule rests on: in a lockfile the packages with no `source` key are exactly
    // the workspace members. If that ever stops being true of the real Cargo.lock, every test
    // above is measuring the wrong thing.
    let lock = std::fs::read_to_string(repo_root().join("Cargo.lock")).unwrap();
    let no_source: Vec<&str> = lock
        .split("[[package]]")
        .skip(1)
        .filter(|block| !block.contains("source = "))
        .filter_map(|block| {
            block
                .lines()
                .find_map(|l| l.strip_prefix("name = \""))
                .map(|n| n.trim_end_matches('"'))
        })
        .collect();
    let manifest = std::fs::read_to_string(repo_root().join("Cargo.toml")).unwrap();
    let members: Vec<&str> = manifest
        .lines()
        .filter_map(|l| l.trim().strip_prefix("\"crates/"))
        .map(|l| l.trim_end_matches("\",").trim_end_matches('"'))
        .collect();
    assert!(!members.is_empty(), "the workspace lists its members");
    let mut found = no_source.clone();
    let mut want = members.clone();
    found.sort_unstable();
    want.sort_unstable();
    assert_eq!(found, want, "every path package is a workspace member");
}
