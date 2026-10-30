//! Link previews (Jacob, 2026-10-27, #2064): when a row completes a message that holds a bare
//! URL, the relay fetches the page **once** — with its own address, never a viewer's — and
//! appends a `link_preview` part to that message, so every device draws the same card from the
//! same facts and nobody's client talks to the site until they press Open. Old clients skip the
//! unknown part and keep today's bare link.
//!
//! What is fetched and how: a provider parser first (Spotify, YouTube, X, GitHub, Bluesky,
//! Vimeo, Loom, SoundCloud, Apple Music, Google Maps, Wikipedia, direct files), then the generic
//! Open Graph / Twitter-card / `<title>` reading. Every request goes through [`Fetcher`]: public
//! host names only, resolved **before** connecting and pinned to those addresses, private /
//! loopback / link-local / CGNAT / metadata ranges refused (again on every redirect), http(s)
//! only, ≤ [`MAX_REDIRECTS`] hops, 5 s to connect, [`TOTAL_BUDGET`] for the whole unfurl, HTML
//! read up to [`PAGE_MAX_BYTES`], pictures up to [`IMAGE_MAX_BYTES`] and re-encoded to at most
//! [`IMAGE_MAX_SIDE`] px and [`IMAGE_MAX_OUT_BYTES`]. No cookies, no credentials of any kind.
//!
//! What is kept: only what the card shows, as the part on the message; the picture as a blob in
//! the chat's own slot (readable by exactly the chat's members). A cache under
//! `<data>/link-previews/` keyed by the canonical URL answers repeats for [`OK_TTL_MS`]
//! (failures [`FAIL_TTL_MS`]). A message whose sender removed the preview (`preview: "off"`)
//! is never unfurled. Kill switch: `ARBOS_LINK_PREVIEWS=off`; host denylist
//! `ARBOS_LINK_PREVIEW_DENY=a.com,b.org`. Any failure means no part — silently.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Cursor;
use std::net::SocketAddr;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use bytes::Bytes;
use reqwest::header;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

use arbos_core::doc::ChatDoc;
use arbos_core::ids::{now_ms, ulid};

use crate::link_icons::{absolute, attr, is_private_ip, meta_content, public_domain, tags};
use crate::people::PEOPLE_PROJECT;
use crate::state::AppState;

/// The part kind the clients read.
pub const PART_KIND: &str = "link_preview";
/// Bumped when the part's fields change shape.
pub const SPEC_VERSION: i64 = 1;
/// A message field the sender sets to remove the card for everyone.
pub const PREVIEW_FIELD: &str = "preview";
pub const PREVIEW_OFF: &str = "off";

pub const MAX_REDIRECTS: usize = 3;
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Page + provider call + picture, all told.
pub const TOTAL_BUDGET: Duration = Duration::from_secs(10);
pub const PAGE_MAX_BYTES: usize = 512 * 1024;
pub const IMAGE_MAX_BYTES: usize = 5 * 1024 * 1024;
pub const IMAGE_MAX_SIDE: u32 = 1280;
/// Square art (album covers) is kept smaller.
pub const ART_MAX_SIDE: u32 = 300;
pub const IMAGE_MAX_OUT_BYTES: usize = 300 * 1024;
pub const OK_TTL_MS: i64 = 24 * 60 * 60 * 1000;
pub const FAIL_TTL_MS: i64 = 60 * 60 * 1000;
/// A (room, message) that was looked at is not looked at again for this long.
const ATTEMPT_TTL_MS: i64 = 60 * 60 * 1000;
/// Rows arrive in bursts (a streaming agent turn); the room is scanned this long after the last.
const DEBOUNCE_DEVICE: Duration = Duration::from_millis(900);
const DEBOUNCE_BOX: Duration = Duration::from_millis(2500);
/// How many of the newest messages a scan looks at.
const SCAN_TAIL: usize = 24;
/// A message older than this when first seen is history, not a new link.
const FRESH_MS: i64 = 15 * 60 * 1000;
/// Concurrent unfurls across the relay.
const WORKERS: usize = 4;
/// Per-host fetch cap per minute.
const HOST_PER_MINUTE: usize = 60;
const USER_AGENT: &str = "Mozilla/5.0 (compatible; ArbosBot/1.0; +https://arbos.life/bot)";
const TITLE_MAX: usize = 200;
const DESCRIPTION_MAX: usize = 300;
const POST_TEXT_MAX: usize = 600;

// ---- switches -----------------------------------------------------------------------------------

/// `ARBOS_LINK_PREVIEWS=off` turns the whole thing off without a deploy.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        !matches!(
            std::env::var("ARBOS_LINK_PREVIEWS")
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "off" | "0" | "false" | "no"
        )
    })
}

fn denied_hosts() -> &'static HashSet<String> {
    static DENY: OnceLock<HashSet<String>> = OnceLock::new();
    DENY.get_or_init(|| {
        std::env::var("ARBOS_LINK_PREVIEW_DENY")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().trim_start_matches("www.").to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect()
    })
}

fn host_denied(host: &str) -> bool {
    let h = host.trim_start_matches("www.").to_ascii_lowercase();
    denied_hosts()
        .iter()
        .any(|d| h == *d || h.ends_with(&format!(".{d}")))
}

// ---- finding the URL in a message -----------------------------------------------------------

/// The first bare `http(s)://` URL in `text`: not the target of a markdown link
/// (`[label](url)`), not inside an inline code span or a fenced block, with trailing
/// punctuation and an unbalanced closing bracket trimmed. `None` when the message has none.
pub fn first_bare_url(text: &str) -> Option<String> {
    let mut in_fence = false;
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || t.starts_with("    ") {
            continue;
        }
        let bytes = line.as_bytes();
        let mut from = 0usize;
        while let Some(i) = line[from..].find("http") {
            let at = from + i;
            let rest = &line[at..];
            let scheme_len = if rest.starts_with("https://") {
                8
            } else if rest.starts_with("http://") {
                7
            } else {
                from = at + 4;
                continue;
            };
            // Part of a bigger word (e.g. `xhttp://`) is not a link.
            if at > 0 && bytes[at - 1].is_ascii_alphanumeric() {
                from = at + scheme_len;
                continue;
            }
            // `](http…` is a markdown link target; `` `http… `` inside a code span.
            let before = &line[..at];
            if before.trim_end().ends_with("](") || before.matches('`').count() % 2 == 1 {
                from = at + scheme_len;
                continue;
            }
            let end = rest
                .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | '`'))
                .unwrap_or(rest.len());
            let mut cand = &rest[..end];
            while let Some(last) = cand.chars().last() {
                let trim = match last {
                    '.' | ',' | ';' | ':' | '!' | '?' | '*' | '_' | '~' => true,
                    ')' => cand.matches('(').count() < cand.matches(')').count(),
                    ']' => cand.matches('[').count() < cand.matches(']').count(),
                    _ => false,
                };
                if !trim {
                    break;
                }
                cand = &cand[..cand.len() - last.len_utf8()];
            }
            if let Ok(u) = Url::parse(cand) {
                if let Some(h) = u.host_str() {
                    if h.contains('.') {
                        return Some(cand.to_owned());
                    }
                }
            }
            from = at + end.max(scheme_len);
        }
    }
    None
}

/// Tracking parameters dropped from every canonical URL.
fn is_tracking_param(key: &str, host: &str) -> bool {
    let k = key.to_ascii_lowercase();
    if k.starts_with("utm_")
        || matches!(
            k.as_str(),
            "si" | "fbclid"
                | "igshid"
                | "igsh"
                | "gclid"
                | "dclid"
                | "msclkid"
                | "mc_cid"
                | "mc_eid"
                | "ref_src"
                | "ref_url"
                | "_ga"
                | "_gl"
                | "yclid"
                | "feature"
                | "pp"
                | "nd"
                | "context"
        )
    {
        return true;
    }
    (host == "x.com" || host == "twitter.com") && matches!(k.as_str(), "s" | "t")
}

/// The URL the preview is keyed and shown by: lower-case host, `twitter.com` → `x.com`,
/// `youtu.be` / Shorts / Music → `www.youtube.com/watch?v=`, Spotify's `intl-xx` segment and
/// every tracking parameter dropped, fragment removed.
pub fn canonical(raw: &str) -> Option<Url> {
    let mut u = Url::parse(raw.trim()).ok()?;
    if u.scheme() != "http" && u.scheme() != "https" {
        return None;
    }
    let mut host = u.host_str()?.to_ascii_lowercase();
    if host == "twitter.com" || host == "mobile.twitter.com" || host == "www.twitter.com" {
        host = "x.com".into();
    }
    if host == "www.x.com" {
        host = "x.com".into();
    }
    if host == "m.youtube.com" || host == "youtube.com" {
        host = "www.youtube.com".into();
    }
    // YouTube's short, Shorts and Music forms all become the plain watch URL.
    let mut path = u.path().to_owned();
    let mut video: Option<String> = None;
    if host == "youtu.be" {
        video = path
            .trim_start_matches('/')
            .split('/')
            .next()
            .map(str::to_owned);
    } else if host == "www.youtube.com" || host == "music.youtube.com" {
        if let Some(id) = path
            .strip_prefix("/shorts/")
            .or_else(|| path.strip_prefix("/live/"))
        {
            video = id.split('/').next().map(str::to_owned);
        } else if path == "/watch" {
            video = u
                .query_pairs()
                .find(|(k, _)| k == "v")
                .map(|(_, v)| v.into_owned());
        }
    }
    if let Some(id) = video.filter(|v| !v.is_empty()) {
        host = "www.youtube.com".into();
        path = "/watch".into();
        let keep: Vec<(String, String)> = u
            .query_pairs()
            .filter(|(k, _)| k == "t" || k == "list")
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        u.set_query(None);
        {
            let mut q = u.query_pairs_mut();
            q.append_pair("v", &id);
            for (k, v) in keep {
                q.append_pair(&k, &v);
            }
        }
    }
    if host == "open.spotify.com" {
        if let Some(rest) = path.strip_prefix("/intl-") {
            if let Some((_, tail)) = rest.split_once('/') {
                path = format!("/{tail}");
            }
        }
    }
    u.set_host(Some(&host)).ok()?;
    u.set_path(&path);
    u.set_fragment(None);
    let kept: Vec<(String, String)> = u
        .query_pairs()
        .filter(|(k, _)| !is_tracking_param(k, &host))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    u.set_query(None);
    if !kept.is_empty() {
        let mut q = u.query_pairs_mut();
        for (k, v) in kept {
            q.append_pair(&k, &v);
        }
    }
    if u.port() == Some(80) && u.scheme() == "http"
        || u.port() == Some(443) && u.scheme() == "https"
    {
        let _ = u.set_port(None);
    }
    Some(u)
}

// ---- the fetcher ----------------------------------------------------------------------------

