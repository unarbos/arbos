//! Reply-to context: when a person replies to one message (`replyTo` on their row), the quoted
//! message rides in front of their words in the **user** turn — never in the system prompt, so
//! the byte-stable prefix (D87) and its prompt cache stay intact.

use arbos_core::doc::{ChatDoc, MessageView, ReplyRef};
use serde_json::Value;

/// Longer quoted messages are cut to this many characters (whole words) plus an ellipsis.
pub const EXCERPT_CHARS: usize = 480;

/// Who this agent is: its display name and the `agent:<project_id>` sender its rows carry.
pub struct Me<'a> {
    pub name: &'a str,
    pub project_id: &'a str,
}

/// `[Replying to <who>: "<text>"]` for the message `msg` answers, or `None` when it answers
/// nothing or the quoted row is gone. A cross-chat reply ("Reply in DM", `replyRef`) quotes
/// the snapshot the row carries and names the chat it came from, since that row is not in
/// this doc.
pub fn quoted_message_note(doc: &ChatDoc, msg: &MessageView, me: &Me<'_>) -> Option<String> {
    if let Some(r) = ReplyRef::from_row(&msg.rest) {
        return Some(render_cross_chat_note(&r, me));
    }
    let reply_to = msg.rest.get("replyTo").and_then(Value::as_str)?;
    let quoted = doc.message(reply_to)?;
    Some(render_note(&quoted, msg, me))
}

fn render_cross_chat_note(r: &ReplyRef, me: &Me<'_>) -> String {
    let mine = r.sender == "agent" || r.sender == format!("agent:{}", me.project_id);
    let who = if mine {
        format!("your earlier message (you, {})", me.name)
    } else if r.sender.starts_with("agent") {
        format!("{}'s message (another agent)", r.who())
    } else {
        format!("{}'s message", r.who())
    };
    let place = if r.chat_title.trim().is_empty() {
        "another chat".to_owned()
    } else {
        format!("the chat \"{}\"", r.chat_title.trim())
    };
    let (body, cut) = excerpt(
        if r.text.trim().is_empty() {
            "(no text)"
        } else {
            r.text.trim()
        },
        EXCERPT_CHARS,
    );
    let tail = if cut {
        format!(
            " [truncated: showing {EXCERPT_CHARS} of {} chars; the full message is in that chat, not here]",
            r.text.split_whitespace().collect::<Vec<_>>().join(" ").chars().count()
        )
    } else {
        String::new()
    };
    format!(
        "[Replying here, privately, to {who} in {place}: \"{body}\"{tail}. That message is not in this chat; answer with it in mind.]\n\n"
    )
}

fn render_note(quoted: &MessageView, replying: &MessageView, me: &Me<'_>) -> String {
    let who = quoted_author(quoted, replying, me);
    // The message being replied to travels whole (Jacob, 2026-10-26): a code post cut to an
    // excerpt is exactly what the agent cannot answer about. Beyond the full cap the marker
    // names the fetch call.
    let text = quoted_text(quoted);
    let chat_id = replying
        .rest
        .get("chatId")
        .and_then(Value::as_str)
        .unwrap_or("");
    let body = crate::truncate::clip_marked(
        text.trim(),
        crate::truncate::FULL_MSG_CHARS,
        &quoted.id,
        Some(&crate::truncate::fetch_message_hint(chat_id, &quoted.id)),
    );
    let body = if body.contains('\n') {
        format!("\n<<<\n{body}\n>>>\n")
    } else {
        format!("\"{body}\"")
    };
    format!(
        "[Replying to {who}: {body} (message id {}). Answer with that message in mind; you can see it.]\n\n",
        quoted.id
    )
}

/// The quoted row is one of this agent's own: `sender: agent` (solo chat) or
/// `agent:<my project>` (shared chat), on an assistant / project row.
pub fn quoted_is_mine(quoted: &MessageView, me: &Me<'_>) -> bool {
    let sender = quoted
        .rest
        .get("sender")
        .and_then(Value::as_str)
        .unwrap_or("");
    matches!(quoted.role.as_str(), "assistant" | "project")
        && (sender == "agent" || sender.is_empty() || sender == format!("agent:{}", me.project_id))
}

