//! Keeping an `arbos-kernel` binary current from the same signed feed the
//! desktop uses.
//!
//! Why this is here and not in the kernel: nothing in it is kernel-specific
//! except knowing how to read `arbos-kernel --version`. The feed, the
//! signature, the staging and the rollback are the same code the app updates
//! itself with, and putting the decision here means there is one answer to
//! "may this binary be replaced, and with what" rather than two that can
//! drift.
//!
//! It also means the whole mechanism can be run by hand — `arbos-updatectl
//! kernel --install` — before anything runs unattended. A kernel on a box
//! nobody ssh's into and nobody attaches to is exactly the one that goes
//! stale, and it can be fixed today without the kernel having learned
//! anything.
//!
//! ## What this deliberately does not do
//!
//! It does not decide *when*. A kernel is serving live agents, and the only
//! safe moment is between turns with nothing waiting on a human — which the
//! kernel's own `idle::verdict` already answers and this crate cannot see.
//! Every function here is "is this allowed, and do it"; the caller owns the
//! moment.

use crate::{
    feed::{Available, Component, Platform, current_arch},
    install,
    sign::PublicKey,
    version::Version,
};
use anyhow::{Context, Result, bail};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

/// What a kernel binary says it is.
///
/// `arbos-kernel --version` prints `arbos-kernel 0.2.0 3fff013a2b4c protocol 1`.
/// The commit is what decides whether it is current: the marketing version has
/// not moved off `0.2.0` in weeks, so on its own it cannot tell this morning's
/// build from last week's — which is the whole reason a kernel went stale
/// without anybody noticing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    pub path: PathBuf,
    pub version: Version,
    /// Short git sha, as the binary was stamped. `unknown` for a build made
    /// outside a repository.
    pub sha: String,
    pub protocol: u32,
}

impl Running {
    /// Ask a binary what it is, by running it.
    ///
    /// Running it rather than reading it: the version is compiled in, and a
    /// binary that will not execute is one there is no point updating *to*,
    /// so this doubles as the check that a staged payload works before it is
    /// moved into place.
    pub fn read(binary: &Path) -> Result<Self> {
        let out = Command::new(binary)
            .arg("--version")
            .output()
            .with_context(|| format!("running {} --version", binary.display()))?;
        if !out.status.success() {
            bail!("{} --version exited {}", binary.display(), out.status);
        }
        let line = String::from_utf8_lossy(&out.stdout);
        Self::parse(line.lines().next().unwrap_or_default(), binary)
    }

    /// `arbos-kernel <semver> <sha> protocol <n>`.
    pub fn parse(line: &str, binary: &Path) -> Result<Self> {
        let mut words = line.split_whitespace();
        let name = words.next().unwrap_or_default();
        if name != "arbos-kernel" {
            bail!("`{line}` is not an arbos-kernel version line");
        }
        let version = Version::parse(words.next().unwrap_or_default())
            .with_context(|| format!("version in `{line}`"))?;
        let sha = words.next().unwrap_or("unknown").to_owned();
        // `protocol <n>` — absent in an old enough build, which is a build
        // worth replacing rather than refusing to look at.
        let protocol = match (words.next(), words.next()) {
            (Some("protocol"), Some(n)) => n.parse().unwrap_or(0),
            _ => 0,
        };
        Ok(Self {
            path: binary.to_path_buf(),
            version,
            sha,
            protocol,
        })
    }
}

/// Why a kernel is not going to be updated.
///
/// Refusals are answers, not errors: a kernel that says "I am a build tree,
/// leave me alone" is behaving correctly, and the caller reports it rather
/// than retrying it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The binary lives under a `target/debug` or `target/release` directory:
    /// somebody is working on it. Swapping it under them would replace the
    /// thing they just built with something from the internet.
    BuildTree(PathBuf),
    /// Held at a version by configuration.
    Pinned(String),
    /// The running build is the newest the channel has.
    Current,
    /// The running build is *newer* than the channel's newest — a local build,
    /// almost always. The same rule the ssh path has always used.
    Ahead,
    /// The channel publishes nothing for this platform.
    NoPayload,
}

