//! MCP tools `chat_read` and `chat_search`: the full people/group transcript already on this
//! box's room doc. Scoped to the chat of the current turn; project chats keep
//! `history_search` / `history_read` / `read_thread` / `chat_messages`.

use std::sync::Arc;

use serde_json::{json, Value};

use arbos_core::doc::MessageView;

use crate::agent::Agent;
use crate::mcp::{err_result, text_result};

const DEFAULT_LAST: i64 = 50;
const MAX_LAST: i64 = 500;
const DEFAULT_SEARCH_LIMIT: i64 = 50;
const MAX_SEARCH_LIMIT: i64 = 500;

const PROJECT_CHAT_HINT: &str = "chat_read and chat_search are for people and group chats only; \
in a project chat use history_search, history_read, or read_thread";

/// MCP tool specs for the shared people/group transcript.
pub fn tool_specs() -> Vec<Value> {
    vec![
        json!({
            "name": "chat_read",
            "description": "Read the people/group chat of this turn from the live room doc (full text, oldest first). Default the newest 50 rows (max 500). Page older history with `before_id` (or the returned `next_cursor`), newer with `after_id`; optional `since_ms` / `until_ms`. Use for summaries, catch-ups and counts — do not guess from the short recent window. For a large range, page (or hand pages to a subagent) rather than reading everything at once.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "last": { "type": "integer", "minimum": 1, "maximum": 500 },
                    "before_id": { "type": "string" },
                    "after_id": { "type": "string" },
                    "since_ms": { "type": "integer" },
                    "until_ms": { "type": "integer" }
                }
            }
        }),
        json!({
            "name": "chat_search",
            "description": "Search the people/group chat of this turn in the live room doc. Optional `query` (substring of the text), `from` (sender handle or name, case-insensitive), `since_ms` / `until_ms`, `limit` (default 50, max 500). Matching rows newest first, same shape as chat_read. Use for summaries, catch-ups and counts.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "from": { "type": "string" },
                    "since_ms": { "type": "integer" },
                    "until_ms": { "type": "integer" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 500 }
                }
            }
        }),
    ]
}

pub async fn call_tool(a: &Arc<Agent>, name: &str, args: &Value, chat_id: &str) -> Value {
    match name {
        "chat_read" => chat_read(a, args, chat_id).await,
        "chat_search" => chat_search(a, args, chat_id).await,
        _ => err_result(format!("unknown tool {name}")),
    }
}

/// One line for people/group preludes: point at these tools instead of guessing.
pub fn shared_chat_tools_note() -> &'static str {
    "For summaries, catch-ups, or counts across this chat, use chat_read and chat_search — do not guess from the recent messages above. For a large range, page with the cursor (or hand pages to a subagent) rather than reading everything at once."
}

async fn load_shared_rows(a: &Arc<Agent>, chat_id: &str) -> Result<Vec<MessageView>, String> {
    if chat_id.trim().is_empty() || chat_id == "shared" {
        return Err("no current people or group chat on this turn".into());
    }
    let is_people = a.people.is_people_chat(chat_id);
    let is_group = a.groups.kind_of(chat_id).as_deref() == Some("group");
    if !is_people && !is_group {
        let Some(_) = a.sync.host(chat_id).await else {
            return Err(format!(
                "this agent is not a member of chat {chat_id} (or the room is not open on this box)"
            ));
        };
        return Err(PROJECT_CHAT_HINT.into());
    }
    let Some(host) = a.sync.host(chat_id).await else {
        return Err(format!(
            "this agent is not a member of chat {chat_id} (or the room is not open on this box)"
        ));
    };
    let all = { host.lock().await.doc.messages() };
    Ok(all
        .into_iter()
        .filter(|m| matches!(m.role.as_str(), "user" | "assistant" | "project"))
        .filter(|m| !m.text().trim().is_empty())
        .collect())
}

fn created_at(m: &MessageView) -> i64 {
    m.rest.get("createdAt").and_then(Value::as_i64).unwrap_or(0)
}

fn in_time_range(m: &MessageView, since_ms: Option<i64>, until_ms: Option<i64>) -> bool {
    let at = created_at(m);
    if let Some(s) = since_ms {
        if at < s {
            return false;
        }
    }
    if let Some(u) = until_ms {
        if at > u {
            return false;
        }
    }
    true
}

fn sender_handle(m: &MessageView) -> String {
    m.rest
        .get("senderHandle")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            m.rest.get("sender").and_then(Value::as_str).map(|s| {
                s.strip_prefix("user:")
                    .or_else(|| s.strip_prefix("agent:"))
                    .or_else(|| s.strip_prefix("project:"))
                    .unwrap_or(s)
                    .to_owned()
            })
        })
        .unwrap_or_else(|| {
            if m.role == "assistant" || m.role == "project" {
                "agent".into()
            } else {
                "user".into()
            }
        })
}

fn sender_name(m: &MessageView) -> String {
    m.rest
        .get("senderName")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| sender_handle(m))
}

fn row_role(m: &MessageView) -> &'static str {
    match m.role.as_str() {
        "assistant" | "project" => "agent",
        _ => "person",
    }
}

