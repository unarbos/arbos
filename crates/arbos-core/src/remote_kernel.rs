//! Putting `arbos-kernel` on another machine and keeping it current: the
//! version line both sides speak, the release asset for a machine type,
//! the shell that installs or updates (a release binary first, a source
//! build when the machine allows it), the shell that stops a running
//! kernel — only that process; its jobs die with it through their leash —
//! and the steps a window shows while it happens ("Installing…",
//! "Updating 0.2.0 → 0.2.1…").
//!
//! The desktop runs these over `ssh` when it opens a remote place; the
//! kernel's `spawn host=` road uses the same version rule. Everything
//! here is text and comparisons, so it is tested without a network.

use std::cmp::Ordering;
use std::fmt;

/// The repository releases are cut from.
pub const RELEASE_REPO: &str = "unarbos/arbos";

/// `arbos-kernel 0.2.0 <sha> protocol 1`, as `arbos-kernel --version`
/// prints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelVersion {
    pub semver: (u64, u64, u64),
    pub sha: String,
    pub protocol: u32,
}

impl KernelVersion {
    pub fn parse(line: &str) -> Option<Self> {
        let mut words = line.split_whitespace();
        if words.next()? != "arbos-kernel" {
            return None;
        }
        let semver = parse_semver(words.next()?)?;
        let rest: Vec<&str> = words.collect();
        let protocol = rest
            .iter()
            .position(|w| *w == "protocol")
            .and_then(|i| rest.get(i + 1))
            .and_then(|p| p.parse().ok())
            .unwrap_or(0);
        let sha = rest
            .iter()
            .find(|w| **w != "protocol" && w.chars().all(|c| c.is_ascii_hexdigit()) && w.len() >= 7)
            .map(|s| s.to_string())
            .unwrap_or_default();
        Some(Self {
            semver,
            sha,
            protocol,
        })
    }

    /// `0.2.1`
    pub fn short(&self) -> String {
        format!("{}.{}.{}", self.semver.0, self.semver.1, self.semver.2)
    }

    /// Whether `self` (the remote's) must be replaced by `wanted` (the
    /// desktop's): an older semver, or the same semver from a different
    /// build. A newer remote is left alone.
    pub fn needs_update_to(&self, wanted: &Self) -> bool {
        match self.semver.cmp(&wanted.semver) {
            Ordering::Less => true,
            Ordering::Greater => false,
            Ordering::Equal => {
                !self.sha.is_empty() && !wanted.sha.is_empty() && self.sha != wanted.sha
            }
        }
    }
}

fn parse_semver(s: &str) -> Option<(u64, u64, u64)> {
    let core = s.trim_start_matches('v').split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

/// How a release ships the kernel for one machine type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asset {
    /// A tarball holding `<dir>/arbos-kernel`.
    Tarball { url: String, inner: String },
    /// A bare executable.
    Binary { url: String },
}

impl Asset {
    pub fn url(&self) -> &str {
        match self {
            Self::Tarball { url, .. } | Self::Binary { url } => url,
        }
    }
}

/// The release asset for `os_arch` (`linux-amd64`, `darwin-arm64`, as the
/// probe spells it) at `version` (`0.2.1`), or None when no release is
/// cut for that machine type — the source build is the way then. Names
/// follow `.github/workflows/release.yml`: Linux as a tarball named with
/// the tag, macOS as a bare binary named without the `v`.
pub fn release_asset(os_arch: &str, version: &str) -> Option<Asset> {
    let base = format!("https://github.com/{RELEASE_REPO}/releases/download/v{version}");
    match os_arch {
        "linux-amd64" | "linux-x86_64" => {
            let dir = format!("arbos-kernel-v{version}-linux-x86_64");
            Some(Asset::Tarball {
                url: format!("{base}/{dir}.tar.gz"),
                inner: format!("{dir}/arbos-kernel"),
            })
        }
        "darwin-arm64" | "macos-arm64" => Some(Asset::Binary {
            url: format!("{base}/arbos-kernel-{version}-macos-arm64"),
        }),
        _ => None,
    }
}

