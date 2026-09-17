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
    Installing {
        version: String,
        /// The binary's size when the window is copying its own, so the
        /// line says what is crossing the wire ("27 MB"); 0 when unknown.
        bytes: u64,
    },
    Updating { from: String, to: String },
    Building,
    Stopping,
    /// Bringing the processes that were running the replaced binary onto
    /// the new one. Named separately from `Stopping` because it is the
    /// step that can take seconds and can partly refuse, and a window
    /// that says "stopping" through it is describing the wrong thing.
    Restarting {
        running: usize,
    },
    Starting,
    Connecting,
    Ready,
    Failed {
        step: String,
        why: String,
    },
}

impl fmt::Display for Progress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Probing => write!(f, "Checking the machine…"),
            Self::Installing { version, bytes } => match bytes / (1024 * 1024) {
                0 => write!(f, "Installing arbos-kernel {version}…"),
                mb => write!(f, "Installing arbos-kernel {version} ({mb} MB)…"),
            },
            Self::Updating { from, to } => write!(f, "Updating {from} → {to}…"),
            Self::Building => write!(f, "Building arbos-kernel from source…"),
            Self::Stopping => write!(f, "Stopping the old kernel…"),
            Self::Restarting { running } => match running {
                1 => write!(f, "Bringing 1 kernel onto the new build…"),
                n => write!(f, "Bringing {n} kernels onto the new build…"),
            },
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

/// The `arbos-install:` / `arbos-stop:` / `arbos-boot:` lines of a script's
/// output, in order, for the window and the log.
pub fn steps_in(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|l| {
            l.trim()
                .strip_prefix("arbos-install: ")
                .or_else(|| l.trim().strip_prefix("arbos-stop: "))
                .or_else(|| l.trim().strip_prefix("arbos-boot: "))
                .map(str::to_string)
        })
        .collect()
}

/// A path as one shell word, safe inside single quotes.
fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', r#"'\''"#))
}

/// What a process on the remote was doing with the binary being replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub pid: i32,
    /// The inode of the image it is running. `None` where it could not be
    /// read.
    pub inode: Option<u64>,
    /// The place it serves, when its argv says `serve <dir>`.
    pub place: Option<String>,
    /// Its running file had already lost its name before this pass began —
    /// it was stale from an earlier install, not made stale by this one.
    pub was_already_stale: bool,
}

/// How one process ended up after the pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// A supervisor put a replacement on the new build back on the place.
    /// Nothing was relaunched: waiting to see what happens is the only
    /// reliable way to tell a supervised process from an unsupervised one.
    Supervised,
    /// Nothing came back, so the pass started it again from the argv and
    /// working directory it recorded before the stop.
    Relaunched,
    /// It went away and had no place to come back to (a worktree child, or
    /// a process that had finished anyway). Left alone deliberately.
    Ended,
    /// It would not stop inside the graceful window. The place is left as
    /// it is — a second kernel on a held lock is worse than a stale one.
    RefusedStillRunning,
    /// The relaunch itself failed.
    Failed,
}

impl Outcome {
    pub fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "supervised" => Self::Supervised,
            "relaunched" => Self::Relaunched,
            "ended" => Self::Ended,
            "refused-still-running" => Self::RefusedStillRunning,
            "failed" => Self::Failed,
            _ => return None,
        })
    }

    /// Whether this outcome leaves the machine in the state the pass was
    /// run to reach.
    pub fn is_good(self) -> bool {
        matches!(self, Self::Supervised | Self::Relaunched | Self::Ended)
    }
}

/// What one process ended up as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ended {
    pub pid: i32,
    pub place: Option<String>,
    pub outcome: Outcome,
    /// The pid that took its place, when one did.
    pub replaced_by: Option<i32>,
    /// How long the pass waited before it decided, in milliseconds. What
    /// the window's chosen horizon actually cost on this machine.
    pub waited_ms: Option<u64>,
}

