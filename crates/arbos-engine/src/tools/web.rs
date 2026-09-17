//! `fetch` and `search`.
//!
//! `search` answers as numbered sources — `[n] Title — URL` and a snippet —
//! so the model can cite `[n]` and close with a Sources list. The backend
//! is the first one configured: a custom `search_url`, Exa, Brave, Tavily
//! (by their env keys), OpenRouter's web plugin when the model provider is
//! OpenRouter, and DuckDuckGo's HTML page with no key at all.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use super::ToolOut;
use crate::access::Access;
use crate::tool::{BoxFuture, Plan, PlanCx, RunCx, Tool, WebCfg, req, simple_schema};

pub struct Fetch;
pub struct Search;

const MAX_RESULTS: usize = 8;
const SNIPPET_CHARS: usize = 400;
const TITLE_CHARS: usize = 120;
const CITE_LINE: &str = "Cite with [n] after each claim you take from a source, and end your reply with a Sources list of the URLs you used.";

impl Tool for Fetch {
    fn name(&self) -> &'static str {
        "fetch"
    }
    fn schema(&self) -> Value {
        simple_schema(
            "fetch",
            "Fetch a URL as text. What you learn from it is cited by that URL.",
            &[("url", "http(s) URL.", true)],
        )
    }
    fn plan(&self, _cx: &PlanCx, _args: &Value) -> Result<Plan> {
        Ok(Plan::access(Access::none()))
    }
    fn run(&self, _cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        Box::pin(async move { fetch(req(&args, "url")?).await })
    }
}

impl Tool for Search {
    fn name(&self) -> &'static str {
        "search"
    }
    fn schema(&self) -> Value {
        simple_schema(
            "search",
            "Web search. Returns numbered sources ([n] title — URL, snippet). Cite [n] for what you use and end with a Sources list.",
            &[("query", "", true), ("max_results", "1–20 (8).", false)],
        )
    }
    fn plan(&self, _cx: &PlanCx, _args: &Value) -> Result<Plan> {
        Ok(Plan::access(Access::none()))
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        Box::pin(async move {
            let query = req(&args, "query")?.trim().to_string();
            if query.is_empty() {
                bail!("search: query must not be empty");
            }
            let n = args
                .get("max_results")
                .and_then(|v| {
                    v.as_u64()
                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                })
                .map(|n| (n as usize).clamp(1, 20))
                .unwrap_or(MAX_RESULTS);
            search(&cx.web, &query, n).await
        })
    }
}

pub async fn fetch(url: &str) -> Result<ToolOut> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        bail!("fetch only http(s)");
    }
    // The cloud metadata service hands out the instance's credentials;
    // fetch has no reason to read it, whatever the page said.
    if arbos_core::containment::url_is_metadata(url) {
        bail!(
            "fetch refuses the cloud metadata service ({url}): it serves instance credentials, not pages"
        );
    }
    let client = reqwest::Client::builder()
        .user_agent("arbos/0.1")
        .redirect(reqwest::redirect::Policy::limited(8))
        .build()?;
    let resp = client.get(url).send().await?;
    let status = resp.status();
    let final_url = resp.url().to_string();
    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let text = resp.text().await.unwrap_or_default();
    let body = if ctype.contains("html") {
        html_to_text(&text)
    } else {
        text
    };
    Ok(ToolOut::text(format!(
        "Source: {final_url}\nHTTP {status}\n{body}"
    )))
}