/// The steps of putting a kernel on a machine, for the window's line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    Probing,
    Installing { version: String },
    Updating { from: String, to: String },
    Building,
    Stopping,
    Starting,
    Connecting,
    Ready,
    Failed { step: String, why: String },
}

impl fmt::Display for Progress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Probing => write!(f, "Checking the machine…"),
            Self::Installing { version } => write!(f, "Installing arbos-kernel {version}…"),
            Self::Updating { from, to } => write!(f, "Updating {from} → {to}…"),
            Self::Building => write!(f, "Building arbos-kernel from source…"),
            Self::Stopping => write!(f, "Stopping the old kernel…"),
            Self::Starting => write!(f, "Starting the kernel…"),
            Self::Connecting => write!(f, "Connecting…"),
            Self::Ready => write!(f, "Connected"),
            Self::Failed { step, why } => write!(f, "{step} failed: {why}"),
        }
    }
}

/// The shell that puts `arbos-kernel` at `bin` on a machine of type
/// `os_arch`, as `version` (with `git_sha` for the source build). Tries
/// the release asset first — downloaded next to the target, checked
/// against the release's `.sha256` when the release has one, unpacked,
/// moved into place as `<bin>.new` then `mv -f` (a kernel that is being
/// executed must never be overwritten in place). Falls back to a source
/// build only when `allow_build` (the machine's `build = true`), through
/// `cargo install --git … --rev <sha>`. Prints one `arbos-install: <step>`
/// line per step for the window; exits 0 with `<bin>` executable, 3 when
/// nothing could be done, saying what to do.
pub fn install_script(
    bin: &str,
    version: &str,
    git_sha: &str,
    os_arch: &str,
    allow_build: bool,
) -> String {
    let bin_dir = parent_of(bin);
    let mut s = String::new();
    s.push_str("set -u\n");
    s.push_str(&format!(
        "umask 077; mkdir -p \"{bin_dir}\" \"$HOME/.cache/arbos\" || exit 2\n"
    ));
    s.push_str(&format!(
        "tmp=\"$HOME/.cache/arbos/kernel-{version}.download\"\n"
    ));
    s.push_str("rm -rf \"$tmp\"; mkdir -p \"$tmp\"\n");
    s.push_str(&format!("target=\"{bin}\"\n"));
    match release_asset(os_arch, version) {
        Some(asset) => {
            s.push_str(&format!(
                "echo 'arbos-install: downloading {}'\n",
                asset.url()
            ));
            s.push_str(&format!(
                "if command -v curl >/dev/null 2>&1; then curl -fsSL --retry 2 -o \"$tmp/asset\" \"{url}\" && curl -fsSL -o \"$tmp/asset.sha256\" \"{url}.sha256\" 2>/dev/null; \
elif command -v wget >/dev/null 2>&1; then wget -q -O \"$tmp/asset\" \"{url}\" && wget -q -O \"$tmp/asset.sha256\" \"{url}.sha256\" 2>/dev/null; fi\n",
                url = asset.url()
            ));
            // The checksum, when the release published one.
            s.push_str(
                "if [ -s \"$tmp/asset\" ] && [ -s \"$tmp/asset.sha256\" ]; then \
want=$(cut -d' ' -f1 \"$tmp/asset.sha256\"); \
have=$( (sha256sum \"$tmp/asset\" 2>/dev/null || shasum -a 256 \"$tmp/asset\") | cut -d' ' -f1); \
if [ \"$want\" != \"$have\" ]; then echo 'arbos-install: checksum mismatch; not installing'; rm -f \"$tmp/asset\"; fi; fi\n",
            );
            match asset {
                Asset::Tarball { inner, .. } => s.push_str(&format!(
                    "if [ -s \"$tmp/asset\" ]; then tar -xzf \"$tmp/asset\" -C \"$tmp\" && [ -f \"$tmp/{inner}\" ] && mv -f \"$tmp/{inner}\" \"$target.new\"; fi\n"
                )),
                Asset::Binary { .. } => s.push_str(
                    "if [ -s \"$tmp/asset\" ]; then mv -f \"$tmp/asset\" \"$target.new\"; fi\n",
                ),
            }
            s.push_str(
                "if [ -f \"$target.new\" ]; then chmod +x \"$target.new\" && \"$target.new\" --version >/dev/null 2>&1 && mv -f \"$target.new\" \"$target\" && echo 'arbos-install: installed from release' && rm -rf \"$tmp\" && exit 0; rm -f \"$target.new\"; echo 'arbos-install: the release asset did not run here'; fi\n",
            );
        }
        None => {
            s.push_str(&format!(
                "echo 'arbos-install: no release is cut for {os_arch}'\n"
            ));
        }
    }
    if allow_build {
        s.push_str("echo 'arbos-install: building from source'\n");
        s.push_str(
            "if ! command -v cargo >/dev/null 2>&1; then [ -x \"$HOME/.cargo/bin/cargo\" ] || curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain nightly >/dev/null 2>&1; . \"$HOME/.cargo/env\" 2>/dev/null; fi\n",
        );
        s.push_str(&format!(
            "if command -v cargo >/dev/null 2>&1 && cargo install --quiet --locked --git https://github.com/{RELEASE_REPO} --rev {git_sha} arbos-kernel --root \"$tmp/root\" 2>\"$tmp/build.log\"; then mv -f \"$tmp/root/bin/arbos-kernel\" \"$target.new\" && chmod +x \"$target.new\" && mv -f \"$target.new\" \"$target\" && echo 'arbos-install: installed from source' && rm -rf \"$tmp\" && exit 0; fi\n"
        ));
        s.push_str("echo \"arbos-install: source build failed: $(tail -n 3 \"$tmp/build.log\" 2>/dev/null | tr '\\n' ' ')\"\n");
    } else {
        s.push_str(&format!(
            "echo 'arbos-install: source build not allowed here (set build = true for this machine in ~/.config/arbos/machines.toml, or put an arbos-kernel built for {os_arch} at {bin})'\n"
        ));
    }
    s.push_str("exit 3\n");
    s
}

