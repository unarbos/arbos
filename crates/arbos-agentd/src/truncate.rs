//! Nothing shown to the agent is cut silently. Every place agentd shortens a chat message or
//! a window of them says so with a marker that names what is missing and how to fetch the
//! rest (Jacob, 2026-10-26: Hezbola got a code post cut at `def __i` and did not know).
//!
//! The fetch tool named in the markers is `chat_messages` (`mcp.rs`): one message in full by
//! id, or a page of the chat's history around / before an id.

/// Per-message cap in the "recent messages" window (a few thousand chars: a code post fits).
pub const RECENT_MSG_CHARS: usize = 3000;
/// Total budget of the recent-messages window; older lines fall off first, with a marker.
pub const RECENT_WINDOW_CHARS: usize = 16_000;
/// A message passed "in full" (the one that @mentions the agent, the one being replied to) is
/// still capped here, with a marker: beyond this the agent fetches it with the tool.
pub const FULL_MSG_CHARS: usize = 20_000;

/// The exact call that returns one message in full.
pub fn fetch_message_hint(chat_id: &str, message_id: &str) -> String {
    format!("chat_messages {{\"chat_id\": \"{chat_id}\", \"message_id\": \"{message_id}\"}}")
}

/// The exact call that pages the history older than `before_id`.
pub fn fetch_older_hint(chat_id: &str, before_id: &str) -> String {
    format!(
        "chat_messages {{\"chat_id\": \"{chat_id}\", \"before\": \"{before_id}\", \"limit\": 20}}"
    )
}

/// `text` cut to `max` chars with an explicit marker; `fetch` (when known) tells the agent how
/// to read the rest. Untouched when it fits.
pub fn clip_marked(text: &str, max: usize, message_id: &str, fetch: Option<&str>) -> String {
    let n = text.chars().count();
    if n <= max {
        return text.to_owned();
    }
    let head: String = text.chars().take(max).collect();
    let how = match fetch {
        Some(f) => format!("; full text: {f}"),
        None => String::new(),
    };
    let id = if message_id.is_empty() {
        String::new()
    } else {
        format!("; message id {message_id}")
    };
    format!("{head}\n… [truncated: showing {max} of {n} chars{id}{how}]")
}

/// Marker for a window that dropped its oldest `n` messages.
pub fn omitted_marker(n: usize, chat_id: &str, oldest_shown_id: &str) -> String {
    let how = if oldest_shown_id.is_empty() {
        String::new()
    } else {
        format!(
            "; older history: {}",
            fetch_older_hint(chat_id, oldest_shown_id)
        )
    };
    format!(
        "[{n} earlier message{} not shown{how}]",
        if n == 1 { "" } else { "s" }
    )
}

/// One line telling the agent that the window is a cut and how to read any message whole.
pub fn window_note(chat_id: &str) -> String {
    format!(
        "(Messages above are the most recent ones, each cut at {RECENT_MSG_CHARS} chars where marked; read any message in full with {} and page older history with {}.)",
        fetch_message_hint(chat_id, "<message id>"),
        fetch_older_hint(chat_id, "<oldest shown message id>")
    )
}

/// One earlier message of a shared chat, before budgeting.
pub struct WindowLine {
    pub id: String,
    pub from: String,
    pub text: String,
    /// Passed whole (up to [`FULL_MSG_CHARS`]) whatever its position: the message that
    /// @mentions this agent, or the one the current message replies to.
    pub full: bool,
}