/// One search hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Which backend `search` will use, by name, for the tool result.
/// `ARBOS_SEARCH_BACKEND=duckduckgo|openrouter|exa|brave|tavily|custom`
/// forces one (QA, or a user who prefers the keyless page).
pub fn backend_name(web: &WebCfg) -> &'static str {
    if let Some(forced) = env_key("ARBOS_SEARCH_BACKEND") {
        return match forced.trim().to_ascii_lowercase().as_str() {
            "custom" => "custom search_url",
            "exa" => "Exa",
            "brave" => "Brave",
            "tavily" => "Tavily",
            "openrouter" | "openrouter-web" => "OpenRouter web",
            _ => "DuckDuckGo",
        };
    }
    if web.search_url.as_deref().is_some_and(|u| !u.is_empty()) {
        "custom search_url"
    } else if env_key("EXA_API_KEY").is_some() {
        "Exa"
    } else if env_key("BRAVE_API_KEY").is_some() {
        "Brave"
    } else if env_key("TAVILY_API_KEY").is_some() {
        "Tavily"
    } else if web.api_base.contains("openrouter.ai") && web.api_key.is_some() {
        "OpenRouter web"
    } else {
        "DuckDuckGo"
    }
}

fn env_key(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// The one refusal a kernel with no working search gives, leading with the
/// one thing to do. The first time in this process it carries what each
/// provider said; after that only the sentence, so a second and third
/// search do not paint the same two failures in red again (Jacob's
/// desktop feedback 2026-09-17-12: two red rows, 1m 41s, and the model
/// reached this sentence itself two calls later).
static SAID_NO_SEARCH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn no_search_here(detail: &str) -> anyhow::Error {
    let first = !SAID_NO_SEARCH.swap(true, std::sync::atomic::Ordering::SeqCst);
    let lead = "No web search on this kernel: add a search key to its environment (EXA_API_KEY, BRAVE_API_KEY or TAVILY_API_KEY) or a search_url in config.toml. Until then, fetch a URL you know; do not retry search.";
    if first {
        anyhow::anyhow!("{lead} ({detail})")
    } else {
        anyhow::anyhow!("{lead}")
    }
}

/// A provider's failure that means "no search here", as opposed to a bad
/// query or a network fault: the keyless page's bot check.
fn is_no_search(e: &anyhow::Error) -> bool {
    let s = format!("{e:#}");
    s.contains("bot check") || s.contains("CAPTCHA")
}

pub async fn search(web: &WebCfg, query: &str, n: usize) -> Result<ToolOut> {
    let backend = backend_name(web);
    let hits = match backend {
        "custom search_url" => custom(web, query, n).await?,
        "Exa" => exa(&env_key("EXA_API_KEY").unwrap_or_default(), query, n).await?,
        "Brave" => brave(&env_key("BRAVE_API_KEY").unwrap_or_default(), query, n).await?,
        "Tavily" => tavily(&env_key("TAVILY_API_KEY").unwrap_or_default(), query, n).await?,
        "OpenRouter web" => {
            let hits = openrouter_web(web, query, n).await?;
            if hits.is_empty() {
                // The model answered without searching: no sources to
                // show. The keyless page may still have some.
                match duckduckgo(query, n).await {
                    Ok(more) => {
                        return Ok(ToolOut::text(render(
                            "DuckDuckGo (OpenRouter web returned no sources)",
                            query,
                            &more,
                        )));
                    }
                    Err(e) if is_no_search(&e) => {
                        return Err(no_search_here(&format!(
                            "OpenRouter's web plugin returned no sources, and DuckDuckGo's keyless page answered with a bot check"
                        )));
                    }
                    Err(e) => bail!("OpenRouter web returned no sources; {e}"),
                }
            }
            hits
        }
        _ => match duckduckgo(query, n).await {
            Ok(h) => h,
            Err(e) if is_no_search(&e) => {
                return Err(no_search_here(
                    "DuckDuckGo's keyless page answered with a bot check instead of results",
                ));
            }
            Err(e) => return Err(e),
        },
    };
    Ok(ToolOut::text(render(backend, query, &hits)))
}

fn render(backend: &str, query: &str, hits: &[Hit]) -> String {
    if hits.is_empty() {
        return format!("search ({backend}) for {query:?}: no results.");
    }
    let mut body = format!("search ({backend}) for {query:?}:\n");
    for (i, h) in hits.iter().enumerate() {
        let title = if h.title.trim().is_empty() {
            h.url.clone()
        } else {
            clip(h.title.trim(), TITLE_CHARS)
        };
        body.push_str(&format!("\n[{}] {title} — {}\n", i + 1, h.url));
        let snippet = clip(
            h.snippet
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .trim(),
            SNIPPET_CHARS,
        );
        if !snippet.is_empty() {
            body.push_str("    ");
            body.push_str(&snippet);
            body.push('\n');
        }
    }
    body.push('\n');
    body.push_str(CITE_LINE);
    body
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n).collect();
        out.push('…');
        out
    }
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent("arbos/0.1")
        .timeout(std::time::Duration::from_secs(30))
        .build()?)
}

