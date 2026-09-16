//! Fetching a feed and a payload.
//!
//! Small on purpose, and here rather than in each caller so that the two
//! programs that update a kernel — `arbos-updatectl` and `arbos-kernel update`
//! — reach the network the same way, with the same timeout, and could not
//! drift into disagreeing about what an HTTP error means.
//!
//! Nothing here decides whether a URL may be fetched. That is
//! [`crate::feed::Download::check_url`]'s, and every caller runs it first: a
//! function that sometimes checks is one that sometimes does not.

use anyhow::{Context, Result};
use std::time::Duration;

/// Long enough for a kernel payload on a slow link, short enough that a
/// server which has stopped answering does not hold an update open forever.
const TIMEOUT: Duration = Duration::from_secs(120);

fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(TIMEOUT)
        .build()
        .context("building an HTTP client")
}

pub fn text(url: &str) -> Result<String> {
    client()?
        .get(url)
        .send()
        .and_then(|response| response.error_for_status())
        .with_context(|| format!("fetching {url}"))?
        .text()
        .with_context(|| format!("reading {url}"))
}

pub fn bytes(url: &str) -> Result<Vec<u8>> {
    Ok(client()?
        .get(url)
        .send()
        .and_then(|response| response.error_for_status())
        .with_context(|| format!("fetching {url}"))?
        .bytes()
        .with_context(|| format!("reading {url}"))?
        .to_vec())
}

/// The channel's feed, parsed.
pub fn feed(channel: crate::Channel) -> Result<crate::Feed> {
    let body = text(channel.feed_url())
        .with_context(|| format!("asking the {} channel", channel.as_str()))?;
    crate::Feed::parse(&body)
}
