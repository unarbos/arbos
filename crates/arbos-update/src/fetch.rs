//! Where the app fetches from, and how it survives a bad connection.
//!
//! On 2026-10-23 a Mac's update failed with one `tcp connect error: Operation timed out` to
//! github.com: a single attempt, no connect timeout (the OS gave up after ~75 s), no retry, no
//! resume. This module is the answer:
//!
//! - URLs: the stable feed and its payload are fetched from **arbos.life** first (the site mirrors
//!   the GitHub release into `/updates/mac/`, and redirects to GitHub for anything it does not
//!   hold) and from the **GitHub release asset** second; attempts alternate between the two.
//! - Timeouts: 30 s to connect, 60 s without a byte, per attempt.
//! - Retries: up to [`Retry::DEFAULT`] attempts with exponential backoff.
//! - Resume: the `.part` file is kept between attempts and continued with a `Range` request.
//!
//! The signature check over the complete bytes is untouched: the source of the bytes never
//! decides whether they install.

use std::fmt;
use std::time::Duration;

/// The in-app updater's own copy of the stable feed, served by the site.
pub const SITE_UPDATES_BASE: &str = "https://arbos.life/updates/mac";
/// `SITE_UPDATES_BASE/appcast.xml`.
pub const SITE_FEED_URL: &str = "https://arbos.life/updates/mac/appcast.xml";
/// The GitHub release asset base the site mirrors.
pub const GITHUB_RELEASES_BASE: &str = "https://github.com/unarbos/arbos/releases/download/";

/// Per-attempt timeouts and the retry schedule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Retry {
    /// Total attempts, counting the first.
    pub attempts: u32,
    /// Delay after the first failure; doubles each time.
    pub base: Duration,
    /// The delay never exceeds this.
    pub cap: Duration,
    pub connect_timeout: Duration,
    /// Longest wait for the next byte of a response.
    pub read_timeout: Duration,
}

impl Retry {
    pub const DEFAULT: Retry = Retry {
        attempts: 5,
        base: Duration::from_secs(2),
        cap: Duration::from_secs(30),
        connect_timeout: Duration::from_secs(30),
        read_timeout: Duration::from_secs(60),
    };

    /// How long to wait after `failed` attempts have failed (`failed` ≥ 1):
    /// base, 2·base, 4·base, … capped.
    pub fn delay(&self, failed: u32) -> Duration {
        let doublings = failed.saturating_sub(1).min(16);
        let factor = 1u32 << doublings;
        self.base.saturating_mul(factor).min(self.cap)
    }
}

impl Default for Retry {
    fn default() -> Self {
        Retry::DEFAULT
    }
}

/// `https://github.com/unarbos/arbos/releases/download/<tag>/<file>` → `(tag, file)`.
pub fn github_asset_parts(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix(GITHUB_RELEASES_BASE)?;
    let (tag, file) = rest.split_once('/')?;
    if tag.is_empty() || file.is_empty() || file.contains('/') || file.contains('?') {
        return None;
    }
    Some((tag, file))
}

/// `v0.6.173` — a stable tag, the only kind the site mirrors.
fn is_stable_tag(tag: &str) -> bool {
    tag.strip_prefix('v')
        .is_some_and(|rest| rest.chars().next().is_some_and(|c| c.is_ascii_digit()))
}

/// The places to download a payload from, in order of preference. A stable GitHub release
/// asset is fetched from the site mirror first and from GitHub second; any other URL (an edge
/// build, a test server) is used as is.
pub fn download_urls(enclosure_url: &str) -> Vec<String> {
    match github_asset_parts(enclosure_url) {
        Some((tag, file)) if is_stable_tag(tag) => vec![
            format!("{SITE_UPDATES_BASE}/{file}"),
            enclosure_url.to_string(),
        ],
        _ => vec![enclosure_url.to_string()],
    }
}

/// The feeds to try for a check, in order. The site feed falls back to the GitHub feed it
/// mirrors; any other feed (edge, a test server) is used alone.
pub fn feed_urls(feed_url: &str) -> Vec<String> {
    if feed_url == SITE_FEED_URL {
        vec![
            SITE_FEED_URL.to_string(),
            crate::DEFAULT_FEED_URL.to_string(),
        ]
    } else {
        vec![feed_url.to_string()]
    }
}

