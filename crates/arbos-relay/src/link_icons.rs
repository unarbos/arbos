//! `GET /link-icon?domain=<host>` — the small picture the apps draw before an inline link
//! (Jacob, 2026-10-25: every `[Name](https://…)` in a reply carries its site's icon, as in
//! Grok Bot). One endpoint for both apps so the sites see the relay, never the user: the relay
//! fetches `https://<domain>/` once, takes the page's `og:image` when it is a usable square-ish
//! picture, else the `<link rel="icon">`, else `/favicon.ico`, scales it to a 64×64 PNG and keeps
//! it on disk under `<data>/link-icons/` for [`FOUND_TTL`]; a domain with no icon is remembered
//! for [`MISSING_TTL`] and answers 204 so the apps draw their neutral placeholder. Only public
//! hosts over http(s) are fetched: no IP literals, no localhost, no private or link-local
//! addresses after resolution, at most [`MAX_REDIRECTS`] redirects each checked the same way, and
//! bounded bodies and time.

use std::collections::HashMap;
use std::io::Cursor;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use serde::Deserialize;

use arbos_core::ids::now_ms;

use crate::auth::Principal;
use crate::signin::api_error;
use crate::state::AppState;

/// A found icon is served from the cache this long before the site is asked again.
pub const FOUND_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// A domain without an icon is not asked again for this long.
pub const MISSING_TTL_MS: i64 = 24 * 60 * 60 * 1000;
/// The side of the PNG the apps get (drawn at 15–16 pt, so 4× on a Retina display).
pub const ICON_SIDE: u32 = 64;
/// How much of a page is read looking for `og:image` / `<link rel=icon>`.
const PAGE_MAX_BYTES: usize = 512 * 1024;
/// The largest picture that is downloaded and decoded.
const IMAGE_MAX_BYTES: usize = 3 * 1024 * 1024;
const MAX_REDIRECTS: usize = 3;
const FETCH_TIMEOUT: Duration = Duration::from_secs(8);
/// An `og:image` wider than this (w/h) is a hero photo, not a mark: the icon is used instead.
const OG_MAX_ASPECT: f32 = 1.6;

#[derive(Deserialize)]
pub struct LinkIconQuery {
    pub domain: String,
}

/// What the cache knows about a domain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cached {
    pub fetched_at: i64,
    /// The 64×64 PNG, or `None` when the site has no usable icon.
    pub png: Option<Bytes>,
}

impl Cached {
    /// Still good at `now`: found icons live [`FOUND_TTL_MS`], misses [`MISSING_TTL_MS`].
    pub fn fresh(&self, now: i64) -> bool {
        let ttl = if self.png.is_some() {
            FOUND_TTL_MS
        } else {
            MISSING_TTL_MS
        };
        now - self.fetched_at < ttl
    }
}

fn memory() -> &'static Mutex<HashMap<String, Cached>> {
    static MEM: OnceLock<Mutex<HashMap<String, Cached>>> = OnceLock::new();
    MEM.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `GET /link-icon?domain=` for any signed-in principal (device, box or admin).
pub async fn get(
    State(state): State<AppState>,
    _principal: Principal,
    Query(q): Query<LinkIconQuery>,
) -> Response {
    let Some(domain) = public_domain(&q.domain) else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_domain",
            "domain must be a public host name",
        );
    };
    let now = now_ms();
    let cached = match lookup(&state, &domain, now).await {
        Some(c) => c,
        None => {
            let png = match resolve_public(&domain).await {
                Ok(()) => fetch_icon(&state.http, &format!("https://{domain}"), &domain)
                    .await
                    .ok()
                    .flatten(),
                Err(why) => {
                    tracing::debug!(domain, why, "link icon: host refused");
                    None
                }
            };
            let c = Cached {
                fetched_at: now,
                png,
            };
            remember(&state, &domain, &c).await;
            c
        }
    };
    match cached.png {
        Some(png) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "image/png".to_owned()),
                (
                    header::CACHE_CONTROL,
                    format!("public, max-age={}", FOUND_TTL_MS / 1000),
                ),
            ],
            png,
        )
            .into_response(),
        None => (
            StatusCode::NO_CONTENT,
            [(
                header::CACHE_CONTROL,
                format!("public, max-age={}", MISSING_TTL_MS / 1000),
            )],
        )
            .into_response(),
    }
}

