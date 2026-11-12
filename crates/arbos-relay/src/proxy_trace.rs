//! What a model call looked like, in terms that can go in the journal.
//!
//! The proxy sees every prompt in full and must log none of it. What it logs instead is the
//! shape: a hash per cacheable block, where the cache breakpoints were set and for how long,
//! and what the upstream said about which backend answered. That is enough to tell a prompt
//! head that moved from a backend that did not share its cache, and not enough to reconstruct
//! anything that was said.

use std::fmt;

use axum::http::HeaderMap;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Hex characters kept per hash. Half a SHA-256 is far more than enough to tell two prompt
/// heads apart, and short enough to read off a journal line.
pub const HASH_HEX: usize = 16;

/// Longest single header value kept, in bytes.
const VALUE_MAX: usize = 96;

/// Longest bucket of header values kept, in bytes.
const BUCKET_MAX: usize = 128;

/// The cacheable shape of one request body.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PromptShape {
    /// Hash of the system prompt's text, or `None` when the body carries no system prompt.
    pub system_sha: Option<String>,
    /// Hash of the tool definitions, cache markers excluded.
    pub tools_sha: Option<String>,
    /// Hash of system and tools together: the head two requests have to share to share a cache.
    pub prefix_sha: Option<String>,
    pub tools: usize,
    pub messages: usize,
    /// Marked blocks per location, e.g. `system:1,tools:1,messages:2`. Empty when none is marked,
    /// which means nothing in the request was offered for caching at all.
    pub cache_marks: String,
    /// The longest retention the request asked for, e.g. `5m` or `1h`. Empty when none was.
    pub cache_ttl: String,
}

/// Read the shape off a request body. Never fails: an unparsable body is a body with no head.
pub fn prompt_shape(body: &Value) -> PromptShape {
    let mut shape = PromptShape::default();
    let mut marks = Marks::default();

    let messages = body.get("messages").and_then(Value::as_array);
    shape.messages = messages.map_or(0, Vec::len);

    let system = match body.get("system") {
        Some(system) => {
            marks.count("system", system);
            Some(block_text(system))
        }
        // The OpenAI-compatible shape keeps the system prompt at the head of `messages`.
        None => messages
            .and_then(|m| m.first())
            .filter(|m| m.get("role").and_then(Value::as_str) == Some("system"))
            .map(|m| {
                marks.count("system", m);
                block_text(m.get("content").unwrap_or(&Value::Null))
            }),
    };
    shape.system_sha = system.as_deref().map(short_sha);

    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
        shape.tools = tools.len();
        for tool in tools {
            marks.count("tools", tool);
        }
        let stripped = Value::Array(tools.iter().map(without_cache_control).collect());
        shape.tools_sha = Some(short_sha(&stripped.to_string()));
    }

    if let Some(messages) = messages {
        for (i, message) in messages.iter().enumerate() {
            // The system message was already counted under `system`.
            if i == 0 && body.get("system").is_none() && shape.system_sha.is_some() {
                continue;
            }
            marks.count("messages", message);
        }
    }

    shape.prefix_sha = match (&shape.system_sha, &shape.tools_sha) {
        (None, None) => None,
        (system, tools) => Some(short_sha(&format!(
            "{}|{}",
            system.as_deref().unwrap_or(""),
            tools.as_deref().unwrap_or("")
        ))),
    };
    shape.cache_marks = marks.render();
    shape.cache_ttl = marks.longest_ttl();
    shape
}

/// What the upstream said about itself, from the response headers.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct UpstreamMarks {
    /// Which backend answered, as `name=value`. OpenRouter routes one model to several.
    pub backend: String,
    /// Whatever the upstream reported about caching, as `name=value`.
    pub cache: String,
    /// The upstream's own request id, for searching its logs by.
    pub upstream_id: String,
}

/// Pick the named headers out of an upstream response. Everything else is dropped: the response
/// can carry cookies and auth echoes, and this goes to the journal.
pub fn upstream_marks(headers: &HeaderMap) -> UpstreamMarks {
    let mut marks = UpstreamMarks::default();
    for (name, value) in headers {
        let name = name.as_str();
        let Ok(value) = value.to_str() else { continue };
        let bucket = if name.starts_with("x-openrouter") || name.starts_with("x-provider") {
            &mut marks.backend
        } else if name.contains("cache") {
            &mut marks.cache
        } else if name == "x-request-id" || name == "request-id" {
            if marks.upstream_id.is_empty() {
                marks.upstream_id = clip(value);
            }
            continue;
        } else {
            continue;
        };
        if !bucket.is_empty() {
            bucket.push(',');
        }
        bucket.push_str(&clip(&format!("{name}={value}")));
        *bucket = clip_to(bucket, BUCKET_MAX);
    }
    marks
}

