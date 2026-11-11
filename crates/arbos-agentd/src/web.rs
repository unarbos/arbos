//! Web research for every harness (design D14/D21 as built): `web_search` and `web_fetch` on the
//! project MCP server, so the main agent (through codemode), Prime, Claude Code and the pi
//! fallback all have the same two tools.
//!
//! Search goes to a keyless search endpoint that speaks MCP over HTTP (`ARBOS_WEB_SEARCH_MCP_URL`,
//! default Exa's public server, free tier, no key in the box) with an HTML search page as the
//! fallback. Fetch is a plain GET with HTML reduced to readable text, falling back to the same
//! endpoint's page reader when a site blocks bots or renders only with JavaScript. Credentials
//! never travel through these tools: anything that needs a secret goes through `egress`.

use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::agent::Agent;
use crate::mcp::{err_result, materialize_handle, text_result};

static HTML_OMITTED: LazyLock<[regex_lite::Regex; 5]> = LazyLock::new(|| {
    ["script", "style", "noscript", "svg", "template"]
        .map(|tag| regex_lite::Regex::new(&format!(r"(?is)<{tag}\b[^>]*>.*?</{tag}>")).unwrap())
});

static HTML_COMMENTS: LazyLock<regex_lite::Regex> =
    LazyLock::new(|| regex_lite::Regex::new(r"(?s)<!--.*?-->").unwrap());

