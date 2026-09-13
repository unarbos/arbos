use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{sync::OnceLock, time::Duration};
use tokio_util::sync::CancellationToken;

/// The turn was stopped while the model was streaming.
#[derive(Debug)]
pub struct Interrupted;

impl std::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("interrupted")
    }
}

impl std::error::Error for Interrupted {}

/// Where a provider call failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailKind {
    /// Could not connect, or the connection dropped.
    Transport,
    /// No bytes for `stream_idle`.
    Idle,
    /// Non-2xx response.
    Status,
    /// An `{"error": …}` frame inside the SSE stream.
    Stream,
}

/// A provider call that did not complete. Carries what the retry policy
/// needs: the status, the server's own retry hints, and whether the user has
/// already seen part of this attempt's answer.
#[derive(Debug, Clone)]
pub struct ProviderError {
    pub kind: FailKind,
    pub status: Option<u16>,
    pub message: String,
    /// `retry-after-ms` or `Retry-After`, when the server sent one.
    pub retry_after: Option<Duration>,
    /// `x-should-retry: true|false`, when the server sent one.
    pub should_retry: Option<bool>,
    /// Assistant text already streamed to the window in this attempt. A
    /// retry would append a second answer under the first.
    pub visible: bool,
    /// That text, so the turn can keep it and carry on from there.
    pub partial: String,
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.kind, self.status) {
            (FailKind::Status, Some(s)) => write!(f, "{} {}", s, status_label(s))?,
            (FailKind::Idle, _) => f.write_str("no data from provider")?,
            (FailKind::Transport, _) => f.write_str("connection failed")?,
            (FailKind::Stream, _) => f.write_str("provider error mid-stream")?,
            (FailKind::Status, None) => f.write_str("provider error")?,
        }
        if !self.message.is_empty() {
            write!(f, ": {}", self.message)?;
        }
        Ok(())
    }
}

impl std::error::Error for ProviderError {}

/// A short human label, the way Claude Code names the reason in its
/// "Retrying in Ns" line.
pub fn status_label(status: u16) -> &'static str {
    match status {
        400 => "bad request",
        401 => "bad API key",
        402 => "billing",
        403 => "forbidden",
        404 => "model not found",
        408 => "request timeout",
        409 => "conflict",
        413 => "request too large",
        422 => "unprocessable",
        425 => "too early",
        429 => "rate limited",
        500 => "server error",
        502 => "bad gateway",
        503 => "unavailable",
        504 => "gateway timeout",
        529 => "overloaded",
        _ => "error",
    }
}

fn header_duration(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    if let Some(ms) = headers
        .get("retry-after-ms")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
    {
        return Some(Duration::from_millis(ms.max(0.0) as u64));
    }
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map(|secs| Duration::from_secs_f64(secs.max(0.0)))
}

fn header_should_retry(headers: &reqwest::header::HeaderMap) -> Option<bool> {
    match headers.get("x-should-retry").and_then(|v| v.to_str().ok()) {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    }
}

/// Keep the useful part of an error body: the `message` if it is JSON,
/// else the first line, capped.
fn error_message(body: &str) -> String {
    let text = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message").or(Some(e)))
                .and_then(|m| m.as_str().map(str::to_string))
        })
        .unwrap_or_else(|| body.lines().next().unwrap_or("").to_string());
    let text = text.trim();
    if text.chars().count() > 240 {
        let cut: String = text.chars().take(240).collect();
        format!("{cut}…")
    } else {
        text.to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Images sent beside `content` as `image_url` parts. Only `user`
    /// messages may carry them on Chat Completions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImagePart>,
    /// Provider reasoning blocks from the call that produced this assistant
    /// message, sent back verbatim. See `Completion::reasoning_details`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_details: Option<Value>,
}

impl ChatMessage {
    pub fn plain(role: &str, content: Option<String>) -> Self {
        Self {
            role: role.into(),
            content,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            images: Vec::new(),
            reasoning_details: None,
        }
    }
}

/// One image on the wire. The kernel never writes this to disk; it is
/// built at projection time from a file path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImagePart {
    pub mime: String,
    pub b64: String,
}

