//! Connector tools (F1): `connectors_list` and `connector_call`. The box never holds a
//! service token; every call goes to the relay broker (`POST /connectors/call`), which signs
//! the upstream request and returns a redacted reply. Actions of kind `write`/`send` pass
//! through the approval gate first (`request_approval` below → `Agent::request_approval`):
//! one card per call, classified once here with the catalog's kind (`mcp.rs` skips
//! `connector_call` in its generic gate). An approved `draft_message` is delivered through the
//! matching connector by `route_draft` + `send_through_relay`.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use arbos_core::approval::{
    classify_tool, ApprovalOutcome as CoreOutcome, ApprovalRequest as CoreRequest, Risk, RiskMatch,
};

use crate::agent::Agent;
use crate::mcp::{err_result, materialize_handle, text_result};

const CATALOG_TTL: Duration = Duration::from_secs(60);
const INLINE_MAX: usize = 16_000;

// ---- approval --------------------------------------------------------------------------------
// String-typed twins of `arbos_core::approval::{ApprovalRequest, ApprovalOutcome}` kept for the
// callers written against the early stub (vault, local helper); `request_approval` is the real
// gate.

/// What the user is asked to approve (title, summary, risk, payload).
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    pub title: String,
    pub summary: String,
    /// `send | destructive | money | credential | computer | other`.
    pub risk: &'static str,
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalOutcome {
    Approved,
    Rejected { reason: String },
    Expired,
}

impl From<CoreOutcome> for ApprovalOutcome {
    fn from(o: CoreOutcome) -> Self {
        match o {
            CoreOutcome::Approved => ApprovalOutcome::Approved,
            CoreOutcome::Rejected { reason } => ApprovalOutcome::Rejected {
                reason: reason.unwrap_or_default(),
            },
            CoreOutcome::Expired => ApprovalOutcome::Expired,
        }
    }
}

/// Post an approval card in `chat_id` and block until the user decides (or the 24 h /
/// `ARBOS_APPROVAL_TIMEOUT_SECS` expiry). Audit entries and the card state are written by
/// `Agent::request_approval`; a card that cannot be posted counts as rejected.
pub async fn request_approval(
    agent: &Arc<Agent>,
    chat_id: &str,
    req: ApprovalRequest,
) -> ApprovalOutcome {
    let core = CoreRequest {
        title: req.title,
        summary: req.summary,
        risk: Risk::parse(req.risk),
        payload: req.payload,
    };
    agent.request_approval(chat_id, core).await.into()
}

// ---- tools ---------------------------------------------------------------------------------

pub fn tool_specs() -> Vec<Value> {
    vec![
        json!({ "name": "connectors_list", "description": "External accounts the user connected (Gmail/Calendar, Slack, GitHub, Harness Comms) with the actions each one offers and their kind (read | write | send). write/send actions ask the user for approval.",
                "inputSchema": { "type": "object", "properties": {} } }),
        json!({ "name": "connector_call", "description": "Act on an external service as the user through the relay (the box never sees a token). provider and action come from connectors_list; args follow that action's schema. Reads run at once; write/send actions wait for the user's approval and fail when it is refused.",
                "inputSchema": { "type": "object", "properties": { "provider": { "type": "string" }, "action": { "type": "string" }, "args": { "type": "object" } }, "required": ["provider", "action"] } }),
    ]
}

async fn relay_get(agent: &Agent, path: &str) -> Result<Value, String> {
    let r = agent
        .http_client()
        .get(format!("{}{}", agent.cfg.relay_url, path))
        .bearer_auth(&agent.cfg.box_token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = r.status();
    let text = r.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "relay {path}: HTTP {status} {}",
            crate::relay_error::message(&text)
        ));
    }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

fn catalog_cache() -> &'static Mutex<Option<(Instant, Value)>> {
    static CACHE: OnceLock<Mutex<Option<(Instant, Value)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

async fn catalog(agent: &Agent) -> Result<Value, String> {
    if let Some((at, v)) = catalog_cache().lock().expect("catalog lock").as_ref() {
        if at.elapsed() < CATALOG_TTL {
            return Ok(v.clone());
        }
    }
    let v = relay_get(agent, "/connectors/catalog").await?;
    *catalog_cache().lock().expect("catalog lock") = Some((Instant::now(), v.clone()));
    Ok(v)
}

fn action_spec<'a>(catalog: &'a Value, provider: &str, action: &str) -> Option<&'a Value> {
    catalog["providers"]
        .as_array()?
        .iter()
        .find(|p| p["provider"].as_str() == Some(provider))?["actions"]
        .as_array()?
        .iter()
        .find(|a| a["name"].as_str() == Some(action))
}