static HTML_ANCHORS: LazyLock<regex_lite::Regex> = LazyLock::new(|| {
    regex_lite::Regex::new(r#"(?is)<a\b[^>]*href\s*=\s*["']([^"']+)["'][^>]*>(.*?)</a>"#).unwrap()
});

static HTML_LIST_ITEMS: LazyLock<regex_lite::Regex> =
    LazyLock::new(|| regex_lite::Regex::new(r"(?i)<li\b[^>]*>").unwrap());

static HTML_HEADINGS: LazyLock<regex_lite::Regex> =
    LazyLock::new(|| regex_lite::Regex::new(r"(?i)<h([1-6])\b[^>]*>").unwrap());

static HTML_BREAKS: LazyLock<regex_lite::Regex> = LazyLock::new(|| {
    regex_lite::Regex::new(
        r"(?i)<(br|/p|/div|/li|/h[1-6]|/tr|/table|/section|/article|/header|/footer|/pre|/blockquote|/ul|/ol|/dd|/dt|hr)\b[^>]*>",
    )
    .unwrap()
});

static HTML_CELLS: LazyLock<regex_lite::Regex> =
    LazyLock::new(|| regex_lite::Regex::new(r"(?i)</t[dh]>").unwrap());

static HTML_TAGS: LazyLock<regex_lite::Regex> =
    LazyLock::new(|| regex_lite::Regex::new(r"(?s)<[^>]*>").unwrap());

static HTML_NUMERIC_ENTITIES: LazyLock<regex_lite::Regex> =
    LazyLock::new(|| regex_lite::Regex::new(r"&#(x?)([0-9a-fA-F]+);").unwrap());

static SEARCH_LINKS: LazyLock<Result<regex_lite::Regex, String>> = LazyLock::new(|| {
    regex_lite::Regex::new(r#"<a[^>]*class="result__a"[^>]*href="([^"]+)"[^>]*>(.*?)</a>"#)
        .map_err(|e| e.to_string())
});

static SEARCH_SNIPPETS: LazyLock<Result<regex_lite::Regex, String>> = LazyLock::new(|| {
    regex_lite::Regex::new(r#"<a[^>]*class="result__snippet"[^>]*>(.*?)</a>"#)
        .map_err(|e| e.to_string())
});

const DEFAULT_SEARCH_MCP_URL: &str = "https://mcp.exa.ai/mcp";
const SEARCH_TOOL: &str = "web_search_exa";
const FETCH_TOOL: &str = "web_fetch_exa";
const HTML_SEARCH_URL: &str = "https://html.duckduckgo.com/html/";
const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36 ArbosAgent/1.0";
const SEARCH_TIMEOUT: Duration = Duration::from_secs(25);
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_RESULTS: u64 = 8;
const MAX_RESULTS: u64 = 10;
const DEFAULT_FETCH_CHARS: usize = 20_000;
const MAX_FETCH_CHARS: usize = 60_000;
/// Text longer than this becomes a `peek` handle holding the whole page, so the part past
/// `max_chars` is readable without a second fetch. Matches `OFFLOAD_CHARS` in the pi extension
/// (`harness/arbos-pi-ext/index.ts`) and the 4,000-character rule stated in `prompts/router.md`.
const OFFLOAD_CHARS: usize = 4_000;
/// Bytes read from one page before giving up on it.
const MAX_BODY_BYTES: usize = 3_000_000;
/// An HTML page that reduces to less text than this is probably a bot wall or a JS-only shell.
const THIN_PAGE_CHARS: usize = 300;
/// Retries of a rate-limited search before the HTML fallback. The keyless endpoint rejects a
/// burst in 150–230 ms, so the old code reached the weaker fallback long before the quota could
/// refill (Jacob, Test5, 2026-11-04, T5 13:51:04, twice).
const RATE_LIMIT_RETRIES: u32 = 2;
/// Wait before retrying a rate-limited search; doubles per retry (so 1 s, then 2 s).
const RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(1);
/// How long after a rejection every search on this box queues behind one lane. Codemode fans
/// searches out in parallel and they all spend the same quota, so a burst is what provokes the
/// limit; during one, they go one at a time.
const RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(60);
/// The longest one `web_search` may spend waiting to be let through before it gives up and takes
/// the HTML fallback. The lane needs a ceiling or it becomes the failure it replaced: one
/// rejection serialises every search for a minute, and a codemode fan-out of ten would have its
/// last search wait out nine turns of retries and sleeps — minutes — for an answer the first
/// search already proved was not coming. Every search in a fan-out starts its own budget at the
/// same moment, so the whole fan-out settles inside one of these rather than N of them.
const RATE_LIMIT_BUDGET: Duration = Duration::from_secs(8);

pub fn tool_specs() -> Vec<Value> {
    vec![
        json!({ "name": "web_search", "description": "Search the web (no key needed). Use it to find a service's API reference, OpenAPI/Swagger spec, SDK or CLI, GitHub repository, pricing page, or the meaning of an error, before guessing or asking the user. Returns titles, URLs and the relevant text of the top pages; follow up with web_fetch on the best URL.",
                "inputSchema": { "type": "object", "properties": {
                    "query": { "type": "string", "description": "Describe the page you want, e.g. \"<service> REST API reference authentication endpoints\"" },
                    "objective": { "type": "string", "description": "Optional: what you need from the results (which pages should rank first, what facts to pull)" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 10, "description": "Results to return (default 8)" } },
                  "required": ["query"] } }),
        json!({ "name": "web_fetch", "description": "Read a public URL as text: documentation pages, OpenAPI/Swagger JSON or YAML, README files, llms.txt, raw source on GitHub, release notes. HTML is reduced to readable text with links kept; JSON, YAML and plain text come back verbatim. No credentials: a request that needs a key goes through egress instead.",
                "inputSchema": { "type": "object", "properties": {
                    "url": { "type": "string" },
                    "max_chars": { "type": "integer", "minimum": 1000, "maximum": 60000, "description": "Cut the inline text here (default 20000). The whole page is kept in a peek handle, so read past the cut with peek(handle, {chars:[from,to]}) or peek(handle, {grep:\"...\"}) instead of fetching the URL again." } },
                  "required": ["url"] } }),
    ]
}

fn search_mcp_url() -> String {
    std::env::var("ARBOS_WEB_SEARCH_MCP_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_SEARCH_MCP_URL.to_owned())
}

/// `mcp_call`'s own wording for a rejected search. That `format!` is a few lines up and is the
/// only thing that produces this string, so matching it here does not reach across a boundary.
fn rate_limited(err: &str) -> bool {
    err.starts_with("search endpoint answered 429")
}

/// Paces `web_search` against an endpoint that rate-limits.
///
/// Two separate jobs, which is why there are two locks. `lane` makes concurrent searches take
/// turns while the endpoint is rejecting: codemode fans them out in parallel, they all draw on
/// the same quota, and the burst is what provokes the limit in the first place. `state` holds
/// how long to wait before the next attempt. Idle, neither costs anything — the lane is only
/// taken once a rejection has been seen, so searches still fan out when nothing is limiting.
#[derive(Default)]
struct Pacer {
    lane: tokio::sync::Mutex<()>,
    state: std::sync::Mutex<PacerState>,
}

#[derive(Default)]
struct PacerState {
    /// No search goes out before this.
    next_allowed: Option<Instant>,
    /// Searches share the lane until this passes.
    serialize_until: Option<Instant>,
}

impl Pacer {
    /// Is a burst still queueing?
    fn serializing(&self) -> bool {
        let s = self.state.lock().expect("pacer");
        s.serialize_until.is_some_and(|t| Instant::now() < t)
    }

    /// How long this attempt must hold off, if at all.
    fn wait(&self) -> Option<Duration> {
        let s = self.state.lock().expect("pacer");
        s.next_allowed
            .and_then(|t| t.checked_duration_since(Instant::now()))
            .filter(|d| !d.is_zero())
    }

    /// The endpoint rejected attempt `attempt` (0-based).
    fn note_limited(&self, attempt: u32) {
        let now = Instant::now();
        let mut s = self.state.lock().expect("pacer");
        s.next_allowed = Some(now + RATE_LIMIT_BACKOFF.saturating_mul(1 << attempt.min(4)));
        s.serialize_until = Some(now + RATE_LIMIT_COOLDOWN);
    }

    /// A search got through. The hold-off goes, but the lane stays until the cooldown runs out:
    /// one answer may be *because* the burst was queued, so it is not proof the quota is back.
    fn note_ok(&self) {
        self.state.lock().expect("pacer").next_allowed = None;
    }
}

fn pacer() -> &'static Pacer {
    static P: std::sync::OnceLock<Pacer> = std::sync::OnceLock::new();
    P.get_or_init(Pacer::default)
}

/// Time left before `deadline`, or `None` once it has passed.
fn remaining(deadline: Instant) -> Option<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
}

/// Holds this search until the lane and the hold-off let it through. `false` when that would
/// take longer than `deadline` leaves: the caller then stops retrying and the HTML fallback
/// runs, so a queued search returns a weaker answer instead of holding the turn open.
///
/// Nothing is locked and nothing sleeps while the endpoint is answering, so searches still fan
/// out in parallel until a rejection is seen. A search that cannot get the lane in time does
/// not go out unpaced either — an unpaced burst is the thing the lane exists to prevent.
async fn hold<'a>(
    p: &'a Pacer,
    lane: &mut Option<tokio::sync::MutexGuard<'a, ()>>,
    deadline: Instant,
) -> bool {
    if lane.is_none() && p.serializing() {
        let Some(left) = remaining(deadline) else {
            return false;
        };
        match tokio::time::timeout(left, p.lane.lock()).await {
            Ok(g) => *lane = Some(g),
            Err(_) => return false,
        }
    }
    match p.wait() {
        None => true,
        Some(d) => match remaining(deadline) {
            Some(left) if d <= left => {
                tokio::time::sleep(d).await;
                true
            }
            _ => false,
        },
    }
}

/// The primary search, paced. A rate limit costs a short wait and a retry instead of dropping
/// straight to the HTML fallback, which parses DuckDuckGo with regexes and is weaker than the
/// answer the retry gets. Only a rejection is retried; any other error returns at once, and all
/// the waiting a rejection buys fits inside `RATE_LIMIT_BUDGET`.
async fn paced_search(
    agent: &Arc<Agent>,
    query: &str,
    objective: &str,
    limit: u64,
) -> Result<String, String> {
    let p = pacer();
    let deadline = Instant::now() + RATE_LIMIT_BUDGET;
    let mut lane = None;
    let mut limited = String::new();
    for attempt in 0..=RATE_LIMIT_RETRIES {
        if !hold(p, &mut lane, deadline).await {
            break;
        }
        match mcp_call(
            agent,
            SEARCH_TOOL,
            json!({ "query": query, "objective": objective, "numResults": limit }),
            SEARCH_TIMEOUT,
        )
        .await
        {
            Err(e) if rate_limited(&e) => {
                tracing::info!(
                    attempt,
                    "web_search: the search endpoint is rate-limiting; backing off and queueing searches behind one lane"
                );
                limited = e;
                p.note_limited(attempt);
            }
            Ok(text) if text.trim().is_empty() => {
                p.note_ok();
                return Err("no results".to_owned());
            }
            Ok(text) => {
                p.note_ok();
                return Ok(text);
            }
            Err(e) => return Err(e),
        }
    }
    // Empty when the budget went on queueing and this search never reached the endpoint, which
    // `web_search` would otherwise log as a blank reason.
    Err(if limited.is_empty() {
        "search endpoint is rate-limiting and the queue is longer than this search can wait"
            .to_owned()
    } else {
        limited
    })
}

/// The text content of one MCP `tools/call` answer, from a JSON body or an SSE stream.
/// `Err(text)` when the server flagged the result as an error.
fn mcp_text(body: &str) -> Option<Result<String, String>> {
    let candidates: Vec<&str> = if body.trim_start().starts_with('{') {
        vec![body.trim()]
    } else {
        body.lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .map(str::trim)
            .collect()
    };
    for c in candidates {
        let Ok(v) = serde_json::from_str::<Value>(c) else {
            continue;
        };
        if let Some(e) = v.get("error") {
            return Some(Err(e
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
                .to_owned()));
        }
        let Some(content) = v.pointer("/result/content").and_then(Value::as_array) else {
            continue;
        };
        let text = content
            .iter()
            .filter_map(|c| c.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n");
        if v.pointer("/result/isError")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Some(Err(text));
        }
        return Some(Ok(text));
    }
    None
}

async fn mcp_call(
    agent: &Arc<Agent>,
    tool: &str,
    arguments: Value,
    timeout: Duration,
) -> Result<String, String> {
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": tool, "arguments": arguments } });
    let resp = agent
        .http_client()
        .post(search_mcp_url())
        .timeout(timeout)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("user-agent", USER_AGENT)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("search endpoint unreachable: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "search endpoint answered {status}: {}",
            clip_chars(&text, 200)
        ));
    }
    match mcp_text(&text) {
        Some(r) => r,
        None => Err("search endpoint returned nothing readable".into()),
    }
}