impl Refusal {
    /// One line, for a log or a status field.
    pub fn say(&self) -> String {
        match self {
            Self::BuildTree(path) => format!(
                "{} is inside a build tree, so it is somebody's working copy and will not be \
                 replaced",
                path.display()
            ),
            Self::Pinned(at) => format!("pinned to {at}"),
            Self::Current => "already the newest build on this channel".into(),
            Self::Ahead => "newer than anything the channel has published".into(),
            Self::NoPayload => "the channel publishes no kernel for this platform".into(),
        }
    }
}

/// Whether `running` should be replaced by what the feed offers, or why not.
///
/// The rule is `remote_kernel::needs_update_to`'s, which the ssh path has used
/// since #229, so a kernel updated over ssh and a kernel updating itself agree
/// about what "behind" means:
///
/// - older marketing version — behind
/// - same version, different commit — behind
/// - same version, same commit — current
/// - newer version — ahead, and left alone
pub fn behind(running: &Running, offered: &Available) -> Result<(), Refusal> {
    match running.version.cmp_release(&offered.version) {
        std::cmp::Ordering::Less => Ok(()),
        std::cmp::Ordering::Greater => Err(Refusal::Ahead),
        std::cmp::Ordering::Equal => match same_commit(&running.sha, &offered.commit) {
            true => Err(Refusal::Current),
            false => Ok(()),
        },
    }
}

/// Whether two commits name the same build.
///
/// Public because the app needs it for a second question with the same shape:
/// **is the kernel I am about to attach to the one this bundle ships?**
/// `kernel.json` carries the `git_sha` of the process that wrote it and
/// `hello` carries it too, so comparing either with the bundled kernel's own
/// commit says whether a running kernel is this build or a survivor of an
/// older one. On 2026-09-17 a survivor 223 commits behind was attached to
/// after an update and sent frames it had never heard of.
///
/// One is short and one is long, so whichever is shorter decides. An
/// `unknown` sha — a build from a tarball — matches nothing, which is right:
/// a build that cannot account for itself is not one to trust as current.
pub fn same_commit(running: &str, offered: &str) -> bool {
    if running.is_empty() || offered.is_empty() || running == "unknown" {
        return false;
    }
    let n = running.len().min(offered.len());
    running[..n].eq_ignore_ascii_case(&offered[..n])
}

/// Whether this binary is one somebody is working on.
///
/// `cargo build` puts binaries under `target/debug` or `target/release`. A
/// kernel running from there belongs to whoever is building it, and replacing
/// it would both confuse them and be undone by their next build.
pub fn in_build_tree(binary: &Path) -> bool {
    let mut components = binary.components().rev().skip(1);
    matches!(
        components.next().and_then(|c| c.as_os_str().to_str()),
        Some("debug" | "release")
    ) && matches!(
        components.next().and_then(|c| c.as_os_str().to_str()),
        Some("target")
    )
}

/// Everything except the moment: is there a newer kernel for this machine, and
/// may this binary be replaced by it?
pub fn plan(
    running: &Running,
    feed: &crate::Feed,
    pin: Option<&str>,
) -> Result<Available, Refusal> {
    if in_build_tree(&running.path) {
        return Err(Refusal::BuildTree(running.path.clone()));
    }
    if let Some(pin) = pin {
        return Err(Refusal::Pinned(pin.to_owned()));
    }
    let platform = Platform::current().ok_or(Refusal::NoPayload)?;
    let offered = feed
        .newest(platform, current_arch(), Component::Kernel)
        .ok_or(Refusal::NoPayload)?;
    behind(running, &offered)?;
    Ok(offered)
}

/// How hard the staged binary is tried before it replaces anything.
///
/// This matters more than it looks. The swap is followed by an `execv`, and
/// **`execv` only returns an error when the exec itself fails** — a binary
/// that starts and then dies while booting is not caught by it. There is no
/// "it came up, so keep it" moment afterwards, because by then the old image
/// is gone. So whatever confidence there is has to be bought here, before the
/// old binary is moved aside.
#[derive(Debug, Clone, Copy)]
pub enum Probe<'a> {
    /// It runs, and reports the version the feed promised. The floor: it
    /// proves the file is an executable for this machine that links and
    /// reaches `main`.
    Version,
    /// That, and it reads a real place no worse than the binary it is
    /// replacing.
    ///
    /// `arbos-kernel check <place>` walks the store with the same parsers
    /// `serve` boots on, so a new build that cannot read this machine's
    /// agents says so here rather than after the exec. It is compared with
    /// the *old* binary's answer rather than required to be clean, because a
    /// place with pre-existing errors is not the new build's fault and
    /// refusing on it would make every update impossible on exactly the
    /// machines that need one.
    Place(&'a Path),
}

