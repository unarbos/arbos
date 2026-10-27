//! Secret hand-off in chat (Jacob, 2026-10-26): "can I give you a secret" → a card with a
//! masked field whose value goes from the device straight into the vault; "save this" on a
//! pasted key → the relay reads the row once, encrypts it, rewrites the row into the
//! "Secret saved as NAME" ghost and every holder scrubs its copies. The agent only ever
//! handles **names**: no value crosses a tool call, a card payload or a tool result here.
//!
//! Three entry points:
//! - [`request_secret`] — the `request_secret` tool posts the request card (`payload.kind =
//!   secret_offer`, no value field) and waits; the client saves through `POST /secrets` and
//!   answers the card with `approve { secret: { id, name, env } }`.
//! - [`save_from_message`] — the `secrets_save_from_message` tool (and the fast path in
//!   [`on_user_message`]) calls `POST /box/secrets/from-message`, then scrubs this box.
//! - [`on_user_message`] — runs before the model sees a user message: "save this" with a
//!   key-like token saves at once (the model gets the name, never the key); a key without
//!   those words gets the offer card (`secret_detected`, A save / B leave) and the turn goes on.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{json, Value};
use tokio::sync::oneshot;

use arbos_core::approval::{ApprovalOutcome, ApprovalRequest, Risk};
use arbos_core::redact::{asks_to_save, find_key_like, key_hint, KeyLike};

use crate::agent::Agent;
use crate::approval::WaitMode;
use crate::mcp::{err_result, text_result};
use crate::vault::{relay_post, scrub_now, wait_budget};

/// `payload.kind` of the request card (masked field on the client; the agent waits).
pub const OFFER_KIND: &str = "secret_offer";
/// `payload.kind` of the "that looks like a key" card (A save / B leave; optional, no wait).
pub const DETECTED_KIND: &str = "secret_detected";

/// Card answers that carried a saved secret: approval id → `{ id, name, env }`.
fn answers() -> &'static Mutex<BTreeMap<String, Value>> {
    static S: OnceLock<Mutex<BTreeMap<String, Value>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// An `approve` command's payload named a saved secret (`secret: { id, name, env }`): keep it
/// for the tool waiting on that card. Called before the waiter is woken.
pub fn note_card_answer(approval_id: &str, payload: &Value) {
    if let Some(s) = payload.get("secret").filter(|s| s.is_object()) {
        // Names and ids only: a client that put anything else in here is ignored.
        let clean = json!({
            "id": s.get("id").and_then(Value::as_str).unwrap_or(""),
            "name": s.get("name").and_then(Value::as_str).unwrap_or(""),
            "env": s.get("env").and_then(Value::as_str).unwrap_or(""),
        });
        answers()
            .lock()
            .expect("handoff answers")
            .insert(approval_id.to_owned(), clean);
    }
}

fn take_card_answer(approval_id: &str) -> Option<Value> {
    answers()
        .lock()
        .expect("handoff answers")
        .remove(approval_id)
}

pub fn tool_specs() -> Vec<Value> {
    vec![
        json!({ "name": "request_secret",
            "description": "Ask the user for a secret (an API key, a token, a password) you do not have. Posts a card with a masked field; the value goes from their device straight into their vault and never through this chat or you. name = the vault name to suggest (UPPER_SNAKE, e.g. STRIPE_KEY; they can edit it), purpose = one short sentence shown on the card. Waits for the answer and returns the name you were given access to — use it as $NAME or arbos-secret://NAME/value through egress. Never ask the user to paste a secret into the chat.",
            "inputSchema": { "type": "object", "properties": {
                "name": { "type": "string", "description": "suggested vault name, UPPER_SNAKE" },
                "purpose": { "type": "string", "description": "one sentence shown on the card" } },
                "required": ["name", "purpose"] } }),
        json!({ "name": "secrets_save_from_message",
            "description": "The user pasted a key into the chat and asked you to save it (\"save this\"): move it into their vault under a name and remove it from the chat, your context and your logs. Do not copy the value into this call — give message_id (the message that holds it; omit for the latest one that does), name (UPPER_SNAKE; omit to use the key's suggested name), purpose (one sentence, becomes the note), and hint = first 4 + '…' + last 4 characters only when the message holds several keys. Returns the name; the message becomes \"Secret saved as NAME\" for everyone.",
            "inputSchema": { "type": "object", "properties": {
                "message_id": { "type": "string" },
                "name": { "type": "string" },
                "purpose": { "type": "string" },
                "hint": { "type": "string", "description": "sk_l…3xA1 — never the value" } } } }),
    ]
}