pub async fn web_search(agent: &Arc<Agent>, args: &Value, chat_id: &str) -> Value {
    let query = args["query"].as_str().unwrap_or("").trim().to_owned();
    if query.is_empty() {
        return err_result("query is required");
    }
    let objective = args["objective"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("Find the pages that best answer: {query}"));
    let limit = args["limit"]
        .as_u64()
        .unwrap_or(DEFAULT_RESULTS)
        .clamp(1, MAX_RESULTS);
    record(agent, chat_id, "web_search", &query);
    match paced_search(agent, &query, &objective, limit).await {
        Ok(text) => text_result(crate::store::redact(&text)),
        Err(primary_err) => {
            tracing::warn!(error = %primary_err, "web_search: primary endpoint failed; HTML fallback");
            match html_search(agent, &query, limit as usize).await {
                Ok(text) => text_result(crate::store::redact(&text)),
                Err(e) => err_result(format!(
                    "web search failed ({primary_err}; fallback: {e}). Try web_fetch on a URL you know, such as the service's docs or GitHub page."
                )),
            }
        }
    }
}

/// Fallback search: a plain HTML results page, parsed with regexes.
async fn html_search(agent: &Arc<Agent>, query: &str, limit: usize) -> Result<String, String> {
    let resp = agent
        .http_client()
        .get(HTML_SEARCH_URL)
        .query(&[("q", query)])
        .timeout(SEARCH_TIMEOUT)
        .header("user-agent", USER_AGENT)
        .header("accept", "text/html")
        .send()
        .await
        .map_err(|e| format!("search page unreachable: {e}"))?;
    let status = resp.status();
    let html = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("search page answered {status}"));
    }
    let link_re = SEARCH_LINKS.as_ref().map_err(Clone::clone)?;
    let snippet_re = SEARCH_SNIPPETS.as_ref().map_err(Clone::clone)?;
    let snippets: Vec<String> = snippet_re
        .captures_iter(&html)
        .map(|c| html_to_text(&c[1], None))
        .collect();
    let mut out = Vec::new();
    for (i, cap) in link_re.captures_iter(&html).enumerate() {
        if out.len() >= limit {
            break;
        }
        let href = decode_entities(&cap[1]);
        let url = redirect_target(&href).unwrap_or(href);
        let title = html_to_text(&cap[2], None);
        let snippet = snippets.get(i).cloned().unwrap_or_default();
        out.push(format!(
            "{}. {title}\n   {url}\n   {snippet}",
            out.len() + 1
        ));
    }
    if out.is_empty() {
        return Err("no results parsed from the search page".into());
    }
    Ok(out.join("\n\n"))
}

