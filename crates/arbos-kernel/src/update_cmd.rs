//! `arbos-kernel update` — bring this binary up to what its channel publishes.
//!
//! The whole of the update with a person choosing the moment, which is why it
//! exists before anything runs unattended: the mechanism can be watched on a
//! real machine before a kernel is trusted to do it between turns.
//!
//! It is also the answer for the machine that prompted the feature. A kernel
//! nobody ssh's into and nobody attaches to is reached by neither of the two
//! paths that update a kernel today, so it goes stale and stays stale. This is
//! one command on that box.
//!
//! Everything it does is `arbos-update`'s: the same channel feed the desktop
//! reads, the same Ed25519 check over the payload's bytes, and the same two
//! renames, so there is no moment at which the binary is half replaced and a
//! failure puts the old one back.
//!
//! ## What it does not do
//!
//! Restart anything. The process serving a place keeps the code it started
//! with until it restarts, and this says so rather than pretending otherwise.
//! Choosing that moment is slice 4's — `idle::update_verdict` is the gate —
//! and doing it by hand is the point of this command.

use anyhow::{Context, Result};
use arbos_update::{
    Channel,
    kernel::{self, Refusal},
    net, sign,
};
use std::path::PathBuf;

pub const USAGE: &str = "\
arbos-kernel update [--install] [--channel stable|dev] [--binary PATH] [--pin X.Y.Z+N]
                    [--place DIR ...]
    What the channel has, and whether this binary is behind it. --install
    replaces it; without that it only reports. Replacing the binary does not
    update a running kernel: it keeps the code it started with until it
    restarts, and --place says which ones are still serving what.
    --binary PATH checks and replaces that file instead of this one. That is
    how a kernel too old to have `update` (it answers `unknown command
    update`) is brought forward: run this from any newer arbos-kernel against
    the old file; afterwards it updates itself.";

pub struct Args {
    pub install: bool,
    pub channel: Option<Channel>,
    pub binary: Option<PathBuf>,
    pub pin: Option<String>,
    /// Places whose running kernel to report on. Defaults to the working
    /// directory when it is one.
    pub places: Vec<PathBuf>,
}

impl Args {
    pub fn parse(argv: impl Iterator<Item = String>) -> Result<Self> {
        let mut args = Self {
            install: false,
            channel: None,
            binary: None,
            pin: None,
            places: Vec::new(),
        };
        let mut rest = argv.peekable();
        while let Some(arg) = rest.next() {
            match arg.as_str() {
                "--install" => args.install = true,
                "--channel" => {
                    let name = rest.next().context("--channel wants stable or dev")?;
                    args.channel =
                        Some(Channel::parse(&name).context("--channel is stable or dev")?);
                }
                "--binary" => {
                    args.binary =
                        Some(PathBuf::from(rest.next().context("--binary wants a path")?));
                }
                "--pin" => args.pin = Some(rest.next().context("--pin wants a version")?),
                "--place" => args.places.push(PathBuf::from(
                    rest.next().context("--place wants a directory")?,
                )),
                "-h" | "--help" | "help" => {
                    println!("{USAGE}");
                    std::process::exit(0);
                }
                other => anyhow::bail!("update: no such option `{other}`\n\n{USAGE}"),
            }
        }
        Ok(args)
    }
}

/// Which builds this machine follows when nothing says otherwise.
///
/// `dev` for a machine registered with a hub, `stable` for one that is not.
/// A machine on the hub is part of Jacob's mesh and is meant to track `main`
/// — that is what the mesh is for. A kernel nobody put on a hub is more
/// likely somebody else's, and gets the quieter channel.
fn default_channel() -> Channel {
    match arbos_core::hub::HubConfig::load() {
        Ok(Some(_)) => Channel::Dev,
        _ => Channel::Stable,
    }
}