/// Uppercase `_` form of a suggested name; empty when nothing usable is left.
fn env_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_uppercase());
        } else if matches!(c, '_' | '-' | ' ' | '.') && !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    out.trim_end_matches('_').chars().take(64).collect()
}

/// The `request_secret` tool: post the card, wait for the device to save, return the name.
pub async fn request_secret(agent: &Arc<Agent>, args: &Value, chat_id: &str) -> Value {
    let name = env_name(args["name"].as_str().unwrap_or(""));
    let purpose = args["purpose"]
        .as_str()
        .unwrap_or("")
        .trim()
        .chars()
        .take(300)
        .collect::<String>();
    if name.is_empty() {
        return err_result("name is required (UPPER_SNAKE, e.g. STRIPE_KEY)");
    }
    let agent_name = agent.cfg.project_name.clone();
    let (tx, rx) = oneshot::channel();
    let req = ApprovalRequest {
        title: format!("Give {agent_name} a secret"),
        summary: purpose.clone(),
        risk: Risk::Credential,
        payload: json!({ "kind": OFFER_KIND, "name": name, "purpose": purpose, "agent": agent_name,
            "projectId": agent.cfg.project_id, "chatId": chat_id,
            "answers": [] }),
    };
    let approval_id = match agent
        .begin_approval(chat_id, req, WaitMode::Inline, Some(tx))
        .await
    {
        Ok(id) => id,
        Err(e) => return err_result(format!("the card could not be posted: {e}")),
    };
    let _ = agent.store.t3_append(
        chat_id,
        None,
        Some(chat_id),
        "secret_offer",
        Some("assistant"),
        &format!("request_secret {name}: {purpose}"),
        None,
    );
    let outcome = match tokio::time::timeout(wait_budget(), rx).await {
        Ok(Ok(o)) => o,
        Ok(Err(_)) => ApprovalOutcome::Expired,
        Err(_) => {
            let _ = agent
                .decide_approval(
                    &approval_id,
                    ApprovalOutcome::Expired,
                    "arbos:handoff:timeout",
                )
                .await;
            ApprovalOutcome::Expired
        }
    };
    match outcome {
        ApprovalOutcome::Approved => match take_card_answer(&approval_id) {
            Some(saved) if saved["env"].as_str().is_some_and(|e| !e.is_empty()) => {
                let env = saved["env"].as_str().unwrap_or("").to_owned();
                let _ = crate::vault::refresh_all(agent).await;
                agent.append_audit(
                    "secret_handoff",
                    json!({ "path": "card", "env": env, "chat_id": chat_id, "approval_id": approval_id }),
                );
                text_result(json!({ "saved": true, "name": saved["name"], "env": env,
                    "how": format!("You have access. Use it as ${env} in a shell or as arbos-secret://{env}/value through egress; the value never appears to you — do not ask for it or echo it.") }).to_string())
            }
            _ => text_result(json!({ "saved": false,
                "reason": "the card was acknowledged but no secret arrived — the user's app is too old to save from the card; ask them to add it in Settings → Secrets and give you access" }).to_string()),
        },
        ApprovalOutcome::Rejected { .. } => text_result(
            json!({ "saved": false, "declined": true, "reason": "the user closed the card without saving; do not ask again unless they offer" }).to_string(),
        ),
        ApprovalOutcome::Expired => text_result(
            json!({ "saved": false, "expired": true, "reason": "no answer in time; the user can still add it in Settings → Secrets" }).to_string(),
        ),
    }
}