/// One bounded answer.
pub struct Fetched {
    pub final_url: String,
    pub status: u16,
    pub content_type: String,
    pub content_length: Option<u64>,
    pub body: Bytes,
}

/// Outbound HTTP with the safety rules in the module doc. `allow` names one host (`host:port`)
/// that is fetched as-is — the tests' local server — and nothing else bypasses the checks.
pub struct Fetcher {
    deadline: Instant,
    allow: Option<String>,
}

impl Fetcher {
    pub fn new() -> Self {
        Self {
            deadline: Instant::now() + TOTAL_BUDGET,
            allow: None,
        }
    }

    #[cfg(test)]
    pub fn allowing(host_port: &str) -> Self {
        Self {
            deadline: Instant::now() + TOTAL_BUDGET,
            allow: Some(host_port.to_owned()),
        }
    }

    fn remaining(&self) -> Option<Duration> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        (left > Duration::from_millis(200)).then_some(left)
    }

    /// Resolve `host` and refuse it unless every address is public; the addresses are what
    /// the request connects to (DNS pinning: no second lookup can swap in a private one).
    async fn pin(&self, url: &Url) -> Option<Vec<SocketAddr>> {
        let host = url.host_str()?;
        let port = url.port_or_known_default()?;
        if let Some(a) = &self.allow {
            if format!("{host}:{port}") == *a || host == a.split(':').next().unwrap_or("") {
                return Some(Vec::new());
            }
        }
        public_domain(host)?;
        if host_denied(host) {
            return None;
        }
        if !host_rate_ok(host) {
            return None;
        }
        let addrs: Vec<SocketAddr> =
            tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::lookup_host((host, port)))
                .await
                .ok()?
                .ok()?
                .collect();
        if addrs.is_empty() || addrs.iter().any(|a| is_private_ip(a.ip())) {
            return None;
        }
        Some(addrs)
    }

    /// GET `url` with `accept`, following at most [`MAX_REDIRECTS`] redirects (each pinned and
    /// checked again), reading at most `max` body bytes (`max == 0` = headers only; a page
    /// over the cap is cut, a picture over it refused).
    pub async fn get(&self, url: &str, accept: &str, max: usize) -> Option<Fetched> {
        let mut url = Url::parse(url).ok()?;
        for _ in 0..=MAX_REDIRECTS {
            if url.scheme() != "http" && url.scheme() != "https" {
                return None;
            }
            let left = self.remaining()?;
            let addrs = self.pin(&url).await?;
            let mut b = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(CONNECT_TIMEOUT.min(left))
                .timeout(left)
                .user_agent(USER_AGENT)
                .no_proxy();
            if !addrs.is_empty() {
                b = b.resolve_to_addrs(url.host_str()?, &addrs);
            }
            let client = b.build().ok()?;
            let resp = client
                .get(url.clone())
                .header(header::ACCEPT, accept)
                .header(header::ACCEPT_LANGUAGE, "en")
                .send()
                .await
                .ok()?;
            let status = resp.status();
            if status.is_redirection() {
                let next = resp.headers().get(header::LOCATION)?.to_str().ok()?;
                url = Url::parse(&absolute(url.as_str(), next)?).ok()?;
                continue;
            }
            let content_type = resp
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_ascii_lowercase();
            let content_length = resp.content_length();
            if max == 0 {
                return Some(Fetched {
                    final_url: url.to_string(),
                    status: status.as_u16(),
                    content_type,
                    content_length,
                    body: Bytes::new(),
                });
            }
            if content_length.is_some_and(|l| l as usize > max) && max != PAGE_MAX_BYTES {
                return None;
            }
            let mut body = Vec::new();
            let mut stream = resp;
            while let Ok(Ok(Some(chunk))) =
                tokio::time::timeout(self.remaining()?, stream.chunk()).await
            {
                if body.len() + chunk.len() > max {
                    if max == PAGE_MAX_BYTES {
                        body.extend_from_slice(&chunk[..max - body.len()]);
                        break;
                    }
                    return None;
                }
                body.extend_from_slice(&chunk);
            }
            return Some(Fetched {
                final_url: url.to_string(),
                status: status.as_u16(),
                content_type,
                content_length,
                body: Bytes::from(body),
            });
        }
        None
    }

    async fn html(&self, url: &str) -> Option<(String, String)> {
        let f = self
            .get(
                url,
                "text/html,application/xhtml+xml;q=0.9,*/*;q=0.5",
                PAGE_MAX_BYTES,
            )
            .await?;
        if !(200..300).contains(&f.status) {
            return None;
        }
        if !f.content_type.is_empty()
            && !f.content_type.contains("html")
            && !f.content_type.contains("xml")
        {
            return None;
        }
        Some((f.final_url, String::from_utf8_lossy(&f.body).into_owned()))
    }

    async fn json(&self, url: &str) -> Option<Value> {
        let f = self.get(url, "application/json", PAGE_MAX_BYTES).await?;
        if !(200..300).contains(&f.status) {
            return None;
        }
        serde_json::from_slice(&f.body).ok()
    }
}

/// One bounded, pinned, checked GET for other relay modules (the shared link-icon resolver
/// fetches a host's icon through this; `accept` and `max` as in [`Fetcher::get`]).
#[allow(dead_code)] // kept for other relay modules; the site-icon resolver drives a `Fetcher` of its own
pub async fn fetch_bounded(url: &str, accept: &str, max: usize) -> Option<Fetched> {
    Fetcher::new().get(url, accept, max).await
}

fn host_rate_ok(host: &str) -> bool {
    static HITS: OnceLock<Mutex<HashMap<String, VecDeque<i64>>>> = OnceLock::new();
    let now = now_ms();
    let mut map = HITS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap();
    let q = map.entry(host.to_owned()).or_default();
    while q.front().is_some_and(|t| now - t > 60_000) {
        q.pop_front();
    }
    if q.len() >= HOST_PER_MINUTE {
        return false;
    }
    q.push_back(now);
    true
}

// ---- what a preview is ------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preview {
    /// `spotify` | `youtube` | `x` | `github` | `bluesky` | `vimeo` | `loom` | `soundcloud` |
    /// `apple_music` | `google_maps` | `wikipedia` | `arbos` | `file` | `web`.
    pub provider: String,
    /// What the footer says: "Spotify", "YouTube", "GitHub", the site name, the host.
    pub provider_name: String,
    /// `song` `album` `playlist` `podcast` `artist` `video` `post` `pull_request` `issue` `repo`
    /// `gist` `article` `image` `file` `place` `page`.
    pub kind: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Short pre-formatted footer facts in order ("Song", "4:03", "opened 4 days ago").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<String>,
    /// `open` `merged` `closed` `draft` for GitHub items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// `{additions, deletions, files}` for a pull request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<Value>,
    /// "18:24" drawn on a video thumbnail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<String>,
    /// The picture to fetch, and whether it is square art (kept small) or media (16:9 on top).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    #[serde(default)]
    pub art: bool,
}

impl Preview {
    fn new(provider: &str, provider_name: &str, kind: &str, title: &str) -> Self {
        Self {
            provider: provider.into(),
            provider_name: provider_name.into(),
            kind: kind.into(),
            title: clip(title, TITLE_MAX),
            subtitle: None,
            description: None,
            facts: Vec::new(),
            state: None,
            diff: None,
            duration: None,
            image_url: None,
            art: false,
        }
    }
}

/// A re-encoded picture ready for the chat's blob slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Picture {
    pub mime: String,
    pub width: u32,
    pub height: u32,
    #[serde(skip)]
    pub bytes: Bytes,
}

/// A finished unfurl: what the card shows plus its picture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unfurled {
    pub canonical_url: String,
    pub preview: Preview,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture: Option<Picture>,
}

impl Unfurled {
    /// `compact` or `expanded`: media on top only for a landscape picture wide enough to be
    /// one, on kinds that read as media.
    pub fn layout(&self) -> &'static str {
        match &self.picture {
            Some(p)
                if !self.preview.art
                    && p.width >= 600
                    && p.width as f32 >= p.height as f32 * 1.2
                    && matches!(
                        self.preview.kind.as_str(),
                        "video" | "article" | "post" | "place" | "image" | "page"
                    ) =>
            {
                "expanded"
            }
            _ => "compact",
        }
    }

    /// The part written on the message. `image_ref` is the blob key in the chat's slot;
    /// `site_icon` is the `{ref, width, height, mime}` object the shared link-icon resolver
    /// provides once it lands (absent until then — clients fall back to a brand mark / neutral
    /// tile by `siteHost`).
    pub fn part_json(&self, url: &str, image_ref: Option<&str>, site_icon: Option<Value>) -> Value {
        let p = &self.preview;
        let mut v = json!({
            "id": ulid(),
            "kind": PART_KIND,
            "specVersion": SPEC_VERSION,
            "url": url,
            "canonicalUrl": self.canonical_url,
            "provider": p.provider,
            "providerName": p.provider_name,
            "type": p.kind,
            "title": p.title,
            "layout": self.layout(),
            "fetchedAt": now_ms(),
        });
        // `siteHost` lets a client pick a bundled brand mark; `siteIcon` (a blob in the chat's
        // slot, `{ref, width, height, mime}`) is reserved for the shared link-icon tile and is
        // not written by this release.
        if let Some(h) = Url::parse(&self.canonical_url).ok().map(|u| host_of(&u)) {
            v["siteHost"] = json!(h);
        }
        if let Some(icon) = site_icon {
            v["siteIcon"] = icon;
        }
        let o = v.as_object_mut().unwrap();
        if let Some(s) = &p.subtitle {
            o.insert("subtitle".into(), json!(s));
        }
        if let Some(s) = &p.description {
            o.insert("description".into(), json!(s));
        }
        if !p.facts.is_empty() {
            o.insert("facts".into(), json!(p.facts));
        }
        if let Some(s) = &p.state {
            o.insert("state".into(), json!(s));
        }
        if let Some(d) = &p.diff {
            o.insert("diff".into(), d.clone());
        }
        if let Some(s) = &p.duration {
            o.insert("duration".into(), json!(s));
        }
        if let (Some(pic), Some(r)) = (&self.picture, image_ref) {
            o.insert(
                "image".into(),
                json!({ "ref": r, "width": pic.width, "height": pic.height, "mime": pic.mime, "art": p.art }),
            );
        }
        v
    }
}

// ---- text helpers ---------------------------------------------------------------------------