fn attachment_names(m: &MessageView) -> Vec<String> {
    m.parts
        .iter()
        .filter(|p| p.get("kind").and_then(Value::as_str) == Some("attachment"))
        .filter_map(|p| {
            p.get("attachment")
                .and_then(|a| a.get("name"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect()
}

fn row_json(m: &MessageView) -> Value {
    json!({
        "id": m.id,
        "sender": {
            "handle": sender_handle(m),
            "name": sender_name(m),
        },
        "role": row_role(m),
        "time": created_at(m),
        "text": m.text(),
        "attachments": attachment_names(m),
        "reply_to": m.rest.get("replyTo").and_then(Value::as_str),
    })
}

fn arg_i64(args: &Value, key: &str) -> Option<i64> {
    args.get(key).and_then(Value::as_i64)
}

fn arg_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

async fn chat_read(a: &Arc<Agent>, args: &Value, chat_id: &str) -> Value {
    let rows = match load_shared_rows(a, chat_id).await {
        Ok(r) => r,
        Err(e) => return err_result(e),
    };
    let since_ms = arg_i64(args, "since_ms");
    let until_ms = arg_i64(args, "until_ms");
    let last = arg_i64(args, "last")
        .unwrap_or(DEFAULT_LAST)
        .clamp(1, MAX_LAST) as usize;
    let before_id = arg_str(args, "before_id").or_else(|| arg_str(args, "cursor"));
    let after_id = arg_str(args, "after_id");

    let visible: Vec<&MessageView> = rows
        .iter()
        .filter(|m| in_time_range(m, since_ms, until_ms))
        .collect();

    let (slice, has_older): (Vec<&MessageView>, bool) = if let Some(after) = after_id {
        let start = visible
            .iter()
            .position(|m| m.id == after)
            .map(|i| i + 1)
            .unwrap_or(0);
        let end = (start + last).min(visible.len());
        (visible[start..end].to_vec(), start > 0)
    } else {
        let end = match before_id {
            Some(b) => visible
                .iter()
                .position(|m| m.id == b)
                .unwrap_or(visible.len()),
            None => visible.len(),
        };
        let start = end.saturating_sub(last);
        (visible[start..end].to_vec(), start > 0)
    };

    let next_cursor = if has_older {
        slice.first().map(|m| m.id.clone())
    } else {
        None
    };
    let out: Vec<Value> = slice.iter().map(|m| row_json(m)).collect();
    text_result(
        json!({
            "chat_id": chat_id,
            "messages": out,
            "next_cursor": next_cursor,
            "total_in_chat": visible.len(),
        })
        .to_string(),
    )
}

async fn chat_search(a: &Arc<Agent>, args: &Value, chat_id: &str) -> Value {
    let rows = match load_shared_rows(a, chat_id).await {
        Ok(r) => r,
        Err(e) => return err_result(e),
    };
    let since_ms = arg_i64(args, "since_ms");
    let until_ms = arg_i64(args, "until_ms");
    let limit = arg_i64(args, "limit")
        .unwrap_or(DEFAULT_SEARCH_LIMIT)
        .clamp(1, MAX_SEARCH_LIMIT) as usize;
    let query = arg_str(args, "query").map(|q| q.to_lowercase());
    let from = arg_str(args, "from").map(|f| f.trim_start_matches('@').to_lowercase());

    let mut hits: Vec<&MessageView> = rows
        .iter()
        .filter(|m| in_time_range(m, since_ms, until_ms))
        .filter(|m| {
            if let Some(q) = query.as_deref() {
                if !m.text().to_lowercase().contains(q) {
                    return false;
                }
            }
            if let Some(f) = from.as_deref() {
                let handle = sender_handle(m).to_lowercase();
                let name = sender_name(m).to_lowercase();
                if handle != f && name != f && !handle.contains(f) && !name.contains(f) {
                    return false;
                }
            }
            true
        })
        .collect();
    // Newest first.
    hits.reverse();
    hits.truncate(limit);
    let out: Vec<Value> = hits.iter().map(|m| row_json(m)).collect();
    text_result(
        json!({
            "chat_id": chat_id,
            "messages": out,
            "total_matches": out.len(),
        })
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::people::PeopleChatInfo;
    use crate::status::tests::test_agent;
    use arbos_core::doc::UserMessageOpts;

    fn people_info(agent: &Agent) -> PeopleChatInfo {
        PeopleChatInfo {
            members: vec![
                json!({
                    "member": format!("agent:{}", agent.cfg.project_id),
                    "handle": "doug",
                    "project": { "name": "Doug" },
                    "joined_at": 1,
                }),
                json!({
                    "member": "user:alice",
                    "handle": "alice",
                    "name": "Alice",
                    "role": "owner",
                }),
                json!({
                    "member": "user:bob",
                    "handle": "bob",
                    "name": "Bob",
                    "role": "guest",
                }),
                json!({
                    "member": "user:cara",
                    "handle": "cara",
                    "name": "Cara",
                    "role": "guest",
                }),
            ],
            settings: json!({ "reply_policy": "mention" }),
            owner_user: "alice".into(),
            slug: "doug".into(),
            kind: "group".into(),
            owner_address: Some("Alice".into()),
        }
    }

    fn parse_tool(v: Value) -> Value {
        assert_ne!(v.get("isError"), Some(&json!(true)), "{v}");
        let text = v["content"][0]["text"].as_str().expect("text");
        serde_json::from_str(text).expect(text)
    }

    fn err_text(v: Value) -> String {
        assert_eq!(v.get("isError"), Some(&json!(true)), "{v}");
        v["content"][0]["text"].as_str().unwrap_or("").to_owned()
    }

    async fn seed_300(agent: &Arc<Agent>, chat: &str) {
        agent.people.test_insert(chat, people_info(agent));
        let host = agent.sync.open_room(chat).await.unwrap();
        let senders = [
            ("alice", "Alice", "user:alice"),
            ("bob", "Bob", "user:bob"),
            ("cara", "Cara", "user:cara"),
        ];
        {
            let h = host.lock().await;
            h.doc.transact(|d| {
                for i in 0..300 {
                    let (handle, name, sender) = senders[i % 3];
                    let id = format!("m{i:03}");
                    let text = format!("message {i} from {handle} with full body #{i}");
                    d.append_user_message(
                        &id,
                        &text,
                        "dev:test",
                        &UserMessageOpts {
                            sender: Some(sender.into()),
                            sender_handle: Some(handle.into()),
                            sender_name: Some(name.into()),
                            ..Default::default()
                        },
                    )
                    .unwrap();
                }
            });
        }
    }

    #[tokio::test]
    async fn chat_read_last_50_newest_oldest_first_with_sender_and_full_text() {
        let (agent, root) = test_agent().await;
        let chat = "01CHATREADFIXTURE000000001";
        seed_300(&agent, chat).await;

        let v = parse_tool(chat_read(&agent, &json!({ "last": 50 }), chat).await);
        let msgs = v["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 50);
        assert_eq!(msgs[0]["id"], "m250");
        assert_eq!(msgs[49]["id"], "m299");
        // 250 % 3 == 1 → bob; full text is not cut.
        assert_eq!(msgs[0]["sender"]["handle"], "bob");
        assert_eq!(msgs[0]["sender"]["name"], "Bob");
        assert_eq!(msgs[0]["role"], "person");
        assert_eq!(msgs[0]["text"], "message 250 from bob with full body #250");
        assert_eq!(v["next_cursor"], "m250");
        assert_eq!(v["total_in_chat"], 300);

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn chat_read_paging_cursor_returns_next_50() {
        let (agent, root) = test_agent().await;
        let chat = "01CHATREADFIXTURE000000002";
        seed_300(&agent, chat).await;

        let first = parse_tool(chat_read(&agent, &json!({ "last": 50 }), chat).await);
        let cursor = first["next_cursor"].as_str().unwrap().to_owned();
        let second =
            parse_tool(chat_read(&agent, &json!({ "last": 50, "before_id": cursor }), chat).await);
        let msgs = second["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 50);
        assert_eq!(msgs[0]["id"], "m200");
        assert_eq!(msgs[49]["id"], "m249");
        assert_eq!(second["next_cursor"], "m200");

        let via_cursor =
            parse_tool(chat_read(&agent, &json!({ "last": 50, "cursor": cursor }), chat).await);
        assert_eq!(via_cursor["messages"], second["messages"]);

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn chat_search_from_alice_only_her_rows() {
        let (agent, root) = test_agent().await;
        let chat = "01CHATREADFIXTURE000000003";
        seed_300(&agent, chat).await;

        let v =
            parse_tool(chat_search(&agent, &json!({ "from": "alice", "limit": 500 }), chat).await);
        let msgs = v["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 100);
        assert!(msgs.iter().all(|m| m["sender"]["handle"] == "alice"));
        // Newest first.
        assert_eq!(msgs[0]["id"], "m297");
        assert_eq!(msgs[99]["id"], "m000");

        let by_name =
            parse_tool(chat_search(&agent, &json!({ "from": "Alice", "limit": 5 }), chat).await);
        assert_eq!(by_name["messages"].as_array().unwrap().len(), 5);
        assert!(by_name["messages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["sender"]["name"] == "Alice"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn chat_not_a_member_errors() {
        let (agent, root) = test_agent().await;
        let chat = "01CHATNOTAMEMBER0000000001";
        let err = err_text(chat_read(&agent, &json!({}), chat).await);
        assert!(
            err.contains("not a member") || err.contains("not open"),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn project_chat_errors_pointing_at_existing_tools() {
        let (agent, root) = test_agent().await;
        let chat = "01CHATPROJECTONLY000000001";
        let _ = agent.sync.open_room(chat).await.unwrap();
        let err = err_text(chat_read(&agent, &json!({}), chat).await);
        assert!(err.contains("history_search"), "{err}");
        assert!(err.contains("history_read"), "{err}");
        assert!(err.contains("read_thread"), "{err}");
        let err2 = err_text(chat_search(&agent, &json!({ "query": "x" }), chat).await);
        assert!(err2.contains("history_search"), "{err2}");
        let _ = std::fs::remove_dir_all(root);
    }
}
