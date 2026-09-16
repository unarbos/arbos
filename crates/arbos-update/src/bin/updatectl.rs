//! `arbos-updatectl` — what CI signs and publishes an update with, and what a
//! person sets the signing key up with once.
//!
//!   arbos-updatectl keygen --public crates/arbos-update/update-key.pub
//!       A fresh key pair. Writes the public half to the file (commit it) and
//!       prints the secret half to stdout and nowhere else, so it can be piped
//!       straight into `gh secret set ARBOS_UPDATE_SIGNING_KEY` without ever
//!       landing in a file or a shell history.
//!
//!   arbos-updatectl sign <file>
//!       The base64 signature over a file's bytes.
//!
//!   arbos-updatectl verify --signature <base64> <file>
//!       Whether that signature is this key's. Exits non-zero when it is not.
//!
//!   arbos-updatectl add --feed dist/arbos-dev.json --channel dev \
//!       --version 0.2.0 --build 1877 --commit 9f552c5 \
//!       --base-url https://github.com/unarbos/arbos/releases/download/dev \
//!       --notes-file dist/notes.txt --keep 10 \
//!       --artifact macos/arm64=dist/Arbos-0.2.0-1877-macos-arm64.zip \
//!       --artifact linux/x86_64=dist/arbos-0.2.0-1877-linux-x86_64.tar.gz \
//!       --artifact linux/x86_64/kernel=dist/arbos-kernel-0.2.0-1877-linux-x86_64.tar.gz
//!       One release into a channel's feed: each artifact measured, digested
//!       and signed, the entry put in, older entries past `--keep` dropped.
//!       Merges into the feed already there, so the file fetched from the last
//!       publish is the one to pass in.
//!
//!   arbos-updatectl show --feed dist/arbos-dev.json --current 0.2.0+1876
//!       What an app on `--current` would be offered. The smoke test for a
//!       feed that was just written.
//!
//! The secret is read from `ARBOS_UPDATE_SIGNING_KEY` and never from an
//! argument: an argument is visible to every other process on the machine.

use anyhow::{Context, Result, bail};
use arbos_update::{
    Channel, Component, Download, Feed, Format, Platform, Release, Version,
    feed::Link,
    kernel as kernel_mod,
    sign::{self, PublicKey, SecretKey},
};
use std::{collections::BTreeMap, path::PathBuf, process::ExitCode};

/// The variable CI puts the private half in.
const KEY_ENV: &str = "ARBOS_UPDATE_SIGNING_KEY";

/// Where the public half lives when nothing says otherwise.
const KEY_FILE: &str = "crates/arbos-update/update-key.pub";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("arbos-updatectl: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let mut argv = std::env::args().skip(1);
    let command = argv.next().unwrap_or_default();
    let args = Args::parse(argv)?;
    match command.as_str() {
        "keygen" => keygen(&args),
        "sign" => sign_file(&args),
        "verify" => verify_file(&args),
        "add" => add(&args),
        "show" => show(&args),
        "kernel" => kernel(&args),
        "" | "-h" | "--help" | "help" => {
            println!("{USAGE}");
            Ok(())
        }
        other => bail!("no such command `{other}`\n\n{USAGE}"),
    }
}

const USAGE: &str = "\
arbos-updatectl keygen  [--public <path>]
arbos-updatectl sign    <file>
arbos-updatectl verify  --signature <base64> [--public <path>] <file>
arbos-updatectl add     --feed <path> --channel <stable|dev> --version <x.y.z>
                        --build <n> --commit <sha> --base-url <url>
                        [--notes <text> | --notes-file <path>] [--notes-url <url>]
                        [--minimum-system-version <x.y>] [--keep <n>]
                        --artifact <platform>/<arch>[/<component>]=<path> ...
                        [--link <platform>:<kind>=<url> ...]
arbos-updatectl show    --feed <path> [--current <x.y.z+n>] [--platform <p>] [--arch <a>]
arbos-updatectl kernel  [--channel <stable|dev>] [--binary <path>] [--pin <x.y.z+n>]
                        [--install]

The signing key is read from ARBOS_UPDATE_SIGNING_KEY.";