/// The domain the apps may ask for: lower-case ASCII host name with at least one dot, no
/// port, no IP literal, nothing that names the machine itself or a private network by name.
pub fn public_domain(raw: &str) -> Option<String> {
    let d = raw.trim().trim_end_matches('.').to_ascii_lowercase();
    if d.is_empty() || d.len() > 253 || !d.contains('.') {
        return None;
    }
    if !d
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
    {
        return None;
    }
    if d.split('.')
        .any(|l| l.is_empty() || l.starts_with('-') || l.ends_with('-'))
    {
        return None;
    }
    if d.parse::<IpAddr>().is_ok() || d.split('.').all(|l| l.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    if d == "localhost"
        || d.ends_with(".localhost")
        || d.ends_with(".local")
        || d.ends_with(".internal")
        || d.ends_with(".arpa")
    {
        return None;
    }
    Some(d)
}

/// True for addresses the relay must never fetch from: loopback, private, link-local,
/// multicast, unspecified, and the IPv4-mapped forms of those.
pub fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_unspecified()
                || v4.is_documentation()
                // Shared address space (CGNAT) and the "this network" block.
                || (v4.octets()[0] == 100 && (64..=127).contains(&v4.octets()[1]))
                || v4.octets()[0] == 0
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_ip(IpAddr::V4(v4));
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // fc00::/7 unique local, fe80::/10 link local.
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

/// Resolve `domain` and refuse it when any address is private.
async fn resolve_public(domain: &str) -> Result<(), &'static str> {
    let addrs = tokio::net::lookup_host((domain, 443))
        .await
        .map_err(|_| "does not resolve")?;
    let mut any = false;
    for a in addrs {
        any = true;
        if is_private_ip(a.ip()) {
            return Err("resolves to a private address");
        }
    }
    if any {
        Ok(())
    } else {
        Err("does not resolve")
    }
}

/// The icon for the site at `base` (`scheme://host[:port]`), as a 64×64 PNG: the page's
/// `og:image` when square-ish, else its `<link rel="icon">`, else `/favicon.ico`. `Ok(None)`
/// when the site has none that decodes. `domain` is the host the apps asked for; a redirect or
/// an icon URL pointing elsewhere is still checked with [`public_domain`] unless it stays on
/// `base`'s own host (which may be a test server).
pub async fn fetch_icon(
    http: &reqwest::Client,
    base: &str,
    domain: &str,
) -> anyhow::Result<Option<Bytes>> {
    let page = get_bounded(http, base, PAGE_MAX_BYTES, base, domain).await;
    let mut candidates: Vec<(String, bool)> = Vec::new();
    if let Some((final_url, html)) = &page {
        let text = String::from_utf8_lossy(html);
        if let Some(og) = meta_content(&text, "og:image") {
            if let Some(u) = absolute(final_url, &og) {
                candidates.push((u, true));
            }
        }
        if let Some(icon) = icon_href(&text) {
            if let Some(u) = absolute(final_url, &icon) {
                candidates.push((u, false));
            }
        }
    }
    candidates.push((format!("{}/favicon.ico", base.trim_end_matches('/')), false));
    for (url, is_og) in candidates {
        let Some((_, bytes)) = get_bounded(http, &url, IMAGE_MAX_BYTES, base, domain).await else {
            continue;
        };
        if let Some(png) = normalize(&bytes, is_og) {
            return Ok(Some(png));
        }
    }
    Ok(None)
}