/// `&amp;`, `&#39;`, `&#x27;` and the common named entities, decoded.
pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let Some(end) = tail.find(';').filter(|e| *e <= 10) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let ent = &tail[1..end];
        let decoded = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            "mdash" => Some('—'),
            "ndash" => Some('–'),
            "hellip" => Some('…'),
            "rsquo" => Some('’'),
            "lsquo" => Some('‘'),
            "rdquo" => Some('”'),
            "ldquo" => Some('“'),
            "middot" => Some('·'),
            "copy" => Some('©'),
            _ => ent
                .strip_prefix('#')
                .and_then(|n| {
                    if let Some(h) = n.strip_prefix(['x', 'X']) {
                        u32::from_str_radix(h, 16).ok()
                    } else {
                        n.parse::<u32>().ok()
                    }
                })
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Tags removed, `<br>` and block ends as newlines, entities decoded, whitespace tidied.
pub fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let Some(end) = tail.find('>') else {
            break;
        };
        let tag = tail[1..end].trim().to_ascii_lowercase();
        if tag.starts_with("br") || tag.starts_with("/p") || tag.starts_with("/div") {
            out.push('\n');
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    let text = decode_entities(&out);
    let lines: Vec<String> = text
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    let mut joined = String::new();
    let mut blank = 0;
    for l in lines {
        if l.is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        if !joined.is_empty() {
            joined.push('\n');
        }
        joined.push_str(&l);
    }
    joined.trim().to_owned()
}

fn clip(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

/// `<title>…</title>`, decoded.
pub fn html_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let open_end = lower[start..].find('>')? + start + 1;
    let close = lower[open_end..].find("</title>")? + open_end;
    non_empty(Some(strip_tags(&html[open_end..close])))
}

/// `content` of `<meta itemprop="<name>">`.
fn itemprop(html: &str, name: &str) -> Option<String> {
    for tag in tags(html, "meta") {
        if attr(&tag, "itemprop").is_some_and(|v| v.eq_ignore_ascii_case(name)) {
            if let Some(c) = attr(&tag, "content") {
                if !c.trim().is_empty() {
                    return Some(c.trim().to_owned());
                }
            }
        }
    }
    None
}

/// "PT18M24S" / seconds → "18:24" / "1:02:03".
pub fn fmt_duration_secs(total: u64) -> String {
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

pub fn parse_iso8601_duration(s: &str) -> Option<u64> {
    let s = s.trim().strip_prefix("PT")?;
    let mut total = 0u64;
    let mut num = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            num.push(c);
        } else {
            let n: u64 = num.parse().ok()?;
            num.clear();
            total += match c {
                'H' => n * 3600,
                'M' => n * 60,
                'S' => n,
                _ => return None,
            };
        }
    }
    (total > 0).then_some(total)
}

/// 1234 → "1.2K", 1_250_000 → "1.3M".
pub fn fmt_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0).replace(".0M", "M")
    } else if n >= 10_000 {
        format!("{}K", n / 1000)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1000.0).replace(".0K", "K")
    } else {
        n.to_string()
    }
}

/// "2026-10-23T18:21:18Z" → "4 days ago" / "3 hours ago" / "just now".
pub fn fmt_age(iso: &str, now: i64) -> Option<String> {
    let t = chrono::DateTime::parse_from_rfc3339(iso)
        .ok()?
        .timestamp_millis();
    let d = (now - t).max(0) / 1000;
    Some(match d {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", d / 60),
        3600..=86_399 => {
            let h = d / 3600;
            format!("{h} hour{} ago", if h == 1 { "" } else { "s" })
        }
        86_400..=2_591_999 => {
            let days = d / 86_400;
            format!("{days} day{} ago", if days == 1 { "" } else { "s" })
        }
        _ => {
            let months = d / 2_592_000;
            if months < 12 {
                format!("{months} month{} ago", if months == 1 { "" } else { "s" })
            } else {
                let y = months / 12;
                format!("{y} year{} ago", if y == 1 { "" } else { "s" })
            }
        }
    })
}

fn host_of(url: &Url) -> String {
    url.host_str()
        .unwrap_or("")
        .trim_start_matches("www.")
        .to_owned()
}

fn bytes_label(n: u64) -> String {
    if n >= 1 << 30 {
        format!("{:.1} GB", n as f64 / (1u64 << 30) as f64)
    } else if n >= 1 << 20 {
        format!("{:.1} MB", n as f64 / (1u64 << 20) as f64)
    } else if n >= 1 << 10 {
        format!("{} KB", n >> 10)
    } else {
        format!("{n} B")
    }
}

// ---- the generic Open Graph reading ----------------------------------------------------------

/// Everything a page's `<head>` says about itself.
#[derive(Debug, Default, Clone)]
pub struct PageMeta {
    pub title: Option<String>,
    pub description: Option<String>,
    pub site_name: Option<String>,
    pub image: Option<String>,
    pub og_type: Option<String>,
    pub duration_secs: Option<u64>,
}

pub fn read_page(final_url: &str, html: &str) -> PageMeta {
    let m = |p: &str| non_empty(meta_content(html, p).map(|s| decode_entities(&s)));
    let image = m("og:image")
        .or_else(|| m("og:image:url"))
        .or_else(|| m("og:image:secure_url"))
        .or_else(|| m("twitter:image"))
        .or_else(|| m("twitter:image:src"))
        .and_then(|i| absolute(final_url, &i));
    PageMeta {
        title: m("og:title")
            .or_else(|| m("twitter:title"))
            .or_else(|| html_title(html)),
        description: m("og:description")
            .or_else(|| m("twitter:description"))
            .or_else(|| m("description")),
        site_name: m("og:site_name").or_else(|| m("application-name")),
        image,
        og_type: m("og:type").map(|t| t.to_ascii_lowercase()),
        duration_secs: m("video:duration")
            .or_else(|| m("music:duration"))
            .and_then(|d| d.parse().ok())
            .or_else(|| itemprop(html, "duration").and_then(|d| parse_iso8601_duration(&d))),
    }
}

/// The generic card: a title is required; everything else is optional.
fn generic_preview(
    url: &Url,
    meta: &PageMeta,
    provider: &str,
    provider_name: Option<&str>,
) -> Option<Preview> {
    let title = meta.title.clone()?;
    let site = provider_name
        .map(str::to_owned)
        .or_else(|| meta.site_name.clone())
        .unwrap_or_else(|| host_of(url));
    let kind = match meta.og_type.as_deref() {
        Some(t) if t.starts_with("video") => "video",
        Some("article") | Some("blog") => "article",
        Some(t) if t.starts_with("music") => "song",
        _ => "page",
    };
    let mut p = Preview::new(provider, &site, kind, &title);
    p.description = meta
        .description
        .as_deref()
        .map(|d| clip(d, DESCRIPTION_MAX));
    p.image_url = meta.image.clone();
    p.duration = meta.duration_secs.map(fmt_duration_secs);
    if p.provider != "web" {
        p.facts.push(host_of(url));
    }
    Some(p)
}

// ---- providers ----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Spotify,
    YouTube,
    X,
    GitHub,
    Bluesky,
    Vimeo,
    Loom,
    SoundCloud,
    AppleMusic,
    GoogleMaps,
    Wikipedia,
    Arbos,
    Web,
}

/// Which parser the canonical URL gets. Instagram, Threads, TikTok and Linear have no special
/// case on purpose (Jacob, 2026-10-27): they fall through to the generic reading and, behind
/// their login walls, to the plain link.
pub fn provider_of(url: &Url) -> Provider {
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    let h = host.trim_start_matches("www.");
    match h {
        "open.spotify.com" => Provider::Spotify,
        "youtube.com" | "music.youtube.com" | "youtu.be" => Provider::YouTube,
        "x.com" => Provider::X,
        "github.com" | "gist.github.com" => Provider::GitHub,
        "bsky.app" => Provider::Bluesky,
        "vimeo.com" | "player.vimeo.com" => Provider::Vimeo,
        "loom.com" => Provider::Loom,
        "soundcloud.com" | "on.soundcloud.com" => Provider::SoundCloud,
        "music.apple.com" => Provider::AppleMusic,
        "maps.app.goo.gl" | "goo.gl" | "maps.google.com" => Provider::GoogleMaps,
        "arbos.life" => Provider::Arbos,
        _ if h.ends_with(".wikipedia.org") => Provider::Wikipedia,
        _ if (h == "google.com" || h.ends_with(".google.com"))
            && url.path().starts_with("/maps") =>
        {
            Provider::GoogleMaps
        }
        _ => Provider::Web,
    }
}

async fn oembed(fetch: &Fetcher, endpoint: &str, url: &Url) -> Option<Value> {
    let mut e = Url::parse(endpoint).ok()?;
    e.query_pairs_mut().append_pair("url", url.as_str());
    fetch.json(e.as_str()).await
}

fn jstr(v: &Value, k: &str) -> Option<String> {
    non_empty(v.get(k).and_then(Value::as_str).map(str::to_owned))
}
fn ju64(v: &Value, k: &str) -> Option<u64> {
    v.get(k).and_then(Value::as_u64)
}

async fn spotify(fetch: &Fetcher, url: &Url) -> Option<Preview> {
    let mut segs = url.path_segments()?.filter(|s| !s.is_empty());
    let what = segs.next()?.to_owned();
    let kind_label = match what.as_str() {
        "track" => ("song", "Song"),
        "album" => ("album", "Album"),
        "playlist" => ("playlist", "Playlist"),
        "episode" => ("podcast", "Episode"),
        "show" => ("podcast", "Podcast"),
        "artist" => ("artist", "Artist"),
        _ => return None,
    };
    let page = fetch.html(url.as_str()).await;
    let meta = page
        .as_ref()
        .map(|(f, h)| read_page(f, h))
        .unwrap_or_default();
    let (title, subtitle, image, duration) = if let Some(t) = meta.title.clone() {
        (
            t,
            meta.description.clone(),
            meta.image.clone(),
            meta.duration_secs,
        )
    } else {
        let o = oembed(fetch, "https://open.spotify.com/oembed", url).await?;
        (jstr(&o, "title")?, None, jstr(&o, "thumbnail_url"), None)
    };
    let mut p = Preview::new("spotify", "Spotify", kind_label.0, &title);
    // Spotify's description reads "M83 · Hurry Up, We're Dreaming · Song · 2011": keep the
    // artist and album as the subtitle and let the facts carry the kind.
    p.subtitle = subtitle.map(|d| {
        let keep: Vec<&str> = d
            .split(" · ")
            .filter(|s| {
                !matches!(
                    *s,
                    "Song" | "Album" | "Playlist" | "Episode" | "Podcast" | "Artist" | "Single"
                ) && s.parse::<u32>().is_err()
            })
            .collect();
        clip(&keep.join(" · "), 120)
    });
    p.facts.push(kind_label.1.into());
    if let Some(d) = duration {
        p.facts.push(fmt_duration_secs(d));
    }
    p.image_url = image;
    p.art = true;
    Some(p)
}