/// Everything `bootstrap_script` reported, parsed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BootstrapReport {
    /// The inode at the binary's path before and after the swap. Equal
    /// inodes mean nothing moved.
    pub swapped: Option<(u64, u64)>,
    pub found: Vec<Found>,
    pub ended: Vec<Ended>,
    /// `arbos-boot: error …` lines, which are the script's own refusals.
    pub errors: Vec<String>,
    pub steps: Vec<String>,
}

impl BootstrapReport {
    /// Everything that did not reach a good end.
    pub fn unhappy(&self) -> Vec<&Ended> {
        self.ended.iter().filter(|e| !e.outcome.is_good()).collect()
    }

    /// The longest a single process took to settle, which is the number
    /// worth reporting: the horizon is a bound, not a cost.
    pub fn slowest_ms(&self) -> Option<u64> {
        self.ended.iter().filter_map(|e| e.waited_ms).max()
    }
}

/// Read what [`bootstrap_script`] printed.
pub fn bootstrap_report(output: &str) -> BootstrapReport {
    let mut report = BootstrapReport {
        steps: steps_in(output),
        ..Default::default()
    };
    for line in output.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("arbos-boot: error ") {
            report.errors.push(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("arbos-boot-swap: ") {
            let mut words = rest.split_whitespace();
            if let (Some(a), Some(b)) = (words.next(), words.next())
                && let (Ok(a), Ok(b)) = (a.parse(), b.parse())
            {
                report.swapped = Some((a, b));
            }
        } else if let Some(rest) = line.strip_prefix("arbos-boot-found: ") {
            let words: Vec<&str> = rest.split_whitespace().collect();
            if let Some(pid) = words.first().and_then(|p| p.parse().ok()) {
                report.found.push(Found {
                    pid,
                    inode: words.get(1).and_then(|i| i.parse().ok()),
                    place: words.get(3).filter(|p| **p != "-").map(|p| p.to_string()),
                    was_already_stale: words.get(2) == Some(&"stale"),
                });
            }
        } else if let Some(rest) = line.strip_prefix("arbos-boot-done: ") {
            let words: Vec<&str> = rest.split_whitespace().collect();
            if let (Some(pid), Some(outcome)) = (
                words.first().and_then(|p| p.parse().ok()),
                words.get(2).and_then(|o| Outcome::parse(o)),
            ) {
                report.ended.push(Ended {
                    pid,
                    place: words.get(1).filter(|p| **p != "-").map(|p| p.to_string()),
                    outcome,
                    replaced_by: words.get(3).and_then(|p| p.parse().ok()),
                    waited_ms: words.get(4).and_then(|w| w.parse().ok()),
                });
            }
        }
    }
    report
}

/// The shell that brings a whole installation forward: the binary at `bin`
/// is replaced by the one already sitting at `incoming`, and **every**
/// process that was running the old file is brought onto the new one.
///
/// # Swap before stop
///
/// The order here is the whole point, and it is the opposite of the
/// obvious one. Nothing is stopped until the new binary is already at the
/// path.
///
/// Stopping first leaves a window in which a supervisor does exactly what
/// it exists to do — relaunch immediately, from the path, which still
/// holds the old build. That process is stale from birth, and because it
/// holds the place's lock the supervisor can never replace it: on the
/// 2026-09-17 target a two-second window of that ordering produced a
/// kernel on an unlinked image and 1411 `place already served` errors in
/// 32 minutes. Swapping first turns the same race harmless: a supervisor
/// that beats us relaunches onto the new build and has done our work.
///
/// # Finding the processes
///
/// By the file they run, never by name, and never "the kernel for this
/// place" — an install that restarts the process it connected to leaves
/// the others on a dead image. A process matches when the inode of its
/// running image is the one the path held before the swap, **or** when
/// `/proc/<pid>/exe` names `bin`, `bin.previous` or `bin.arbos-old` with
/// any ` (deleted)` marker stripped. The second form is what catches a
/// process two installs old, whose inode matches nothing on disk but
/// whose magic link still says where it came from.
///
/// The scan runs twice: once before the swap, to record argv and working
/// directory while there is still someone to ask, and once after, to
/// catch anything a supervisor started in between. Anything on the new
/// inode is left alone.
///
/// Confinement is by permission rather than by care: only processes owned
/// by this user are considered, and `readlink` on another user's
/// `/proc/<pid>/exe` is refused by the kernel.
///
/// # Stopping
///
/// `TERM`, which is the graceful stop — every running turn ends the way
/// the stop button ends it — then up to `grace` seconds. A process that
/// will not leave is **refused**, not forced: the place keeps its stale
/// kernel and the report says so, because two kernels on one lock is a
/// worse state than the one we set out to fix.
///
/// Whether a kernel is *busy* cannot be asked of the installations this
/// exists for. A kernel too old to have `update` has no verdict to give
/// and writes no marker a shell can read, so the pass reports busy as
/// unknown rather than inventing an answer. The graceful stop is the
/// protection in the meantime.
///
/// # Waiting rather than classifying
///
/// After the stop the pass waits up to `window` seconds for a replacement
/// on that place whose image is the **new** inode. If one appears, a
/// supervisor is doing the work and the pass does nothing; if none does,
/// it relaunches from the recorded argv and working directory. It never
/// decides in advance whether something is supervised — the parent pid
/// cannot tell a `systemd` unit from a bare `tmux` pane, and the only
/// reliable answer is what actually happens.
///
/// The `new` inode qualifier is load-bearing. A replacement appears
/// within a second in the supervised case whether or not the swap
/// reached it, and only its image says which.
///
/// # Relaunching without wedging the caller
///
/// Anything relaunched gets its standard streams closed and reopened on
/// the place's own log. A relaunched process that keeps the ssh session's
/// stdout holds the connection open for as long as it runs, so the
/// caller sees a finished, correct update as an unending hang — the
/// worst shape a success can take. `setsid` does not do this by itself:
/// it detaches the terminal and leaves the descriptors alone.
pub fn bootstrap_script(bin: &str, incoming: &str, grace_secs: u32, window_secs: u32) -> String {
    BOOTSTRAP
        .replace("@TARGET@", &sq(bin))
        .replace("@INCOMING@", &sq(incoming))
        .replace("@GRACE@", &grace_secs.to_string())
        .replace("@WINDOW@", &window_secs.to_string())
}

/// Placeholders rather than `format!` so the shell's own braces need no
/// escaping and the script reads here as it runs there.
const BOOTSTRAP: &str = r#"
set -u
target=@TARGET@
incoming=@INCOMING@
grace=@GRACE@
window=@WINDOW@

say() { echo "arbos-boot: $*"; }
ino() { stat -L -c %i "$1" 2>/dev/null || stat -L -f %i "$1" 2>/dev/null; }
now_ms() { echo $(($(date +%s%N 2>/dev/null || echo 0)/1000000)); }

case "$(uname -s)" in
  Linux) ;;
  *) say "error this pass reads /proc, and $(uname -s) has none: enumerate that machine by hand"; exit 6 ;;
