//! Model proxy (D2): OpenAI-compatible and Anthropic-compatible passthrough.
//! The box authenticates with its box token; the relay adds the real provider
//! key, streams bytes through untouched, and meters tokens + USD per project.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures::StreamExt;
use serde_json::{json, Value};

use crate::auth::Principal;
use crate::state::AppState;

#[derive(Debug, Clone, Copy)]
enum Api {
    OpenAi,
    Anthropic,
    /// OpenRouter's Decisions API (`POST /api/alpha/decisions`): typed yes/no, choice and score
    /// answers from a decision model such as TypeSafe Jev. Never streams; billed on input tokens.
    Decisions,
}

/// OpenRouter's API root (`https://openrouter.ai/api`) for endpoints outside `/v1`, derived from
/// the configured OpenAI-compatible base so a self-hoster's custom OpenRouter base still works.
fn openrouter_root(openai_base: &str) -> String {
    let base = openai_base.trim_end_matches('/');
    if base.contains("openrouter") {
        base.strip_suffix("/v1").unwrap_or(base).to_owned()
    } else {
        "https://openrouter.ai/api".to_owned()
    }
}

#[derive(Default, Debug)]
struct Meter {
    model: String,
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    cost: Option<f64>,
}

pub(crate) fn start_of_utc_day_ms() -> i64 {
    let now = arbos_core::ids::now_ms();
    now - now.rem_euclid(86_400_000)
}

/// USD per million tokens: input, output, cache read, cache write.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rate {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
}

impl Rate {
    const fn anthropic(input: f64, output: f64) -> Self {
        Self {
            input,
            output,
            cache_read: input * 0.1,
            cache_write: input * 1.25,
        }
    }

    fn times(self, k: f64) -> Self {
        Self {
            input: self.input * k,
            output: self.output * k,
            cache_read: self.cache_read * k,
            cache_write: self.cache_write * k,
        }
    }
}

/// List price when the upstream does not report cost (the Anthropic messages path).
/// `prompt_tokens` = input + cache read + cache write, for the long-context tiers.
/// Rates follow OpenRouter's listing (pi's model registry, 2026-10-21). The old family guess
/// put `claude-fable-5.1` at $1 / $5 — a tenth of its $10 / $50 — so a 242k-token turn was
/// metered at $0.30 instead of $3 and the spend caps saw a tenth of the money
/// (feedback relay-model-pricing-2026-10-21).
fn price(model: &str, prompt_tokens: i64) -> Rate {
    let m = model.to_ascii_lowercase();
    if m.contains("fable-5.1") {
        return Rate {
            input: 10.0,
            output: 50.0,
            cache_read: 0.25,
            cache_write: 12.5,
        };
    }
    if m.contains("fable") {
        return Rate {
            input: 10.0,
            output: 50.0,
            cache_read: 1.0,
            cache_write: 12.5,
        };
    }
    if m.contains("opus") {
        return Rate::anthropic(4.0, 20.0);
    }
    if m.contains("sonnet") {
        return Rate::anthropic(2.0, 10.0);
    }
    if m.contains("haiku-5.5") {
        // Haiku 5.5 bills 5× past 100k prompt tokens.
        let base = Rate::anthropic(0.1, 0.5);
        return if prompt_tokens > 100_000 {
            base.times(5.0)
        } else {
            base
        };
    }
    if m.contains("haiku") {
        return Rate::anthropic(0.1, 0.5);
    }
    if m.contains("mini") || m.contains("nano") || m.contains("flash") {
        return Rate::anthropic(0.2, 1.25);
    }
    Rate::anthropic(1.0, 5.0)
}

/// Leaf id of an OpenRouter-style `vendor/name` (or the bare id).
fn model_leaf(model: &str) -> &str {
    model.rsplit('/').next().unwrap_or(model)
}

/// An OpenAI model id as the apps send it: `openai/<leaf>` (OpenRouter style) or a bare
/// `gpt-*` / `o*` / `chatgpt-*` leaf.
fn is_openai_model(model: &str) -> bool {
    let m = model.trim();
    if let Some(leaf) = m.strip_prefix("openai/") {
        return !leaf.is_empty();
    }
    if m.contains('/') {
        return false;
    }
    let l = m.to_ascii_lowercase();
    l.starts_with("gpt-")
        || l.starts_with("chatgpt-")
        || l.starts_with("o1")
        || l.starts_with("o3")
        || l.starts_with("o4")
}

/// Send this OpenAI model straight to OpenAI (first-party key) instead of through OpenRouter?
/// Yes when the relay has an `OPENAI_API_KEY` and the route is `direct` or `auto`; the model id
/// then loses its `openai/` prefix. OpenRouter's OpenAI route refuses our account.
fn openai_goes_direct(cfg: &crate::state::Config, model: &str) -> bool {
    use crate::state::OpenAiRoute;
    if !is_openai_model(model) || cfg.openai_key.is_none() {
        return false;
    }
    match cfg.openai_route {
        OpenAiRoute::Direct | OpenAiRoute::Auto => true,
        OpenAiRoute::OpenRouter => false,
    }
}

/// GPT-5 / GPT-6 (and o-series) OpenAI reasoning models: OpenRouter lists no `temperature`,
/// and OpenAI itself rejects `max_tokens` in favour of `max_completion_tokens`.
fn is_openai_reasoning_model(model: &str) -> bool {
    let leaf = model_leaf(model).to_ascii_lowercase();
    leaf.starts_with("gpt-5")
        || leaf.starts_with("gpt-6")
        || leaf.starts_with("o1")
        || leaf.starts_with("o3")
        || leaf.starts_with("o4")
}

/// GPT-6 Astra and GPT-6.1 Sol (and GPT-5) refuse `reasoning_effort: none|minimal`.
fn openai_requires_min_effort(model: &str) -> bool {
    let leaf = model_leaf(model).to_ascii_lowercase();
    leaf.contains("astra") || leaf.contains("6.1-sol") || leaf.starts_with("gpt-5")
}

