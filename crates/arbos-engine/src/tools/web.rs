use anyhow::{Result, bail};
use serde_json::Value;

use super::ToolOut;
use crate::access::Access;
use crate::tool::{BoxFuture, Plan, PlanCx, RunCx, Tool, req, simple_schema};

pub struct Fetch;
pub struct Search;

impl Tool for Fetch {
    fn name(&self) -> &'static str {
        "fetch"
    }
    fn schema(&self) -> Value {
        simple_schema(
            "fetch",
            "Fetch a URL as text.",
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
        simple_schema("search", "Web search.", &[("query", "Search query.", true)])
    }
    fn plan(&self, _cx: &PlanCx, _args: &Value) -> Result<Plan> {
        Ok(Plan::access(Access::none()))
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        Box::pin(
            async move { search_with(&cx.search_url, &cx.search_key, req(&args, "query")?).await },
        )
    }
}

pub async fn fetch(url: &str) -> Result<ToolOut> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        bail!("fetch only http(s)");
    }
    let client = reqwest::Client::builder()
        .user_agent("arbos/0.1")
        .redirect(reqwest::redirect::Policy::limited(8))
        .build()?;
    let resp = client.get(url).send().await?;
    let status = resp.status();
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
    Ok(ToolOut::text(format!("HTTP {status} {url}\n{body}")))
}

const DEFAULT_SEARCH: &str = "https://html.duckduckgo.com/html/?q={q}";

pub async fn search(query: &str) -> Result<ToolOut> {
    search_with("", "", query).await
}

/// `search_url` is a template with `{q}`; empty means the built-in engine.
/// A page that is a block, a challenge, or a non-2xx answer is an error
/// the model can act on, not an empty result (QA bug qa-010).
pub async fn search_with(search_url: &str, search_key: &str, query: &str) -> Result<ToolOut> {
    let template = if search_url.trim().is_empty() {
        DEFAULT_SEARCH
    } else {
        search_url.trim()
    };
    let url = search_query_url(template, query);
    let host = url
        .split('/')
        .nth(2)
        .unwrap_or("the search engine")
        .to_string();
    let client = reqwest::Client::builder().user_agent("arbos/0.1").build()?;
    let mut req = client.get(&url);
    if !search_key.is_empty() {
        req = req.bearer_auth(search_key).header("X-API-Key", search_key);
    }
    let resp = req.send().await?;
    let status = resp.status();
    let html = resp.text().await.unwrap_or_default();
    let hits = result_links(&html, &host);
    if let Some(why) = search_block(status.as_u16(), &html, &hits) {
        bail!(
            "search blocked: {host} answered {why}. This is not \"no results\". Use fetch on a URL you already know, or ask the user to set search_url/search_key in the host config."
        );
    }
    let snippets = html_to_text(&html);
    let mut body = String::new();
    for (i, h) in hits.iter().enumerate() {
        body.push_str(&format!("{}. {h}\n", i + 1));
    }
    body.push('\n');
    body.push_str(&crate::evict::evict_body(&snippets, "search"));
    Ok(ToolOut::text(body))
}

pub fn search_query_url(template: &str, query: &str) -> String {
    if template.contains("{q}") {
        template.replace("{q}", &urlencoding(query))
    } else {
        format!("{template}{}", urlencoding(query))
    }
}

/// Result links: `http(s)` hrefs that do not point back at the engine.
pub fn result_links(html: &str, engine_host: &str) -> Vec<String> {
    let mut hits = Vec::new();
    for cap in link_hrefs(html) {
        if cap.starts_with("http") && !cap.contains(engine_host) && !cap.contains("duckduckgo.com")
        {
            hits.push(cap);
        }
        if hits.len() >= 8 {
            break;
        }
    }
    hits
}

/// Why a search answer is a block rather than a result page, or None.
pub fn search_block(status: u16, html: &str, hits: &[String]) -> Option<String> {
    // A search page answers 200. DuckDuckGo serves its bot challenge as
    // 202; 403 and 429 are refusals.
    if status != 200 {
        return Some(format!("HTTP {status}"));
    }
    let lower = html.to_ascii_lowercase();
    let markers = [
        "bots use duckduckgo too",
        "anomaly",
        "unusual traffic",
        "automated requests",
        "are you a robot",
        "captcha",
        "access denied",
        "verify you are human",
        "rate limit",
    ];
    if let Some(m) = markers.iter().find(|m| lower.contains(*m)) {
        return Some(format!("a challenge page ({m:?})"));
    }
    if hits.is_empty() && lower.contains("<form") && lower.len() < 20_000 {
        // No results and only a form: the engine wants something from a
        // person, not a query.
        return Some("a page with no results and a form".into());
    }
    None
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

fn link_hrefs(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find("href=\"") {
        rest = &rest[i + 6..];
        if let Some(end) = rest.find('"') {
            out.push(html_unescape(&rest[..end]));
            rest = &rest[end + 1..];
        } else {
            break;
        }
    }
    out
}

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
}

fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let mut skip = false;
    let lower = html.to_ascii_lowercase();
    let bytes = html.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            in_tag = true;
            let tail = &lower[i..];
            skip = tail.starts_with("<script") || tail.starts_with("<style");
            if tail.starts_with("<br") || tail.starts_with("<p") || tail.starts_with("<li") {
                out.push('\n');
            }
        } else if bytes[i] == b'>' {
            in_tag = false;
            skip = false;
        } else if !in_tag && !skip {
            out.push(bytes[i] as char);
        }
        i += 1;
    }
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    html_unescape(&collapsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_page_is_an_error_not_an_empty_result() {
        let ddg = r#"<html><body><div class="anomaly-modal__title">Unfortunately, bots use DuckDuckGo too.</div><form action="/html/"><input name="q"></form></body></html>"#;
        let hits = result_links(ddg, "html.duckduckgo.com");
        assert!(hits.is_empty());
        let why = search_block(200, ddg, &hits).expect("block detected");
        assert!(why.contains("challenge"), "{why}");
        assert!(
            search_block(202, "<html></html>", &[])
                .unwrap()
                .contains("HTTP 202")
        );
        assert!(search_block(429, "", &[]).unwrap().contains("HTTP 429"));
        assert!(
            search_block(
                200,
                r#"<html><body><form><input name="q"></form></body></html>"#,
                &[]
            )
            .is_some()
        );
    }

    #[test]
    fn a_result_page_is_not_a_block() {
        let page = r#"<html><body><a href="https://html.duckduckgo.com/settings">s</a><a class="result__a" href="https://example.com/a">A</a><a href="https://example.org/b">B</a></body></html>"#;
        let hits = result_links(page, "html.duckduckgo.com");
        assert_eq!(hits, ["https://example.com/a", "https://example.org/b"]);
        assert!(search_block(200, page, &hits).is_none());
        // An honest empty result without a form is still a result.
        assert!(search_block(200, "<html><body>No results for xyzzy</body></html>", &[]).is_none());
    }

    #[test]
    fn the_configured_provider_template_is_used() {
        assert_eq!(
            search_query_url(
                "https://searx.local/search?format=html&q={q}",
                "full duplex"
            ),
            "https://searx.local/search?format=html&q=full+duplex"
        );
        assert_eq!(
            search_query_url("https://api.example/search?q=", "a b"),
            "https://api.example/search?q=a+b"
        );
        assert!(
            search_query_url(DEFAULT_SEARCH, "x")
                .starts_with("https://html.duckduckgo.com/html/?q=x")
        );
    }
}