/// The generation id and the backend as the stream itself reports them. OpenRouter puts both in
/// every chunk; Anthropic direct puts its message id in `message_start` and names no provider.
pub fn generation_marks(text: &str) -> (Option<String>, Option<String>) {
    let mut id = None;
    let mut provider = None;
    for line in std::iter::once(text).chain(text.lines()) {
        let payload = line
            .strip_prefix("data:")
            .map(str::trim)
            .unwrap_or(line.trim());
        if payload.is_empty() || payload == "[DONE]" {
            continue;
        }
        let Ok(chunk) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        if id.is_none() {
            id = chunk
                .get("id")
                .or_else(|| chunk.pointer("/message/id"))
                .or_else(|| chunk.pointer("/response/id"))
                .and_then(Value::as_str)
                .map(clip);
        }
        if provider.is_none() {
            provider = chunk.get("provider").and_then(Value::as_str).map(clip);
        }
        if id.is_some() && provider.is_some() {
            break;
        }
    }
    (id, provider)
}

/// Marked blocks per location, in the order the locations are reported.
#[derive(Default)]
struct Marks {
    counts: Vec<(&'static str, usize)>,
    ttl_seconds: u64,
    ttl: String,
}

impl Marks {
    /// Count every `cache_control` at or under `value`, and remember the longest ttl asked for.
    fn count(&mut self, location: &'static str, value: &Value) {
        let found = self.walk(value);
        if found == 0 {
            return;
        }
        match self.counts.iter_mut().find(|(name, _)| *name == location) {
            Some((_, n)) => *n += found,
            None => self.counts.push((location, found)),
        }
    }

    fn walk(&mut self, value: &Value) -> usize {
        match value {
            Value::Object(map) => {
                let mut found = 0;
                if let Some(control) = map.get("cache_control") {
                    found += 1;
                    let ttl = control
                        .get("ttl")
                        .and_then(Value::as_str)
                        // An ephemeral breakpoint with no ttl is Anthropic's five-minute default.
                        .unwrap_or("5m");
                    let seconds = ttl_seconds(ttl);
                    if seconds >= self.ttl_seconds {
                        self.ttl_seconds = seconds;
                        self.ttl = ttl.to_string();
                    }
                }
                for (key, nested) in map {
                    if key != "cache_control" {
                        found += self.walk(nested);
                    }
                }
                found
            }
            Value::Array(items) => items.iter().map(|item| self.walk(item)).sum(),
            _ => 0,
        }
    }

    fn render(&self) -> String {
        self.counts
            .iter()
            .map(|(name, n)| format!("{name}:{n}"))
            .collect::<Vec<_>>()
            .join(",")
    }

    fn longest_ttl(&self) -> String {
        self.ttl.clone()
    }
}

fn ttl_seconds(ttl: &str) -> u64 {
    let digits: String = ttl.chars().take_while(char::is_ascii_digit).collect();
    let n: u64 = digits.parse().unwrap_or(0);
    match ttl.trim_start_matches(|c: char| c.is_ascii_digit()) {
        "h" => n * 3600,
        "d" => n * 86_400,
        _ => n * 60,
    }
}

/// The text of a block, a list of blocks, or a bare string, with everything else ignored.
fn block_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(block_text).collect::<Vec<_>>().join("\n"),
        Value::Object(map) => map
            .get("text")
            .map(block_text)
            .unwrap_or_else(|| map.get("content").map(block_text).unwrap_or_default()),
        _ => String::new(),
    }
}

/// The same value with every `cache_control` removed: where the breakpoints sit is reported
/// separately, and moving one must not look like a changed tool.
fn without_cache_control(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, _)| key.as_str() != "cache_control")
                .map(|(key, nested)| (key.clone(), without_cache_control(nested)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_cache_control).collect()),
        other => other.clone(),
    }
}

fn short_sha(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    hex::encode(digest)[..HASH_HEX].to_string()
}

fn clip(value: &str) -> String {
    let mut out = clip_to(value, VALUE_MAX);
    if out.len() < value.len() {
        out.push('~');
    }
    out
}

/// At most `max` bytes, cut on a character boundary.
fn clip_to(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.to_string();
    }
    let mut end = max;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

impl fmt::Display for PromptShape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "prefix={} tools={} messages={} marks={}",
            self.prefix_sha.as_deref().unwrap_or("-"),
            self.tools,
            self.messages,
            if self.cache_marks.is_empty() {
                "-"
            } else {
                self.cache_marks.as_str()
            }
        )
    }
}