fn bump_effort_if_needed(effort: &str) -> Option<&'static str> {
    match effort.to_ascii_lowercase().as_str() {
        "none" | "off" | "minimal" => Some("low"),
        _ => None,
    }
}

/// Rewrite a chat/completions or responses body so GPT-5/GPT-6 (incl. Astra) are accepted
/// upstream: drop sampling params, prefer `max_completion_tokens`, and bump a forbidden
/// `none`/`minimal` reasoning effort to `low`.
fn normalize_openai_reasoning_body(model: &str, body: &mut Value) {
    if !is_openai_reasoning_model(model) {
        return;
    }
    if let Some(obj) = body.as_object_mut() {
        for k in ["temperature", "top_p", "logprobs", "top_logprobs"] {
            obj.remove(k);
        }
        if let Some(max) = obj.remove("max_tokens") {
            obj.entry("max_completion_tokens").or_insert(max);
        }
        if openai_requires_min_effort(model) {
            if let Some(effort) = obj.get("reasoning_effort").and_then(Value::as_str) {
                if let Some(bumped) = bump_effort_if_needed(effort) {
                    obj.insert("reasoning_effort".into(), json!(bumped));
                }
            }
            if let Some(reasoning) = obj.get_mut("reasoning") {
                if let Some(r) = reasoning.as_object_mut() {
                    if let Some(effort) = r.get("effort").and_then(Value::as_str) {
                        if let Some(bumped) = bump_effort_if_needed(effort) {
                            r.insert("effort".into(), json!(bumped));
                        }
                    }
                }
            }
        }
    }
}

impl Meter {
    fn usd(&self) -> f64 {
        if let Some(c) = self.cost {
            return c;
        }
        let r = price(&self.model, self.input + self.cache_read + self.cache_write);
        (self.input as f64 * r.input
            + self.output as f64 * r.output
            + self.cache_read as f64 * r.cache_read
            + self.cache_write as f64 * r.cache_write)
            / 1_000_000.0
    }

    fn absorb_openai_usage(&mut self, u: &Value) {
        if let Some(n) = u.get("prompt_tokens").and_then(Value::as_i64) {
            self.input = n;
        }
        if let Some(n) = u.get("completion_tokens").and_then(Value::as_i64) {
            self.output = n;
        }
        if let Some(n) = u
            .pointer("/prompt_tokens_details/cached_tokens")
            .and_then(Value::as_i64)
        {
            self.cache_read = n;
            self.input -= n.min(self.input);
        }
        if let Some(n) = u
            .pointer("/prompt_tokens_details/cache_write_tokens")
            .and_then(Value::as_i64)
        {
            self.cache_write = n;
        }
        if let Some(c) = u.get("cost").and_then(Value::as_f64) {
            self.cost = Some(c);
        }
    }

    /// Decisions API usage: `{ input_tokens, output_tokens, cost }` (cost in USD, output free).
    fn absorb_decisions_usage(&mut self, u: &Value) {
        if let Some(n) = u.get("input_tokens").and_then(Value::as_i64) {
            self.input = n;
        }
        if let Some(n) = u.get("output_tokens").and_then(Value::as_i64) {
            self.output = n;
        }
        if let Some(c) = u.get("cost").and_then(Value::as_f64) {
            self.cost = Some(c);
        }
    }

    fn absorb_anthropic_usage(&mut self, u: &Value) {
        if let Some(n) = u.get("input_tokens").and_then(Value::as_i64) {
            self.input = n;
        }
        if let Some(n) = u.get("output_tokens").and_then(Value::as_i64) {
            self.output = n;
        }
        if let Some(n) = u.get("cache_read_input_tokens").and_then(Value::as_i64) {
            self.cache_read = n;
        }
        if let Some(n) = u.get("cache_creation_input_tokens").and_then(Value::as_i64) {
            self.cache_write = n;
        }
    }

    /// Scan one SSE event or JSON body for usage.
    fn scan(&mut self, api: Api, text: &str) {
        for line in text.lines() {
            let payload = line
                .strip_prefix("data:")
                .map(str::trim)
                .unwrap_or(line.trim());
            if payload.is_empty() || payload == "[DONE]" || !payload.starts_with('{') {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(payload) else {
                continue;
            };
            if let Some(m) = v.get("model").and_then(Value::as_str) {
                self.model = m.to_owned();
            }
            match api {
                Api::OpenAi => {
                    if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
                        self.absorb_openai_usage(u);
                    }
                }
                Api::Anthropic => {
                    if let Some(u) = v.pointer("/message/usage") {
                        self.absorb_anthropic_usage(u);
                        if let Some(m) = v.pointer("/message/model").and_then(Value::as_str) {
                            self.model = m.to_owned();
                        }
                    }
                    if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
                        self.absorb_anthropic_usage(u);
                    }
                }
                Api::Decisions => {
                    if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
                        self.absorb_decisions_usage(u);
                    }
                }
            }
        }
    }
}

/// Which project a model call is metered on. A box serves every project of its user and a
/// device drives the projects it owns or was invited into, so the caller names one in
/// `X-Arbos-Project`. The name only counts when `state.allows` says the caller may touch that
/// project: metering is spend, and a project's usage drives its own daily cap, so an unchecked
/// name would let a stranger bill their calls to someone else's agent (#1952). A box without a
/// usable name falls back to its user's most recently active project.
pub(crate) fn project_of(state: &AppState, p: &Principal, headers: &HeaderMap) -> Option<String> {
    if let Some(h) = headers
        .get("x-arbos-project")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
    {
        if let Ok(Some(proj)) = state.db.project(h) {
            if state.allows(p, &proj.id) {
                return Some(proj.id);
            }
        }
    }
    let Principal::Box { user_id } = p else {
        return None;
    };
    state
        .db
        .projects_of_user(user_id)
        .ok()
        .and_then(|v| v.into_iter().next())
        .map(|p| p.id)
}

const PASS_HEADERS: &[&str] = &[
    "anthropic-version",
    "anthropic-beta",
    "accept",
    "content-type",
    "x-title",
    "http-referer",
];

