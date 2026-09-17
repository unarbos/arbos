//! A chat's title from the model, after its first turn (F-156). A chat
//! nobody named is labelled from its first prompt's opening words —
//! *This project is a* — and Cursor replaces that with a model summary a
//! few seconds after the first reply. The desktop holds no key, so the
//! summary is the kernel's to make; this is the one call.

use anyhow::{Context, Result};
use arbos_core::Place;

use crate::provider::{ChatMessage, Provider};

/// Words the title may have at most; `chattitle::normalize` cuts to its
/// own bound after this asks for a short one.
const PROMPT: &str = "Title this chat in three to five plain words, like a short label in a sidebar: what it is about, not what was said. No quotes, no trailing punctuation, no label such as \"Title:\". Answer with the title alone.";

/// The replay agent a scripted title must be pinned to: under the replay
/// provider a title is made only when the script has a line for
/// `<agent>:title`, so no test's turn lines are taken for one.
pub fn replay_agent(agent: &str) -> String {
    format!("{agent}:title")
}

/// One call for `agent`'s title from its first exchange, or `None` when
/// no call can be made (no key; replay with no line for it). A failure
/// of the call itself is an error: the caller records that it was tried.
pub async fn title_for(
    place: &Place,
    host: &arbos_core::Host,
    agent: &str,
    first_user: &str,
    first_reply: &str,
) -> Result<Option<String>> {
    let replay = crate::replay::current()?;
    let replay_agent = replay_agent(agent);
    if let Some(r) = &replay
        && !r.has_line_for(&replay_agent)
    {
        return Ok(None);
    }
    let key = if replay.is_some() {
        "replay".to_string()
    } else {
        let dir = place.path().to_path_buf();
        let env = host.config.key_env();
        let from_place =
            tokio::task::spawn_blocking(move || crate::secrets::model_key_from_place(&dir, &env))
                .await
                .ok()
                .flatten();
        match from_place {
            Some(Ok(k)) => k,
            Some(Err(_)) | None => match host.api_key() {
                Some(k) => k,
                None => return Ok(None),
            },
        }
    };
    let api_base = if replay.is_some() {
        host.config
            .api_base()
            .unwrap_or_else(|_| "http://replay.invalid/v1".to_string())
    } else {
        host.config.api_base()?
    };
    let provider = Provider {
        base: api_base,
        key,
        model: host.config.model(),
        reasoning_effort: None,
        cache_ttl: None,
        data_policy: host.config.data_policy.clone(),
        stream_idle: std::time::Duration::from_secs(30),
        first_byte: std::time::Duration::from_secs(20),
        max_tokens: if replay.is_some() { None } else { Some(32) },
        trace: None,
        trace_agent: replay_agent.clone(),
        trace_purpose: "title".into(),
        trace_line: 0,
        replay,
    };
    let user = ChatMessage::plain(
        "user",
        Some(format!(
            "{PROMPT}\n\nThe user's first message:\n{}\n\nThe reply's opening:\n{}",
            arbos_core::text::clip(first_user.trim(), 800),
            arbos_core::text::clip(first_reply.trim(), 400)
        )),
    );
    let done = provider
        .complete(&[user], &[])
        .await
        .with_context(|| format!("{} could not title the chat", provider.model))?;
    Ok(arbos_core::chattitle::normalize(&done.content))
}