esac
[ -x "$incoming" ] || { say "error no runnable kernel at $incoming"; exit 2; }
[ -e "$target" ]   || { say "error nothing at $target to replace"; exit 2; }

work=$(mktemp -d "${TMPDIR:-/tmp}/arbos-boot.XXXXXX") || exit 2
trap 'rm -rf "$work"' EXIT
uid=$(id -u)
new=

old=$(ino "$target")
[ -n "$old" ] || { say "error cannot read the inode of $target"; exit 2; }
say "replacing inode $old at $target"

# Every process of this user running the old file, by identity or by the
# exact path its magic link names. Anything on $new is deliberately not a
# match: it is already where we are trying to get to.
scan() {
  for d in /proc/[0-9]*; do
    p=${d#/proc/}
    [ "$(stat -c %u "$d" 2>/dev/null)" = "$uid" ] || continue
    link=$(readlink "$d/exe" 2>/dev/null) || continue
    [ -n "$link" ] || continue
    path=${link% (deleted)}
    i=$(ino "$d/exe")
    hit=no
    [ -n "$i" ] && [ "$i" = "$old" ] && hit=yes
    case "$path" in
      "$target"|"$target".previous|"$target".arbos-old) hit=yes ;;
    esac
    [ -n "$new" ] && [ -n "$i" ] && [ "$i" = "$new" ] && hit=no
    [ "$hit" = yes ] && printf '%s %s %s\n' "$p" "${i:-0}" "$link"
  done
}