impl From<crate::image::ImagePart> for ImagePart {
    fn from(p: crate::image::ImagePart) -> Self {
        Self {
            mime: p.mime.to_string(),
            b64: p.b64,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

pub enum Delta {
    Text(String),
    Thinking(String),
    /// A tool call whose arguments are complete. Fired as soon as we can
    /// tell, so the executor can start it while the model keeps streaming.
    Call(ToolCall),
}

#[derive(Debug, Clone)]
pub struct Provider {
    pub base: String,
    pub key: String,
    pub model: String,
    pub reasoning_effort: Option<String>,
    /// Longest silence tolerated mid-stream before the call counts as lost.
    pub stream_idle: Duration,
    /// `max_tokens` to send. None = omit the field.
    pub max_tokens: Option<u64>,
    /// Folder that gets one JSON file per call with the request, the
    /// response headers, every raw chunk with its arrival time, and the
    /// parsed result. None = no tracing.
    pub trace: Option<std::path::PathBuf>,
}

/// Everything one provider call did, for the trace file.
#[derive(Debug, Default, Serialize)]
struct Trace {
    started_ms: i64,
    ended_ms: i64,
    url: String,
    model: String,
    request: Value,
    status: Option<u16>,
    headers: Vec<(String, String)>,
    /// `(ms since start, raw bytes as text)` per chunk.
    chunks: Vec<(u64, String)>,
    content: String,
    calls: Vec<ToolCall>,
    usage: Option<(u64, u64)>,
    error: Option<String>,
}

impl Trace {
    fn write(&mut self, dir: &Option<std::path::PathBuf>) {
        let Some(dir) = dir else { return };
        self.ended_ms = now_ms();
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
        let n = std::fs::read_dir(dir).map(|d| d.count()).unwrap_or(0);
        let path = dir.join(format!("{:04}-{}.json", n + 1, self.started_ms));
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, text);
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn http() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent("arbos/0.1")
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(4)
            .tcp_nodelay(true)
            .tcp_keepalive(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(8))
            .build()
            .expect("reqwest client")
    })
}

/// Open the TLS + HTTP/2 session before the first user turn, and remember
/// the model list so `context_window` can answer without a round trip.
pub async fn warm(base: &str, key: &str) {
    let _ = models_list(base, key).await;
}

/// `{base}/models`, fetched once per process per base.
async fn models_list(base: &str, key: &str) -> Option<Value> {
    static CACHE: OnceLock<std::sync::Mutex<std::collections::HashMap<String, Value>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(v) = cache.lock().ok().and_then(|c| c.get(base).cloned()) {
        return Some(v);
    }
    let url = format!("{}/models", base.trim_end_matches('/'));
    let resp = http().get(url).bearer_auth(key).send().await.ok()?;
    let v: Value = resp.json().await.ok()?;
    if let Ok(mut c) = cache.lock() {
        c.insert(base.to_string(), v.clone());
    }
    Some(v)
}

/// The model's context window in tokens, when the provider says. OpenRouter
/// lists `context_length` per model and per serving provider; the smaller
/// of the two is what a request can count on. OpenAI's own `/models` lists
/// neither, so this is None there and the caller keeps its default.
pub async fn context_window(base: &str, key: &str, model: &str) -> Option<u64> {
    let m = model_entry(base, key, model).await?;
    let listed = m.get("context_length").and_then(Value::as_u64);
    let served = m
        .get("top_provider")
        .and_then(|t| t.get("context_length"))
        .and_then(Value::as_u64);
    match (listed, served) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// The most tokens one completion may have, when the provider says.
pub async fn max_completion_tokens(base: &str, key: &str, model: &str) -> Option<u64> {
    let m = model_entry(base, key, model).await?;
    m.get("top_provider")
        .and_then(|t| t.get("max_completion_tokens"))
        .or_else(|| m.get("max_completion_tokens"))
        .and_then(Value::as_u64)
}

async fn model_entry(base: &str, key: &str, model: &str) -> Option<Value> {
    let list = models_list(base, key).await?;
    let data = list.get("data")?.as_array()?;
    data.iter()
        .find(|m| m.get("id").and_then(Value::as_str) == Some(model))
        .cloned()
}

/// One finished model call.
#[derive(Debug, Default)]
pub struct Completion {
    pub content: String,
    pub calls: Vec<ToolCall>,
    /// `(prompt_tokens, total_tokens)` when the provider reported usage.
    pub usage: Option<(u64, u64)>,
    /// This call's price in US dollars, when the provider reported it
    /// (OpenRouter `usage.cost`, asked for with `usage: {include: true}`).
    pub cost: Option<f64>,
    /// `reasoning_details` blocks, to be sent back with this assistant
    /// message on later calls. Gemini 3 stops thinking without its thought
    /// signatures; Anthropic rejects a broken thinking chain.
    pub reasoning_details: Vec<Value>,
}

impl Provider {
    pub async fn complete(&self, messages: &[ChatMessage], tools: &[Value]) -> Result<Completion> {
        self.complete_stream(messages, tools, &CancellationToken::new(), |_| {})
            .await
    }

    pub async fn complete_stream(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        cancel: &CancellationToken,
        mut on_delta: impl FnMut(Delta),
    ) -> Result<Completion> {
        let mut msgs = messages_json(messages);
        if wants_cache_control(&self.model) {
            mark_cache_breakpoints(&mut msgs);
        }
        let mut body = json!({
            "model": self.model,
            "messages": msgs,
            "stream": true,
        });
        if self.base.contains("openai.com") {
            body["stream_options"] = json!({ "include_usage": true });
        }
        // OpenRouter streams token counts by default; the price only when
        // asked. Other endpoints ignore the key or reject it, so it is
        // sent to OpenRouter alone.
        if self.base.contains("openrouter.ai") {
            body["usage"] = json!({ "include": true });
        }
        if let Some(n) = self.max_tokens {
            body["max_tokens"] = json!(n);
        }
        // No tools → no tool_choice. OpenAI rejects tool_choice without tools.
        if !tools.is_empty() {
            body["tools"] = json!(tools);
            body["tool_choice"] = json!("auto");
            body["parallel_tool_calls"] = json!(true);
        }
        if let Some(effort) = reasoning_effort(
            &self.model,
            &self.base,
            !tools.is_empty(),
            self.reasoning_effort.as_deref(),
        ) {
            body["reasoning_effort"] = json!(effort);
        }
        let url = format!("{}/chat/completions", self.base.trim_end_matches('/'));
        let mut trace = Trace {
            started_ms: now_ms(),
            url: url.clone(),
            model: self.model.clone(),
            request: if self.trace.is_some() {
                body.clone()
            } else {
                Value::Null
            },
            ..Trace::default()
        };
        let result = self
            .stream_inner(url, body, cancel, &mut on_delta, &mut trace)
            .await;
        match &result {
            Ok(c) => {
                trace.content = c.content.clone();
                trace.calls = c.calls.clone();
                trace.usage = c.usage;
            }
            Err(e) => trace.error = Some(format!("{e:#}")),
        }
        trace.write(&self.trace);
        result
    }

    async fn stream_inner(
        &self,
        url: String,
        body: Value,
        cancel: &CancellationToken,
        on_delta: &mut impl FnMut(Delta),
        trace: &mut Trace,
    ) -> Result<Completion> {
        let t0 = std::time::Instant::now();
        let request = http().post(url).bearer_auth(&self.key).json(&body).send();
        // The wait for headers is bounded like the wait for each chunk. A
        // provider that queues the request and says nothing held one call
        // for 195 s before its first byte; the connect timeout does not
        // cover that, and neither did anything else.
        let mut resp = tokio::select! {
            r = tokio::time::timeout(self.stream_idle, request) => match r {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    return Err(ProviderError {
                        kind: FailKind::Transport,
                        status: None,
                        message: e.to_string(),
                        retry_after: None,
                        should_retry: None,
                        visible: false,
                        partial: String::new(),
                    }
                    .into());
                }
                Err(_) => {
                    return Err(ProviderError {
                        kind: FailKind::Idle,
                        status: None,
                        message: format!("no response headers for {}s", self.stream_idle.as_secs()),
                        retry_after: None,
                        should_retry: None,
                        visible: false,
                        partial: String::new(),
                    }
                    .into());
                }
            },
            _ = cancel.cancelled() => return Err(Interrupted.into()),
        };
        let status = resp.status();
        trace.status = Some(status.as_u16());
        if self.trace.is_some() {
            trace.headers = resp
                .headers()
                .iter()
                .filter(|(k, _)| k.as_str() != "authorization")
                .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("<bin>").to_string()))
                .collect();
        }
        if !status.is_success() {
            let retry_after = header_duration(resp.headers());
            let should_retry = header_should_retry(resp.headers());
            let text = resp.text().await.unwrap_or_default();
            if self.trace.is_some() {
                trace
                    .chunks
                    .push((t0.elapsed().as_millis() as u64, text.clone()));
            }
            return Err(ProviderError {
                kind: FailKind::Status,
                status: Some(status.as_u16()),
                message: error_message(&text),
                retry_after,
                should_retry,
                visible: false,
                partial: String::new(),
            }
            .into());
        }