/// "your earlier message", "Jacob's message", "@ops's message" (another agent), …
fn quoted_author(quoted: &MessageView, replying: &MessageView, me: &Me<'_>) -> String {
    let sender = quoted
        .rest
        .get("sender")
        .and_then(Value::as_str)
        .unwrap_or("");
    let handle = quoted
        .rest
        .get("senderHandle")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|h| !h.is_empty());
    let name = quoted
        .rest
        .get("senderName")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|n| !n.is_empty());
    match quoted.role.as_str() {
        "assistant" | "project" => {
            // Mine: `sender: agent` (solo chat) or `agent:<my project>` (shared chat).
            let mine = sender == "agent"
                || sender.is_empty()
                || sender == format!("agent:{}", me.project_id);
            if mine {
                return format!("your earlier message (you, {})", me.name);
            }
            match (name, handle) {
                (Some(n), _) => format!("{n}'s message (another agent)"),
                (None, Some(h)) => format!("@{h}'s message (another agent)"),
                (None, None) => "another agent's message".to_owned(),
            }
        }
        "system" => "a system line".to_owned(),
        _ => {
            let same_person = sender
                == replying
                    .rest
                    .get("sender")
                    .and_then(Value::as_str)
                    .unwrap_or("user");
            match (same_person, name, handle) {
                (true, Some(n), _) => format!("{n}'s own earlier message"),
                (true, None, Some(h)) => format!("@{h}'s own earlier message"),
                (true, None, None) => "the user's own earlier message".to_owned(),
                (false, Some(n), _) => format!("{n}'s message"),
                (false, None, Some(h)) => format!("@{h}'s message"),
                (false, None, None) => "another message".to_owned(),
            }
        }
    }
}

/// The quoted row's prose; an attachment-only row names its files.
fn quoted_text(quoted: &MessageView) -> String {
    let text = quoted.text();
    let text = text.trim();
    if !text.is_empty() {
        return text.to_owned();
    }
    let files: Vec<&str> = quoted
        .parts
        .iter()
        .filter(|p| p.get("kind").and_then(Value::as_str) == Some("attachment"))
        .filter_map(|p| p.pointer("/attachment/name").and_then(Value::as_str))
        .collect();
    if files.is_empty() {
        "(no text)".to_owned()
    } else {
        format!("(attachment: {})", files.join(", "))
    }
}