/// `//host/l/?uddg=<url>&…` style redirect links → the real URL.
fn redirect_target(href: &str) -> Option<String> {
    let (_, q) = href.split_once("uddg=")?;
    let enc = q.split('&').next()?;
    Some(percent_decode(enc))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub async fn web_fetch(agent: &Arc<Agent>, args: &Value, chat_id: &str) -> Value {
    let url = args["url"].as_str().unwrap_or("").trim().to_owned();
    if url.is_empty() {
        return err_result("url is required");
    }
    if let Err(why) = check_url(&url) {
        return err_result(why);
    }
    let max_chars = args["max_chars"]
        .as_u64()
        .map(|n| n as usize)
        .unwrap_or(DEFAULT_FETCH_CHARS)
        .clamp(1_000, MAX_FETCH_CHARS);
    record(agent, chat_id, "web_fetch", &url);
    let direct = direct_fetch(agent, &url).await;
    let fallback_reason = match &direct {
        Ok(page) if page.thin => {
            Some("the page has almost no readable text (bot wall or JavaScript-only)".to_owned())
        }
        Ok(_) => None,
        Err(e) => Some(e.clone()),
    };
    if let Some(reason) = fallback_reason {
        tracing::info!(url, reason, "web_fetch: trying the reader endpoint");
        // Ask the endpoint for everything it will give, not for `max_chars`: the extra text goes
        // into the handle so this branch can be paged like the direct one.
        if let Ok(text) = mcp_call(
            agent,
            FETCH_TOOL,
            json!({ "urls": [url], "maxCharacters": MAX_FETCH_CHARS }),
            FETCH_TIMEOUT,
        )
        .await
        {
            if text.trim().len() > THIN_PAGE_CHARS {
                let body = offload(agent, chat_id, &url, &text, max_chars);
                return text_result(crate::store::redact(&format!(
                    "URL: {url}\nSource: reader endpoint ({reason})\n\n{body}"
                )));
            }
        }
    }
    match direct {
        Ok(page) => {
            let body = offload(agent, chat_id, &url, &page.text, max_chars);
            text_result(crate::store::redact(&format!(
                "URL: {url}\nStatus: {}\nContent-Type: {}\n\n{body}",
                page.status, page.content_type
            )))
        }
        Err(e) => err_result(format!("web_fetch failed: {e}")),
    }
}

/// The inline body for one fetched page: the head, capped at `max_chars`, plus — for anything
/// over `OFFLOAD_CHARS` — a handle holding the **whole** text.
///
/// Character offsets into the handle line up with the inline head, so
/// `peek(handle, {chars:[max_chars, …]})` continues exactly where the head stops. Without this
/// the remainder was dropped inside `direct_fetch` and re-fetching at a higher `max_chars` was
/// the only way to reach it — impossible at all past `MAX_FETCH_CHARS`.
fn offload(agent: &Arc<Agent>, chat_id: &str, url: &str, text: &str, max_chars: usize) -> String {
    let chars = text.chars().count();
    if chars <= OFFLOAD_CHARS {
        return clip_chars(text, max_chars);
    }
    // Redact before the blob is written: `peek` reads the file back verbatim, so an unredacted
    // handle would hand out what the inline text is careful not to show.
    let text = crate::store::redact(text);
    let chars = text.chars().count();
    let handle = materialize_handle(agent, chat_id, &format!("web_fetch {url}"), &text);
    let banner = format!(
        "[handle {handle}: web_fetch {url}, {chars} chars, {} lines]",
        text.lines().count()
    );
    if chars <= max_chars {
        return format!("{banner}\n{text}");
    }
    let head: String = text.chars().take(max_chars).collect();
    // `peek` clips its own answer at 16,000 bytes, so suggest a window it returns whole.
    let to = chars.min(max_chars + 8_000);
    format!(
        "{banner}\n{head}\n…[cut at {max_chars} of {chars} chars. The rest is in the handle: \
         peek(\"{handle}\", {{chars:[{max_chars},{to}]}}) reads on from here and \
         peek(\"{handle}\", {{grep:\"pattern\"}}) searches the whole page. Do not fetch this URL again.]"
    )
}

struct Page {
    status: u16,
    content_type: String,
    /// The whole page, not the inline head: the caller offloads it to a handle and clips its own
    /// copy. Bounded by `MAX_BODY_BYTES`, which is read off the wire.
    text: String,
    thin: bool,
}

async fn direct_fetch(agent: &Arc<Agent>, url: &str) -> Result<Page, String> {
    let resp = agent
        .http_client()
        .get(url)
        .timeout(FETCH_TIMEOUT)
        .header("user-agent", USER_AGENT)
        .header(
            "accept",
            "text/html,application/json,application/yaml,text/plain,text/markdown,*/*;q=0.8",
        )
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let status = resp.status().as_u16();
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let final_url = resp.url().to_string();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("body read failed: {e}"))?;
    let body = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_BODY_BYTES)]).into_owned();
    if matches!(status, 401 | 403 | 429 | 503) && body.len() < 20_000 {
        return Err(format!("status {status}"));
    }
    let is_html = content_type.contains("html")
        || (content_type.is_empty() && body.trim_start().starts_with('<'));
    let text = if is_html {
        html_to_text(&body, Some(&final_url))
    } else {
        body
    };
    let thin = is_html && text.trim().len() < THIN_PAGE_CHARS;
    Ok(Page {
        status,
        content_type: if content_type.is_empty() {
            "unknown".into()
        } else {
            content_type
        },
        text,
        thin,
    })
}