/// `GET <search_url>?q=<query>` → `{results:[{title,url,snippet}]}` (or a
/// bare array). `search_key`, when set, goes as a bearer token.
async fn custom(web: &WebCfg, query: &str, n: usize) -> Result<Vec<Hit>> {
    let base = web.search_url.clone().unwrap_or_default();
    let sep = if base.contains('?') { '&' } else { '?' };
    let url = format!("{base}{sep}q={}", urlencoding(query));
    let mut req = client()?.get(&url);
    if let Some(k) = web.search_key.as_deref().filter(|k| !k.is_empty()) {
        req = req.bearer_auth(k);
    }
    let resp = req
        .send()
        .await
        .with_context(|| format!("search_url {base}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("search_url {base}: HTTP {status}: {}", clip(&text, 200));
    }
    let v: Value = serde_json::from_str(&text)
        .with_context(|| format!("search_url {base}: not JSON: {}", clip(&text, 120)))?;
    let arr = v
        .get("results")
        .and_then(Value::as_array)
        .or_else(|| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(hits_from(
        &arr,
        &["title"],
        &["url", "link"],
        &["snippet", "text", "content", "description"],
        n,
    ))
}

async fn exa(key: &str, query: &str, n: usize) -> Result<Vec<Hit>> {
    let resp = client()?
        .post("https://api.exa.ai/search")
        .header("x-api-key", key)
        .json(&json!({
            "query": query,
            "numResults": n,
            "contents": { "highlights": { "maxCharacters": SNIPPET_CHARS } }
        }))
        .send()
        .await
        .context("Exa")?;
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        bail!("Exa: HTTP {status}: {}", clip(&v.to_string(), 200));
    }
    let arr = v
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut hits = hits_from(&arr, &["title"], &["url"], &["text", "snippet"], n);
    // Highlights come as an array of strings.
    for (h, r) in hits.iter_mut().zip(arr.iter()) {
        if h.snippet.is_empty() {
            if let Some(hl) = r.get("highlights").and_then(Value::as_array) {
                h.snippet = hl
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" ");
            }
        }
    }
    Ok(hits)
}

async fn brave(key: &str, query: &str, n: usize) -> Result<Vec<Hit>> {
    let resp = client()?
        .get("https://api.search.brave.com/res/v1/web/search")
        .query(&[("q", query), ("count", &n.to_string())])
        .header("X-Subscription-Token", key)
        .header("Accept", "application/json")
        .send()
        .await
        .context("Brave")?;
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        bail!("Brave: HTTP {status}: {}", clip(&v.to_string(), 200));
    }
    let arr = v
        .pointer("/web/results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(hits_from(
        &arr,
        &["title"],
        &["url"],
        &["description", "snippet"],
        n,
    ))
}

async fn tavily(key: &str, query: &str, n: usize) -> Result<Vec<Hit>> {
    let resp = client()?
        .post("https://api.tavily.com/search")
        .json(&json!({ "api_key": key, "query": query, "max_results": n }))
        .send()
        .await
        .context("Tavily")?;
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        bail!("Tavily: HTTP {status}: {}", clip(&v.to_string(), 200));
    }
    let arr = v
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(hits_from(
        &arr,
        &["title"],
        &["url"],
        &["content", "snippet"],
        n,
    ))
}

