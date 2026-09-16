//! The feed: what a channel publishes, and how the app decides it is behind.
//!
//! One JSON document per channel, fetched over HTTPS by the running app.
//! Shaped after a Sparkle appcast — a list of releases, each with a version, a
//! note, and one download per platform carrying its length and its signature —
//! because that is the shape that has already survived a decade of Mac apps
//! updating themselves, and because a Sparkle appcast is a mechanical
//! transform away if the desktop ever hosts Sparkle proper. JSON rather than
//! RSS only because `serde_json` is already in both trees and an XML parser is
//! not.
//!
//! Two channels, and the difference is only which document is fetched:
//!
//! - **stable** — the tagged releases. Attached to each GitHub release, read
//!   through `releases/latest`, so the URL never has to name a version.
//! - **dev** — every green commit on `main`. Attached to one rolling
//!   pre-release tagged `dev`, so the URL never has to name a build.

use crate::{sign::PublicKey, version::Version};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The format number in every document. Bumped only if an older app could
/// misread a newer feed; a new optional field does not bump it.
pub const FEED_FORMAT: u32 = 1;

/// Where a payload may be downloaded from.
///
/// The signature already makes a swapped payload useless, but a feed served by
/// somebody else could still point a fresh download at a host of their
/// choosing and watch who asks. Both channels publish through GitHub releases,
/// so the app has no reason to fetch from anywhere else and says so.
pub const ALLOWED_HOSTS: [&str; 2] = ["github.com", "objects.githubusercontent.com"];

/// Which set of builds a machine follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    /// Tagged releases. What anybody who has not asked otherwise gets.
    #[default]
    Stable,
    /// Every green commit on `main`.
    Dev,
}

impl Channel {
    pub const ALL: [Self; 2] = [Self::Stable, Self::Dev];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Dev => "dev",
        }
    }

    /// What the settings row says under the name.
    pub fn describe(self) -> &'static str {
        match self {
            Self::Stable => "Tagged releases.",
            Self::Dev => "Every green commit on main.",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "stable" => Some(Self::Stable),
            "dev" => Some(Self::Dev),
            _ => None,
        }
    }

    /// The document to fetch.
    ///
    /// Neither URL names a version, which is the whole trick: `releases/latest`
    /// is redirected by GitHub to the newest published release, and the dev
    /// channel keeps one pre-release tagged `dev` whose assets are replaced in
    /// place.
    pub fn feed_url(self) -> &'static str {
        match self {
            Self::Stable => {
                "https://github.com/unarbos/arbos/releases/latest/download/arbos-stable.json"
            }
            Self::Dev => "https://github.com/unarbos/arbos/releases/download/dev/arbos-dev.json",
        }
    }

    /// The file the publish step writes and attaches.
    pub fn feed_file(self) -> &'static str {
        match self {
            Self::Stable => "arbos-stable.json",
            Self::Dev => "arbos-dev.json",
        }
    }
}

/// Which of our apps an entry is about.
///
/// `Ios` exists here and has no [`Download`], on purpose. An iPhone is not
/// updated by a program replacing a directory; it is updated by TestFlight or
/// the App Store, which is a [`Link`] and not a payload. Naming the platform
/// now means the day an iOS build exists it is one entry in a feed that
/// already has a place for it, rather than a second feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Macos,
    Linux,
    Ios,
}

impl Platform {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Macos => "macos",
            Self::Linux => "linux",
            Self::Ios => "ios",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "macos" => Some(Self::Macos),
            "linux" => Some(Self::Linux),
            "ios" => Some(Self::Ios),
            _ => None,
        }
    }

    /// Whether a payload for this platform is something a running program can
    /// install by itself. False for iOS, where the system owns installation.
    pub fn installs_itself(self) -> bool {
        match self {
            Self::Macos | Self::Linux => true,
            Self::Ios => false,
        }
    }

    /// The platform this binary is running on, where the feed has a name for
    /// it and the app can install its own update. Anything else says so rather
    /// than offering a download it cannot install.
    pub fn current() -> Option<Self> {
        match std::env::consts::OS {
            "macos" => Some(Self::Macos),
            "linux" => Some(Self::Linux),
            _ => None,
        }
    }
}

