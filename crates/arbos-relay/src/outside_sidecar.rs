//! Sidecar support for outside agents (2026-10-28, #2189; design `docs/outside-agents-sidecar-spec.md`
//! in the store). The sidecar is one small program on the agent's own machine that holds the relay
//! connection, mirrors chats and nudges the agent; this module gives it what `outside.rs` did not
//! have yet: who is in a chat, a context bundle on wake events, and the public texts an agent is
//! handed (`/llms.txt`) or runs (`/sidecar/install.sh`).
//!
//! History (`GET /asp/chats/{id}/messages`), `attachments[]`, files and the scope switches are
//! #2182's; the hosted Wake URL is #2185's. Nothing here duplicates them.

use arbos_core::doc::MessageView;
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use crate::db::Project;
use crate::notify::ChatRef;
use crate::outside::AgentPrincipal;
use crate::people;
use crate::signin::api_error;
use crate::state::AppState;

/// The agent instructions, served at `/llms.txt`; the canonical relay host is rewritten to this
/// relay's public URL so self-hosters hand out the right address.
const LLMS_TXT: &str = include_str!("../assets/llms.txt");
/// The one-line installer, served at `/sidecar/install.sh`.
const INSTALL_SH: &str = include_str!("../assets/sidecar-install.sh");
const CANONICAL_RELAY: &str = "https://relay.arbos.life";
/// Most rows a wake event may carry as context.
pub const CONTEXT_MAX: usize = 20;

fn scope_refused(what: &str) -> Response {
    api_error(
        StatusCode::FORBIDDEN,
        "scope",
        format!("{what} is switched off for this agent"),
    )
}

// ---- members --------------------------------------------------------------------------------------

fn user_member(state: &AppState, uid: &str, role: &str) -> Value {
    let card = state.db.user_card(uid).ok().flatten();
    json!({
        "kind": "user",
        "id": uid,
        "name": card.as_ref().and_then(|c| c.name.clone()).unwrap_or_default(),
        "handle": card.map(|c| c.handle).unwrap_or_default(),
        "role": role,
        "is_me": false,
    })
}

fn agent_member(state: &AppState, pid: &str, me: &Project) -> Value {
    let name = if pid == me.id {
        me.name.clone()
    } else {
        state
            .db
            .project(pid)
            .ok()
            .flatten()
            .map(|p| p.name)
            .unwrap_or_default()
    };
    json!({
        "kind": "agent",
        "id": pid,
        "name": name,
        "handle": people::agent_slug(&name),
        "role": "agent",
        "is_me": pid == me.id,
    })
}

/// Everyone in `chat` as the sidecar mirrors them. The agent's own 1:1 chat has two members:
/// the owner and the agent.
pub fn members_of(state: &AppState, me: &Project, chat: &Value) -> Vec<Value> {
    let chat_id = chat["id"].as_str().unwrap_or_default();
    if chat["project_id"].as_str().is_some() {
        return vec![
            user_member(state, &me.user_id, "owner"),
            agent_member(state, &me.id, me),
        ];
    }
    let Ok(cm) = people::load_chat(state, chat_id) else {
        return Vec::new();
    };
    cm.members
        .iter()
        .map(|m| match (m.user_id(), m.project_id()) {
            (Some(uid), _) => user_member(state, uid, &m.role),
            (_, Some(pid)) => agent_member(state, pid, me),
            _ => json!({ "kind": "system", "id": m.member, "role": m.role, "is_me": false }),
        })
        .collect()
}

/// `GET /asp/chats/{id}/members` (`chat.read`) — who is in one of the agent's chats.
pub async fn members(
    State(state): State<AppState>,
    me: AgentPrincipal,
    Path(chat_id): Path<String>,
) -> Response {
    if !me.agent.allows("chat.read") {
        return scope_refused("reading");
    }
    let Some(chat) = crate::outside::chat_json_for(&state, &me.project, &chat_id) else {
        return api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "not a chat of this agent",
        );
    };
    let members = members_of(&state, &me.project, &chat);
    Json(json!({ "chat": chat, "members": members })).into_response()
}

// ---- context bundle ---------------------------------------------------------------------------------