        let mut buf = String::new();
        let mut content = String::new();
        let mut calls: Vec<PartialCall> = Vec::new();
        let mut usage = None;
        let mut cost = None;
        let mut reasoning_details: Vec<Value> = Vec::new();
        // Time since the last delta that carried text, reasoning or tool
        // arguments. Keep-alive comments and empty deltas do not count: a
        // provider once held a stream open with a comment every 0.4 s for
        // 334 s while the model produced nothing.
        let mut last_progress = std::time::Instant::now();
        let fail = |kind: FailKind, message: String, content: &str| -> anyhow::Error {
            ProviderError {
                kind,
                status: None,
                message,
                retry_after: None,
                should_retry: None,
                visible: !content.is_empty(),
                partial: content.to_string(),
            }
            .into()
        };

        loop {
            let left = self.stream_idle.saturating_sub(last_progress.elapsed());
            if left.is_zero() {
                return Err(fail(
                    FailKind::Idle,
                    format!("no model output for {}s", self.stream_idle.as_secs()),
                    &content,
                ));
            }
            let chunk = tokio::select! {
                c = tokio::time::timeout(left, resp.chunk()) => match c {
                    Ok(Ok(c)) => c,
                    Ok(Err(e)) => return Err(fail(FailKind::Transport, e.to_string(), &content)),
                    Err(_) => return Err(fail(
                        FailKind::Idle,
                        format!("no model output for {}s", self.stream_idle.as_secs()),
                        &content,
                    )),
                },
                _ = cancel.cancelled() => return Err(Interrupted.into()),
            };
            // End of stream without `[DONE]`: whatever complete lines are
            // still buffered must be parsed, or a tool call's tail is lost.
            let stream_over = chunk.is_none();
            if let Some(chunk) = chunk {
                let text = String::from_utf8_lossy(&chunk);
                if self.trace.is_some() {
                    trace
                        .chunks
                        .push((t0.elapsed().as_millis() as u64, text.to_string()));
                }
                buf.push_str(&text);
            } else if !buf.ends_with('\n') {
                buf.push('\n');
            }
            while let Some(frame) = take_sse_line(&mut buf) {
                if frame == "[DONE]" {
                    emit_complete_calls(&mut calls, true, on_delta);
                    return Ok(Completion {
                        content,
                        calls: finish_calls(calls),
                        usage,
                        cost,
                        reasoning_details,
                    });
                }
                let Ok(v) = serde_json::from_str::<Value>(&frame) else {
                    continue;
                };
                if let Some(err) = v.get("error") {
                    let mut e = ProviderError {
                        kind: FailKind::Stream,
                        status: err.get("code").and_then(|c| c.as_u64()).map(|c| c as u16),
                        message: error_message(&frame),
                        retry_after: None,
                        should_retry: None,
                        visible: !content.is_empty(),
                        partial: content.clone(),
                    };
                    // Providers name overloads in the body, not the status.
                    let blob = frame.to_ascii_lowercase();
                    if [
                        "overloaded",
                        "rate_limit",
                        "rate limit",
                        "server_error",
                        "temporar",
                        "timeout",
                        "try again",
                    ]
                    .iter()
                    .any(|k| blob.contains(k))
                    {
                        e.should_retry = Some(true);
                    }
                    return Err(e.into());
                }
                if let Some(pair) = usage_of(&v) {
                    usage = Some(pair);
                }
                if let Some(c) = cost_of(&v) {
                    cost = Some(c);
                }
                let Some(choice) = v.get("choices").and_then(|c| c.get(0)) else {
                    continue;
                };
                let delta = choice.get("delta").unwrap_or(&Value::Null);
                if let Some(t) = text_field(delta, &["content"]) {
                    content.push_str(&t);
                    last_progress = std::time::Instant::now();
                    on_delta(Delta::Text(t));
                }
                if let Some(arr) = delta.get("reasoning_details").and_then(Value::as_array) {
                    merge_reasoning_details(&mut reasoning_details, arr);
                }
                if let Some(t) = text_field(delta, &["reasoning_content", "reasoning", "thinking"])
                {
                    last_progress = std::time::Instant::now();
                    on_delta(Delta::Thinking(t));
                }
                if merge_tool_deltas(&mut calls, delta.get("tool_calls")) {
                    last_progress = std::time::Instant::now();
                    // A model stuck repeating itself streams until the
                    // provider's token limit, minutes later. Cut it here:
                    // drop the connection, keep the calls we have, and mark
                    // this one so the tool result tells the model what
                    // happened. One failed step instead of a stalled turn.
                    if let Some(why) = calls
                        .iter()
                        .find(|c| !c.emitted)
                        .and_then(|c| runaway(&c.arguments))
                    {
                        for c in calls
                            .iter_mut()
                            .filter(|c| !c.emitted && runaway(&c.arguments).is_some())
                        {
                            c.arguments = json!({ BAD_ARGS: format!("{why} — {}…", tail_preview(&c.arguments)) }).to_string();
                        }
                        drop(resp);
                        emit_complete_calls(&mut calls, true, on_delta);
                        return Ok(Completion {
                            content,
                            calls: finish_calls(calls),
                            usage,
                            cost,
                            reasoning_details,
                        });
                    }
                    emit_complete_calls(&mut calls, false, on_delta);
                }
            }
            if stream_over {
                break;
            }
        }
        emit_complete_calls(&mut calls, true, on_delta);
        Ok(Completion {
            content,
            calls: finish_calls(calls),
            usage,
            cost,
            reasoning_details,
        })
    }
}