/// The window the agent (and the respond decision) sees: the newest `max_lines` lines within
/// [`RECENT_WINDOW_CHARS`], each cut at [`RECENT_MSG_CHARS`] (or [`FULL_MSG_CHARS`] when
/// `full`) with a marker that names the message id and the fetch call. Returns the lines
/// oldest first as `{id, from, text}` plus the count of older messages left out; when any were,
/// the first entry is a `{note}` marker that says how to page them.
pub fn build_window(
    chat_id: &str,
    lines: Vec<WindowLine>,
    max_lines: usize,
) -> (Vec<serde_json::Value>, usize) {
    let total = lines.len();
    let mut kept: Vec<(String, String, String)> = Vec::new();
    let mut budget = RECENT_WINDOW_CHARS;
    for l in lines.into_iter().rev() {
        if kept.len() >= max_lines {
            break;
        }
        let cap = if l.full {
            FULL_MSG_CHARS
        } else {
            RECENT_MSG_CHARS
        };
        let text = clip_marked(
            &l.text,
            cap,
            &l.id,
            Some(&fetch_message_hint(chat_id, &l.id)),
        );
        let cost = text.chars().count();
        // A full message always fits (it is the point of the window); otherwise stop at the budget.
        if cost > budget && !kept.is_empty() && !l.full {
            break;
        }
        budget = budget.saturating_sub(cost);
        kept.push((l.id, l.from, text));
    }
    kept.reverse();
    let omitted = total - kept.len();
    let mut out = Vec::with_capacity(kept.len() + 1);
    if omitted > 0 {
        let oldest = kept.first().map(|k| k.0.as_str()).unwrap_or("");
        out.push(serde_json::json!({ "note": omitted_marker(omitted, chat_id, oldest) }));
    }
    for (id, from, text) in kept {
        out.push(serde_json::json!({ "id": id, "from": from, "text": text }));
    }
    (out, omitted)
}

/// Render window lines for a prompt prelude: `- from: text`, markers as their own lines.
pub fn render_window(lines: &[serde_json::Value]) -> String {
    let mut s = String::new();
    for l in lines {
        if let Some(n) = l.get("note").and_then(serde_json::Value::as_str) {
            s.push_str("- ");
            s.push_str(n);
            s.push('\n');
            continue;
        }
        let from = l
            .get("from")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("?");
        let text = l
            .get("text")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let id = l
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        s.push_str(&format!(
            "- {from} (message id {id}): {}\n",
            text.replace('\n', "\n  ")
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_marks_the_cut_and_names_the_fetch() {
        let long = "x".repeat(50);
        let out = clip_marked(
            &long,
            10,
            "01MSG",
            Some(&fetch_message_hint("01CHAT", "01MSG")),
        );
        assert!(out.starts_with(&"x".repeat(10)));
        assert!(out.contains("[truncated: showing 10 of 50 chars; message id 01MSG; full text: chat_messages {\"chat_id\": \"01CHAT\", \"message_id\": \"01MSG\"}]"));
        assert_eq!(clip_marked("short", 10, "id", None), "short");
        assert!(clip_marked(&long, 10, "", None).ends_with("[truncated: showing 10 of 50 chars]"));
    }

    #[test]
    fn window_keeps_full_lines_whole_and_marks_the_rest() {
        let code = format!(
            "def __init__(self):\n{}\n    return tail_marker_XYZ",
            "    x = 1\n".repeat(600)
        );
        assert!(code.chars().count() > RECENT_MSG_CHARS);
        let lines = vec![
            WindowLine {
                id: "m1".into(),
                from: "a".into(),
                text: "old".into(),
                full: false,
            },
            WindowLine {
                id: "m2".into(),
                from: "b".into(),
                text: code.clone(),
                full: false,
            },
            WindowLine {
                id: "m3".into(),
                from: "c".into(),
                text: code.clone(),
                full: true,
            },
        ];
        let (out, omitted) = build_window("01CHAT", lines, 2);
        assert_eq!(omitted, 1);
        assert!(out[0]["note"].as_str().unwrap().starts_with("[1 earlier message not shown; older history: chat_messages {\"chat_id\": \"01CHAT\", \"before\": \"m2\""));
        let cut = out[1]["text"].as_str().unwrap();
        assert!(cut.contains("[truncated: showing 3000 of"));
        assert!(cut.contains("message id m2; full text: chat_messages {\"chat_id\": \"01CHAT\", \"message_id\": \"m2\"}"));
        assert!(!cut.contains("tail_marker_XYZ"));
        let whole = out[2]["text"].as_str().unwrap();
        assert!(
            whole.ends_with("tail_marker_XYZ"),
            "a full line is passed whole"
        );
        let rendered = render_window(&out);
        assert!(rendered.starts_with("- [1 earlier message not shown"));
        assert!(rendered.contains("- c (message id m3): def __init__"));
    }

    #[test]
    fn omitted_marker_counts_and_pages() {
        assert_eq!(
            omitted_marker(1, "01CHAT", "01OLD"),
            "[1 earlier message not shown; older history: chat_messages {\"chat_id\": \"01CHAT\", \"before\": \"01OLD\", \"limit\": 20}]"
        );
        assert!(omitted_marker(7, "01CHAT", "").starts_with("[7 earlier messages not shown]"));
    }
}