/// The shell that stops the kernel serving `place_dir` on a machine, so a
/// newer binary can take its place: only that process gets TERM — its
/// jobs hold a leash to it and go with it, nothing else on the machine is
/// touched — then up to 20 s for it to leave. Prints `arbos-stop: stopped`,
/// `arbos-stop: not running`, or `arbos-stop: still running <pid>` (exit 4).
pub fn stop_script(place_dir: &str) -> String {
    format!(
        r#"f="{dir}/.arbos/runtime/kernel.json"; [ -f "$f" ] || f="{dir}/.arbos/kernel.json"
if [ ! -f "$f" ]; then echo 'arbos-stop: not running'; exit 0; fi
pid=$(tr -d '\n' < "$f" | sed -n 's/.*"pid":[[:space:]]*\([0-9][0-9]*\).*/\1/p' | head -n1)
if [ -z "$pid" ] || ! kill -0 "$pid" 2>/dev/null; then echo 'arbos-stop: not running'; exit 0; fi
kill -TERM "$pid" 2>/dev/null
i=0; while kill -0 "$pid" 2>/dev/null && [ $i -lt 200 ]; do sleep 0.1; i=$((i+1)); done
if kill -0 "$pid" 2>/dev/null; then echo "arbos-stop: still running $pid"; exit 4; fi
echo 'arbos-stop: stopped'"#,
        dir = place_dir
    )
}

/// The `arbos-install:` / `arbos-stop:` lines of a script's output, in
/// order, for the window and the log.
pub fn steps_in(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|l| {
            l.trim()
                .strip_prefix("arbos-install: ")
                .or_else(|| l.trim().strip_prefix("arbos-stop: "))
                .map(str::to_string)
        })
        .collect()
}