/// Streamed `reasoning_details` come as arrays of partial blocks. Text
/// blocks with the same id (or, lacking one, consecutive text blocks) are
/// one block whose text arrived in pieces; encrypted signature blocks are
/// whole. The result is what goes back on the next request, unchanged.
fn merge_reasoning_details(acc: &mut Vec<Value>, arr: &[Value]) {
    for item in arr {
        let is_text = item.get("type").and_then(Value::as_str) == Some("reasoning.text");
        let id = item.get("id").cloned();
        if is_text {
            if let Some(last) = acc.last_mut() {
                let same = last.get("type").and_then(Value::as_str) == Some("reasoning.text")
                    && (id.is_none() || last.get("id").cloned() == id);
                if same {
                    let add = item
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if let Some(Value::String(t)) = last.get_mut("text") {
                        t.push_str(&add);
                    }
                    // A signature may arrive on a later piece of the same block.
                    if let Some(sig) = item.get("signature") {
                        last["signature"] = sig.clone();
                    }
                    continue;
                }
            }
        }
        acc.push(item.clone());
    }
}

/// Bytes of arguments a single tool call may stream before it counts as
/// runaway regardless of content. Whole files go through `write`; nothing
/// legitimate is this large.
const MAX_ARGS_BYTES: usize = 256 * 1024;
/// How much tail to inspect for repetition, and the longest repeating unit.
const TAIL: usize = 2048;
const MAX_UNIT: usize = 24;