async fn forward(
    state: AppState,
    principal: Principal,
    headers: HeaderMap,
    api: Api,
    path_tail: &str,
    mut body: Bytes,
) -> Response {
    let project_id =
        project_of(&state, &principal, &headers).unwrap_or_else(|| "relay-admin".to_owned());
    if let Some(r) = crate::credits::project_cap_refusal(&state, &project_id) {
        return r;
    }
    if let Err(r) = crate::credits::check_call(&state, &principal, &project_id) {
        return crate::credits::refused_response(&r);
    }

    let mut req_json: Option<Value> = serde_json::from_slice(&body).ok();
    let mut model = req_json
        .as_ref()
        .and_then(|v| v.get("model"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let is_stream = req_json
        .as_ref()
        .and_then(|v| v.get("stream"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let (url, key, key_header): (String, String, &str) = match api {
        Api::OpenAi => {
            let direct = openai_goes_direct(&state.cfg, &model);
            let base = if direct {
                state
                    .cfg
                    .openai_direct_base
                    .trim_end_matches('/')
                    .to_owned()
            } else {
                state.cfg.openai_base.trim_end_matches('/').to_owned()
            };
            if direct {
                // OpenAI wants the bare id; OpenRouter wanted `openai/<id>`.
                let leaf = model_leaf(&model).to_owned();
                if leaf != model {
                    model = leaf;
                    if let Some(v) = req_json.as_mut() {
                        v["model"] = json!(model);
                    }
                }
            }
            let key = if direct {
                state.cfg.openai_key.clone()
            } else if base.contains("openrouter") {
                state.cfg.openrouter_key.clone()
            } else {
                state
                    .cfg
                    .openai_key
                    .clone()
                    .or(state.cfg.openrouter_key.clone())
            };
            let Some(key) = key else {
                return (
                    StatusCode::BAD_GATEWAY,
                    "relay has no OpenAI-compatible provider key",
                )
                    .into_response();
            };
            if let Some(v) = req_json.as_mut() {
                normalize_openai_reasoning_body(&model, v);
                if is_stream {
                    v["stream_options"] = json!({ "include_usage": true });
                    if base.contains("openrouter") {
                        v["usage"] = json!({ "include": true });
                    }
                } else if base.contains("openrouter") {
                    v["usage"] = json!({ "include": true });
                }
                body = Bytes::from(v.to_string());
            }
            (format!("{base}/{path_tail}"), key, "authorization")
        }
        Api::Anthropic => {
            let base = state.cfg.anthropic_base.trim_end_matches('/').to_owned();
            if base.contains("openrouter") {
                let Some(key) = state.cfg.openrouter_key.clone() else {
                    return (StatusCode::BAD_GATEWAY, "relay has no OpenRouter key")
                        .into_response();
                };
                if !model.contains('/') && !model.is_empty() {
                    model = format!("anthropic/{model}");
                    if let Some(v) = req_json.as_mut() {
                        v["model"] = json!(model);
                        body = Bytes::from(v.to_string());
                    }
                }
                (format!("{base}/{path_tail}"), key, "authorization")
            } else {
                let Some(key) = state.cfg.anthropic_key.clone() else {
                    return (StatusCode::BAD_GATEWAY, "relay has no Anthropic key").into_response();
                };
                (format!("{base}/{path_tail}"), key, "x-api-key")
            }
        }
        Api::Decisions => {
            let Some(key) = state.cfg.openrouter_key.clone() else {
                return (
                    StatusCode::BAD_GATEWAY,
                    "relay has no OpenRouter key (decision models are served by OpenRouter)",
                )
                    .into_response();
            };
            let root = openrouter_root(&state.cfg.openai_base);
            (format!("{root}/alpha/decisions"), key, "authorization")
        }
    };

    let mut req = state.http.post(&url).body(body);
    for h in PASS_HEADERS {
        if let Some(v) = headers.get(*h) {
            req = req.header(*h, v);
        }
    }
    req = if key_header == "authorization" {
        req.bearer_auth(&key)
    } else {
        req.header("x-api-key", key)
    };
    if matches!(api, Api::Anthropic) && headers.get("anthropic-version").is_none() {
        req = req.header("anthropic-version", "2023-06-01");
    }
    if headers.get("content-type").is_none() {
        req = req.header("content-type", "application/json");
    }

    let t0 = Instant::now();
    let upstream = match req.send().await {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("upstream: {e}")).into_response(),
    };
    let status = upstream.status();
    let mut resp_headers = HeaderMap::new();
    for (k, v) in upstream.headers() {
        let name = k.as_str();
        if name == "content-type"
            || name == "cache-control"
            || name.starts_with("x-ratelimit")
            || name == "openai-processing-ms"
        {
            resp_headers.insert(HeaderName::from_bytes(name.as_bytes()).unwrap(), v.clone());
        }
    }
    resp_headers.insert("x-arbos-proxied", HeaderValue::from_static("1"));

    // `x-arbos-purpose: <feature>` (agentd's memory tree sends `memory-tree`): the usage row's
    // `api` becomes `<api>:<feature>` and the ledger reference `<model>|<feature>`, so a feature's
    // spend sums exactly; untagged calls keep the old shapes.
    let purpose: Option<String> = headers
        .get("x-arbos-purpose")
        .and_then(|v| v.to_str().ok())
        .map(|p| {
            p.chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .take(32)
                .collect::<String>()
                .to_ascii_lowercase()
        })
        .filter(|p| !p.is_empty());
    let base_api = match api {
        Api::OpenAi => "openai",
        Api::Anthropic => "anthropic",
        Api::Decisions => "decisions",
    };
    let api_name: String = match &purpose {
        Some(p) => format!("{base_api}:{p}"),
        None => base_api.to_owned(),
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(64);
    let db = state.db.clone();
    let pid = project_id.clone();
    let meter_state = state.clone();
    let payer = principal.clone();
    tokio::spawn(async move {
        let mut meter = Meter {
            model: model.clone(),
            ..Default::default()
        };
        let mut ttfb: Option<i64> = None;
        let mut stream = upstream.bytes_stream();
        let mut carry = String::new();
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(b) => {
                    if ttfb.is_none() {
                        ttfb = Some(t0.elapsed().as_millis() as i64);
                    }
                    carry.push_str(&String::from_utf8_lossy(&b));
                    if let Some(idx) = carry.rfind('\n') {
                        let (done, rest) = carry.split_at(idx + 1);
                        meter.scan(api, done);
                        carry = rest.to_owned();
                    }
                    if tx.send(Ok(b)).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(std::io::Error::other(e))).await;
                    break;
                }
            }
        }
        if !carry.is_empty() {
            meter.scan(api, &carry);
        }
        if status.is_success() {
            let _ = db.record_usage(
                &pid,
                &api_name,
                &meter.model,
                meter.input,
                meter.output,
                meter.cache_read,
                meter.cache_write,
                meter.usd(),
                t0.elapsed().as_millis() as i64,
                ttfb,
            );
            let reference = match &purpose {
                Some(p) => format!("{}|{p}", meter.model),
                None => meter.model.clone(),
            };
            crate::credits::charge_call(
                &meter_state,
                &payer,
                &pid,
                meter.usd(),
                "inference",
                Some(&reference),
            );
        }
    });
    let body = Body::from_stream(tokio_stream_wrap(rx));
    (
        StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
        resp_headers,
        body,
    )
        .into_response()
}

fn tokio_stream_wrap(
    rx: tokio::sync::mpsc::Receiver<Result<Bytes, std::io::Error>>,
) -> impl futures::Stream<Item = Result<Bytes, std::io::Error>> {
    futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    })
}

