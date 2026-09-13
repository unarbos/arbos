//! One model call, ridden through failures.
//!
//! Owns the batch executor for the call, so a failed stream's speculative
//! tool calls are aborted before the retry starts a fresh set. Every retry
//! and every model switch is shown to the user as it happens; a final
//! failure is recorded on the transcript so the window explains itself and
//! the wake is not replayed forever.

use anyhow::Result;
use arbos_core::{Event, EventKind, Usage, append_event};
use serde_json::Value;
use std::path::Path;
use tokio::sync::mpsc;

use crate::{
    batch::{self, BatchCfg, Msg, Outcome},
    control::TurnControl,
    provider::{ChatMessage, Delta, FailKind, Interrupted, Provider, ProviderError, ToolCall},
    retry::{self, Models, RetryPolicy, Verdict},
    tool::{RunCx, View},
};

pub enum Step {
    Done {
        content: String,
        calls: Vec<ToolCall>,
        usage: Option<Usage>,
        outcomes: Vec<(ToolCall, Outcome)>,
        /// Goes back with this step's assistant message; see `Completion`.
        reasoning_details: Vec<Value>,
    },
    Interrupted,
    Failed {
        message: String,
    },
    /// The connection died after the model had already said something. A
    /// plain retry would answer twice, so the turn keeps `partial`, notes
    /// the cut, and takes another step from there.
    Cut {
        partial: String,
        why: String,
    },
}

pub struct StepCx<'a> {
    pub provider: &'a mut Provider,
    pub models: &'a mut Models,
    pub policy: &'a RetryPolicy,
    pub view: &'a View,
    pub cx: &'a RunCx,
    pub control: &'a TurnControl,
    pub batch_cfg: BatchCfg,
    pub transcript: &'a Path,
    pub window: u64,
}

pub async fn model_step(s: StepCx<'_>, messages: &[ChatMessage], tools: &[Value]) -> Result<Step> {
    let mut attempt: u32 = 0;
    // Set once the model has said it cannot read images: the turn goes on
    // with the pictures replaced by a note, on the same model.
    let mut text_only: Option<Vec<ChatMessage>> = None;
    loop {
        attempt += 1;
        s.provider.model = s.models.current().to_string();
        let messages: &[ChatMessage] = text_only.as_deref().unwrap_or(messages);

        let (tx, rx) = mpsc::unbounded_channel();
        let executor = tokio::spawn(batch::run(
            s.view.clone(),
            s.cx.clone(),
            s.control.clone(),
            s.batch_cfg,
            rx,
        ));
        let hooks = &s.cx.hooks;
        let emit = |delta: Delta| match delta {
            Delta::Text(text) => hooks.emit(&Event::new(EventKind::Assistant {
                text,
                reasoning_details: None,
            })),
            Delta::Thinking(text) => hooks.emit(&Event::new(EventKind::Thinking { text })),
            Delta::Call(call) => {
                let _ = tx.send(Msg::Call(call));
            }
        };
        let streamed = s
            .provider
            .complete_stream(messages, tools, s.control.cancel(), emit)
            .await;

        let err = match streamed {
            Ok(done) => {
                let _ = tx.send(Msg::Commit);
                drop(tx);
                let outcomes = executor
                    .await
                    .map_err(|e| anyhow::anyhow!("batch task: {e}"))?;
                return Ok(Step::Done {
                    content: done.content,
                    calls: done.calls,
                    usage: done.usage.map(|(used, _)| Usage {
                        used,
                        size: s.window,
                    }),
                    outcomes,
                    reasoning_details: done.reasoning_details,
                });
            }
            Err(e) => e,
        };

        // Whatever started speculatively for this attempt is discarded.
        let _ = tx.send(Msg::Abort);
        drop(tx);
        let _ = executor.await;

        if err.is::<Interrupted>() || s.control.is_stopped() {
            return Ok(Step::Interrupted);
        }
        let Some(pe) = err.downcast_ref::<ProviderError>() else {
            return Ok(Step::Failed {
                message: format!("{}: {err:#}", s.models.current()),
            });
        };
        let model = s.models.current().to_string();
        // A text-only model given a screenshot: the provider rejects the whole
        // request (OpenRouter: 404 "No endpoints found that support image
        // input"). Dropping the images and saying so beats failing the turn
        // or leaving the user's chosen model for one that can see.
        if text_only.is_none()
            && rejects_images(pe)
            && messages.iter().any(|m| !m.images.is_empty())
        {
            let stripped = strip_images(messages);
            hooks.emit(&Event::new(EventKind::Notice {
                text: format!(
                    "{model} does not accept image input; sending this turn without the attached image(s)."
                ),
                failed: false,
            }));
            text_only = Some(stripped);
            attempt = 0;
            continue;
        }
        if pe.visible
            && matches!(
                pe.kind,
                FailKind::Transport | FailKind::Idle | FailKind::Stream
            )
        {
            return Ok(Step::Cut {
                partial: pe.partial.clone(),
                why: format!("{model}: {pe}"),
            });
        }
        match retry::verdict(pe, attempt, s.policy, s.models.has_next()) {
            Verdict::Retry => {
                let wait = retry::delay(s.policy, attempt, pe.retry_after);
                hooks.emit(&Event::new(EventKind::Notice {
                    text: format!(
                        "{model}: {pe} — retrying in {} (attempt {}/{})",
                        retry::human(wait),
                        attempt + 1,
                        s.policy.max_attempts
                    ),
                    failed: false,
                }));
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = s.control.cancel().cancelled() => return Ok(Step::Interrupted),
                }
            }
            Verdict::Fallback => {
                let Some(next) = s.models.next().map(str::to_string) else {
                    return Ok(Step::Failed {
                        message: failure_text(&model, pe, attempt),
                    });
                };
                let why = if attempt > 1 {
                    format!("{pe} after {attempt} attempts")
                } else {
                    pe.to_string()
                };
                // On the transcript: the model and the user should both know
                // who answered this turn.
                append_event(
                    s.transcript,
                    &Event::new(EventKind::Notice {
                        text: format!("switched to {next} for this turn: {model} {why}"),
                        failed: false,
                    }),
                )?;
                attempt = 0;
            }
            Verdict::Fail => {
                return Ok(Step::Failed {
                    message: failure_text(&model, pe, attempt),
                });
            }
        }
    }
}

