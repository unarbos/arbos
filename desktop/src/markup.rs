//! Tool-call markup a model wrote as prose, cut from what the window shows.
//!
//! The kernel strips it from the settled `assistant` line (#278), so the
//! transcript never keeps it; but the live `assistant_delta` stream carries
//! the raw tokens as they arrive, and Cursor never shows its own tool
//! markup as prose. So the streamed body is filtered the same way here —
//! from the opening tag to its close, or to the end while the close has not
//! arrived — and the settled line replaces it when the step ends.
//!
//! The same families the kernel knows: Anthropic's `<function_calls>` /
//! `<invoke …>`, Hermes and Qwen's `<tool_call>`, `<function_call>`,
//! `<function=name>`, Llama's `<|python_tag|>` / `<|tool_call|>`, and
//! Mistral's `[TOOL_CALLS]`.

/// An element: cut from the opener to the first of the closers, or to the
/// end of the text when none has arrived.
const ELEMENTS: &[(&str, &[&str])] = &[
    ("<function_calls>", &["</function_calls>"]),
    ("<invoke", &["</invoke>", "</function_calls>"]),
    ("<tool_calls>", &["</tool_calls>"]),
    ("<tool_call>", &["</tool_call>"]),
    ("<function_call>", &["</function_call>"]),
    ("<function=", &["</function>"]),
];

/// A marker: everything from it to the end of the text is the call.
const MARKERS: &[&str] = &["<|python_tag|>", "<|tool_call|>", "[TOOL_CALLS]"];

/// `text` without the tool-call markup, as the live stream should show it.
/// Also drops an opener still arriving at the very end (`<inv`), so the
/// tag never flickers in before it is whole.
pub fn strip_live(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let Some((at, opener)) = first_opener(rest) else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..at]);
        let after = &rest[at + opener.len()..];
        rest = match closers_for(opener) {
            Some(closers) => match closers
                .iter()
                .filter_map(|closer| after.find(closer).map(|end| end + closer.len()))
                .min()
            {
                Some(end) => &after[end..],
                None => "",
            },
            None => "",
        };
    }
    let trimmed = out.trim_end();
    let cut = partial_opener_len(trimmed);
    tidy(&trimmed[..trimmed.len() - cut])
}

fn first_opener(text: &str) -> Option<(usize, &'static str)> {
    ELEMENTS
        .iter()
        .map(|(opener, _)| *opener)
        .chain(MARKERS.iter().copied())
        .filter_map(|opener| text.find(opener).map(|at| (at, opener)))
        .min_by_key(|(at, _)| *at)
}

fn closers_for(opener: &str) -> Option<&'static [&'static str]> {
    ELEMENTS
        .iter()
        .find(|(o, _)| *o == opener)
        .map(|(_, closers)| *closers)
}

/// Bytes at the end of `text` that are the start of an opener not yet
/// whole: `<`, `<inv`, `<function_c`, `<|py`, `[TOOL`.
fn partial_opener_len(text: &str) -> usize {
    let tail_start = text
        .char_indices()
        .rev()
        .take(24)
        .map(|(i, _)| i)
        .last()
        .unwrap_or(0);
    for at in text[tail_start..]
        .char_indices()
        .map(|(i, _)| tail_start + i)
    {
        let tail = &text[at..];
        if tail.len() < 2 {
            if tail == "<" || tail == "[" {
                return tail.len();
            }
            continue;
        }
        let starts_one = ELEMENTS
            .iter()
            .map(|(opener, _)| *opener)
            .chain(MARKERS.iter().copied())
            .any(|opener| opener.len() > tail.len() && opener.starts_with(tail));
        if starts_one {
            return tail.len();
        }
    }
    0
}

/// Collapse the blank lines a cut left behind and trim the end.
fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = 0;
    for line in text.lines() {
        // The kernel speaks to the model in lines it marks `[kernel] …`
        // (an ask's answer, a nudge). A model that repeats one in its own
        // reply puts a machine's aside in a person's paragraph (F-211,
        // d20: *[kernel] The user provided the following answer to your
        // question: red*, twice). Cursor never shows such a line.
        if line.trim_start().starts_with("[kernel]") {
            continue;
        }
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::strip_live;

    #[test]
    fn markup_after_prose_is_cut_to_the_end_while_open() {
        let live = "I'll list the folder first.\n\n<function_calls>\n<invoke name=\"bash\">\n<parameter name=\"cmd\">ls</par";
        assert_eq!(strip_live(live), "I'll list the folder first.");
    }

    #[test]
    fn a_closed_element_leaves_the_prose_around_it() {
        let text = "Before.\n<tool_call>{\"name\":\"ls\"}</tool_call>\nAfter.";
        // The cut line leaves one paragraph break, never a run of them.
        assert_eq!(strip_live(text), "Before.\n\nAfter.");
    }

    #[test]
    fn markup_only_is_nothing() {
        assert_eq!(strip_live("<invoke name=\"bash\">"), "");
        assert_eq!(strip_live("[TOOL_CALLS] [{\"name\": \"ls\"}]"), "");
    }

    #[test]
    fn an_opener_still_arriving_does_not_flicker_in() {
        assert_eq!(strip_live("Sure. <inv"), "Sure.");
        assert_eq!(strip_live("Sure. <function_c"), "Sure.");
        assert_eq!(strip_live("Sure. <"), "Sure.");
    }

    #[test]
    fn prose_html_and_code_are_left_alone() {
        assert_eq!(
            strip_live("Call `invoke()` on the <b>client</b>."),
            "Call `invoke()` on the <b>client</b>."
        );
        assert_eq!(strip_live("a < b and b > c"), "a < b and b > c");
    }
}