/// Only public http(s) URLs with no secret placeholder in them.
fn check_url(url: &str) -> Result<(), String> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err("only http(s) URLs can be fetched".into());
    }
    if lower.contains("arbos-secret://") || lower.contains("op://") || url.contains('$') {
        return Err(
            "this URL carries a secret placeholder; a request that needs a secret goes through egress (placeholder in a header or the body, never in the URL)"
                .into(),
        );
    }
    let host = lower
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .rsplit('@')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_owned();
    let private = host.is_empty()
        || host == "localhost"
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host.starts_with("127.")
        || host.starts_with("10.")
        || host.starts_with("192.168.")
        || host.starts_with("169.254.")
        || host == "0.0.0.0"
        || host == "::1"
        || host
            .strip_prefix("172.")
            .and_then(|r| r.split('.').next())
            .and_then(|o| o.parse::<u8>().ok())
            .is_some_and(|o| (16..=31).contains(&o));
    if private {
        return Err("web_fetch reads public sites only".into());
    }
    Ok(())
}

fn record(agent: &Arc<Agent>, chat_id: &str, tool: &str, subject: &str) {
    let _ = agent.store.t3_append(
        chat_id,
        None,
        Some(chat_id),
        tool,
        Some("assistant"),
        &format!("{tool} {}", clip_chars(subject, 400)),
        None,
    );
}

fn clip_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let cut: String = s.chars().take(max).collect();
    format!("{cut}\n…[cut at {max} chars; raise max_chars or fetch a more specific URL]")
}

/// HTML → readable text: scripts, styles and comments dropped; block tags become line breaks;
/// list items get a dash; links keep their target as `text (url)`; entities decoded; blank
/// lines collapsed. No external parser: the pages that matter here (docs, READMEs, specs) are
/// simple, and anything else can go to the reader endpoint.
pub fn html_to_text(html: &str, base_url: Option<&str>) -> String {
    let mut s = html.to_owned();
    for re in HTML_OMITTED.iter() {
        s = re.replace_all(&s, " ").into_owned();
    }
    s = HTML_COMMENTS.replace_all(&s, " ").into_owned();
    s = HTML_ANCHORS
        .replace_all(&s, |c: &regex_lite::Captures| {
            let href = decode_entities(&c[1]);
            let label = strip_tags(&c[2]);
            let label = label.trim();
            match absolute_url(&href, base_url) {
                Some(u) if !label.is_empty() && !u.starts_with("javascript:") && label != u => {
                    format!("{label} ({u})")
                }
                _ => label.to_owned(),
            }
        })
        .into_owned();
    s = HTML_LIST_ITEMS.replace_all(&s, "\n- ").into_owned();
    s = HTML_HEADINGS
        .replace_all(&s, |c: &regex_lite::Captures| {
            let n: usize = c[1].parse().unwrap_or(1);
            format!("\n\n{} ", "#".repeat(n))
        })
        .into_owned();
    s = HTML_BREAKS.replace_all(&s, "\n").into_owned();
    s = HTML_CELLS.replace_all(&s, " | ").into_owned();
    let text = strip_tags(&s);
    let text = decode_entities(&text);
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0;
    for line in text.lines() {
        let line: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                out.push('\n');
            }
        } else {
            blank_run = 0;
            out.push_str(&line);
            out.push('\n');
        }
    }
    out.trim().to_owned()
}