/// The host of a URL, for messages (`arbos.life`).
pub fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    rest.split(['/', '?', '#']).next().unwrap_or(rest)
}

/// Every attempt failed. `Display` is one plain sentence for the person; the attempts' errors
/// are in `errors` for the log.
#[derive(Debug)]
pub struct DownloadFailed {
    pub attempts: u32,
    /// `(url, error)` per attempt, oldest first.
    pub errors: Vec<(String, anyhow::Error)>,
}

impl DownloadFailed {
    /// Plain words: which hosts were tried, how often, and what the connection did on the last
    /// try — not which library said so. Short enough for the Settings row; the caller adds what
    /// to do next.
    pub fn plain(&self) -> String {
        let Some((_, last)) = self.errors.last() else {
            return "Could not download the update.".to_string();
        };
        let mut hosts: Vec<&str> = Vec::new();
        for (url, _) in &self.errors {
            let host = host_of(url);
            if !hosts.contains(&host) {
                hosts.push(host);
            }
        }
        let hosts = hosts.join(" or ");
        let what = describe(last);
        if self.attempts <= 1 {
            format!("Could not download from {hosts} ({what}).")
        } else {
            format!(
                "Could not download from {hosts} after {} tries ({what}).",
                self.attempts
            )
        }
    }

    /// Every attempt's error, one line each, for the log.
    pub fn detail(&self) -> String {
        self.errors
            .iter()
            .enumerate()
            .map(|(i, (url, err))| format!("attempt {}: {url}: {err:#}", i + 1))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl fmt::Display for DownloadFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.plain())
    }
}

impl std::error::Error for DownloadFailed {}