/// GET with a byte cap, a timeout and up to [`MAX_REDIRECTS`] redirects, each target checked
/// to be `base`'s own host or a public domain. Returns the final URL and the body.
async fn get_bounded(
    http: &reqwest::Client,
    url: &str,
    max: usize,
    base: &str,
    domain: &str,
) -> Option<(String, Bytes)> {
    let base_host = reqwest::Url::parse(base).ok()?.host_str()?.to_owned();
    let mut url = url.to_owned();
    for _ in 0..=MAX_REDIRECTS {
        let parsed = reqwest::Url::parse(&url).ok()?;
        if parsed.scheme() != "https" && parsed.scheme() != "http" {
            return None;
        }
        let host = parsed.host_str()?.to_owned();
        if host != base_host && host != domain {
            public_domain(&host)?;
            if let Ok(addrs) = tokio::net::lookup_host((host.as_str(), 443)).await {
                if addrs.into_iter().any(|a| is_private_ip(a.ip())) {
                    return None;
                }
            }
        }
        let resp = http
            .get(parsed.clone())
            .header(header::USER_AGENT, "Arbos link icons (+https://arbos.life)")
            .header(header::ACCEPT, "text/html,image/*;q=0.9,*/*;q=0.5")
            .timeout(FETCH_TIMEOUT)
            .send()
            .await
            .ok()?;
        if resp.status().is_redirection() {
            let next = resp.headers().get(header::LOCATION)?.to_str().ok()?;
            url = absolute(parsed.as_str(), next)?;
            continue;
        }
        if !resp.status().is_success() {
            return None;
        }
        if let Some(len) = resp.content_length() {
            if len as usize > max {
                return None;
            }
        }
        let mut body = Vec::new();
        let mut stream = resp;
        while let Ok(Some(chunk)) = stream.chunk().await {
            if body.len() + chunk.len() > max {
                // A page is read up to the cap (the head is what matters); a picture over
                // the cap is refused.
                if max == PAGE_MAX_BYTES {
                    body.extend_from_slice(&chunk[..max - body.len()]);
                    break;
                }
                return None;
            }
            body.extend_from_slice(&chunk);
        }
        return Some((parsed.to_string(), Bytes::from(body)));
    }
    None
}

/// `content` of the first `<meta property|name="<prop>" …>` in `html`, in either attribute order.
pub fn meta_content(html: &str, prop: &str) -> Option<String> {
    for tag in tags(html, "meta") {
        let is_prop = attr(&tag, "property")
            .or_else(|| attr(&tag, "name"))
            .is_some_and(|v| v.eq_ignore_ascii_case(prop));
        if is_prop {
            if let Some(c) = attr(&tag, "content") {
                if !c.trim().is_empty() {
                    return Some(c.trim().to_owned());
                }
            }
        }
    }
    None
}

/// `href` of the best `<link rel="icon|shortcut icon|apple-touch-icon">`: the first PNG/SVG-free
/// icon with the largest declared size wins, else the first icon.
pub fn icon_href(html: &str) -> Option<String> {
    let mut best: Option<(u32, String)> = None;
    for tag in tags(html, "link") {
        let Some(rel) = attr(&tag, "rel") else {
            continue;
        };
        let rel = rel.to_ascii_lowercase();
        let is_icon = rel
            .split_whitespace()
            .any(|r| r == "icon" || r == "apple-touch-icon");
        if !is_icon {
            continue;
        }
        let Some(href) = attr(&tag, "href") else {
            continue;
        };
        if href.trim().is_empty() || href.to_ascii_lowercase().ends_with(".svg") {
            continue;
        }
        let size = attr(&tag, "sizes")
            .and_then(|s| {
                s.split(['x', 'X'])
                    .next()
                    .and_then(|n| n.trim().parse::<u32>().ok())
            })
            .unwrap_or(if rel.contains("apple-touch-icon") {
                180
            } else {
                32
            });
        if best.as_ref().is_none_or(|(s, _)| size > *s) {
            best = Some((size, href.trim().to_owned()));
        }
    }
    best.map(|(_, h)| h)
}

/// Every `<name …>` tag of `html` (lower-cased name match), attributes text only.
pub(crate) fn tags(html: &str, name: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let open = format!("<{name}");
    let mut out = Vec::new();
    let mut from = 0usize;
    while let Some(i) = lower[from..].find(&open) {
        let start = from + i;
        let after = start + open.len();
        if !lower[after..].starts_with([' ', '\t', '\n', '\r', '/', '>']) {
            from = after;
            continue;
        }
        let Some(end) = lower[after..].find('>') else {
            break;
        };
        out.push(html[after..after + end].to_owned());
        from = after + end + 1;
    }
    out
}