fn strip_tags(s: &str) -> String {
    HTML_TAGS.replace_all(s, " ").into_owned()
}

fn absolute_url(href: &str, base: Option<&str>) -> Option<String> {
    let href = href.trim();
    if href.is_empty() || href.starts_with('#') {
        return None;
    }
    if href.starts_with("http://") || href.starts_with("https://") {
        return Some(href.to_owned());
    }
    let base = base?;
    if let Some(rest) = href.strip_prefix("//") {
        let scheme = base.split("://").next().unwrap_or("https");
        return Some(format!("{scheme}://{rest}"));
    }
    let (scheme, rest) = base.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    if let Some(path) = href.strip_prefix('/') {
        return Some(format!("{scheme}://{host}/{path}"));
    }
    if href.contains(':') {
        return None;
    }
    let dir = rest
        .split(['?', '#'])
        .next()
        .and_then(|p| p.rsplit_once('/'))
        .map(|(d, _)| d)
        .unwrap_or(host);
    Some(format!("{scheme}://{dir}/{href}"))
}

fn decode_entities(s: &str) -> String {
    let mut out = s
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&apos;", "'")
        .replace("&hellip;", "…")
        .replace("&mdash;", "—")
        .replace("&ndash;", "–");
    out = HTML_NUMERIC_ENTITIES
        .replace_all(&out, |c: &regex_lite::Captures| {
            let n = if c[1].is_empty() {
                c[2].parse::<u32>().ok()
            } else {
                u32::from_str_radix(&c[2], 16).ok()
            };
            n.and_then(char::from_u32)
                .map(|ch| ch.to_string())
                .unwrap_or_default()
        })
        .into_owned();
    out.replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_becomes_readable_text_with_links() {
        let html = r#"<html><head><style>p{}</style><script>x()</script></head>
        <body><h1>API reference</h1><p>Base URL: <code>https://api.example.com/v1</code></p>
        <ul><li><a href="/docs/auth">Authentication</a></li><li>List nodes &amp; prices</li></ul>
        <!-- hidden --><table><tr><td>GET</td><td>/nodes</td></tr></table></body></html>"#;
        let text = html_to_text(html, Some("https://docs.example.com/intro"));
        assert!(text.starts_with("# API reference"));
        assert!(text.contains("Base URL: https://api.example.com/v1"));
        assert!(text.contains("- Authentication (https://docs.example.com/docs/auth)"));
        assert!(text.contains("- List nodes & prices"));
        assert!(text.contains("GET | /nodes"));
        assert!(!text.contains("x()"));
        assert!(!text.contains("hidden"));
    }

    /// Jacob, Test5, 2026-11-04, T5 13:51:04 (twice): `web_search: primary endpoint failed;
    /// HTML fallback`, because the keyless endpoint answered `429 Too Many Requests … You've
    /// hit Exa's free MCP rate limit`. A rejection took 150–230 ms, so the search was handed to
    /// a DuckDuckGo regex parse almost instantly. A rejection now buys a wait and a retry.
    #[test]
    fn a_rejection_backs_off_and_queues_the_burst() {
        let p = Pacer::default();
        // Nothing has been refused, so nothing waits and searches still fan out in parallel.
        assert!(!p.serializing());
        assert!(p.wait().is_none());

        p.note_limited(0);
        assert!(
            p.serializing(),
            "a burst spends one quota, so it queues behind one lane while the limit holds"
        );
        let first = p.wait().expect("the next attempt holds off");
        assert!(first <= RATE_LIMIT_BACKOFF && first > Duration::ZERO);

        // The wait doubles while rejections continue.
        p.note_limited(1);
        let second = p.wait().expect("still holding off");
        assert!(second > first, "{second:?} should exceed {first:?}");

        // A search got through: the hold-off goes so the next one is not delayed for nothing.
        p.note_ok();
        assert!(p.wait().is_none());
        // The lane stays, though — that answer may be *because* the burst was queued.
        assert!(p.serializing());
    }

    /// A queued search gives up rather than holding the turn open. Without a ceiling the lane is
    /// itself the failure: one rejection serialises every search for a minute, so the last search
    /// of a codemode fan-out would wait out every search ahead of it — and all of them end in the
    /// same fallback anyway. Past the budget, pacing stops and the weaker answer is taken.
    #[tokio::test]
    async fn a_queued_search_stops_waiting_at_its_budget() {
        let p = Pacer::default();
        p.note_limited(0);
        assert!(p.serializing());
        assert!(p.wait().is_some(), "there is a hold-off to wait out");

        // Budget already spent: the hold-off is not waited out and the lane is not even taken.
        let mut spent = None;
        assert!(!hold(&p, &mut spent, Instant::now()).await);
        assert!(spent.is_none());

        // With budget, the same hold-off is waited out and the search goes on.
        let mut first = None;
        assert!(hold(&p, &mut first, Instant::now() + RATE_LIMIT_BUDGET).await);
        assert!(
            first.is_some(),
            "a serialising burst queues behind the lane"
        );
        assert!(p.wait().is_none(), "the hold-off was waited out");

        // A second search cannot have the lane while the first holds it, and it stops waiting at
        // its own deadline instead of queueing behind the whole cooldown.
        let mut second = None;
        let t = Instant::now();
        assert!(!hold(&p, &mut second, Instant::now() + Duration::from_millis(50)).await);
        assert!(second.is_none());
        assert!(
            t.elapsed() < RATE_LIMIT_COOLDOWN,
            "waited {:?}, which is the stall this budget exists to stop",
            t.elapsed()
        );
    }

    /// Only a rate limit is worth retrying; every other failure still goes straight on.
    #[test]
    fn only_a_rejection_counts_as_rate_limited() {
        assert!(rate_limited(
            "search endpoint answered 429 Too Many Requests: You've hit Exa's free MCP rate limit"
        ));
        assert!(!rate_limited(
            "search endpoint answered 500 Internal Server Error: nope"
        ));
        assert!(!rate_limited(
            "search endpoint unreachable: connection refused"
        ));
        assert!(!rate_limited("no results"));
    }

    /// End to end against an endpoint that behaves like Exa's free MCP: it rejects the first
    /// call and serves the second. The answer must come from the retry, not from the HTML
    /// fallback — which is the whole point, since the fallback is a regex parse of a results
    /// page and is weaker than what the retry gets.
    #[tokio::test]
    async fn a_rate_limited_search_retries_instead_of_falling_back() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        CALLS.store(0, Ordering::SeqCst);

        let app = axum::Router::new().route(
            "/mcp",
            axum::routing::post(|| async {
                use axum::response::IntoResponse;
                if CALLS.fetch_add(1, Ordering::SeqCst) == 0 {
                    return (
                        axum::http::StatusCode::TOO_MANY_REQUESTS,
                        "You've hit Exa's free MCP rate limit. Please try again later.",
                    )
                        .into_response();
                }
                axum::Json(json!({
                    "jsonrpc": "2.0", "id": 1,
                    "result": { "content": [{ "type": "text",
                        "text": "Python 3.15.2 — https://www.python.org/downloads/" }] }
                }))
                .into_response()
            }),
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });

        let (agent, _root) = crate::status::tests::test_agent().await;
        std::env::set_var(
            "ARBOS_WEB_SEARCH_MCP_URL",
            format!("http://127.0.0.1:{port}/mcp"),
        );
        let out = web_search(
            &agent,
            &json!({ "query": "latest python release" }),
            "c-test",
        )
        .await;
        std::env::remove_var("ARBOS_WEB_SEARCH_MCP_URL");

        // The call count first: it is the assertion that does not depend on what any network
        // the test happens to have would answer.
        assert_eq!(
            CALLS.load(Ordering::SeqCst),
            2,
            "the rejection is retried once, not handed to DuckDuckGo"
        );
        let text = serde_json::to_string(&out).unwrap();
        assert!(
            text.contains("Python 3.15.2"),
            "the retry's answer is the result: {text}"
        );
    }

    #[test]
    fn mcp_text_reads_json_and_sse() {
        let json_body =
            r#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"hello"}]}}"#;
        assert_eq!(mcp_text(json_body), Some(Ok("hello".into())));
        let sse = "event: message\ndata: {\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"a\"},{\"type\":\"text\",\"text\":\"b\"}],\"isError\":true}}\n\n";
        assert_eq!(mcp_text(sse), Some(Err("a\nb".into())));
        assert_eq!(mcp_text("nothing"), None);
    }

    #[test]
    fn url_checks() {
        assert!(check_url("https://docs.example.com/openapi.json").is_ok());
        assert!(check_url("ftp://x").is_err());
        assert!(check_url("http://127.0.0.1:7789/mcp").is_err());
        assert!(check_url("http://169.254.169.254/latest").is_err());
        assert!(check_url("http://172.20.0.1/").is_err());
        assert!(check_url("https://api.example.com/v1?key=$MY_KEY").is_err());
        assert!(check_url("https://api.example.com/arbos-secret://K/value").is_err());
    }

    #[test]
    fn redirect_links_are_unwrapped() {
        assert_eq!(
            redirect_target(
                "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa%3Fb%3D1&rut=abc"
            ),
            Some("https://example.com/a?b=1".into())
        );
        assert_eq!(
            absolute_url("guide", Some("https://h.io/docs/api/intro")),
            Some("https://h.io/docs/api/guide".into())
        );
        assert_eq!(
            absolute_url("//cdn.h.io/x", Some("https://h.io/")),
            Some("https://cdn.h.io/x".into())
        );
        assert_eq!(absolute_url("mailto:a@b", Some("https://h.io/")), None);
    }

    #[test]
    fn entities_decode() {
        assert_eq!(
            decode_entities("a &amp; b &lt;c&gt; &#65;&#x42; &quot;q&quot;"),
            "a & b <c> AB \"q\""
        );
    }

    /// `peek`'s `chars` slice (`mcp.rs`), so the test reads a handle the way the agent does.
    fn peek_chars(agent: &Arc<Agent>, handle: &str, from: usize, to: usize) -> String {
        let h = agent
            .store
            .handle(handle)
            .expect("handle lookup")
            .expect("handle is registered");
        let full = std::fs::read_to_string(&h.path).expect("blob on disk");
        full.chars().skip(from).take(to - from).collect()
    }

    /// The regression: text past `max_chars` used to be dropped inside `direct_fetch`, so no
    /// sequence of calls could reach it. It now lives in the handle, at the offsets the inline
    /// body advertises.
    #[tokio::test]
    async fn a_cut_page_can_be_read_on_past_the_cut() {
        let (agent, _root) = crate::status::tests::test_agent().await;
        let page: String = (0..5_000).map(|i| format!("line {i}\n")).collect();
        let max_chars = 4_500;
        let body = offload(
            &agent,
            "c-test",
            "https://docs.example.com/big",
            &page,
            max_chars,
        );

        let handle = body
            .split_once("[handle ")
            .and_then(|(_, r)| r.split_once(':'))
            .map(|(id, _)| id.to_owned())
            .expect("the body names a handle");
        let past_the_cut: String = page.chars().skip(max_chars).take(200).collect();
        assert!(
            !body.contains(&past_the_cut),
            "the inline body stops at the cut"
        );
        assert_eq!(
            peek_chars(&agent, &handle, max_chars, max_chars + 200),
            past_the_cut
        );
        assert_eq!(
            peek_chars(&agent, &handle, 0, page.chars().count()),
            page,
            "the handle holds the whole page, not the head"
        );
    }

    /// The inline half of the same call: capped at `max_chars` and pointing at the handle.
    #[tokio::test]
    async fn a_cut_page_says_how_to_read_on() {
        let (agent, _root) = crate::status::tests::test_agent().await;
        let page = "Z".repeat(50_000);
        let body = offload(
            &agent,
            "c-test",
            "https://docs.example.com/big",
            &page,
            20_000,
        );
        assert_eq!(
            body.matches('Z').count(),
            20_000,
            "inline text honours max_chars"
        );
        assert!(body.contains("[handle h_"));
        assert!(body.contains("chars:[20000,28000]"));
        assert!(body.contains("Do not fetch this URL again."));
    }

    /// Under the threshold nothing is offloaded and the body is exactly the page, as before.
    #[tokio::test]
    async fn a_small_page_is_unchanged_and_mints_no_handle() {
        let (agent, _root) = crate::status::tests::test_agent().await;
        let page = "short page\nwith two lines";
        assert_eq!(
            offload(
                &agent,
                "c-test",
                "https://docs.example.com/small",
                page,
                20_000
            ),
            page
        );
    }

    /// Offloading happens at `OFFLOAD_CHARS`, which is what makes `prompts/router.md`'s
    /// "outputs over 4,000 characters become handles" true for `web_fetch`.
    #[tokio::test]
    async fn a_page_over_four_thousand_chars_becomes_a_handle_even_when_nothing_is_cut() {
        let (agent, _root) = crate::status::tests::test_agent().await;
        assert_eq!(OFFLOAD_CHARS, 4_000);
        let page = "y".repeat(OFFLOAD_CHARS + 1);
        let body = offload(
            &agent,
            "c-test",
            "https://docs.example.com/mid",
            &page,
            20_000,
        );
        assert!(body.contains("[handle h_"));
        assert!(
            body.ends_with(&page),
            "nothing was cut, so the whole page is inline"
        );
    }

    /// A secret in the page must not survive into the blob: `peek` reads that file back verbatim.
    #[tokio::test]
    async fn an_offloaded_page_is_redacted_before_it_reaches_the_blob() {
        let (agent, _root) = crate::status::tests::test_agent().await;
        let secret = "sk-liveSecretValueForRedactionTest000000";
        crate::store::redact_live_value(secret);
        let page = format!("{}\n{secret}\n", "pad\n".repeat(2_000));
        let body = offload(
            &agent,
            "c-test",
            "https://docs.example.com/leak",
            &page,
            1_000,
        );
        let handle = body
            .split_once("[handle ")
            .and_then(|(_, r)| r.split_once(':'))
            .map(|(id, _)| id.to_owned())
            .expect("the body names a handle");
        assert!(!peek_chars(&agent, &handle, 0, page.chars().count()).contains(secret));
    }
}