/// Which program a payload carries.
///
/// One release of one commit builds both, and they are updated by different
/// things at different moments: the app by a person pressing a blue button,
/// the kernel by itself when it is between turns. A headless box running a
/// kernel has no use for a 32 MB tarball of which the desktop is most, so
/// they are published separately and named here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Component {
    /// The desktop, with the kernel inside it — a macOS bundle, or the Linux
    /// tree with the two binaries side by side.
    #[default]
    App,
    /// `arbos-kernel` on its own.
    Kernel,
}

impl Component {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Kernel => "kernel",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "app" => Some(Self::App),
            "kernel" => Some(Self::Kernel),
            _ => None,
        }
    }

    /// Whether to leave it out of the JSON, so a feed with no kernel payloads
    /// in it is byte-for-byte what it was before this field existed.
    fn is_app(&self) -> bool {
        matches!(self, Self::App)
    }
}

/// Where a build lives for a platform that installs through somebody else's
/// store. Carried beside the downloads so one release describes every app of
/// one commit, and shown rather than installed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Link {
    pub platform: Platform,
    /// `testflight`, `app_store`. Free text so a new one needs no new app.
    pub kind: String,
    pub url: String,
    /// What the store calls this build — a TestFlight build number, say —
    /// where that is not the build number in this release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// How the payload is packed. Both are directory archives that keep symlinks
/// and permissions, which a macOS bundle needs and a Linux tree wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    /// `ditto -c -k --keepParent`, which is what Sparkle expects and what
    /// keeps a code signature intact across a round trip.
    Zip,
    TarGz,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::TarGz => "tar_gz",
        }
    }

    /// Read off the file name, which is the only place CI says it.
    pub fn of_file(name: &str) -> Option<Self> {
        let name = name.to_ascii_lowercase();
        if name.ends_with(".zip") {
            return Some(Self::Zip);
        }
        if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
            return Some(Self::TarGz);
        }
        None
    }
}

