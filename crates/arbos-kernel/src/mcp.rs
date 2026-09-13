//! MCP servers per place and per user.
//!
//! Declared in `.arbos/mcp.toml` (or `.cursor/mcp.json` / `.mcp.json`, the
//! Cursor and Claude Code shape, or `~/.config/arbos/mcp.toml`):
//!
//! ```toml
//! [servers.fs]
//! command = "npx"
//! args = ["-y", "@modelcontextprotocol/server-filesystem", "."]
//! env = { LOG = "1" }            # literal values
//! env_from = ["GITHUB_TOKEN"]    # copied from the kernel's environment
//!
//! [servers.docs]
//! url = "https://mcp.example.com/mcp"      # Streamable HTTP
//! headers = { Authorization = "Bearer …" }
//! ```
//!
//! Every tool a server offers reaches the model as `mcp__<server>__<tool>`.
//! A stdio server is spawned per request (initialize, request, exit); an
//! HTTP server gets `initialize` then the request, with `Mcp-Session-Id`
//! echoed when it issues one. `ARBOS_MCP_CMD` still works, as a server
//! named `env`.

use anyhow::{Context, Result, anyhow, bail};
use arbos_core::Place;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const PROTOCOL: &str = "2025-03-26";
const HTTP_TIMEOUT: Duration = Duration::from_secs(60);

/// One configured server.
#[derive(Debug, Clone)]
pub struct Server {
    pub name: String,
    pub transport: Transport,
}

#[derive(Debug, Clone)]
pub enum Transport {
    Stdio {
        command: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
    },
    Http {
        url: String,
        headers: Vec<(String, String)>,
    },
}

/// One tool the server advertises, as it should reach the model.
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON schema of the arguments (`inputSchema`).
    pub input_schema: Value,
}

#[derive(Deserialize, Default)]
struct TomlFile {
    #[serde(default)]
    servers: BTreeMap<String, TomlServer>,
}

#[derive(Deserialize, Default)]
struct TomlServer {
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    env_from: Vec<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    /// Cursor / Claude Code: `"type": "stdio" | "http" | "sse"`.
    #[serde(default, rename = "type")]
    kind: Option<String>,
}

/// The config files, in the order they are read; the first server to
/// claim a name wins.
pub fn config_paths(place: &Place) -> Vec<PathBuf> {
    let mut out = vec![
        place.arbos().join("mcp.toml"),
        place.path.join(".cursor").join("mcp.json"),
        place.path.join(".mcp.json"),
    ];
    if let Some(cfg) = config_dir() {
        out.push(cfg.join("mcp.toml"));
    }
    out
}

fn config_dir() -> Option<PathBuf> {
    if let Some(base) = std::env::var_os("XDG_CONFIG_HOME").filter(|b| !b.is_empty()) {
        return Some(PathBuf::from(base).join("arbos"));
    }
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(|home| PathBuf::from(home).join(".config").join("arbos"))
}

/// Every server configured for the place, plus `ARBOS_MCP_CMD` as `env`.
/// Config trouble is reported on stderr; the rest still loads.
pub fn load_servers(place: &Place) -> Vec<Server> {
    let mut out: Vec<Server> = Vec::new();
    for path in config_paths(place) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let parsed = if path.extension().and_then(|e| e.to_str()) == Some("json") {
            parse_json(&text)
        } else {
            toml::from_str::<TomlFile>(&text).map_err(|e| anyhow!("{e}"))
        };
        let file = match parsed {
            Ok(f) => f,
            Err(e) => {
                eprintln!("mcp: {}: {e}", path.display());
                continue;
            }
        };
        for (name, entry) in file.servers {
            if out.iter().any(|s| s.name == name) {
                continue;
            }
            match server_from(&name, entry, &path) {
                Ok(server) => out.push(server),
                Err(e) => eprintln!("mcp: {}: {name}: {e}", path.display()),
            }
        }
    }
    if let Some(cmd) = std::env::var("ARBOS_MCP_CMD")
        .ok()
        .filter(|c| !c.is_empty())
    {
        if !out.iter().any(|s| s.name == "env") {
            let args = std::env::var("ARBOS_MCP_ARGS")
                .unwrap_or_default()
                .split_whitespace()
                .map(str::to_string)
                .collect();
            out.push(Server {
                name: "env".into(),
                transport: Transport::Stdio {
                    command: cmd,
                    args,
                    env: Vec::new(),
                },
            });
        }
    }
    out
}

/// `{"mcpServers": {name: {command, args, env} | {url, headers}}}`.
fn parse_json(text: &str) -> Result<TomlFile> {
    let v: Value = serde_json::from_str(text)?;
    let map = v
        .get("mcpServers")
        .or_else(|| v.get("servers"))
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("no mcpServers object"))?;
    let mut servers = BTreeMap::new();
    for (name, entry) in map {
        let server: TomlServer =
            serde_json::from_value(entry.clone()).with_context(|| format!("server {name}"))?;
        servers.insert(name.clone(), server);
    }
    Ok(TomlFile { servers })
}