/// Share of distinct 8-byte windows in the tail below which the output is
/// repeating itself. Measured on real traces: the most repetitive genuine
/// edit (dense Rust match arms) scored 0.069; every degenerate stream
/// scored under 0.03, whatever its period.
const MIN_DISTINCT_RATIO: f64 = 0.05;
const WINDOW: usize = 8;

/// Why these arguments look like a model that has lost the thread, or None.
fn runaway(args: &str) -> Option<String> {
    if args.len() > MAX_ARGS_BYTES {
        return Some(format!(
            "tool arguments exceeded {} KB",
            MAX_ARGS_BYTES / 1024
        ));
    }
    if args.len() < TAIL * 2 {
        return None;
    }
    let tail = &args.as_bytes()[args.len() - TAIL..];
    // Exact short period: name it, the message is more useful.
    for unit in 1..=MAX_UNIT {
        let pat = &tail[..unit];
        if tail.chunks(unit).all(|c| c == &pat[..c.len()]) {
            let shown = String::from_utf8_lossy(pat).escape_debug().to_string();
            return Some(format!(
                "model output degenerated: the last {TAIL} bytes of the arguments repeat {shown:?}. \
                 Send a smaller change: replace fewer lines, or split it into several edits"
            ));
        }
    }
    // Longer or drifting periods (a repeated comment line, escalating
    // backslashes, tab soup): almost every window has been seen before.
    let distinct: std::collections::HashSet<&[u8]> = tail.windows(WINDOW).collect();
    let ratio = distinct.len() as f64 / (TAIL - WINDOW + 1) as f64;
    if ratio < MIN_DISTINCT_RATIO {
        return Some(format!(
            "model output degenerated: the last {TAIL} bytes of the arguments are repetitive \
             ({} distinct {WINDOW}-byte windows). Send a smaller change: replace fewer lines, or split it into several edits",
            distinct.len()
        ));
    }
    None
}

