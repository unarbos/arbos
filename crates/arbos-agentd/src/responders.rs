//! Who replies in a group chat: the judge shared by project groups (`groups.rs`, design D69)
//! and people chats (`people.rs`, D72).
//!
//! One Decisions-API call per message per box to the system model (`typesafe/jev-1.14`), the
//! same state on every box in the room: the people, every candidate agent (handle, role, how
//! the text @mentions it), the recent lines, the message and the line it replies to. Two kinds
//! of question, one template each:
//!
//! - `addressee` (`choice`): who is the message for — one option per agent, `any one agent`
//!   (one unspecified agent: "any of you", "the agent"), `several agents`, `all agents`,
//!   `nobody`. The answer is a probability per option.
//! - `reply_<i>` (`noul`): should this agent reply — one per candidate, used to pick which
//!   agents when the addressee is `several agents` and which one when it is `any one agent`.
//!
//! The pick rule is the same everywhere, so the boxes agree without talking: `nobody` → silence;
//! one named agent → that agent iff its probability reaches the threshold; `any one agent` →
//! the candidate with the highest `reply` probability (one agent, not three); `several agents`
//! → every candidate whose `reply` probability reaches the threshold; `all agents` → everyone.
//! Each of those needs the addressee's own probability at or above the threshold (a confident
//! yes). A tie goes to the candidate that comes first in agent-id order (ULIDs: creation
//! order). No keyword rule runs
//! before or after the judge; @mentions, names, replies and the roster are signals in the
//! state, never a verdict (Jacob, 2026-10-27). There is no second model to fall back to: on an
//! error the caller applies its own default.
//!
//! Every decision is logged (`memory_decision_log`, side `respond`): model build, input hash,
//! addressee probabilities, per-agent scores, chosen set, the one-line reason, latency, tokens
//! and cost.

use std::time::Duration;

use serde_json::{json, Map, Value};
use sha2::Digest;

use crate::agent::Agent;
use crate::llm::Llm;
use crate::settings::DecisionConfig;

/// Decision-model round trip budget; p99 on OpenRouter is ~0.6 s.
const DECISION_TIMEOUT: Duration = Duration::from_millis(3000);
const MESSAGE_MAX_CHARS: usize = 4000;
/// Two addressee options this close are a tie, settled by candidate order.
const TIE_EPSILON: f64 = 0.005;

pub const OPT_ANY_ONE: &str = "any one agent";
pub const OPT_SEVERAL: &str = "several agents";
pub const OPT_ALL: &str = "all agents";
pub const OPT_NOBODY: &str = "nobody";

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
    /// obligation; the judge reads this instead of treating every mention as a question.
    pub mentioned_as: Option<&'static str>,
}

/// The message and its room, as the judge sees it.
#[derive(Debug, Clone)]
pub struct Room {
    /// `project_group` (the user with several of their Projects) or `people_group`.
    pub kind: &'static str,
    pub chat_id: String,
    pub title: String,
    /// Who wrote the message, as a label (`the user`, `Sam`, `Project Research`).
    pub from: String,
    pub text: String,
    /// Earlier messages, oldest first: `[{ "from": label, "text": … }]`.
    pub recent: Vec<Value>,
    /// Candidates in agent-id order (the caller sorts; the order is part of the tie-break).
    pub candidates: Vec<Candidate>,
    /// Human members by spoken name (people groups). Empty for project groups. Lets the judge
    /// tell a first-name ping or call invite aimed at people from a request to an agent.
    pub people: Vec<String>,
    /// The line `message` replies to, when it quotes one: `{ "from": label, "text": … }`.
    pub reply_to: Option<Value>,
}

