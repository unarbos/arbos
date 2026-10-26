//! Who replies in a group chat: the responder decision shared by project groups (`groups.rs`,
//! design D69) and people group chats (`people.rs`, D72).
//!
//! Provider `jev` (default): one Decisions-API request with one yes/no (`noul`) question per
//! agent. The state names every agent with its role/description and carries the recent chat
//! plus the new message; each answer is a calibrated probability that this agent should reply,
//! and an agent replies when its probability is at or above `threshold`. No text is generated,
//! so the call takes ~0.2 s and costs input tokens only.
//!
//! Provider `llm`: the previous router — the `decision_model.id` chat model answers with a JSON
//! list of responders. It is also the first fail-open step when the decision model errors; the
//! last step is the caller's own default (every agent / the previous mention rules).
//!
//! Every decision is logged (`memory_decision_log`, side `respond`): provider, model build,
//! input hash, per-agent scores, chosen set, latency, tokens and cost.

use std::time::Duration;

use serde_json::{json, Map, Value};
use sha2::Digest;

use crate::agent::Agent;
use crate::llm::Llm;
use crate::settings::{DecisionConfig, DecisionProvider};

/// Decision-model round trip budget; p99 on OpenRouter is ~0.6 s.
const DECISION_TIMEOUT: Duration = Duration::from_millis(3000);
const MESSAGE_MAX_CHARS: usize = 4000;

/// System prompt of the `llm` provider (and the fail-open step of `jev`).
const LLM_SYSTEM: &str = r#"You are the reply router for a group chat with people and several agents.
Decide which agents (if any) should reply to the latest message. You may pick none, one, several, or all.

Pick an agent when the message asks for their help, names or mentions them, continues a thread they were just in, or fits their role better than the other agents'. Pick several when more than one should speak (for example "all agents…" or "both of you agents…").

Pick none when people are talking to each other — including short pings by first name or @handle ("Yo Jen", "Ken"), inviting humans to a call or meet-up ("Call?", "you free?"), or continuing that human thread after such a ping; when the message addresses only humans or people (for example "all humans…", "humans say…", "people only…") even if an agent just spoke; or when the message is only a remark that needs no agent reply. Use `people` (when present) to tell humans from agents. Staying quiet means an empty responders list — do not pick an agent so it can announce it will stay quiet.

When the message is only a cancel, hold or stop aimed at work one agent just announced (for example "no don't do it yet", "hold off", "stop"), pick only that agent — never every agent so they can each say "Understood".

Output: {"responders":["agent_id",...],"reason":"one line","confidence":0-1}
Use the agent ids from the list. An empty responders list means nobody replies."#;

/// One agent that could reply.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub id: String,
    pub name: String,
    /// The agent's role or description (empty when the project has none).
    pub role: String,
    /// Hosts the group (project groups) — shown to the model, not a rule.
    pub host: bool,
    /// How `message` @mentions this agent, when it does: `addressed` (vocative), `object`
    /// (the thing a request to someone else is about: "with @x", "about @x", "to @x"), or
    /// `referenced` (named mid-sentence). An @mention is a consideration signal, not an
    /// obligation; the decision reads this instead of treating every mention as a question.
    pub mentioned_as: Option<&'static str>,
}

/// The message and its room, as the decision sees it.
#[derive(Debug, Clone)]
pub struct Room {
    /// `project_group` (the user with several of their Projects) or `people_group`.
    pub kind: &'static str,
    pub chat_id: String,
    pub title: String,
    /// Who wrote the message, as a label (`the user`, `@sam (guest)`, `Project Research`).
    pub from: String,
    pub text: String,
    /// Earlier messages, oldest first: `[{ "from": label, "text": … }]`.
    pub recent: Vec<Value>,
    pub candidates: Vec<Candidate>,
    /// Human members as `@handle` (people groups). Empty for project groups. Lets the decision
    /// tell a first-name ping or call invite aimed at people from a request to an agent.
    pub people: Vec<String>,
}

/// What the decision concluded and how.
#[derive(Debug, Clone)]
pub struct ResponderDecision {
    pub chosen: Vec<String>,
    /// `P(reply)` per candidate id, in candidate order (`jev` only).
    pub scores: Vec<(String, f64)>,
    pub provider: &'static str,
    pub model: String,
    pub latency_ms: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: Option<f64>,
    pub input_hash: String,
    pub threshold: f64,
    pub reason: String,
    pub confidence: Option<f64>,
    /// Raw answers (`jev`) or the model's JSON (`llm`).
    pub raw: Value,
    /// Providers that failed before this one answered.
    pub errors: Vec<String>,
}