impl Probe<'_> {
    /// Run it. `staged` is the candidate; `current` is what it would replace.
    fn run(self, staged: &Path, current: &Path, expected: &Version) -> Result<()> {
        let new = Running::read(staged).context("the downloaded kernel would not run")?;
        if new.version.cmp_release(expected) != std::cmp::Ordering::Equal {
            bail!(
                "the download says it is {} but the feed said {}",
                new.version.human(),
                expected.human()
            );
        }
        let Self::Place(place) = self else {
            return Ok(());
        };
        let theirs = read_place(current, place);
        let ours = read_place(staged, place).with_context(|| {
            format!(
                "the downloaded kernel could not read {} at all",
                place.display()
            )
        })?;
        if let Ok(theirs) = theirs
            && ours > theirs
        {
            bail!(
                "the downloaded kernel finds {ours} problems in {} where the one it would \
                 replace finds {theirs} — refusing it rather than serving with it",
                place.display()
            );
        }
        Ok(())
    }
}

/// How many errors a binary sees in a place. `check` exits non-zero when it
/// finds any, so the count comes from the report rather than the status.
fn read_place(binary: &Path, place: &Path) -> Result<usize> {
    let out = Command::new(binary)
        .arg("check")
        .arg(place)
        .arg("--json")
        .output()
        .with_context(|| format!("running {} check", binary.display()))?;
    let report: serde_json::Value = serde_json::from_slice(&out.stdout)
        .with_context(|| format!("reading what {} said about {}", binary.display(), place.display()))?;
    Ok(report
        .get("findings")
        .and_then(|f| f.as_array())
        .map(|findings| {
            findings
                .iter()
                .filter(|f| f.get("level").and_then(|l| l.as_str()) == Some("error"))
                .count()
        })
        .unwrap_or(0))
}

/// Put a verified payload in place of `target`.
///
/// The same two renames the app's install uses, so the same promise holds:
/// there is no moment at which the binary is half replaced. What is different
/// is that the check is "does the new binary run and say it is what the feed
/// said", which for a single executable is a stronger test than looking at the
/// file.
///
/// `payload` must already have been checked against the feed
/// ([`Download::check_payload`]) — this does not download and does not verify
/// a signature, because a function that sometimes verifies is one that
/// sometimes does not.
pub fn install_payload(
    payload: &Path,
    offered: &Available,
    target: &Path,
    probe: Probe<'_>,
) -> Result<()> {
    let done = staged(payload, offered, target, probe);
    // However that went, the staging directory goes. A payload that would not
    // run is the common failure, and leaving its unpacked remains beside the
    // binary means the next attempt starts by tripping over them.
    if let Ok(staging) = install::staging_for(target) {
        let _ = std::fs::remove_dir_all(staging);
    }
    done
}

fn staged(
    payload: &Path,
    offered: &Available,
    target: &Path,
    probe: Probe<'_>,
) -> Result<()> {
    let staged_dir = install::unpack(payload, offered.download.format, target)?;
    // The tarball carries one directory with the binary in it.
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("arbos-kernel");
    let staged = match staged_dir.join(name).is_file() {
        true => staged_dir.join(name),
        // A payload that is the bare binary rather than a directory holding it.
        false if staged_dir.is_file() => staged_dir.clone(),
        false => bail!(
            "the download has no {name} in it ({} is missing)",
            staged_dir.join(name).display()
        ),
    };

    // Before anything is moved. This is the only place confidence can be
    // bought: after the swap comes an execv, and execv does not report a
    // binary that starts and then dies.
    probe.run(&staged, target, &offered.version)?;

    // And a way back, for the failure the probe cannot see. Cheap — one
    // binary — and it turns "the new kernel dies at boot on a box nobody
    // watches" from unrecoverable into one `mv`.
    let previous = previous_path(target);
    let _ = std::fs::remove_file(&previous);
    std::fs::copy(target, &previous)
        .with_context(|| format!("keeping the current binary as {}", previous.display()))?;

    let swap = install::Swap::begin(target, &staged)?;
    // In place now. A failure here puts the old binary back before the error
    // reaches anybody — `Swap` does it from its `Drop`.
    if let Err(e) = Running::read(target) {
        // `Swap` puts the old binary back as it unwinds; the copy beside it
        // is then redundant.
        let _ = std::fs::remove_file(&previous);
        return Err(e).context("the new kernel did not survive being moved into place");
    }
    swap.commit()
}