fn keygen(args: &Args) -> Result<()> {
    let path = PathBuf::from(args.one("public").unwrap_or(KEY_FILE.into()));
    let (secret, public) = sign::generate()?;
    let kept = std::fs::read_to_string(&path).unwrap_or_default();
    // The file is mostly the note explaining what it is and how it got there.
    // Keep every comment line and replace only the key.
    let mut body: String = kept
        .lines()
        .filter(|line| line.trim().is_empty() || line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    body.push_str(&public.to_line());
    body.push('\n');
    std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    eprintln!("wrote the public half to {} — commit it", path.display());
    eprintln!("the secret half is on stdout; pipe it into `gh secret set {KEY_ENV}`");
    println!("{}", secret.to_base64());
    Ok(())
}

fn sign_file(args: &Args) -> Result<()> {
    let path = args.only_free("a file to sign")?;
    let bytes = read(&path)?;
    println!("{}", secret_key()?.sign(&bytes)?);
    Ok(())
}

fn verify_file(args: &Args) -> Result<()> {
    let path = args.only_free("a file to verify")?;
    let signature = args.need("signature")?;
    let key = public_key(args)?;
    key.verify(&read(&path)?, &signature)?;
    println!("ok {}", path.display());
    Ok(())
}

fn add(args: &Args) -> Result<()> {
    let feed_path = PathBuf::from(args.need("feed")?);
    let channel =
        Channel::parse(&args.need("channel")?).with_context(|| "--channel is stable or dev")?;
    let version = args.need("version")?;
    // Parsed and thrown away: a version the app cannot order is a release
    // nobody will ever be offered, and CI should hear about it here.
    Version::parse(&version).context("--version")?;
    let build: u64 = args.need("build")?.parse().context("--build is a number")?;
    let commit = args.need("commit")?;
    let base_url = args.need("base-url")?;
    let base_url = base_url.trim_end_matches('/').to_owned();
    let keep: usize = match args.one("keep") {
        Some(keep) => keep.parse().context("--keep is a number")?,
        None => 10,
    };
    let notes = match (args.one("notes"), args.one("notes-file")) {
        (Some(_), Some(_)) => bail!("--notes and --notes-file are two answers to one question"),
        (Some(notes), None) => notes,
        (None, Some(path)) => {
            std::fs::read_to_string(&path).with_context(|| format!("reading {path}"))?
        }
        (None, None) => String::new(),
    };
    let secret = secret_key()?;

    let mut downloads = Vec::new();
    for spec in args.all("artifact") {
        downloads.push(measure(&spec, &base_url, &secret)?);
    }
    if downloads.is_empty() {
        bail!("no --artifact: a release with nothing to download is not a release");
    }

    // Platforms nobody's program installs — an iOS build in TestFlight. A
    // link, not a payload, so it is neither measured nor signed.
    let mut links = Vec::new();
    for spec in args.all("link") {
        let (target, url) = spec
            .split_once('=')
            .with_context(|| format!("--link {spec} is not <platform>:<kind>=<url>"))?;
        let (platform, kind) = target
            .split_once(':')
            .with_context(|| format!("--link {spec} is not <platform>:<kind>=<url>"))?;
        links.push(Link {
            platform: Platform::parse(platform)
                .with_context(|| format!("`{platform}` is not a platform the feed knows"))?,
            kind: kind.to_owned(),
            url: url.to_owned(),
            label: None,
        });
    }

    let mut feed = match feed_path.exists() {
        true => Feed::read(&feed_path)?,
        false => Feed::new(channel, now()),
    };
    if feed.channel != channel {
        bail!(
            "{} is the {} feed, not {}",
            feed_path.display(),
            feed.channel.as_str(),
            channel.as_str()
        );
    }
    feed.generated = now();
    feed.put(
        Release {
            version: version.clone(),
            build,
            commit: commit.clone(),
            published: now(),
            notes: notes.trim().to_owned(),
            notes_url: args.one("notes-url"),
            minimum_system_version: args.one("minimum-system-version"),
            downloads,
            links,
        },
        keep,
    );
    feed.write(&feed_path)?;
    eprintln!(
        "{}: {} {version}+{build} ({commit}), {} release(s) kept",
        feed_path.display(),
        channel.as_str(),
        feed.releases.len()
    );
    Ok(())
}

/// One `--artifact macos/arm64=path/to/file` into the entry the feed carries:
/// its length, its digest, its signature, and the URL it will be fetched from.
fn measure(spec: &str, base_url: &str, secret: &SecretKey) -> Result<Download> {
    let shape = || format!("--artifact {spec} is not <platform>/<arch>[/<component>]=<path>");
    let (target, path) = spec.split_once('=').with_context(shape)?;
    let (platform, rest) = target.split_once('/').with_context(shape)?;
    // The component is optional and defaults to the app, so every publish
    // step written before kernels could update themselves still says what it
    // always said.
    let (arch, component) = match rest.split_once('/') {
        Some((arch, component)) => (
            arch,
            Component::parse(component)
                .with_context(|| format!("`{component}` is not a component the feed knows"))?,
        ),
        None => (rest, Component::App),
    };
    let platform = Platform::parse(platform)
        .with_context(|| format!("`{platform}` is not a platform the feed knows"))?;
    let path = PathBuf::from(path);
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .with_context(|| format!("{} has no file name", path.display()))?
        .to_owned();
    let format = Format::of_file(&name)
        .with_context(|| format!("{name} is neither a .zip nor a .tar.gz"))?;
    let bytes = read(&path)?;
    Ok(Download {
        platform,
        arch: arch.to_owned(),
        component,
        format,
        url: format!("{base_url}/{name}"),
        size: bytes.len() as u64,
        sha256: sign::sha256_hex(&bytes),
        signature: secret.sign(&bytes)?,
    })
}

fn show(args: &Args) -> Result<()> {
    let feed = Feed::read(&PathBuf::from(args.need("feed")?))?;
    let current = match args.one("current") {
        Some(text) => Version::parse(&text).context("--current")?,
        None => Version::new(0, 0, 0, 0),
    };
    let platform = match args.one("platform") {
        Some(name) => Platform::parse(&name).context("--platform")?,
        None => Platform::current().context("--platform: this is not a platform Arbos ships")?,
    };
    let arch = args
        .one("arch")
        .unwrap_or_else(|| arbos_update::feed::current_arch().to_owned());
    println!(
        "{} feed, generated {}, {} release(s)",
        feed.channel.as_str(),
        feed.generated,
        feed.releases.len()
    );
    match feed.available(&current, platform, &arch) {
        Some(update) => {
            println!(
                "{} on {}/{arch} would be offered {} ({})",
                current.human(),
                platform.as_str(),
                update.version.human(),
                update.commit
            );
            println!("  {}", update.download.url);
            println!(
                "  {} bytes, sha256 {}",
                update.download.size, update.download.sha256
            );
        }
        None => println!(
            "{} on {}/{arch} is up to date",
            current.human(),
            platform.as_str()
        ),
    }
    Ok(())
}

/// Bring an `arbos-kernel` binary up to what the channel publishes.
///
/// The whole of the update, with a person choosing the moment — which is the
/// point of it existing as a command before anything runs unattended. A
/// kernel on a box nobody ssh's into and nobody attaches to is exactly the one
/// that goes stale, and this fixes it today without the kernel having learned
/// anything.
///
/// It refuses the same four ways a kernel updating itself would: a binary in a
/// build tree is somebody's working copy, a pin is a pin, a newer build is
/// left alone, and a channel with no kernel for this machine is not an error.
fn kernel(args: &Args) -> Result<()> {
    let channel = match args.one("channel") {
        Some(name) => Channel::parse(&name).context("--channel is stable or dev")?,
        None => Channel::default(),
    };
    let binary = kernel_mod::find(args.one("binary").map(PathBuf::from).as_deref())?;
    let running = kernel_mod::Running::read(&binary)?;
    println!(
        "running   {} {} ({})",
        running.version.human(),
        running.sha,
        binary.display()
    );

    let feed = Feed::parse(&fetch_text(channel.feed_url())?)?;
    let offered = match kernel_mod::plan(&running, &feed, args.one("pin").as_deref()) {
        Ok(offered) => offered,
        Err(refusal) => {
            println!("no update: {}", refusal.say());
            return Ok(());
        }
    };
    println!(
        "available {} ({}) on the {} channel",
        offered.version.human(),
        offered.commit,
        channel.as_str()
    );
    println!("          {}", offered.download.url);
    if !args.flag("install") {
        println!("\nrun again with --install to replace it");
        return Ok(());
    }

    let key = sign::built_in_key()
        .context("this build carries no update key, so it cannot check a payload")?;
    println!("fetching  {} bytes", offered.download.size);
    let bytes = fetch_bytes(&offered.download.url)?;
    let scratch = std::env::temp_dir().join("arbos-kernel-update");
    kernel_mod::verify_and_install(&bytes, &offered, &binary, &key, &scratch)?;
    let now = kernel_mod::Running::read(&binary)?;
    println!("installed {} {}", now.version.human(), now.sha);
    println!(
        "\nThe running kernel is still the old one until it restarts. Stop it the way it was\n\
         started — a supervisor loop restarts it, and `kill -TERM` on the pid in\n\
         <place>/.arbos/runtime/kernel.json is the graceful stop."
    );
    Ok(())
}

fn http() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .context("building an HTTP client")
}