/// `POST /proxy/v1/chat/completions`, `/proxy/v1/responses`, `/proxy/v1/embeddings`, … (OpenAI-compatible).
pub async fn openai(
    State(state): State<AppState>,
    principal: Principal,
    Path(tail): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    forward(state, principal, headers, Api::OpenAi, &tail, body).await
}

/// `POST /proxy/anthropic/v1/messages` (Anthropic-compatible; Claude Code's `ANTHROPIC_BASE_URL` lands here).
pub async fn anthropic(
    State(state): State<AppState>,
    principal: Principal,
    Path(tail): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    forward(state, principal, headers, Api::Anthropic, &tail, body).await
}

/// Decisions answered recently, by the sha-256 of the request body. The respond judge sends
/// the same `state` + `questions` from every box in a people room for one message; the first
/// answer is served to the rest, so the agents' verdicts agree (the model is not bit-for-bit
/// deterministic: ±0.05 between identical calls was measured on 2026-10-27) and the room pays
/// once. Bounded by size and age.
#[derive(Default)]
pub struct DecisionsCache {
    entries: HashMap<String, (Instant, Bytes)>,
    order: std::collections::VecDeque<String>,
}

/// How long one answer is reused.
const DECISIONS_CACHE_TTL: Duration = Duration::from_secs(15 * 60);
const DECISIONS_CACHE_MAX: usize = 4096;

impl DecisionsCache {
    pub fn get(&mut self, key: &str) -> Option<Bytes> {
        let (at, body) = self.entries.get(key)?;
        if at.elapsed() > DECISIONS_CACHE_TTL {
            self.entries.remove(key);
            return None;
        }
        Some(body.clone())
    }

    pub fn put(&mut self, key: String, body: Bytes) {
        if self
            .entries
            .insert(key.clone(), (Instant::now(), body))
            .is_none()
        {
            self.order.push_back(key);
        }
        while self.order.len() > DECISIONS_CACHE_MAX {
            if let Some(old) = self.order.pop_front() {
                self.entries.remove(&old);
            }
        }
    }
}

fn decisions_cache_key(body: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(body))
}

/// `POST /proxy/decisions` → OpenRouter `POST /api/alpha/decisions` (TypeSafe Jev and other
/// decision models). Same box-token auth, spend cap, credits and usage metering as chat calls;
/// `usage.cost` from the upstream is the charged amount. An identical request answered in the
/// last 15 minutes is served from the relay's cache (`x-arbos-decision-cache: hit`), unmetered.
pub async fn decisions(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let key = decisions_cache_key(&body);
    let served = |cached: Bytes| {
        let mut h = HeaderMap::new();
        h.insert("content-type", HeaderValue::from_static("application/json"));
        h.insert("x-arbos-proxied", HeaderValue::from_static("1"));
        h.insert("x-arbos-decision-cache", HeaderValue::from_static("hit"));
        (StatusCode::OK, h, Body::from(cached)).into_response()
    };
    let lookup = |state: &AppState| {
        state
            .decisions_cache
            .lock()
            .ok()
            .and_then(|mut c| c.get(&key))
    };
    if let Some(cached) = lookup(&state) {
        return served(cached);
    }
    // Single flight: identical requests arriving together wait for the first answer.
    let gate = state
        .decisions_inflight
        .lock()
        .await
        .entry(key.clone())
        .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    let _permit = gate.lock().await;
    if let Some(cached) = lookup(&state) {
        state.decisions_inflight.lock().await.remove(&key);
        return served(cached);
    }
    let resp = forward_decisions(&state, principal, headers, body, &key).await;
    state.decisions_inflight.lock().await.remove(&key);
    resp
}

async fn forward_decisions(
    state: &AppState,
    principal: Principal,
    headers: HeaderMap,
    body: Bytes,
    key: &str,
) -> Response {
    let resp = forward(
        state.clone(),
        principal,
        headers,
        Api::Decisions,
        "alpha/decisions",
        body,
    )
    .await;
    if resp.status() != StatusCode::OK {
        return resp;
    }
    // Small JSON: read it whole so it can be remembered and still returned.
    let (parts, body) = resp.into_parts();
    let bytes = match axum::body::to_bytes(body, 4 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("upstream body: {e}")).into_response(),
    };
    if serde_json::from_slice::<Value>(&bytes)
        .ok()
        .is_some_and(|v| v.get("answers").is_some())
    {
        if let Ok(mut c) = state.decisions_cache.lock() {
            c.put(key.to_owned(), bytes.clone());
        }
    }
    let mut h = parts.headers;
    h.insert("x-arbos-decision-cache", HeaderValue::from_static("miss"));
    (parts.status, h, Body::from(bytes)).into_response()
}