/// What the relay answered for a save-from-message (names only).
pub struct Saved {
    pub secret_id: String,
    pub name: String,
    pub env: String,
}

/// `POST /box/secrets/from-message`, then scrub this box (the relay also pushes
/// `secret_scrub` to every box of the owner; doing it here too makes the agent's own
/// session clean before the turn goes on).
pub async fn save_from_message(
    agent: &Arc<Agent>,
    chat_id: &str,
    message_id: &str,
    people_chat: bool,
    name: Option<&str>,
    purpose: Option<&str>,
    hint: Option<&str>,
) -> Result<Saved, String> {
    let body = json!({
        "chat_id": chat_id, "message_id": message_id, "people_chat": people_chat,
        "name": name.map(env_name).filter(|n| !n.is_empty()),
        "purpose": purpose, "hint": hint,
    });
    let (status, v) = relay_post(agent, "/box/secrets/from-message", &body).await?;
    if status != 201 {
        let msg = v["error"]["message"]
            .as_str()
            .or(v["message"].as_str())
            .unwrap_or("the relay refused");
        let kind = v["error"]["type"].as_str().unwrap_or("");
        return Err(if kind.is_empty() {
            msg.to_owned()
        } else {
            format!("{kind}: {msg}")
        });
    }
    let saved = Saved {
        secret_id: v["secret"]["id"].as_str().unwrap_or("").to_owned(),
        name: v["secret"]["name"].as_str().unwrap_or("").to_owned(),
        env: v["secret"]["env"].as_str().unwrap_or("").to_owned(),
    };
    if !saved.secret_id.is_empty() {
        scrub_now(agent, &saved.secret_id).await;
    }
    let _ = crate::vault::refresh_all(agent).await;
    agent.append_audit(
        "secret_handoff",
        json!({ "path": "save_this", "env": saved.env, "chat_id": chat_id, "message_id": message_id }),
    );
    Ok(saved)
}

/// The latest user message in the chat that holds something key-like: `(id, text)`.
async fn latest_key_message(agent: &Arc<Agent>, chat_id: &str) -> Option<(String, String)> {
    let host = agent.sync.host(chat_id).await?;
    let h = host.lock().await;
    h.doc
        .messages()
        .into_iter()
        .rev()
        .filter(|m| m.role == "user")
        .map(|m| (m.id.clone(), m.text()))
        .find(|(_, t)| !find_key_like(t).is_empty())
}

/// Is this chat a people chat (the project is a member, not the owner)?
async fn is_people_chat(agent: &Arc<Agent>, chat_id: &str) -> bool {
    match agent.sync.host(chat_id).await {
        Some(h) => h.lock().await.room.starts_with("people/"),
        None => false,
    }
}

/// The `secrets_save_from_message` tool.
pub async fn secrets_save_from_message(agent: &Arc<Agent>, args: &Value, chat_id: &str) -> Value {
    let wanted = args["message_id"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let (message_id, text) = match wanted {
        Some(id) => {
            let Some(host) = agent.sync.host(chat_id).await else {
                return err_result("this chat has no doc");
            };
            let text = {
                let h = host.lock().await;
                h.doc.message(&id).map(|m| m.text())
            };
            match text {
                Some(t) => (id, t),
                None => return err_result("no such message in this chat"),
            }
        }
        None => match latest_key_message(agent, chat_id).await {
            Some(x) => x,
            None => {
                return err_result(
                    "no message in this chat holds anything that looks like a key; ask the user to paste it or use request_secret",
                )
            }
        },
    };
    // The hint the relay needs when the message has several keys: computed here from the
    // doc, so the model never has to write any part of the value into a tool call.
    let hint = args["hint"]
        .as_str()
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            let found = find_key_like(&text);
            let confident: Vec<&KeyLike> = found.iter().filter(|k| k.confident).collect();
            match (found.len(), confident.len()) {
                (1, _) => None,
                (_, 1) => Some(key_hint(confident[0].value(&text))),
                _ => None,
            }
        });
    let people = is_people_chat(agent, chat_id).await;
    match save_from_message(
        agent,
        chat_id,
        &message_id,
        people,
        args["name"].as_str(),
        args["purpose"].as_str(),
        hint.as_deref(),
    )
    .await
    {
        Ok(s) => text_result(json!({ "saved": true, "name": s.name, "env": s.env,
            "message": "removed from the chat on every device and from your context; the row now reads \"Secret saved as NAME\"",
            "how": format!("Use it as ${} in a shell or as arbos-secret://{}/value through egress; never echo the value.", s.env, s.env) }).to_string()),
        Err(e) => err_result(e),
    }
}

