//! `arbos-sidecar mcp`: a stdio MCP server (JSON-RPC 2.0). Reads come from the mirror; actions go
//! to the daemon over the control socket. Spec §5.

use anyhow::Result;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::config::{Config, Paths};
use crate::control;
use crate::mirror::Mirror;

pub const PROTOCOL_VERSION: &str = "2025-06-18";

fn obj(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required, "additionalProperties": false })
}

pub fn tools() -> Vec<Value> {
    let chat = json!({ "type": "string", "description": "chat id" });
    let message = json!({ "type": "string", "description": "message id" });
    let text = json!({ "type": "string", "description": "plain prose, up to 16 KB" });
    let t = |name: &str, desc: &str, schema: Value| json!({ "name": name, "description": desc, "inputSchema": schema });
    vec![
        t("arbos_chats", "Your chats: id, kind (agent | dm | group), title, last activity, unanswered count.", obj(json!({}), &[])),
        t("arbos_pending", "Messages that woke you and that you have not answered yet, per chat. Read these first.", obj(json!({}), &[])),
        t("arbos_read_chat", "Rows of one chat, oldest first, from the local mirror. Use before= to page back; has_more says whether older rows exist.",
          obj(json!({ "chat": chat, "limit": { "type": "integer", "minimum": 1, "maximum": 200 }, "before": message }), &["chat"])),
        t("arbos_search", "Find rows whose text or sender matches a phrase (case-insensitive), newest first.",
          obj(json!({ "query": { "type": "string" }, "chat": chat, "limit": { "type": "integer", "minimum": 1, "maximum": 50 } }), &["query"])),
        t("arbos_members", "Who is in a chat: kind (user | agent), name, handle, role, is_me.", obj(json!({ "chat": chat }), &["chat"])),
        t("arbos_brief", "A short rolling summary of a chat (counts and the last five rows).", obj(json!({ "chat": chat }), &["chat"])),
        t("arbos_fetch_file", "Download a picture or file named by attachments[].blob to a local path (needs the files.read switch).",
          obj(json!({ "chat": chat, "blob": { "type": "string" } }), &["chat", "blob"])),
        t("arbos_post", "Say something in a chat.", obj(json!({ "chat": chat, "text": text }), &["chat", "text"])),
        t("arbos_reply", "Answer one specific message (preferred over arbos_post when you were addressed).",
          obj(json!({ "chat": chat, "message": message, "text": text }), &["chat", "message", "text"])),
        t("arbos_react", "Add or remove an emoji reaction on a message.",
          obj(json!({ "chat": chat, "message": message, "emoji": { "type": "string" }, "on": { "type": "boolean" } }), &["chat", "message", "emoji"])),
        t("arbos_typing", "Show or clear \"… is typing\" in a chat (clears by itself after 5 s).", obj(json!({ "chat": chat, "on": { "type": "boolean" } }), &["chat"])),
        t("arbos_status", "Set the working line under the composer, present tense, up to 40 characters (\"reading Stripe\").",
          obj(json!({ "chat": chat, "text": { "type": "string", "maxLength": 40 } }), &["chat", "text"])),
        t("arbos_me", "Who you are, your scopes, and the sidecar's health.", obj(json!({}), &[])),
    ]
}

fn text_result(v: &Value) -> Value {
    json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(v).unwrap_or_default() }] })
}

fn error_result(msg: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": msg }], "isError": true })
}

