//! Beeper adapter through the Harness Comms API (WhatsApp, Telegram, Discord … on the user's own
//! accounts). Inbound is a cursor loop over `POST /v1/messages/poll`; outbound queues
//! `POST /v1/requests` with an idempotent `request_id`. The API key is the `harness_comms`
//! connector's stored token (F1), so nothing is configured twice.
//!
//! Because these are the user's personal accounts, only messages the user sent themselves
//! (`sender.is_self`) may pair or receive the pairing hint; contacts are never written to.

use std::time::Duration;

use serde_json::{json, Value};

use super::{handle_inbound, mock_on, Inbound, Platform};
use crate::connectors::providers::Provider;
use crate::connectors::{self};
use crate::db::Connector;
use crate::state::AppState;

const CURSOR_META: &str = "beeper_poll_cursor";
const POLL_ENV: &str = "ARBOS_BEEPER_POLL_S";
/// The service caches poll results for 30 s per cursor; polling faster returns the same page.
const DEFAULT_POLL_S: u64 = 30;

/// The Harness Comms account this poller runs on. The poller has no caller, so it takes the one
/// connected account that has an owner: with no owner (the ownership migration could not
/// attribute it) or with two owners, polling stops rather than spending someone's credentials by
/// guess. An admin resolves it with `/admin/connectors`.
fn connector(state: &AppState) -> Option<Connector> {
    state
        .db
        .connector_by_provider_sole_owner(Provider::HarnessComms.as_str())
        .ok()
        .flatten()
        .filter(|c| c.status == "connected")
}

pub fn status(state: &AppState) -> (bool, String) {
    match connector(state) {
        Some(c) => (
            true,
            format!(
                "via Harness Comms · {}",
                c.config["baseUrl"].as_str().unwrap_or("")
            ),
        ),
        None => (
            false,
            "connect Harness Comms in Settings → Connectors".into(),
        ),
    }
}

async fn creds(state: &AppState) -> anyhow::Result<(String, String)> {
    let cn = connector(state).ok_or_else(|| anyhow::anyhow!("harness_comms is not connected"))?;
    let base = cn.config["baseUrl"]
        .as_str()
        .map(|u| u.trim_end_matches('/').to_owned())
        .ok_or_else(|| anyhow::anyhow!("harness_comms has no baseUrl"))?;
    let (bearer, _) = connectors::bearer_for(state, Provider::HarnessComms, &cn)
        .await
        .map_err(|(_, e)| anyhow::anyhow!(e))?;
    Ok((base, bearer))
}

/// One poll message → `Inbound`. `None` for our own API sends and non-text items.
pub fn inbound_from_message(m: &Value) -> Option<Inbound> {
    if m.get("source").and_then(Value::as_str) == Some("api") {
        return None;
    }
    if m.get("client_tag")
        .and_then(Value::as_str)
        .is_some_and(|t| t.starts_with("harness:"))
    {
        return None;
    }
    let text = m.get("text").and_then(Value::as_str)?.to_owned();
    let chat_id = m.get("chat_id").and_then(Value::as_str)?.to_owned();
    let sender = m.get("sender")?;
    let user_id = sender.get("id").and_then(Value::as_str)?.to_owned();
    let is_self = sender
        .get("is_self")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let name = sender
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(if is_self { "you" } else { "contact" })
        .to_owned();
    let network = m
        .get("network_label")
        .and_then(Value::as_str)
        .unwrap_or("Beeper")
        .to_owned();
    Some(Inbound {
        platform: Platform::Beeper,
        external_chat_id: chat_id,
        external_user_id: user_id,
        sender_name: name.clone(),
        text,
        event_key: m.get("id").map(|id| {
            format!(
                "beeper:{}",
                id.as_str().map(str::to_owned).unwrap_or(id.to_string())
            )
        }),
        trusted_sender: is_self,
        label: format!("Beeper · {network} · {name}"),
    })
}

/// `POST /v1/requests` with the given idempotent `request_id` (202 = recorded, not delivered).
pub async fn send(
    state: &AppState,
    chat_id: &str,
    text: &str,
    request_id: &str,
) -> anyhow::Result<Value> {
    let (base, bearer) = creds(state).await?;
    let r = state
        .http
        .post(format!("{base}/v1/requests"))
        .bearer_auth(bearer)
        .json(&json!({
            "request_id": request_id,
            "target": { "connector": "beeper", "chat_id": chat_id },
            "text": text,
            "intent": "Arbos channel reply",
        }))
        .send()
        .await?;
    let status = r.status();
    let v: Value = r.json().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!(
            "harness comms: HTTP {status} {}",
            v.to_string().chars().take(200).collect::<String>()
        );
    }
    Ok(
        json!({ "request_id": request_id, "status": v.get("status").or(v["data"].get("status")).cloned().unwrap_or(Value::Null) }),
    )
}

/// One poll round: returns how many messages were handled.
pub async fn poll_once(state: &AppState) -> anyhow::Result<usize> {
    let (base, bearer) = creds(state).await?;
    let cursor = state.db.meta_get(CURSOR_META)?;
    let body = match &cursor {
        Some(c) => json!({ "cursor": c }),
        None => json!({}),
    };
    let r = state
        .http
        .post(format!("{base}/v1/messages/poll"))
        .bearer_auth(bearer)
        .json(&body)
        .timeout(Duration::from_secs(40))
        .send()
        .await?;
    let status = r.status();
    let v: Value = r.json().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("poll: HTTP {status}");
    }
    let data = v.get("data").cloned().unwrap_or(Value::Null);
    let mut handled = 0;
    for m in data
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(inb) = inbound_from_message(m) {
            handle_inbound(state, inb).await;
            handled += 1;
        }
    }
    if let Some(next) = data.get("cursor").and_then(Value::as_str) {
        state.db.meta_set(CURSOR_META, next)?;
    }
    Ok(handled)
}

pub async fn spawn_poller(state: AppState) {
    if mock_on(&state) {
        return;
    }
    let every = std::env::var(POLL_ENV)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_POLL_S)
        .max(10);
    tokio::spawn(async move {
        let mut announced = false;
        loop {
            tokio::time::sleep(Duration::from_secs(every)).await;
            if connector(&state).is_none() {
                continue;
            }
            if !announced {
                tracing::info!(
                    every_s = every,
                    "beeper: polling Harness Comms for new messages"
                );
                announced = true;
            }
            match poll_once(&state).await {
                Ok(n) if n > 0 => tracing::info!(messages = n, "beeper poll"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "beeper poll"),
            }
        }
    });
}
