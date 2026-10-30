//! The one site-icon resolver ([feedback link-icons-relay-2026-10-27]): the high-resolution
//! picture a host shows on a link tile — the apps' Links tabs, Library rows and search results
//! (`GET /unfurl/icon?host=`, with `GET /link-icon?domain=` kept as an alias for the bubbles'
//! inline icons) and the `siteIcon` of a `link_preview` part (the embed cards) all read the
//! same bytes, fetched once per host.
//!
//! Jacob's report (2026-10-27): the github.com row of his Links tab showed a 16 px favicon
//! stretched over a 36 pt tile. The rules that stop that:
//!
//! 1. **Order:** the page's `<link rel="apple-touch-icon">` (largest declared size; 180 px by
//!    convention), then the largest `<link rel="icon">` with a raster type, then the
//!    conventional `/apple-touch-icon.png`, then `/favicon.ico` — and a candidate counts only
//!    when it decodes to **at least [`MIN_SOURCE_SIDE`] (64) px** on its shorter side. A 16 px
//!    or 32 px favicon is never enlarged; a host with nothing larger has no icon (the apps draw
//!    the host's letter). `og:image` is not an icon (a hero photo even when square-ish).
//! 2. **Scale down, never up:** the chosen picture is centre-cropped square and resampled with
//!    Lanczos3 to at most [`ICON_SIDE`] (128) px — 36 pt at 3× on an iPhone is 108 px, so the
//!    apps always downscale from here.
//! 3. **One fetch per host**, through the unfurl module's pinned-DNS fetcher ([`unfurl::fetch_bounded`]:
//!    public host names only, resolved before connecting and pinned, private / link-local /
//!    metadata ranges refused on every hop, ≤ 3 redirects, 512 KB page / 5 MB picture caps,
//!    10 s budget, the relay's own User-Agent, no cookies, no credentials). Cached on disk
//!    under `<data>/site-icons/v<VERSION>/<host>.png|.none` with memory in front — found
//!    [`FOUND_TTL_MS`] (7 d), missing [`MISSING_TTL_MS`] (1 d); bumping [`VERSION`] retires
//!    every cached icon at once.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use serde::Deserialize;
use serde_json::{json, Value};

use arbos_core::ids::now_ms;

use crate::auth::Principal;
use crate::link_icons::{absolute, attr, public_domain, tags};
use crate::signin::api_error;
use crate::state::AppState;
use crate::unfurl::{Fetcher, IMAGE_MAX_BYTES, PAGE_MAX_BYTES};

/// The side of the stored PNG: the largest tile on either app (36 pt at 3×) is 108 px.
pub const ICON_SIDE: u32 = 128;
/// A candidate shorter than this on its shorter side is not an icon worth showing.
pub const MIN_SOURCE_SIDE: u32 = 64;
/// The cache's layout version: part of the on-disk path, so a change of rules (or of
/// [`ICON_SIDE`]) starts every host afresh.
pub const VERSION: u32 = 2;
/// A found icon is served from the cache this long before the host is asked again.
pub const FOUND_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// A host without an icon is not asked again for this long.
pub const MISSING_TTL_MS: i64 = 24 * 60 * 60 * 1000;

/// A resolved icon: a square PNG of `side` ≤ [`ICON_SIDE`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Icon {
    pub png: Bytes,
    pub side: u32,
}

impl Icon {
    /// The `siteIcon` object of a `link_preview` part for this icon stored under `blob_ref`.
    pub fn site_icon_json(&self, blob_ref: &str) -> Value {
        json!({
            "ref": blob_ref,
            "width": self.side,
            "height": self.side,
            "mime": "image/png",
        })
    }
}

/// What the cache knows about a host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cached {
    pub fetched_at: i64,
    pub icon: Option<Icon>,
}

impl Cached {
    pub fn fresh(&self, now: i64) -> bool {
        let ttl = if self.icon.is_some() {
            FOUND_TTL_MS
        } else {
            MISSING_TTL_MS
        };
        now - self.fetched_at < ttl
    }
}

#[derive(Deserialize)]
pub struct IconQuery {
    pub host: String,
}

/// `GET /unfurl/icon?host=` for any signed-in principal: the host's icon as a PNG (200), or
/// 204 when it has none ≥ 64 px — the apps then draw the host's letter.
pub async fn get(
    State(state): State<AppState>,
    _principal: Principal,
    Query(q): Query<IconQuery>,
) -> Response {
    let Some(host) = public_domain(&q.host) else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_host",
            "host must be a public host name",
        );
    };
    respond(resolve(&state, &host).await)
}