async fn youtube(fetch: &Fetcher, url: &Url) -> Option<Preview> {
    let id = url
        .query_pairs()
        .find(|(k, _)| k == "v")
        .map(|(_, v)| v.into_owned())
        .filter(|v| {
            v.len() >= 8
                && v.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })?;
    let o = oembed(fetch, "https://www.youtube.com/oembed?format=json", url).await;
    let page = fetch.html(url.as_str()).await;
    let meta = page
        .as_ref()
        .map(|(f, h)| read_page(f, h))
        .unwrap_or_default();
    let title = o
        .as_ref()
        .and_then(|o| jstr(o, "title"))
        .or_else(|| meta.title.clone())?;
    let channel = o
        .as_ref()
        .and_then(|o| jstr(o, "author_name"))
        .or_else(|| meta.site_name.clone());
    let mut p = Preview::new("youtube", "YouTube", "video", &title);
    p.subtitle = channel.clone();
    if let Some(c) = channel {
        p.facts.push(c);
    }
    p.duration = meta.duration_secs.map(fmt_duration_secs);
    p.image_url = Some(format!("https://i.ytimg.com/vi/{id}/maxresdefault.jpg"));
    Some(p)
}

/// X and Bluesky publish the post as an oEmbed blockquote: `<p>text</p>&mdash; Name (@handle)
/// <a>date</a>`. Returns (text, handle, date).
pub fn parse_blockquote(html: &str) -> (Option<String>, Option<String>, Option<String>) {
    let lower = html.to_ascii_lowercase();
    let text = lower.find("<p").and_then(|s| {
        let open_end = lower[s..].find('>')? + s + 1;
        let close = lower[open_end..].find("</p>")? + open_end;
        non_empty(Some(strip_tags(&html[open_end..close])))
    });
    let tail = lower.rfind("</p>").map(|i| &html[i + 4..]).unwrap_or(html);
    let tail_text = strip_tags(tail);
    let handle = tail_text
        .find("(@")
        .and_then(|i| tail_text[i + 1..].split(')').next())
        .map(str::to_owned)
        .or_else(|| {
            tail_text
                .split_whitespace()
                .find(|w| w.starts_with('@'))
                .map(|w| {
                    w.trim_matches(|c: char| {
                        !c.is_alphanumeric() && c != '@' && c != '.' && c != '_'
                    })
                    .to_owned()
                })
        });
    let date = {
        let lower_tail = tail.to_ascii_lowercase();
        lower_tail.rfind("<a").and_then(|a| {
            let open_end = lower_tail[a..].find('>')? + a + 1;
            let close = lower_tail[open_end..].find("</a>")? + open_end;
            non_empty(Some(strip_tags(&tail[open_end..close])))
        })
    };
    (text, handle, date)
}

async fn x_post(fetch: &Fetcher, url: &Url) -> Option<Preview> {
    if !url.path().contains("/status/") {
        return None;
    }
    let o = oembed(
        fetch,
        "https://publish.twitter.com/oembed?omit_script=1&dnt=1&hide_thread=1",
        url,
    )
    .await?;
    let name = jstr(&o, "author_name")?;
    let (text, handle, date) = parse_blockquote(&jstr(&o, "html").unwrap_or_default());
    let handle = handle.or_else(|| {
        jstr(&o, "author_url").and_then(|u| u.rsplit('/').next().map(|h| format!("@{h}")))
    });
    let mut p = Preview::new("x", "X", "post", &name);
    p.subtitle = handle;
    p.description = text.map(|t| clip(&t, POST_TEXT_MAX));
    if let Some(d) = date {
        p.facts.push(d);
    }
    Some(p)
}

async fn bluesky(fetch: &Fetcher, url: &Url) -> Option<Preview> {
    if !url.path().contains("/post/") {
        return None;
    }
    let o = oembed(fetch, "https://embed.bsky.app/oembed", url).await?;
    let name = jstr(&o, "author_name")?;
    let (text, handle, date) = parse_blockquote(&jstr(&o, "html").unwrap_or_default());
    let mut p = Preview::new("bluesky", "Bluesky", "post", &name);
    p.subtitle = handle.or_else(|| {
        url.path_segments()
            .and_then(|mut s| s.nth(1).map(|h| format!("@{h}")))
    });
    p.description = text.map(|t| clip(&t, POST_TEXT_MAX));
    if let Some(d) = date {
        p.facts.push(d);
    }
    Some(p)
}

async fn github(fetch: &Fetcher, url: &Url) -> Option<Preview> {
    let host = url.host_str()?.trim_start_matches("www.");
    let segs: Vec<String> = url
        .path_segments()?
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let now = now_ms();
    let api = |path: String| async move {
        let full = format!("https://api.github.com{path}");
        fetch.json(&full).await
    };
    if host == "gist.github.com" {
        let id = segs.last()?.clone();
        let g = api(format!("/gists/{id}")).await?;
        let files = g.get("files").and_then(Value::as_object);
        let first = files.and_then(|f| f.keys().next().cloned());
        let title = jstr(&g, "description").or(first.clone())?;
        let owner = g
            .get("owner")
            .and_then(|o| jstr(o, "login"))
            .unwrap_or_default();
        let mut p = Preview::new("github", "GitHub", "gist", &title);
        p.subtitle = Some(format!("{owner} · gist"));
        if let Some(n) = files.map(|f| f.len()) {
            p.facts
                .push(format!("{n} file{}", if n == 1 { "" } else { "s" }));
        }
        if let Some(lang) = files
            .and_then(|f| f.values().next())
            .and_then(|f| jstr(f, "language"))
        {
            p.facts.push(lang);
        }
        return Some(p);
    }
    let (owner, repo) = (segs.first()?.clone(), segs.get(1)?.clone());
    let full = format!("{owner}/{repo}");
    match (segs.get(2).map(String::as_str), segs.get(3)) {
        (Some("pull"), Some(n)) => {
            let n: u64 = n.parse().ok()?;
            let pr = api(format!("/repos/{owner}/{repo}/pulls/{n}")).await?;
            let title = jstr(&pr, "title")?;
            let user = pr
                .get("user")
                .and_then(|u| jstr(u, "login"))
                .unwrap_or_default();
            let merged = pr.get("merged").and_then(Value::as_bool).unwrap_or(false);
            let draft = pr.get("draft").and_then(Value::as_bool).unwrap_or(false);
            let state = if merged {
                "merged"
            } else if draft {
                "draft"
            } else if jstr(&pr, "state").as_deref() == Some("open") {
                "open"
            } else {
                "closed"
            };
            let commits = ju64(&pr, "commits").unwrap_or(0);
            let base = pr
                .get("base")
                .and_then(|b| jstr(b, "ref"))
                .unwrap_or_else(|| "main".into());
            let mut p = Preview::new("github", "GitHub", "pull_request", &title);
            p.subtitle = Some(format!("{full} · #{n}"));
            p.description = Some(match state {
                "merged" => format!(
                    "{} merged {commits} commit{} into {base}",
                    pr.get("merged_by")
                        .and_then(|u| jstr(u, "login"))
                        .unwrap_or(user.clone()),
                    if commits == 1 { "" } else { "s" }
                ),
                _ => format!(
                    "{user} wants to merge {commits} commit{} into {base}",
                    if commits == 1 { "" } else { "s" }
                ),
            });
            p.state = Some(state.into());
            let (a, d, f) = (
                ju64(&pr, "additions").unwrap_or(0),
                ju64(&pr, "deletions").unwrap_or(0),
                ju64(&pr, "changed_files").unwrap_or(0),
            );
            p.diff = Some(json!({ "additions": a, "deletions": d, "files": f }));
            p.facts
                .push(format!("{f} file{}", if f == 1 { "" } else { "s" }));
            if let Some(age) = jstr(&pr, "created_at").and_then(|c| fmt_age(&c, now)) {
                p.facts.push(format!("opened {age}"));
            }
            Some(p)
        }
        (Some("issues"), Some(n)) => {
            let n: u64 = n.parse().ok()?;
            let is = api(format!("/repos/{owner}/{repo}/issues/{n}")).await?;
            let title = jstr(&is, "title")?;
            let user = is
                .get("user")
                .and_then(|u| jstr(u, "login"))
                .unwrap_or_default();
            let state = if jstr(&is, "state").as_deref() == Some("open") {
                "open"
            } else {
                "closed"
            };
            let mut p = Preview::new("github", "GitHub", "issue", &title);
            p.subtitle = Some(format!("{full} · #{n}"));
            p.description = jstr(&is, "body")
                .map(|b| clip(&strip_tags(&b).replace('\n', " "), DESCRIPTION_MAX))
                .or_else(|| Some(format!("opened by {user}")));
            p.state = Some(state.into());
            let c = ju64(&is, "comments").unwrap_or(0);
            p.facts
                .push(format!("{c} comment{}", if c == 1 { "" } else { "s" }));
            if let Some(age) = jstr(&is, "created_at").and_then(|c| fmt_age(&c, now)) {
                p.facts.push(format!("opened {age}"));
            }
            Some(p)
        }
        (None, _) => {
            let r = api(format!("/repos/{owner}/{repo}")).await?;
            let title = jstr(&r, "full_name").unwrap_or(full.clone());
            let mut p = Preview::new("github", "GitHub", "repo", &title);
            p.description = jstr(&r, "description").map(|d| clip(&d, DESCRIPTION_MAX));
            if let Some(l) = jstr(&r, "language") {
                p.facts.push(l);
            }
            if let Some(s) = ju64(&r, "stargazers_count") {
                p.facts.push(format!("★ {}", fmt_count(s)));
            }
            if let Some(f) = ju64(&r, "forks_count") {
                p.facts.push(format!("{} forks", fmt_count(f)));
            }
            Some(p)
        }
        _ => None,
    }
}

async fn simple_oembed(
    fetch: &Fetcher,
    endpoint: &str,
    url: &Url,
    provider: &str,
    name: &str,
    kind: &str,
    art: bool,
) -> Option<Preview> {
    let o = oembed(fetch, endpoint, url).await?;
    let title = jstr(&o, "title")?;
    let mut p = Preview::new(provider, name, kind, &title);
    p.subtitle = jstr(&o, "author_name");
    if let Some(a) = p.subtitle.clone() {
        p.facts.push(a);
    }
    if let Some(d) = ju64(&o, "duration") {
        p.duration = Some(fmt_duration_secs(d));
    }
    p.image_url = jstr(&o, "thumbnail_url");
    p.art = art;
    Some(p)
}

async fn apple_music(fetch: &Fetcher, url: &Url) -> Option<Preview> {
    let (f, h) = fetch.html(url.as_str()).await?;
    let meta = read_page(&f, &h);
    let title = meta.title?;
    let kind = if url.query_pairs().any(|(k, _)| k == "i") {
        ("song", "Song")
    } else if url.path().contains("/album/") {
        ("album", "Album")
    } else if url.path().contains("/playlist/") {
        ("playlist", "Playlist")
    } else {
        ("song", "Music")
    };
    let title = title
        .split(" - Apple Music")
        .next()
        .unwrap_or(&title)
        .to_owned();
    let mut p = Preview::new("apple_music", "Apple Music", kind.0, &title);
    p.subtitle = meta.description.map(|d| clip(&d, 120));
    p.facts.push(kind.1.into());
    p.image_url = meta.image;
    p.art = true;
    Some(p)
}