/// What the judge concluded and how.
#[derive(Debug, Clone)]
pub struct ResponderDecision {
    pub chosen: Vec<String>,
    /// The `addressee` option with the highest probability (after the tie-break).
    pub addressee: String,
    pub addressee_p: f64,
    /// Probability per `addressee` option (candidate ids for the agents, then the three words).
    pub addressee_probs: Vec<(String, f64)>,
    /// `P(reply)` per candidate id, in candidate order.
    pub scores: Vec<(String, f64)>,
    pub model: String,
    pub latency_ms: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: Option<f64>,
    pub input_hash: String,
    pub threshold: f64,
    /// Composed from the answer: `judge: for=nobody (0.97) → silent`, …
    pub reason: String,
    /// How sure the judge is of the chosen set: the addressee's probability, or the weakest
    /// `reply` probability among the chosen for several / all.
    pub confidence: f64,
    pub tie_break: bool,
    /// Raw answers.
    pub raw: Value,
}

impl ResponderDecision {
    pub fn chose(&self, id: &str) -> bool {
        self.chosen.iter().any(|c| c == id)
    }

    /// The reason line from one agent's point of view (`me`), for that agent's gate log.
    pub fn reason_for(&self, me: &str, room: &Room) -> String {
        let name = |id: &str| -> String {
            room.candidates
                .iter()
                .find(|c| c.id == id)
                .map(|c| c.name.clone())
                .unwrap_or_else(|| id.to_owned())
        };
        let my_p = self
            .scores
            .iter()
            .find(|(id, _)| id == me)
            .map(|(_, p)| *p)
            .unwrap_or(0.0);
        let verdict = if self.chose(me) { "reply" } else { "silent" };
        let tie = if self.tie_break {
            " [tie-break: agent-id order]"
        } else {
            ""
        };
        let addressee = match self.addressee.as_str() {
            OPT_NOBODY | OPT_ALL | OPT_SEVERAL | OPT_ANY_ONE => self.addressee.clone(),
            id => name(id),
        };
        match self.addressee.as_str() {
            OPT_NOBODY => format!(
                "judge: for={addressee} ({:.2}); me {my_p:.2} → {verdict}{tie}",
                self.addressee_p
            ),
            OPT_ALL if self.addressee_p >= self.threshold => format!(
                "judge: for={addressee} ({:.2}) ≥ {:.2} → everyone replies{tie}",
                self.addressee_p, self.threshold
            ),
            OPT_ANY_ONE if self.addressee_p >= self.threshold => {
                let taker = self.chosen.first().map(|id| name(id)).unwrap_or_default();
                if self.chose(me) {
                    format!(
                        "judge: for={addressee} ({:.2}); me {my_p:.2} is the best score → reply{tie}",
                        self.addressee_p
                    )
                } else {
                    format!(
                        "judge: for={addressee} ({:.2}); {taker} has the best score, me {my_p:.2} → silent{tie}",
                        self.addressee_p
                    )
                }
            }
            OPT_ALL | OPT_SEVERAL | OPT_ANY_ONE if self.addressee_p < self.threshold => format!(
                "judge: for={addressee} ({:.2}) < {:.2} → silent (not confident){tie}",
                self.addressee_p, self.threshold
            ),
            OPT_SEVERAL => {
                let others: Vec<String> = self
                    .chosen
                    .iter()
                    .filter(|id| *id != me)
                    .map(|id| name(id))
                    .collect();
                let with = if self.chose(me) && !others.is_empty() {
                    format!(" (with {})", others.join(", "))
                } else if !self.chose(me) && !others.is_empty() {
                    format!(" ({} reply)", others.join(", "))
                } else {
                    String::new()
                };
                let cmp = if my_p >= self.threshold { "≥" } else { "<" };
                format!(
                    "judge: for={addressee} ({:.2}); me {my_p:.2} {cmp} {:.2} → {verdict}{with}{tie}",
                    self.addressee_p, self.threshold
                )
            }
            id if id == me => {
                let cmp = if self.addressee_p >= self.threshold {
                    "≥"
                } else {
                    "<"
                };
                let why = if self.addressee_p >= self.threshold {
                    ""
                } else {
                    " (not confident)"
                };
                format!(
                    "judge: for={addressee} ({:.2}) {cmp} {:.2} → {verdict}{why}{tie}",
                    self.addressee_p, self.threshold
                )
            }
            _ => format!(
                "judge: for={addressee} ({:.2}); not me → silent{tie}",
                self.addressee_p
            ),
        }
    }
}