/// OpenRouter's web plugin: one small, non-streaming chat call with
/// `plugins: [{id: "web"}]`; the sources come back as `url_citation`
/// annotations on the answer. The same key that runs the model.
async fn openrouter_web(web: &WebCfg, query: &str, n: usize) -> Result<Vec<Hit>> {
    let key = web.api_key.clone().unwrap_or_default();
    let base = web.api_base.trim_end_matches('/');
    // A small model: its answer is thrown away, only the sources count.
    // Some providers abort plugin calls (Gemini Flash gave 504s), so the
    // default is fixed rather than the chat's own model.
    let model = if web.model.is_empty() {
        "openai/gpt-5.4-mini".to_string()
    } else {
        web.model.clone()
    };
    let resp = client()?
        .post(format!("{base}/chat/completions"))
        .bearer_auth(&key)
        .header("HTTP-Referer", "https://github.com/unarbos/arbos")
        .header("X-Title", "Arbos")
        .json(&json!({
            "model": model,
            "plugins": [{ "id": "web", "max_results": n }],
            "max_tokens": 400,
            "messages": [{
                "role": "user",
                "content": format!("Search the web for: {query}\nReply with one line per relevant source: its title and what it says about the query. Use the search results; do not answer from memory.")
            }]
        }))
        .send()
        .await
        .context("OpenRouter web")?;
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let msg = v
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        bail!("OpenRouter web: HTTP {status}: {}", clip(&msg, 200));
    }
    let message = v
        .pointer("/choices/0/message")
        .cloned()
        .unwrap_or(Value::Null);
    let mut hits = Vec::new();
    if let Some(anns) = message.get("annotations").and_then(Value::as_array) {
        for a in anns {
            let Some(c) = a.get("url_citation") else {
                continue;
            };
            let url = c
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if url.is_empty() || hits.iter().any(|h: &Hit| h.url == url) {
                continue;
            }
            hits.push(Hit {
                title: c
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                url,
                snippet: c
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            });
            if hits.len() >= n {
                break;
            }
        }
    }
    Ok(hits)
}

/// DuckDuckGo's HTML page: each result block has a `result__a` link, and
/// a `result__snippet`. No key, no JSON.
pub async fn duckduckgo(query: &str, n: usize) -> Result<Vec<Hit>> {
    let url = format!("https://html.duckduckgo.com/html/?q={}", urlencoding(query));
    let resp = client()?.get(url).send().await.context("DuckDuckGo")?;
    let html = resp.text().await.unwrap_or_default();
    if html.contains("anomaly-modal") || html.contains("bots use DuckDuckGo too") {
        bail!("DuckDuckGo answered with a bot check (CAPTCHA) instead of results");
    }
    let mut hits = Vec::new();
    for block in html.split("class=\"result__body\"").skip(1) {
        let Some(url) = attr_after(block, "class=\"result__a\"", "href=\"") else {
            continue;
        };
        let url = ddg_target(&html_unescape(&url));
        if !url.starts_with("http") || url.contains("duckduckgo.com") {
            continue;
        }
        let title = block
            .find("class=\"result__a\"")
            .and_then(|i| block[i..].find('>').map(|j| i + j + 1))
            .and_then(|start| {
                block[start..]
                    .find("</a>")
                    .map(|end| &block[start..start + end])
            })
            .map(html_to_text)
            .unwrap_or_default();
        let snippet = block
            .find("class=\"result__snippet\"")
            .and_then(|i| block[i..].find('>').map(|j| i + j + 1))
            .and_then(|start| {
                block[start..]
                    .find("</a>")
                    .or_else(|| block[start..].find("</div>"))
                    .map(|end| &block[start..start + end])
            })
            .map(html_to_text)
            .unwrap_or_default();
        if hits.iter().any(|h: &Hit| h.url == url) {
            continue;
        }
        hits.push(Hit {
            title,
            url,
            snippet,
        });
        if hits.len() >= n {
            break;
        }
    }
    Ok(hits)
}