/// The value of attribute `key` in a tag's attribute text (quoted or bare).
pub(crate) fn attr(tag: &str, key: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0usize;
    while let Some(i) = lower[from..].find(key) {
        let at = from + i;
        let before_ok = at == 0 || !lower.as_bytes()[at - 1].is_ascii_alphanumeric();
        let rest = &tag[at + key.len()..];
        let rest_trim = rest.trim_start();
        if before_ok && rest_trim.starts_with('=') {
            let v = rest_trim[1..].trim_start();
            return Some(match v.chars().next() {
                Some(q @ ('"' | '\'')) => v[1..].split(q).next().unwrap_or("").to_owned(),
                _ => v
                    .split([' ', '>', '\t', '\n', '/'])
                    .next()
                    .unwrap_or("")
                    .to_owned(),
            });
        }
        from = at + key.len();
    }
    None
}

/// `href` made absolute against `base_url` (scheme-relative and root-relative forms too).
pub fn absolute(base_url: &str, href: &str) -> Option<String> {
    let base = reqwest::Url::parse(base_url).ok()?;
    let href = href.trim();
    let u = if let Some(rest) = href.strip_prefix("//") {
        format!("{}://{}", base.scheme(), rest)
    } else {
        base.join(href).ok()?.to_string()
    };
    Some(u)
}

/// Decode any supported picture and scale it to a [`ICON_SIDE`]-square PNG (centre crop). An
/// `og:image` that is a wide hero photo (aspect over [`OG_MAX_ASPECT`]) is refused so the site's
/// mark is used instead; anything that does not decode is refused.
pub fn normalize(bytes: &[u8], is_og: bool) -> Option<Bytes> {
    let img = image::load_from_memory(bytes).ok()?;
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return None;
    }
    let aspect = w.max(h) as f32 / w.min(h) as f32;
    if is_og && aspect > OG_MAX_ASPECT {
        return None;
    }
    let square = img.resize_to_fill(ICON_SIDE, ICON_SIDE, image::imageops::FilterType::Lanczos3);
    let mut out = Cursor::new(Vec::new());
    square
        .to_rgba8()
        .write_to(&mut out, image::ImageFormat::Png)
        .ok()?;
    Some(Bytes::from(out.into_inner()))
}

// ---- cache: memory in front of `<data>/link-icons/<domain>.png|.none` ------------------------

fn cache_dir(state: &AppState) -> PathBuf {
    state.cfg.data_dir.join("link-icons")
}

async fn lookup(state: &AppState, domain: &str, now: i64) -> Option<Cached> {
    if let Some(c) = memory().lock().unwrap().get(domain).cloned() {
        if c.fresh(now) {
            return Some(c);
        }
    }
    let dir = cache_dir(state);
    let png = dir.join(format!("{domain}.png"));
    let none = dir.join(format!("{domain}.none"));
    let (path, found) = if tokio::fs::metadata(&png).await.is_ok() {
        (png, true)
    } else if tokio::fs::metadata(&none).await.is_ok() {
        (none, false)
    } else {
        return None;
    };
    let fetched_at = tokio::fs::metadata(&path)
        .await
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis() as i64;
    let c = Cached {
        fetched_at,
        png: if found {
            Some(Bytes::from(tokio::fs::read(&path).await.ok()?))
        } else {
            None
        },
    };
    if !c.fresh(now) {
        return None;
    }
    memory()
        .lock()
        .unwrap()
        .insert(domain.to_owned(), c.clone());
    Some(c)
}