fn tail_preview(args: &str) -> String {
    let start = args
        .char_indices()
        .rev()
        .nth(120)
        .map(|(i, _)| i)
        .unwrap_or(0);
    args[start..].escape_debug().to_string()
}

/// The `reasoning_effort` to send, or None to omit the key. One decision:
///
/// - Models that cannot turn reasoning off (Claude Fable on OpenRouter)
///   always get a real effort; `low` when none is configured.
/// - OpenAI's own host rejects function tools unless the effort is
///   `"none"` on the models that accept `"none"` at all (Astra / Terra,
///   gpt-5.1 and later). Earlier models 400 on the key, so they never
///   get it. OpenRouter never gets the override: its reasoning models
///   400 on `"none"`.
/// - Otherwise the configured effort, if it is a real one.
fn reasoning_effort<'a>(
    model: &str,
    base: &str,
    tools: bool,
    configured: Option<&'a str>,
) -> Option<&'a str> {
    let real = configured.filter(|e| !e.is_empty() && *e != "none");
    if model.to_ascii_lowercase().contains("fable") {
        return Some(real.unwrap_or("low"));
    }
    if tools && base.contains("openai.com") && accepts_none(model) {
        return Some("none");
    }
    real
}

/// OpenAI models that take `reasoning_effort: "none"`: the named 2026
/// models and gpt-5.1 or later. gpt-4.x, gpt-4o, o-series and gpt-5.0 do not.
fn accepts_none(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    if m.contains("astra") || m.contains("terra") {
        return true;
    }
    let Some(rest) = m.strip_prefix("gpt-") else {
        return false;
    };
    let mut parts = rest.split(|c: char| !c.is_ascii_digit());
    let major: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    major > 5 || (major == 5 && minor >= 1)
}

#[derive(Default)]
struct PartialCall {
    id: String,
    name: String,
    arguments: String,
    emitted: bool,
}

impl PartialCall {
    fn to_call(&self) -> ToolCall {
        ToolCall {
            id: if self.id.is_empty() {
                "call".into()
            } else {
                self.id.clone()
            },
            name: self.name.clone(),
            arguments: parse_arguments(&self.arguments),
        }
    }

