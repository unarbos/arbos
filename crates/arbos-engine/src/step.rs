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
    /// `vision_model` from config: who describes images the turn's model
    /// cannot see. Empty: a vision-capable fallback, else the default.
    pub vision_model: &'a str,
    /// Whether the provider's model list says the turn's model takes image
    /// input. None: the list does not say; the first call tells.
    pub sees_images: Option<bool>,
}

pub async fn model_step(
    mut s: StepCx<'_>,
    messages: &[ChatMessage],
    tools: &[Value],
) -> Result<Step> {
    let mut attempt: u32 = 0;
    // Set once the model has said it cannot read images: the turn goes on
    // with the pictures replaced by a note, on the same model.
    let mut text_only: Option<Vec<ChatMessage>> = None;
    // Set once the provider has rejected the stored thinking blocks
    // (Gemini: "corrupted thought signature"): the same model, once more,
    // with every earlier `reasoning_details` left out.
    let mut no_reasoning: Option<Vec<ChatMessage>> = None;
    // A model known not to see (the provider's list says so, or it refused
    // images earlier this process) gets the pictures in words up front,
    // rather than a call that is bound to fail.
    let has_images = messages.iter().any(|m| !m.images.is_empty());
    if has_images
        && (s.sees_images == Some(false) || crate::describe::is_text_only(s.models.current()))
    {
        let model = s.models.current().to_string();
        let hooks = std::sync::Arc::clone(&s.cx.hooks);
        text_only = Some(describe_or_strip(&mut s, &model, messages, hooks.as_ref()).await);
    }
    loop {
        attempt += 1;
        s.provider.model = s.models.current().to_string();
        let messages: &[ChatMessage] = no_reasoning
            .as_deref()
            .or(text_only.as_deref())
            .unwrap_or(messages);

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
            Delta::Waiting(for_) => hooks.working(for_.as_secs()),
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
                        cost: done.cost,
                        cached: done.cached,
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
        // input"). A vision model puts the pictures into words and the turn
        // goes on with the user's chosen model; the images are never dropped
        // silently, and the user's model choice stands.
        if text_only.is_none()
            && rejects_images(pe)
            && messages.iter().any(|m| !m.images.is_empty())
        {
            crate::describe::remember_text_only(&model);
            let hooks = std::sync::Arc::clone(&s.cx.hooks);
            text_only = Some(describe_or_strip(&mut s, &model, messages, hooks.as_ref()).await);
            attempt = 0;
            continue;
        }
        // The provider refuses the thinking blocks we sent back from an
        // earlier step (a signature it no longer accepts). They are not
        // needed to answer; drop them and ask the same model again, once.
        if no_reasoning.is_none()
            && rejects_reasoning(pe)
            && messages.iter().any(|m| m.reasoning_details.is_some())
        {
            let stripped = strip_reasoning(messages);
            hooks.emit(&Event::new(EventKind::Notice {
                text: format!(
                    "{model} rejected the stored thinking blocks ({}); retrying once without them.",
                    pe.message.trim()
                ),
                failed: false,
            }));
            no_reasoning = Some(stripped);
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
/// A 4xx that names the thinking blocks: Gemini's "Corrupted thought
/// signature", Anthropic's invalid `thinking` block / signature errors,
/// anything mentioning `reasoning_details`.
fn rejects_reasoning(e: &ProviderError) -> bool {
    let m = e.message.to_ascii_lowercase();
    let names_thinking = m.contains("thought signature")
        || m.contains("reasoning_details")
        || m.contains("reasoning details")
        || (m.contains("thinking") && m.contains("signature"))
        || (m.contains("thinking") && m.contains("invalid"));
    let client_side = matches!(e.status, Some(400) | Some(422))
        || (e.kind == FailKind::Stream && e.status.is_none());
    client_side && names_thinking
}

/// The same conversation without the `reasoning_details` echoes.
fn strip_reasoning(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    messages
        .iter()
        .map(|m| {
            let mut m = m.clone();
            m.reasoning_details = None;
            m
        })
        .collect()
}

/// The conversation for a model that cannot see: every image described by
/// a vision model, recorded on the transcript as `image_described` lines
/// (the window draws them inside the message card). When no model can
/// describe them, the old fallback: images stripped, one notice saying
/// which model could not see and why the description failed.
async fn describe_or_strip(
    s: &mut StepCx<'_>,
    model: &str,
    messages: &[ChatMessage],
    hooks: &dyn crate::tools::Hooks,
) -> Vec<ChatMessage> {
    let images = crate::describe::images_in(messages);
    let listed = if s.provider.replay.is_some() {
        Vec::new()
    } else {
        crate::provider::vision_models(&s.provider.base, &s.provider.key).await
    };
    let vision =
        crate::describe::vision_model(s.vision_model, s.models, model, &s.provider.base, &listed);
    let context = messages
        .iter()
        .rev()
        .find(|m| m.role == "user" && !m.images.is_empty())
        .and_then(|m| m.content.clone())
        .unwrap_or_default();
    let mut describer = s.provider.clone();
    describer.model = vision.clone();
    describer.reasoning_effort = None;
    describer.trace_purpose = "describe".to_string();
    match crate::describe::describe(&describer, &images, &context).await {
        Ok(texts) => {
            let described: Vec<(String, String)> =
                images.iter().map(|(p, _)| p.clone()).zip(texts).collect();
            for ev in crate::describe::events(&described, &vision) {
                if let Err(e) = append_event(s.transcript, &ev) {
                    eprintln!("image_described: {e:#}");
                }
                hooks.emit(&ev);
            }
            crate::describe::with_descriptions(messages, &described, &vision)
        }
        Err(e) => {
            hooks.emit(&Event::new(EventKind::Notice {
                text: format!(
                    "{model} does not accept image input, and {vision} could not describe the attached image(s) ({e:#}); sending this turn without them."
                ),
                failed: false,
            }));
            strip_images(messages)
        }
    }
}

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
            path: String::new(),
        });
        let out = strip_images(&[ChatMessage::plain("system", Some("s".into())), m]);
        assert_eq!(out[0].content.as_deref(), Some("s"));
        assert!(out[1].images.is_empty());
        let c = out[1].content.as_deref().unwrap();
        assert!(c.starts_with("look\n\n[1 image omitted"), "{c}");
    }
}