fn server_from(name: &str, entry: TomlServer, file: &Path) -> Result<Server> {
    if !valid_name(name) {
        bail!("server names are letters, digits, - and _");
    }
    if let Some(kind) = entry.kind.as_deref() {
        if kind.eq_ignore_ascii_case("sse") {
            bail!("transport \"sse\" is not supported; use a Streamable HTTP url or a command");
        }
    }
    if let Some(url) = entry.url.filter(|u| !u.is_empty()) {
        return Ok(Server {
            name: name.to_string(),
            transport: Transport::Http {
                url,
                headers: entry.headers.into_iter().collect(),
            },
        });
    }
    let Some(command) = entry.command.filter(|c| !c.is_empty()) else {
        bail!("needs a command or a url");
    };
    let mut env: Vec<(String, String)> = entry.env.into_iter().collect();
    for var in entry.env_from {
        match std::env::var(&var) {
            Ok(v) if !v.is_empty() => env.push((var, v)),
            _ => eprintln!(
                "mcp: {}: {name}: env_from {var} is not set in the kernel's environment",
                file.display()
            ),
        }
    }
    Ok(Server {
        name: name.to_string(),
        transport: Transport::Stdio {
            command,
            args: entry.args,
            env,
        },
    })
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 48
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

/// The registered tool name: `mcp__<server>__<tool>`, with characters the
/// model APIs reject replaced.
pub fn tool_name(server: &str, tool: &str) -> String {
    let clean: String = tool
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("mcp__{server}__{clean}")
}

impl Server {
    /// One JSON-RPC exchange: initialize, then `method`, then the reply
    /// whose id matches.
    fn rpc(&self, method: &str, params: Value) -> Result<Value> {
        match &self.transport {
            Transport::Stdio { command, args, env } => {
                rpc_stdio(&self.name, command, args, env, method, params)
            }
            Transport::Http { url, headers } => rpc_http(&self.name, url, headers, method, params),
        }
    }

    pub fn tools(&self) -> Result<Vec<ToolSpec>> {
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
            out.push(ToolSpec {
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

fn init_request(id: u64) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL,
            "capabilities": {},
            "clientInfo": {"name": "arbos-kernel", "version": env!("CARGO_PKG_VERSION")}
        }
    })
}

fn unwrap_reply(server: &str, method: &str, v: Value) -> Result<Value> {
    if let Some(err) = v.get("error") {
        bail!(
            "MCP {server} {method}: {}",
            err.get("message")
                .and_then(Value::as_str)
                .unwrap_or("error")
        );
    }
    Ok(v.get("result").cloned().unwrap_or(Value::Null))
}

fn rpc_stdio(
    server: &str,
    command: &str,
    args: &[String],
    env: &[(String, String)],
    method: &str,
    params: Value,
) -> Result<Value> {
    let mut child = Command::new(command)
        .args(args)
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("start MCP server {server} ({command})"))?;
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    writeln!(stdin, "{}", init_request(1))?;
    writeln!(
        stdin,
        "{}",
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
    )?;
    let req = json!({"jsonrpc": "2.0", "id": 2, "method": method, "params": params});
    writeln!(stdin, "{req}")?;
    let reader = BufReader::new(stdout);
    let mut answer = None;
    for line in reader.lines().take(256) {
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
    let v = answer.ok_or_else(|| anyhow!("MCP server {server} sent no reply to {method}"))?;
    unwrap_reply(server, method, v)
}

/// Streamable HTTP: POST each message; the reply is JSON or an SSE stream
/// whose `data:` lines carry JSON-RPC messages.
fn rpc_http(
    server: &str,
    url: &str,
    headers: &[(String, String)],
    method: &str,
    params: Value,
) -> Result<Value> {
    let client = reqwest::blocking::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .user_agent("arbos-kernel")
        .build()?;
    let post = |body: &Value, session: Option<&str>| -> Result<(Option<String>, Vec<Value>)> {
        let mut req = client
            .post(url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");
        for (k, v) in headers {
            req = req.header(k.as_str(), v.as_str());
        }
        if let Some(s) = session {
            req = req.header("Mcp-Session-Id", s);
        }
        let resp = req
            .body(body.to_string())
            .send()
            .with_context(|| format!("MCP {server}: POST {url}"))?;
        let status = resp.status();
        let sid = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let ctype = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let text = resp.text().unwrap_or_default();
        if !status.is_success() {
            bail!(
                "MCP {server}: HTTP {status}: {}",
                text.chars().take(200).collect::<String>()
            );
        }
        let mut messages = Vec::new();
        if ctype.contains("text/event-stream") {
            for line in text.lines() {
                if let Some(data) = line.strip_prefix("data:") {
                    if let Ok(v) = serde_json::from_str::<Value>(data.trim()) {
                        messages.push(v);
                    }
                }
            }
        } else if !text.trim().is_empty() {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                messages.push(v);
            }
        }
        Ok((sid, messages))
    };
    let (session, _) = post(&init_request(1), None)?;
    let _ = post(
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        session.as_deref(),
    );
    let req = json!({"jsonrpc": "2.0", "id": 2, "method": method, "params": params});
    let (_, messages) = post(&req, session.as_deref())?;
    let v = messages
        .into_iter()
        .find(|m| m.get("id").and_then(Value::as_i64) == Some(2))
        .ok_or_else(|| anyhow!("MCP {server} sent no reply to {method}"))?;
    unwrap_reply(server, method, v)
}