    /// A JSON object has no valid strict prefix, so a parse that succeeds
    /// means the arguments are whole.
    fn args_complete(&self) -> bool {
        let a = self.arguments.trim_end();
        a.ends_with('}') && serde_json::from_str::<Value>(a).is_ok()
    }
}

/// Key under which unparseable arguments travel so the tool layer can tell
/// the model what it actually sent instead of a bare "missing path".
pub const BAD_ARGS: &str = "__arbos_bad_arguments";

/// Streamed arguments as JSON. Empty means `{}` (a no-argument tool). Bad
/// JSON is kept verbatim under [`BAD_ARGS`] rather than silently becoming
/// `{}`: the model sees its own broken output in the tool result and can
/// resend, and the trace shows what came over the wire.
fn parse_arguments(raw: &str) -> Value {
    let raw = raw.trim();
    if raw.is_empty() {
        return json!({});
    }
    match serde_json::from_str::<Value>(raw) {
        Ok(v @ Value::Object(_)) => v,
        Ok(other) => json!({ BAD_ARGS: format!("arguments were not a JSON object: {other}") }),
        Err(e) => {
            json!({ BAD_ARGS: format!("arguments were not valid JSON ({e}): {}", tail_preview(raw)) })
        }
    }
}

/// Fire `Delta::Call` for every call we now know is complete: an earlier
/// index once a later one has started, any index whose JSON parses, and
/// everything once the stream is over.
fn emit_complete_calls(
    calls: &mut [PartialCall],
    stream_over: bool,
    on_delta: &mut impl FnMut(Delta),
) {
    let n = calls.len();
    for i in 0..n {
        if calls[i].emitted || calls[i].name.is_empty() {
            continue;
        }
        let later_started = i + 1 < n && !calls[i + 1].name.is_empty();
        if stream_over || later_started || calls[i].args_complete() {
            calls[i].emitted = true;
            on_delta(Delta::Call(calls[i].to_call()));
        }
    }
}

fn take_sse_line(buf: &mut String) -> Option<String> {
    loop {
        let i = buf.find('\n')?;
        let mut line = buf[..i].to_string();
        buf.replace_range(..=i, "");
        if line.ends_with('\r') {
            line.pop();
        }
        if line.is_empty() {
            continue;
        }
        // Not a data line: an SSE comment (`: OPENROUTER PROCESSING`
        // keep-alives), `event:`, `id:`, `retry:`. Skip it and keep going —
        // returning here would leave every later line in the buffer unread
        // until the next chunk, or forever at end of stream.
        let Some(payload) = line
            .strip_prefix("data:")
            .or_else(|| line.strip_prefix("data"))
        else {
            continue;
        };
        let payload = payload.strip_prefix(' ').unwrap_or(payload).trim();
        if payload.is_empty() {
            continue;
        }
        return Some(payload.to_string());
    }
}

fn text_field(delta: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(s) = delta
            .get(*key)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            return Some(s.to_string());
        }
    }
    None
}

/// Returns true when anything changed.
fn merge_tool_deltas(calls: &mut Vec<PartialCall>, raw: Option<&Value>) -> bool {
    let Some(arr) = raw.and_then(|v| v.as_array()) else {
        return false;
    };
    for tc in arr {
        let i = tc.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        while calls.len() <= i {
            calls.push(PartialCall::default());
        }
        if let Some(id) = tc.get("id").and_then(|v| v.as_str()) {
            calls[i].id = id.to_string();
        }
        let fnv = tc.get("function").unwrap_or(&Value::Null);
        if let Some(n) = fnv.get("name").and_then(|v| v.as_str()) {
            calls[i].name.push_str(n);
        }
        if let Some(a) = fnv.get("arguments").and_then(|v| v.as_str()) {
            calls[i].arguments.push_str(a);
        }
    }
    !arr.is_empty()
}

fn finish_calls(calls: Vec<PartialCall>) -> Vec<ToolCall> {
    calls
        .iter()
        .filter(|c| !c.name.is_empty())
        .map(PartialCall::to_call)
        .collect()
}

