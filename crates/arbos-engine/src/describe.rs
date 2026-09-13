//! Images for a model that cannot see. When the turn's model takes no image
//! input, a vision-capable model describes each attached image in words,
//! the description goes on the transcript as an `image_described` line,
//! and the turn goes on with the selected model reading the words. Nothing
//! is dropped and the user's model choice stands.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result};
use arbos_core::{Event, EventKind};

use crate::provider::{ChatMessage, ImagePart, Provider};
use crate::retry::Models;

/// What to fall back to when the config names no `vision_model` and no
/// fallback looks vision-capable. Cheap, fast, and on every OpenRouter key.
pub const OPENROUTER_VISION_DEFAULT: &str = "openai/gpt-4.1-mini";
pub const OPENAI_VISION_DEFAULT: &str = "gpt-4.1-mini";

/// Models a provider refused images for, this process. The next turn skips
/// the doomed call and describes first.
fn text_only() -> &'static Mutex<HashSet<String>> {
    static SET: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SET.get_or_init(Default::default)
}

pub fn remember_text_only(model: &str) {
    if let Ok(mut s) = text_only().lock() {
        s.insert(model.to_string());
    }
}

pub fn is_text_only(model: &str) -> bool {
    text_only()
        .lock()
        .map(|s| s.contains(model))
        .unwrap_or(false)
}

/// Whether a model id reads as a vision model by name alone: for hosts
/// whose `/models` says nothing about modalities.
pub fn looks_vision(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    let m = m.rsplit('/').next().unwrap_or(&m);
    if m.contains("vision") {
        return true;
    }
    let text_only_hint = m.contains("mercury")
        || m.contains("embed")
        || m.contains("whisper")
        || m.contains("tts")
        || m.contains("instruct-text");
    if text_only_hint {
        return false;
    }
    m.starts_with("gpt-4o")
        || m.starts_with("gpt-4.1")
        || m.starts_with("gpt-5")
        || m.starts_with("o3")
        || m.starts_with("o4")
        || m.starts_with("chatgpt")
        || m.starts_with("claude")
        || m.starts_with("gemini")
        || m.starts_with("pixtral")
        || m.contains("qwen") && m.contains("vl")
        || m.contains("llama-3.2") && m.contains("90b")
        || m.contains("llama-4")
        || m.starts_with("grok-2-vision")
        || m.starts_with("grok-4")
}

/// The model that describes images for `current`: `configured` when set;
/// else the first fallback the provider lists as taking images (`listed`);
/// else the first fallback that looks vision-capable by name; else the
/// host's default. Never `current` itself.
pub fn vision_model(
    configured: &str,
    models: &Models,
    current: &str,
    base: &str,
    listed: &[String],
) -> String {
    let configured = configured.trim();
    if !configured.is_empty() && configured != current {
        return configured.to_string();
    }
    if let Some(m) = models
        .all()
        .iter()
        .find(|m| m.as_str() != current && listed.iter().any(|l| l == *m))
    {
        return m.clone();
    }
    if let Some(m) = models
        .all()
        .iter()
        .find(|m| m.as_str() != current && looks_vision(m))
    {
        return m.clone();
    }
    if base.contains("openai.com") {
        OPENAI_VISION_DEFAULT.to_string()
    } else {
        OPENROUTER_VISION_DEFAULT.to_string()
    }
}

/// `(path, part)` for every image in the conversation, in order, without
/// repeats. The path is what the transcript calls the attachment.
pub fn images_in(messages: &[ChatMessage]) -> Vec<(String, ImagePart)> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for m in messages {
        for img in &m.images {
            let key = if img.path.is_empty() {
                format!("#{}", out.len() + 1)
            } else {
                img.path.clone()
            };
            if seen.insert(key.clone()) {
                out.push((key, img.clone()));
            }
        }
    }
    out
}

const PROMPT: &str = "You describe images for an assistant that cannot see them. For each attached image, in order, write a precise description another model can act on: every piece of visible text verbatim (labels, code, numbers, error messages, menu items), the layout and what each region is, UI state (selected, disabled, hovered), colours only when they carry meaning, and anything that looks wrong or notable. No preamble, no opinions. Format exactly:\n\nImage 1: <description>\n\nImage 2: <description>\n\n(one block per image; use the number of the image, nothing else as a heading)";