pub fn run(args: Args) -> Result<i32> {
    let channel = args.channel.unwrap_or_else(default_channel);
    // Its own binary unless told otherwise: a kernel updating itself is the
    // point, and `current_exe` is the one answer that is always right about
    // which copy is being run.
    let binary = match &args.binary {
        Some(named) => kernel::find(Some(named))?,
        None => std::env::current_exe().context("this program cannot find itself on disk")?,
    };
    let running = kernel::Running::read(&binary)?;
    println!(
        "running   {} {} ({})",
        running.version.human(),
        running.sha,
        binary.display()
    );
    println!("channel   {}", channel.as_str());

    let feed = net::feed(channel)?;
    let offered = match kernel::plan(&running, &feed, args.pin.as_deref()) {
        Ok(offered) => offered,
        Err(refusal) => {
            println!("no update: {}", refusal.say());
            // "Up to date" is only true if what is *serving* is up to date.
            if report_serving(&args, &running) {
                println!(
                    "\nThe binary is current and a running kernel is not. Restart it, or it \n\
                     goes on serving the older code."
                );
            }
            // A working copy or a pin is a decision, not a failure; being
            // current is the happy answer. Only "this channel has nothing for
            // this machine" is worth a non-zero exit, because a script asking
            // for an update it cannot have should notice.
            return Ok(match refusal {
                Refusal::NoPayload => 1,
                _ => 0,
            });
        }
    };
    println!(
        "available {} ({}) published {}",
        offered.version.human(),
        offered.commit,
        offered.published
    );
    if !offered.notes.trim().is_empty() {
        println!("          {}", offered.notes.trim());
    }
    println!("          {}", offered.download.url);

    if !args.install {
        report_serving(&args, &running);
        println!("\nrun again with --install to replace it");
        return Ok(0);
    }

    let key = sign::built_in_key()
        .context("this build carries no update key, so it cannot check a payload")?;
    println!("\nfetching  {} bytes", offered.download.size);
    let bytes = net::bytes(&offered.download.url)?;
    let scratch = std::env::temp_dir().join("arbos-kernel-update");
    kernel::verify_and_install(&bytes, &offered, &binary, &key, &scratch)?;

    let now = kernel::Running::read(&binary)?;
    println!("installed {} {}", now.version.human(), now.sha);
    report_serving(&args, &now);
    println!();
    println!("{}", restart_note(&binary));
    Ok(0)
}

/// What is actually serving a place, as that process reported itself.
///
/// **Not the file on disk.** A kernel writes its own version and commit into
/// `kernel.json` when it starts, so that file is the only honest answer to
/// "what code is running here". Reading the binary instead would report a
/// machine as current the moment its file was replaced, while the process
/// went on serving the old image — which is exactly how `subnet120` stayed
/// stale with nobody noticing, its `/proc/<pid>/exe` reading `(deleted)`.
#[derive(Debug)]
struct Serving {
    place: PathBuf,
    pid: i32,
    version: String,
    sha: String,
    /// The file the serving process was started from is gone (Linux:
    /// `/proc/<pid>/exe` reads `… (deleted)`): it runs an image no file
    /// holds any more. None where the machine cannot say (no /proc).
    binary_gone: Option<bool>,
}

/// Whether `pid` runs an image that is not the file at `on_disk`, where
/// /proc can say: its exe unlinked (`(deleted)`), or a different file
/// from the one at the binary's path — a directory renamed to a backup
/// moves the running inode to a real, undeleted path, so existence alone
/// would call it fine.
fn pid_binary_gone(pid: i32, on_disk: &std::path::Path) -> Option<bool> {
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    if !exe.exists() || exe.to_string_lossy().ends_with(" (deleted)") {
        return Some(true);
    }
    let running = arbos_core::binary_identity::of(&exe);
    let installed = arbos_core::binary_identity::of(on_disk);
    Some(running.is_some() && installed.is_some() && running != installed)
}

fn serving(place_dir: &std::path::Path, on_disk: &std::path::Path) -> Option<Serving> {
    let place = arbos_core::Place::new(place_dir.to_path_buf());
    let text = std::fs::read_to_string(place.kernel_json_read()).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let pid = json.get("pid")?.as_i64()? as i32;
    if !alive(pid) {
        return None;
    }
    Some(Serving {
        place: place_dir.to_path_buf(),
        pid,
        version: json
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_owned(),
        // A kernel old enough not to write its commit is, by that alone, old.
        sha: json
            .get("git_sha")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_owned(),
        binary_gone: pid_binary_gone(pid, on_disk),
    })
}

fn alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 asks whether the pid could be signalled and sends
    // nothing.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// The places to look at: the ones named, or the working directory when it is
/// one. A kernel binary has no register of places, so this reports on what it
/// was pointed at rather than guessing.
fn places(args: &Args) -> Vec<PathBuf> {
    if !args.places.is_empty() {
        return args.places.clone();
    }
    std::env::current_dir()
        .ok()
        .filter(|dir| dir.join(".arbos").is_dir())
        .into_iter()
        .collect()
}

/// Say what is serving, and whether it matches the binary now on disk.
///
/// Returns whether anything is running code older than the file — the state in
/// which "the kernel is up to date" would be a lie.
fn report_serving(args: &Args, on_disk: &kernel::Running) -> bool {
    let running: Vec<Serving> = places(args)
        .iter()
        .filter_map(|dir| serving(dir, &on_disk.path))
        .collect();
    if running.is_empty() {
        return false;
    }
    let mut stale = false;
    println!();
    for one in &running {
        let matches = same_build(&one.sha, &on_disk.sha);
        stale |= !matches;
        // A process whose own file is gone runs an image nothing on disk
        // holds: whatever its sha says, only a restart puts it on the
        // build in front of you. Seven such on two machines went unseen
        // for days because nothing printed this (mesh sweep, 2026-09-17).
        let gone = one.binary_gone == Some(true);
        stale |= gone;
        println!(
            "serving   {} {} (pid {}) in {}{}",
            one.version,
            one.sha,
            one.pid,
            one.place.display(),
            match (gone, matches) {
                (true, _) => format!(
                    "  ← binary replaced under it; restart to run {} (on disk)",
                    on_disk.sha
                ),
                (false, true) => String::new(),
                (false, false) => "  ← older than the binary on disk".to_string(),
            }
        );
    }
    stale
}