// ---------------------------------------------------------------------------
// Audio (F11): speech-to-text and text-to-speech passthrough with metering.
// Kept apart from `forward` because the request bodies are multipart / the
// responses are binary, and the upstream is api.openai.com even when models
// go through OpenRouter.
// ---------------------------------------------------------------------------

/// USD per minute of audio for the transcription models.
fn stt_price_per_minute(model: &str) -> f64 {
    let m = model.to_ascii_lowercase();
    if m.contains("mini") {
        0.003
    } else {
        0.006
    }
}

/// USD per million input characters for the speech models.
fn tts_price_per_million_chars(model: &str) -> f64 {
    let m = model.to_ascii_lowercase();
    if m.contains("hd") {
        30.0
    } else if m.contains("mini") {
        12.0
    } else {
        15.0
    }
}

/// Duration of a RIFF/WAVE blob (anywhere in `bytes`, so a multipart body works) in seconds.
fn wav_duration_secs(bytes: &[u8]) -> Option<f64> {
    let riff = bytes.windows(4).position(|w| w == b"RIFF")?;
    let mut pos = riff + 12;
    let mut byte_rate: Option<u32> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().ok()?) as usize;
        if id == b"fmt " && pos + 16 <= bytes.len() {
            byte_rate = Some(u32::from_le_bytes(
                bytes[pos + 16..pos + 20].try_into().ok()?,
            ));
        }
        if id == b"data" {
            let rate = byte_rate?;
            if rate == 0 {
                return None;
            }
            let data_len = size.min(bytes.len().saturating_sub(pos + 8));
            return Some(data_len as f64 / rate as f64);
        }
        pos += 8 + size + (size & 1);
    }
    None
}

/// Value of a text field of a multipart body (`name="model"`), no multipart crate needed.
fn multipart_field(body: &[u8], name: &str) -> Option<String> {
    let needle = format!("name=\"{name}\"");
    let at = body
        .windows(needle.len())
        .position(|w| w == needle.as_bytes())?;
    let rest = &body[at + needle.len()..];
    let blank = rest.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
    let value = &rest[blank..];
    let end = value
        .windows(2)
        .position(|w| w == b"\r\n")
        .unwrap_or(value.len());
    Some(String::from_utf8_lossy(&value[..end]).trim().to_owned())
}

/// 16 kHz mono 16-bit PCM WAV of a sine tone.
pub fn sine_wav(seconds: f64, hz: f64) -> Vec<u8> {
    const RATE: u32 = 16_000;
    let n = (seconds * RATE as f64) as usize;
    let data_len = (n * 2) as u32;
    let mut out = Vec::with_capacity(44 + n * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..n {
        let t = i as f64 / RATE as f64;
        // Short fade in/out so the mock does not click.
        let env = (t * 20.0).min(1.0).min((seconds - t) * 20.0).max(0.0);
        let s = (2.0 * std::f64::consts::PI * hz * t).sin() * 0.4 * env;
        out.extend_from_slice(&((s * i16::MAX as f64) as i16).to_le_bytes());
    }
    out
}

fn audio_project(state: &AppState, principal: &Principal, headers: &HeaderMap) -> String {
    project_of(state, principal, headers).unwrap_or_else(|| "relay-admin".to_owned())
}

/// `Some(response)` when the project's daily cap is already spent or the user may not spend.
fn audio_spend_cap(state: &AppState, principal: &Principal, project_id: &str) -> Option<Response> {
    if let Err(r) = crate::credits::check_call(state, principal, project_id) {
        return Some(crate::credits::refused_response(&r));
    }
    crate::credits::project_cap_refusal(state, project_id)
}

fn audio_key(state: &AppState) -> Option<String> {
    state.cfg.openai_key.clone()
}

/// Mock transcription body (`ARBOS_VOICE_MOCK=1`). Keeps `duration` on the wire; clients read `text`.
pub(crate) fn transcription_mock_json(secs: f64) -> Value {
    let text = format!(
        "[mock transcript] {:.1} s of audio received by the relay.",
        secs
    );
    json!({ "text": text, "duration": secs })
}

/// `POST /proxy/v1/audio/transcriptions`: multipart (`file`, `model`, …) → `{ "text": … }`.
/// Metered by audio minutes; `ARBOS_VOICE_MOCK=1` answers without an upstream.
pub async fn audio_transcriptions(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let project_id = audio_project(&state, &principal, &headers);
    if let Some(r) = audio_spend_cap(&state, &principal, &project_id) {
        return r;
    }
    let model = multipart_field(&body, "model").unwrap_or_else(|| "whisper-1".to_owned());
    let secs = wav_duration_secs(&body).unwrap_or(0.0);
    let usd = secs / 60.0 * stt_price_per_minute(&model);
    let t0 = Instant::now();

    if state.cfg.voice_mock {
        let _ = state.db.record_usage(
            &project_id,
            "openai-audio",
            &format!("{model} (mock)"),
            0,
            0,
            0,
            0,
            0.0,
            0,
            None,
        );
        return (
            StatusCode::OK,
            [
                ("content-type", "application/json"),
                ("x-arbos-voice-mock", "1"),
            ],
            transcription_mock_json(secs).to_string(),
        )
            .into_response();
    }

    let Some(key) = audio_key(&state) else {
        return (
            StatusCode::BAD_GATEWAY,
            "relay has no OpenAI key (OPENAI_API_KEY); set ARBOS_VOICE_MOCK=1 for a mock",
        )
            .into_response();
    };
    let Some(content_type) = headers.get("content-type").cloned() else {
        return (StatusCode::BAD_REQUEST, "multipart/form-data body required").into_response();
    };
    let url = format!(
        "{}/audio/transcriptions",
        state.cfg.openai_audio_base.trim_end_matches('/')
    );
    let upstream = state
        .http
        .post(url)
        .bearer_auth(key)
        .header("content-type", content_type)
        .body(body)
        .send()
        .await;
    let upstream = match upstream {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("upstream: {e}")).into_response(),
    };
    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let bytes = upstream.bytes().await.unwrap_or_default();
    if status.is_success() {
        let _ = state.db.record_usage(
            &project_id,
            "openai-audio",
            &model,
            0,
            0,
            0,
            0,
            usd,
            t0.elapsed().as_millis() as i64,
            None,
        );
    }
    if status.is_success() {
        crate::credits::charge_call(
            &state,
            &principal,
            &project_id,
            usd,
            "inference",
            Some(&model),
        );
    }
    (
        status,
        [
            ("content-type", "application/json"),
            ("x-arbos-proxied", "1"),
        ],
        bytes,
    )
        .into_response()
}