/// One call to `provider` (already set to the vision model) with every
/// image; returns a description per image, in order. When the answer
/// cannot be split, one image gets the whole text and the rest a note.
pub async fn describe(
    provider: &Provider,
    images: &[(String, ImagePart)],
    context: &str,
) -> Result<Vec<String>> {
    let mut user = ChatMessage::plain(
        "user",
        Some(format!(
            "{PROMPT}\n\nThe user said, for context: {}\n\nImages, in order: {}",
            arbos_core::text::clip(context, 600),
            images
                .iter()
                .enumerate()
                .map(|(i, (p, _))| format!("{}={}", i + 1, p))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    );
    user.images = images.iter().map(|(_, part)| part.clone()).collect();
    let done = provider
        .complete(&[user], &[])
        .await
        .with_context(|| format!("{} could not describe the image(s)", provider.model))?;
    Ok(split(&done.content, images.len()))
}

/// `Image N:` blocks → N descriptions. Missing blocks become a note.
pub fn split(text: &str, n: usize) -> Vec<String> {
    let mut out: Vec<Option<String>> = vec![None; n];
    let mut current: Option<usize> = None;
    let mut buf = String::new();
    let flush = |current: Option<usize>, buf: &mut String, out: &mut Vec<Option<String>>| {
        if let Some(i) = current
            && i < n
            && !buf.trim().is_empty()
        {
            out[i] = Some(buf.trim().to_string());
        }
        buf.clear();
    };
    for line in text.lines() {
        let t = line.trim().trim_start_matches(['#', '*', '>']).trim();
        let heading = t
            .strip_prefix("Image ")
            .or_else(|| t.strip_prefix("image "))
            .and_then(|rest| {
                let (num, after) = rest.split_once(':')?;
                let num: usize = num.trim().trim_end_matches('*').parse().ok()?;
                Some((num, after.trim().trim_start_matches('*').trim().to_string()))
            });
        match heading {
            Some((num, rest)) if num >= 1 && num <= n => {
                flush(current, &mut buf, &mut out);
                current = Some(num - 1);
                buf.push_str(&rest);
            }
            _ => {
                if current.is_none() && !t.is_empty() {
                    current = Some(0);
                }
                if !buf.is_empty() {
                    buf.push('\n');
                }
                buf.push_str(line.trim_end());
            }
        }
    }
    flush(current, &mut buf, &mut out);
    out.into_iter()
        .map(|d| d.unwrap_or_else(|| "(no description came back for this image)".to_string()))
        .collect()
}

/// The conversation with every image replaced by its description, in the
/// message text, so a text-only model reads what the picture showed.
pub fn with_descriptions(
    messages: &[ChatMessage],
    described: &[(String, String)],
    model: &str,
) -> Vec<ChatMessage> {
    let mut by_path: std::collections::HashMap<&str, &str> = described
        .iter()
        .map(|(p, d)| (p.as_str(), d.as_str()))
        .collect();
    let mut nth = 0usize;
    messages
        .iter()
        .map(|m| {
            if m.images.is_empty() {
                return m.clone();
            }
            let mut extra = String::new();
            for img in &m.images {
                nth += 1;
                let key = if img.path.is_empty() {
                    format!("#{nth}")
                } else {
                    img.path.clone()
                };
                let text = by_path
                    .remove(key.as_str())
                    .unwrap_or("(no description came back for this image)");
                extra.push_str(&format!("\n\n[image {key} — described by {model}]\n{text}"));
            }
            let content = match m.content.as_deref() {
                Some(c) if !c.trim().is_empty() => format!("{c}{extra}"),
                _ => extra.trim_start().to_string(),
            };
            ChatMessage {
                content: Some(content),
                images: Vec::new(),
                ..m.clone()
            }
        })
        .collect()
}

/// The transcript lines for what happened, one per image.
pub fn events(described: &[(String, String)], model: &str) -> Vec<Event> {
    described
        .iter()
        .map(|(path, text)| {
            Event::new(EventKind::ImageDescribed {
                path: path.clone(),
                model: model.to_string(),
                text: text.clone(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_reads_numbered_blocks_and_fills_gaps() {
        let out = split(
            "Image 1: a red button\nlabelled Save\n\n**Image 2:** a table with 3 rows",
            3,
        );
        assert_eq!(out[0], "a red button\nlabelled Save");
        assert_eq!(out[1], "a table with 3 rows");
        assert!(out[2].contains("no description"));
    }

    #[test]
    fn split_gives_an_unlabelled_answer_to_the_only_image() {
        let out = split(
            "A screenshot of a terminal showing `cargo test` passing.",
            1,
        );
        assert_eq!(out.len(), 1);
        assert!(out[0].starts_with("A screenshot"));
    }

    #[test]
    fn vision_model_prefers_config_then_a_seeing_fallback_then_the_default() {
        let models = Models::new(
            "inception/mercury-2.5".into(),
            &[
                "deepseek/deepseek-chat".into(),
                "google/gemini-3.8-flash".into(),
            ],
        );
        let base = "https://openrouter.ai/api/v1";
        assert_eq!(
            vision_model("", &models, "inception/mercury-2.5", base, &[]),
            "google/gemini-3.8-flash"
        );
        // The provider's list wins over the name heuristic.
        assert_eq!(
            vision_model(
                "",
                &models,
                "inception/mercury-2.5",
                base,
                &["deepseek/deepseek-chat".into()]
            ),
            "deepseek/deepseek-chat"
        );
        assert_eq!(
            vision_model("x/y", &models, "inception/mercury-2.5", base, &[]),
            "x/y"
        );
        let none = Models::new("inception/mercury-2.5".into(), &["none".into()]);
        assert_eq!(
            vision_model("", &none, "inception/mercury-2.5", base, &[]),
            OPENROUTER_VISION_DEFAULT
        );
        assert!(!looks_vision("inception/mercury-2.5"));
        assert!(looks_vision("anthropic/claude-opus-5"));
    }

    #[test]
    fn descriptions_replace_pixels_in_the_text() {
        let mut m = ChatMessage::plain("user", Some("look at this".into()));
        m.images.push(ImagePart {
            mime: "image/png".into(),
            b64: "AAAA".into(),
            path: "shot.png".into(),
        });
        let out = with_descriptions(
            &[m],
            &[("shot.png".to_string(), "a blue square".to_string())],
            "openai/gpt-4.1-mini",
        );
        assert!(out[0].images.is_empty());
        let c = out[0].content.as_deref().unwrap();
        assert!(
            c.starts_with(
                "look at this\n\n[image shot.png — described by openai/gpt-4.1-mini]\na blue square"
            ),
            "{c}"
        );
    }
}