pub async fn connectors_list(agent: &Agent) -> Value {
    let (cat, list) = match (catalog(agent).await, relay_get(agent, "/connectors").await) {
        (Ok(c), Ok(l)) => (c, l),
        (Err(e), _) | (_, Err(e)) => return err_result(e),
    };
    let items = list["items"].as_array().cloned().unwrap_or_default();
    let mut connected = Vec::new();
    let mut available = Vec::new();
    for p in cat["providers"].as_array().cloned().unwrap_or_default() {
        let name = p["provider"].as_str().unwrap_or("").to_owned();
        let cn = items
            .iter()
            .find(|c| c["provider"].as_str() == Some(name.as_str()));
        let actions: Vec<Value> = p["actions"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|x| json!({ "name": x["name"], "kind": x["kind"], "description": x["description"], "args": x["args"]["properties"], "required": x["args"]["required"] }))
                    .collect()
            })
            .unwrap_or_default();
        match cn {
            Some(c) if c["status"] == "connected" => connected.push(json!({
                "provider": name, "label": c["label"], "status": c["status"], "mock": c["mock"], "actions": actions,
            })),
            Some(c) => available.push(json!({ "provider": name, "label": p["label"], "status": c["status"], "note": c["last_error"] })),
            None => available.push(json!({ "provider": name, "label": p["label"], "status": "not_connected" })),
        }
    }
    text_result(
        json!({
            "mock": list["mock"],
            "connected": connected,
            "not_connected": available,
            "note": "write/send actions ask the user for approval first. Connect accounts in Arbos → Settings → Connectors.",
        })
        .to_string(),
    )
}

pub async fn connector_call(agent: &Arc<Agent>, args: &Value, chat_id: &str) -> Value {
    let provider = args["provider"].as_str().unwrap_or("").trim().to_owned();
    let action = args["action"].as_str().unwrap_or("").trim().to_owned();
    let call_args = match args.get("args") {
        Some(Value::Object(o)) => Value::Object(o.clone()),
        Some(Value::String(s)) => serde_json::from_str(s).unwrap_or_else(|_| json!({})),
        _ => json!({}),
    };
    if provider.is_empty() || action.is_empty() {
        return err_result("provider and action are required (see connectors_list)");
    }
    let cat = match catalog(agent).await {
        Ok(c) => c,
        Err(e) => return err_result(e),
    };
    let Some(spec) = action_spec(&cat, &provider, &action) else {
        return err_result(format!(
            "unknown action {provider}.{action}; call connectors_list for the catalog"
        ));
    };
    let kind = spec["kind"].as_str().unwrap_or("read");
    if matches!(kind, "write" | "send") {
        // The single classification of this call: the catalog kind is authoritative, so it is
        // passed explicitly and `classify_tool` runs here and nowhere else for connector_call.
        let gate_args =
            json!({ "provider": provider, "action": action, "kind": kind, "args": call_args });
        let hit = classify_tool("connector_call", &gate_args).unwrap_or(RiskMatch {
            class: "connector-send",
            risk: Risk::Send,
            why: "leaves the box as a message the user did not write",
            path_class: None,
        });
        if let Some(granted) = agent.take_grant("connector_call", &gate_args).await {
            tracing::info!(
                provider,
                action,
                approval_id = granted,
                "pre-approved connector call"
            );
        } else {
            let outcome = request_approval(
                agent,
                chat_id,
                ApprovalRequest {
                    title: format!("{provider}: {action}"),
                    summary: summarize(&provider, &action, &call_args),
                    risk: hit.risk.as_str(),
                    payload: json!({ "kind": "tool_call", "tool": "connector_call", "input": gate_args, "class": hit.class, "why": hit.why }),
                },
            )
            .await;
            match outcome {
                ApprovalOutcome::Approved => {}
                ApprovalOutcome::Rejected { reason } => {
                    return err_result(format!(
                        "{provider}.{action} not approved: the user rejected it{}; do not retry it unchanged",
                        if reason.trim().is_empty() { String::new() } else { format!(" ({reason})") }
                    ));
                }
                ApprovalOutcome::Expired => {
                    return err_result(format!(
                        "{provider}.{action}: the approval expired before the user decided"
                    ));
                }
            }
        }
    }
    let text = match send_through_relay(agent, chat_id, &provider, &action, &call_args).await {
        Ok(t) => t,
        Err(e) => return err_result(e),
    };
    if text.len() > INLINE_MAX {
        let handle = materialize_handle(
            agent,
            chat_id,
            &format!("{provider}.{action} result"),
            &text,
        );
        return text_result(
            json!({ "handle": handle, "preview": clip(&text, 4000), "note": "full result by handle (peek)" })
                .to_string(),
        );
    }
    text_result(text)
}

