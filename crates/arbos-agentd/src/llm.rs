//! Small model calls from agentd's own Rust code (memory decisions, compaction, sub_llm),
//! always through the relay proxy with the box token (D2).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::config::Config;

pub struct Llm {
    http: reqwest::Client,
    base: String,
    /// `POST {relay}/proxy/decisions` → OpenRouter's Decisions API (decision models such as Jev).
    decisions_url: String,
    token: String,
    project_id: String,
    /// Sent as `x-arbos-purpose`; the relay tags the usage row and ledger entry with it so a
    /// feature's spend (e.g. `memory-tree`) can be summed exactly.
    purpose: Option<String>,
    /// Models whose endpoint refuses `reasoning {enabled: false}`, learned from the first
    /// rejection. Shared by every `with_purpose` clone, so one chat's 400 spares every later
    /// call on this box the same wasted round trip.
    reasoning_always_on: Arc<Mutex<HashSet<String>>>,
}

/// One answer of a decision model. `noul` questions answer with the yes-probability; `choice`
/// and `score` questions with the pick plus a probability per option and a confidence.
#[derive(Debug, Clone)]
pub struct DecisionAnswer {
    pub kind: String,
    /// `noul`: P(yes). `choice`/`score`: the confidence in the pick.
    pub probability: f64,
    pub raw: Value,
}

/// A whole Decisions-API response: answers keyed by the question ids the caller chose, the
/// exact model build that answered, usage, and the round-trip time.
#[derive(Debug, Clone)]
pub struct Decision {
    pub answers: std::collections::BTreeMap<String, DecisionAnswer>,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: Option<f64>,
    pub latency_ms: i64,
}

