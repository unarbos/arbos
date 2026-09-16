//! Tool calls a model writes as text instead of calling: Anthropic-style
//! `<invoke name="bash"><parameter name="command">ls</parameter></invoke>`
//! inside `<function_calls>`, Hermes/Qwen `<tool_call>{…}</tool_call>`,
//! Llama `<|python_tag|>`, `<function=name>…</function>`. Nothing ran,
//! and the raw tags look broken on every client (subnet120 from the
//! phone, 2026-09-16). The markup is cut from the text before it reaches
//! the transcript; the turn then nudges once, as for a JSON call in text.

/// Tag names whose element (open … close, or open to the end of the text
/// when unclosed) is tool-call markup.
const BLOCK_TAGS: &[&str] = &[
    "function_calls",
    "invoke",
    "tool_call",
    "tool_calls",
    "function_call",
    "antml:function_calls",
    "antml:invoke",
];

/// Single markers that open a call in some models' tokenizers.
const MARKERS: &[&str] = &[
    "<|python_tag|>",
    "<|tool_call|>",
    "<|tool_calls|>",
    "[TOOL_CALLS]",
];

/// Whether `text` carries tool-call markup.
pub fn has_tool_markup(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    BLOCK_TAGS
        .iter()
        .any(|t| lower.contains(&format!("<{t}>")) || lower.contains(&format!("<{t} ")))
        || lower.contains("<function=")
        || MARKERS
            .iter()
            .any(|m| lower.contains(&m.to_ascii_lowercase()))
}

/// `text` with every markup element removed — from its opening tag to its
/// closing tag, or to the end when unclosed — and the prose around it
/// kept; and whether anything was removed.
pub fn strip_tool_markup(text: &str) -> (String, bool) {
    if !has_tool_markup(text) {
        return (text.to_string(), false);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        // The earliest opener of any kind.
        let lower = rest.to_ascii_lowercase();
        let mut first: Option<(usize, usize)> = None; // (start, end of the element)
        for tag in BLOCK_TAGS {
            for opener in [format!("<{tag}>"), format!("<{tag} ")] {
                if let Some(start) = lower.find(&opener) {
                    let close = format!("</{tag}>");
                    let end = lower[start..]
                        .find(&close)
                        .map(|i| start + i + close.len())
                        .unwrap_or(rest.len());
                    if first.is_none_or(|(s, _)| start < s) {
                        first = Some((start, end));
                    }
                }
            }
        }
        if let Some(start) = lower.find("<function=") {
            let end = lower[start..]
                .find("</function>")
                .map(|i| start + i + "</function>".len())
                .unwrap_or(rest.len());
            if first.is_none_or(|(s, _)| start < s) {
                first = Some((start, end));
            }
        }
        for m in MARKERS {
            if let Some(start) = lower.find(&m.to_ascii_lowercase()) {
                // A marker opens a call that runs to the end of the text
                // (or to the next blank line).
                let after = &rest[start + m.len()..];
                let end = after
                    .find("\n\n")
                    .map(|i| start + m.len() + i)
                    .unwrap_or(rest.len());
                if first.is_none_or(|(s, _)| start < s) {
                    first = Some((start, end));
                }
            }
        }
        let Some((start, end)) = first else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        rest = &rest[end..];
    }
    // Stray closers of a block cut at the front, and the blank lines the
    // cut left behind.
    for tag in BLOCK_TAGS {
        out = out.replace(&format!("</{tag}>"), "");
    }
    let mut cleaned: String = out
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    while cleaned.contains("\n\n\n") {
        cleaned = cleaned.replace("\n\n\n", "\n\n");
    }
    (cleaned.trim().to_string(), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invoke_markup_is_cut_and_the_prose_around_it_kept() {
        let raw = "I'll list the folder first.\n\n<function_calls>\n<invoke name=\"bash\">\n<parameter name=\"command\">ls -la</parameter>\n</invoke>\n</function_calls>\n\nThen I'll read the README.";
        let (clean, had) = strip_tool_markup(raw);
        assert!(had);
        assert_eq!(
            clean,
            "I'll list the folder first.\n\nThen I'll read the README."
        );
        // Unclosed at the end of the reply (the model was cut off).
        let (clean, had) = strip_tool_markup(
            "Running it now:\n<invoke name=\"bash\">\n<parameter name=\"command\">pytest -q",
        );
        assert!(had);
        assert_eq!(clean, "Running it now:");
        // Hermes / Qwen shape, and Llama's marker.
        let (clean, had) = strip_tool_markup(
            "<tool_call>\n{\"name\": \"read\", \"arguments\": {\"path\": \"a.py\"}}\n</tool_call>",
        );
        assert!(had);
        assert_eq!(clean, "");
        let (clean, had) =
            strip_tool_markup("Let me check.<|python_tag|>bash.call(command=\"ls\")");
        assert!(had);
        assert_eq!(clean, "Let me check.");
        let (clean, had) = strip_tool_markup("<function=grep>{\"pattern\": \"TODO\"}</function>");
        assert!(had && clean.is_empty());
    }

    #[test]
    fn ordinary_prose_and_code_are_left_alone() {
        for text in [
            "Use `invoke` from the CLI to run it: `arbos-kernel invoke root`.",
            "The <b>bold</b> tag is HTML, not a call.",
            "```rust\nfn call() { invoke(1) }\n```",
            "The function_calls table in the schema has three columns.",
            "Reply with exactly: typed during the cut.",
        ] {
            let (clean, had) = strip_tool_markup(text);
            assert!(!had, "{text}");
            assert_eq!(clean, text);
        }
    }
}
