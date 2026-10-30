//! GIF search for the apps' picker (ARB-544, Jacob 2026-10-28): the relay is the only thing
//! that talks to KLIPY. The key lives in the relay's environment (`KLIPY_API_KEY`, from the
//! Arbos vault item "KLIPY — Arbos GIF API key") and never reaches a device.
//!
//! What the provider's terms allow, and how this module keeps to them:
//!
//! - **No stored copies of their GIFs.** `GET /gif/media?u=` is a pass-through: the bytes
//!   stream from `static.klipy.com` to the app and nothing is written to disk. The response
//!   says `Cache-Control: no-store`. Only KLIPY hosts are proxied (no open proxy).
//! - **Branding** is the apps' job ("Search KLIPY" placeholder, "Powered by KLIPY" line).
//! - **Testing key = 100 calls per hour for the whole app.** Search / trending / categories
//!   answers are kept in memory for [`SEARCH_TTL`] (the terms forbid a *media* cache, not a
//!   short results cache), identical in-flight queries share one upstream call, and the relay
//!   stops itself at [`HOURLY_BUDGET`] calls per rolling hour. Over budget, or an upstream
//!   429, answers `429 gif_busy` ("GIFs are busy, try again shortly.") with `Retry-After`;
//!   the apps show that line and do not retry on their own.
//! - **Content rating:** `contentfilter` is the Tenor-compatible scale KLIPY documents
//!   (`high` = G, `medium` = PG, `low` = PG-13, `off` = everything); Jacob's cap is PG-13, so
//!   the default is `low` (`KLIPY_CONTENT_FILTER` overrides).
//!
//! Without a key every endpoint answers `503 gif_unavailable`; the apps show "GIFs are not set
//! up yet" instead of an empty grid.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::Query;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex as AsyncMutex;

use crate::auth::Principal;
use crate::signin::api_error;

/// Search / trending answers are reused this long for the same query.
pub const SEARCH_TTL: Duration = Duration::from_secs(10 * 60);
/// The category list changes rarely.
pub const CATEGORIES_TTL: Duration = Duration::from_secs(6 * 60 * 60);
/// Upstream API calls the relay allows itself per rolling hour (the testing key is 100).
pub const HOURLY_BUDGET: usize = 90;
/// A GIF larger than this is not relayed (the apps cap decoding anyway).
pub const MEDIA_MAX_BYTES: u64 = 15 * 1024 * 1024;
/// Results per page; KLIPY allows up to 50.
pub const PAGE_SIZE: u32 = 24;
/// Query text longer than this is cut (KLIPY refuses over 50).
pub const QUERY_MAX_CHARS: usize = 50;
pub const USER_AGENT: &str = "Arbos relay (+https://arbos.life)";

const KLIPY_API: &str = "https://api.klipy.com/v2";
/// Hosts whose media the pass-through will relay.
const MEDIA_HOSTS: &[&str] = &["static.klipy.com", "media.klipy.com", "klipy.com"];

/// How the relay reaches the provider: the key and base URL from the environment (tests
/// point the base at a local server).
#[derive(Clone, Debug)]
pub struct Provider {
    pub key: Option<String>,
    pub api_base: String,
    pub content_filter: String,
    pub budget: usize,
}

impl Provider {
    pub fn from_env() -> Self {
        Self {
            key: std::env::var("KLIPY_API_KEY")
                .ok()
                .map(|k| k.trim().to_owned())
                .filter(|k| !k.is_empty()),
            api_base: std::env::var("KLIPY_API_BASE")
                .ok()
                .filter(|b| !b.is_empty())
                .unwrap_or_else(|| KLIPY_API.to_owned()),
            content_filter: std::env::var("KLIPY_CONTENT_FILTER")
                .ok()
                .filter(|f| matches!(f.as_str(), "off" | "low" | "medium" | "high"))
                .unwrap_or_else(|| "low".to_owned()),
            budget: std::env::var("KLIPY_HOURLY_BUDGET")
                .ok()
                .and_then(|b| b.parse().ok())
                .unwrap_or(HOURLY_BUDGET),
        }
    }
}