impl Llm {
    pub fn new(cfg: &Config) -> Self {
        Self {
            http: reqwest::Client::new(),
            base: format!("{}/proxy/v1", cfg.relay_url),
            decisions_url: format!("{}/proxy/decisions", cfg.relay_url),
            token: cfg.box_token.clone(),
            project_id: cfg.project_id.clone(),
            purpose: None,
            reasoning_always_on: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// The same client with every call tagged `x-arbos-purpose: <purpose>`.
    pub fn with_purpose(&self, purpose: &str) -> Self {
        Self {
            http: self.http.clone(),
            base: self.base.clone(),
            decisions_url: self.decisions_url.clone(),
            token: self.token.clone(),
            project_id: self.project_id.clone(),
            purpose: Some(purpose.to_owned()),
            reasoning_always_on: Arc::clone(&self.reasoning_always_on),
        }
    }

    fn purpose_header(&self, r: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.purpose {
            Some(p) => r.header("x-arbos-purpose", p),
            None => r,
        }
    }

    /// Ask a decision model typed questions about `state` (Decisions API: `noul` = yes/no
    /// probability, `choice`, `score`). All questions are answered in parallel in one request
    /// and cannot see each other. No text is generated; the answer is calibrated probabilities.
    pub async fn decide(
        &self,
        model: &str,
        state: &Value,
        questions: &Value,
        timeout: std::time::Duration,
    ) -> Result<Decision> {
        let t0 = Instant::now();
        let body = json!({ "model": model, "state": state, "questions": questions });
        let resp = self
            .purpose_header(
                self.http
                    .post(&self.decisions_url)
                    .bearer_auth(&self.token)
                    .header("x-arbos-project", &self.project_id),
            )
            .timeout(timeout)
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        let v: Value = resp.json().await?;
        if !status.is_success() {
            return Err(anyhow!("decision call failed: HTTP {status}: {}", v));
        }
        let mut answers = std::collections::BTreeMap::new();
        let Some(obj) = v.get("answers").and_then(Value::as_object) else {
            return Err(anyhow!("decision response without answers: {v}"));
        };
        for (id, a) in obj {
            let kind = a
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let probability = match kind.as_str() {
                "noul" => a.get("noul").and_then(Value::as_f64),
                "choice" | "score" => a.get("confidence").and_then(Value::as_f64),
                _ => None,
            }
            .ok_or_else(|| anyhow!("decision answer {id} has an unexpected shape: {a}"))?;
            answers.insert(
                id.clone(),
                DecisionAnswer {
                    kind,
                    probability,
                    raw: a.clone(),
                },
            );
        }
        let usage = v.get("usage").cloned().unwrap_or(Value::Null);
        Ok(Decision {
            answers,
            model: v
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or(model)
                .to_owned(),
            input_tokens: usage
                .get("input_tokens")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            output_tokens: usage
                .get("output_tokens")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            cost_usd: usage.get("cost").and_then(Value::as_f64),
            latency_ms: t0.elapsed().as_millis() as i64,
        })
    }

    /// A client for explicit endpoints (the `eval-responders` subcommand talks to the relay proxy
    /// or to OpenRouter directly): `chat_base` ends in `/v1`.
    pub fn with_endpoints(chat_base: &str, decisions_url: &str, token: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            base: chat_base.trim_end_matches('/').to_owned(),
            decisions_url: decisions_url.to_owned(),
            token: token.to_owned(),
            project_id: String::new(),
            purpose: None,
            reasoning_always_on: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// One non-streaming chat completion. Returns `(text, latency_ms)`.
    pub async fn complete(
        &self,
        model: &str,
        system: &str,
        user: &str,
        max_tokens: u32,
    ) -> Result<(String, i64)> {
        let (text, ms, _) = self
            .complete_with_usage(model, system, user, max_tokens)
            .await?;
        Ok((text, ms))
    }

    /// `complete` plus the upstream `usage` object (`prompt_tokens`, `completion_tokens`, and
    /// `cost` in USD when the relay proxy asked OpenRouter for it).
    pub async fn complete_with_usage(
        &self,
        model: &str,
        system: &str,
        user: &str,
        max_tokens: u32,
    ) -> Result<(String, i64, Value)> {
        let c = self
            .complete_full(model, system, user, max_tokens, None)
            .await?;
        Ok((c.text, c.latency_ms, c.usage))
    }

    /// `complete_with_usage` plus the choice's `finish_reason` and optional extra body fields
    /// (e.g. `{"reasoning": {"enabled": false}}` for a call that must spend its whole token
    /// budget on visible text).
    pub async fn complete_full(
        &self,
        model: &str,
        system: &str,
        user: &str,
        max_tokens: u32,
        extra: Option<&Value>,
    ) -> Result<Completion> {
        let t0 = Instant::now();
        let mut body = json!({
            "model": model,
            "messages": [{ "role": "system", "content": system }, { "role": "user", "content": user }],
            "max_tokens": max_tokens,
            "temperature": 0,
        });
        if let (Some(extra), Some(obj)) = (extra.and_then(Value::as_object), body.as_object_mut()) {
            for (k, v) in extra {
                obj.insert(k.clone(), v.clone());
            }
        }
        let resp = self
            .purpose_header(
                self.http
                    .post(format!("{}/chat/completions", self.base))
                    .bearer_auth(&self.token),
            )
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        let v: Value = resp.json().await?;
        if !status.is_success() {
            return Err(anyhow!("model call failed: HTTP {status}: {}", v));
        }
        let text = v
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let usage = v.get("usage").cloned().unwrap_or(Value::Null);
        let finish_reason = v
            .pointer("/choices/0/finish_reason")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let refusal = v
            .pointer("/choices/0/message/refusal")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned);
        Ok(Completion {
            text,
            latency_ms: t0.elapsed().as_millis() as i64,
            usage,
            finish_reason,
            refusal,
        })
    }

    /// A completion whose whole budget must go to text the user reads.
    ///
    /// Asks for reasoning off, because hidden thinking counts against `max_tokens` and used to
    /// cut the visible sentence at the cap (#1411). Some endpoints refuse: Opus 5.5 answers
    /// HTTP 400 "Reasoning is mandatory for this endpoint and cannot be disabled", and every
    /// quick reply on it failed and fell back to a full background job (Jacob, Test5,
    /// 2026-11-04). On that refusal the model is remembered and the call is retried at
    /// `thinking_max_tokens` — a budget with room for the thinking as well, so the answer still
    /// arrives whole on this one call. Later calls on the same model skip the refused attempt.
    pub async fn complete_visible(
        &self,
        model: &str,
        system: &str,
        user: &str,
        max_tokens: u32,
        thinking_max_tokens: u32,
    ) -> Result<Completion> {
        if self.reasoning_always_on(model) {
            return self
                .complete_full(model, system, user, thinking_max_tokens, None)
                .await;
        }
        let off = json!({ "reasoning": { "enabled": false } });
        let first = self
            .complete_full(model, system, user, max_tokens, Some(&off))
            .await;
        match first {
            Err(e) if is_reasoning_mandatory(&e.to_string()) => {
                tracing::info!(
                    model,
                    "endpoint requires reasoning; retrying with room for it and remembering the model"
                );
                if let Ok(mut s) = self.reasoning_always_on.lock() {
                    s.insert(model.to_owned());
                }
                self.complete_full(model, system, user, thinking_max_tokens, None)
                    .await
            }
            other => other,
        }
    }

    /// This model's endpoint already refused `reasoning {enabled: false}` on this box.
    pub fn reasoning_always_on(&self, model: &str) -> bool {
        self.reasoning_always_on
            .lock()
            .map(|s| s.contains(model))
            .unwrap_or(false)
    }

    /// A forced-JSON call: strips code fences and parses the first JSON object.
    pub async fn complete_json(
        &self,
        model: &str,
        system: &str,
        user: &str,
        max_tokens: u32,
    ) -> Result<(Value, i64)> {
        let (v, ms, _) = self
            .complete_json_with_usage(model, system, user, max_tokens)
            .await?;
        Ok((v, ms))
    }

    /// `complete_json` plus the upstream `usage` object.
    pub async fn complete_json_with_usage(
        &self,
        model: &str,
        system: &str,
        user: &str,
        max_tokens: u32,
    ) -> Result<(Value, i64, Value)> {
        let c = self
            .complete_full(
                model,
                &format!("{system}\nAnswer with one JSON object and nothing else."),
                user,
                max_tokens,
                None,
            )
            .await?;
        // A declined request is named as one, so a caller can tell it from a call that failed.
        if let Some(r) = c.refusal_reason() {
            return Err(anyhow!("the model provider refused the request: {r}"));
        }
        let (text, ms, usage) = (c.text, c.latency_ms, c.usage);
        let v = parse_json_loose(&text).ok_or_else(|| {
            anyhow!(
                "model did not return JSON: {}",
                text.chars().take(200).collect::<String>()
            )
        })?;
        Ok((v, ms, usage))
    }
}

/// One non-streaming completion, with what the caller needs to judge it.
#[derive(Debug, Clone)]
pub struct Completion {
    pub text: String,
    pub latency_ms: i64,
    pub usage: Value,
    /// `stop`, `length` (the `max_tokens` cap cut the answer), `content_filter`, … as the
    /// provider reports it; `None` when absent.
    pub finish_reason: Option<String>,
    /// The provider's own refusal text, when it answered with one instead of content
    /// (OpenAI-style `message.refusal`).
    pub refusal: Option<String>,
}

impl Completion {
    /// The provider declined the request's content rather than failing: it said so on the
    /// message, or it stopped on a moderation reason. Either way the text is not an answer, and
    /// sending the same request again gets the same refusal.
    ///
    /// Without this, a declined call reached the caller as "model did not return JSON" — a
    /// failure, by its wording, that no lane could tell from a transient one (Jacob, Test5
    /// round 2, the F variant of #2750).
    pub fn refusal_reason(&self) -> Option<String> {
        if let Some(r) = self.refusal.as_deref() {
            return Some(r.to_owned());
        }
        let reason = self.finish_reason.as_deref()?.to_ascii_lowercase();
        matches!(
            reason.as_str(),
            "content_filter" | "content-filter" | "safety" | "blocklist" | "prohibited_content"
        )
        .then(|| format!("the provider stopped on {reason}"))
    }

    /// The provider stopped because `max_tokens` was reached: the text is a fragment.
    pub fn truncated(&self) -> bool {
        matches!(
            self.finish_reason
                .as_deref()
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("length") | Some("max_tokens")
        )
    }
}

/// The endpoint rejected the call because reasoning may not be turned off on it (Opus 5.5:
/// HTTP 400 "Reasoning is mandatory for this endpoint and cannot be disabled"). Matched on the
/// wording rather than the status, since the same refusal arrives as 400 or 422 by provider.
pub fn is_reasoning_mandatory(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("reasoning") && (e.contains("mandatory") || e.contains("cannot be disabled"))
}

/// Find the first `{…}` or `[…]` in model output and parse it.
pub fn parse_json_loose(text: &str) -> Option<Value> {
    let t = text.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t);
    let t = t.strip_suffix("```").unwrap_or(t).trim();
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        return Some(v);
    }
    let start = t.find(['{', '['])?;
    let open = t.as_bytes()[start] as char;
    let close = if open == '{' { '}' } else { ']' };
    let end = t.rfind(close)?;
    serde_json::from_str(&t[start..=end]).ok()
}