impl ResponderDecision {
    pub fn chose(&self, id: &str) -> bool {
        self.chosen.iter().any(|c| c == id)
    }
}

/// Whitespace-collapsed and capped, with a marker when cut (the decision model cannot fetch,
/// so the marker only says how much is missing).
fn clip(s: &str, max: usize) -> String {
    let s: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    crate::truncate::clip_marked(&s, max, "", None)
}

/// A short hash of the exact inputs, so a logged decision can be matched to its state.
fn input_hash(state: &Value) -> String {
    let bytes = serde_json::to_vec(state).unwrap_or_default();
    hex::encode(sha2::Sha256::digest(&bytes))[..16].to_owned()
}

/// Question id per candidate: its index keeps ids stable and free of odd characters.
fn question_id(i: usize) -> String {
    format!("reply_{i}")
}

/// The Decisions-API `state`: the same object for every agent (all questions see all agents).
pub fn jev_state(room: &Room) -> Value {
    let agents: Vec<Value> = room
        .candidates
        .iter()
        .map(|c| {
            let mut a = Map::new();
            a.insert("name".into(), json!(c.name));
            a.insert(
                "role".into(),
                json!(if c.role.trim().is_empty() {
                    "no description given".to_owned()
                } else {
                    clip(&c.role, 600)
                }),
            );
            if c.host {
                a.insert("hosts_this_group".into(), json!(true));
            }
            if let Some(m) = c.mentioned_as {
                a.insert("mentioned_as".into(), json!(m));
            }
            Value::Object(a)
        })
        .collect();
    let mut state = Map::new();
    state.insert(
        "chat".into(),
        json!({
            "kind": match room.kind {
                "people_group" => "group chat between people and the agents they added",
                _ => "group chat between the user and several of their project agents",
            },
            "title": room.title,
        }),
    );
    if !room.people.is_empty() {
        state.insert("people".into(), json!(room.people));
    }
    state.insert("agents".into(), json!(agents));
    state.insert("recent_messages".into(), json!(room.recent));
    state.insert(
        "message".into(),
        json!({ "from": room.from, "text": clip(&room.text, MESSAGE_MAX_CHARS) }),
    );
    Value::Object(state)
}

/// One `noul` question per candidate, phrased with the agent's name so the criteria are specific.
pub fn jev_questions(room: &Room) -> Value {
    let mut q = Map::new();
    // Only when `message` @mentions someone: the clause moves unmentioned cases otherwise.
    let any_mentioned = room.candidates.iter().any(|c| c.mentioned_as.is_some());
    for (i, c) in room.candidates.iter().enumerate() {
        let name = &c.name;
        let mention_clause = if any_mentioned {
            format!(" An @mention is a consideration signal, not an obligation: `mentioned_as` on an agent says how `message` names it — `addressed` means spoken to; `object` means only talked about as the thing a request to someone else is about (\"@other, can you talk with {name}?\", \"send it to {name}\"), which by itself is not a question for {name}.")
        } else {
            String::new()
        };
        q.insert(
            question_id(i),
            json!({
                "type": "noul",
                "instructions": format!(
                    "Should the agent \"{name}\" reply to `message`? Several agents may reply to one message; judge {name} on its own, from its name and role in `agents`, from `people` (the humans in the room, when listed), from `recent_messages`, and from who `message` is from and what it asks. Prefer false when `message` is for humans or people rather than agents — including short name pings and call invites between people — staying quiet means no reply at all, not a reply that says you will stay quiet. Prefer false for {name} when `message` is only a cancel/hold/stop of work another agent just announced — only that agent should reply, not a pile of \"Understood\".{mention_clause}"
                ),
                "criteria": {
                    "true": format!(
                        "`message` asks {name} for something, directly or as part of a request to several or all agents (for example \"all agents…\"); names or mentions {name}; continues a thread {name} was just speaking in, unless this `message` is now addressed only to humans or people; answers a question {name} asked; asks for work that belongs to {name}'s role; or is a cancel/hold/stop of work {name} just announced in `recent_messages` (for example \"no don't do it yet\" after {name} said they were filing or starting something)."
                    ),
                    "false": format!(
                        "`message` is people talking to each other (greeting or pinging someone by first name or @handle, inviting humans to a call or meet-up, or continuing that thread — for example \"Yo Jen\", \"Ken\", \"Call?\" after naming a person) even if short or informal; addresses only humans or people (for example \"all humans…\", \"humans say…\", \"people only…\") even if {name} just spoke; is addressed only to other agents; nothing in it belongs to {name}'s role; is only a remark, reaction or acknowledgement that needs no agent reply; or is only a cancel/hold/stop aimed at work another agent (not {name}) just announced — do not reply just to say you understood or will stay quiet."
                    ),
                },
            }),
        );
    }
    Value::Object(q)
}