struct Cached {
    at: Instant,
    ttl: Duration,
    body: Value,
}

/// Module state: provider settings, the results cache, in-flight coalescing locks and the
/// rolling-hour call ledger. One per process (`OnceLock`), so no `AppState` plumbing.
pub struct Gifs {
    provider: Mutex<Provider>,
    http: reqwest::Client,
    cache: Mutex<HashMap<String, Cached>>,
    inflight: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    calls: Mutex<VecDeque<Instant>>,
}

static GIFS: OnceLock<Gifs> = OnceLock::new();

pub fn gifs() -> &'static Gifs {
    GIFS.get_or_init(|| Gifs::new(Provider::from_env()))
}

/// Error the apps show as one short sentence.
#[derive(Debug, PartialEq, Eq)]
pub enum GifError {
    /// No key on this relay.
    Unavailable,
    /// Our budget or the provider's rate limit.
    Busy { retry_after_s: u64 },
    /// The provider answered badly or not at all.
    Upstream,
}

impl GifError {
    fn into_response(self) -> Response {
        match self {
            GifError::Unavailable => api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "gif_unavailable",
                "GIFs are not set up on this server yet.",
            ),
            GifError::Busy { retry_after_s } => {
                let mut r = api_error(
                    StatusCode::TOO_MANY_REQUESTS,
                    "gif_busy",
                    "GIFs are busy, try again shortly.",
                );
                r.headers_mut().insert(
                    header::RETRY_AFTER,
                    header::HeaderValue::from_str(&retry_after_s.to_string())
                        .unwrap_or(header::HeaderValue::from_static("60")),
                );
                r
            }
            GifError::Upstream => api_error(
                StatusCode::BAD_GATEWAY,
                "gif_upstream",
                "GIFs could not be loaded. Try again.",
            ),
        }
    }
}

impl Gifs {
    pub fn new(provider: Provider) -> Self {
        Self {
            provider: Mutex::new(provider),
            http: reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(20))
                .build()
                .unwrap_or_default(),
            cache: Mutex::new(HashMap::new()),
            inflight: Mutex::new(HashMap::new()),
            calls: Mutex::new(VecDeque::new()),
        }
    }

    fn provider(&self) -> Provider {
        self.provider.lock().unwrap().clone()
    }

    /// Take one call from the rolling-hour budget; `None` when it is spent, with the
    /// seconds until the oldest call ages out.
    fn take_budget(&self, budget: usize) -> Result<(), u64> {
        let now = Instant::now();
        let mut calls = self.calls.lock().unwrap();
        while calls
            .front()
            .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(3600))
        {
            calls.pop_front();
        }
        if calls.len() >= budget {
            let wait = calls
                .front()
                .map(|t| 3600u64.saturating_sub(now.duration_since(*t).as_secs()))
                .unwrap_or(60)
                .max(5);
            return Err(wait);
        }
        calls.push_back(now);
        Ok(())
    }

    /// Calls made in the last hour (for tests and the admin line).
    pub fn calls_last_hour(&self) -> usize {
        let now = Instant::now();
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|t| now.duration_since(**t) <= Duration::from_secs(3600))
            .count()
    }

    fn cached(&self, key: &str) -> Option<Value> {
        let mut cache = self.cache.lock().unwrap();
        let now = Instant::now();
        cache.retain(|_, c| now.duration_since(c.at) < c.ttl);
        cache.get(key).map(|c| c.body.clone())
    }

    fn remember(&self, key: &str, body: Value, ttl: Duration) {
        self.cache.lock().unwrap().insert(
            key.to_owned(),
            Cached {
                at: Instant::now(),
                ttl,
                body,
            },
        );
    }

    /// One upstream call, cached under `cache_key`; identical concurrent calls wait for the
    /// first and read its answer.
    async fn call(
        &self,
        cache_key: String,
        path: &str,
        params: &[(&str, String)],
        ttl: Duration,
    ) -> Result<Value, GifError> {
        let provider = self.provider();
        let Some(key) = provider.key.clone() else {
            return Err(GifError::Unavailable);
        };
        if let Some(v) = self.cached(&cache_key) {
            return Ok(v);
        }
        let lock = self
            .inflight
            .lock()
            .unwrap()
            .entry(cache_key.clone())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone();
        let _guard = lock.lock().await;
        if let Some(v) = self.cached(&cache_key) {
            self.inflight.lock().unwrap().remove(&cache_key);
            return Ok(v);
        }
        let result = async {
            self.take_budget(provider.budget)
                .map_err(|retry_after_s| GifError::Busy { retry_after_s })?;
            let mut query: Vec<(&str, String)> = vec![
                ("key", key),
                ("contentfilter", provider.content_filter.clone()),
                ("country", "US".to_owned()),
                ("locale", "en_US".to_owned()),
            ];
            query.extend(params.iter().cloned());
            let resp = self
                .http
                .get(format!(
                    "{}/{path}",
                    provider.api_base.trim_end_matches('/')
                ))
                .query(&query)
                .send()
                .await
                .map_err(|e| {
                    tracing::warn!("klipy {path}: {e}");
                    GifError::Upstream
                })?;
            let status = resp.status();
            if status == StatusCode::TOO_MANY_REQUESTS {
                let retry = resp
                    .headers()
                    .get(header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(120);
                return Err(GifError::Busy {
                    retry_after_s: retry,
                });
            }
            if !status.is_success() {
                tracing::warn!("klipy {path}: http {status}");
                return Err(GifError::Upstream);
            }
            resp.json::<Value>().await.map_err(|e| {
                tracing::warn!("klipy {path}: bad json: {e}");
                GifError::Upstream
            })
        }
        .await;
        if let Ok(v) = &result {
            self.remember(&cache_key, v.clone(), ttl);
        }
        self.inflight.lock().unwrap().remove(&cache_key);
        result
    }
}

