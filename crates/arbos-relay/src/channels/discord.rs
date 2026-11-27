//! Discord bot adapter: Gateway inbound (one websocket the relay holds: identify, heartbeat,
//! resume, `MESSAGE_CREATE`), `POST /channels/{id}/messages` outbound.
//!
//! Env: `ARBOS_DISCORD_BOT_TOKEN`. The bot answers a DM, or a server message that mentions it;
//! Discord delivers the text of both without the privileged Message Content intent.

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;

use super::{handle_inbound, mock_on, Inbound, Platform};
use crate::state::AppState;

const TOKEN_ENV: &str = "ARBOS_DISCORD_BOT_TOKEN";
const API: &str = "https://discord.com/api/v10";
const GATEWAY: &str = "wss://gateway.discord.gg";
const GATEWAY_QUERY: &str = "/?v=10&encoding=json";
/// GUILD_MESSAGES (1 << 9) | DIRECT_MESSAGES (1 << 12). Neither is privileged.
const INTENTS: u64 = (1 << 9) | (1 << 12);
/// Discord's `content` limit for one message.
const MAX_MESSAGE_CHARS: usize = 2000;
/// What one inbound message may carry into a turn (Discord's own cap is 4000 with Nitro).
const MAX_INBOUND_CHARS: usize = 4000;
const MAX_NAME_CHARS: usize = 64;
/// One gateway payload; the events this adapter subscribes to are a few KiB.
const MAX_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(20);
const MIN_BACKOFF: Duration = Duration::from_secs(1);
/// At most 720 reconnects a day, under Discord's 1000 identifies per 24 h.
const MAX_BACKOFF: Duration = Duration::from_secs(120);
/// Longest a 429 `retry_after` is honoured before the send fails instead.
const MAX_RETRY_AFTER_S: f64 = 10.0;

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

pub fn status(_state: &AppState) -> (bool, String) {
    match env(TOKEN_ENV) {
        Some(_) => (true, "bot token · gateway".into()),
        None => (false, format!("missing {TOKEN_ENV}")),
    }
}

/// A Discord id: a decimal u64 as a string. Anything else never reaches a URL or the database.
fn is_snowflake(s: &str) -> bool {
    !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit())
}

fn snowflake(v: Option<&Value>) -> Option<&str> {
    v?.as_str().filter(|s| is_snowflake(s))
}

/// One `MESSAGE_CREATE` → `Inbound`. `None` for the bot itself, other bots and webhooks, system
/// messages, and a server message that does not mention the bot (`self_id`).
pub fn inbound_from_message(msg: &Value, self_id: &str) -> Option<Inbound> {
    if !is_snowflake(self_id) {
        return None;
    }
    let author = msg.get("author")?;
    let user_id = snowflake(author.get("id"))?;
    if user_id == self_id
        || author.get("bot").and_then(Value::as_bool) == Some(true)
        || msg.get("webhook_id").is_some_and(|w| !w.is_null())
    {
        return None;
    }
    // DEFAULT and REPLY; joins, pins, boosts and the rest are not something a user said.
    if !matches!(msg.get("type").and_then(Value::as_u64), Some(0 | 19)) {
        return None;
    }
    let chat_id = snowflake(msg.get("channel_id"))?;
    let id = snowflake(msg.get("id"))?;
    let content = msg.get("content").and_then(Value::as_str)?;
    let in_server = msg.get("guild_id").is_some_and(|g| !g.is_null());
    let mentioned = msg
        .get("mentions")
        .and_then(Value::as_array)
        .is_some_and(|ms| ms.iter().any(|m| snowflake(m.get("id")) == Some(self_id)));
    if in_server && !mentioned {
        return None;
    }
    let text: String = content
        .replace(&format!("<@{self_id}>"), "")
        .replace(&format!("<@!{self_id}>"), "")
        .chars()
        .take(MAX_INBOUND_CHARS)
        .collect();
    let name: String = ["global_name", "username"]
        .iter()
        .find_map(|k| author.get(*k).and_then(Value::as_str))
        .filter(|n| !n.trim().is_empty())
        .unwrap_or("Discord user")
        .chars()
        .take(MAX_NAME_CHARS)
        .collect();
    let kind = if in_server { "server" } else { "DM" };
    Some(Inbound {
        platform: Platform::Discord,
        external_chat_id: chat_id.to_owned(),
        external_user_id: user_id.to_owned(),
        sender_name: name.clone(),
        text,
        event_key: Some(format!("discord:{id}")),
        trusted_sender: true,
        label: format!("Discord · {kind} · {name}"),
    })
}