/// Parse the `llm` provider's JSON into the chosen set (ids filtered to the candidates).
pub fn llm_pick(v: &Value, room: &Room) -> Vec<String> {
    let mut picked: Vec<String> = v
        .get("responders")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .filter(|id| room.candidates.iter().any(|c| &c.id == id))
                .collect()
        })
        .unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    picked.retain(|id| seen.insert(id.clone()));
    picked
}

/// The `llm` provider's user message.
pub fn llm_user(room: &Room) -> String {
    let agents: Vec<Value> = room
        .candidates
        .iter()
        .map(|c| {
            json!({
                "agent_id": c.id,
                "name": c.name,
                "role": if c.role.trim().is_empty() { Value::Null } else { json!(clip(&c.role, 600)) },
                "host": c.host,
                "mentioned_as": c.mentioned_as,
            })
        })
        .collect();
    let mut body = Map::new();
    body.insert(
        "chat".into(),
        json!({ "kind": room.kind, "id": room.chat_id, "title": room.title }),
    );
    if !room.people.is_empty() {
        body.insert("people".into(), json!(room.people));
    }
    body.insert("agents".into(), json!(agents));
    body.insert("recent_messages".into(), json!(room.recent));
    body.insert(
        "message".into(),
        json!({ "from": room.from, "text": clip(&room.text, MESSAGE_MAX_CHARS) }),
    );
    Value::Object(body).to_string()
}

/// Pick the agents whose yes-probability reaches the threshold.
pub fn threshold_pick(scores: &[(String, f64)], threshold: f64) -> Vec<String> {
    scores
        .iter()
        .filter(|(_, p)| *p >= threshold)
        .map(|(id, _)| id.clone())
        .collect()
}

/// Decide who replies. `Ok` always carries a decision from `jev` or `llm`; `Err` lists what
/// failed, and the caller applies its own default (fail open).
pub async fn decide(
    llm: &Llm,
    cfg: &DecisionConfig,
    llm_model: &str,
    room: &Room,
) -> Result<ResponderDecision, Vec<String>> {
    let mut errors = Vec::new();
    if cfg.provider == DecisionProvider::Jev {
        match decide_jev(llm, cfg, room).await {
            Ok(d) => return Ok(d),
            Err(e) => {
                tracing::warn!(chat = %room.chat_id, error = %e, "respond: decision model failed; falling back to the LLM router");
                errors.push(format!("jev: {e}"));
            }
        }
    }
    match decide_llm(llm, llm_model, cfg.threshold, room).await {
        Ok(mut d) => {
            d.errors = errors;
            Ok(d)
        }
        Err(e) => {
            tracing::warn!(chat = %room.chat_id, error = %e, "respond: LLM router failed");
            errors.push(format!("llm: {e}"));
            Err(errors)
        }
    }
}

/// The `jev` provider: one request, one `noul` per agent, threshold on the probability.
pub async fn decide_jev(
    llm: &Llm,
    cfg: &DecisionConfig,
    room: &Room,
) -> anyhow::Result<ResponderDecision> {
    let state = jev_state(room);
    let questions = jev_questions(room);
    let hash = input_hash(&state);
    let d = llm
        .decide(&cfg.scorer, &state, &questions, DECISION_TIMEOUT)
        .await?;
    let mut scores = Vec::with_capacity(room.candidates.len());
    let mut raw = Map::new();
    for (i, c) in room.candidates.iter().enumerate() {
        let qid = question_id(i);
        let a = d
            .answers
            .get(&qid)
            .ok_or_else(|| anyhow::anyhow!("no answer for {qid} ({})", c.name))?;
        if a.kind != "noul" {
            anyhow::bail!("answer for {qid} is {} not noul", a.kind);
        }
        scores.push((c.id.clone(), a.probability));
        raw.insert(c.id.clone(), a.raw.clone());
    }
    let chosen = threshold_pick(&scores, cfg.threshold);
    // How sure the model is of the chosen set: the weakest "yes" among the chosen, or the weakest
    // "no" when nobody was chosen.
    let confidence = if chosen.is_empty() {
        scores.iter().map(|(_, p)| 1.0 - p).fold(1.0_f64, f64::min)
    } else {
        scores
            .iter()
            .filter(|(id, _)| chosen.contains(id))
            .map(|(_, p)| *p)
            .fold(1.0_f64, f64::min)
    };
    Ok(ResponderDecision {
        reason: format!(
            "jev: {}/{} agents at or above {:.2}",
            chosen.len(),
            scores.len(),
            cfg.threshold
        ),
        chosen,
        scores,
        provider: "jev",
        model: d.model,
        latency_ms: d.latency_ms,
        input_tokens: d.input_tokens,
        output_tokens: d.output_tokens,
        cost_usd: d.cost_usd,
        input_hash: hash,
        threshold: cfg.threshold,
        confidence: Some(confidence),
        raw: Value::Object(raw),
        errors: Vec::new(),
    })
}