/// Forward one action to the relay broker and return its reply body. No gate here: callers
/// hold the approval already (`connector_call` after its card, `dispatch_draft` after the
/// draft's card).
pub(crate) async fn send_through_relay(
    agent: &Agent,
    chat_id: &str,
    provider: &str,
    action: &str,
    args: &Value,
) -> Result<String, String> {
    let _ = agent.store.t3_append(
        chat_id,
        None,
        Some(chat_id),
        "connector_call",
        Some("assistant"),
        &format!("{provider}.{action} {}", clip(&args.to_string(), 400)),
        None,
    );
    let r = agent
        .http_client()
        .post(format!("{}/connectors/call", agent.cfg.relay_url))
        .bearer_auth(&agent.cfg.box_token)
        .json(&json!({ "provider": provider, "action": action, "args": args }))
        .send()
        .await;
    let (status, text) = match r {
        Ok(r) => {
            let st = r.status();
            (st, r.text().await.unwrap_or_default())
        }
        Err(e) => return Err(format!("relay unreachable: {e}")),
    };
    if !status.is_success() {
        return Err(format!(
            "{provider}.{action}: {}",
            crate::relay_error::message(&text)
        ));
    }
    Ok(text)
}

/// Providers the user has connected (relay `GET /connectors`, `status == "connected"`).
pub(crate) async fn connected_providers(agent: &Agent) -> Result<Vec<String>, String> {
    let list = relay_get(agent, "/connectors").await?;
    Ok(list["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter(|c| c["status"] == "connected")
                .filter_map(|c| c["provider"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default())
}

/// The connector action that delivers an approved `draft_message`, given what is connected:
/// email → `google.send`, else `harness_comms.send`; slack → `slack.post_message`;
/// telegram/whatsapp → `harness_comms.send` (Beeper chat id). `None` when no connected
/// provider carries the channel (the draft then stays in the box outbox).
pub(crate) fn route_draft(
    channel: &str,
    to: &str,
    subject: Option<&str>,
    body: &str,
    connected: &[String],
) -> Option<(&'static str, &'static str, Value)> {
    let has = |p: &str| connected.iter().any(|c| c == p);
    match channel {
        "email" if has("google") => Some((
            "google",
            "send",
            json!({ "to": to, "subject": subject.unwrap_or(""), "body": body }),
        )),
        "email" if has("harness_comms") => Some((
            "harness_comms",
            "send",
            json!({ "to": to, "subject": subject.unwrap_or(""), "text": body }),
        )),
        "slack" if has("slack") => Some((
            "slack",
            "post_message",
            json!({ "channel": to, "text": body }),
        )),
        "telegram" | "whatsapp" if has("harness_comms") => Some((
            "harness_comms",
            "send",
            json!({ "chat_id": to, "text": body }),
        )),
        _ => None,
    }
}

fn summarize(provider: &str, action: &str, args: &Value) -> String {
    let s = |k: &str| args[k].as_str().unwrap_or("").to_owned();
    match (provider, action) {
        ("google", "send") | ("google", "create_draft") => {
            format!(
                "Email to {} — {}: {}",
                s("to"),
                s("subject"),
                clip(&s("body"), 200)
            )
        }
        ("google", "create_event") => format!(
            "Calendar event {} ({} → {})",
            s("summary"),
            s("start"),
            s("end")
        ),
        ("slack", "post_message") => {
            format!("Slack post in {}: {}", s("channel"), clip(&s("text"), 200))
        }
        ("github", "create_issue") => format!("GitHub issue in {}: {}", s("repo"), s("title")),
        ("github", "comment") => format!(
            "GitHub comment on {}#{}: {}",
            s("repo"),
            args["number"],
            clip(&s("body"), 200)
        ),
        ("harness_comms", "send") => {
            let target = if !s("chat_id").is_empty() {
                s("chat_id")
            } else {
                s("to")
            };
            format!("Message to {target}: {}", clip(&s("text"), 200))
        }
        ("harness_comms", "draft") => format!("Draft: {}", clip(&s("text"), 200)),
        _ => format!("{provider}.{action} {}", clip(&args.to_string(), 200)),
    }
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::route_draft;

    /// Invariant: an approved draft goes to the connected provider for its channel and nowhere
    /// else; with nothing connected it is not routed at all.
    #[test]
    fn draft_routing_follows_connected_providers() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let (p, a, args) = route_draft(
            "email",
            "sam@example.com",
            Some("Hi"),
            "body",
            &s(&["google"]),
        )
        .unwrap();
        assert_eq!((p, a), ("google", "send"));
        assert_eq!(args["to"], "sam@example.com");
        let (p, a, args) = route_draft(
            "email",
            "sam@example.com",
            None,
            "body",
            &s(&["harness_comms"]),
        )
        .unwrap();
        assert_eq!((p, a), ("harness_comms", "send"));
        assert_eq!(args["text"], "body");
        let (p, a, _) = route_draft("slack", "#ops", None, "hi", &s(&["slack", "google"])).unwrap();
        assert_eq!((p, a), ("slack", "post_message"));
        let (p, a, args) =
            route_draft("whatsapp", "chat-1", None, "hi", &s(&["harness_comms"])).unwrap();
        assert_eq!((p, a), ("harness_comms", "send"));
        assert_eq!(args["chat_id"], "chat-1");
        assert!(route_draft("email", "x", None, "b", &s(&["slack"])).is_none());
        assert!(route_draft("sms", "+1", None, "b", &s(&["google", "harness_comms"])).is_none());
        assert!(route_draft("slack", "#ops", None, "b", &[]).is_none());
    }
}