/// The response for a resolved icon: the PNG with a week's cache, or 204 with a day's.
pub fn respond(icon: Option<Icon>) -> Response {
    match icon {
        Some(icon) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "image/png".to_owned()),
                (
                    header::CACHE_CONTROL,
                    format!("public, max-age={}", FOUND_TTL_MS / 1000),
                ),
            ],
            icon.png,
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

/// The icon for `host` (a public host name, see [`public_domain`]): the cache, else one fetch
/// through the pinned fetcher, remembered either way.
pub async fn resolve(state: &AppState, host: &str) -> Option<Icon> {
    let now = now_ms();
    if let Some(c) = lookup(state, host, now).await {
        return c.icon;
    }
    let fetch = Fetcher::new();
    let icon = resolve_with(&fetch, &format!("https://{host}")).await;
    remember(
        state,
        host,
        &Cached {
            fetched_at: now,
            icon: icon.clone(),
        },
    )
    .await;
    icon
}

/// The icon for `host`, copied into blob slot `slot` for a `link_preview` part: returns the
/// part's `siteIcon` object, or `None` when the host has no icon ≥ 64 px. The blob key is
/// derived from the host and the cache version, so a chat with many links to one host stores
/// the picture once.
pub async fn site_icon_for(state: &AppState, slot: &str, host: &str) -> Option<Value> {
    let host = public_domain(host)?;
    let icon = resolve(state, &host).await?;
    let key = blob_key(&host);
    crate::blobs::store(state, slot, &key, "image/png", &icon.png)
        .await
        .ok()?;
    Some(icon.site_icon_json(&key))
}

/// `si<VERSION>-<host with dots as dashes>.png` — [`crate::blobs::safe_key`] allows `-_.`.
pub fn blob_key(host: &str) -> String {
    let safe: String = host
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("si{VERSION}-{safe}.png")
}

/// What a page declares, in the order the resolver tries it.
#[derive(Debug, PartialEq, Eq)]
pub struct Declared {
    /// `href` of the largest `apple-touch-icon` / `apple-touch-icon-precomposed`.
    pub touch: Option<String>,
    /// `href` of the largest raster `icon` / `shortcut icon` (SVG skipped: not decodable here).
    pub icon: Option<String>,
}

/// The icon links a page declares. Sizes come from `sizes="WxH"` (`any` counts as 0); an
/// apple-touch-icon without one is taken as 180, a plain icon without one as 32.
pub fn declared(html: &str) -> Declared {
    let mut touch: Option<(u32, String)> = None;
    let mut icon: Option<(u32, String)> = None;
    for tag in tags(html, "link") {
        let Some(rel) = attr(&tag, "rel") else {
            continue;
        };
        let rel = rel.to_ascii_lowercase();
        let rels: Vec<&str> = rel.split_whitespace().collect();
        let is_touch = rels
            .iter()
            .any(|r| *r == "apple-touch-icon" || *r == "apple-touch-icon-precomposed");
        let is_icon = rels.contains(&"icon");
        if !is_touch && !is_icon {
            continue;
        }
        let Some(href) = attr(&tag, "href") else {
            continue;
        };
        let href = href.trim();
        if href.is_empty() {
            continue;
        }
        let lower = href.to_ascii_lowercase();
        let svg = lower.ends_with(".svg")
            || lower.contains(".svg?")
            || attr(&tag, "type").is_some_and(|t| t.to_ascii_lowercase().contains("svg"));
        if svg {
            continue;
        }
        // `sizes="16x16 192x192"` → 192; `sizes="any"` (an SVG-style declaration) → 0; no
        // attribute → the convention: 180 for a touch icon, 32 for a plain one.
        let size = match attr(&tag, "sizes") {
            Some(s) => s
                .split_whitespace()
                .filter_map(|wh| wh.split(['x', 'X']).next()?.trim().parse::<u32>().ok())
                .max()
                .unwrap_or(0),
            None if is_touch => 180,
            None => 32,
        };
        let slot = if is_touch { &mut touch } else { &mut icon };
        if slot.as_ref().is_none_or(|(s, _)| size > *s) {
            *slot = Some((size, href.to_owned()));
        }
    }
    Declared {
        touch: touch.map(|(_, h)| h),
        icon: icon.map(|(_, h)| h),
    }
}

/// The candidate URLs for a host, in order, given its page (final URL + HTML) when it answered.
pub fn candidates(base: &str, page: Option<(&str, &str)>) -> Vec<String> {
    let mut out = Vec::new();
    let root = base.trim_end_matches('/');
    if let Some((final_url, html)) = page {
        let d = declared(html);
        for href in [d.touch, d.icon].into_iter().flatten() {
            if let Some(u) = absolute(final_url, &href) {
                if !out.contains(&u) {
                    out.push(u);
                }
            }
        }
    }
    for conventional in [
        format!("{root}/apple-touch-icon.png"),
        format!("{root}/favicon.ico"),
    ] {
        if !out.contains(&conventional) {
            out.push(conventional);
        }
    }
    out
}

