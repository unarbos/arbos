//! Per-chat turn state shared by the control socket and the side task (spec §13): when the
//! chat was nudged, whether an ACK bubble is open, how long to keep "… is typing" on, and the
//! latency clock for the owner-facing log.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::config::Config;
use crate::log::SessionLog;
use crate::mirror::now_ms;
use crate::relay::Relay;
use crate::waker::Side;

#[derive(Debug, Clone, Default)]
pub struct Turn {
    /// Relay timestamp of the newest row that woke the agent.
    pub event_ts: i64,
    /// When the nudge went out.
    pub nudged_at: i64,
    /// The ACK bubble (`message.start`) if one is open, and when it opened.
    pub ack_message: Option<String>,
    pub ack_at: Option<i64>,
    /// Keep typing on until this time (0 = off).
    pub typing_until: i64,
    pub session: String,
}

#[derive(Clone, Default)]
pub struct Turns {
    inner: Arc<Mutex<HashMap<String, Turn>>>,
}

impl Turns {
    pub fn get(&self, chat: &str) -> Turn {
        self.inner
            .lock()
            .unwrap()
            .get(chat)
            .cloned()
            .unwrap_or_default()
    }

    pub fn update(&self, chat: &str, f: impl FnOnce(&mut Turn)) -> Turn {
        let mut m = self.inner.lock().unwrap();
        let t = m.entry(chat.to_owned()).or_default();
        f(t);
        t.clone()
    }

    pub fn chats_typing(&self, now: i64) -> Vec<String> {
        self.inner
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, t)| t.typing_until > now)
            .map(|(c, _)| c.clone())
            .collect()
    }

    pub fn open_acks(&self) -> Vec<(String, String, i64)> {
        self.inner
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(c, t)| {
                t.ack_message
                    .as_ref()
                    .map(|m| (c.clone(), m.clone(), t.ack_at.unwrap_or(0)))
            })
            .collect()
    }

    /// Typing off and the ack closed for `chat` (a reply posted or the turn gave up).
    pub fn finish(&self, chat: &str) {
        self.update(chat, |t| {
            t.typing_until = 0;
            t.ack_message = None;
            t.ack_at = None;
        });
    }
}

/// The daemon's side task: typing keeper, status lines, session.jsonl, uploads, ack timeouts.
pub async fn side_task(
    relay: Relay,
    cfg: Config,
    turns: Turns,
    log: SessionLog,
    mut rx: mpsc::UnboundedReceiver<Side>,
) {
    let mut typing_tick = tokio::time::interval(Duration::from_secs(4));
    let mut upload_tick = tokio::time::interval(Duration::from_secs(10));
    let mut ack_tick = tokio::time::interval(Duration::from_secs(5));
    loop {
        tokio::select! {
            side = rx.recv() => match side {
                Some(Side::Nudged { chats, events, session, at }) => {
                    for (chat, event_ts) in &chats {
                        let until = at + (cfg.waker.typing_timeout_s as i64) * 1000;
                        turns.update(chat, |t| {
                            t.event_ts = *event_ts;
                            t.nudged_at = at;
                            t.ack_message = None;
                            t.ack_at = None;
                            t.typing_until = until;
                            t.session = session.clone();
                        });
                        // Typing on the instant we nudge (spec §13.1).
                        let _ = relay
                            .event(json!({ "type": "typing", "chat": chat, "on": true }))
                            .await;
                        let _ = log.append(json!({
                            "kind": "wake", "chat": chat, "events": events, "session": session,
                            "ms": { "event_to_nudge": (at - event_ts).max(0) },
                        }));
                        let _ = log.append(json!({ "kind": "typing", "chat": chat }));
                    }
                }
                Some(Side::Status { chats, text }) => {
                    for chat in chats {
                        let _ = relay
                            .event(json!({ "type": "status", "chat": chat, "text": text }))
                            .await;
                    }
                }
                Some(Side::Log(v)) => {
                    let _ = log.append(v);
                }
                None => return,
            },
            _ = typing_tick.tick() => {
                for chat in turns.chats_typing(now_ms()) {
                    let _ = relay
                        .event(json!({ "type": "typing", "chat": chat, "on": true }))
                        .await;
                }
            }
            _ = ack_tick.tick() => {
                let now = now_ms();
                let limit = (cfg.waker.ack_timeout_s as i64) * 1000;
                for (chat, message, at) in turns.open_acks() {
                    if now - at > limit {
                        let _ = relay
                            .event(json!({ "type": "message.end", "chat": chat, "message": message }))
                            .await;
                        turns.finish(&chat);
                        let _ = log.append(json!({
                            "kind": "error", "chat": chat, "message": message,
                            "error": "ack_without_reply", "step": "ack_timeout",
                        }));
                    }
                }
            }
            _ = upload_tick.tick() => {
                let (batch, offset) = log.pending_upload(100);
                if !batch.is_empty() {
                    match relay.upload_log(&batch).await {
                        Ok(_) => {
                            let _ = log.mark_uploaded(offset);
                            let _ = log.append(json!({ "kind": "upload", "entries": batch.len() }));
                        }
                        Err(e) => tracing::debug!(error = %e, "log upload"),
                    }
                }
            }
        }
    }
}

/// Latency fields for an ack or a reply in `chat`, from the turn clock.
pub fn latencies(t: &Turn, now: i64, is_ack: bool) -> Value {
    let mut ms = serde_json::Map::new();
    if t.event_ts > 0 {
        ms.insert(
            if is_ack {
                "event_to_ack".into()
            } else {
                "event_to_reply".into()
            },
            json!((now - t.event_ts).max(0)),
        );
    }
    if t.nudged_at > 0 && is_ack {
        ms.insert("nudge_to_ack".into(), json!((now - t.nudged_at).max(0)));
    }
    if !is_ack {
        if let Some(a) = t.ack_at {
            ms.insert("ack_to_reply".into(), json!((now - a).max(0)));
        } else if t.nudged_at > 0 {
            ms.insert("nudge_to_reply".into(), json!((now - t.nudged_at).max(0)));
        }
    }
    Value::Object(ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latency_fields_follow_the_clock() {
        let t = Turn {
            event_ts: 1000,
            nudged_at: 1023,
            ack_message: Some("m".into()),
            ack_at: Some(2200),
            typing_until: 0,
            session: "s".into(),
        };
        let a = latencies(&t, 2200, true);
        assert_eq!(a["event_to_ack"], 1200);
        assert_eq!(a["nudge_to_ack"], 1177);
        let r = latencies(&t, 10603, false);
        assert_eq!(r["event_to_reply"], 9603);
        assert_eq!(r["ack_to_reply"], 8403);
        let turns = Turns::default();
        turns.update("c1", |t| {
            t.typing_until = now_ms() + 10_000;
            t.ack_message = Some("m1".into());
            t.ack_at = Some(1);
        });
        assert_eq!(turns.chats_typing(now_ms()).len(), 1);
        assert_eq!(turns.open_acks().len(), 1);
        turns.finish("c1");
        assert!(turns.chats_typing(now_ms()).is_empty());
        assert!(turns.open_acks().is_empty());
    }
}