/// The arch a payload was built for, as the feed spells it: `arm64`,
/// `x86_64`.
pub fn current_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Feed {
    /// [`FEED_FORMAT`].
    pub format: u32,
    pub channel: Channel,
    /// When the publish step wrote it, RFC 3339. For a person reading the
    /// file; nothing decides anything on it.
    pub generated: String,
    /// Newest first once [`Feed::sorted`] has been through it. Never trusted
    /// to be in any order on the way in.
    pub releases: Vec<Release>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Release {
    /// The marketing version, `0.2.0`.
    pub version: String,
    /// Commits on the branch — see [`crate::version`]. The half that moves
    /// between two dev builds of one version.
    pub build: u64,
    /// The commit the payload was built from, short.
    pub commit: String,
    /// RFC 3339.
    pub published: String,
    /// One or two lines. What the update control shows before it is clicked.
    pub notes: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes_url: Option<String>,
    /// `13.0`. Carried so an old Mac can be told why it is being left behind
    /// rather than handed a payload that will not launch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_system_version: Option<String>,
    /// What a program can install by itself.
    pub downloads: Vec<Download>,
    /// What it cannot — an iOS build in TestFlight, say. Absent from every
    /// feed written so far, and readable by every app that ships before the
    /// first one appears.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<Link>,
}

impl Release {
    pub fn version(&self) -> Result<Version> {
        Ok(Version::parse(&self.version)
            .with_context(|| format!("release `{}`", self.version))?
            .with_build(self.build))
    }

    pub fn download(
        &self,
        platform: Platform,
        arch: &str,
        component: Component,
    ) -> Option<&Download> {
        self.downloads
            .iter()
            .find(|d| d.platform == platform && d.arch == arch && d.component == component)
    }

    pub fn link(&self, platform: Platform) -> Option<&Link> {
        self.links.iter().find(|l| l.platform == platform)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Download {
    pub platform: Platform,
    pub arch: String,
    /// Which program this payload is. Absent means [`Component::App`], which
    /// is what every feed written before kernels could update themselves
    /// meant — and still means, so an old app reading a new feed picks the
    /// same download it always did.
    #[serde(default, skip_serializing_if = "Component::is_app")]
    pub component: Component,
    pub format: Format,
    pub url: String,
    /// Bytes. Checked before the signature so a truncated download fails with
    /// the plain reason rather than the cryptographic one.
    pub size: u64,
    /// Lowercase hex, as `shasum -a 256` prints it.
    pub sha256: String,
    /// Base64 Ed25519 over the bytes of the file — see [`crate::sign`].
    pub signature: String,
}

impl Download {
    /// The name the payload lands under while it is being staged.
    pub fn file_name(&self) -> &str {
        self.url
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or("payload")
    }

    /// Whether the URL is one this app will fetch at all: HTTPS, and a host
    /// the channels actually publish through.
    pub fn check_url(&self) -> Result<()> {
        let rest = self
            .url
            .strip_prefix("https://")
            .with_context(|| format!("update URL is not https: {}", self.url))?;
        let host = rest
            .split('/')
            .next()
            .unwrap_or_default()
            .rsplit('@')
            .next()
            .unwrap_or_default()
            .split(':')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !ALLOWED_HOSTS.contains(&host.as_str()) {
            bail!("update URL is not on a host Arbos publishes through: {host}");
        }
        Ok(())
    }

    /// Whether these bytes are the payload the feed described. Length, then
    /// digest, then signature — cheapest and most legible first, and the one
    /// that actually decides trust last.
    pub fn check_payload(&self, bytes: &[u8], key: &PublicKey) -> Result<()> {
        if bytes.len() as u64 != self.size {
            bail!(
                "download is {} bytes, the feed says {}",
                bytes.len(),
                self.size
            );
        }
        let digest = crate::sign::sha256_hex(bytes);
        if !digest.eq_ignore_ascii_case(&self.sha256) {
            bail!("download does not match its checksum");
        }
        key.verify(bytes, &self.signature)
    }
}

/// A newer build than the one running, with the file to fetch for this
/// machine.
#[derive(Debug, Clone)]
pub struct Available {
    pub version: Version,
    pub notes: String,
    pub notes_url: Option<String>,
    pub published: String,
    pub commit: String,
    pub download: Download,
}

impl Feed {
    pub fn parse(text: &str) -> Result<Self> {
        let feed: Self = serde_json::from_str(text).context("update feed is not readable")?;
        if feed.format > FEED_FORMAT {
            bail!(
                "this update feed is format {} and this build understands {FEED_FORMAT} — update by hand once",
                feed.format
            );
        }
        Ok(feed)
    }

    pub fn read(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text)
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let mut body = serde_json::to_string_pretty(self)?;
        body.push('\n');
        std::fs::write(path, body).with_context(|| format!("writing {}", path.display()))
    }

    pub fn new(channel: Channel, generated: String) -> Self {
        Self {
            format: FEED_FORMAT,
            channel,
            generated,
            releases: Vec::new(),
        }
    }

    /// Newest first, with a release of the same version and build replacing
    /// the one already there — a re-run of the publish step must not leave two
    /// entries for one commit.
    pub fn sorted(&mut self) {
        self.releases.sort_by(|a, b| {
            match (a.version(), b.version()) {
                (Ok(a), Ok(b)) => b.cmp(&a),
                // A release this build cannot parse keeps its place rather
                // than taking the front.
                _ => std::cmp::Ordering::Equal,
            }
        });
    }

    /// Put `release` in, replacing any entry for the same version and build,
    /// and keep at most `keep` of them.
    pub fn put(&mut self, release: Release, keep: usize) {
        self.releases
            .retain(|r| !(r.version == release.version && r.build == release.build));
        self.releases.push(release);
        self.sorted();
        self.releases.truncate(keep.max(1));
    }

    /// The newest release that is strictly newer than `current` and has
    /// something this machine can install.
    ///
    /// Strictly newer, always: a feed served by somebody else cannot walk this
    /// app backwards onto an older build whose signature is perfectly valid.
    pub fn available(
        &self,
        current: &Version,
        platform: Platform,
        arch: &str,
    ) -> Option<Available> {
        self.available_component(current, platform, arch, Component::App)
    }

    /// The same, for a named component. The app compares on the version pair
    /// and refuses anything not strictly newer; see [`Self::newest`] for the
    /// kernel, which has no build number of its own to compare with.
    pub fn available_component(
        &self,
        current: &Version,
        platform: Platform,
        arch: &str,
        component: Component,
    ) -> Option<Available> {
        if !platform.installs_itself() {
            return None;
        }
        let mut best: Option<(Version, &Release, &Download)> = None;
        for release in &self.releases {
            let Ok(version) = release.version() else {
                continue;
            };
            if version <= *current {
                continue;
            }
            let Some(download) = release.download(platform, arch, component) else {
                continue;
            };
            if download.check_url().is_err() {
                continue;
            }
            if best.as_ref().is_none_or(|(best, _, _)| version > *best) {
                best = Some((version, release, download));
            }
        }
        best.map(|(version, release, download)| Available {
            version,
            notes: release.notes.clone(),
            notes_url: release.notes_url.clone(),
            published: release.published.clone(),
            commit: release.commit.clone(),
            download: download.clone(),
        })
    }

    /// The newest release carrying this component, whatever is running.
    ///
    /// The app asks [`Self::available`], which is ordered on the version pair
    /// and refuses anything not strictly newer. A kernel cannot: it knows its
    /// semver and the commit it was built from, and nothing tells it which
    /// build number that was. So it asks for the newest and decides by commit
    /// — see [`crate::kernel::behind`], which is the same rule
    /// `remote_kernel::needs_update_to` has always used over ssh.
    pub fn newest(
        &self,
        platform: Platform,
        arch: &str,
        component: Component,
    ) -> Option<Available> {
        let mut best: Option<(Version, &Release, &Download)> = None;
        for release in &self.releases {
            let Ok(version) = release.version() else {
                continue;
            };
            let Some(download) = release.download(platform, arch, component) else {
                continue;
            };
            if download.check_url().is_err() {
                continue;
            }
            if best.as_ref().is_none_or(|(best, _, _)| version > *best) {
                best = Some((version, release, download));
            }
        }
        best.map(|(version, release, download)| Available {
            version,
            notes: release.notes.clone(),
            notes_url: release.notes_url.clone(),
            published: release.published.clone(),
            commit: release.commit.clone(),
            download: download.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Available, Channel, Component, Download, Feed, Format, Link, Platform, Release,
    };
    use crate::{sign, version::Version};

    fn download(platform: Platform, arch: &str, url: &str) -> Download {
        Download {
            platform,
            arch: arch.into(),
            component: Component::App,
            format: Format::Zip,
            url: url.into(),
            size: 4,
            sha256: sign::sha256_hex(b"zip!"),
            signature: String::new(),
        }
    }

    fn release(version: &str, build: u64, downloads: Vec<Download>) -> Release {
        Release {
            version: version.into(),
            build,
            commit: "abc1234".into(),
            published: "2026-09-15T18:00:00Z".into(),
            notes: "a commit".into(),
            notes_url: None,
            minimum_system_version: Some("13.0".into()),
            downloads,
            links: Vec::new(),
        }
    }

    fn mac(url: &str) -> Vec<Download> {
        vec![download(Platform::Macos, "arm64", url)]
    }

    fn feed(releases: Vec<Release>) -> Feed {
        let mut feed = Feed::new(Channel::Dev, "2026-09-15T18:00:00Z".into());
        feed.releases = releases;
        feed
    }

    const HOST: &str = "https://github.com/unarbos/arbos/releases/download/dev";

    fn pick(feed: &Feed, current: &str) -> Option<Available> {
        feed.available(&Version::parse(current).unwrap(), Platform::Macos, "arm64")
    }

    #[test]
    fn reads_a_feed_the_publish_step_would_write() {
        let mut written = feed(vec![release("0.2.0", 1877, mac(&format!("{HOST}/a.zip")))]);
        written.sorted();
        let text = serde_json::to_string_pretty(&written).unwrap();
        let read = Feed::parse(&text).unwrap();
        assert_eq!(read.channel, Channel::Dev);
        assert_eq!(read.releases.len(), 1);
        assert_eq!(read.releases[0].version().unwrap().build, 1877);
    }

    #[test]
    fn refuses_a_feed_from_a_future_it_cannot_read() {
        let text = r#"{"format":99,"channel":"dev","generated":"x","releases":[]}"#;
        let err = Feed::parse(text).unwrap_err().to_string();
        assert!(err.contains("format 99"), "{err}");
    }

    #[test]
    fn nonsense_is_an_error_and_not_a_panic() {
        assert!(Feed::parse("").is_err());
        assert!(Feed::parse("null").is_err());
        assert!(Feed::parse("{}").is_err());
        assert!(
            Feed::parse(r#"{"format":1,"channel":"moon","generated":"","releases":[]}"#).is_err()
        );
    }

    #[test]
    fn offers_the_newest_build_and_only_when_it_is_newer() {
        let feed = feed(vec![
            release("0.2.0", 1875, mac(&format!("{HOST}/old.zip"))),
            release("0.2.0", 1877, mac(&format!("{HOST}/new.zip"))),
            release("0.2.0", 1876, mac(&format!("{HOST}/mid.zip"))),
        ]);
        assert_eq!(
            pick(&feed, "0.2.0+1875").unwrap().download.url,
            format!("{HOST}/new.zip")
        );
        assert!(pick(&feed, "0.2.0+1877").is_none(), "running the newest");
        assert!(pick(&feed, "0.3.0+1").is_none(), "running something newer");
    }

    #[test]
    fn never_walks_backwards_onto_an_older_build() {
        // The signature stops a forged payload; this stops a real one from an
        // older build being offered as though it were an update.
        let feed = feed(vec![release(
            "0.1.47",
            1200,
            mac(&format!("{HOST}/old.zip")),
        )]);
        assert!(pick(&feed, "0.2.0+1877").is_none());
    }

    #[test]
    fn skips_a_release_with_nothing_for_this_machine() {
        let feed = feed(vec![
            release(
                "0.2.0",
                1878,
                vec![download(
                    Platform::Linux,
                    "x86_64",
                    &format!("{HOST}/l.tar.gz"),
                )],
            ),
            release("0.2.0", 1877, mac(&format!("{HOST}/m.zip"))),
        ]);
        let picked = pick(&feed, "0.2.0+1876").unwrap();
        assert_eq!(picked.version.build, 1877);
        assert_eq!(picked.download.url, format!("{HOST}/m.zip"));
    }

    #[test]
    fn serves_each_platform_its_own_file_from_one_release() {
        let mut downloads = mac(&format!("{HOST}/m.zip"));
        downloads.push(download(
            Platform::Linux,
            "x86_64",
            &format!("{HOST}/l.tar.gz"),
        ));
        let feed = feed(vec![release("0.2.0", 1877, downloads)]);
        let current = Version::parse("0.2.0+1").unwrap();
        assert_eq!(
            feed.available(&current, Platform::Macos, "arm64")
                .unwrap()
                .download
                .url,
            format!("{HOST}/m.zip")
        );
        assert_eq!(
            feed.available(&current, Platform::Linux, "x86_64")
                .unwrap()
                .download
                .url,
            format!("{HOST}/l.tar.gz")
        );
        assert!(
            feed.available(&current, Platform::Macos, "x86_64")
                .is_none()
        );
    }

    #[test]
    fn ignores_a_release_it_cannot_read_rather_than_failing_the_feed() {
        // One bad entry must not stop a good one being offered: the feed is
        // written by a newer CI than the app reading it, always.
        let feed = feed(vec![
            release("not-a-version", 9999, mac(&format!("{HOST}/bad.zip"))),
            release("0.2.0", 1877, mac(&format!("{HOST}/good.zip"))),
        ]);
        assert_eq!(
            pick(&feed, "0.2.0+1").unwrap().download.url,
            format!("{HOST}/good.zip")
        );
    }

    #[test]
    fn will_not_be_pointed_somewhere_else_to_download() {
        for url in [
            "http://github.com/unarbos/arbos/releases/download/dev/a.zip",
            "https://github.com.example.net/a.zip",
            "https://evil.test/a.zip",
            "https://github.com@evil.test/a.zip",
        ] {
            let feed = feed(vec![release("0.2.0", 1877, mac(url))]);
            assert!(pick(&feed, "0.2.0+1").is_none(), "{url} was accepted");
        }
        for url in [
            "https://github.com/unarbos/arbos/releases/download/dev/a.zip",
            "https://objects.githubusercontent.com/x/a.zip",
            "https://GitHub.com/unarbos/arbos/releases/download/dev/a.zip",
        ] {
            let feed = feed(vec![release("0.2.0", 1877, mac(url))]);
            assert!(pick(&feed, "0.2.0+1").is_some(), "{url} was refused");
        }
    }

    #[test]
    fn a_payload_is_checked_by_length_then_digest_then_signature() {
        let (secret, key) = sign::generate().unwrap();
        let bytes = b"Arbos.app, zipped".to_vec();
        let mut d = download(Platform::Macos, "arm64", &format!("{HOST}/a.zip"));
        d.size = bytes.len() as u64;
        d.sha256 = sign::sha256_hex(&bytes);
        d.signature = secret.sign(&bytes).unwrap();
        d.check_payload(&bytes, &key).unwrap();

        let short = &bytes[..bytes.len() - 1];
        assert!(
            d.check_payload(short, &key)
                .unwrap_err()
                .to_string()
                .contains("bytes"),
            "a truncated download should say so plainly"
        );

        let mut swapped = bytes.clone();
        swapped[0] = b'a';
        let err = d.check_payload(&swapped, &key).unwrap_err().to_string();
        assert!(err.contains("checksum"), "{err}");

        // Right bytes for the digest, wrong key: only the signature catches it.
        let (other, _) = sign::generate().unwrap();
        d.signature = other.sign(&bytes).unwrap();
        let err = d.check_payload(&bytes, &key).unwrap_err().to_string();
        assert!(err.contains("signing key"), "{err}");
    }

    #[test]
    fn a_republished_build_replaces_its_own_entry_and_the_list_is_capped() {
        let mut feed = Feed::new(Channel::Dev, "now".into());
        for build in [1875, 1876, 1877] {
            feed.put(
                release("0.2.0", build, mac(&format!("{HOST}/{build}.zip"))),
                2,
            );
        }
        feed.put(release("0.2.0", 1877, mac(&format!("{HOST}/again.zip"))), 2);
        assert_eq!(feed.releases.len(), 2);
        assert_eq!(feed.releases[0].build, 1877);
        assert_eq!(
            feed.releases[0].downloads[0].url,
            format!("{HOST}/again.zip")
        );
        assert_eq!(feed.releases[1].build, 1876);
    }

    #[test]
    fn channels_round_trip_through_the_names_settings_writes() {
        for channel in Channel::ALL {
            assert_eq!(Channel::parse(channel.as_str()), Some(channel));
        }
        assert_eq!(Channel::parse("  DEV "), Some(Channel::Dev));
        assert_eq!(Channel::parse("nightly"), None);
        assert_eq!(Channel::default(), Channel::Stable);
    }

    #[test]
    fn carries_an_ios_build_without_offering_it_as_a_download() {
        // The iPhone app will live in TestFlight, which installs itself and
        // is nobody's payload. A feed written after that day still has to be
        // readable by every app shipped before it.
        let mut with_ios = release("0.2.0", 1877, mac(&format!("{HOST}/m.zip")));
        with_ios.links.push(Link {
            platform: Platform::Ios,
            kind: "testflight".into(),
            url: "https://testflight.apple.com/join/abcdefgh".into(),
            label: Some("TestFlight 42".into()),
        });
        let feed = feed(vec![with_ios]);
        let text = serde_json::to_string(&feed).unwrap();
        let read = Feed::parse(&text).unwrap();
        assert_eq!(
            read.releases[0].link(Platform::Ios).unwrap().kind,
            "testflight"
        );
        // The Mac still gets its Mac payload out of the same release.
        assert_eq!(
            pick(&read, "0.2.0+1").unwrap().download.url,
            format!("{HOST}/m.zip")
        );
        // And nothing ever hands iOS to the in-app installer.
        assert!(
            read.available(&Version::parse("0.1.0").unwrap(), Platform::Ios, "arm64")
                .is_none()
        );
        assert!(!Platform::Ios.installs_itself());
    }

    #[test]
    fn a_feed_with_no_links_in_it_still_reads() {
        // Every feed written before iOS exists. `links` is absent, not empty.
        let text = serde_json::to_string(&feed(vec![release(
            "0.2.0",
            1877,
            mac(&format!("{HOST}/m.zip")),
        )]))
        .unwrap();
        assert!(!text.contains("links"), "{text}");
        assert!(Feed::parse(&text).unwrap().releases[0].links.is_empty());
    }

    #[test]
    fn a_format_is_read_off_the_file_name_ci_wrote() {
        assert_eq!(
            Format::of_file("Arbos-0.2.0-1877-macos-arm64.zip"),
            Some(Format::Zip)
        );
        assert_eq!(
            Format::of_file("arbos-0.2.0-1877-linux-x86_64.tar.gz"),
            Some(Format::TarGz)
        );
        assert_eq!(Format::of_file("Arbos.dmg"), None);
    }
}