/// Run one tool. Public so the CLI shares it.
pub async fn call_tool(paths: &Paths, cfg: Option<&Config>, name: &str, args: &Value) -> Value {
    let mirror = Mirror::new(paths.clone());
    let s = |k: &str| args[k].as_str().unwrap_or("").to_owned();
    let stale_ms = cfg
        .map(|c| c.waker.stale_wake_s as i64 * 1000)
        .unwrap_or(6 * 3600 * 1000);
    match name {
        "arbos_chats" => {
            let chats: Vec<Value> = mirror
                .chats()
                .into_iter()
                .map(|c| {
                    let unanswered = mirror
                        .rows(&c.id)
                        .iter()
                        .filter(|r| {
                            r["wake"] == true
                                && r["event_id"].as_i64().unwrap_or(0) > c.answered_through
                        })
                        .count();
                    json!({ "id": c.id, "kind": c.kind, "title": c.title, "last_at": c.last_at, "unanswered": unanswered })
                })
                .collect();
            text_result(&json!(chats))
        }
        "arbos_pending" => text_result(&json!(mirror.pending(stale_ms))),
        "arbos_read_chat" => {
            let chat = s("chat");
            if chat.is_empty() {
                return error_result("chat is required");
            }
            let limit = args["limit"].as_u64().unwrap_or(40).clamp(1, 200) as usize;
            if mirror.rows(&chat).is_empty() {
                let _ = control::call(paths, &json!({ "op": "history", "chat": chat })).await;
            }
            let (rows, has_more) = mirror.read(&chat, limit, args["before"].as_str());
            let before = rows.first().and_then(|r| r["id"].as_str()).map(str::to_owned);
            text_result(&json!({
                "chat": chat, "messages": rows, "has_more": has_more,
                "before": if has_more { before } else { None },
                "note": if has_more { "older rows exist — call again with before=<first id>" } else { "" },
            }))
        }
        "arbos_search" => {
            let q = s("query");
            if q.is_empty() {
                return error_result("query is required");
            }
            let limit = args["limit"].as_u64().unwrap_or(20).clamp(1, 50) as usize;
            text_result(&json!(mirror.search(&q, args["chat"].as_str(), limit)))
        }
        "arbos_members" => {
            let chat = s("chat");
            let cur = mirror.members(&chat);
            let fresh = cur["fetched"]
                .as_i64()
                .is_some_and(|t| crate::mirror::now_ms() - t < 10 * 60 * 1000);
            if !fresh {
                let _ = control::call(paths, &json!({ "op": "members", "chat": chat })).await;
            }
            text_result(&mirror.members(&chat))
        }
        "arbos_brief" => {
            let chat = s("chat");
            let b = mirror.brief(&chat);
            if b.is_empty() {
                let _ = mirror.refresh_brief(&chat);
            }
            text_result(&json!({ "brief": mirror.brief(&chat) }))
        }
        "arbos_fetch_file" => {
            match control::action(paths, &json!({ "op": "file", "chat": s("chat"), "blob": s("blob") })).await {
                Ok(v) => text_result(&v),
                Err(e) => error_result(&e.to_string()),
            }
        }
        "arbos_post" | "arbos_reply" => {
            let op = if name == "arbos_post" { "post" } else { "reply" };
            let text = s("text");
            if text.trim().is_empty() {
                return error_result("text is required");
            }
            match control::action(paths, &json!({ "op": op, "chat": s("chat"), "message": s("message"), "text": text })).await {
                Ok(v) => text_result(&json!({ "message": v["message"], "chat": v["chat"] })),
                Err(e) => error_result(&e.to_string()),
            }
        }
        "arbos_react" => {
            match control::action(paths, &json!({ "op": "react", "chat": s("chat"), "message": s("message"), "emoji": s("emoji"), "on": args["on"].as_bool().unwrap_or(true) })).await {
                Ok(_) => text_result(&json!({ "ok": true })),
                Err(e) => error_result(&e.to_string()),
            }
        }
        "arbos_typing" => {
            match control::action(paths, &json!({ "op": "typing", "chat": s("chat"), "on": args["on"].as_bool().unwrap_or(true) })).await {
                Ok(_) => text_result(&json!({ "ok": true })),
                Err(e) => error_result(&e.to_string()),
            }
        }
        "arbos_status" => {
            match control::action(paths, &json!({ "op": "status", "chat": s("chat"), "text": s("text") })).await {
                Ok(_) => text_result(&json!({ "ok": true })),
                Err(e) => error_result(&e.to_string()),
            }
        }
        "arbos_me" => {
            let state = mirror.state();
            let me = control::call(paths, &json!({ "op": "me" })).await.ok();
            let running = me.as_ref().is_some_and(|v| v["ok"] != false);
            text_result(&json!({
                "agent": me.as_ref().map(|v| v["agent"].clone()).unwrap_or(state.agent),
                "scopes": me.as_ref().map(|v| v["scopes"].clone()).unwrap_or(Value::Null),
                "relay": state.relay,
                "sidecar": {
                    "version": env!("CARGO_PKG_VERSION"),
                    "running": running,
                    "cursor": state.cursor,
                    "last_wake": mirror.last_wake(),
                },
            }))
        }
        _ => error_result(&format!("unknown tool {name}")),
    }
}