/// The app-facing shape of one result: a preview for the grid and the GIF the message
/// carries, both as URLs on the provider's host (the apps fetch them through `/gif/media`).
pub fn item_from_klipy(v: &Value) -> Option<Value> {
    // Ads (an `ad` type) are never shown — the account runs with ads off, and a stray one
    // must not reach a chat.
    if v.get("type").and_then(Value::as_str) == Some("ad") {
        return None;
    }
    let id = v.get("id")?.as_str()?.to_owned();
    let formats = v.get("media_formats")?.as_object()?;
    let pick = |names: &[&str]| -> Option<(String, u32, u32, u64)> {
        for n in names {
            if let Some(f) = formats.get(*n) {
                let url = f.get("url")?.as_str()?.to_owned();
                let dims = f.get("dims").and_then(Value::as_array);
                let w = dims
                    .and_then(|d| d.first())
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32;
                let h = dims
                    .and_then(|d| d.get(1))
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32;
                let size = f.get("size").and_then(Value::as_u64).unwrap_or(0);
                return Some((url, w, h, size));
            }
        }
        None
    };
    let (preview_url, pw, ph, _) = pick(&["tinygif", "nanogif", "gif"])?;
    let (url, w, h, size) = pick(&["gif", "mediumgif", "tinygif"])?;
    if !media_url_allowed(&url) || !media_url_allowed(&preview_url) {
        return None;
    }
    let title = v
        .get("title")
        .and_then(Value::as_str)
        .filter(|t| !t.trim().is_empty())
        .or_else(|| v.get("content_description").and_then(Value::as_str))
        .unwrap_or("GIF")
        .to_owned();
    let page_url = v
        .get("itemurl")
        .and_then(Value::as_str)
        .unwrap_or("https://klipy.com")
        .to_owned();
    Some(json!({
        "id": id,
        "title": title,
        "width": w,
        "height": h,
        "size": size,
        "url": url,
        "preview": { "url": preview_url, "width": pw, "height": ph },
        "pageUrl": page_url,
    }))
}

fn page_from_klipy(v: &Value) -> Value {
    let items: Vec<Value> = v
        .get("results")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(item_from_klipy).collect())
        .unwrap_or_default();
    let next = v
        .get("next")
        .and_then(Value::as_str)
        .filter(|n| !n.is_empty())
        .map(str::to_owned);
    json!({ "items": items, "next": next, "provider": "klipy" })
}