fn sender_json(state: &AppState, me: &Project, chat: &Value, m: &MessageView) -> Value {
    let raw = m.rest["sender"].as_str().unwrap_or("");
    if let Some(uid) = raw.strip_prefix("user:") {
        let mut v = user_member(state, uid, "member");
        if let Some(n) = m.rest["senderName"].as_str().filter(|s| !s.is_empty()) {
            v["name"] = json!(n);
        }
        if let Some(h) = m.rest["senderHandle"].as_str().filter(|s| !s.is_empty()) {
            v["handle"] = json!(h);
        }
        return json!({ "kind": "user", "id": uid, "name": v["name"], "handle": v["handle"] });
    }
    if let Some(pid) = raw.strip_prefix("agent:") {
        let a = agent_member(state, pid, me);
        return json!({ "kind": "agent", "id": pid, "name": a["name"], "handle": a["handle"] });
    }
    // Project (1:1) chats carry no sender field: the owner writes, the agent answers.
    if chat["project_id"].as_str().is_some() {
        return match m.role.as_str() {
            "assistant" => {
                json!({ "kind": "agent", "id": me.id, "name": me.name, "handle": people::agent_slug(&me.name) })
            }
            _ => {
                let o = user_member(state, &me.user_id, "owner");
                json!({ "kind": "user", "id": me.user_id, "name": o["name"], "handle": o["handle"] })
            }
        };
    }
    json!({ "kind": "system" })
}

/// One row in the `message.received.message` shape (#2182's history rows carry the same fields
/// plus `attachments[]`; once both branches are on main this calls its shared builder).
pub fn row_json(state: &AppState, me: &Project, chat: &Value, m: &MessageView) -> Value {
    let reply_to = m.rest["replyTo"]
        .as_str()
        .or_else(|| m.rest["inReplyTo"].as_str())
        .filter(|s| !s.is_empty());
    json!({
        "id": m.id,
        "ts": m.rest["createdAt"].as_i64(),
        "text": m.text(),
        "from": sender_json(state, me, chat, m),
        "reply_to": reply_to,
        "mentions": m.rest["mentions"].as_array().cloned().unwrap_or_default(),
    })
}

/// Up to `n` finished rows before `before` (oldest first) plus the members: what a sidecar — or a
/// bot with no mirror — needs to answer one wake without a second round trip.
pub fn context_for(
    state: &AppState,
    me: &Project,
    chat: &Value,
    before: Option<&str>,
    n: usize,
) -> Value {
    let chat_id = chat["id"].as_str().unwrap_or_default();
    let chat_ref = match chat["project_id"].as_str() {
        Some(pid) => ChatRef::project(pid, chat_id),
        None => ChatRef::people(chat_id),
    };
    let rows = crate::matrix::backfill::load_doc(state, chat_id, chat_ref.project_id.as_deref())
        .ok()
        .flatten()
        .map(|doc| doc.messages())
        .unwrap_or_default();
    let rows: Vec<MessageView> = rows
        .into_iter()
        .filter(|m| matches!(m.role.as_str(), "user" | "assistant" | "project"))
        .filter(|m| !matches!(m.status.as_str(), "queued" | "streaming"))
        .filter(|m| m.has_visible_content())
        .collect();
    let end = before
        .and_then(|b| rows.iter().position(|m| m.id == b))
        .unwrap_or(rows.len());
    let start = end.saturating_sub(n.min(CONTEXT_MAX));
    let recent: Vec<Value> = rows[start..end]
        .iter()
        .map(|m| row_json(state, me, chat, m))
        .collect();
    json!({ "recent": recent, "members": members_of(state, me, chat) })
}

/// Decorate the wake events of an inbox page with `context` when the agent asked for `n > 0`
/// rows. The callers (`inbox`, `socket`) already refuse agents without `chat.read`. Other
/// events pass through untouched.
pub fn attach_context(state: &AppState, me: &Project, events: &mut [(i64, Value)], n: usize) {
    if n == 0 {
        return;
    }
    for (_, e) in events.iter_mut() {
        let wakes = e["wake"].as_bool() == Some(true) || e["type"] == "member.added";
        if !wakes || e["chat"]["id"].as_str().is_none() {
            continue;
        }
        let chat = e["chat"].clone();
        let before = e["message"]["id"].as_str().map(str::to_owned);
        e["context"] = context_for(state, me, &chat, before.as_deref(), n);
    }
}