/// Whitespace-collapsed and capped, with a marker when cut (the judge cannot fetch, so the
/// marker only says how much is missing).
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

/// The Decisions-API `state`: the same object on every box (all questions see all agents).
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
    let mut message = Map::new();
    message.insert("from".into(), json!(room.from));
    message.insert("text".into(), json!(clip(&room.text, MESSAGE_MAX_CHARS)));
    if let Some(r) = &room.reply_to {
        message.insert("reply_to".into(), r.clone());
    }
    state.insert("message".into(), Value::Object(message));
    Value::Object(state)
}

/// The questions: one `addressee` choice over the room, one `reply` yes/no per candidate.
pub fn jev_questions(room: &Room) -> Value {
    let mut q = Map::new();
    let names: Vec<String> = room.candidates.iter().map(|c| c.name.clone()).collect();
    let mut criteria = Map::new();
    for c in &room.candidates {
        criteria.insert(
            c.name.clone(),
            json!(format!(
                "`message` is for {name} alone: it asks or tells {name} something — by @mention or by name (also a bare first name, \"Dear x\", \"x?\"); as a reply to {name}'s line (`message.reply_to` from {name}) that asks or tells it something, even with no @; as the sender's next question or instruction right after {name} answered them in `recent_messages` (\"Who is in this chat?\", \"How many messages are there?\" moments after {name} replied to that person are for {name}); as a correction naming {name} (\"*the x agent sorry\"); as a cancel/hold/stop of work {name} just announced; or as a request that belongs to {name}'s role and to no other agent.",
                name = c.name
            )),
        );
    }
    criteria.insert(
        OPT_ANY_ONE.into(),
        json!("`message` asks for ONE agent without saying which (\"any of you agents, …\", \"can one of you …\", \"can an agent …\", \"I'd like an agent to …\"): exactly one should take it, not everyone. A remark about what an agent did or should have done (\"just wanted one agent to do it\") is `nobody`."),
    );
    criteria.insert(
        OPT_SEVERAL.into(),
        json!("`message` names or asks two or more of the agents but not every one of them (for example \"@a and @b, …\", \"a and b, standup\")."),
    );
    criteria.insert(
        OPT_ALL.into(),
        json!("`message` clearly addresses every agent in the room (for example \"@agents …\", \"all agents\", \"attention all agents\", \"agents, standup\", \"hey agents\", \"each of you\"), even when it asks nothing yet. \"Any of you\" or \"one of the agents\" is `any one agent`, not all."),
    );
    criteria.insert(
        OPT_NOBODY.into(),
        json!("No agent should reply: people are talking to each other (greetings, pings by first name or @handle of a person, call invites, jokes, product feedback for a person, group admin talk); it is only a remark, reaction (\"lol\", \"oh\", \"meow\") or acknowledgement; it is ABOUT agents or about one agent rather than TO one (\"the agents are getting better\", \"doug is broken\", \"they get triggered by the word agents\"); it only thanks or praises an agent (\"thanks @x\", \"@x well done\"); it mentions an agent only to pass information along or to tell another PERSON to ask it (\"ask @x\", \"@sam use this agent\"); or it replies to an agent's line without asking it anything."),
    );
    q.insert(
        "addressee".into(),
        json!({
            "type": "choice",
            "instructions": format!(
                "Who is `message` for? Read `message` with `recent_messages` (what was just said, who was answering whom), `people` (the humans in the room; a first name or @handle that is a person's is never an agent), `agents` (every agent's name, role and — when present — `mentioned_as`: how `message` @mentions it; `object` means it is only talked about, as in \"talk with {first}\" or \"send it to {first}\") and `message.reply_to` (the line it quotes). Pick the ONE agent it is for, or `{any_one}` (one agent, unspecified), `{several}`, `{all}`, or `{nobody}`. Staying quiet means no reply at all, never a reply that says the agent will stay quiet. An @mention is a strong signal that the agent is meant, not proof: \"thanks @x\" and \"ask @x about it\" are `{nobody}`. A message that quotes an agent's line (`reply_to`) and asks or tells it something is for that agent; one that only reacts to it is `{nobody}`. Agents in the room: {names}.",
                first = names.first().cloned().unwrap_or_else(|| "@agent".into()),
                any_one = OPT_ANY_ONE, several = OPT_SEVERAL, all = OPT_ALL, nobody = OPT_NOBODY,
                names = names.join(", ")
            ),
            "criteria": Value::Object(criteria),
        }),
    );
    for (i, c) in room.candidates.iter().enumerate() {
        let name = &c.name;
        q.insert(
            question_id(i),
            json!({
                "type": "noul",
                "instructions": format!(
                    "Should the agent \"{name}\" reply to `message`? Judge {name} on its own, from its name and role in `agents` (and its `mentioned_as`, when `message` @mentions it — `object` means only talked about, not asked), from `people`, from `recent_messages`, from `message.reply_to` and from what `message` asks. Prefer false when `message` is for people rather than agents, is ABOUT agents rather than TO them, only thanks or reacts, or is a cancel/hold/stop of work another agent (not {name}) just announced. Staying quiet means no reply at all."
                ),
                "criteria": {
                    "true": format!(
                        "`message` asks {name} for something, directly or as part of a request to several or all agents; addresses {name} by @mention or name; replies to {name}'s line with a question or request; continues an exchange {name} was just having with the sender (a follow-up question, a correction); asks for work that belongs to {name}'s role; or is a cancel/hold/stop of work {name} just announced in `recent_messages`."
                    ),
                    "false": format!(
                        "`message` is people talking to each other; is about agents or about {name} rather than to {name}; only thanks, praises or reacts; mentions {name} only to pass information along or as the object of a request to someone else; is addressed only to other agents; is only a remark or acknowledgement that needs no agent reply; or is a cancel/hold/stop aimed at work another agent (not {name}) announced."
                    ),
                },
            }),
        );
    }
    Value::Object(q)
}