/// First `max` characters on a word boundary, whitespace collapsed; `true` when it was cut.
fn excerpt(text: &str, max: usize) -> (String, bool) {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return (flat, false);
    }
    let mut cut: String = flat.chars().take(max).collect();
    if let Some(ix) = cut.rfind(' ') {
        if ix > max / 2 {
            cut.truncate(ix);
        }
    }
    (
        format!("{}…", cut.trim_end_matches([',', ';', ':', '.'])),
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const BOB: Me<'static> = Me {
        name: "Bob",
        project_id: "01P1",
    };

    fn view(id: &str, role: &str, text: &str, rest: Value) -> MessageView {
        MessageView {
            id: id.to_owned(),
            role: role.to_owned(),
            status: "done".to_owned(),
            parts: vec![json!({ "kind": "text", "text": text })],
            rest,
        }
    }

    /// The quoted text and its author reach the user turn.
    #[test]
    fn note_names_author_and_quotes_text() {
        let agent = view(
            "a1",
            "assistant",
            "The latest commit is 7ba9601d8.",
            json!({ "sender": "agent" }),
        );
        let reply = view(
            "u2",
            "user",
            "What time was it?",
            json!({ "sender": "user", "senderName": "Jacob", "replyTo": "a1" }),
        );
        let note = render_note(&agent, &reply, &BOB);
        assert!(note.starts_with(
            "[Replying to your earlier message (you, Bob): \"The latest commit is 7ba9601d8.\""
        ));
        assert!(note.ends_with("]\n\n"));

        let other = view(
            "u1",
            "user",
            "Launch is Friday",
            json!({ "sender": "user:gh_2", "senderHandle": "sam", "senderName": "Sam Okafor" }),
        );
        let reply = view(
            "u3",
            "user",
            "Which Friday?",
            json!({ "sender": "user:gh_1", "senderHandle": "jacob", "replyTo": "u1" }),
        );
        assert!(render_note(&other, &reply, &BOB)
            .starts_with("[Replying to Sam Okafor's message: \"Launch is Friday\""));

        let own = view(
            "u1",
            "user",
            "Remind me tomorrow",
            json!({ "sender": "user", "senderName": "Jacob" }),
        );
        let reply = view(
            "u4",
            "user",
            "Make that Thursday",
            json!({ "sender": "user", "senderName": "Jacob", "replyTo": "u1" }),
        );
        assert!(render_note(&own, &reply, &BOB)
            .starts_with("[Replying to Jacob's own earlier message: \"Remind me tomorrow\""));
    }

    /// Long quotes are cut on a word boundary and flagged as an excerpt.
    #[test]
    fn long_quote_becomes_excerpt() {
        let long = "word ".repeat(300);
        let (body, cut) = excerpt(&long, EXCERPT_CHARS);
        assert!(cut);
        assert!(body.ends_with('…'));
        assert!(body.chars().count() <= EXCERPT_CHARS + 1);
        assert!(!body.contains("  "));
        let (short, cut) = excerpt("  two\n lines ", 40);
        assert!(!cut);
        assert_eq!(short, "two lines");
    }

    /// An attachment-only quote names the file instead of quoting nothing.
    #[test]
    fn attachment_quote_names_file() {
        let mut m = view("a", "user", "", json!({ "sender": "user" }));
        m.parts = vec![
            json!({ "kind": "attachment", "attachment": { "name": "deck.pdf", "mime": "application/pdf" } }),
        ];
        assert_eq!(quoted_text(&m), "(attachment: deck.pdf)");
    }

    /// Another agent's row in a shared chat is credited to its handle, not to me.
    #[test]
    fn other_agent_is_named_by_handle() {
        let theirs = view(
            "a9",
            "assistant",
            "I can take the deploy.",
            json!({ "sender": "agent:01P2", "senderHandle": "ops" }),
        );
        let reply = view(
            "u5",
            "user",
            "Do it",
            json!({ "sender": "user:gh_1", "replyTo": "a9", "mentions": ["agent:01P1"] }),
        );
        assert!(render_note(&theirs, &reply, &BOB)
            .starts_with("[Replying to @ops's message (another agent):"));
        let mine = view(
            "a8",
            "assistant",
            "Done.",
            json!({ "sender": "agent:01P1", "senderHandle": "bob" }),
        );
        assert!(render_note(&mine, &reply, &BOB)
            .starts_with("[Replying to your earlier message (you, Bob):"));
    }

    /// Reply in DM (reply-in-dm-context-2026-10-25): a `replyRef` row quotes the snapshot and
    /// names the source chat, without needing that chat's doc; it wins over a stale `replyTo`.
    #[test]
    fn cross_chat_reply_quotes_the_snapshot_and_names_the_chat() {
        let doc = ChatDoc::new(None, false);
        let reply = view(
            "d1",
            "user",
            "Works",
            json!({
                "sender": "user:gh_2", "senderName": "Bob",
                "replyTo": "not-in-this-doc",
                "replyRef": {
                    "chatId": "01GROUP", "projectId": "people", "messageId": "01ORIG",
                    "chatTitle": "Arbonites", "sender": "user:gh_1",
                    "senderName": "Jacob", "senderHandle": "unconst",
                    "text": "Testing reply in dm"
                }
            }),
        );
        let note = quoted_message_note(&doc, &reply, &BOB).expect("note");
        assert!(
            note.starts_with(
                "[Replying here, privately, to Jacob's message in the chat \"Arbonites\": \"Testing reply in dm\""
            ),
            "{note}"
        );
        // My own quoted row is named as mine; another agent's as another agent's.
        let mine = ReplyRef {
            sender: "agent:01P1".into(),
            text: "Done.".into(),
            ..ReplyRef::default()
        };
        assert!(render_cross_chat_note(&mine, &BOB)
            .contains("to your earlier message (you, Bob) in another chat"));
        let theirs = ReplyRef {
            sender: "agent:01P2".into(),
            sender_handle: "ops".into(),
            ..ReplyRef::default()
        };
        assert!(render_cross_chat_note(&theirs, &BOB).contains("to @ops's message (another agent)"));
    }
}