// ---- public texts -----------------------------------------------------------------------------------

fn relay_of(state: &AppState) -> String {
    state.cfg.public_url.trim_end_matches('/').to_owned()
}

/// `GET /llms.txt` and `GET /agents/llms.txt` — the instructions a person pastes into their agent.
pub async fn llms_txt(State(state): State<AppState>) -> Response {
    let body = LLMS_TXT.replace(CANONICAL_RELAY, &relay_of(&state));
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        body,
    )
        .into_response()
}

/// `GET /sidecar/install.sh` — `curl -fsSL …/sidecar/install.sh | sh -s -- …`.
pub async fn install_sh(State(state): State<AppState>) -> Response {
    let body = INSTALL_SH.replace(CANONICAL_RELAY, &relay_of(&state));
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/x-shellscript; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_name_the_relay_and_no_secret() {
        assert!(LLMS_TXT.contains("/sidecar/install.sh"));
        assert!(LLMS_TXT.contains("ask your user"));
        // The instructions describe the key's shape and never carry a real one.
        assert!(!LLMS_TXT.contains("arbk_7") && !LLMS_TXT.contains("arbk_0"));
        assert!(INSTALL_SH.starts_with("#!/bin/sh"));
        assert!(INSTALL_SH.contains("\" init --relay"));
        assert!(INSTALL_SH.contains("install-service"));
        // The installer never echoes the key.
        assert!(!INSTALL_SH.contains("echo \"$KEY\""));
        assert!(!INSTALL_SH.contains("echo $KEY"));
    }

    /// A group with two people and the agent: the members endpoint names everyone with
    /// `is_me` on the agent; a wake event asked for with `context=3` carries the rows before it
    /// and the members; a reaction by the agent lands on the row; `context=0` leaves events as
    /// they were.
    #[tokio::test]
    async fn members_context_and_react() {
        use crate::notify::Sender;
        use crate::outside::{apply_event, mint_key, on_announce, AgentPrincipal};
        use crate::people::tests_support::{group, person, project_of};
        use crate::state::test_support::test_state;
        use crate::state::AuthMode;
        use arbos_core::auth::hash;
        use arbos_core::doc::UserMessageOpts;
        use arbos_core::ids::ulid;

        let state = test_state(AuthMode::Github);
        let (owner, _) = person(&state, 7201);
        let (other, _) = person(&state, 7202);
        let pid = project_of(&state, &owner, "Side Bot");
        let key = mint_key();
        state
            .db
            .insert_outside_agent(&pid, &owner, "custom", &hash(&key))
            .unwrap();
        let g = group(&state, &[&owner, &other]);
        state
            .db
            .add_member(&g, &format!("agent:{pid}"), "agent", &owner, &json!({}))
            .unwrap();
        let project = state.db.project(&pid).unwrap().unwrap();
        let chat = crate::outside::chat_json_for(&state, &project, &g).unwrap();

        // Members: two people + the agent, the agent flagged as me.
        let members = members_of(&state, &project, &chat);
        assert_eq!(members.len(), 3);
        let me = members.iter().find(|m| m["is_me"] == true).unwrap();
        assert_eq!(me["kind"], "agent");
        assert_eq!(me["handle"], "side-bot");
        assert!(members
            .iter()
            .any(|m| m["kind"] == "user" && m["id"] == json!(owner) && m["role"] == "owner"));

        // Three rows in the doc, the last one mentions the agent.
        let ids: Vec<String> = (0..3).map(|_| ulid()).collect();
        let texts = [
            "morning all",
            "who has the numbers?",
            "@side-bot total please",
        ];
        for (i, id) in ids.iter().enumerate() {
            let (id2, text, from) = (id.clone(), texts[i].to_owned(), other.clone());
            crate::notices::write_doc(&state, None, &g, move |doc| {
                doc.append_user_message(
                    &id2,
                    &text,
                    "dev:test",
                    &UserMessageOpts {
                        sender: Some(format!("user:{from}")),
                        sender_name: Some("Other".into()),
                        ..Default::default()
                    },
                )?;
                Ok(())
            })
            .await
            .unwrap();
        }
        let sender = Sender::user(&state, &other);
        on_announce(
            &state,
            &ChatRef::people(&g),
            &sender,
            &ids[2],
            texts[2],
            None,
            &[],
        );
        let mut events = state.db.asp_events_after(&pid, 0, 10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].1["wake"], true);

        // context=0 → untouched; context=3 → the two rows before the wake row + members.
        attach_context(&state, &project, &mut events, 0);
        assert!(events[0].1.get("context").is_none());
        attach_context(&state, &project, &mut events, 3);
        let ctx = &events[0].1["context"];
        let recent = ctx["recent"].as_array().unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0]["text"], "morning all");
        assert_eq!(recent[1]["text"], "who has the numbers?");
        assert_eq!(recent[1]["from"]["kind"], "user");
        assert_eq!(recent[1]["from"]["name"], "Other");
        assert_eq!(ctx["members"].as_array().unwrap().len(), 3);

        // The agent reacts to the mention; the reaction sits on the row under its sender key.
        let agent = state
            .db
            .outside_agent_by_key_hash(&hash(&key))
            .unwrap()
            .unwrap();
        let me = AgentPrincipal {
            agent,
            project: project.clone(),
        };
        let r = apply_event(
            &state,
            &me,
            &json!({ "type": "message.react", "chat": g, "message": ids[2], "emoji": "👍" }),
        )
        .await
        .unwrap();
        assert_eq!(r["ok"], true);
        let doc = crate::matrix::backfill::load_doc(&state, &g, None)
            .unwrap()
            .unwrap();
        let row = doc.message(&ids[2]).unwrap();
        let reactions = row.rest["reactions"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        assert!(
            reactions
                .keys()
                .any(|k| k.starts_with("👍") && k.ends_with(&format!("agent:{pid}"))),
            "{reactions:?}"
        );
        // Unknown message → an error, not a write.
        let bad = apply_event(
            &state,
            &me,
            &json!({ "type": "message.react", "chat": g, "message": "nope", "emoji": "👍" }),
        )
        .await;
        assert!(bad.is_err());
    }

    /// Jacob (16:32 UTC): the relay must add no delay. An inbox long-poll parked with nothing
    /// queued answers within milliseconds of the event being stored — and an event stored in the
    /// gap between the table read and the wait is not missed (the Notify is enabled first).
    #[tokio::test]
    async fn inbox_long_poll_returns_at_once_on_a_new_event() {
        use crate::outside::{deliver_for_test, inbox, mint_key, AgentPrincipal, InboxQuery};
        use crate::people::tests_support::{person, project_of};
        use crate::state::test_support::test_state;
        use crate::state::AuthMode;
        use arbos_core::auth::hash;
        use axum::body::to_bytes;
        use axum::extract::{Query, State};

        let state = test_state(AuthMode::Github);
        let (owner, _) = person(&state, 7301);
        let pid = project_of(&state, &owner, "Fast Bot");
        let key = mint_key();
        state
            .db
            .insert_outside_agent(&pid, &owner, "custom", &hash(&key))
            .unwrap();
        let agent = state
            .db
            .outside_agent_by_key_hash(&hash(&key))
            .unwrap()
            .unwrap();
        let project = state.db.project(&pid).unwrap().unwrap();
        let me = AgentPrincipal { agent, project };
        let q = InboxQuery {
            since: 0,
            wait: 25,
            limit: None,
            context: 0,
        };
        let s2 = state.clone();
        let waiter = tokio::spawn(async move { inbox(State(s2), me, Query(q)).await });
        // Let the call park, then store an event and time the answer.
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        let t = std::time::Instant::now();
        deliver_for_test(
            &state,
            &pid,
            json!({ "type": "message.received", "chat": { "id": "c1" }, "wake": true }),
        );
        let resp = tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
            .await
            .expect("the long-poll answered within 2 s")
            .unwrap();
        let took = t.elapsed();
        assert!(
            took < std::time::Duration::from_millis(500),
            "took {took:?}"
        );
        let body = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["events"].as_array().unwrap().len(), 1);
        assert_eq!(v["next"], 1);
    }
}