async fn wikipedia(fetch: &Fetcher, url: &Url) -> Option<Preview> {
    let host = url.host_str()?;
    let title = url
        .path()
        .strip_prefix("/wiki/")?
        .split('/')
        .next()?
        .to_owned();
    if title.is_empty() {
        return None;
    }
    let s = fetch
        .json(&format!("https://{host}/api/rest_v1/page/summary/{title}"))
        .await?;
    let t = jstr(&s, "title")?;
    let mut p = Preview::new("wikipedia", "Wikipedia", "article", &t);
    p.subtitle = jstr(&s, "description");
    p.description = jstr(&s, "extract").map(|e| clip(&e, DESCRIPTION_MAX));
    p.image_url = s
        .get("thumbnail")
        .and_then(|th| jstr(th, "source"))
        .or_else(|| s.get("originalimage").and_then(|th| jstr(th, "source")));
    Some(p)
}

/// A direct image, video or PDF URL: the card is the file itself.
async fn direct_file(fetch: &Fetcher, url: &Url, head: &Fetched) -> Option<Preview> {
    let name = url
        .path_segments()
        .and_then(|mut s| s.rfind(|x: &&str| !x.is_empty()))
        .map(percent_decode)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| host_of(url));
    let ct = head
        .content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_owned();
    let size = head.content_length.map(bytes_label);
    let mut p = if ct.starts_with("image/") {
        let mut p = Preview::new("file", &host_of(url), "image", &name);
        p.image_url = Some(url.to_string());
        p.facts.push("Image".into());
        p
    } else if ct.starts_with("video/") {
        let mut p = Preview::new("file", &host_of(url), "file", &name);
        p.facts.push("Video".into());
        p
    } else if ct == "application/pdf" {
        let mut p = Preview::new("file", &host_of(url), "file", &name);
        p.facts.push("PDF".into());
        p
    } else {
        return None;
    };
    let _ = fetch;
    if let Some(s) = size {
        p.facts.push(s);
    }
    Some(p)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The whole reading for one canonical URL: provider parser, then the generic page, then
/// a direct file. `None` = no card.
pub async fn resolve(fetch: &Fetcher, url: &Url) -> Option<Preview> {
    let provider = provider_of(url);
    let special = match provider {
        Provider::Spotify => spotify(fetch, url).await,
        Provider::YouTube => youtube(fetch, url).await,
        Provider::X => x_post(fetch, url).await,
        Provider::GitHub => github(fetch, url).await,
        Provider::Bluesky => bluesky(fetch, url).await,
        Provider::Vimeo => {
            simple_oembed(
                fetch,
                "https://vimeo.com/api/oembed.json",
                url,
                "vimeo",
                "Vimeo",
                "video",
                false,
            )
            .await
        }
        Provider::Loom => {
            simple_oembed(
                fetch,
                "https://www.loom.com/v1/oembed",
                url,
                "loom",
                "Loom",
                "video",
                false,
            )
            .await
        }
        Provider::SoundCloud => {
            simple_oembed(
                fetch,
                "https://soundcloud.com/oembed?format=json",
                url,
                "soundcloud",
                "SoundCloud",
                "song",
                true,
            )
            .await
        }
        Provider::AppleMusic => apple_music(fetch, url).await,
        Provider::Wikipedia => wikipedia(fetch, url).await,
        Provider::GoogleMaps | Provider::Arbos | Provider::Web => None,
    };
    if let Some(p) = special {
        return Some(p);
    }
    // X without its oEmbed, a private GitHub repo, a Spotify wall: nothing generic would be
    // right, so no card rather than a half one.
    if matches!(provider, Provider::X | Provider::Bluesky | Provider::GitHub) {
        return None;
    }
    let head = fetch.get(url.as_str(), "*/*", 0).await?;
    if !(200..300).contains(&head.status) {
        return None;
    }
    let ct = head
        .content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_owned();
    if !(ct.is_empty() || ct.contains("html") || ct.contains("xml")) {
        return direct_file(fetch, url, &head).await;
    }
    let (final_url, html) = fetch.html(url.as_str()).await?;
    let meta = read_page(&final_url, &html);
    let (pid, pname) = match provider {
        Provider::GoogleMaps => ("google_maps", Some("Google Maps")),
        Provider::Arbos => ("arbos", Some("Arbos")),
        _ => ("web", None),
    };
    let mut p = generic_preview(url, &meta, pid, pname)?;
    if provider == Provider::GoogleMaps {
        p.kind = "place".into();
    }
    Some(p)
}

// ---- pictures -------------------------------------------------------------------------------

/// Decode, bound, re-encode: at most `max_side` px on the long edge, JPEG under
/// [`IMAGE_MAX_OUT_BYTES`] (quality then size stepped down). Pictures under 48 px are dropped.
pub fn process_picture(bytes: &[u8], art: bool) -> Option<Picture> {
    let img = crate::image_sniff::decode(bytes).ok()?;
    let (w, h) = (img.width(), img.height());
    if w < 48 || h < 48 {
        return None;
    }
    let max_side = if art { ART_MAX_SIDE } else { IMAGE_MAX_SIDE };
    let mut img = if w > max_side || h > max_side {
        img.resize(max_side, max_side, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };
    let mut quality = 82u8;
    loop {
        let rgb = img.to_rgb8();
        let mut out = Cursor::new(Vec::new());
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
        enc.encode_image(&rgb).ok()?;
        let data = out.into_inner();
        if data.len() <= IMAGE_MAX_OUT_BYTES {
            return Some(Picture {
                mime: "image/jpeg".into(),
                width: img.width(),
                height: img.height(),
                bytes: Bytes::from(data),
            });
        }
        if quality > 60 {
            quality -= 12;
        } else {
            let (nw, nh) = (
                (img.width() * 4 / 5).max(48),
                (img.height() * 4 / 5).max(48),
            );
            if nw == img.width() && nh == img.height() {
                return None;
            }
            img = img.resize_exact(nw, nh, image::imageops::FilterType::Lanczos3);
            quality = 72;
        }
    }
}

async fn fetch_picture(fetch: &Fetcher, url: &str, art: bool) -> Option<Picture> {
    let f = fetch.get(url, "image/*,*/*;q=0.5", IMAGE_MAX_BYTES).await?;
    if !(200..300).contains(&f.status) {
        return None;
    }
    let bytes = f.body;
    tokio::task::spawn_blocking(move || process_picture(&bytes, art))
        .await
        .ok()
        .flatten()
}

/// YouTube's `maxresdefault` is missing for older videos: step down to `hqdefault`.
async fn fetch_picture_with_fallback(fetch: &Fetcher, p: &Preview) -> Option<Picture> {
    let url = p.image_url.as_deref()?;
    if let Some(pic) = fetch_picture(fetch, url, p.art).await {
        return Some(pic);
    }
    if url.contains("/maxresdefault.jpg") {
        return fetch_picture(
            fetch,
            &url.replace("/maxresdefault.jpg", "/hqdefault.jpg"),
            p.art,
        )
        .await;
    }
    None
}

// ---- cache ----------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    fetched_at: i64,
    unfurled: Option<Unfurled>,
}

impl CacheEntry {
    fn fresh(&self, now: i64) -> bool {
        let ttl = if self.unfurled.is_some() {
            OK_TTL_MS
        } else {
            FAIL_TTL_MS
        };
        now - self.fetched_at < ttl
    }
}

fn cache_key(canonical_url: &str) -> String {
    hex::encode(Sha256::digest(canonical_url.as_bytes()))[..40].to_owned()
}

fn cache_dir(state: &AppState) -> std::path::PathBuf {
    state.cfg.data_dir.join("link-previews")
}

fn memory() -> &'static Mutex<HashMap<String, CacheEntry>> {
    static MEM: OnceLock<Mutex<HashMap<String, CacheEntry>>> = OnceLock::new();
    MEM.get_or_init(|| Mutex::new(HashMap::new()))
}

async fn cache_lookup(state: &AppState, key: &str, now: i64) -> Option<CacheEntry> {
    if let Some(e) = memory().lock().unwrap().get(key).cloned() {
        if e.fresh(now) {
            return Some(e);
        }
    }
    let dir = cache_dir(state);
    let text = tokio::fs::read(dir.join(format!("{key}.json")))
        .await
        .ok()?;
    let mut e: CacheEntry = serde_json::from_slice(&text).ok()?;
    if !e.fresh(now) {
        return None;
    }
    if let Some(u) = e.unfurled.as_mut() {
        if let Some(pic) = u.picture.as_mut() {
            match tokio::fs::read(dir.join(format!("{key}.img"))).await {
                Ok(b) => pic.bytes = Bytes::from(b),
                Err(_) => u.picture = None,
            }
        }
    }
    memory().lock().unwrap().insert(key.to_owned(), e.clone());
    Some(e)
}

async fn cache_store(state: &AppState, key: &str, e: &CacheEntry) {
    memory().lock().unwrap().insert(key.to_owned(), e.clone());
    let dir = cache_dir(state);
    let _ = tokio::fs::create_dir_all(&dir).await;
    if let Some(pic) = e.unfurled.as_ref().and_then(|u| u.picture.as_ref()) {
        let _ = tokio::fs::write(dir.join(format!("{key}.img")), &pic.bytes).await;
    }
    if let Ok(text) = serde_json::to_vec(e) {
        let _ = tokio::fs::write(dir.join(format!("{key}.json")), text).await;
    }
    // Memory stays bounded: drop stale entries when it grows.
    let mut mem = memory().lock().unwrap();
    if mem.len() > 2_000 {
        let now = now_ms();
        mem.retain(|_, e| e.fresh(now));
    }
}

/// One unfurl through the cache: fetched at most once per canonical URL per TTL.
pub async fn unfurl_cached(state: &AppState, canonical_url: &Url) -> Option<Unfurled> {
    let key = cache_key(canonical_url.as_str());
    let now = now_ms();
    if let Some(e) = cache_lookup(state, &key, now).await {
        return e.unfurled;
    }
    let started = Instant::now();
    let fetch = Fetcher::new();
    let unfurled = match resolve(&fetch, canonical_url).await {
        Some(preview) => {
            let picture = fetch_picture_with_fallback(&fetch, &preview).await;
            Some(Unfurled {
                canonical_url: canonical_url.to_string(),
                preview,
                picture,
            })
        }
        None => None,
    };
    tracing::info!(
        url = %canonical_url,
        ok = unfurled.is_some(),
        ms = started.elapsed().as_millis() as u64,
        "link preview"
    );
    cache_store(
        state,
        &key,
        &CacheEntry {
            fetched_at: now,
            unfurled: unfurled.clone(),
        },
    )
    .await;
    unfurled
}