fn usage_of(v: &Value) -> Option<(u64, u64)> {
    let u = v.get("usage")?;
    let prompt = u.get("prompt_tokens")?.as_u64()?;
    let total = u
        .get("total_tokens")
        .and_then(|t| t.as_u64())
        .unwrap_or(prompt);
    Some((prompt, total))
}

/// OpenRouter puts the call's price in `usage.cost` (dollars). Absent
/// elsewhere.
fn cost_of(v: &Value) -> Option<f64> {
    v.get("usage")?.get("cost")?.as_f64()
}

/// Anthropic models cache nothing unless the request says where. OpenAI
/// and most others cache the prefix automatically and reject or ignore the
/// marker, so it is only sent to Claude.
fn wants_cache_control(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    m.contains("claude") || m.starts_with("anthropic/")
}

/// Two breakpoints: after the system prompt (contract + tool list, the
/// same every step) and after the last message (everything so far — the
/// next step's prefix). Cache reads cost a tenth of fresh tokens and cut
/// time to first byte; without the markers every step re-reads the whole
/// conversation at full price.
fn mark_cache_breakpoints(msgs: &mut [Value]) {
    fn mark(m: &mut Value) {
        let Some(content) = m.get_mut("content") else {
            return;
        };
        match content {
            Value::String(s) => {
                let text = std::mem::take(s);
                *content = json!([{ "type": "text", "text": text, "cache_control": { "type": "ephemeral" } }]);
            }
            Value::Array(parts) => {
                if let Some(last) = parts
                    .iter_mut()
                    .rev()
                    .find(|p| p.get("type").and_then(Value::as_str) == Some("text"))
                {
                    last["cache_control"] = json!({ "type": "ephemeral" });
                } else if let Some(last) = parts.last_mut() {
                    last["cache_control"] = json!({ "type": "ephemeral" });
                }
            }
            _ => {}
        }
    }
    let n = msgs.len();
    if n == 0 {
        return;
    }
    if msgs[0].get("role").and_then(Value::as_str) == Some("system") {
        mark(&mut msgs[0]);
    }
    if n > 1 {
        // An assistant message with only tool_calls has no content to mark;
        // walk back to the newest message that has some.
        for i in (1..n).rev() {
            let has_content = msgs[i].get("content").is_some_and(|c| match c {
                Value::String(s) => !s.is_empty(),
                Value::Array(a) => !a.is_empty(),
                _ => false,
            });
            if has_content {
                mark(&mut msgs[i]);
                break;
            }
        }
    }
}

/// The exact `messages` array sent to the provider.
pub fn messages_json(messages: &[ChatMessage]) -> Vec<Value> {
    messages
        .iter()
        .map(|m| {
            let mut v = json!({"role": m.role});
            if m.images.is_empty() {
                if let Some(c) = &m.content {
                    v["content"] = json!(c);
                }
            } else {
                // Multimodal content is an array of typed parts. Text first
                // so the model reads the caption before the pixels.
                let mut parts = Vec::new();
                if let Some(c) = m.content.as_deref().filter(|c| !c.is_empty()) {
                    parts.push(json!({"type": "text", "text": c}));
                }
                for img in &m.images {
                    parts.push(json!({
                        "type": "image_url",
                        "image_url": {"url": format!("data:{};base64,{}", img.mime, img.b64)}
                    }));
                }
                v["content"] = json!(parts);
            }
            if let Some(id) = &m.tool_call_id {
                v["tool_call_id"] = json!(id);
            }
            if let Some(rd) = &m.reasoning_details {
                v["reasoning_details"] = rd.clone();
            }
            if let Some(name) = &m.name {
                v["name"] = json!(name);
            }
            if let Some(calls) = &m.tool_calls {
                v["tool_calls"] = json!(calls
                    .iter()
                    .map(|c| json!({
                        "id": c.id,
                        "type": "function",
                        "function": {
                            "name": c.name,
                            "arguments": serde_json::to_string(&c.arguments).unwrap_or_else(|_| "{}".into()),
                        }
                    }))
                    .collect::<Vec<_>>());
            }
            v
        })
        .collect()
}