fn fetch_text(url: &str) -> Result<String> {
    http()?
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .with_context(|| format!("fetching {url}"))?
        .text()
        .with_context(|| format!("reading {url}"))
}

fn fetch_bytes(url: &str) -> Result<Vec<u8>> {
    Ok(http()?
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .with_context(|| format!("fetching {url}"))?
        .bytes()
        .with_context(|| format!("reading {url}"))?
        .to_vec())
}

fn secret_key() -> Result<SecretKey> {
    let text = std::env::var(KEY_ENV)
        .ok()
        .filter(|t| !t.trim().is_empty())
        .with_context(|| {
            format!("{KEY_ENV} is not set — run `arbos-updatectl keygen` once and store it")
        })?;
    SecretKey::from_base64(&text).with_context(|| format!("{KEY_ENV}"))
}

fn public_key(args: &Args) -> Result<PublicKey> {
    let path = args.one("public").unwrap_or(KEY_FILE.into());
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {path}"))?;
    sign::parse_key_file(&text).with_context(|| format!("{path} holds no `ed25519 <base64>` line"))
}

fn read(path: &std::path::Path) -> Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("reading {}", path.display()))
}

/// RFC 3339, to the second, in UTC. Written by hand: this is one field in one
/// document that nothing decides anything on, which is not worth a date crate
/// in the kernel's dependency graph.
fn now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (hour, minute, second) = (rest / 3600, (rest % 3600) / 60, rest % 60);
    // Civil date from a day count — Howard Hinnant's `civil_from_days`.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// `--name value`, repeatable, plus whatever is left over.