# argv, working directory and place, read while there is still someone to
# ask. After the stop none of this can be recovered.
record() {
  p=$1; i=$2; link=$3
  [ -f "$work/$p.argv" ] && return 0
  cat "/proc/$p/cmdline" > "$work/$p.argv" 2>/dev/null || return 0
  [ -s "$work/$p.argv" ] || return 0
  readlink "/proc/$p/cwd" > "$work/$p.cwd" 2>/dev/null || echo / > "$work/$p.cwd"
  tr '\0' '\n' < "$work/$p.argv" | awk 'prev=="serve"{print $0; exit} {prev=$0}' > "$work/$p.place"
  case "$link" in
    *" (deleted)") stale=stale ;;
    *) stale=current ;;
  esac
  echo "$stale" > "$work/$p.state"
  echo "$i" > "$work/$p.ino"
  echo "$p" >> "$work/pids"
  place=$(cat "$work/$p.place")
  echo "arbos-boot-found: $p $i $stale ${place:--}"
}

: > "$work/pids"
scan | while read -r p i link; do record "$p" "$i" "$link"; done
found=$(wc -l < "$work/pids" | tr -d ' ')
say "$found process(es) of this user are running it"

# --- the swap, through the kernel's own mechanism: probed before and
# after, the replaced build kept beside it, rolled back if it will not run.
place_args=""
first_place=$(for p in $(cat "$work/pids"); do cat "$work/$p.place" 2>/dev/null; done | grep . | head -n1)
[ -n "$first_place" ] && place_args="--place $first_place"
say "installing the new binary before stopping anything"
if ! "$incoming" update --binary "$target" --from "$incoming" --install $place_args > "$work/swap.out" 2>&1; then
  say "error the swap failed: $(tail -n 3 "$work/swap.out" | tr '\n' ' ')"
  exit 5
fi
new=$(ino "$target")
echo "arbos-boot-swap: $old $new"
if [ "$new" = "$old" ]; then say "error the file at $target did not change"; exit 5; fi
say "the new build is inode $new; nothing has been stopped yet"

# Anything a supervisor started between the first scan and the swap is
# still on the old file. It is caught here, while it can still be read.
scan | while read -r p i link; do record "$p" "$i" "$link"; done