/// DuckDuckGo wraps targets as `//duckduckgo.com/l/?uddg=<encoded>&…`.
fn ddg_target(href: &str) -> String {
    if let Some(i) = href.find("uddg=") {
        let rest = &href[i + 5..];
        let end = rest.find('&').unwrap_or(rest.len());
        return urldecode(&rest[..end]);
    }
    if let Some(stripped) = href.strip_prefix("//") {
        return format!("https://{stripped}");
    }
    href.to_string()
}

/// The value of `attr` in the tag that contains `marker`, searching
/// forward from the marker within the same tag.
fn attr_after(block: &str, marker: &str, attr: &str) -> Option<String> {
    let i = block.find(marker)?;
    let tag_start = block[..i].rfind('<')?;
    let tag_end = block[i..].find('>')? + i;
    let tag = &block[tag_start..tag_end];
    let j = tag.find(attr)? + attr.len();
    let end = tag[j..].find('"')? + j;
    Some(tag[j..end].to_string())
}

fn hits_from(
    arr: &[Value],
    titles: &[&str],
    urls: &[&str],
    snippets: &[&str],
    n: usize,
) -> Vec<Hit> {
    let pick = |v: &Value, keys: &[&str]| -> String {
        keys.iter()
            .find_map(|k| v.get(*k).and_then(Value::as_str))
            .unwrap_or("")
            .to_string()
    };
    let mut out = Vec::new();
    for v in arr {
        let url = pick(v, urls);
        if url.is_empty() || out.iter().any(|h: &Hit| h.url == url) {
            continue;
        }
        out.push(Hit {
            title: pick(v, titles),
            url,
            snippet: pick(v, snippets),
        });
        if out.len() >= n {
            break;
        }
    }
    out
}

fn urlencoding(s: &str) -> String {
    s.bytes()
        .flat_map(|b| {
            if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' {
                vec![b as char]
            } else if b == b' ' {
                vec!['+']
            } else {
                format!("%{b:02X}").chars().collect()
            }
        })
        .collect()
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
                out.push(b'%');
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let mut skip = false;
    let lower = html.to_ascii_lowercase();
    for (i, c) in html.char_indices() {
        if c == '<' {
            in_tag = true;
            let tail = &lower[i..];
            skip = tail.starts_with("<script") || tail.starts_with("<style");
            if tail.starts_with("<br") || tail.starts_with("<p") || tail.starts_with("<li") {
                out.push('\n');
            }
        } else if c == '>' {
            in_tag = false;
            skip = false;
        } else if !in_tag && !skip {
            out.push(c);
        }
    }
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    html_unescape(&collapsed)
}

#[cfg(test)]
mod no_search_tests {
    use super::*;

    /// Two providers' failures were one red line the person had to read
    /// twice; now the refusal leads with the one thing to do, carries the
    /// providers' words once per process, and is one sentence after.
    #[test]
    fn the_refusal_leads_with_the_fix_and_carries_detail_once() {
        SAID_NO_SEARCH.store(false, std::sync::atomic::Ordering::SeqCst);
        let first =
            no_search_here("DuckDuckGo's keyless page answered with a bot check").to_string();
        assert!(
            first.starts_with("No web search on this kernel: add a search key"),
            "{first}"
        );
        assert!(first.contains("EXA_API_KEY"), "{first}");
        assert!(first.contains("do not retry search"), "{first}");
        assert!(
            first.contains("(DuckDuckGo's keyless page answered with a bot check)"),
            "{first}"
        );
        let second = no_search_here("anything").to_string();
        assert!(
            second.starts_with("No web search on this kernel"),
            "{second}"
        );
        assert!(
            !second.contains("anything"),
            "the detail is said once: {second}"
        );
        assert!(is_no_search(&anyhow::anyhow!(
            "DuckDuckGo answered with a bot check (CAPTCHA) instead of results"
        )));
        assert!(!is_no_search(&anyhow::anyhow!(
            "DuckDuckGo: connection refused"
        )));
    }
}