/// The `llm` provider: the chat model answers with a JSON list of responders.
pub async fn decide_llm(
    llm: &Llm,
    model: &str,
    threshold: f64,
    room: &Room,
) -> anyhow::Result<ResponderDecision> {
    let user = llm_user(room);
    let hash = input_hash(&Value::String(user.clone()));
    let (v, ms, usage) = llm
        .complete_json_with_usage(model, LLM_SYSTEM, &user, 600)
        .await?;
    let chosen = llm_pick(&v, room);
    Ok(ResponderDecision {
        chosen,
        scores: Vec::new(),
        provider: "llm",
        model: model.to_owned(),
        latency_ms: ms,
        input_tokens: usage
            .get("prompt_tokens")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        output_tokens: usage
            .get("completion_tokens")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        cost_usd: usage.get("cost").and_then(Value::as_f64),
        input_hash: hash,
        threshold,
        reason: v
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("llm router")
            .to_owned(),
        confidence: v.get("confidence").and_then(Value::as_f64),
        raw: v,
        errors: Vec::new(),
    })
}

impl Agent {
    /// `decide` with this project's `decision_model` settings.
    pub async fn decide_responders(&self, room: &Room) -> Result<ResponderDecision, Vec<String>> {
        let (cfg, llm_model): (DecisionConfig, String) = {
            let m = self.settings.models.read().await;
            (m.decision_cfg.clone(), m.decision.id.clone())
        };
        decide(&self.llm, &cfg, &llm_model, room).await
    }

    /// One `memory_decision_log` row (side `respond`) per decision, with inputs hash, scores,
    /// chosen set, latency, tokens and cost; `fallback` names the default a caller applied when
    /// both providers failed.
    pub fn log_responder_decision(
        &self,
        room: &Room,
        event_id: &str,
        decision: Result<&ResponderDecision, &[String]>,
        fallback: Option<(&str, &[String])>,
    ) {
        match decision {
            Ok(d) => {
                let scores: Map<String, Value> = d
                    .scores
                    .iter()
                    .map(|(id, p)| (id.clone(), json!((p * 1000.0).round() / 1000.0)))
                    .collect();
                let detail = json!({
                    "provider": d.provider,
                    "kind": room.kind,
                    "chat_id": room.chat_id,
                    "candidates": room.candidates.iter().map(|c| json!({ "id": c.id, "name": c.name })).collect::<Vec<_>>(),
                    "input_hash": d.input_hash,
                    "threshold": d.threshold,
                    "scores": scores,
                    "raw": d.raw,
                    "errors": d.errors,
                });
                let usage = json!({
                    "provider": d.provider,
                    "input_tokens": d.input_tokens,
                    "output_tokens": d.output_tokens,
                    "cost_usd": d.cost_usd,
                });
                let action = if d.chosen.is_empty() {
                    "NONE"
                } else if d.chosen.len() == room.candidates.len() {
                    "ALL"
                } else {
                    "REPLY"
                };
                let _ = self.store.log_decision_with_usage(
                    "respond",
                    Some(event_id),
                    Some(&detail),
                    Some(action),
                    &d.chosen,
                    Some(&d.reason),
                    d.confidence,
                    Some(&d.model),
                    Some(d.latency_ms),
                    Some(&usage),
                );
                tracing::info!(
                    chat = %room.chat_id,
                    provider = d.provider,
                    model = %d.model,
                    chosen = ?d.chosen,
                    scores = ?d.scores,
                    latency_ms = d.latency_ms,
                    cost_usd = ?d.cost_usd,
                    "respond: decision"
                );
            }
            Err(errors) => {
                let (action, targets) = fallback.unwrap_or(("FAILED", &[]));
                let detail = json!({
                    "provider": "none",
                    "kind": room.kind,
                    "chat_id": room.chat_id,
                    "errors": errors,
                    "fallback": action,
                });
                let _ = self.store.log_decision(
                    "respond",
                    Some(event_id),
                    Some(&detail),
                    Some(action),
                    targets,
                    Some("every decision provider failed; applied the default"),
                    None,
                    None,
                    None,
                );
            }
        }
    }
}