///
/// Hand-rolled because this tool has five commands and no ambiguity in any of
/// them, and because the kernel's dependency graph should not grow an argument
/// parser for a CI helper.
struct Args {
    flags: BTreeMap<String, Vec<String>>,
    free: Vec<String>,
}

impl Args {
    fn parse(argv: impl Iterator<Item = String>) -> Result<Self> {
        let mut flags: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut free = Vec::new();
        let mut argv = argv.peekable();
        while let Some(arg) = argv.next() {
            match arg.strip_prefix("--") {
                Some(name) => {
                    let (name, value) = match name.split_once('=') {
                        Some((name, value)) => (name.to_owned(), value.to_owned()),
                        // A flag whose next word is another flag, or which
                        // ends the line, is a switch rather than a name with a
                        // value. `--install` is one; everything else here
                        // takes a value and gets the word after it.
                        None => match argv.peek() {
                            Some(next) if !next.starts_with("--") => (
                                name.to_owned(),
                                argv.next().expect("peeked"),
                            ),
                            _ => (name.to_owned(), String::new()),
                        },
                    };
                    flags.entry(name).or_default().push(value);
                }
                None => free.push(arg),
            }
        }
        Ok(Self { flags, free })
    }

    fn one(&self, name: &str) -> Option<String> {
        self.flags.get(name)?.last().cloned().filter(|v| !v.is_empty())
    }

    /// Whether a bare `--name` was given.
    fn flag(&self, name: &str) -> bool {
        self.flags.contains_key(name)
    }

    fn all(&self, name: &str) -> Vec<String> {
        self.flags.get(name).cloned().unwrap_or_default()
    }

    fn need(&self, name: &str) -> Result<String> {
        self.one(name)
            .with_context(|| format!("--{name} is needed"))
    }

    fn only_free(&self, what: &str) -> Result<PathBuf> {
        match self.free.as_slice() {
            [one] => Ok(PathBuf::from(one)),
            [] => bail!("{what}"),
            many => bail!("{what}, not {}", many.len()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Args, now};

    #[test]
    fn reads_repeated_flags_and_both_spellings() {
        let args = Args::parse(
            [
                "--feed",
                "f.json",
                "--artifact=macos/arm64=a.zip",
                "--artifact",
                "linux/x86_64=b.tar.gz",
                "left-over",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        assert_eq!(args.one("feed").as_deref(), Some("f.json"));
        assert_eq!(
            args.all("artifact"),
            ["macos/arm64=a.zip", "linux/x86_64=b.tar.gz"]
        );
        assert_eq!(args.free, ["left-over"]);
        assert!(args.need("missing").is_err());
    }

    #[test]
    fn a_bare_flag_is_a_switch_and_not_a_missing_value() {
        // `--install` ends the line and takes nothing; `--channel dev` after
        // it still reads as a pair.
        let args = Args::parse(
            ["--channel", "dev", "--install"].into_iter().map(String::from),
        )
        .unwrap();
        assert!(args.flag("install"));
        assert_eq!(args.one("install"), None);
        assert_eq!(args.one("channel").as_deref(), Some("dev"));
        assert!(!args.flag("pin"));

        // And a switch in the middle does not eat the flag after it.
        let args =
            Args::parse(["--install", "--channel", "dev"].into_iter().map(String::from)).unwrap();
        assert!(args.flag("install"));
        assert_eq!(args.one("channel").as_deref(), Some("dev"));
    }

    #[test]
    fn the_timestamp_is_rfc_3339() {
        let stamp = now();
        assert_eq!(stamp.len(), 20, "{stamp}");
        assert!(stamp.ends_with('Z'), "{stamp}");
        assert!(stamp.starts_with("20"), "{stamp}");
        assert_eq!(stamp.as_bytes()[10], b'T', "{stamp}");
    }
}