/// `POST /proxy/v1/audio/speech`: `{ model, input, voice, response_format }` → audio bytes.
/// Metered by input characters; `ARBOS_VOICE_MOCK=1` returns a 1 s sine WAV. Refused with
/// 403 `feature_off` while `features.voice_mode` is off — the one place every client's TTS
/// spend goes through, so an app that still shows the switch cannot spend either.
pub async fn audio_speech(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !crate::features::voice_mode() {
        return speech_feature_off();
    }
    let project_id = audio_project(&state, &principal, &headers);
    if let Some(r) = audio_spend_cap(&state, &principal, &project_id) {
        return r;
    }
    let req: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("bad JSON: {e}")).into_response(),
    };
    let model = req
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("tts-1")
        .to_owned();
    let chars = req
        .get("input")
        .and_then(Value::as_str)
        .map(|s| s.chars().count())
        .unwrap_or(0);
    if chars == 0 {
        return (StatusCode::BAD_REQUEST, "`input` is required").into_response();
    }
    if chars > 4096 {
        return (
            StatusCode::BAD_REQUEST,
            "`input` is longer than 4096 characters",
        )
            .into_response();
    }
    let usd = chars as f64 / 1_000_000.0 * tts_price_per_million_chars(&model);
    let format = req
        .get("response_format")
        .and_then(Value::as_str)
        .unwrap_or("mp3")
        .to_owned();
    let t0 = Instant::now();

    if state.cfg.voice_mock {
        let _ = state.db.record_usage(
            &project_id,
            "openai-audio",
            &format!("{model} (mock)"),
            chars as i64,
            0,
            0,
            0,
            0.0,
            0,
            None,
        );
        return (
            StatusCode::OK,
            [("content-type", "audio/wav"), ("x-arbos-voice-mock", "1")],
            sine_wav(1.0, 440.0),
        )
            .into_response();
    }

    let Some(key) = audio_key(&state) else {
        return (
            StatusCode::BAD_GATEWAY,
            "relay has no OpenAI key (OPENAI_API_KEY); set ARBOS_VOICE_MOCK=1 for a mock",
        )
            .into_response();
    };
    let url = format!(
        "{}/audio/speech",
        state.cfg.openai_audio_base.trim_end_matches('/')
    );
    let upstream = state
        .http
        .post(url)
        .bearer_auth(key)
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await;
    let upstream = match upstream {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("upstream: {e}")).into_response(),
    };
    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = upstream
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .unwrap_or_else(|| match format.as_str() {
            "wav" => "audio/wav".to_owned(),
            "opus" => "audio/ogg".to_owned(),
            "aac" => "audio/aac".to_owned(),
            "flac" => "audio/flac".to_owned(),
            "pcm" => "audio/pcm".to_owned(),
            _ => "audio/mpeg".to_owned(),
        });
    let bytes = upstream.bytes().await.unwrap_or_default();
    // gpt-4o-mini-tts streams its WAV with placeholder RIFF/data sizes (0xFFFFFFFF); the
    // whole body is here now, so hand every client a header that strict decoders accept.
    let bytes = if status.is_success() {
        finish_wav_body(bytes.to_vec())
    } else {
        bytes.to_vec()
    };
    if status.is_success() {
        let _ = state.db.record_usage(
            &project_id,
            "openai-audio",
            &model,
            chars as i64,
            0,
            0,
            0,
            usd,
            t0.elapsed().as_millis() as i64,
            None,
        );
    }
    if status.is_success() {
        crate::credits::charge_call(
            &state,
            &principal,
            &project_id,
            usd,
            "inference",
            Some(&model),
        );
    }
    let mut resp_headers = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(&content_type) {
        resp_headers.insert("content-type", v);
    }
    resp_headers.insert("x-arbos-proxied", HeaderValue::from_static("1"));
    (status, resp_headers, bytes).into_response()
}

/// The answer to a TTS request while voice mode is switched off on the relay.
pub fn speech_feature_off() -> Response {
    (
        StatusCode::FORBIDDEN,
        [("content-type", "application/json")],
        json!({
            "error": "feature_off",
            "message": "Voice mode is off on this relay (features.voice_mode); spoken replies are not synthesised.",
        })
        .to_string(),
    )
        .into_response()
}

/// A buffered TTS body with real RIFF/`data` sizes: streamed WAV placeholders are rewritten,
/// every other format (mp3, opus, …) passes through unchanged.
pub fn finish_wav_body(mut bytes: Vec<u8>) -> Vec<u8> {
    arbos_core::wav::repair_sizes(&mut bytes);
    bytes
}