/// The pick rule over the raw probabilities, at `threshold` (also used by the eval's sweep).
/// `addressee_probs` is in option order: candidates (agent-id order) first, then the words.
pub fn pick(
    addressee_probs: &[(String, f64)],
    scores: &[(String, f64)],
    threshold: f64,
) -> (String, f64, bool, Vec<String>) {
    let mut best: Option<(&str, f64)> = None;
    let mut tie = false;
    for (opt, p) in addressee_probs {
        match best {
            None => best = Some((opt, *p)),
            Some((_, bp)) if *p > bp + TIE_EPSILON => {
                best = Some((opt, *p));
                tie = false;
            }
            Some((_, bp)) if (*p - bp).abs() <= TIE_EPSILON => tie = true,
            Some(_) => {}
        }
    }
    let (addressee, p) = best.unwrap_or((OPT_NOBODY, 1.0));
    let chosen = match addressee {
        OPT_NOBODY => Vec::new(),
        _ if p < threshold => Vec::new(),
        OPT_ALL => scores.iter().map(|(id, _)| id.clone()).collect(),
        OPT_SEVERAL => scores
            .iter()
            .filter(|(_, s)| *s >= threshold)
            .map(|(id, _)| id.clone())
            .collect(),
        OPT_ANY_ONE => {
            // The best-scored candidate; equal scores go to the first in id order.
            let mut best: Option<(&str, f64)> = None;
            for (id, s) in scores {
                if best.is_none_or(|(_, bs)| *s > bs + TIE_EPSILON) {
                    best = Some((id, *s));
                }
            }
            best.map(|(id, _)| vec![id.to_owned()]).unwrap_or_default()
        }
        id => vec![id.to_owned()],
    };
    (addressee.to_owned(), p, tie, chosen)
}

