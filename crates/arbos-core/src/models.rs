//! What a model id says about the model, for hosts whose list says
//! nothing. The provider's `architecture.input_modalities` wins when it
//! is there; this is the guess for when it is not.

/// Whether a model id reads as one that takes image input.
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
        || (m.contains("qwen") && m.contains("vl"))
        || (m.contains("llama-3.2") && m.contains("90b"))
        || m.contains("llama-4")
        || m.starts_with("grok-2-vision")
        || m.starts_with("grok-4")
}

/// Vision models worth describing an image with, best first: cheap, fast,
/// on every OpenRouter key. The first one a catalog lists is the offer.
pub const VISION_PREFERRED: &[&str] = &[
    "openai/gpt-4.1-mini",
    "google/gemini-3.8-flash",
    "openai/gpt-4o-mini",
    "anthropic/claude-sonnet-5",
    "gpt-4.1-mini",
    "gpt-4o-mini",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_that_see_and_names_that_do_not() {
        assert!(looks_vision("anthropic/claude-opus-5"));
        assert!(looks_vision("openai/gpt-4.1-mini"));
        assert!(looks_vision("google/gemini-3.8-flash"));
        assert!(!looks_vision("inception/mercury-2.5"));
        assert!(!looks_vision("deepseek/deepseek-chat"));
    }
}