/// `text` in pieces of at most 2000 characters, cut at a line break when the piece has one.
pub fn chunks(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some((hard, _)) = rest.char_indices().nth(MAX_MESSAGE_CHARS) {
        let cut = rest[..hard].rfind('\n').filter(|&i| i > 0).unwrap_or(hard);
        out.push(&rest[..cut]);
        rest = rest[cut..].trim_start_matches('\n');
    }
    out.push(rest);
    // Discord refuses a message with no visible text.
    out.retain(|c| !c.trim().is_empty());
    out
}

/// Sends every piece in order. A piece that fails after an earlier one went out leaves the reply
/// half-sent, and a retry of the whole reply repeats the first pieces: Discord has no idempotency
/// key for a bot message.
pub async fn send(state: &AppState, chat_id: &str, text: &str) -> anyhow::Result<Value> {
    let token = env(TOKEN_ENV).ok_or_else(|| anyhow::anyhow!("{TOKEN_ENV} not set"))?;
    anyhow::ensure!(is_snowflake(chat_id), "discord: bad channel id");
    let mut ids = Vec::new();
    for part in chunks(text) {
        let mut retried = false;
        let v = loop {
            let r = state
                .http
                .post(format!("{API}/channels/{chat_id}/messages"))
                .header("Authorization", format!("Bot {token}"))
                // A reply never pings anyone, whatever the agent wrote.
                .json(&json!({ "content": part, "allowed_mentions": { "parse": [] } }))
                .timeout(IO_TIMEOUT)
                .send()
                .await?;
            let status = r.status();
            let v: Value = r.json().await.unwrap_or_default();
            if status.as_u16() == 429 && !retried {
                retried = true;
                let wait = v.get("retry_after").and_then(Value::as_f64).unwrap_or(1.0);
                anyhow::ensure!(
                    (0.0..=MAX_RETRY_AFTER_S).contains(&wait),
                    "discord: rate limited"
                );
                tokio::time::sleep(Duration::from_secs_f64(wait)).await;
                continue;
            }
            if !status.is_success() {
                anyhow::bail!(
                    "discord: HTTP {status} {}",
                    v.get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .chars()
                        .take(200)
                        .collect::<String>()
                );
            }
            break v;
        };
        ids.push(v["id"].clone());
    }
    Ok(json!({ "message_ids": ids }))
}

/// What a dropped connection picks up from: the session to resume and the bot's own user id.
struct Session {
    id: String,
    resume_url: String,
    seq: u64,
    self_id: String,
}

/// `resume_gateway_url` from READY, kept only as `wss://<host under discord.gg>`: the bot token
/// is sent to it on resume.
fn resume_url(raw: &str) -> Option<String> {
    let u = url::Url::parse(raw).ok()?;
    let host = u.host_str()?;
    (u.scheme() == "wss" && (host == "discord.gg" || host.ends_with(".discord.gg")))
        .then(|| format!("wss://{host}"))
}