/// What [`on_user_message`] did with a user message before the model sees it.
pub enum Handled {
    /// Nothing key-like, or the save failed: the text goes to the model as it is.
    Nothing,
    /// "save this" fast path: the key is in the vault, the row is the ghost; the model gets
    /// this text instead (the name, the words around the key, never the key).
    Saved(String),
    /// A key without "save this": the offer card is up; the model gets the text plus a note.
    Offered(String),
}

/// Runs in `handle_send` before the prompt is built.
pub async fn on_user_message(
    agent: &Arc<Agent>,
    chat_id: &str,
    message_id: &str,
    text: &str,
) -> Handled {
    let found = find_key_like(text);
    let confident: Vec<&KeyLike> = found.iter().filter(|k| k.confident).collect();
    if confident.is_empty() {
        return Handled::Nothing;
    }
    let people = is_people_chat(agent, chat_id).await;
    let k = confident[0];
    let hint = (confident.len() > 1).then(|| key_hint(k.value(text)));
    if asks_to_save(text) {
        match save_from_message(
            agent,
            chat_id,
            message_id,
            people,
            None,
            None,
            hint.as_deref(),
        )
        .await
        {
            Ok(s) => {
                let rest = text.replace(k.value(text), &format!("[saved as {}]", s.env));
                return Handled::Saved(format!(
                    "[Arbos saved the key from this message into the user's vault as {env} and removed it from the chat (the message now reads \"Secret saved as {env}\"). You have access: use it as ${env} or arbos-secret://{env}/value through egress. Never ask for or repeat the value.]\n{rest}",
                    env = s.env
                ));
            }
            Err(e) => {
                tracing::warn!(error = %e, chat_id, "secret hand-off: fast save failed; the model sees the message");
                return Handled::Nothing;
            }
        }
    }
    // A key without the words: offer, never auto-save (Jacob's default). The card is optional
    // — it does not block the composer or the turn.
    let name = k.suggested_name;
    let agent_name = agent.cfg.project_name.clone();
    let req = ApprovalRequest {
        title: format!(
            "That looks like a key. Save it to your vault as {name} and remove it from this chat?"
        ),
        summary: "Nothing is saved until you choose.".into(),
        risk: Risk::Credential,
        payload: json!({ "kind": DETECTED_KIND, "optional": true, "name": name, "agent": agent_name,
            "chatId": chat_id, "messageId": message_id, "hint": hint, "peopleChat": people,
            "answers": [
                { "id": "save", "label": format!("Save as {name} and remove") },
                { "id": "deny", "label": "Leave it here" },
            ] }),
    };
    if let Err(e) = agent
        .begin_approval(chat_id, req, WaitMode::Deferred, None)
        .await
    {
        tracing::warn!(error = %e, chat_id, "secret hand-off: offer card could not be posted");
        return Handled::Nothing;
    }
    Handled::Offered(format!(
        "{text}\n\n[Arbos offered the user a card to save the pasted key as {name} and remove it from the chat. Do not repeat the key in your reply; if they pick Save you will have it as ${name}.]"
    ))
}