# The pid now serving $place on the new build, if any. Only the new inode
# counts: a supervisor racing a stop produces a replacement on the old
# build within a second, and nothing but its image tells the two apart.
serving_new() {
  for d in /proc/[0-9]*; do
    q=${d#/proc/}
    [ "$q" = "${1:-}" ] && continue
    [ "$(stat -c %u "$d" 2>/dev/null)" = "$uid" ] || continue
    [ "$(ino "$d/exe")" = "$new" ] || continue
    tr '\0' '\n' < "$d/cmdline" 2>/dev/null | awk -v want="$place" 'prev=="serve" && $0==want {found=1; exit} {prev=$0} END{exit !found}' || continue
    echo "$q"
    return 0
  done
  return 1
}

# Wait for one to appear, bounded by the clock rather than by a count of
# turns round the loop. Each turn walks /proc, which costs tens of
# milliseconds here and more on a busier machine, so counting turns made
# a "10 second" horizon run for about 15 and would stretch further the
# more the machine had to do.
wait_for_new() {
  waited_until=$(( $(now_ms) + window * 1000 ))
  while :; do
    got=$(serving_new "${1:-}") && { echo "$got"; return 0; }
    [ "$(now_ms)" -lt "$waited_until" ] || return 1
    sleep 0.1
  done
}

# --- stop, wait, and relaunch only what did not come back ---
for p in $(cat "$work/pids"); do
  place=$(cat "$work/$p.place" 2>/dev/null)
  started=$(now_ms)
  if ! kill -0 "$p" 2>/dev/null; then
    echo "arbos-boot-done: $p ${place:--} ended - 0"
    continue
  fi
  kill -TERM "$p" 2>/dev/null
  stop_by=$(( $(now_ms) + grace * 1000 ))
  while kill -0 "$p" 2>/dev/null && [ "$(now_ms)" -lt "$stop_by" ]; do sleep 0.1; done
  if kill -0 "$p" 2>/dev/null; then
    say "pid $p did not stop in ${grace}s; leaving ${place:-its place} alone"
    echo "arbos-boot-done: $p ${place:--} refused-still-running - $(( $(now_ms) - started ))"
    continue
  fi
  if [ -z "$place" ]; then
    echo "arbos-boot-done: $p - ended - $(( $(now_ms) - started ))"
    continue
  fi
  # Wait to see what happens rather than deciding what it was.
  took=$(wait_for_new "$p") || took=""
  if [ -n "$took" ]; then
    say "$place came back as pid $took on the new build; nothing to relaunch"
    echo "arbos-boot-done: $p $place supervised $took $(( $(now_ms) - started ))"
    continue
  fi
  # Nothing restarted it, so it was not supervised. Start it from what it
  # was, with the standard streams closed: a relaunched process that keeps
  # this session's stdout holds the caller open for as long as it runs.
  log="$place/.arbos/runtime/kernel.out.log"
  mkdir -p "$place/.arbos/runtime" 2>/dev/null
  arg0=$(tr '\0' '\n' < "$work/$p.argv" | head -n1)
  tail -c +$(( ${#arg0} + 2 )) "$work/$p.argv" > "$work/$p.rest"
  cwd=$(cat "$work/$p.cwd" 2>/dev/null); [ -d "$cwd" ] || cwd=$place
  # setsid without --fork execs in place, so xargs stays as the kernel's
  # parent for as long as it runs: an idle process per relaunch, and a
  # process tree in which an unsupervised kernel appears to have
  # something watching it -- the exact reading that made ppid useless for
  # telling the two apart. --fork lets setsid return, xargs finish, and
  # the kernel reparent to init, which is what "nothing restarts this"
  # should look like from outside.
  if ! command -v setsid >/dev/null 2>&1; then launch="nohup"
  elif setsid --help 2>&1 | grep -q -- --fork; then launch="setsid -f"
  else launch="setsid"; fi
  ( cd "$cwd" 2>/dev/null || cd / ; exec </dev/null >>"$log" 2>&1; xargs -0 -a "$work/$p.rest" $launch "$target" ) &
  back=$(wait_for_new) || back=""
  if [ -n "$back" ]; then
    say "$place had no supervisor; started again as pid $back"
    echo "arbos-boot-done: $p $place relaunched $back $(( $(now_ms) - started ))"
  else
    say "error $place had no supervisor and would not start again (see $log)"
    echo "arbos-boot-done: $p $place failed - $(( $(now_ms) - started ))"
  fi
done

say "done"
"#;

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
                version: "0.2.1".into(),
                bytes: 0,
            }
            .to_string(),
            "Installing arbos-kernel 0.2.1…"
        );
        assert_eq!(
            Progress::Installing {
                version: "0.2.1".into(),
                bytes: 27 * 1024 * 1024 + 1,
            }
            .to_string(),
            "Installing arbos-kernel 0.2.1 (27 MB)…"
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

    // --- the bootstrap pass ---------------------------------------------
    //
    // The script itself is proved on a live machine; what is worth pinning
    // here is the part a reader would silently get wrong later. Each of
    // these is a property that cost something to learn.

    #[test]
    fn the_swap_comes_before_anything_is_stopped() {
        let s = bootstrap_script("/home/u/.local/bin/arbos-kernel", "/tmp/incoming", 20, 10);
        let swap = s.find("update --binary").expect("the swap");
        let stop = s.find("kill -TERM").expect("the stop");
        assert!(
            swap < stop,
            "stopping before the swap lets a supervisor relaunch onto the old build, \
             which is how a place gets pinned by a stale kernel"
        );
    }

    #[test]
    fn the_swap_goes_through_the_kernels_own_mechanism_not_a_bare_move() {
        let s = bootstrap_script("/home/u/.local/bin/arbos-kernel", "/tmp/incoming", 20, 10);
        assert!(
            s.contains("update --binary") && s.contains("--from") && s.contains("--install"),
            "install_file is what probes the candidate and keeps .previous: {s}"
        );
    }

    #[test]
    fn processes_are_matched_by_identity_or_exact_path_never_by_name() {
        let s = bootstrap_script("/home/u/.local/bin/arbos-kernel", "/tmp/incoming", 20, 10);
        // The inode form, for a file still findable on disk.
        assert!(s.contains(r#"[ "$i" = "$old" ] && hit=yes"#), "{s}");
        // The path forms, which are the only thing that finds a process
        // whose image was unlinked two installs ago.
        assert!(s.contains(r#"path=${link% (deleted)}"#), "{s}");
        for suffix in [
            "\"$target\"",
            "\"$target\".previous",
            "\"$target\".arbos-old",
        ] {
            assert!(s.contains(suffix), "missing {suffix} in the match: {s}");
        }
        // Never by name: matching `arbos-kernel` anywhere would reach
        // processes this pass has no business touching.
        assert!(
            !s.contains("pgrep") && !s.contains("pkill") && !s.contains("killall"),
            "{s}"
        );
    }

    #[test]
    fn a_process_already_on_the_new_build_is_not_a_match() {
        let s = bootstrap_script("/home/u/.local/bin/arbos-kernel", "/tmp/incoming", 20, 10);
        assert!(
            s.contains(r#"[ -n "$new" ] && [ -n "$i" ] && [ "$i" = "$new" ] && hit=no"#),
            "the second scan must not pick up what the first swap already fixed: {s}"
        );
    }

    #[test]
    fn a_replacement_only_counts_when_it_runs_the_new_inode() {
        let s = bootstrap_script("/home/u/.local/bin/arbos-kernel", "/tmp/incoming", 20, 10);
        // The qualifier that separates a supervisor doing our work from a
        // supervisor racing us onto the build we are replacing.
        assert!(
            s.contains(r#"[ "$(ino "$d/exe")" = "$new" ] || continue"#),
            "a replacement pid alone proves nothing: {s}"
        );
    }

    #[test]
    fn a_relaunched_process_does_not_keep_the_callers_streams() {
        let s = bootstrap_script("/home/u/.local/bin/arbos-kernel", "/tmp/incoming", 20, 10);
        assert!(
            s.contains(r#"exec </dev/null >>"$log" 2>&1"#),
            "a relaunched process holding the session's stdout turns a successful \
             update into an unending hang: {s}"
        );
    }

    /// Measured on the target: one walk of `/proc` costs 55 ms among 58
    /// processes, so a loop that counted turns ran a "10 second" horizon
    /// for 14.5 s — and would stretch further the busier the machine.
    /// The bound is a time, so it is kept against the clock.
    #[test]
    fn the_horizon_is_measured_against_the_clock_not_a_count_of_turns() {
        let s = bootstrap_script("/home/u/.local/bin/arbos-kernel", "/tmp/incoming", 20, 10);
        assert!(
            s.contains("waited_until=$(( $(now_ms) + window * 1000 ))")
                && s.contains("stop_by=$(( $(now_ms) + grace * 1000 ))"),
            "{s}"
        );
        assert!(
            !s.contains("window * 10)") && !s.contains("grace * 10)"),
            "counting turns makes the horizon mean different things on different machines: {s}"
        );
    }

    /// Seen on the target: plain `setsid` execs in place, so the `xargs`
    /// that ran it stayed as the relaunched kernel's parent for the
    /// kernel's whole life. Harmless in itself, but it gives an
    /// unsupervised kernel a parent that looks like a supervisor, which
    /// is the reading this whole pass refuses to rely on.
    #[test]
    fn a_relaunched_kernel_is_left_with_no_parent_pretending_to_watch_it() {
        let s = bootstrap_script("/home/u/.local/bin/arbos-kernel", "/tmp/incoming", 20, 10);
        assert!(s.contains(r#"launch="setsid -f""#), "{s}");
        assert!(s.contains("--fork"), "{s}");
    }

    #[test]
    fn a_process_that_will_not_stop_is_refused_rather_than_forced() {
        let s = bootstrap_script("/home/u/.local/bin/arbos-kernel", "/tmp/incoming", 20, 10);
        assert!(s.contains("refused-still-running"), "{s}");
        assert!(
            !s.contains("kill -9") && !s.contains("kill -KILL"),
            "two kernels on one lock is worse than one stale kernel: {s}"
        );
    }

    #[test]
    fn paths_with_a_quote_in_them_stay_one_shell_word() {
        let s = bootstrap_script("/home/o'brien/bin/arbos-kernel", "/tmp/in", 20, 10);
        assert!(
            s.contains(r#"target='/home/o'\''brien/bin/arbos-kernel'"#),
            "{s}"
        );
    }

    /// A generated script has no compiler behind it, so this is the only
    /// thing standing between a typo and a machine being asked to run it.
    #[test]
    fn the_script_is_valid_shell() {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let s = bootstrap_script("/home/u/.local/bin/arbos-kernel", "/tmp/incoming", 20, 10);
        let mut sh = match Command::new("sh")
            .arg("-n")
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(sh) => sh,
            // No `sh` (an odd build host): nothing to prove here.
            Err(_) => return,
        };
        sh.stdin.take().unwrap().write_all(s.as_bytes()).unwrap();
        let out = sh.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "the bootstrap script is not valid shell:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn the_report_reads_what_the_script_printed() {
        let out = "\
arbos-boot: replacing inode 27292057 at /home/u/.local/bin/arbos-kernel
arbos-boot-found: 1257833 27292057 current /home/u/places/alpha
arbos-boot-found: 1265519 27292057 stale /home/u/places/gamma
arbos-boot-found: 9001 27292057 current -
arbos-boot-swap: 27292057 27292228
arbos-boot-done: 1257833 /home/u/places/alpha relaunched 1300001 2140
arbos-boot-done: 1265519 /home/u/places/gamma supervised 1300009 1310
arbos-boot-done: 9001 - ended - 0
arbos-boot: done";
        let r = bootstrap_report(out);
        assert_eq!(r.swapped, Some((27292057, 27292228)));
        assert_eq!(r.found.len(), 3);
        assert_eq!(r.found[1].place.as_deref(), Some("/home/u/places/gamma"));
        assert!(r.found[1].was_already_stale);
        assert!(!r.found[0].was_already_stale);
        assert_eq!(r.found[2].place, None);
        assert_eq!(r.ended[0].outcome, Outcome::Relaunched);
        assert_eq!(r.ended[0].replaced_by, Some(1300001));
        assert_eq!(r.ended[1].outcome, Outcome::Supervised);
        assert_eq!(r.ended[2].outcome, Outcome::Ended);
        assert!(r.unhappy().is_empty());
        // The horizon is a bound; this is what it cost.
        assert_eq!(r.slowest_ms(), Some(2140));
        assert!(r.errors.is_empty());
    }

    #[test]
    fn a_refusal_reads_as_a_refusal_and_not_as_a_failure() {
        let r = bootstrap_report(
            "arbos-boot: pid 42 did not stop in 20s; leaving /p alone\n\
             arbos-boot-done: 42 /p refused-still-running - 20030",
        );
        let unhappy = r.unhappy();
        assert_eq!(unhappy.len(), 1);
        assert_eq!(unhappy[0].outcome, Outcome::RefusedStillRunning);
        assert!(!unhappy[0].outcome.is_good());
        // A refusal is not an error line: the pass did what it meant to.
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(r.steps.len(), 1);
    }

    /// The bytes a real machine produced, rather than bytes written to
    /// match the parser. Captured from the disposable target on
    /// 2026-09-17, where one kernel had been suspended so that it could
    /// not answer TERM and the other two had no supervisor.
    #[test]
    fn the_report_reads_what_a_real_machine_printed() {
        let out = "\
arbos-boot: replacing inode 27292243 at /home/arbostest/.local/bin/arbos-kernel
arbos-boot-found: 1348327 27292243 current /home/arbostest/places/alpha
arbos-boot-found: 1348330 27292243 current /home/arbostest/places/beta
arbos-boot-found: 1348349 27292243 current /home/arbostest/places/gamma
arbos-boot: 3 process(es) of this user are running it
arbos-boot: installing the new binary before stopping anything
arbos-boot-swap: 27292243 27292497
arbos-boot: the new build is inode 27292497; nothing has been stopped yet
arbos-boot: pid 1348327 did not stop in 20s; leaving /home/arbostest/places/alpha alone
arbos-boot-done: 1348327 /home/arbostest/places/alpha refused-still-running - 20008
arbos-boot: /home/arbostest/places/beta had no supervisor; started again as pid 1355354
arbos-boot-done: 1348330 /home/arbostest/places/beta relaunched 1355354 10229
arbos-boot: /home/arbostest/places/gamma had no supervisor; started again as pid 1361455
arbos-boot-done: 1348349 /home/arbostest/places/gamma relaunched 1361455 10249
arbos-boot: done";
        let r = bootstrap_report(out);
        assert_eq!(r.swapped, Some((27292243, 27292497)));
        assert_eq!(r.found.len(), 3);
        assert_eq!(r.ended.len(), 3);
        // One refusal, and it is not an error: the pass decided.
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let unhappy = r.unhappy();
        assert_eq!(unhappy.len(), 1);
        assert_eq!(unhappy[0].outcome, Outcome::RefusedStillRunning);
        assert_eq!(
            unhappy[0].place.as_deref(),
            Some("/home/arbostest/places/alpha")
        );
        // Nothing took the refused place: a second kernel on a held lock
        // is the state this refuses into existence rather than out of.
        assert_eq!(unhappy[0].replaced_by, None);
        // The grace window is a bound and it was reached exactly; the
        // watch window is a bound and the two relaunches sat on it.
        assert_eq!(r.slowest_ms(), Some(20008));
        for e in &r.ended {
            if e.outcome == Outcome::Relaunched {
                let ms = e.waited_ms.expect("a measured wait");
                assert!(
                    (10_000..11_000).contains(&ms),
                    "a 10 s horizon should cost about 10 s, not {ms} ms"
                );
            }
        }
    }

    #[test]
    fn the_scripts_own_refusals_come_back_as_errors() {
        let r = bootstrap_report("arbos-boot: error nothing at /bin/k to replace");
        assert_eq!(r.errors, vec!["nothing at /bin/k to replace"]);
        assert!(r.swapped.is_none());
    }
}
