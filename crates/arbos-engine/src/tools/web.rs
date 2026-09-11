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
    fn run(&self, _cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        Box::pin(async move { search(req(&args, "query")?).await })
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

pub async fn search(query: &str) -> Result<ToolOut> {
    // DuckDuckGo HTML is enough without a product key.
    let url = format!("https://html.duckduckgo.com/html/?q={}", urlencoding(query));
    let client = reqwest::Client::builder().user_agent("arbos/0.1").build()?;
    let resp = client.get(url).send().await?;
    let html = resp.text().await.unwrap_or_default();
    let mut hits = Vec::new();
    for cap in link_hrefs(&html) {
        if cap.starts_with("http") && !cap.contains("duckduckgo.com") {
            hits.push(cap);
        }
        if hits.len() >= 8 {
            break;
        }
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