/// Extract the return-contract JSON block from a subagent's final reply (design §6.7).
pub fn extract_return_contract(reply: &str) -> Option<Value> {
    let idx = reply.rfind("```json")?;
    let rest = &reply[idx + 7..];
    let end = rest.find("```")?;
    serde_json::from_str::<Value>(rest[..end].trim())
        .ok()
        .filter(|v| v.get("status").is_some())
}

/// The fields the "## Return contract" section of a worker's prompt asks for besides `status`.
const CONTRACT_FIELDS: [&str; 6] = [
    "summary",
    "artifacts",
    "acceptance",
    "open_items",
    "proposed_memory",
    "questions",
];

/// Is this JSON a worker's return contract, rather than JSON a worker wrote for the user?
///
/// Deliberately narrower than [`extract_return_contract`]'s filter, because this one decides
/// what to *remove* from a message a person reads: a `status` of exactly one of the four the
/// contract defines, **and** at least one of the contract's own fields. An API body a worker was
/// asked to show (`{"status": "ok", "data": …}`) matches neither half and stays in the message.
fn is_return_contract(v: &Value) -> bool {
    let Some(obj) = v.as_object() else {
        return false;
    };
    obj.get("status")
        .and_then(Value::as_str)
        .is_some_and(|s| matches!(s, "done" | "blocked" | "partial" | "needs_input"))
        && CONTRACT_FIELDS.iter().any(|f| obj.contains_key(*f))
}

/// `reply` with its trailing return contract removed — the human half of what a worker wrote.
///
/// Bug AC (Jacob, Test5 round 12): every path that posts a worker's final text into a chat goes
/// through here, because the contract is machinery and the user is not reading JSON.
///
/// Only a contract-shaped block at the **end** of the reply is removed, which is where the
/// prompt asks for it. A block with words after it is left alone: a worker explaining or quoting
/// the contract shape writes exactly that, and nothing in the text tells the two apart.
pub fn strip_return_contract(reply: &str) -> String {
    let Some(start) = reply.rfind("```json") else {
        return reply.to_owned();
    };
    let rest = &reply[start + 7..];
    let Some(end) = rest.find("```") else {
        return reply.to_owned();
    };
    if !rest[end + 3..].trim().is_empty() {
        return reply.to_owned();
    }
    match serde_json::from_str::<Value>(rest[..end].trim()) {
        Ok(v) if is_return_contract(&v) => reply[..start].trim_end().to_owned(),
        _ => reply.to_owned(),
    }
}