/// Ask the judge and apply the pick rule. `Err` when the call or its shape failed; the caller
/// applies its own default (people chats fail closed, see `people.rs`).
pub async fn decide(
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
    let who = d
        .answers
        .get("addressee")
        .ok_or_else(|| anyhow::anyhow!("no answer for addressee"))?;
    if who.kind != "choice" {
        anyhow::bail!("answer for addressee is {} not choice", who.kind);
    }
    let probs = who
        .raw
        .get("probabilities")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("addressee answer without probabilities: {}", who.raw))?;
    let mut addressee_probs = Vec::with_capacity(room.candidates.len() + 3);
    for c in &room.candidates {
        let p = probs.get(&c.name).and_then(Value::as_f64).unwrap_or(0.0);
        addressee_probs.push((c.id.clone(), p));
    }
    for opt in [OPT_ANY_ONE, OPT_SEVERAL, OPT_ALL, OPT_NOBODY] {
        let p = probs.get(opt).and_then(Value::as_f64).unwrap_or(0.0);
        addressee_probs.push((opt.to_owned(), p));
    }
    let mut scores = Vec::with_capacity(room.candidates.len());
    let mut raw = Map::new();
    raw.insert("addressee".into(), who.raw.clone());
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
    let (addressee, addressee_p, tie_break, chosen) =
        pick(&addressee_probs, &scores, cfg.threshold);
    let confidence = match addressee.as_str() {
        OPT_SEVERAL | OPT_ANY_ONE if !chosen.is_empty() => scores
            .iter()
            .filter(|(id, _)| chosen.contains(id))
            .map(|(_, p)| *p)
            .fold(1.0_f64, f64::min),
        _ => addressee_p,
    };
    let shown = match addressee.as_str() {
        OPT_NOBODY | OPT_ALL | OPT_SEVERAL | OPT_ANY_ONE => addressee.clone(),
        id => room
            .candidates
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.name.clone())
            .unwrap_or_else(|| id.to_owned()),
    };
    let reason = format!(
        "judge: for={shown} ({addressee_p:.2}); {}/{} reply at ≥ {:.2}{}",
        chosen.len(),
        scores.len(),
        cfg.threshold,
        if tie_break {
            " [tie-break: agent-id order]"
        } else {
            ""
        }
    );
    Ok(ResponderDecision {
        chosen,
        addressee,
        addressee_p,
        addressee_probs,
        scores,
        model: d.model,
        latency_ms: d.latency_ms,
        input_tokens: d.input_tokens,
        output_tokens: d.output_tokens,
        cost_usd: d.cost_usd,
        input_hash: hash,
        threshold: cfg.threshold,
        reason,
        confidence,
        tie_break,
        raw: Value::Object(raw),
    })
}

impl Agent {
    /// `decide` with this project's `decision_model` settings.
    pub async fn decide_responders(&self, room: &Room) -> anyhow::Result<ResponderDecision> {
        let cfg: DecisionConfig = self.settings.models.read().await.decision_cfg.clone();
        decide(&self.llm, &cfg, room).await
    }