/// Whether another try a few seconds later could do better.
///
/// True for the connection: a name that did not resolve, a socket that did not open, a
/// transfer that stalled or dropped. On macOS `getaddrinfo` answers `nodename nor servname
/// provided, or not known` for a few seconds after the Mac wakes or joins a network, before
/// the system resolver has its configuration — a real host, momentarily unknown.
///
/// False for an answer: a 404, a 500, or a body that is not a feed. Those are the server's
/// verdict and will read the same on the next try, so the other feed URL is the only hope.
pub fn worth_retrying(err: &anyhow::Error) -> bool {
    if let Some(re) = err.downcast_ref::<reqwest::Error>() {
        // Every other kind reqwest reports is about the connection or the transfer, so it is
        // worth another try; these two are a verdict on the request itself.
        return !(re.is_status() || re.is_builder());
    }
    let text = format!("{err:#}").to_ascii_lowercase();
    [
        "dns",
        "lookup",
        "resolve",
        "timed out",
        "timeout",
        "refused",
        "reset",
        "unreachable",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

/// One plain phrase for a transport error.
pub fn describe(err: &anyhow::Error) -> String {
    if let Some(re) = err.downcast_ref::<reqwest::Error>() {
        if re.is_timeout() && re.is_connect() {
            return "the connection timed out".into();
        }
        if re.is_timeout() {
            return "the server stopped sending data".into();
        }
        if let Some(status) = re.status() {
            return format!("the server answered {}", status.as_u16());
        }
        if re.is_connect() {
            return "the server could not be reached".into();
        }
        if re.is_body() || re.is_decode() {
            return "the connection dropped".into();
        }
    }
    let text = format!("{err:#}").to_ascii_lowercase();
    if text.contains("timed out") || text.contains("timeout") {
        "the connection timed out".into()
    } else if text.contains("dns") || text.contains("resolve") || text.contains("lookup") {
        "the server name could not be looked up".into()
    } else if text.contains("refused") || text.contains("unreachable") || text.contains("reset") {
        "the server could not be reached".into()
    } else if text.contains("bytes") && text.contains("appcast says") {
        "the file was incomplete".into()
    } else {
        "the connection dropped".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAR: &str =
        "https://github.com/unarbos/arbos/releases/download/v0.6.173/Arbos-0.6.173-macos-universal.tar.gz";

    #[test]
    fn github_asset_is_mirrored_first_then_github() {
        assert_eq!(
            download_urls(TAR),
            vec![
                "https://arbos.life/updates/mac/Arbos-0.6.173-macos-universal.tar.gz".to_string(),
                TAR.to_string(),
            ]
        );
    }

    #[test]
    fn other_urls_are_used_alone() {
        let edge = "https://github.com/unarbos/arbos/releases/download/edge-0.6.5.529/x.tar.gz";
        assert_eq!(
            download_urls(edge),
            vec![edge.to_string()],
            "edge is not mirrored"
        );
        let site = "https://arbos.life/updates/mac/Arbos-0.6.173-macos-universal.tar.gz";
        assert_eq!(download_urls(site), vec![site.to_string()]);
        let local = "http://127.0.0.1:9/payload.tar.gz";
        assert_eq!(download_urls(local), vec![local.to_string()]);
        let fork = "https://github.com/someone/else/releases/download/v1/x.tar.gz";
        assert_eq!(download_urls(fork), vec![fork.to_string()]);
    }

    #[test]
    fn asset_parts_reject_odd_shapes() {
        assert_eq!(
            github_asset_parts(TAR),
            Some(("v0.6.173", "Arbos-0.6.173-macos-universal.tar.gz"))
        );
        assert_eq!(
            github_asset_parts("https://github.com/unarbos/arbos/releases/download/v1/"),
            None
        );
        assert_eq!(
            github_asset_parts("https://github.com/unarbos/arbos/releases/download/v1/a/b"),
            None
        );
        assert_eq!(
            github_asset_parts("https://github.com/unarbos/arbos/releases/download/v1/a?x=1"),
            None
        );
    }

    #[test]
    fn site_feed_falls_back_to_github_feed() {
        assert_eq!(
            feed_urls(SITE_FEED_URL),
            vec![
                SITE_FEED_URL.to_string(),
                crate::DEFAULT_FEED_URL.to_string()
            ]
        );
        let edge = "https://github.com/unarbos/arbos/releases/download/edge/appcast.xml";
        assert_eq!(feed_urls(edge), vec![edge.to_string()]);
    }

    #[test]
    fn backoff_doubles_and_caps() {
        let r = Retry::DEFAULT;
        assert_eq!(r.delay(1), Duration::from_secs(2));
        assert_eq!(r.delay(2), Duration::from_secs(4));
        assert_eq!(r.delay(3), Duration::from_secs(8));
        assert_eq!(r.delay(4), Duration::from_secs(16));
        assert_eq!(r.delay(5), Duration::from_secs(30));
        assert_eq!(
            r.delay(40),
            Duration::from_secs(30),
            "no overflow, stays capped"
        );
        assert_eq!(r.delay(0), Duration::from_secs(2));
        let total: Duration = (1..r.attempts).map(|n| r.delay(n)).sum();
        assert_eq!(
            total,
            Duration::from_secs(30),
            "waits between the 5 attempts"
        );
    }

    #[test]
    fn hosts_for_messages() {
        assert_eq!(host_of(TAR), "github.com");
        assert_eq!(host_of(SITE_FEED_URL), "arbos.life");
        assert_eq!(host_of("http://127.0.0.1:4321/x?y"), "127.0.0.1:4321");
    }

    #[test]
    fn plain_words_after_retries() {
        let failed = DownloadFailed {
            attempts: 5,
            errors: vec![
                (
                    "https://arbos.life/updates/mac/a.tar.gz".into(),
                    anyhow::anyhow!("x"),
                ),
                (
                    TAR.into(),
                    anyhow::anyhow!("tcp connect error: Operation timed out (os error 60)"),
                ),
            ],
        };
        assert_eq!(
            failed.plain(),
            "Could not download from arbos.life or github.com after 5 tries (the connection timed out)."
        );
        let once = DownloadFailed {
            attempts: 1,
            errors: vec![(TAR.into(), anyhow::anyhow!("Connection refused"))],
        };
        assert_eq!(
            once.plain(),
            "Could not download from github.com (the server could not be reached)."
        );
        assert!(failed.detail().starts_with("attempt 1: https://arbos.life"));
        assert!(failed.detail().contains("attempt 2: https://github.com"));
        assert_eq!(
            describe(&anyhow::anyhow!("payload is 10 bytes, the appcast says 20")),
            "the file was incomplete"
        );
        assert_eq!(
            describe(&anyhow::anyhow!("dns error: failed to lookup address")),
            "the server name could not be looked up"
        );
        assert_eq!(
            describe(&anyhow::anyhow!("Connection refused")),
            "the server could not be reached"
        );
    }
}