/// Where the binary that was replaced is kept.
///
/// Not hidden, unlike the staging and backup names: this one is meant to be
/// found. Somebody looking at a kernel that will not start should see the one
/// that did, sitting next to it.
pub fn previous_path(target: &Path) -> PathBuf {
    let mut name = target.as_os_str().to_owned();
    name.push(".previous");
    PathBuf::from(name)
}

/// Check a payload's bytes and put it in place. The whole of the install, for
/// a caller that already has the bytes.
pub fn verify_and_install(
    bytes: &[u8],
    offered: &Available,
    target: &Path,
    key: &PublicKey,
    scratch: &Path,
    probe: Probe<'_>,
) -> Result<()> {
    offered.download.check_url()?;
    offered
        .download
        .check_payload(bytes, key)
        .context("the download did not verify")?;
    std::fs::create_dir_all(scratch)
        .with_context(|| format!("making {}", scratch.display()))?;
    let file = scratch.join(offered.download.file_name());
    std::fs::write(&file, bytes).with_context(|| format!("writing {}", file.display()))?;
    let installed = install_payload(&file, offered, target, probe);
    let _ = std::fs::remove_file(&file);
    installed
}

/// Where `arbos-kernel` is on this machine, when it can be found without being
/// told: beside the caller, then on `PATH`.
pub fn find(named: Option<&Path>) -> Result<PathBuf> {
    if let Some(named) = named {
        return match named.is_file() {
            true => Ok(named.to_path_buf()),
            false => bail!("{} is not a file", named.display()),
        };
    }
    if let Some(beside) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("arbos-kernel")))
        .filter(|path| path.is_file())
    {
        return Ok(beside);
    }
    let out = Command::new("command")
        .args(["-v", "arbos-kernel"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .filter(|path| !path.is_empty());
    match out {
        Some(path) => Ok(PathBuf::from(path)),
        None => bail!("no arbos-kernel beside this program or on PATH; name one with --binary"),
    }
}

/// The name a kernel payload is published under.
pub fn payload_name(version: &Version, platform: Platform, arch: &str) -> String {
    format!(
        "arbos-kernel-{}.{}.{}-{}-{}-{arch}.tar.gz",
        version.major,
        version.minor,
        version.patch,
        version.build,
        platform.as_str()
    )
}

#[cfg(test)]
mod tests {
    use super::{Refusal, Running, behind, in_build_tree, payload_name, plan};
    use crate::{
        Feed, Version,
        feed::{Channel, Component, Download, Format, Platform, Release},
        sign,
    };
    use std::path::{Path, PathBuf};

    const HOST: &str = "https://github.com/unarbos/arbos/releases/download/dev";

    fn running(version: &str, sha: &str, path: &str) -> Running {
        Running {
            path: PathBuf::from(path),
            version: Version::parse(version).unwrap(),
            sha: sha.into(),
            protocol: 1,
        }
    }

    fn kernel_download() -> Download {
        Download {
            platform: Platform::Linux,
            arch: "x86_64".into(),
            component: Component::Kernel,
            format: Format::TarGz,
            url: format!("{HOST}/arbos-kernel-0.2.0-903-linux-x86_64.tar.gz"),
            size: 4,
            sha256: sign::sha256_hex(b"bin!"),
            signature: String::new(),
        }
    }

    fn feed_with(downloads: Vec<Download>, commit: &str) -> Feed {
        let mut feed = Feed::new(Channel::Dev, "now".into());
        feed.releases.push(Release {
            version: "0.2.0".into(),
            build: 903,
            commit: commit.into(),
            published: "2026-09-16T00:00:00Z".into(),
            notes: "a commit".into(),
            notes_url: None,
            minimum_system_version: None,
            downloads,
            links: Vec::new(),
        });
        feed
    }

    #[test]
    fn reads_the_version_line_a_kernel_prints() {
        let got = Running::parse(
            "arbos-kernel 0.2.0 3fff013a2b4c protocol 1",
            Path::new("/usr/local/bin/arbos-kernel"),
        )
        .unwrap();
        assert_eq!(got.version, Version::new(0, 2, 0, 0));
        assert_eq!(got.sha, "3fff013a2b4c");
        assert_eq!(got.protocol, 1);
    }

    #[test]
    fn a_build_too_old_to_name_a_protocol_is_still_read() {
        // The builds most worth replacing are the ones that predate things.
        let got = Running::parse("arbos-kernel 0.1.40 abc123", Path::new("k")).unwrap();
        assert_eq!(got.protocol, 0);
        assert_eq!(got.sha, "abc123");
    }

    #[test]
    fn refuses_to_read_something_that_is_not_a_kernel() {
        for line in ["", "arbos-desktop 0.2.0 abc", "arbos-kernel not-a-version"] {
            assert!(Running::parse(line, Path::new("k")).is_err(), "{line}");
        }
    }

    #[test]
    fn the_commit_decides_when_the_version_has_not_moved() {
        // The whole reason a kernel went stale unnoticed: 0.2.0 for weeks.
        let offered = feed_with(vec![kernel_download()], "a146c41")
            .newest(Platform::Linux, "x86_64", Component::Kernel)
            .unwrap();
        // Same version, same commit — the feed's short sha is a prefix of the
        // kernel's longer one.
        assert_eq!(
            behind(&running("0.2.0", "a146c41f9e02", "/usr/bin/arbos-kernel"), &offered),
            Err(Refusal::Current)
        );
        // Same version, another commit: behind.
        assert!(behind(&running("0.2.0", "181b657aaaaa", "/usr/bin/arbos-kernel"), &offered).is_ok());
    }

    #[test]
    fn an_older_version_is_behind_and_a_newer_one_is_left_alone() {
        let offered = feed_with(vec![kernel_download()], "a146c41")
            .newest(Platform::Linux, "x86_64", Component::Kernel)
            .unwrap();
        assert!(behind(&running("0.1.47", "old12345678", "/usr/bin/k"), &offered).is_ok());
        assert_eq!(
            behind(&running("0.3.0", "new12345678", "/usr/bin/k"), &offered),
            Err(Refusal::Ahead)
        );
    }

    #[test]
    fn a_build_from_no_repository_is_always_behind() {
        // `unknown` says the build cannot account for itself, so it is
        // replaced rather than trusted to be current.
        let offered = feed_with(vec![kernel_download()], "a146c41")
            .newest(Platform::Linux, "x86_64", Component::Kernel)
            .unwrap();
        assert!(behind(&running("0.2.0", "unknown", "/usr/bin/k"), &offered).is_ok());
    }

    #[test]
    fn a_binary_in_a_build_tree_is_somebody_elses_business() {
        assert!(in_build_tree(Path::new("/home/j/arbos/target/release/arbos-kernel")));
        assert!(in_build_tree(Path::new("/home/j/arbos/target/debug/arbos-kernel")));
        assert!(!in_build_tree(Path::new("/usr/local/bin/arbos-kernel")));
        assert!(!in_build_tree(Path::new("/home/j/.cargo/bin/arbos-kernel")));
        // `target` without the profile under it is a folder called target.
        assert!(!in_build_tree(Path::new("/home/j/target/arbos-kernel")));
    }

    #[test]
    fn plan_refuses_a_working_copy_before_it_looks_at_anything_else() {
        let feed = feed_with(vec![kernel_download()], "a146c41");
        let it = running("0.1.0", "old12345678", "/home/j/arbos/target/release/arbos-kernel");
        assert!(matches!(plan(&it, &feed, None), Err(Refusal::BuildTree(_))));
    }

    #[test]
    fn plan_respects_a_pin() {
        let feed = feed_with(vec![kernel_download()], "a146c41");
        let it = running("0.1.0", "old12345678", "/usr/local/bin/arbos-kernel");
        assert_eq!(
            plan(&it, &feed, Some("0.2.0+879")).unwrap_err(),
            Refusal::Pinned("0.2.0+879".into())
        );
    }

    #[test]
    fn plan_says_so_when_the_channel_has_no_kernel_for_this_machine() {
        // A feed from before kernels could update themselves: app payloads
        // only. Every one of those is still readable, and none is offered.
        let mut app = kernel_download();
        app.component = Component::App;
        let feed = feed_with(vec![app], "a146c41");
        let it = running("0.1.0", "old12345678", "/usr/local/bin/arbos-kernel");
        assert_eq!(plan(&it, &feed, None).unwrap_err(), Refusal::NoPayload);
    }

    #[test]
    fn the_app_still_picks_the_app_out_of_a_feed_that_also_carries_kernels() {
        let mut app = kernel_download();
        app.component = Component::App;
        app.url = format!("{HOST}/arbos-0.2.0-903-linux-x86_64.tar.gz");
        let feed = feed_with(vec![app, kernel_download()], "a146c41");
        let picked = feed
            .available(&Version::parse("0.2.0+1").unwrap(), Platform::Linux, "x86_64")
            .unwrap();
        assert_eq!(picked.download.component, Component::App);
        assert!(picked.download.url.contains("/arbos-0.2.0-"));
    }

    #[test]
    fn a_feed_with_no_kernels_is_written_exactly_as_it_was_before() {
        // Every feed published so far. The field has to be invisible, or an
        // app that predates it reads a document it has never seen.
        let mut app = kernel_download();
        app.component = Component::App;
        let text = serde_json::to_string(&feed_with(vec![app], "a146c41")).unwrap();
        assert!(!text.contains("component"), "{text}");
        let read = Feed::parse(&text).unwrap();
        assert_eq!(read.releases[0].downloads[0].component, Component::App);
    }

    /// A stand-in kernel: reports `version`, and claims `errors` problems in
    /// whatever place it is asked about.
    #[cfg(unix)]
    fn stub(at: &Path, version: &str, errors: usize) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let findings = (0..errors)
            .map(|n| format!(r#"{{"level":"error","path":"p{n}","line":null,"what":"x"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        std::fs::write(
            at,
            format!(
                "#!/bin/sh\ncase \"$1\" in\n  check) echo '{{\"place\":\"p\",\"agents\":0,\
                 \"findings\":[{findings}]}}';;\n  *) echo 'arbos-kernel {version} \
                 abc123def456 protocol 1';;\nesac\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(at, std::fs::Permissions::from_mode(0o755)).unwrap();
        at.to_path_buf()
    }

    #[cfg(unix)]
    #[test]
    fn the_place_probe_refuses_a_build_that_reads_the_store_worse() {
        // The failure `execv` cannot report: a binary that runs, and then
        // cannot do the job. This is the only moment it can be caught.
        let home = tempfile::tempdir().unwrap();
        let place = home.path().join("place");
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        let current = stub(&home.path().join("old"), "0.1.40", 0);
        let worse = stub(&home.path().join("new-worse"), "0.2.0", 2);
        let want = Version::parse("0.2.0").unwrap();

        let err = super::Probe::Place(&place)
            .run(&worse, &current, &want)
            .unwrap_err()
            .to_string();
        assert!(err.contains("2 problems"), "{err}");
        assert!(err.contains("refusing"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn the_place_probe_allows_a_build_that_reads_it_no_worse() {
        let home = tempfile::tempdir().unwrap();
        let place = home.path().join("place");
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        let want = Version::parse("0.2.0").unwrap();

        // Same number of problems: not this build's fault, so not its problem.
        let current = stub(&home.path().join("old"), "0.1.40", 3);
        let same = stub(&home.path().join("new-same"), "0.2.0", 3);
        super::Probe::Place(&place).run(&same, &current, &want).unwrap();

        // Fewer: better. A place with pre-existing errors must still be
        // updatable, or the machines that most need a fix can never have one.
        let better = stub(&home.path().join("new-better"), "0.2.0", 1);
        super::Probe::Place(&place)
            .run(&better, &current, &want)
            .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn every_probe_refuses_a_build_that_is_not_what_the_feed_promised() {
        let home = tempfile::tempdir().unwrap();
        let current = stub(&home.path().join("old"), "0.1.40", 0);
        let wrong = stub(&home.path().join("new"), "0.1.41", 0);
        let want = Version::parse("0.2.0").unwrap();
        for probe in [super::Probe::Version, super::Probe::Place(home.path())] {
            let err = probe.run(&wrong, &current, &want).unwrap_err().to_string();
            assert!(err.contains("the feed said"), "{err}");
        }
    }

    #[test]
    fn a_payload_is_named_the_way_ci_publishes_it() {
        assert_eq!(
            payload_name(&Version::parse("0.2.0+903").unwrap(), Platform::Linux, "x86_64"),
            "arbos-kernel-0.2.0-903-linux-x86_64.tar.gz"
        );
    }
}