fn parent_of(path: &str) -> String {
    match path.rfind('/') {
        Some(0) => "/".to_string(),
        Some(ix) => path[..ix].to_string(),
        None => ".".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_lines_parse_and_compare() {
        let a = KernelVersion::parse("arbos-kernel 0.2.0 3fff013 protocol 1").unwrap();
        let b = KernelVersion::parse("arbos-kernel 0.2.1 c6e7698 protocol 1").unwrap();
        assert_eq!(a.short(), "0.2.0");
        assert_eq!(a.sha, "3fff013");
        assert_eq!(b.protocol, 1);
        assert!(a.needs_update_to(&b), "older semver updates");
        assert!(!b.needs_update_to(&a), "a newer remote is left alone");
        let a2 = KernelVersion::parse("arbos-kernel 0.2.0 deadbee protocol 1").unwrap();
        assert!(a.needs_update_to(&a2), "same semver, another build: update");
        assert!(!a.needs_update_to(&a.clone()));
        let unknown = KernelVersion::parse("arbos-kernel 0.2.0 unknown protocol 1").unwrap();
        assert_eq!(
            unknown.sha, "",
            "a build with no sha compares by semver only"
        );
        assert!(!unknown.needs_update_to(&a));
        assert!(KernelVersion::parse("bash: arbos-kernel: not found").is_none());
        assert!(KernelVersion::parse("arbos-kernel 0.2").is_none());
    }

    #[test]
    fn release_assets_follow_the_workflow_names() {
        match release_asset("linux-amd64", "0.2.1").unwrap() {
            Asset::Tarball { url, inner } => {
                assert_eq!(
                    url,
                    "https://github.com/unarbos/arbos/releases/download/v0.2.1/arbos-kernel-v0.2.1-linux-x86_64.tar.gz"
                );
                assert_eq!(inner, "arbos-kernel-v0.2.1-linux-x86_64/arbos-kernel");
            }
            other => panic!("{other:?}"),
        }
        match release_asset("darwin-arm64", "0.2.1").unwrap() {
            Asset::Binary { url } => {
                assert!(url.ends_with("/v0.2.1/arbos-kernel-0.2.1-macos-arm64"))
            }
            other => panic!("{other:?}"),
        }
        assert!(release_asset("linux-arm64", "0.2.1").is_none());
        assert!(release_asset("freebsd-amd64", "0.2.1").is_none());
    }

    #[test]
    fn the_install_script_moves_into_place_never_over_a_running_binary() {
        let s = install_script(
            "$HOME/.cargo/bin/arbos-kernel",
            "0.2.1",
            "c6e7698",
            "linux-amd64",
            false,
        );
        assert!(s.contains("arbos-kernel-v0.2.1-linux-x86_64.tar.gz"), "{s}");
        assert!(
            s.contains("\"$target.new\" --version"),
            "the new binary is run before it replaces the old"
        );
        assert!(s.contains("mv -f \"$target.new\" \"$target\""), "{s}");
        assert!(
            !s.contains("cargo install"),
            "no source build unless allowed"
        );
        assert!(s.contains("source build not allowed here"), "{s}");
        let b = install_script(
            "/opt/arbos/bin/arbos-kernel",
            "0.2.1",
            "c6e7698",
            "linux-arm64",
            true,
        );
        assert!(b.contains("no release is cut for linux-arm64"), "{b}");
        assert!(b.contains("cargo install --quiet --locked --git https://github.com/unarbos/arbos --rev c6e7698 arbos-kernel"), "{b}");
        assert!(b.contains("mkdir -p \"/opt/arbos/bin\""), "{b}");
    }

    #[test]
    fn the_stop_script_signals_one_pid_and_the_steps_are_read_back() {
        let s = stop_script("/home/u/proj");
        assert!(s.contains("kill -TERM \"$pid\""), "{s}");
        assert!(
            !s.contains("killall") && !s.contains("pkill") && !s.contains("-- -"),
            "one pid, no group, no sweep: {s}"
        );
        assert!(s.contains("/home/u/proj/.arbos/runtime/kernel.json"));
        assert_eq!(
            steps_in(
                "junk\narbos-install: downloading x\n  arbos-install: installed from release\narbos-stop: stopped"
            ),
            vec!["downloading x", "installed from release", "stopped"]
        );
    }

    #[test]
    fn progress_reads_as_the_windows_line() {
        assert_eq!(
            Progress::Installing {
                version: "0.2.1".into()
            }
            .to_string(),
            "Installing arbos-kernel 0.2.1…"
        );
        assert_eq!(
            Progress::Updating {
                from: "0.2.0".into(),
                to: "0.2.1".into()
            }
            .to_string(),
            "Updating 0.2.0 → 0.2.1…"
        );
        assert_eq!(Progress::Ready.to_string(), "Connected");
    }
}