async fn remember(state: &AppState, domain: &str, c: &Cached) {
    memory()
        .lock()
        .unwrap()
        .insert(domain.to_owned(), c.clone());
    let dir = cache_dir(state);
    let _ = tokio::fs::create_dir_all(&dir).await;
    let png = dir.join(format!("{domain}.png"));
    let none = dir.join(format!("{domain}.none"));
    match &c.png {
        Some(bytes) => {
            let _ = tokio::fs::remove_file(&none).await;
            let _ = tokio::fs::write(&png, bytes).await;
        }
        None => {
            let _ = tokio::fs::remove_file(&png).await;
            let _ = tokio::fs::write(&none, b"").await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::routing::get as axum_get;
    use axum::Router;

    use super::*;

    #[test]
    fn only_public_host_names_pass() {
        assert_eq!(
            public_domain(" www.ElVirrey.com. "),
            Some("www.elvirrey.com".into())
        );
        assert_eq!(public_domain("nypost.com"), Some("nypost.com".into()));
        for bad in [
            "",
            "localhost",
            "foo.localhost",
            "box.local",
            "relay.internal",
            "127.0.0.1",
            "10.0.0.1",
            "::1",
            "nodots",
            "a..b",
            "-a.com",
            "a-.com",
            "a.com:443",
            "a.com/x",
            "1.2.3.4.in-addr.arpa",
            "exa mple.com",
        ] {
            assert_eq!(public_domain(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn private_addresses_are_refused() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.9",
            "192.168.1.1",
            "169.254.169.254",
            "0.0.0.0",
            "100.64.0.1",
            "224.0.0.1",
            "::1",
            "fe80::1",
            "fc00::1",
            "::ffff:10.0.0.1",
        ] {
            assert!(is_private_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "104.21.5.6", "2606:4700::1111"] {
            assert!(!is_private_ip(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn page_parsing_finds_og_image_and_the_largest_icon() {
        let html = r#"<!doctype html><html><head>
            <META name="viewport" content="width=device-width">
            <meta content="https://cdn.example.com/og.jpg" property="og:image">
            <link rel="shortcut icon" href="/favicon.ico">
            <link rel="icon" type="image/png" sizes="32x32" href="/icons/32.png">
            <link rel="icon" sizes="192x192" href="//static.example.com/192.png">
            <link rel="icon" href="/mark.svg">
            <link rel="stylesheet" href="/x.css"></head><body><metadata>no</metadata></body></html>"#;
        assert_eq!(
            meta_content(html, "og:image").as_deref(),
            Some("https://cdn.example.com/og.jpg")
        );
        assert_eq!(meta_content(html, "twitter:image"), None);
        assert_eq!(
            icon_href(html).as_deref(),
            Some("//static.example.com/192.png")
        );
        assert_eq!(
            absolute(
                "https://www.example.com/a/b",
                "//static.example.com/192.png"
            )
            .as_deref(),
            Some("https://static.example.com/192.png")
        );
        assert_eq!(
            absolute("https://www.example.com/a/b", "/favicon.ico").as_deref(),
            Some("https://www.example.com/favicon.ico")
        );
        assert_eq!(
            absolute("https://www.example.com/a/b", "icons/x.png").as_deref(),
            Some("https://www.example.com/a/icons/x.png")
        );
        // Only `<link rel=icon>` with an href counts; a page without one yields nothing.
        assert_eq!(icon_href("<link rel=\"icon\">"), None);
        assert_eq!(
            icon_href("<link rel=\"apple-touch-icon\" href=\"/apple.png\">").as_deref(),
            Some("/apple.png")
        );
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 40, 40, 255]));
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn pictures_normalise_to_a_64_square_png_and_hero_og_images_are_refused() {
        let out = normalize(&png(300, 100), false).expect("a wide icon still crops");
        let back = image::load_from_memory(&out).unwrap();
        assert_eq!((back.width(), back.height()), (ICON_SIDE, ICON_SIDE));
        assert_eq!(image::guess_format(&out).unwrap(), image::ImageFormat::Png);
        assert!(
            normalize(&png(1200, 630), true).is_none(),
            "a 1.9:1 og:image is a hero photo"
        );
        assert!(
            normalize(&png(512, 400), true).is_some(),
            "a 1.28:1 og:image is a mark"
        );
        assert!(normalize(b"<html>not an image</html>", false).is_none());
        assert!(normalize(b"", false).is_none());
        // The classic /favicon.ico container decodes too.
        let img = image::RgbaImage::from_pixel(32, 32, image::Rgba([10, 20, 200, 255]));
        let mut ico = Cursor::new(Vec::new());
        img.write_to(&mut ico, image::ImageFormat::Ico).unwrap();
        let out = normalize(&ico.into_inner(), false).expect("ico decodes");
        assert_eq!(image::guess_format(&out).unwrap(), image::ImageFormat::Png);
    }

    #[test]
    fn cache_entries_age_by_their_kind() {
        let found = Cached {
            fetched_at: 0,
            png: Some(Bytes::from_static(b"x")),
        };
        let missing = Cached {
            fetched_at: 0,
            png: None,
        };
        assert!(found.fresh(FOUND_TTL_MS - 1));
        assert!(!found.fresh(FOUND_TTL_MS));
        assert!(missing.fresh(MISSING_TTL_MS - 1));
        assert!(!missing.fresh(MISSING_TTL_MS));
    }

    /// A site with an `og:image` hero and a PNG icon: the icon is served (the hero is refused
    /// by aspect); a site with only `/favicon.ico` gets that; a site with nothing yields None.
    /// Each fetch hits the test server; the second call for the same domain is served from the
    /// cache (the server sees one page fetch).
    #[tokio::test]
    async fn fetches_the_icon_once_and_caches_it() {
        let hits = Arc::new(Mutex::new(Vec::<String>::new()));
        let h = hits.clone();
        let app = Router::new()
            .route("/", axum_get({
                let h = h.clone();
                move || {
                    h.lock().unwrap().push("/".into());
                    async {
                        axum::response::Html(
                            r#"<html><head><meta property="og:image" content="/hero.png"><link rel="icon" sizes="64x64" href="/icon.png"></head></html>"#,
                        )
                    }
                }
            }))
            .route("/hero.png", axum_get({
                let h = h.clone();
                move || {
                    h.lock().unwrap().push("/hero.png".into());
                    async { ([(header::CONTENT_TYPE, "image/png")], png(1200, 630)) }
                }
            }))
            .route("/icon.png", axum_get({
                let h = h.clone();
                move || {
                    h.lock().unwrap().push("/icon.png".into());
                    async { ([(header::CONTENT_TYPE, "image/png")], png(64, 64)) }
                }
            }))
            .route("/bare/", axum_get(|| async { axum::response::Html("<html></html>") }))
            .route("/bare/favicon.ico", axum_get(|| async { ([(header::CONTENT_TYPE, "image/png")], png(16, 16)) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let base = format!("http://{addr}");
        let http = reqwest::Client::new();

        let icon = fetch_icon(&http, &base, "example.test")
            .await
            .unwrap()
            .expect("an icon");
        let back = image::load_from_memory(&icon).unwrap();
        assert_eq!((back.width(), back.height()), (ICON_SIDE, ICON_SIDE));
        assert_eq!(
            hits.lock().unwrap().as_slice(),
            ["/", "/hero.png", "/icon.png"]
        );

        // No page markup: /favicon.ico is the fallback.
        let bare = fetch_icon(&http, &format!("{base}/bare"), "bare.test")
            .await
            .unwrap();
        assert!(bare.is_some());

        // Nothing at all answers → None, not an error.
        let none = fetch_icon(&http, "http://127.0.0.1:1/", "none.test")
            .await
            .unwrap();
        assert!(none.is_none());

        // The cache: one state, remember then lookup; a stale entry is not returned.
        let state = crate::state::test_support::test_state(crate::state::AuthMode::Github);
        let c = Cached {
            fetched_at: now_ms(),
            png: Some(icon.clone()),
        };
        remember(&state, "example.test", &c).await;
        assert_eq!(
            lookup(&state, "example.test", now_ms()).await,
            Some(c.clone())
        );
        assert_eq!(
            lookup(&state, "example.test", now_ms() + FOUND_TTL_MS + 1).await,
            None
        );
        let miss = Cached {
            fetched_at: now_ms(),
            png: None,
        };
        remember(&state, "none.test", &miss).await;
        assert_eq!(
            lookup(&state, "none.test", now_ms()).await.map(|c| c.png),
            Some(None)
        );
        assert!(cache_dir(&state).join("none.test.none").exists());
        assert!(!cache_dir(&state).join("example.test.none").exists());
    }
}
