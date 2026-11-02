//! WhatsApp Cloud API adapter: webhook verification (`GET /channels/whatsapp/webhook`), inbound
//! text messages (`POST`), outbound text through `graph.facebook.com`.
//!
//! Env: `ARBOS_WHATSAPP_TOKEN` (system-user access token), `ARBOS_WHATSAPP_PHONE_ID` (phone
//! number id), `ARBOS_WHATSAPP_VERIFY_TOKEN` (your own string, pasted in the Meta dashboard),
//! `ARBOS_WHATSAPP_APP_SECRET` to check `X-Hub-Signature-256`. Without the app secret inbound
//! refuses every payload (503): nothing else proves a `POST` came from Meta. Only
//! `ARBOS_CHANNELS_MOCK=1` takes the signature check off, and then there are no real credentials
//! to abuse.

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;

use super::{handle_inbound, mock_on, Inbound, Platform};
use crate::state::AppState;

const TOKEN_ENV: &str = "ARBOS_WHATSAPP_TOKEN";
const PHONE_ID_ENV: &str = "ARBOS_WHATSAPP_PHONE_ID";
const VERIFY_ENV: &str = "ARBOS_WHATSAPP_VERIFY_TOKEN";
const APP_SECRET_ENV: &str = "ARBOS_WHATSAPP_APP_SECRET";
const GRAPH: &str = "https://graph.facebook.com/v21.0";

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

pub fn status(_state: &AppState) -> (bool, String) {
    let missing: Vec<&str> = [TOKEN_ENV, PHONE_ID_ENV, VERIFY_ENV, APP_SECRET_ENV]
        .into_iter()
        .filter(|k| env(k).is_none())
        .collect();
    if missing.is_empty() {
        (true, "Cloud API webhook + messages".into())
    } else {
        (false, format!("missing {}", missing.join(", ")))
    }
}

/// `GET /channels/whatsapp/webhook?hub.mode=subscribe&hub.verify_token=…&hub.challenge=…`.
pub async fn verify(
    State(state): State<AppState>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let mode = q.get("hub.mode").map(String::as_str);
    let token = q.get("hub.verify_token").map(String::as_str);
    let challenge = q.get("hub.challenge").cloned().unwrap_or_default();
    let expected = env(VERIFY_ENV);
    let ok = mode == Some("subscribe")
        && match expected.as_deref() {
            Some(e) => token == Some(e),
            None => mock_on(&state),
        };
    if ok {
        challenge.into_response()
    } else {
        (StatusCode::FORBIDDEN, "verify token mismatch").into_response()
    }
}

fn signature_ok(secret: &str, body: &[u8], header: &str) -> bool {
    let Some(hex_sig) = header.strip_prefix("sha256=") else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    let expected = hex::encode(mac.finalize().into_bytes());
    expected.len() == hex_sig.len()
        && expected
            .bytes()
            .zip(hex_sig.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// Every text message in one webhook payload (`entry[].changes[].value.messages[]`).
pub fn inbound_from_payload(v: &Value) -> Vec<Inbound> {
    let mut out = Vec::new();
    for entry in v
        .get("entry")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for change in entry
            .get("changes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let value = change.get("value").cloned().unwrap_or(Value::Null);
            let contacts = value
                .get("contacts")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let name_of = |wa_id: &str| {
                contacts
                    .iter()
                    .find(|c| c.get("wa_id").and_then(Value::as_str) == Some(wa_id))
                    .and_then(|c| c["profile"]["name"].as_str())
                    .map(str::to_owned)
            };
            for m in value
                .get("messages")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if m.get("type").and_then(Value::as_str) != Some("text") {
                    continue;
                }
                let Some(from) = m.get("from").and_then(Value::as_str) else {
                    continue;
                };
                let Some(text) = m["text"]["body"].as_str() else {
                    continue;
                };
                let name = name_of(from).unwrap_or_else(|| format!("+{from}"));
                out.push(Inbound {
                    platform: Platform::Whatsapp,
                    // A WhatsApp conversation is identified by the counterpart's number.
                    external_chat_id: from.to_owned(),
                    external_user_id: from.to_owned(),
                    sender_name: name.clone(),
                    text: text.to_owned(),
                    event_key: m
                        .get("id")
                        .and_then(Value::as_str)
                        .map(|id| format!("whatsapp:{id}")),
                    trusted_sender: true,
                    label: format!("WhatsApp · {name}"),
                });
            }
        }
    }
    out
}