/// A media URL the pass-through will relay: https, on a KLIPY host, no credentials.
pub fn media_url_allowed(raw: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(raw) else {
        return false;
    };
    if u.scheme() != "https" || u.username() != "" || u.password().is_some() {
        return false;
    }
    let Some(host) = u.host_str() else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    MEDIA_HOSTS
        .iter()
        .any(|h| host == *h || host.ends_with(&format!(".{h}")))
}

/// The search text as sent upstream: trimmed, cut to the provider's limit.
pub fn normalize_query(q: &str) -> String {
    let q = q.trim();
    q.chars().take(QUERY_MAX_CHARS).collect::<String>()
}

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: String,
    #[serde(default)]
    pub pos: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct PageQuery {
    #[serde(default)]
    pub pos: Option<String>,
}

#[derive(Deserialize)]
pub struct MediaQuery {
    pub u: String,
}

#[derive(Deserialize)]
pub struct ShareQuery {
    pub id: String,
    #[serde(default)]
    pub q: Option<String>,
}

fn media_filter() -> String {
    "gif,mediumgif,tinygif,nanogif".to_owned()
}

/// `GET /gif/search?q=&pos=` for any signed-in principal.
pub async fn search(_principal: Principal, Query(q): Query<SearchQuery>) -> Response {
    let text = normalize_query(&q.q);
    if text.is_empty() {
        return api_error(StatusCode::BAD_REQUEST, "bad_query", "q is required");
    }
    let pos = q.pos.unwrap_or_default();
    let cache_key = format!("search:{}:{pos}", text.to_lowercase());
    let mut params = vec![
        ("q", text),
        ("limit", PAGE_SIZE.to_string()),
        ("media_filter", media_filter()),
    ];
    if !pos.is_empty() {
        params.push(("pos", pos));
    }
    match gifs().call(cache_key, "search", &params, SEARCH_TTL).await {
        Ok(v) => Json(page_from_klipy(&v)).into_response(),
        Err(e) => e.into_response(),
    }
}

/// `GET /gif/trending?pos=` — the grid before the user types.
pub async fn trending(_principal: Principal, Query(q): Query<PageQuery>) -> Response {
    let pos = q.pos.unwrap_or_default();
    let cache_key = format!("trending:{pos}");
    let mut params = vec![
        ("limit", PAGE_SIZE.to_string()),
        ("media_filter", media_filter()),
    ];
    if !pos.is_empty() {
        params.push(("pos", pos));
    }
    match gifs()
        .call(cache_key, "featured", &params, SEARCH_TTL)
        .await
    {
        Ok(v) => Json(page_from_klipy(&v)).into_response(),
        Err(e) => e.into_response(),
    }
}

/// `GET /gif/categories` — the chips row (search terms).
pub async fn categories(_principal: Principal) -> Response {
    match gifs()
        .call("categories".to_owned(), "categories", &[], CATEGORIES_TTL)
        .await
    {
        Ok(v) => {
            let terms: Vec<String> = v
                .get("tags")
                .and_then(Value::as_array)
                .map(|t| {
                    t.iter()
                        .filter_map(|x| x.get("searchterm").and_then(Value::as_str))
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            Json(json!({ "terms": terms, "provider": "klipy" })).into_response()
        }
        Err(e) => e.into_response(),
    }
}

/// `POST /gif/share?id=&q=` — the provider's share signal (tunes its ranking). Costs one
/// budget call, so it is only forwarded when the budget has room; never an error for the app.
pub async fn share(_principal: Principal, Query(q): Query<ShareQuery>) -> Response {
    let g = gifs();
    let provider = g.provider();
    let Some(key) = provider.key else {
        return StatusCode::NO_CONTENT.into_response();
    };
    // Keep most of the budget for searches.
    if g.calls_last_hour() + 10 >= provider.budget || g.take_budget(provider.budget).is_err() {
        return StatusCode::NO_CONTENT.into_response();
    }
    let mut params = vec![("key", key), ("id", q.id)];
    if let Some(term) = q.q.filter(|t| !t.trim().is_empty()) {
        params.push(("q", normalize_query(&term)));
    }
    let _ = g
        .http
        .get(format!(
            "{}/registershare",
            provider.api_base.trim_end_matches('/')
        ))
        .query(&params)
        .send()
        .await;
    StatusCode::NO_CONTENT.into_response()
}

/// `GET /gif/media?u=` — the pass-through for a KLIPY media URL: streamed, never stored,
/// `no-store` to the app. Only the provider's hosts; anything else is 400.
pub async fn media(_principal: Principal, Query(q): Query<MediaQuery>) -> Response {
    if !media_url_allowed(&q.u) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_media_url",
            "only the GIF provider's media can be relayed",
        );
    }
    relay_media(&q.u).await
}