/// `GET /proxy/v1/models` passthrough (pi's model discovery).
pub async fn openai_models(State(state): State<AppState>, _principal: Principal) -> Response {
    let base = state.cfg.openai_base.trim_end_matches('/');
    let key = if base.contains("openrouter") {
        state.cfg.openrouter_key.clone()
    } else {
        state.cfg.openai_key.clone()
    };
    let Some(key) = key else {
        return (StatusCode::BAD_GATEWAY, "no key").into_response();
    };
    match state
        .http
        .get(format!("{base}/models"))
        .bearer_auth(key)
        .send()
        .await
    {
        Ok(r) => {
            let status =
                StatusCode::from_u16(r.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let b = r.bytes().await.unwrap_or_default();
            (status, [("content-type", "application/json")], b).into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `X-Arbos-Project` says which project a model call is metered on. It was only checked
    /// against the caller for a box token; a device's header went through unread, so a signed-in
    /// stranger could bill their calls to someone else's project — padding that project's usage,
    /// walking it into its own daily spend cap (which stops its agent), and firing the
    /// `spend_cap` alert in its owner's app.
    #[tokio::test]
    async fn a_device_cannot_meter_its_calls_on_another_users_project() {
        let state = crate::state::test_support::test_state(crate::state::AuthMode::Github);
        let (alice, _alice_dev) = crate::people::tests_support::person(&state, 21);
        let (bob, bob_dev) = crate::people::tests_support::person(&state, 22);
        let alices = crate::people::tests_support::project_of(&state, &alice, "Research");
        state
            .db
            .grant_credits(&bob, 100.0, "test", "relay")
            .unwrap();

        let mut headers = HeaderMap::new();
        headers.insert(
            "x-arbos-project",
            HeaderValue::from_str(&alices).expect("header"),
        );
        let day = start_of_utc_day_ms();

        // The relay is in voice mock, so the transcription meters without an upstream call.
        let r = audio_transcriptions(
            State(state.clone()),
            bob_dev.clone(),
            headers.clone(),
            Bytes::from_static(b"not really audio"),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(
            state.db.usage_since(&alices, day).unwrap().calls,
            0,
            "a stranger's call was metered on Alice's project"
        );

        // A capped-out project is not a stranger's business either: the refusal reads Alice's
        // spend and pushes a `spend_cap` alert to her app.
        state
            .db
            .record_usage(&alices, "openai", "gpt-5", 0, 0, 0, 0, 99.0, 0, None)
            .unwrap();
        assert!(
            crate::credits::project_cap_refusal(&state, &alices).is_some(),
            "Alice's project is over its cap"
        );
        let _ = state.db.take_pushes(&alices);
        let r = audio_transcriptions(
            State(state.clone()),
            bob_dev,
            headers,
            Bytes::from_static(b"not really audio"),
        )
        .await;
        assert_eq!(
            r.status(),
            StatusCode::OK,
            "a stranger's call was refused by Alice's spend cap"
        );
        assert!(
            state.db.take_pushes(&alices).unwrap().is_empty(),
            "a stranger made Alice's app show a spend-cap alert"
        );
    }

    #[test]
    fn openai_models_go_direct_when_a_key_exists() {
        use crate::state::test_support::test_state;
        use crate::state::{AuthMode, OpenAiRoute};
        let mut cfg = (*test_state(AuthMode::Github).cfg).clone();
        assert!(is_openai_model("openai/gpt-6-luna"));
        assert!(is_openai_model("gpt-5-mini"));
        assert!(is_openai_model("o3-mini"));
        assert!(!is_openai_model("anthropic/claude-haiku-5.5"));
        assert!(!is_openai_model("google/gemini-4-argon"));
        // No OpenAI key: everything stays on the configured (OpenRouter) base.
        cfg.openai_key = None;
        cfg.openai_route = OpenAiRoute::Auto;
        assert!(!openai_goes_direct(&cfg, "openai/gpt-6-luna"));
        // Key present, auto: OpenAI models go first-party; other vendors do not.
        cfg.openai_key = Some("sk-test".into());
        assert!(openai_goes_direct(&cfg, "openai/gpt-6-luna"));
        assert!(openai_goes_direct(&cfg, "gpt-5-mini"));
        assert!(!openai_goes_direct(&cfg, "anthropic/claude-haiku-5.5"));
        // Explicit OpenRouter keeps the old path even with a key.
        cfg.openai_route = OpenAiRoute::OpenRouter;
        assert!(!openai_goes_direct(&cfg, "openai/gpt-6-luna"));
        cfg.openai_route = OpenAiRoute::Direct;
        assert!(openai_goes_direct(&cfg, "openai/gpt-6-luna"));
        assert_eq!(model_leaf("openai/gpt-6-luna"), "gpt-6-luna");
    }

    /// The streamed header OpenAI returns for `gpt-4o-mini-tts` + `response_format: wav`
    /// (both sizes `0xFFFFFFFF`) leaves the proxy with real sizes; mp3 bytes pass untouched.
    #[test]
    fn speech_wav_body_gets_real_sizes() {
        let mut streamed = sine_wav(0.5, 440.0);
        streamed[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        streamed[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(arbos_core::wav::needs_repair(&streamed));
        let fixed = finish_wav_body(streamed.clone());
        assert_eq!(fixed.len(), streamed.len());
        assert!(!arbos_core::wav::needs_repair(&fixed));
        assert_eq!(fixed, sine_wav(0.5, 440.0));

        let mp3 = b"ID3\x04\x00\x00\x00\x00\x00\x00\xff\xfb\x90\x00".to_vec();
        assert_eq!(finish_wav_body(mp3.clone()), mp3);
    }

    /// The respond judge's request is the same from every box in a room: the first answer is
    /// served to the rest from the relay's cache, with no upstream call and no metering.
    #[tokio::test]
    async fn an_identical_decision_request_is_answered_from_the_cache() {
        use crate::state::test_support::test_state;
        use crate::state::AuthMode;
        let state = test_state(AuthMode::Github);
        let body = Bytes::from_static(
            br#"{"model":"typesafe/jev-1.14","state":{"message":{"text":"hi"}},"questions":{}}"#,
        );
        let answer = Bytes::from_static(br#"{"model":"typesafe/jev-1.14-test","answers":{"addressee":{"type":"choice","choice":"nobody"}}}"#);
        // A miss with no provider key is refused upstream-side, not invented.
        let miss = decisions(
            State(state.clone()),
            Principal::Admin,
            HeaderMap::new(),
            body.clone(),
        )
        .await;
        assert_eq!(miss.status(), StatusCode::BAD_GATEWAY);
        state
            .decisions_cache
            .lock()
            .unwrap()
            .put(decisions_cache_key(&body), answer.clone());
        let hit = decisions(
            State(state.clone()),
            Principal::Admin,
            HeaderMap::new(),
            body,
        )
        .await;
        assert_eq!(hit.status(), StatusCode::OK);
        assert_eq!(
            hit.headers()
                .get("x-arbos-decision-cache")
                .and_then(|v| v.to_str().ok()),
            Some("hit")
        );
        let got = axum::body::to_bytes(hit.into_body(), 1 << 20)
            .await
            .unwrap();
        assert_eq!(got, answer);
        // A different body is a different key.
        let other = decisions(
            State(state),
            Principal::Admin,
            HeaderMap::new(),
            Bytes::from_static(br#"{"model":"typesafe/jev-1.14","state":{"message":{"text":"bye"}},"questions":{}}"#),
        )
        .await;
        assert_eq!(other.status(), StatusCode::BAD_GATEWAY);
    }

    /// Three agents of one box judge the same message within milliseconds: the identical
    /// requests arrive together, before any answer is cached. Only the first goes upstream;
    /// the other two wait for it and get the same bytes (2026-10-28 06:30: without this, three
    /// upstream calls answered 0.85 / 0.82 / 0.77 and two agents each thought they had the best
    /// score).
    #[tokio::test]
    async fn concurrent_identical_decision_requests_make_one_upstream_call() {
        use crate::state::test_support::test_state;
        use crate::state::AuthMode;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc as StdArc;
        let calls = StdArc::new(AtomicUsize::new(0));
        let c2 = calls.clone();
        let app = axum::Router::new().route(
            "/openrouter/api/alpha/decisions",
            axum::routing::post(move || {
                let c2 = c2.clone();
                async move {
                    let n = c2.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    axum::Json(json!({ "model": "typesafe/jev-1.14-test", "answers": { "addressee": { "type": "choice", "choice": "nobody", "probabilities": { "nobody": 0.9 }, "confidence": 0.9 } }, "usage": { "input_tokens": 10, "output_tokens": 1, "cost": 0.00001, "call": n } }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut state = test_state(AuthMode::Github);
        let mut cfg = (*state.cfg).clone();
        cfg.openrouter_key = Some("sk-test".into());
        cfg.openai_base = format!("http://127.0.0.1:{port}/openrouter/api/v1");
        state.cfg = std::sync::Arc::new(cfg);
        let body = Bytes::from_static(
            br#"{"model":"typesafe/jev-1.14","state":{"message":{"text":"same"}},"questions":{}}"#,
        );
        let mut tasks = Vec::new();
        for _ in 0..3 {
            let st = state.clone();
            let b = body.clone();
            tasks.push(tokio::spawn(async move {
                let r = decisions(State(st), Principal::Admin, HeaderMap::new(), b).await;
                let status = r.status();
                let cache = r
                    .headers()
                    .get("x-arbos-decision-cache")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);
                let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
                (status, cache, bytes)
            }));
        }
        let mut bodies = Vec::new();
        let mut hits = 0;
        for t in tasks {
            let (status, cache, bytes) = t.await.unwrap();
            assert_eq!(status, StatusCode::OK);
            if cache.as_deref() == Some("hit") {
                hits += 1;
            }
            bodies.push(bytes);
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "one upstream call for three identical requests"
        );
        assert_eq!(hits, 2);
        assert!(
            bodies.iter().all(|b| *b == bodies[0]),
            "every caller got the same answer"
        );
    }

    #[test]
    fn the_decisions_cache_is_bounded() {
        let mut c = DecisionsCache::default();
        for i in 0..(DECISIONS_CACHE_MAX + 10) {
            c.put(format!("k{i}"), Bytes::from_static(b"{}"));
        }
        assert_eq!(c.entries.len(), DECISIONS_CACHE_MAX);
        assert!(c.get("k0").is_none(), "the oldest entries went first");
        assert!(c.get(&format!("k{}", DECISIONS_CACHE_MAX + 9)).is_some());
    }

    /// Voice mode off → TTS is refused before any upstream call or metering (no spend).
    #[test]
    fn speech_is_refused_while_voice_mode_is_off() {
        let r = speech_feature_off();
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            r.headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok()),
            Some("application/json")
        );
    }

    #[test]
    fn astra_is_openai_reasoning_and_needs_min_effort() {
        assert!(is_openai_reasoning_model("openai/gpt-6-astra"));
        assert!(openai_requires_min_effort("openai/gpt-6-astra"));
        assert!(openai_requires_min_effort("openai/gpt-6-astra-pro"));
        assert!(openai_requires_min_effort("openai/gpt-6.1-sol"));
        assert!(!openai_requires_min_effort("openai/gpt-6-luna"));
        assert!(!is_openai_reasoning_model("anthropic/claude-sonnet-5.5"));
    }

    #[test]
    fn normalize_strips_sampling_and_rewrites_max_tokens_for_astra() {
        let mut body = json!({
            "model": "openai/gpt-6-astra",
            "messages": [{"role": "user", "content": "hi"}],
            "max_tokens": 1024,
            "temperature": 0.2,
            "top_p": 0.9,
            "reasoning_effort": "none",
        });
        normalize_openai_reasoning_body("openai/gpt-6-astra", &mut body);
        let obj = body.as_object().unwrap();
        assert!(obj.get("temperature").is_none());
        assert!(obj.get("top_p").is_none());
        assert!(obj.get("max_tokens").is_none());
        assert_eq!(obj.get("max_completion_tokens"), Some(&json!(1024)));
        assert_eq!(obj.get("reasoning_effort"), Some(&json!("low")));
    }

    #[test]
    fn normalize_bumps_nested_reasoning_effort() {
        let mut body = json!({
            "model": "openai/gpt-6-astra",
            "reasoning": { "effort": "minimal" },
            "max_completion_tokens": 512,
        });
        normalize_openai_reasoning_body("openai/gpt-6-astra", &mut body);
        assert_eq!(body["reasoning"]["effort"], json!("low"));
        assert_eq!(body["max_completion_tokens"], json!(512));
    }

    #[test]
    fn normalize_leaves_anthropic_untouched() {
        let mut body = json!({
            "model": "anthropic/claude-sonnet-5.5",
            "max_tokens": 1024,
            "temperature": 0.2,
        });
        let before = body.clone();
        normalize_openai_reasoning_body("anthropic/claude-sonnet-5.5", &mut body);
        assert_eq!(body, before);
    }
}