/// A `secret_detected` card was answered. Save → `save_from_message` with what the card
/// carried; leave → nothing.
pub async fn on_detected_decided(agent: &Arc<Agent>, payload: &Value, approved: bool) {
    if !approved {
        return;
    }
    let chat_id = payload["chatId"].as_str().unwrap_or("");
    let message_id = payload["messageId"].as_str().unwrap_or("");
    if chat_id.is_empty() || message_id.is_empty() {
        return;
    }
    let r = save_from_message(
        agent,
        chat_id,
        message_id,
        payload["peopleChat"].as_bool().unwrap_or(false),
        payload["name"].as_str(),
        None,
        payload["hint"].as_str(),
    )
    .await;
    if let Err(e) = r {
        tracing::warn!(error = %e, chat_id, "secret hand-off: save from the offer card failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::tests::test_agent_with;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use std::sync::atomic::{AtomicUsize, Ordering};

    const KEY: &str = concat!(
        "sk_",
        "live_51HxQ9mT3vK2pLnR8wZ4eYc7bD1fG6hJ0kM9nP5qS2tU8vW3xA1"
    );

    /// A relay stub for the box secrets routes: records every from-message body, hands the
    /// value out once for the scrub, answers empty lists for the refreshes.
    fn relay_stub() -> (Router, Arc<Mutex<Vec<Value>>>, Arc<AtomicUsize>) {
        let bodies: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let scrubs = Arc::new(AtomicUsize::new(0));
        let b2 = bodies.clone();
        let s2 = scrubs.clone();
        let router = Router::new()
            .route(
                "/box/secrets/from-message",
                post(move |Json(b): Json<Value>| {
                    let b2 = b2.clone();
                    async move {
                        b2.lock().unwrap().push(b.clone());
                        let name = b["name"].as_str().filter(|n| !n.is_empty()).unwrap_or("STRIPE_KEY");
                        (
                            axum::http::StatusCode::CREATED,
                            Json(json!({ "secret": { "id": "sec_handoff", "name": name, "env": name,
                                "source": { "kind": "chat" } }, "message_id": b["message_id"], "redacted": true })),
                        )
                    }
                }),
            )
            .route(
                "/box/secrets/{id}/scrub",
                get(move || {
                    let s2 = s2.clone();
                    async move {
                        s2.fetch_add(1, Ordering::SeqCst);
                        Json(json!({ "env": "STRIPE_KEY", "values": [KEY] }))
                    }
                }),
            )
            .route("/box/secrets", get(|| async { Json(json!({ "paused": false, "items": [] })) }))
            .route("/box/secrets/env", get(|| async { Json(json!({ "paused": false, "items": [] })) }));
        (router, bodies, scrubs)
    }

    fn holds(bytes: &[u8]) -> bool {
        bytes.windows(KEY.len()).any(|w| w == KEY.as_bytes())
    }

    /// Every file under the box's state dir that still holds the key.
    fn files_holding(root: &std::path::Path) -> Vec<String> {
        let mut out = Vec::new();
        fn walk(p: &std::path::Path, out: &mut Vec<String>) {
            let Ok(rd) = std::fs::read_dir(p) else { return };
            for e in rd.flatten() {
                let path = e.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if let Ok(bytes) = std::fs::read(&path) {
                    if holds(&bytes) {
                        out.push(path.display().to_string());
                    }
                }
            }
        }
        walk(root, &mut out);
        out
    }

    /// `ARBOS_HANDOFF_KEEP=<dir>` (scripts/secrets/handoff-invariant-check.sh): keep the box's
    /// whole state dir and a TRACE journal of the run there, for the grep that follows.
    fn keep_dir() -> Option<std::path::PathBuf> {
        std::env::var_os("ARBOS_HANDOFF_KEEP").map(std::path::PathBuf::from)
    }

    fn journal_guard(
        keep: &Option<std::path::PathBuf>,
    ) -> Option<tracing::subscriber::DefaultGuard> {
        let dir = keep.as_ref()?;
        std::fs::create_dir_all(dir).ok()?;
        let file = std::fs::File::create(dir.join("agentd-journal.log")).ok()?;
        let sub = tracing_subscriber::fmt()
            .with_writer(Mutex::new(file))
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .finish();
        Some(tracing::subscriber::set_default(sub))
    }

    fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
        let _ = std::fs::create_dir_all(to);
        if let Ok(rd) = std::fs::read_dir(from) {
            for e in rd.flatten() {
                let p = e.path();
                let dest = to.join(e.file_name());
                if p.is_dir() {
                    copy_tree(&p, &dest);
                } else {
                    let _ = std::fs::copy(&p, &dest);
                }
            }
        }
    }

    #[tokio::test]
    async fn save_this_goes_to_the_vault_before_the_model_and_leaves_no_copy_in_the_box() {
        let keep = keep_dir();
        let _journal = journal_guard(&keep);
        let (router, bodies, scrubs) = relay_stub();
        let (agent, root) = test_agent_with(router).await;
        let chat = "01TESTCHATHANDOFF000000000";
        agent.store.upsert_chat(chat, "Main", true, 0).unwrap();
        let host = agent.sync.open_room(chat).await.unwrap();
        let text = format!("here is the stripe key {KEY} save this");
        let msg_id = {
            let h = host.lock().await;
            let id = arbos_core::ids::ulid();
            h.doc
                .append_user_message(
                    &id,
                    &text,
                    "dev-mac",
                    &arbos_core::doc::UserMessageOpts::default(),
                )
                .unwrap();
            id
        };
        // The key sits in the doc and in T3 the way a real turn would have left it — and in
        // the places a harness that already saw it would have written: its session file, its
        // log, a subagent pack, durable memory (memory_add runs the pattern redaction itself,
        // so the memory row proves that layer too).
        let _ = agent
            .store
            .t3_append(chat, None, Some(chat), "user", Some("user"), &text, None);
        let _ = agent.store.memory_add(
            "durable",
            Some(chat),
            "note",
            "chat",
            &format!("the stripe key is {KEY}"),
            None,
            None,
            0.9,
            3,
            &[],
        );
        for (dir, file) in [
            (agent.cfg.pi_sessions_dir(), "20261026_main.jsonl"),
            (agent.cfg.state_dir.join("logs"), "pi-main.log"),
            (agent.cfg.state_dir.join("packs"), "sub1.md"),
        ] {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(file), format!("… {text} …\n")).unwrap();
        }
        agent.sync.persist(&host).await.unwrap();
        assert!(
            !files_holding(&root).is_empty(),
            "setup: the box holds the key"
        );

        let handled = on_user_message(&agent, chat, &msg_id, &text).await;
        let prompt = match handled {
            Handled::Saved(t) => t,
            Handled::Offered(_) => panic!("save this must save, not offer"),
            Handled::Nothing => panic!("nothing happened"),
        };
        assert!(
            prompt.contains("STRIPE_KEY") && prompt.contains("save this"),
            "{prompt}"
        );
        assert!(
            !prompt.contains(KEY),
            "the model's prompt never carries the key: {prompt}"
        );

        // the relay saw the message id and no value; the box fetched the value once to scrub
        let bodies = bodies.lock().unwrap().clone();
        assert_eq!(bodies.len(), 1, "{bodies:?}");
        assert_eq!(bodies[0]["message_id"], msg_id);
        assert_eq!(bodies[0]["people_chat"], false);
        assert!(bodies[0]["hint"].is_null(), "one key → no hint needed");
        assert!(!bodies[0].to_string().contains(KEY));
        assert_eq!(scrubs.load(Ordering::SeqCst), 1);

        // T3 + FTS, the room doc and every file under the state dir are clean
        let hits = agent
            .store
            .t3_search("sk_live", Some(chat), None, 10, 0)
            .unwrap();
        assert!(
            hits.iter().all(|(r, _, _)| !r.text.contains(KEY)),
            "T3 still holds it"
        );
        {
            let h = host.lock().await;
            let m = h.doc.message(&msg_id).unwrap();
            assert!(!m.text().contains(KEY));
            assert!(h.doc.is_shallow_mode());
        }
        agent.sync.persist(&host).await.unwrap();
        let _ = agent.store.flush_after_scrub();
        let left = files_holding(&root);
        assert!(left.is_empty(), "files still holding the key: {left:?}");
        if let Some(dir) = keep {
            copy_tree(&root, &dir.join("box"));
            let _ = std::fs::write(
                dir.join("README.txt"),
                "box work root after the save-this fast path (<project>/.arbos: agentd.db + wal, pi/, logs/, …) + agentd-journal.log (TRACE)\n",
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_pasted_key_without_the_words_gets_an_optional_offer_card() {
        let (router, bodies, _) = relay_stub();
        let (agent, root) = test_agent_with(router).await;
        let chat = "01TESTCHATHANDOFF000000001";
        agent.store.upsert_chat(chat, "Main", true, 0).unwrap();
        let host = agent.sync.open_room(chat).await.unwrap();
        let text = format!("fyi {KEY}");
        let msg_id = {
            let h = host.lock().await;
            let id = arbos_core::ids::ulid();
            h.doc
                .append_user_message(
                    &id,
                    &text,
                    "dev-mac",
                    &arbos_core::doc::UserMessageOpts::default(),
                )
                .unwrap();
            id
        };
        let Handled::Offered(prompt) = on_user_message(&agent, chat, &msg_id, &text).await else {
            panic!("a key without 'save this' must offer, never save");
        };
        assert!(prompt.contains("offered the user a card"), "{prompt}");
        assert!(bodies.lock().unwrap().is_empty(), "nothing saved yet");
        // the card: optional (no composer lock), two answers, names only
        let (card, msg) = {
            let h = host.lock().await;
            let msgs = h.doc.messages();
            let m = msgs
                .iter()
                .rev()
                .find(|m| m.parts.iter().any(|p| p["kind"] == "approval"))
                .cloned()
                .unwrap();
            let p = m
                .parts
                .iter()
                .find(|p| p["kind"] == "approval")
                .cloned()
                .unwrap();
            (p, m)
        };
        assert_eq!(card["payload"]["kind"], DETECTED_KIND);
        assert_eq!(card["payload"]["optional"], true);
        assert_eq!(card["payload"]["name"], "STRIPE_KEY");
        assert_eq!(card["payload"]["answers"][0]["id"], "save");
        assert_eq!(card["payload"]["answers"][1]["id"], "deny");
        assert!(!card.to_string().contains(KEY));
        assert!(
            msg.rest.get("waitingApproval").is_none(),
            "an optional card does not lock the composer"
        );
        // plain prose: nothing
        assert!(matches!(
            on_user_message(&agent, chat, "x", "run the report please").await,
            Handled::Nothing
        ));

        // Save on the card → the save runs with what the card carried
        let approval_id = card["approvalId"].as_str().unwrap().to_owned();
        agent
            .decide_approval_from_command(
                &approval_id,
                ApprovalOutcome::Approved,
                "user:test",
                &json!({ "approvalId": approval_id }),
            )
            .await
            .unwrap();
        for _ in 0..50 {
            if !bodies.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let bodies = bodies.lock().unwrap().clone();
        assert_eq!(bodies.len(), 1, "{bodies:?}");
        assert_eq!(bodies[0]["message_id"], msg_id);
        assert_eq!(bodies[0]["name"], "STRIPE_KEY");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn the_request_card_waits_for_the_device_and_returns_the_name_only() {
        let keep = keep_dir();
        let _journal = journal_guard(&keep);
        let (router, _, _) = relay_stub();
        let (agent, root) = test_agent_with(router).await;
        let chat = "01TESTCHATHANDOFF000000002";
        agent.store.upsert_chat(chat, "Main", true, 0).unwrap();
        let host = agent.sync.open_room(chat).await.unwrap();
        let a2 = agent.clone();
        let tool = tokio::spawn(async move {
            request_secret(
                &a2,
                &json!({ "name": "stripe key", "purpose": "the refunds report" }),
                chat,
            )
            .await
        });
        // the card appears with no value field and the suggested name
        let mut approval_id = None;
        for _ in 0..100 {
            let h = host.lock().await;
            if let Some(p) = h
                .doc
                .messages()
                .iter()
                .rev()
                .flat_map(|m| m.parts.clone())
                .find(|p| p["kind"] == "approval")
            {
                approval_id = p["approvalId"].as_str().map(str::to_owned);
                assert_eq!(p["payload"]["kind"], OFFER_KIND);
                assert_eq!(p["payload"]["name"], "STRIPE_KEY");
                assert!(p["payload"].get("value").is_none());
                break;
            }
            drop(h);
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let approval_id = approval_id.expect("the request card was posted");
        // the device saved through POST /secrets and answers the card with the reference
        agent
            .decide_approval_from_command(
                &approval_id,
                ApprovalOutcome::Approved,
                "user:test",
                &json!({ "approvalId": approval_id, "secret": { "id": "sec_1", "name": "STRIPE_KEY", "env": "STRIPE_KEY" } }),
            )
            .await
            .unwrap();
        let out = tool.await.unwrap();
        let text = out["content"][0]["text"].as_str().unwrap();
        let v: Value = serde_json::from_str(text).unwrap();
        assert_eq!(v["saved"], true);
        assert_eq!(v["env"], "STRIPE_KEY");
        assert!(v["how"].as_str().unwrap().contains("$STRIPE_KEY"));
        assert!(!text.contains("sk_"));

        // a dismissed card → declined, nothing saved
        let a2 = agent.clone();
        let tool = tokio::spawn(async move {
            request_secret(&a2, &json!({ "name": "other", "purpose": "x" }), chat).await
        });
        let mut id = None;
        for _ in 0..100 {
            let h = host.lock().await;
            if let Some(p) = h
                .doc
                .messages()
                .iter()
                .rev()
                .flat_map(|m| m.parts.clone())
                .find(|p| p["kind"] == "approval" && p["payload"]["name"] == "OTHER")
            {
                id = p["approvalId"].as_str().map(str::to_owned);
                break;
            }
            drop(h);
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        agent
            .decide_approval(
                &id.unwrap(),
                ApprovalOutcome::Rejected { reason: None },
                "user:test",
            )
            .await
            .unwrap();
        let out = tool.await.unwrap();
        let v: Value = serde_json::from_str(out["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(v["saved"], false);
        assert_eq!(v["declined"], true);
        if let Some(dir) = keep {
            copy_tree(&root, &dir.join("box"));
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn env_names_are_upper_snake() {
        assert_eq!(env_name("stripe key"), "STRIPE_KEY");
        assert_eq!(env_name("Stripe-Live.key"), "STRIPE_LIVE_KEY");
        assert_eq!(env_name("  "), "");
        assert_eq!(env_name("__x__"), "X");
    }

    #[test]
    fn card_answers_keep_names_only() {
        note_card_answer(
            "ap1",
            &json!({ "approvalId": "ap1", "secret": { "id": "s1", "name": "STRIPE_KEY", "env": "STRIPE_KEY", "value": "sk_live_oops" } }),
        );
        let a = take_card_answer("ap1").unwrap();
        assert_eq!(a["env"], "STRIPE_KEY");
        assert!(a.get("value").is_none());
        assert!(take_card_answer("ap1").is_none());
        note_card_answer("ap2", &json!({ "approvalId": "ap2" }));
        assert!(take_card_answer("ap2").is_none());
    }

    #[test]
    fn tool_specs_name_no_value_field() {
        let specs = tool_specs();
        let names: Vec<&str> = specs.iter().map(|s| s["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["request_secret", "secrets_save_from_message"]);
        for s in &specs {
            let props = s["inputSchema"]["properties"].as_object().unwrap();
            assert!(
                !props.contains_key("value") && !props.contains_key("secret"),
                "{s}"
            );
        }
    }
}