fn rpc_result(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: &Value, code: i64, msg: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": msg } })
}

/// Handle one JSON-RPC message; `None` for notifications.
pub async fn handle(paths: &Paths, cfg: Option<&Config>, msg: &Value) -> Option<Value> {
    let method = msg["method"].as_str().unwrap_or("");
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    // Notifications (initialized, cancelled, …) need no answer.
    msg.get("id")?;
    Some(match method {
        "initialize" => rpc_result(
            &id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "arbos-sidecar", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Arbos chat tools. Start with arbos_pending, read the chat, answer with arbos_reply. Reply only where you are addressed; post nothing if you decide not to answer.",
            }),
        ),
        "ping" => rpc_result(&id, json!({})),
        "tools/list" => rpc_result(&id, json!({ "tools": tools() })),
        "tools/call" => {
            let name = msg["params"]["name"].as_str().unwrap_or("");
            let args = msg["params"].get("arguments").cloned().unwrap_or(json!({}));
            rpc_result(&id, call_tool(paths, cfg, name, &args).await)
        }
        "resources/list" => rpc_result(&id, json!({ "resources": [] })),
        "prompts/list" => rpc_result(&id, json!({ "prompts": [] })),
        _ => rpc_error(&id, -32601, &format!("method not found: {method}")),
    })
}

pub async fn serve(paths: Paths, cfg: Option<Config>) -> Result<()> {
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut lines = BufReader::new(stdin).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let err = rpc_error(&Value::Null, -32700, &format!("parse error: {e}"));
                stdout.write_all(format!("{err}\n").as_bytes()).await?;
                continue;
            }
        };
        if let Some(resp) = handle(&paths, cfg.as_ref(), &msg).await {
            stdout.write_all(format!("{resp}\n").as_bytes()).await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mirror::tests::received;

    #[tokio::test]
    async fn initialize_list_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            config_dir: dir.path().join("c"),
            data_dir: dir.path().join("d"),
        };
        paths.ensure().unwrap();
        let m = Mirror::new(paths.clone());
        for i in 1..=3 {
            m.apply_event(&received(
                i,
                "c1",
                &format!("m{i}"),
                &format!("@bot {i}"),
                i == 3,
            ))
            .unwrap();
        }
        let init = handle(
            &paths,
            None,
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
        )
        .await
        .unwrap();
        assert_eq!(init["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(handle(
            &paths,
            None,
            &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
        )
        .await
        .is_none());
        let list = handle(
            &paths,
            None,
            &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        )
        .await
        .unwrap();
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names.len(), 13);
        assert!(names.contains(&"arbos_reply") && names.contains(&"arbos_pending"));
        // Buzz L9: truncation is explicit.
        let read = handle(
            &paths,
            None,
            &json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "arbos_read_chat", "arguments": { "chat": "c1", "limit": 2 } } }),
        )
        .await
        .unwrap();
        let body: Value =
            serde_json::from_str(read["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(body["has_more"], true);
        assert_eq!(body["before"], "m2");
        assert_eq!(body["messages"].as_array().unwrap().len(), 2);
        let pending = handle(
            &paths,
            None,
            &json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": { "name": "arbos_pending", "arguments": {} } }),
        )
        .await
        .unwrap();
        let body: Value =
            serde_json::from_str(pending["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(body[0]["rows"].as_array().unwrap().len(), 1);
        // Actions without a daemon fail with one sentence, not a stack.
        let post = handle(
            &paths,
            None,
            &json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/call",
            "params": { "name": "arbos_post", "arguments": { "chat": "c1", "text": "hi" } } }),
        )
        .await
        .unwrap();
        assert_eq!(post["result"]["isError"], true);
        assert!(post["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("not running"));
        let unknown = handle(
            &paths,
            None,
            &json!({ "jsonrpc": "2.0", "id": 6, "method": "nope" }),
        )
        .await
        .unwrap();
        assert_eq!(unknown["error"]["code"], -32601);
    }
}