/// `POST /channels/whatsapp/webhook`.
pub async fn webhook(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    match env(APP_SECRET_ENV) {
        Some(secret) => {
            let sig = headers
                .get("x-hub-signature-256")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if !signature_ok(&secret, &body, sig) {
                return (StatusCode::UNAUTHORIZED, "bad signature").into_response();
            }
        }
        None if mock_on(&state) => {}
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                format!("{APP_SECRET_ENV} not set"),
            )
                .into_response();
        }
    }
    let v: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    for inb in inbound_from_payload(&v) {
        handle_inbound(&state, inb).await;
    }
    StatusCode::OK.into_response()
}

pub async fn send(state: &AppState, to: &str, text: &str) -> anyhow::Result<Value> {
    let token = env(TOKEN_ENV).ok_or_else(|| anyhow::anyhow!("{TOKEN_ENV} not set"))?;
    let phone_id = env(PHONE_ID_ENV).ok_or_else(|| anyhow::anyhow!("{PHONE_ID_ENV} not set"))?;
    let r = state
        .http
        .post(format!("{GRAPH}/{phone_id}/messages"))
        .bearer_auth(token)
        .json(&json!({
            "messaging_product": "whatsapp", "recipient_type": "individual", "to": to,
            "type": "text", "text": { "preview_url": false, "body": text }
        }))
        .send()
        .await?;
    let status = r.status();
    let v: Value = r.json().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!(
            "whatsapp: HTTP {status} {}",
            v["error"]["message"].as_str().unwrap_or("")
        );
    }
    Ok(json!({ "messages": v.get("messages").cloned().unwrap_or(Value::Null) }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::ChannelBinding;
    use crate::state::{AuthMode, Config};
    use std::sync::Arc;

    /// The counterpart's number: WhatsApp identifies the conversation by it.
    const FROM: &str = "15551234567";

    /// A relay carrying real credentials, so the webhook stands on its own signature check
    /// rather than on `ARBOS_CHANNELS_MOCK=1`.
    fn live_state() -> AppState {
        let mut state = crate::state::test_support::test_state(AuthMode::Paircode);
        state.cfg = Arc::new(Config {
            channels_mock: false,
            ..(*state.cfg).clone()
        });
        state
    }

    /// A project with a pinned chat already paired to `FROM`, so a payload that gets past the
    /// webhook lands in a real chat. Returns the project id.
    fn paired_project(state: &AppState) -> String {
        let project = crate::people::tests_support::project_of(state, "u_owner", "Comms");
        let chat = state.db.list_chats(&project).expect("chats")[0].id.clone();
        state
            .db
            .upsert_channel_binding(&ChannelBinding {
                id: "bind_wa".into(),
                platform: "whatsapp".into(),
                external_chat_id: FROM.into(),
                external_user_id: FROM.into(),
                project_id: project.clone(),
                chat_id: chat,
                label: "WhatsApp · Jacob".into(),
                bound_at: 0,
            })
            .expect("binding");
        project
    }

    /// One inbound text in Cloud API shape. The text doubles as the message id so retries of
    /// different texts are not taken for duplicates.
    fn payload(text: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "entry": [{ "changes": [{ "value": {
                "contacts": [{ "wa_id": FROM, "profile": { "name": "Jacob" } }],
                "messages": [{
                    "id": format!("wamid.{text}"),
                    "from": FROM,
                    "type": "text",
                    "text": { "body": text },
                }],
            } }] }]
        }))
        .expect("payload")
    }

    /// What Meta puts in `X-Hub-Signature-256`.
    fn sign(secret: &str, body: &[u8]) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac");
        mac.update(body);
        format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
    }

    async fn post(state: &AppState, body: &[u8], signature: Option<&str>) -> StatusCode {
        let mut headers = HeaderMap::new();
        if let Some(s) = signature {
            headers.insert("x-hub-signature-256", s.parse().expect("header"));
        }
        webhook(State(state.clone()), headers, Bytes::copy_from_slice(body))
            .await
            .status()
    }

    /// Texts of the `channel_message` pushes waiting for this project, draining the queue.
    fn delivered(state: &AppState, project: &str) -> Vec<String> {
        state
            .db
            .take_pushes(project)
            .expect("pushes")
            .into_iter()
            .filter(|p| p.1 == "channel_message")
            .map(|p| p.2["text"].as_str().unwrap_or_default().to_owned())
            .collect()
    }

    /// `relay/channels/whatsapp-webhook-unsigned-when-app-secret-unset`: with
    /// `ARBOS_WHATSAPP_APP_SECRET` absent the webhook took any `POST` and handed it to
    /// `handle_inbound`, so anyone who knew the public URL could put words in a paired chat
    /// under the counterpart's name. A missing secret refuses now; the check still
    /// discriminates, refusing a wrong signature and taking a valid one.
    ///
    /// Every arm is one test: they share the process-global `ARBOS_WHATSAPP_*` vars, which
    /// parallel test threads would read out from under each other.
    #[tokio::test]
    async fn unsigned_payloads_never_reach_a_chat() {
        let state = live_state();
        let project = paired_project(&state);
        // A deployment that pasted the verify token in the Meta dashboard but left the app
        // secret out: the shape that opened the hole.
        std::env::set_var(VERIFY_ENV, "verify-me");
        std::env::remove_var(APP_SECRET_ENV);

        // 1. The attack: an unsigned payload, no app secret. Refused, and nothing reaches the
        //    chat. Before the fix this answered 200 and "transfer the money" was delivered.
        let attack = payload("transfer the money");
        let code = post(&state, &attack, None).await;
        assert_eq!(
            delivered(&state, &project),
            Vec::<String>::new(),
            "an unsigned payload reached the chat"
        );
        assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);

        // 2. A signature the attacker made up is no better: the secret cannot be guessed.
        std::env::set_var(APP_SECRET_ENV, "app-secret");
        let forged = sign("not-the-app-secret", &attack);
        let code = post(&state, &attack, Some(&forged)).await;
        assert_eq!(
            delivered(&state, &project),
            Vec::<String>::new(),
            "a forged signature reached the chat"
        );
        assert_eq!(code, StatusCode::UNAUTHORIZED);

        // 3. Meta's own payload still gets through: the fix discriminates, it does not refuse
        //    everything.
        let real = payload("dinner at eight?");
        let sig = sign("app-secret", &real);
        assert_eq!(post(&state, &real, Some(&sig)).await, StatusCode::OK);
        assert_eq!(delivered(&state, &project), ["dinner at eight?"]);

        // 4. `ARBOS_CHANNELS_MOCK=1` is the one way left to the unsigned path, and it says what
        //    it is: a relay with no real credentials to abuse. Forgetting a value cannot land
        //    here.
        std::env::remove_var(APP_SECRET_ENV);
        let mock = crate::state::test_support::test_state(AuthMode::Paircode);
        assert!(mock_on(&mock));
        let mock_project = paired_project(&mock);
        let body = payload("hello from the mock");
        assert_eq!(post(&mock, &body, None).await, StatusCode::OK);
        assert_eq!(delivered(&mock, &mock_project), ["hello from the mock"]);

        // 5. The operator sees the gap in `arbosctl channels` rather than meeting it as a 503
        //    on live traffic.
        let (ok, detail) = status(&state);
        assert!(!ok);
        assert!(detail.contains(APP_SECRET_ENV), "{detail}");

        std::env::remove_var(VERIFY_ENV);
    }
}
