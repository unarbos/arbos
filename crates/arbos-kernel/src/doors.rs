//! Extra attaches on the same tree: MCP tools, Telegram, voice, refresh.

use anyhow::{Context, Result, anyhow, bail};
use arbos_core::{Node, Place, Wake, WakeKind};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    sync::{Arc, mpsc},
};

use crate::{attach::Frame, hooks::KernelHooks};

/// MCP: more tools in the same dispatch slot. One stdio server, spoken to
/// per request: spawn, `initialize`, the request, read the matching reply.
/// Servers that need a long-lived session are not supported yet.
#[derive(Debug, Clone)]
pub struct McpServer {
    pub cmd: String,
    pub args: Vec<String>,
}

/// One tool the server advertises, as it should reach the model.
#[derive(Debug, Clone)]
pub struct McpToolSpec {
    pub name: String,
    pub description: String,
    /// JSON schema of the arguments (`inputSchema`).
    pub input_schema: Value,
}

impl McpServer {
    pub fn from_env() -> Option<Self> {
        let cmd = std::env::var("ARBOS_MCP_CMD")
            .ok()
            .filter(|c| !c.is_empty())?;
        let args = std::env::var("ARBOS_MCP_ARGS")
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_string)
            .collect();
        Some(Self { cmd, args })
    }

    /// One JSON-RPC exchange: initialize, then `method`, then the reply whose
    /// id matches. Notifications and unrelated lines are skipped.
    fn rpc(&self, method: &str, params: Value) -> Result<Value> {
        let mut child = Command::new(&self.cmd)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("start MCP server {}", self.cmd))?;
        let mut stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let init = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "arbos-kernel", "version": env!("CARGO_PKG_VERSION")}
            }
        });
        writeln!(stdin, "{init}")?;
        writeln!(
            stdin,
            "{}",
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
        )?;
        let req = json!({"jsonrpc": "2.0", "id": 2, "method": method, "params": params});
        writeln!(stdin, "{req}")?;
        let reader = BufReader::new(stdout);
        let mut answer = None;
        for line in reader.lines().take(64) {
            let Ok(line) = line else { break };
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if v.get("id").and_then(Value::as_i64) == Some(2) {
                answer = Some(v);
                break;
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        let v =
            answer.ok_or_else(|| anyhow!("MCP server {} sent no reply to {method}", self.cmd))?;
        if let Some(err) = v.get("error") {
            bail!(
                "MCP {method}: {}",
                err.get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("error")
            );
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    }

    pub fn tools(&self) -> Result<Vec<McpToolSpec>> {
        let result = self.rpc("tools/list", json!({}))?;
        let mut out = Vec::new();
        for t in result
            .get("tools")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(name) = t.get("name").and_then(Value::as_str) else {
                continue;
            };
            out.push(McpToolSpec {
                name: name.to_string(),
                description: t
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                input_schema: t
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
            });
        }
        Ok(out)
    }

    /// `tools/call`; the text parts of the result, joined.
    pub fn call(&self, name: &str, arguments: Value) -> Result<String> {
        let result = self.rpc("tools/call", json!({"name": name, "arguments": arguments}))?;
        let text: Vec<String> = result
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|c| match c.get("type").and_then(Value::as_str) {
                Some("text") => c.get("text").and_then(Value::as_str).map(str::to_string),
                Some(other) => Some(format!("[{other} content]")),
                None => None,
            })
            .collect();
        let body = if text.is_empty() {
            result.to_string()
        } else {
            text.join("\n")
        };
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            bail!("{body}");
        }
        Ok(body)
    }
}

/// Names of the tools the env-configured server offers (`ARBOS_MCP_CMD`).
pub fn mcp_list(cmd: &str, args: &[String]) -> Result<Vec<String>> {
    let server = McpServer {
        cmd: cmd.to_string(),
        args: args.to_vec(),
    };
    Ok(server.tools()?.into_iter().map(|t| t.name).collect())
}

/// Telegram long-poll: each message is an inbox node on root.
pub async fn telegram_loop(token: String, hooks: Arc<KernelHooks>) {
    let mut offset: i64 = 0;
    let client = reqwest::Client::new();
    loop {
        let url =
            format!("https://api.telegram.org/bot{token}/getUpdates?timeout=20&offset={offset}");
        let Ok(resp) = client.get(&url).send().await else {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            continue;
        };
        let Ok(v) = resp.json::<Value>().await else {
            continue;
        };
        if let Some(arr) = v.get("result").and_then(|r| r.as_array()) {
            for upd in arr {
                if let Some(id) = upd.get("update_id").and_then(|i| i.as_i64()) {
                    offset = id + 1;
                }
                let Some(text) = upd
                    .pointer("/message/text")
                    .and_then(|t| t.as_str())
                    .map(str::to_string)
                else {
                    continue;
                };
                let _ = hooks.inbox("root", Node::inbox(text, "user"));
            }
        }
    }
}

/// Voice: record to a wav via `rec`/`sox` if present; stop returns a path.
pub fn voice_start() -> Result<String> {
    let path = std::env::temp_dir().join("arbos-voice.wav");
    let _ = Command::new("rec")
        .args([
            path.to_str().unwrap_or("/tmp/arbos-voice.wav"),
            "trim",
            "0",
            "30",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    Ok(path.display().to_string())
}

pub fn voice_stop(hooks: &KernelHooks) -> Result<String> {
    let path = std::env::temp_dir().join("arbos-voice.wav");
    // Best effort: whisper.cpp or `say` invert — if a transcript file exists, wake.
    let txt = path.with_extension("txt");
    let text = if txt.exists() {
        std::fs::read_to_string(&txt)?
    } else {
        format!("voice file {}", path.display())
    };
    let mut n = Node::inbox(text.clone(), "user");
    n.attachments = vec![path.display().to_string()];
    let _ = hooks.inbox("root", n);
    Ok(text)
}

/// Refresh: wake root with a notice. Same tree, no second product.
pub fn refresh(place: &Place, wakes: &tokio::sync::mpsc::UnboundedSender<Wake>) {
    let _ = wakes.send(Wake::new("root", WakeKind::Serve, Some("refresh".into())));
    let _ = place;
}

pub fn spawn_telegram_if_configured(hooks: Arc<KernelHooks>) {
    let Ok(token) = std::env::var("ARBOS_TELEGRAM_TOKEN") else {
        return;
    };
    if token.is_empty() {
        return;
    }
    tokio::spawn(telegram_loop(token, hooks));
}

pub type DoorTx = mpsc::Sender<Frame>;