/// The Gateway connection (`ARBOS_DISCORD_BOT_TOKEN` set): reconnects with a doubling backoff and
/// resumes the session when Discord allows it. A close code that says the token or the intents
/// are wrong ends the task; a relay restart with a fixed token starts it again.
pub async fn spawn_gateway(state: AppState) {
    if mock_on(&state) {
        return;
    }
    let Some(token) = env(TOKEN_ENV) else {
        return;
    };
    tracing::info!("discord: connecting to the gateway");
    tokio::spawn(async move {
        let mut session = None;
        let mut backoff = MIN_BACKOFF;
        loop {
            match connection(&state, &token, &mut session, &mut backoff).await {
                Ok(Some(code)) => {
                    tracing::error!(code, "discord: gateway refused the bot, not reconnecting");
                    return;
                }
                Ok(None) => {}
                Err(e) => tracing::warn!(error = %e, "discord gateway"),
            }
            tokio::time::sleep(backoff.mul_f64(1.0 + rand::random::<f64>())).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    });
}

/// One connection, until it ends. `Ok(Some(code))` is a close Discord says not to reconnect from.
async fn connection(
    state: &AppState,
    token: &str,
    session: &mut Option<Session>,
    backoff: &mut Duration,
) -> anyhow::Result<Option<u16>> {
    let base = session.as_ref().map_or(GATEWAY, |s| &s.resume_url);
    let cfg = WebSocketConfig::default()
        .max_message_size(Some(MAX_PAYLOAD_BYTES))
        .max_frame_size(Some(MAX_PAYLOAD_BYTES));
    let connect = tokio_tungstenite::connect_async_with_config(
        format!("{base}{GATEWAY_QUERY}"),
        Some(cfg),
        false,
    );
    let connected = match tokio::time::timeout(IO_TIMEOUT, connect).await {
        Ok(r) => r.map_err(anyhow::Error::from),
        Err(e) => Err(e.into()),
    };
    if connected.is_err() {
        // A resume host that is gone must not be dialled forever: identify afresh next time.
        *session = None;
    }
    let (ws, _) = connected?;
    let (mut tx, mut rx) = ws.split();

    let hello: Value = match tokio::time::timeout(IO_TIMEOUT, rx.next()).await? {
        Some(Ok(Message::Text(t))) => serde_json::from_str(t.as_str())?,
        _ => anyhow::bail!("no Hello"),
    };
    let every = hello["d"]["heartbeat_interval"]
        .as_u64()
        .filter(|_| hello["op"] == 10)
        .ok_or_else(|| anyhow::anyhow!("no Hello"))?
        .clamp(1_000, 120_000);
    let every = Duration::from_millis(every);
    let mut beat = tokio::time::interval_at(
        tokio::time::Instant::now() + every.mul_f64(rand::random::<f64>()),
        every,
    );
    let mut acked = true;
    let mut seq = session.as_ref().map(|s| s.seq);

    let open = match session.as_ref() {
        Some(s) => json!({ "op": 6, "d": { "token": token, "session_id": s.id, "seq": s.seq } }),
        None => json!({ "op": 2, "d": {
            "token": token,
            "intents": INTENTS,
            "properties": { "os": std::env::consts::OS, "browser": "arbos", "device": "arbos" },
        } }),
    };
    tx.send(Message::text(open.to_string())).await?;

    loop {
        let frame = tokio::select! {
            _ = beat.tick() => {
                anyhow::ensure!(acked, "heartbeat not acknowledged");
                acked = false;
                tx.send(Message::text(json!({ "op": 1, "d": seq }).to_string())).await?;
                continue;
            }
            m = rx.next() => m,
        };
        let text = match frame {
            None => return Ok(None),
            Some(Err(e)) => return Err(e.into()),
            Some(Ok(Message::Text(t))) => t,
            Some(Ok(Message::Close(f))) => {
                let code = f.map_or(0, |f| u16::from(f.code));
                // Authentication failed, or a shard / API version / intent Discord will not take.
                return Ok(matches!(code, 4004 | 4010..=4014).then_some(code));
            }
            Some(Ok(_)) => continue,
        };
        let p: Value = serde_json::from_str(text.as_str())?;
        match p["op"].as_u64() {
            Some(0) => {
                if let Some(s) = p["s"].as_u64() {
                    seq = Some(s);
                    if let Some(sess) = session.as_mut() {
                        sess.seq = s;
                    }
                }
                let d = &p["d"];
                match p["t"].as_str() {
                    Some("READY") => {
                        let (Some(id), Some(self_id), Some(resume_url)) = (
                            d["session_id"].as_str().filter(|s| s.len() <= 128),
                            snowflake(d["user"].get("id")),
                            d["resume_gateway_url"].as_str().and_then(resume_url),
                        ) else {
                            anyhow::bail!("READY without a usable session");
                        };
                        tracing::info!(bot = self_id, "discord: gateway ready");
                        *session = Some(Session {
                            id: id.to_owned(),
                            resume_url,
                            seq: seq.unwrap_or(0),
                            self_id: self_id.to_owned(),
                        });
                        *backoff = MIN_BACKOFF;
                    }
                    Some("RESUMED") => *backoff = MIN_BACKOFF,
                    Some("MESSAGE_CREATE") => {
                        let inb = session
                            .as_ref()
                            .and_then(|s| inbound_from_message(d, &s.self_id));
                        if let Some(inb) = inb {
                            handle_inbound(state, inb).await;
                        }
                    }
                    _ => {}
                }
            }
            // Discord asks for a heartbeat now.
            Some(1) => {
                tx.send(Message::text(json!({ "op": 1, "d": seq }).to_string()))
                    .await?;
            }
            // Reconnect: resume on a new connection.
            Some(7) => return Ok(None),
            // Invalid Session: `d` says whether the session can still be resumed.
            Some(9) => {
                if p["d"] != true {
                    *session = None;
                }
                return Ok(None);
            }
            Some(11) => acked = true,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::Outcome;
    use crate::db::{ChannelPairCode, ChannelScope, Chat};
    use arbos_core::ids::now_ms;

    const BOT: &str = "900000000000000001";

    fn dm(user: &str, content: &str) -> Value {
        json!({
            "id": "1000", "type": 0, "channel_id": "2000", "content": content,
            "author": { "id": user, "username": "jacob", "global_name": "Jacob" },
            "mentions": [],
        })
    }

    #[test]
    fn the_bot_other_bots_and_malformed_events_are_not_messages() {
        assert!(inbound_from_message(&dm("42", "hi"), BOT).is_some());
        // Its own message, echoed back by the gateway.
        assert!(inbound_from_message(&dm(BOT, "hi"), BOT).is_none());
        let mut bot = dm("43", "hi");
        bot["author"]["bot"] = json!(true);
        assert!(inbound_from_message(&bot, BOT).is_none());
        let mut hook = dm("44", "hi");
        hook["webhook_id"] = json!("5000");
        assert!(inbound_from_message(&hook, BOT).is_none());
        // A member-join system message.
        let mut join = dm("45", "");
        join["type"] = json!(7);
        assert!(inbound_from_message(&join, BOT).is_none());
        // Ids that are not snowflakes never become a chat or a user.
        assert!(inbound_from_message(&dm("../users/@me", "hi"), BOT).is_none());
        let mut chan = dm("42", "hi");
        chan["channel_id"] = json!("2000/../../x");
        assert!(inbound_from_message(&chan, BOT).is_none());
        // Before READY names the bot, nothing is a message.
        assert!(inbound_from_message(&dm("42", "hi"), "").is_none());
    }

    #[test]
    fn a_server_message_counts_only_when_it_mentions_the_bot() {
        let mut m = dm("42", "anyone?");
        m["guild_id"] = json!("3000");
        assert!(inbound_from_message(&m, BOT).is_none());
        m["content"] = json!(format!("<@{BOT}> /pair 123456"));
        m["mentions"] = json!([{ "id": BOT }]);
        let inb = inbound_from_message(&m, BOT).expect("mentioned");
        assert_eq!(inb.text.trim(), "/pair 123456");
        assert_eq!(inb.external_chat_id, "2000");
        assert_eq!(inb.external_user_id, "42");
        assert_eq!(inb.event_key.as_deref(), Some("discord:1000"));
        assert_eq!(inb.label, "Discord · server · Jacob");
    }

    #[test]
    fn inbound_text_and_names_are_bounded() {
        let mut m = dm("42", &"x".repeat(MAX_INBOUND_CHARS + 50));
        m["author"]["global_name"] = json!("n".repeat(500));
        let inb = inbound_from_message(&m, BOT).unwrap();
        assert_eq!(inb.text.chars().count(), MAX_INBOUND_CHARS);
        assert_eq!(inb.sender_name.chars().count(), MAX_NAME_CHARS);
    }

    #[test]
    fn a_reply_is_cut_at_the_message_limit() {
        assert_eq!(chunks("short"), ["short"]);
        assert!(chunks(" \n ").is_empty());
        let exact = "é".repeat(MAX_MESSAGE_CHARS);
        assert_eq!(chunks(&exact), [exact.as_str()]);
        // One character over: two messages, nothing lost, none over the limit.
        let over = "é".repeat(MAX_MESSAGE_CHARS + 1);
        let parts = chunks(&over);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts.concat(), over);
        assert!(parts.iter().all(|p| p.chars().count() <= MAX_MESSAGE_CHARS));
        // A line break inside the window is where the cut goes.
        let lines = format!("{}\n{}", "a".repeat(1500), "b".repeat(1500));
        assert_eq!(chunks(&lines), ["a".repeat(1500), "b".repeat(1500)]);
    }

    #[test]
    fn only_a_discord_gateway_host_is_resumed_on() {
        assert_eq!(
            resume_url("wss://gateway-us-east1-b.discord.gg").as_deref(),
            Some("wss://gateway-us-east1-b.discord.gg")
        );
        assert_eq!(resume_url("wss://evil.example/discord.gg"), None);
        assert_eq!(resume_url("wss://discord.gg.evil.example"), None);
        assert_eq!(resume_url("ws://gateway.discord.gg"), None);
    }

    /// The gateway event through the shared door: a stranger's DM gets the hint and reaches no
    /// box, and only the user who paired this DM starts a turn.
    #[tokio::test]
    async fn only_the_paired_user_reaches_a_box() {
        let state = crate::state::test_support::test_state(crate::state::AuthMode::Github);
        let (owner, _) = crate::people::tests_support::person(&state, 71);
        let project = crate::people::tests_support::project_of(&state, &owner, "Comms");
        state
            .db
            .insert_chat(&Chat {
                id: "c1".into(),
                project_id: project.clone(),
                title: "Comms".into(),
                is_pinned: true,
                position: 0,
                created_at: 0,
                doc_epoch: 1,
                kind: "chat".into(),
                participants: Vec::new(),
                forked_from: None,
            })
            .unwrap();
        let event = |id: &str, user: &str, content: &str| {
            let mut m = dm(user, content);
            m["id"] = json!(id);
            inbound_from_message(&m, BOT).expect("a user's DM")
        };

        assert_eq!(
            handle_inbound(&state, event("1", "42", "run the deploy")).await,
            Outcome::Hinted
        );
        assert!(state.db.take_pushes(&project).unwrap().is_empty());

        state
            .db
            .new_channel_pair_code(&ChannelPairCode {
                code: "123456".into(),
                project_id: project.clone(),
                chat_id: "c1".into(),
                created_at: now_ms(),
                expires_at: now_ms() + 60_000,
            })
            .unwrap();
        let Outcome::Paired { binding_id } =
            handle_inbound(&state, event("2", "42", "/pair 123456")).await
        else {
            panic!("expected pairing");
        };
        assert_eq!(
            handle_inbound(&state, event("3", "42", "what's on today?")).await,
            Outcome::Delivered {
                binding_id,
                project_id: project.clone()
            }
        );
        let pushes = state.db.take_pushes(&project).unwrap();
        assert_eq!(pushes.len(), 1);
        assert_eq!(pushes[0].2["platform"], "discord");
        assert_eq!(pushes[0].2["text"], "what's on today?");

        // The same event again (a resume replays it) is not a second turn.
        assert_eq!(
            handle_inbound(&state, event("3", "42", "what's on today?")).await,
            Outcome::Duplicate
        );
        // Someone else, in their own DM with the same bot, is still a stranger.
        let mut other = dm("77", "me too");
        other["id"] = json!("4");
        other["channel_id"] = json!("2001");
        let other = inbound_from_message(&other, BOT).unwrap();
        assert_eq!(handle_inbound(&state, other).await, Outcome::Hinted);
        assert!(state.db.take_pushes(&project).unwrap().is_empty());
        assert_eq!(
            state
                .db
                .list_channel_bindings(&ChannelScope::Everything)
                .unwrap()
                .len(),
            1
        );
    }
}