/// Resolve through `fetch` for the site at `base` (`scheme://host[:port]`). `None` when
/// nothing decodes to ≥ [`MIN_SOURCE_SIDE`].
pub async fn resolve_with(fetch: &Fetcher, base: &str) -> Option<Icon> {
    let page = fetch
        .get(
            base,
            "text/html,application/xhtml+xml;q=0.9,*/*;q=0.5",
            PAGE_MAX_BYTES,
        )
        .await
        .filter(|f| (200..300).contains(&f.status))
        .map(|f| (f.final_url, String::from_utf8_lossy(&f.body).into_owned()));
    let page_ref = page.as_ref().map(|(u, h)| (u.as_str(), h.as_str()));
    for url in candidates(base, page_ref) {
        let Some(f) = fetch.get(&url, "image/*,*/*;q=0.5", IMAGE_MAX_BYTES).await else {
            continue;
        };
        if !(200..300).contains(&f.status) {
            continue;
        }
        if let Some(icon) = normalize(&f.body) {
            return Some(icon);
        }
    }
    None
}

/// Decode any supported picture (PNG, JPEG, ICO with its largest frame, GIF, WebP); refuse one
/// under [`MIN_SOURCE_SIDE`] on its shorter side; centre-crop square and scale **down** to at
/// most [`ICON_SIDE`] with Lanczos3 (never up); PNG out.
pub fn normalize(bytes: &[u8]) -> Option<Icon> {
    let img = crate::image_sniff::decode(bytes).ok()?;
    let (w, h) = (img.width(), img.height());
    let short = w.min(h);
    if short < MIN_SOURCE_SIDE {
        return None;
    }
    let side = short.min(ICON_SIDE);
    let square = if w == h && w == side {
        img
    } else {
        img.resize_to_fill(side, side, image::imageops::FilterType::Lanczos3)
    };
    let mut out = Cursor::new(Vec::new());
    square
        .to_rgba8()
        .write_to(&mut out, image::ImageFormat::Png)
        .ok()?;
    Some(Icon {
        png: Bytes::from(out.into_inner()),
        side,
    })
}

// ---- cache: memory in front of `<data>/site-icons/v<VERSION>/<host>.png|.none` -------------

fn memory() -> &'static Mutex<HashMap<String, Cached>> {
    static MEM: OnceLock<Mutex<HashMap<String, Cached>>> = OnceLock::new();
    MEM.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_dir(state: &AppState) -> PathBuf {
    state
        .cfg
        .data_dir
        .join("site-icons")
        .join(format!("v{VERSION}"))
}

async fn lookup(state: &AppState, host: &str, now: i64) -> Option<Cached> {
    if let Some(c) = memory().lock().unwrap().get(host).cloned() {
        if c.fresh(now) {
            return Some(c);
        }
    }
    let dir = cache_dir(state);
    let png = dir.join(format!("{host}.png"));
    let none = dir.join(format!("{host}.none"));
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
    let icon = if found {
        let bytes = tokio::fs::read(&path).await.ok()?;
        let side = crate::image_sniff::decode(&bytes).ok()?.width();
        Some(Icon {
            png: Bytes::from(bytes),
            side,
        })
    } else {
        None
    };
    let c = Cached { fetched_at, icon };
    if !c.fresh(now) {
        return None;
    }
    memory().lock().unwrap().insert(host.to_owned(), c.clone());
    Some(c)
}