/// Stream `url` through, never storing it. The caller has already decided who may ask and
/// that the host is the provider's ([`media_url_allowed`]); outside agents reach it through
/// `GET /asp/files` for a GIF the relay keeps no copy of.
pub async fn relay_media(url: &str) -> Response {
    let g = gifs();
    let resp = match g.http.get(url).send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("gif media: {e}");
            return GifError::Upstream.into_response();
        }
    };
    let status = resp.status();
    if !status.is_success() {
        return if status == StatusCode::NOT_FOUND {
            api_error(
                StatusCode::NOT_FOUND,
                "gif_gone",
                "This GIF is no longer available.",
            )
        } else {
            GifError::Upstream.into_response()
        };
    }
    if resp.content_length().is_some_and(|l| l > MEDIA_MAX_BYTES) {
        return api_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "gif_too_large",
            "This GIF is too large to show.",
        );
    }
    let mime = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .filter(|m| m.starts_with("image/") || m.starts_with("video/"))
        .unwrap_or("image/gif")
        .to_owned();
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_str(&mime)
            .unwrap_or(header::HeaderValue::from_static("image/gif")),
    );
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("private, no-store"),
    );
    if let Some(len) = resp.content_length() {
        headers.insert(header::CONTENT_LENGTH, header::HeaderValue::from(len));
    }
    (
        StatusCode::OK,
        headers,
        Body::from_stream(resp.bytes_stream()),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use axum::Router;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn media_urls_are_limited_to_the_provider() {
        assert!(media_url_allowed(
            "https://static.klipy.com/ii/abc/ec/f3/p30ORxL1.gif"
        ));
        assert!(media_url_allowed("https://cdn.static.klipy.com/x.gif"));
        assert!(!media_url_allowed("http://static.klipy.com/x.gif"));
        assert!(!media_url_allowed("https://evil.example/x.gif"));
        assert!(!media_url_allowed(
            "https://static.klipy.com.evil.example/x.gif"
        ));
        assert!(!media_url_allowed("https://user:pw@static.klipy.com/x.gif"));
        assert!(!media_url_allowed("file:///etc/passwd"));
    }

    #[test]
    fn klipy_results_become_app_items_and_ads_are_dropped() {
        let v = json!({
            "id": "4551195970372378",
            "title": "Greetings: Man Waving Hello",
            "media_formats": {
                "gif": { "url": "https://static.klipy.com/a/full.gif", "dims": [498, 498], "size": 2614179 },
                "tinygif": { "url": "https://static.klipy.com/a/tiny.gif", "dims": [220, 220], "size": 149153 }
            },
            "itemurl": "https://klipy.com/gifs/greetings-PSr"
        });
        let item = item_from_klipy(&v).expect("item");
        assert_eq!(item["id"], "4551195970372378");
        assert_eq!(item["url"], "https://static.klipy.com/a/full.gif");
        assert_eq!(
            item["preview"]["url"],
            "https://static.klipy.com/a/tiny.gif"
        );
        assert_eq!(item["width"], 498);
        assert_eq!(item["preview"]["width"], 220);
        assert_eq!(item["pageUrl"], "https://klipy.com/gifs/greetings-PSr");

        let mut ad = v.clone();
        ad["type"] = json!("ad");
        assert!(item_from_klipy(&ad).is_none());

        let mut foreign = v.clone();
        foreign["media_formats"]["gif"]["url"] = json!("https://evil.example/x.gif");
        assert!(item_from_klipy(&foreign).is_none());
    }

    #[test]
    fn queries_are_trimmed_and_cut() {
        assert_eq!(normalize_query("  hello  "), "hello");
        assert_eq!(
            normalize_query(&"x".repeat(80)).chars().count(),
            QUERY_MAX_CHARS
        );
    }

    #[test]
    fn budget_is_a_rolling_hour() {
        let g = Gifs::new(Provider {
            key: Some("k".into()),
            api_base: "http://127.0.0.1:1".into(),
            content_filter: "low".into(),
            budget: 2,
        });
        assert!(g.take_budget(2).is_ok());
        assert!(g.take_budget(2).is_ok());
        let wait = g.take_budget(2).unwrap_err();
        assert!(wait >= 5);
        assert_eq!(g.calls_last_hour(), 2);
    }

    /// Without a key every call is `503 gif_unavailable`, and nothing goes upstream.
    #[tokio::test]
    async fn without_a_key_search_is_unavailable() {
        let g = Gifs::new(Provider {
            key: None,
            api_base: "http://127.0.0.1:1".into(),
            content_filter: "low".into(),
            budget: 10,
        });
        let err = g
            .call(
                "search:x:".into(),
                "search",
                &[("q", "x".into())],
                SEARCH_TTL,
            )
            .await
            .unwrap_err();
        assert_eq!(err, GifError::Unavailable);
    }

    /// Identical queries hit upstream once (cache), and a 429 upstream becomes Busy.
    #[tokio::test]
    async fn identical_queries_share_one_upstream_call_and_429_is_busy() {
        static HITS: AtomicUsize = AtomicUsize::new(0);
        async fn upstream(Query(q): Query<HashMap<String, String>>) -> Response {
            HITS.fetch_add(1, Ordering::SeqCst);
            assert_eq!(q.get("key").map(String::as_str), Some("test-key"));
            assert_eq!(q.get("contentfilter").map(String::as_str), Some("low"));
            if q.get("q").map(String::as_str) == Some("busy") {
                return (StatusCode::TOO_MANY_REQUESTS, [(header::RETRY_AFTER, "7")])
                    .into_response();
            }
            Json(json!({
                "results": [{
                    "id": "1",
                    "title": "t",
                    "media_formats": {
                        "gif": { "url": "https://static.klipy.com/a/full.gif", "dims": [10, 10], "size": 1 },
                        "tinygif": { "url": "https://static.klipy.com/a/tiny.gif", "dims": [5, 5], "size": 1 }
                    },
                    "itemurl": "https://klipy.com/gifs/t"
                }],
                "next": "Mg=="
            }))
            .into_response()
        }
        let app = Router::new().route("/search", get(upstream));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let g = Arc::new(Gifs::new(Provider {
            key: Some("test-key".into()),
            api_base: format!("http://{addr}"),
            content_filter: "low".into(),
            budget: 10,
        }));
        let params = [("q", "hello".to_owned())];
        let (a, b) = tokio::join!(
            g.call("search:hello:".into(), "search", &params, SEARCH_TTL),
            g.call("search:hello:".into(), "search", &params, SEARCH_TTL),
        );
        let page = page_from_klipy(&a.unwrap());
        assert_eq!(page["items"][0]["id"], "1");
        assert_eq!(page["next"], "Mg==");
        assert!(b.is_ok());
        let c = g
            .call("search:hello:".into(), "search", &params, SEARCH_TTL)
            .await;
        assert!(c.is_ok());
        assert_eq!(HITS.load(Ordering::SeqCst), 1, "cached + coalesced");
        assert_eq!(g.calls_last_hour(), 1);

        let busy = g
            .call(
                "search:busy:".into(),
                "search",
                &[("q", "busy".to_owned())],
                SEARCH_TTL,
            )
            .await
            .unwrap_err();
        assert_eq!(busy, GifError::Busy { retry_after_s: 7 });
    }
}