// ---- the ingest hook ----------------------------------------------------------------------

/// What a scan found on one message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub msg_id: String,
    pub url: String,
}

/// Messages in `rows` (projected, oldest first) that want a preview: a finished user /
/// assistant / member row created within [`FRESH_MS`] of `now`, holding a bare URL, with no
/// `link_preview` part yet and the preview not switched off.
pub fn candidates(rows: &[Value], now: i64) -> Vec<Candidate> {
    rows.iter()
        .filter_map(|m| {
            let id = m.get("id")?.as_str()?;
            let role = m.get("role").and_then(Value::as_str).unwrap_or("");
            if role == "system" {
                return None;
            }
            if m.get("status").and_then(Value::as_str) != Some("done") {
                return None;
            }
            if m.get(PREVIEW_FIELD).and_then(Value::as_str) == Some(PREVIEW_OFF) {
                return None;
            }
            if m.get("hidden").and_then(Value::as_bool) == Some(true)
                || m.get("deletedAt").is_some_and(|v| !v.is_null())
                || m.get("redactedAt").is_some_and(|v| !v.is_null())
            {
                return None;
            }
            let created = m.get("createdAt").and_then(Value::as_i64).unwrap_or(0);
            if created > 0 && now - created > FRESH_MS {
                return None;
            }
            let parts = m.get("parts")?.as_array()?;
            if parts
                .iter()
                .any(|p| p.get("kind").and_then(Value::as_str) == Some(PART_KIND))
            {
                return None;
            }
            let text: String = parts
                .iter()
                .filter(|p| p.get("kind").and_then(Value::as_str) == Some("text"))
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            let url = first_bare_url(&text)?;
            Some(Candidate {
                msg_id: id.to_owned(),
                url,
            })
        })
        .collect()
}

fn attempted() -> &'static Mutex<HashMap<(String, String), i64>> {
    static A: OnceLock<Mutex<HashMap<(String, String), i64>>> = OnceLock::new();
    A.get_or_init(|| Mutex::new(HashMap::new()))
}

/// True the first time a (room, message) is seen within [`ATTEMPT_TTL_MS`].
fn first_attempt(room: &str, msg_id: &str, now: i64) -> bool {
    let mut a = attempted().lock().unwrap();
    if a.len() > 5_000 {
        a.retain(|_, t| now - *t < ATTEMPT_TTL_MS);
    }
    let key = (room.to_owned(), msg_id.to_owned());
    if a.get(&key).is_some_and(|t| now - *t < ATTEMPT_TTL_MS) {
        return false;
    }
    a.insert(key, now);
    true
}

fn generations() -> &'static Mutex<HashMap<String, u64>> {
    static G: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    G.get_or_init(|| Mutex::new(HashMap::new()))
}

fn workers() -> &'static Semaphore {
    static S: OnceLock<Semaphore> = OnceLock::new();
    S.get_or_init(|| Semaphore::new(WORKERS))
}

/// Called by the room socket after every stored, broadcast row that is not a heal. Debounces
/// per room, then scans its newest messages for links. Never blocks the socket.
pub fn on_row(state: &AppState, project_id: &str, chat_id: &str, from_box: bool) {
    if !enabled() || chat_id == arbos_core::wire::PROJECT_DOC_CHAT_ID {
        return;
    }
    let room_name = arbos_core::wire::room_name(project_id, chat_id);
    let generation = {
        let mut g = generations().lock().unwrap();
        let e = g.entry(room_name.clone()).or_insert(0);
        *e += 1;
        *e
    };
    let state = state.clone();
    let project_id = project_id.to_owned();
    let chat_id = chat_id.to_owned();
    tokio::spawn(async move {
        tokio::time::sleep(if from_box {
            DEBOUNCE_BOX
        } else {
            DEBOUNCE_DEVICE
        })
        .await;
        if generations().lock().unwrap().get(&room_name).copied() != Some(generation) {
            return; // a newer row restarted the clock
        }
        if let Err(e) = scan_room(&state, &project_id, &chat_id).await {
            tracing::debug!(room = %room_name, error = %e, "link preview scan");
        }
    });
}

async fn scan_room(state: &AppState, project_id: &str, chat_id: &str) -> anyhow::Result<()> {
    let room_name = arbos_core::wire::room_name(project_id, chat_id);
    let rows = crate::rooms::room_tail_rows(state, &room_name, SCAN_TAIL).await?;
    let now = now_ms();
    let found: Vec<Candidate> = candidates(&rows, now)
        .into_iter()
        .filter(|c| first_attempt(&room_name, &c.msg_id, now))
        .collect();
    for c in found {
        let Some(canonical_url) = canonical(&c.url) else {
            continue;
        };
        let _permit = workers().acquire().await?;
        let Some(unfurled) = unfurl_cached(state, &canonical_url).await else {
            continue;
        };
        let project = (project_id != PEOPLE_PROJECT).then_some(project_id);
        if let Err(e) = write_part(state, project, chat_id, &c, &unfurled).await {
            tracing::debug!(room = %room_name, msg = %c.msg_id, error = %e, "link preview write");
        }
    }
    Ok(())
}

/// The blob slot a chat's previews live in: the project id, or `people:{chat}` for a people
/// chat — exactly who may read the chat may read its pictures.
pub fn blob_slot(project_id: Option<&str>, chat_id: &str) -> String {
    match project_id {
        Some(p) => p.to_owned(),
        None => format!("{}{chat_id}", crate::people::PEOPLE_ACL_PREFIX),
    }
}