/// Whether the provider refused the request because the model has no image
/// input, as opposed to not serving the model at all.
fn rejects_images(e: &ProviderError) -> bool {
    let m = e.message.to_ascii_lowercase();
    matches!(e.status, Some(400) | Some(404) | Some(422))
        && (m.contains("image input")
            || m.contains("image_url")
            || m.contains("does not support image")
            || m.contains("vision"))
}

/// The same conversation with every image replaced by a note in the text,
/// so the model knows something was there and the user's words still land.
fn strip_images(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    messages
        .iter()
        .map(|m| {
            if m.images.is_empty() {
                return m.clone();
            }
            let n = m.images.len();
            let note = format!(
                "[{n} image{} omitted: the model does not accept image input]",
                if n == 1 { "" } else { "s" }
            );
            let content = match m.content.as_deref() {
                Some(c) if !c.trim().is_empty() => format!("{c}\n\n{note}"),
                _ => note,
            };
            ChatMessage {
                content: Some(content),
                images: Vec::new(),
                ..m.clone()
            }
        })
        .collect()
}

/// The line the user reads when nothing more can be done. Names the model,
/// the reason, and one thing they can do about it.
fn failure_text(model: &str, e: &ProviderError, attempts: u32) -> String {
    let tried = if attempts > 1 {
        format!(" after {attempts} attempts")
    } else {
        String::new()
    };
    let hint = if e.visible {
        "The answer was cut off mid-stream; send the message again."
    } else if let Some(h) = e.retry_after {
        return format!(
            "{model}: {e}{tried}. The provider asked to wait {} before retrying; try later or set fallback_models in config.toml.",
            retry::human(h)
        );
    } else {
        match e.status {
            Some(401) => {
                "Check the API key (api_key or api_key_env in ~/.config/arbos/config.toml)."
            }
            Some(402) | Some(403) => "Check billing or access for this key.",
            Some(404) => "Check the model name in config.toml or agent.md.",
            Some(400) | Some(413) | Some(422) => {
                "The request was rejected. If the provider says the context is too long, the kernel compacts before the next call; set window_tokens = 0 in config.toml so it plans against the model's own context length."
            }
            Some(429) | Some(529) | Some(500..=599) => {
                "The provider is busy; try again in a minute or set fallback_models in config.toml."
            }
            _ => "Check the network and the provider status page, then send the message again.",
        }
    };
    format!("{model}: {e}{tried}. {hint}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ImagePart;

    fn err(status: u16, message: &str) -> ProviderError {
        ProviderError {
            kind: FailKind::Status,
            status: Some(status),
            message: message.into(),
            retry_after: None,
            should_retry: None,
            visible: false,
            partial: String::new(),
        }
    }

    #[test]
    fn image_rejection_is_recognised() {
        assert!(rejects_images(&err(
            404,
            "No endpoints found that support image input"
        )));
        assert!(rejects_images(&err(400, "image_url is not supported")));
        assert!(!rejects_images(&err(404, "model not found")));
        assert!(!rejects_images(&err(500, "image input")));
    }

    #[test]
    fn strip_replaces_images_with_a_note() {
        let mut m = ChatMessage::plain("user", Some("look".into()));
        m.images.push(ImagePart {
            mime: "image/png".into(),
            b64: "AAAA".into(),
        });
        let out = strip_images(&[ChatMessage::plain("system", Some("s".into())), m]);
        assert_eq!(out[0].content.as_deref(), Some("s"));
        assert!(out[1].images.is_empty());
        let c = out[1].content.as_deref().unwrap();
        assert!(c.starts_with("look\n\n[1 image omitted"), "{c}");
    }
}
