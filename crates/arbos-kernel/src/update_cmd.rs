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
    What the channel has, and whether this binary is behind it. --install
    replaces it; without that it only reports. The running kernel keeps the
    code it started with until it restarts.";

pub struct Args {
    pub install: bool,
    pub channel: Option<Channel>,
    pub binary: Option<PathBuf>,
    pub pin: Option<String>,
}

impl Args {
    pub fn parse(argv: impl Iterator<Item = String>) -> Result<Self> {
        let mut args = Self {
            install: false,
            channel: None,
            binary: None,
            pin: None,
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
    println!();
    println!("{}", restart_note(&binary));
    Ok(0)
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
        assert_eq!(args.binary.as_deref(), Some(std::path::Path::new("/usr/bin/k")));
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
}