/// Store the picture in the chat's slot and append the part — unless the message gained a
/// preview or lost the right to one meanwhile.
pub async fn write_part(
    state: &AppState,
    project_id: Option<&str>,
    chat_id: &str,
    c: &Candidate,
    unfurled: &Unfurled,
) -> anyhow::Result<()> {
    let slot = blob_slot(project_id, chat_id);
    let mut image_ref = None;
    if let Some(pic) = &unfurled.picture {
        let key = format!(
            "lp-{}.jpg",
            cache_key(&format!(
                "{}|{}|{}",
                unfurled.canonical_url, pic.width, pic.height
            ))
        );
        crate::blobs::store(state, &slot, &key, &pic.mime, &pic.bytes)
            .await
            .map_err(|e| anyhow::anyhow!("blob: {e}"))?;
        image_ref = Some(key);
    }
    // `siteIcon`: the shared link-icon resolver's picture for the final host, copied into the
    // chat's slot ([feedback link-icons-relay-2026-10-27]); absent when the host has none.
    let site_icon = match Url::parse(&unfurled.canonical_url)
        .ok()
        .map(|u| host_of(&u))
    {
        Some(host) => crate::site_icons::site_icon_for(state, &slot, &host).await,
        None => None,
    };
    let part = unfurled.part_json(&c.url, image_ref.as_deref(), site_icon);
    let msg_id = c.msg_id.clone();
    crate::notices::write_doc(state, project_id, chat_id, move |doc: &ChatDoc| {
        let m = doc
            .message(&msg_id)
            .ok_or_else(|| anyhow::anyhow!("message gone"))?;
        if m.parts
            .iter()
            .any(|p| p.get("kind").and_then(Value::as_str) == Some(PART_KIND))
        {
            anyhow::bail!("already previewed");
        }
        if m.rest.get(PREVIEW_FIELD).and_then(Value::as_str) == Some(PREVIEW_OFF) {
            anyhow::bail!("preview off");
        }
        doc.add_part_from_json(&msg_id, &part)
            .ok_or_else(|| anyhow::anyhow!("part not added"))?;
        Ok(())
    })
    .await?;
    tracing::info!(chat = %chat_id, msg = %c.msg_id, provider = %unfurled.preview.provider, "link preview written");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::routing::get as axum_get;
    use axum::Router;

    use super::*;

    #[test]
    fn finds_the_first_bare_url_and_skips_labelled_and_code_links() {
        assert_eq!(
            first_bare_url(
                "look https://open.spotify.com/track/7eiYNCojnoUVnz14kjBcQH?si=163bbf4a6caf4b39"
            )
            .as_deref(),
            Some("https://open.spotify.com/track/7eiYNCojnoUVnz14kjBcQH?si=163bbf4a6caf4b39")
        );
        assert_eq!(
            first_bare_url("see [the docs](https://example.com/a) today"),
            None
        );
        assert_eq!(first_bare_url("run `curl https://example.com/a` now"), None);
        assert_eq!(
            first_bare_url("```\nhttps://example.com/in-fence\n```\nplain"),
            None
        );
        assert_eq!(
            first_bare_url(
                "```\nhttps://example.com/in-fence\n```\nthen https://example.com/after."
            )
            .as_deref(),
            Some("https://example.com/after")
        );
        assert_eq!(
            first_bare_url("(https://en.wikipedia.org/wiki/Foo_(bar))").as_deref(),
            Some("https://en.wikipedia.org/wiki/Foo_(bar)")
        );
        assert_eq!(
            first_bare_url("a https://x.com/mira/status/1?s=20, b https://b.com").as_deref(),
            Some("https://x.com/mira/status/1?s=20")
        );
        assert_eq!(first_bare_url("arbos://mention/u1 and nothing"), None);
        assert_eq!(
            first_bare_url("http://localhost:7700/x"),
            None,
            "no dot, no link"
        );
        assert_eq!(first_bare_url("xhttps://glued.com/a"), None);
        assert_eq!(
            first_bare_url("<https://example.com/angle>").as_deref(),
            Some("https://example.com/angle")
        );
    }

    #[test]
    fn canonical_urls_drop_tracking_and_unify_youtube_and_x() {
        let c = |s: &str| canonical(s).unwrap().to_string();
        assert_eq!(
            c("https://open.spotify.com/intl-de/track/7eiYNCojnoUVnz14kjBcQH?si=163bbf4a6caf4b39"),
            "https://open.spotify.com/track/7eiYNCojnoUVnz14kjBcQH"
        );
        assert_eq!(
            c("https://youtu.be/kX2fPn7Qa3c?si=abc&t=42"),
            "https://www.youtube.com/watch?v=kX2fPn7Qa3c&t=42"
        );
        assert_eq!(
            c("https://www.youtube.com/shorts/kX2fPn7Qa3c"),
            "https://www.youtube.com/watch?v=kX2fPn7Qa3c"
        );
        assert_eq!(
            c("https://music.youtube.com/watch?v=kX2fPn7Qa3c&feature=shared"),
            "https://www.youtube.com/watch?v=kX2fPn7Qa3c"
        );
        assert_eq!(
            c("https://m.youtube.com/watch?v=kX2fPn7Qa3c&list=PL1"),
            "https://www.youtube.com/watch?v=kX2fPn7Qa3c&list=PL1"
        );
        assert_eq!(
            c("https://twitter.com/mira/status/19?s=20&t=zz"),
            "https://x.com/mira/status/19"
        );
        assert_eq!(
            c("https://Example.COM/a?utm_source=x&id=7#frag"),
            "https://example.com/a?id=7"
        );
        assert_eq!(c("https://example.com:443/a"), "https://example.com/a");
        assert!(canonical("ftp://example.com/a").is_none());
        assert!(canonical("arbos://mention/x").is_none());
    }

    #[test]
    fn providers_are_picked_by_host() {
        let p = |s: &str| provider_of(&canonical(s).unwrap());
        assert_eq!(p("https://open.spotify.com/track/x"), Provider::Spotify);
        assert_eq!(p("https://youtu.be/kX2fPn7Qa3c"), Provider::YouTube);
        assert_eq!(p("https://twitter.com/a/status/1"), Provider::X);
        assert_eq!(
            p("https://github.com/unarbos/arbos/pull/2088"),
            Provider::GitHub
        );
        assert_eq!(p("https://gist.github.com/u/abc"), Provider::GitHub);
        assert_eq!(
            p("https://bsky.app/profile/a.bsky.social/post/3k"),
            Provider::Bluesky
        );
        assert_eq!(p("https://vimeo.com/123"), Provider::Vimeo);
        assert_eq!(p("https://www.loom.com/share/abc"), Provider::Loom);
        assert_eq!(p("https://soundcloud.com/a/b"), Provider::SoundCloud);
        assert_eq!(
            p("https://music.apple.com/us/album/x/1?i=2"),
            Provider::AppleMusic
        );
        assert_eq!(p("https://maps.app.goo.gl/abc"), Provider::GoogleMaps);
        assert_eq!(
            p("https://www.google.com/maps/place/x"),
            Provider::GoogleMaps
        );
        assert_eq!(p("https://en.wikipedia.org/wiki/Loro"), Provider::Wikipedia);
        assert_eq!(p("https://arbos.life/s/abc"), Provider::Arbos);
        assert_eq!(
            p("https://www.instagram.com/p/abc/"),
            Provider::Web,
            "no special case on purpose"
        );
        assert_eq!(p("https://www.tiktok.com/@a/video/1"), Provider::Web);
        assert_eq!(p("https://linear.app/arbos/issue/ARB-1"), Provider::Web);
    }

    #[test]
    fn page_meta_reads_og_twitter_and_title_in_order() {
        let html = r#"<html><head><title>Fallback &amp; title</title>
            <meta property="og:title" content="Why every chat app unfurls links server-side">
            <meta name="description" content="plain description">
            <meta property="og:description" content="How iMessage, Discord &amp; Slack fetch previews">
            <meta property="og:site_name" content="Latent Space">
            <meta property="og:image" content="/img/cover.jpg">
            <meta property="og:image:width" content="1200"><meta property="og:image:height" content="630">
            <meta property="og:type" content="article">
            <link rel="canonical" href="https://www.latent.space/p/unfurl">
            </head><body></body></html>"#;
        let m = read_page("https://www.latent.space/p/why-chat-apps-unfurl", html);
        assert_eq!(
            m.title.as_deref(),
            Some("Why every chat app unfurls links server-side")
        );
        assert_eq!(
            m.description.as_deref(),
            Some("How iMessage, Discord & Slack fetch previews")
        );
        assert_eq!(m.site_name.as_deref(), Some("Latent Space"));
        assert_eq!(
            m.image.as_deref(),
            Some("https://www.latent.space/img/cover.jpg")
        );
        assert_eq!(m.og_type.as_deref(), Some("article"));
        let bare = read_page(
            "https://a.com/",
            "<html><head><title> Only a   title </title></head></html>",
        );
        assert_eq!(bare.title.as_deref(), Some("Only a title"));
        assert!(read_page("https://a.com/", "<html></html>").title.is_none());
        let u = Url::parse("https://www.latent.space/p/x").unwrap();
        let p = generic_preview(&u, &m, "web", None).unwrap();
        assert_eq!(p.kind, "article");
        assert_eq!(p.provider_name, "Latent Space");
        assert!(
            generic_preview(&u, &PageMeta::default(), "web", None).is_none(),
            "no title, no card"
        );
    }

    #[test]
    fn x_blockquotes_parse_into_text_handle_and_date() {
        let html = r#"<blockquote class="twitter-tweet"><p lang="en" dir="ltr">Subnet 120 just crossed 1,000 validators.<br>The curve finally looks like the whitepaper &amp; it took 14 months <a href="https://t.co/abc">pic.twitter.com/abc</a></p>&mdash; Mira Kaplan (@mirakaplan) <a href="https://twitter.com/mirakaplan/status/1982731845112041984?ref_src=twsrc%5Etfw">October 27, 2026</a></blockquote>"#;
        let (text, handle, date) = parse_blockquote(html);
        assert_eq!(
            text.as_deref(),
            Some("Subnet 120 just crossed 1,000 validators.\nThe curve finally looks like the whitepaper & it took 14 months pic.twitter.com/abc")
        );
        assert_eq!(handle.as_deref(), Some("@mirakaplan"));
        assert_eq!(date.as_deref(), Some("October 27, 2026"));
        let bsky = r#"<blockquote class="bluesky-embed"><p lang="en">hello world</p>&mdash; Mira (<a href="https://bsky.app/profile/mira.bsky.social">@mira.bsky.social</a>) <a href="https://bsky.app/profile/mira.bsky.social/post/3k">Oct 27, 2026 at 2:14 PM</a></blockquote>"#;
        let (t, h, d) = parse_blockquote(bsky);
        assert_eq!(t.as_deref(), Some("hello world"));
        assert_eq!(h.as_deref(), Some("@mira.bsky.social"));
        assert_eq!(d.as_deref(), Some("Oct 27, 2026 at 2:14 PM"));
    }

    #[test]
    fn text_helpers() {
        assert_eq!(
            decode_entities("a &amp; b &#39;c&#x27; &lt;d&gt; &quot;e&quot; &unknown; &"),
            "a & b 'c' <d> \"e\" &unknown; &"
        );
        assert_eq!(fmt_duration_secs(243), "4:03");
        assert_eq!(fmt_duration_secs(3723), "1:02:03");
        assert_eq!(parse_iso8601_duration("PT18M24S"), Some(1104));
        assert_eq!(parse_iso8601_duration("PT1H2M3S"), Some(3723));
        assert_eq!(parse_iso8601_duration("bogus"), None);
        assert_eq!(fmt_count(312), "312");
        assert_eq!(fmt_count(1_234), "1.2K");
        assert_eq!(fmt_count(48_000), "48K");
        assert_eq!(fmt_count(2_000_000), "2M");
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-27T18:00:00Z")
            .unwrap()
            .timestamp_millis();
        assert_eq!(
            fmt_age("2026-10-23T17:21:18Z", now).as_deref(),
            Some("4 days ago")
        );
        assert_eq!(
            fmt_age("2026-10-23T18:21:18Z", now).as_deref(),
            Some("3 days ago")
        );
        assert_eq!(
            fmt_age("2026-10-27T15:00:00Z", now).as_deref(),
            Some("3 hours ago")
        );
        assert_eq!(clip("abcdef", 4), "abc…");
        assert_eq!(bytes_label(3 * 1024 * 1024 + 200_000), "3.2 MB");
        assert_eq!(percent_decode("a%20b%2Fc.pdf"), "a b/c.pdf");
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([40, 120, 200, 255]));
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn pictures_are_bounded_and_reencoded() {
        let p = process_picture(&png(3000, 1500), false).unwrap();
        assert_eq!((p.width, p.height), (1280, 640));
        assert_eq!(p.mime, "image/jpeg");
        assert!(p.bytes.len() <= IMAGE_MAX_OUT_BYTES);
        let art = process_picture(&png(640, 640), true).unwrap();
        assert_eq!((art.width, art.height), (300, 300));
        assert!(
            process_picture(&png(20, 20), false).is_none(),
            "too small to be a picture"
        );
        assert!(process_picture(b"nope", false).is_none());
        let u = Unfurled {
            canonical_url: "https://a.com".into(),
            preview: Preview::new("web", "a.com", "article", "t"),
            picture: Some(p.clone()),
        };
        assert_eq!(u.layout(), "expanded");
        let mut compact = u.clone();
        compact.preview.art = true;
        assert_eq!(compact.layout(), "compact");
        let mut square = u.clone();
        square.picture = Some(Picture {
            width: 300,
            height: 300,
            ..p
        });
        assert_eq!(square.layout(), "compact");
        let mut none = u.clone();
        none.picture = None;
        assert_eq!(none.layout(), "compact");
    }

    #[test]
    fn the_part_carries_only_what_the_card_shows() {
        let mut preview = Preview::new("github", "GitHub", "pull_request", "Link-open confirm");
        preview.subtitle = Some("unarbos/arbos · #2088".into());
        preview.state = Some("open".into());
        preview.diff = Some(json!({"additions": 612, "deletions": 38, "files": 9}));
        preview.facts = vec!["9 files".into(), "opened 4 days ago".into()];
        let u = Unfurled {
            canonical_url: "https://github.com/unarbos/arbos/pull/2088".into(),
            preview,
            picture: None,
        };
        let v = u.part_json("https://github.com/unarbos/arbos/pull/2088?x=1", None, None);
        assert_eq!(v["kind"], PART_KIND);
        assert_eq!(v["specVersion"], SPEC_VERSION);
        assert_eq!(v["provider"], "github");
        assert_eq!(v["type"], "pull_request");
        assert_eq!(v["layout"], "compact");
        assert_eq!(v["state"], "open");
        assert_eq!(v["diff"]["additions"], 612);
        assert_eq!(v["siteHost"], "github.com");
        assert!(v.get("siteIcon").is_none(), "reserved for the shared tile");
        assert!(v.get("image").is_none());
        assert!(v.get("description").is_none());
        assert!(v["id"].as_str().is_some_and(|s| s.len() == 26));
    }

    #[test]
    fn candidates_are_fresh_finished_rows_with_a_bare_link_and_no_preview_yet() {
        let now = now_ms();
        let row = |id: &str, text: &str| {
            json!({ "id": id, "role": "user", "status": "done", "createdAt": now - 1000,
                    "parts": [{ "id": "p1", "kind": "text", "text": text }] })
        };
        let mut rows = vec![
            row("a", "https://open.spotify.com/track/x"),
            row("b", "no link here"),
            row("c", "see [docs](https://example.com)"),
        ];
        let mut streaming = row("d", "https://example.com/d");
        streaming["status"] = json!("streaming");
        rows.push(streaming);
        let mut off = row("e", "https://example.com/e");
        off["preview"] = json!("off");
        rows.push(off);
        let mut done = row("f", "https://example.com/f");
        done["parts"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": "p2", "kind": PART_KIND, "title": "t"}));
        rows.push(done);
        let mut old = row("g", "https://example.com/g");
        old["createdAt"] = json!(now - FRESH_MS - 1);
        rows.push(old);
        let mut sys = row("h", "https://example.com/h");
        sys["role"] = json!("system");
        rows.push(sys);
        let mut agent = row("i", "Here is the source: https://x.com/mira/status/1");
        agent["role"] = json!("assistant");
        rows.push(agent);
        let got = candidates(&rows, now);
        assert_eq!(
            got,
            vec![
                Candidate {
                    msg_id: "a".into(),
                    url: "https://open.spotify.com/track/x".into()
                },
                Candidate {
                    msg_id: "i".into(),
                    url: "https://x.com/mira/status/1".into()
                },
            ]
        );
        assert!(first_attempt("r", "a", now));
        assert!(
            !first_attempt("r", "a", now + 1),
            "the same message is not retried within the hour"
        );
        assert!(first_attempt("r", "a", now + ATTEMPT_TTL_MS + 1));
    }

    #[test]
    fn host_denylist_and_rate_limit() {
        assert!(!host_denied("example.com"));
        for _ in 0..HOST_PER_MINUTE {
            assert!(host_rate_ok("ratelimit.test"));
        }
        assert!(
            !host_rate_ok("ratelimit.test"),
            "the 61st fetch in a minute waits"
        );
        assert!(host_rate_ok("other.test"));
    }

    /// The fetcher against a local server: redirects are followed and re-checked, pages are
    /// cut at the cap, pictures over the cap are refused, private targets are never contacted,
    /// and a generic page resolves into a card with its picture.
    #[tokio::test]
    async fn fetches_pages_and_pictures_within_the_rules() {
        let hits = Arc::new(Mutex::new(Vec::<String>::new()));
        let h = hits.clone();
        let big_page = format!(
            "<html><head><title>Big</title></head><body>{}</body></html>",
            "x".repeat(PAGE_MAX_BYTES + 10_000)
        );
        let app = Router::new()
            .route("/", axum_get({
                let h = h.clone();
                move || { h.lock().unwrap().push("/".into()); async {
                    axum::response::Html(r#"<html><head><meta property="og:title" content="A page &amp; a half"><meta property="og:description" content="desc"><meta property="og:image" content="/cover.png"><meta property="og:type" content="article"><meta property="og:site_name" content="Test Site"></head></html>"#)
                } }
            }))
            .route("/cover.png", axum_get({
                let h = h.clone();
                move || { h.lock().unwrap().push("/cover.png".into()); async { ([(header::CONTENT_TYPE, "image/png")], png(1200, 630)) } }
            }))
            .route("/redir", axum_get(|| async { (StatusCode::FOUND, [(header::LOCATION, "/")]) }))
            .route("/loop", axum_get(|| async { (StatusCode::FOUND, [(header::LOCATION, "/loop")]) }))
            .route("/private", axum_get(|| async { (StatusCode::FOUND, [(header::LOCATION, "http://169.254.169.254/latest/meta-data/")]) }))
            .route("/big", axum_get(move || { let p = big_page.clone(); async move { axum::response::Html(p) } }))
            .route("/huge.png", axum_get(|| async { ([(header::CONTENT_TYPE, "image/png")], vec![0u8; IMAGE_MAX_BYTES + 4096]) }))
            .route("/file.pdf", axum_get(|| async { ([(header::CONTENT_TYPE, "application/pdf")], vec![0u8; 204_800]) }))
            .route("/notitle", axum_get(|| async { axum::response::Html("<html><body>nothing</body></html>") }));
        use axum::http::StatusCode;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let base = format!("http://{addr}");
        let fetch = Fetcher::allowing(&addr.to_string());

        let f = fetch
            .get(&format!("{base}/redir"), "text/html", PAGE_MAX_BYTES)
            .await
            .unwrap();
        assert_eq!(f.status, 200);
        assert!(f.final_url.ends_with('/'));
        assert!(
            fetch
                .get(&format!("{base}/loop"), "text/html", PAGE_MAX_BYTES)
                .await
                .is_none(),
            "redirect loops stop"
        );
        assert!(
            fetch
                .get(&format!("{base}/private"), "text/html", PAGE_MAX_BYTES)
                .await
                .is_none(),
            "a redirect to a private address is refused"
        );
        let cut = fetch
            .get(&format!("{base}/big"), "text/html", PAGE_MAX_BYTES)
            .await
            .unwrap();
        assert_eq!(cut.body.len(), PAGE_MAX_BYTES);
        assert!(
            fetch
                .get(&format!("{base}/huge.png"), "image/*", IMAGE_MAX_BYTES)
                .await
                .is_none(),
            "a picture over the cap is refused"
        );

        // Without the allowance the local server is never contacted.
        let strict = Fetcher::new();
        let before = hits.lock().unwrap().len();
        assert!(strict
            .get(&base, "text/html", PAGE_MAX_BYTES)
            .await
            .is_none());
        assert_eq!(hits.lock().unwrap().len(), before);

        // A generic page becomes a card with its picture.
        let u = Url::parse(&format!("{base}/")).unwrap();
        let p = resolve(&fetch, &u).await.expect("a card");
        assert_eq!(p.title, "A page & a half");
        assert_eq!(p.kind, "article");
        assert_eq!(p.provider_name, "Test Site");
        let pic = fetch_picture_with_fallback(&fetch, &p)
            .await
            .expect("the cover");
        assert_eq!((pic.width, pic.height), (1200, 630));
        let unfurled = Unfurled {
            canonical_url: u.to_string(),
            preview: p,
            picture: Some(pic),
        };
        assert_eq!(unfurled.layout(), "expanded");
        let part = unfurled.part_json(
            u.as_str(),
            Some("lp-x.jpg"),
            Some(json!({"ref": "li-1.png", "width": 128, "height": 128, "mime": "image/png"})),
        );
        assert_eq!(part["siteIcon"]["ref"], "li-1.png");
        assert_eq!(part["image"]["width"], 1200);
        assert_eq!(part["layout"], "expanded");

        // A direct file is a file card; a page without a title is nothing.
        let pdf = resolve(&fetch, &Url::parse(&format!("{base}/file.pdf")).unwrap())
            .await
            .expect("file card");
        assert_eq!(pdf.kind, "file");
        assert_eq!(pdf.title, "file.pdf");
        assert_eq!(pdf.facts, vec!["PDF".to_owned(), "200 KB".to_owned()]);
        assert!(
            resolve(&fetch, &Url::parse(&format!("{base}/notitle")).unwrap())
                .await
                .is_none()
        );
    }

    /// End to end on an in-memory relay: a people chat gets a user row with a link to the
    /// local server; the scan writes the `link_preview` part with its blob in the chat's slot;
    /// a second scan writes nothing more; a `preview: off` message is left alone.
    #[tokio::test]
    async fn writes_the_part_once_and_honours_preview_off() {
        let app = Router::new()
            .route("/post", axum_get(|| async {
                axum::response::Html(r#"<html><head><meta property="og:title" content="Hello card"><meta property="og:image" content="/pic.png"></head></html>"#)
            }))
            .route("/pic.png", axum_get(|| async { ([(header::CONTENT_TYPE, "image/png")], png(800, 450)) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let url = format!("http://{addr}/post");

        let state = crate::state::test_support::test_state(crate::state::AuthMode::Github);
        let (alice, _) = crate::people::tests_support::person(&state, 9101);
        let (bob, _) = crate::people::tests_support::person(&state, 9102);
        let chat_id = crate::people::tests_support::dm(&state, &alice, &bob);
        let room = arbos_core::wire::room_name(PEOPLE_PROJECT, &chat_id);

        // Alice's device writes two rows: one with the link, one switched off.
        let doc = ChatDoc::new(Some(7), false);
        let base = doc.oplog_vv();
        doc.append_user_message(
            "m1",
            &format!("look {url}"),
            "dev:alice",
            &Default::default(),
        )
        .unwrap();
        doc.append_user_message(
            "m2",
            &format!("not this {url}"),
            "dev:alice",
            &Default::default(),
        )
        .unwrap();
        doc.set_message_fields("m2", &[(PREVIEW_FIELD, json!(PREVIEW_OFF))]);
        let bytes = doc.export_updates_since(&base);
        state
            .db
            .append_row(&room, "dev:alice", "b1", &bytes)
            .unwrap();

        // The unfurl itself, with the local server allowed.
        let rows = crate::rooms::room_tail_rows(&state, &room, SCAN_TAIL)
            .await
            .unwrap();
        let found = candidates(&rows, now_ms());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].msg_id, "m1");
        let fetch = Fetcher::allowing(&addr.to_string());
        let canonical_url = canonical(&found[0].url).unwrap();
        let preview = resolve(&fetch, &canonical_url).await.expect("card");
        let picture = fetch_picture_with_fallback(&fetch, &preview).await;
        let unfurled = Unfurled {
            canonical_url: canonical_url.to_string(),
            preview,
            picture,
        };
        write_part(&state, None, &chat_id, &found[0], &unfurled)
            .await
            .unwrap();

        let after = crate::matrix::backfill::load_doc(&state, &chat_id, None)
            .unwrap()
            .unwrap();
        let m1 = after.message("m1").unwrap();
        let part = m1
            .parts
            .iter()
            .find(|p| p["kind"] == PART_KIND)
            .expect("the part");
        assert_eq!(part["title"], "Hello card");
        assert_eq!(part["layout"], "expanded");
        let key = part["image"]["ref"].as_str().unwrap().to_owned();
        let slot = blob_slot(None, &chat_id);
        assert!(
            crate::blobs::read(&state, &slot, &key).await.is_some(),
            "picture in the chat's slot"
        );
        assert!(after
            .message("m2")
            .unwrap()
            .parts
            .iter()
            .all(|p| p["kind"] != PART_KIND));

        // Nothing left to do, and writing again is refused.
        let rows = crate::rooms::room_tail_rows(&state, &room, SCAN_TAIL)
            .await
            .unwrap();
        assert!(candidates(&rows, now_ms()).is_empty());
        let err = write_part(&state, None, &chat_id, &found[0], &unfurled)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("already previewed"));

        // The cache remembers a failure as well as a success.
        let miss = Url::parse(&format!("http://{addr}/none")).unwrap();
        assert!(unfurl_cached(&state, &miss).await.is_none());
        let key = cache_key(miss.as_str());
        assert!(cache_lookup(&state, &key, now_ms())
            .await
            .is_some_and(|e| e.unfurled.is_none()));
    }
}