async fn remember(state: &AppState, host: &str, c: &Cached) {
    memory().lock().unwrap().insert(host.to_owned(), c.clone());
    let dir = cache_dir(state);
    let _ = tokio::fs::create_dir_all(&dir).await;
    let png = dir.join(format!("{host}.png"));
    let none = dir.join(format!("{host}.none"));
    match &c.icon {
        Some(icon) => {
            let _ = tokio::fs::remove_file(&none).await;
            let _ = tokio::fs::write(&png, &icon.png).await;
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

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([20, 20, 20, 255]));
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    fn ico(side: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(side, side, image::Rgba([10, 20, 200, 255]));
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Ico).unwrap();
        out.into_inner()
    }

    fn side_of(icon: &Icon) -> (u32, u32) {
        let back = image::load_from_memory(&icon.png).unwrap();
        (back.width(), back.height())
    }

    #[test]
    fn a_page_declares_its_touch_icon_and_its_largest_raster_icon() {
        let html = r#"<html><head>
            <link rel="icon" type="image/svg+xml" href="/mark.svg">
            <link rel="icon" sizes="32x32" href="/i32.png">
            <link rel="shortcut icon" href="/favicon.ico">
            <link rel="icon" type="image/png" sizes="16x16 192x192" href="/i192.png">
            <link rel="apple-touch-icon" sizes="152x152" href="/t152.png">
            <link rel="apple-touch-icon-precomposed" href="/t180.png">
            <link rel="stylesheet" href="/x.css"></head></html>"#;
        assert_eq!(
            declared(html),
            Declared {
                touch: Some("/t180.png".into()),
                icon: Some("/i192.png".into()),
            }
        );
        assert_eq!(
            declared("<html></html>"),
            Declared {
                touch: None,
                icon: None
            }
        );
        // `sizes="any"` is an SVG-style declaration: counts as the smallest.
        assert_eq!(
            declared(
                r#"<link rel="icon" sizes="any" href="/a.png"><link rel="icon" href="/b.png">"#
            )
            .icon,
            Some("/b.png".into())
        );
    }

    #[test]
    fn candidates_run_touch_then_icon_then_the_conventional_paths_without_repeats() {
        let html = r#"<link rel="apple-touch-icon" href="/apple-touch-icon.png"><link rel="icon" sizes="64x64" href="//cdn.example.com/i.png">"#;
        assert_eq!(
            candidates("https://example.com", Some(("https://example.com/", html))),
            vec![
                "https://example.com/apple-touch-icon.png",
                "https://cdn.example.com/i.png",
                "https://example.com/favicon.ico",
            ]
        );
        assert_eq!(
            candidates("https://example.com/", None),
            vec![
                "https://example.com/apple-touch-icon.png",
                "https://example.com/favicon.ico",
            ]
        );
    }

    #[test]
    fn pictures_under_64_are_refused_and_larger_ones_scale_down_never_up() {
        assert!(
            normalize(&png(16, 16)).is_none(),
            "a 16 px favicon is never enlarged"
        );
        assert!(normalize(&png(32, 32)).is_none());
        assert!(
            normalize(&png(200, 48)).is_none(),
            "the shorter side counts"
        );
        let exact = normalize(&png(64, 64)).expect("64 is the floor");
        assert_eq!((side_of(&exact), exact.side), ((64, 64), 64));
        let touch = normalize(&png(180, 180)).expect("apple-touch-icon");
        assert_eq!((side_of(&touch), touch.side), ((128, 128), 128));
        let wide = normalize(&png(300, 100)).expect("centre-cropped square");
        assert_eq!(side_of(&wide), (100, 100));
        let big_ico = normalize(&ico(256)).expect("a 256 px .ico decodes");
        assert_eq!(side_of(&big_ico), (128, 128));
        assert!(
            normalize(&ico(32)).is_none(),
            "the classic 32 px .ico is too small"
        );
        assert!(normalize(b"<html>not an image</html>").is_none());
        assert_eq!(
            image::guess_format(&touch.png).unwrap(),
            image::ImageFormat::Png
        );
    }

    #[test]
    fn site_icon_json_and_blob_keys() {
        let icon = normalize(&png(128, 128)).unwrap();
        assert_eq!(
            icon.site_icon_json("si2-github-com.png"),
            json!({ "ref": "si2-github-com.png", "width": 128, "height": 128, "mime": "image/png" })
        );
        assert_eq!(blob_key("github.com"), "si2-github-com.png");
        assert_eq!(blob_key("www.nypost.com"), "si2-www-nypost-com.png");
        assert!(crate::blobs::safe_key(&blob_key("a-b.c_d.com")));
    }

    #[test]
    fn cache_entries_age_by_their_kind() {
        let found = Cached {
            fetched_at: 0,
            icon: normalize(&png(64, 64)),
        };
        let missing = Cached {
            fetched_at: 0,
            icon: None,
        };
        assert!(found.fresh(FOUND_TTL_MS - 1));
        assert!(!found.fresh(FOUND_TTL_MS));
        assert!(missing.fresh(MISSING_TTL_MS - 1));
        assert!(!missing.fresh(MISSING_TTL_MS));
    }

    /// Jacob's case: a site whose page declares only a 32 px icon and whose /favicon.ico is
    /// 16 px, but which serves /apple-touch-icon.png at 180 → the touch icon, downscaled to
    /// 128, and the small ones never used; a site with a 192 px declared icon and no touch
    /// icon → that icon; a site with only a 16 px favicon → none; nothing listening → none.
    #[tokio::test]
    async fn resolves_the_largest_real_icon_and_never_a_small_favicon() {
        let hits = Arc::new(Mutex::new(Vec::<String>::new()));
        let h = hits.clone();
        let app = Router::new()
            .route("/", axum_get({
                let h = h.clone();
                move || {
                    h.lock().unwrap().push("/".into());
                    async {
                        axum::response::Html(
                            r#"<html><head><link rel="icon" sizes="32x32" href="/i32.png"><meta property="og:image" content="/hero.png"></head></html>"#,
                        )
                    }
                }
            }))
            .route("/i32.png", axum_get({
                let h = h.clone();
                move || { h.lock().unwrap().push("/i32.png".into()); async { ([(header::CONTENT_TYPE, "image/png")], png(32, 32)) } }
            }))
            .route("/apple-touch-icon.png", axum_get({
                let h = h.clone();
                move || { h.lock().unwrap().push("/apple-touch-icon.png".into()); async { ([(header::CONTENT_TYPE, "image/png")], png(180, 180)) } }
            }))
            .route("/favicon.ico", axum_get({
                let h = h.clone();
                move || { h.lock().unwrap().push("/favicon.ico".into()); async { ([(header::CONTENT_TYPE, "image/x-icon")], ico(16)) } }
            }))
            .route("/hero.png", axum_get({
                let h = h.clone();
                move || { h.lock().unwrap().push("/hero.png".into()); async { ([(header::CONTENT_TYPE, "image/png")], png(1200, 630)) } }
            }))
            .route("/declared/", axum_get(|| async {
                axum::response::Html(r#"<link rel="icon" type="image/png" sizes="192x192" href="/declared/i192.png">"#)
            }))
            .route("/declared/i192.png", axum_get(|| async { ([(header::CONTENT_TYPE, "image/png")], png(192, 192)) }))
            .route("/small/", axum_get(|| async { axum::response::Html("<html></html>") }))
            .route("/small/favicon.ico", axum_get(|| async { ([(header::CONTENT_TYPE, "image/x-icon")], ico(16)) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let base = format!("http://{addr}");
        let fetch = Fetcher::allowing(&addr.to_string());

        let icon = resolve_with(&fetch, &base).await.expect("the touch icon");
        assert_eq!(side_of(&icon), (128, 128));
        assert_eq!(
            hits.lock().unwrap().as_slice(),
            ["/", "/i32.png", "/apple-touch-icon.png"],
            "the 32 px declared icon is fetched, refused, and the conventional touch icon wins; the hero and the favicon are never asked for"
        );

        let fetch = Fetcher::allowing(&addr.to_string());
        let declared = resolve_with(&fetch, &format!("{base}/declared/"))
            .await
            .expect("the 192 px icon");
        assert_eq!(side_of(&declared), (128, 128));

        let fetch = Fetcher::allowing(&addr.to_string());
        assert!(
            resolve_with(&fetch, &format!("{base}/small/"))
                .await
                .is_none(),
            "only a 16 px favicon → no icon"
        );

        let fetch = Fetcher::allowing("127.0.0.1:1");
        assert!(resolve_with(&fetch, "http://127.0.0.1:1").await.is_none());

        // The cache: remember then lookup, by host and version; a stale entry is not returned;
        // a miss is remembered as `.none`.
        let state = crate::state::test_support::test_state(crate::state::AuthMode::Github);
        let c = Cached {
            fetched_at: now_ms(),
            icon: Some(icon.clone()),
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
        assert!(cache_dir(&state).join("example.test.png").exists());
        assert!(cache_dir(&state).ends_with("site-icons/v2"));
        let miss = Cached {
            fetched_at: now_ms(),
            icon: None,
        };
        remember(&state, "small.test", &miss).await;
        assert_eq!(
            lookup(&state, "small.test", now_ms()).await.map(|c| c.icon),
            Some(None)
        );
        assert!(cache_dir(&state).join("small.test.none").exists());

        // The part's `siteIcon`: the cached icon copied into the chat's slot.
        let v = site_icon_for(&state, "people:chat1", "example.test")
            .await
            .expect("site icon");
        assert_eq!(v["ref"], "si2-example-test.png");
        assert_eq!(v["width"], 128);
        assert_eq!(v["mime"], "image/png");
        let stored = crate::blobs::read(&state, "people:chat1", "si2-example-test.png")
            .await
            .expect("blob");
        assert_eq!(stored, icon.png.to_vec());
        assert!(site_icon_for(&state, "people:chat1", "small.test")
            .await
            .is_none());
        assert!(site_icon_for(&state, "people:chat1", "localhost")
            .await
            .is_none());
    }
}
