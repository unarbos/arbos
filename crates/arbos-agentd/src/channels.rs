//! The `channel_message` push (F7), dispatched by the push poller in `routines.rs`: a message
//! from Slack / Telegram / WhatsApp / Beeper lands in the bound chat as a user message with
//! `sender = channel:<platform>:<external user id>`, runs as a normal turn, and the reply prose
//! goes back through `POST /projects/{id}/channels/reply`.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use arbos_core::doc::{CommandView, MessageView};
use arbos_core::ids::{now_ms, ulid};

use crate::agent::Agent;

/// A hand-off to a subagent answers later, through a result turn in the same chat; relay those
/// too, for at most this long.
const FOLLOW_UP_MAX: Duration = Duration::from_secs(20 * 60);
const FOLLOW_UP_POLL: Duration = Duration::from_secs(5);

/// One line prepended to the model prompt when the user message came through a channel.
pub fn prompt_note(m: &MessageView) -> Option<String> {
    let sender = m.rest.get("sender").and_then(Value::as_str)?;
    let platform = sender.strip_prefix("channel:")?.split(':').next()?;
    let name = m
        .rest
        .get("senderName")
        .and_then(Value::as_str)
        .unwrap_or("the user");
    let platform_label = match platform {
        "slack" => "Slack",
        "telegram" => "Telegram",
        "whatsapp" => "WhatsApp",
        "beeper" => "Beeper",
        other => other,
    };
    Some(format!(
        "[Arbos] This message arrived via {platform_label} from {name}. Your reply is sent back there as plain chat text: keep it short, no Markdown tables or headings."
    ))
}

/// The prose of an assistant message as a channel sees it: text parts only, trimmed.
fn reply_text(m: &MessageView) -> String {
    m.text().trim().to_owned()
}

impl Agent {
    async fn post_channel_reply(
        &self,
        binding_id: &str,
        text: &str,
        in_reply_to: &str,
    ) -> Result<()> {
        let r = self
            .http_client()
            .post(format!(
                "{}/projects/{}/channels/reply",
                self.cfg.relay_url, self.cfg.project_id
            ))
            .bearer_auth(&self.cfg.box_token)
            .json(&json!({
                "bindingId": binding_id,
                "text": text,
                "requestId": format!("arbos:channel:{}:{in_reply_to}", self.cfg.project_id),
            }))
            .send()
            .await?;
        if !r.status().is_success() {
            let status = r.status();
            let text = r.text().await.unwrap_or_default();
            return Err(anyhow!(
                "channel reply: HTTP {} {}",
                status,
                crate::relay_error::message(&text)
            ));
        }
        Ok(())
    }

    /// `channel_message { bindingId, platform, externalChatId, externalUserId, chatId, text, senderName }`.
    pub(crate) async fn handle_channel_message(self: &Arc<Self>, payload: Value) -> Result<()> {
        let field = |k: &str| {
            payload
                .get(k)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("channel_message push missing {k}"))
        };
        let binding_id = field("bindingId")?;
        let platform = field("platform")?;
        let external_user_id = field("externalUserId")?;
        let text = field("text")?;
        let chat_id = field("chatId")?;
        let sender_name = payload
            .get("senderName")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let sender = format!("channel:{platform}:{external_user_id}");
        tracing::info!(%platform, binding = %binding_id, chat = %chat_id, "channel message");

        // The bound chat may be new to this box; the registry poll opens rooms every 3 s, so open
        // it here when it is not hosted yet. A chat that no longer exists falls back to the pinned one.
        let chat_id = if self.store.chat(&chat_id)?.is_some() {
            chat_id
        } else {
            let chats = self.store.chats()?;
            chats
                .iter()
                .find(|c| c.is_pinned)
                .or(chats.first())
                .map(|c| c.id.clone())
                .ok_or_else(|| anyhow!("no chat to deliver the channel message to"))?
        };
        let host = match self.sync.host(&chat_id).await {
            Some(h) => h,
            None => self.sync.open_room(&chat_id).await?,
        };
        let msg_id = ulid();
        let cmd = {
            let mut h = host.lock().await;
            let cmd_id = h.doc.append_user_message_from(
                &msg_id,
                &text,
                &self.cfg.device_id,
                &sender,
                &[
                    ("senderName", json!(sender_name)),
                    ("channelPlatform", json!(platform)),
                    ("channelBindingId", json!(binding_id)),
                ],
            );
            h.mark_seen(&cmd_id);
            self.note_own_row(&msg_id);
            CommandView {
                id: cmd_id,
                kind: "send".into(),
                payload: json!({ "messageId": msg_id }),
                issued_by: self.cfg.device_id.clone(),
                issued_at: now_ms(),
                status: "pending".into(),
            }
        };
        let outcome = self.handle_command(&chat_id, cmd).await;

        let mut sent: Vec<String> = Vec::new();
        let reply = {
            let h = host.lock().await;
            h.doc.messages().into_iter().rev().find(|m| {
                m.role == "assistant"
                    && m.rest.get("inReplyTo").and_then(Value::as_str) == Some(msg_id.as_str())
            })
        };
        let text = match (&outcome, reply.as_ref()) {
            (Ok(()), Some(m)) if !reply_text(m).is_empty() => reply_text(m),
            (Ok(()), Some(m)) => format!("(The turn ended with status {} and no text.)", m.status),
            (Ok(()), None) => "(No reply was written.)".to_owned(),
            (Err(e), _) => format!("Sorry — that failed on my side: {e}"),
        };
        if let Some(m) = &reply {
            sent.push(m.id.clone());
        }
        if let Err(e) = self.post_channel_reply(&binding_id, &text, &msg_id).await {
            tracing::warn!(error = %e, "channel reply");
        }
        outcome?;

        // A hand-off answers later: watch for further assistant replies to this message while a
        // subagent is running and relay each one once. The first check waits a beat so a
        // subagent that is still starting counts as busy.
        let started = std::time::Instant::now();
        loop {
            tokio::time::sleep(FOLLOW_UP_POLL).await;
            let busy = self
                .store
                .subagents()
                .unwrap_or_default()
                .iter()
                .any(|s| s.status == "running" || s.status == "queued")
                || self.any_turn_running();
            let new: Vec<MessageView> = {
                let h = host.lock().await;
                h.doc
                    .messages()
                    .into_iter()
                    .filter(|m| {
                        m.role == "assistant"
                            && m.status == "done"
                            && m.rest.get("inReplyTo").and_then(Value::as_str)
                                == Some(msg_id.as_str())
                            && !sent.contains(&m.id)
                    })
                    .collect()
            };
            for m in new {
                let t = reply_text(&m);
                sent.push(m.id.clone());
                if t.is_empty() {
                    continue;
                }
                if let Err(e) = self
                    .post_channel_reply(&binding_id, &t, &format!("{msg_id}:{}", m.id))
                    .await
                {
                    tracing::warn!(error = %e, "channel follow-up reply");
                }
            }
            if !busy || started.elapsed() > FOLLOW_UP_MAX {
                break;
            }
        }
        Ok(())
    }
}