    /// One `memory_decision_log` row (side `respond`) per decision, with inputs hash,
    /// addressee probabilities, scores, chosen set, latency, tokens and cost; `fallback` names
    /// the default a caller applied when the judge failed.
    pub fn log_responder_decision(
        &self,
        room: &Room,
        event_id: &str,
        decision: Result<&ResponderDecision, &str>,
        fallback: Option<(&str, &[String])>,
    ) {
        let r3 = |p: &f64| json!((p * 1000.0).round() / 1000.0);
        match decision {
            Ok(d) => {
                let scores: Map<String, Value> =
                    d.scores.iter().map(|(id, p)| (id.clone(), r3(p))).collect();
                let addressee: Map<String, Value> = d
                    .addressee_probs
                    .iter()
                    .map(|(id, p)| (id.clone(), r3(p)))
                    .collect();
                let detail = json!({
                    "provider": "judge",
                    "kind": room.kind,
                    "chat_id": room.chat_id,
                    "candidates": room.candidates.iter().map(|c| json!({ "id": c.id, "name": c.name })).collect::<Vec<_>>(),
                    "input_hash": d.input_hash,
                    "threshold": d.threshold,
                    "addressee": d.addressee,
                    "addressee_probs": addressee,
                    "scores": scores,
                    "tie_break": d.tie_break,
                    "raw": d.raw,
                });
                let usage = json!({
                    "provider": "judge",
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
                    Some(d.confidence),
                    Some(&d.model),
                    Some(d.latency_ms),
                    Some(&usage),
                );
                tracing::info!(
                    chat = %room.chat_id,
                    model = %d.model,
                    addressee = %d.addressee,
                    chosen = ?d.chosen,
                    scores = ?d.scores,
                    latency_ms = d.latency_ms,
                    cost_usd = ?d.cost_usd,
                    "respond: judge"
                );
            }
            Err(error) => {
                let (action, targets) = fallback.unwrap_or(("FAILED", &[]));
                let detail = json!({
                    "provider": "none",
                    "kind": room.kind,
                    "chat_id": room.chat_id,
                    "error": error,
                    "fallback": action,
                });
                let _ = self.store.log_decision(
                    "respond",
                    Some(event_id),
                    Some(&detail),
                    Some(action),
                    targets,
                    Some(&format!("judge failed: {error}; applied the default")),
                    None,
                    None,
                    None,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probs(v: &[(&str, f64)]) -> Vec<(String, f64)> {
        v.iter().map(|(k, p)| (k.to_string(), *p)).collect()
    }

    #[test]
    fn nobody_means_silence_whatever_the_scores_say() {
        let (who, p, tie, chosen) = pick(
            &probs(&[
                ("A", 0.01),
                ("B", 0.02),
                (OPT_SEVERAL, 0.0),
                (OPT_ALL, 0.03),
                (OPT_NOBODY, 0.94),
            ]),
            &probs(&[("A", 0.7), ("B", 0.9)]),
            0.6,
        );
        assert_eq!(who, OPT_NOBODY);
        assert!(p > 0.9);
        assert!(!tie);
        assert!(chosen.is_empty());
    }

    #[test]
    fn one_agent_needs_a_confident_yes() {
        let (who, _, _, chosen) = pick(
            &probs(&[
                ("A", 0.82),
                ("B", 0.10),
                (OPT_SEVERAL, 0.0),
                (OPT_ALL, 0.0),
                (OPT_NOBODY, 0.08),
            ]),
            &probs(&[("A", 0.9), ("B", 0.1)]),
            0.6,
        );
        assert_eq!((who.as_str(), chosen), ("A", vec!["A".to_string()]));
        let (_, _, _, chosen) = pick(
            &probs(&[
                ("A", 0.52),
                ("B", 0.10),
                (OPT_SEVERAL, 0.0),
                (OPT_ALL, 0.0),
                (OPT_NOBODY, 0.38),
            ]),
            &probs(&[("A", 0.9), ("B", 0.1)]),
            0.6,
        );
        assert!(chosen.is_empty(), "0.52 is not a confident yes at 0.60");
    }

    #[test]
    fn all_means_everyone_several_uses_the_per_agent_scores() {
        let (who, _, _, chosen) = pick(
            &probs(&[
                ("A", 0.05),
                ("B", 0.05),
                (OPT_ANY_ONE, 0.0),
                (OPT_SEVERAL, 0.1),
                (OPT_ALL, 0.75),
                (OPT_NOBODY, 0.05),
            ]),
            &probs(&[("A", 0.9), ("B", 0.3)]),
            0.6,
        );
        assert_eq!(who, OPT_ALL);
        assert_eq!(
            chosen,
            vec!["A".to_string(), "B".to_string()],
            "everyone, scores or not"
        );
        let (who, _, _, chosen) = pick(
            &probs(&[
                ("A", 0.05),
                ("B", 0.05),
                (OPT_ANY_ONE, 0.0),
                (OPT_SEVERAL, 0.8),
                (OPT_ALL, 0.05),
                (OPT_NOBODY, 0.05),
            ]),
            &probs(&[("A", 0.9), ("B", 0.3), ("C", 0.7)]),
            0.6,
        );
        assert_eq!(who, OPT_SEVERAL);
        assert_eq!(chosen, vec!["A".to_string(), "C".to_string()]);
        let (_, _, _, chosen) = pick(
            &probs(&[
                ("A", 0.05),
                ("B", 0.05),
                (OPT_ANY_ONE, 0.0),
                (OPT_SEVERAL, 0.0),
                (OPT_ALL, 0.45),
                (OPT_NOBODY, 0.45),
            ]),
            &probs(&[("A", 0.9), ("B", 0.9)]),
            0.6,
        );
        assert!(
            chosen.is_empty(),
            "all agents at 0.45 is not a confident yes"
        );
    }

    #[test]
    fn any_one_agent_means_the_best_scored_one() {
        let (who, _, _, chosen) = pick(
            &probs(&[
                ("A", 0.1),
                ("B", 0.1),
                (OPT_ANY_ONE, 0.7),
                (OPT_SEVERAL, 0.05),
                (OPT_ALL, 0.0),
                (OPT_NOBODY, 0.05),
            ]),
            &probs(&[("A", 0.64), ("B", 0.8), ("C", 0.75)]),
            0.6,
        );
        assert_eq!((who.as_str(), chosen), (OPT_ANY_ONE, vec!["B".to_string()]));
        // Equal scores: the first in id order, on every box.
        let (_, _, _, chosen) = pick(
            &probs(&[
                ("A", 0.1),
                ("B", 0.1),
                (OPT_ANY_ONE, 0.7),
                (OPT_SEVERAL, 0.0),
                (OPT_ALL, 0.0),
                (OPT_NOBODY, 0.1),
            ]),
            &probs(&[("A", 0.8), ("B", 0.8)]),
            0.6,
        );
        assert_eq!(chosen, vec!["A".to_string()]);
    }

    #[test]
    fn a_tie_goes_to_the_first_agent_in_id_order() {
        let (who, _, tie, chosen) = pick(
            &probs(&[
                ("A", 0.45),
                ("B", 0.45),
                (OPT_SEVERAL, 0.0),
                (OPT_ALL, 0.0),
                (OPT_NOBODY, 0.1),
            ]),
            &probs(&[("A", 0.5), ("B", 0.5)]),
            0.4,
        );
        assert!(tie);
        assert_eq!((who.as_str(), chosen), ("A", vec!["A".to_string()]));
    }

    #[test]
    fn the_state_carries_the_signals_not_verdicts() {
        let room = Room {
            kind: "people_group",
            chat_id: "c".into(),
            title: "group".into(),
            from: "Jacob".into(),
            text: "thanks @affine".into(),
            recent: vec![json!({ "from": "@affine (agent)", "text": "Done." })],
            candidates: vec![Candidate {
                id: "A".into(),
                name: "@affine".into(),
                role: String::new(),
                host: false,
                mentioned_as: Some("object"),
            }],
            people: vec!["Jacob".into()],
            reply_to: Some(json!({ "from": "@affine (agent)", "text": "Done." })),
        };
        let state = jev_state(&room);
        assert_eq!(state["agents"][0]["mentioned_as"], "object");
        assert_eq!(state["message"]["reply_to"]["from"], "@affine (agent)");
        let q = jev_questions(&room);
        assert_eq!(q["addressee"]["type"], "choice");
        assert!(q["addressee"]["criteria"][OPT_NOBODY]
            .as_str()
            .unwrap()
            .contains("thanks"));
        assert_eq!(q["reply_0"]["type"], "noul");
    }
}