fn same_build(a: &str, b: &str) -> bool {
    if a == "unknown" || b == "unknown" || a.is_empty() || b.is_empty() {
        return false;
    }
    let n = a.len().min(b.len());
    a[..n].eq_ignore_ascii_case(&b[..n])
}

/// What is left to do, said plainly.
///
/// The binary on disk is new and every kernel process started from the old one
/// is still running the old code — that is how replacing a file works, and it
/// is the one thing somebody running this by hand has to know.
fn restart_note(binary: &std::path::Path) -> String {
    format!(
        "The binary is replaced. Any kernel already running keeps the old code until it\n\
         restarts. Under a supervisor loop, stopping it is enough:\n\
         \n\
         \x20   kill -TERM $(sed -n 's/.*\"pid\":[[:space:]]*\\([0-9]*\\).*/\\1/p' \\\n\
         \x20       <place>/.arbos/runtime/kernel.json)\n\
         \n\
         That is the graceful stop: every running turn ends the way the stop button ends\n\
         it. Started by hand, start it again from {}.",
        binary.display()
    )
}

#[cfg(test)]
mod tests {
    use super::Args;
    use arbos_update::Channel;

    fn parse(words: &[&str]) -> anyhow::Result<Args> {
        Args::parse(words.iter().map(|w| w.to_string()))
    }

    #[test]
    fn reports_by_default_and_installs_only_when_asked() {
        assert!(!parse(&[]).unwrap().install);
        assert!(parse(&["--install"]).unwrap().install);
    }

    #[test]
    fn reads_the_options_a_person_would_type() {
        let args = parse(&["--channel", "dev", "--binary", "/usr/bin/k", "--install"]).unwrap();
        assert_eq!(args.channel, Some(Channel::Dev));
        assert_eq!(
            args.binary.as_deref(),
            Some(std::path::Path::new("/usr/bin/k"))
        );
        assert!(args.install);

        let args = parse(&["--pin", "0.2.0+903"]).unwrap();
        assert_eq!(args.pin.as_deref(), Some("0.2.0+903"));
        assert_eq!(args.channel, None, "no channel means the machine decides");
    }

    #[test]
    fn refuses_what_it_cannot_do_rather_than_guessing() {
        for bad in [
            vec!["--channel"],
            vec!["--channel", "nightly"],
            vec!["--binary"],
            vec!["--pin"],
            vec!["--instal"],
            vec!["serve"],
        ] {
            assert!(parse(&bad).is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn the_restart_note_names_the_binary_and_the_graceful_stop() {
        let note = super::restart_note(std::path::Path::new("/usr/local/bin/arbos-kernel"));
        assert!(note.contains("/usr/local/bin/arbos-kernel"), "{note}");
        assert!(note.contains("kill -TERM"), "{note}");
        assert!(note.contains("kernel.json"), "{note}");
    }

    /// `serving`'s "binary replaced under it": a process whose own file
    /// was unlinked reads `(deleted)` in /proc, and the line says so.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_process_whose_file_was_unlinked_reads_binary_gone() {
        let dir = std::env::temp_dir().join(format!("arbos-pid-gone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("sleeper");
        std::fs::copy("/bin/sleep", &bin).unwrap();
        // ETXTBSY: another test's fork in this process can hold the fresh
        // file's write descriptor for the instant between its fork and
        // exec. Not the thing under test; try again.
        let mut child = None;
        for _ in 0..50 {
            match std::process::Command::new(&bin).arg("30").spawn() {
                Ok(c) => {
                    child = Some(c);
                    break;
                }
                Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(e) => panic!("spawn: {e}"),
            }
        }
        let mut child = child.expect("the copied sleep started");
        let pid = child.id() as i32;
        assert_eq!(super::pid_binary_gone(pid, &bin), Some(false));
        // An update: a new file renamed over the same path. /proc names
        // the old inode as deleted although the path exists.
        std::fs::copy("/bin/sleep", dir.join("sleeper.new")).unwrap();
        std::fs::rename(dir.join("sleeper.new"), &bin).unwrap();
        assert!(bin.exists());
        assert_eq!(super::pid_binary_gone(pid, &bin), Some(true));

        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
