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
    arbos_core::home_dir().map(|home| home.join(".config").join("arbos"))
}

/// How many of [`config_paths`] belong to the place itself (its `.arbos/`
/// and the Cursor / Claude Code files in its root); the rest is the
/// machine's.
const PLACE_SCOPE: usize = 3;

/// A config file that could not be used, and what that cost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub path: PathBuf,
    /// The parse error, or `<name>: <why>` for one server's entry.
    pub error: String,
    /// The file is the place's and did not parse: the walk stopped at it,
    /// and no file after it — the place's other files or the machine's —
    /// was read in its stead.
    pub blocked_global: bool,
}

/// What [`load`] found: the servers, and the files that let it down.
#[derive(Debug, Default)]
pub struct Loaded {
    pub servers: Vec<Server>,
    pub problems: Vec<Problem>,
}

/// Every server configured for the place, plus `ARBOS_MCP_CMD` as `env`.
/// A place file that does not parse is a problem, said; the machine's
/// file is then not read, so a server the place meant to define is never
/// silently the machine's one of the same name (qal-j31: one typo in
/// `.arbos/mcp.toml` handed every name in it to the global file, and
/// nobody was told).
pub fn load(place: &Place) -> Loaded {
    let mut loaded = load_from(&config_paths(place), PLACE_SCOPE);
    if let Some(cmd) = std::env::var("ARBOS_MCP_CMD")
        .ok()
        .filter(|c| !c.is_empty())
    {
        if !loaded.servers.iter().any(|s| s.name == "env") {
            let args = std::env::var("ARBOS_MCP_ARGS")
                .unwrap_or_default()
                .split_whitespace()
                .map(str::to_string)
                .collect();
            loaded.servers.push(Server {
                name: "env".into(),
                transport: Transport::Stdio {
                    command: cmd,
                    args,
                    env: Vec::new(),
                },
            });
        }
    }
    loaded
}

/// [`load`]'s servers alone.
pub fn load_servers(place: &Place) -> Vec<Server> {
    load(place).servers
}

/// The walk over `paths` in order, the first `place_scope` of them the
/// place's own. The first file to define a name wins. A place file that
/// does not parse stops the walk there: the names it meant to define are
/// unknown, so any later file — `.cursor/mcp.json` as much as the
/// machine's — could hand one of them a different server unnoticed
/// (qal-j31). Files before it in the walk keep what they defined.
fn load_from(paths: &[PathBuf], place_scope: usize) -> Loaded {
    let mut out = Loaded::default();
    for (i, path) in paths.iter().enumerate() {
        let place_file = i < place_scope;
        let Ok(text) = std::fs::read_to_string(path) else {
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
                out.problems.push(Problem {
                    path: path.clone(),
                    error: e.to_string(),
                    blocked_global: place_file,
                });
                if place_file {
                    break;
                }
                continue;
            }
        };
        for (name, entry) in file.servers {
            if out.servers.iter().any(|s| s.name == name) {
                continue;
            }
            match server_from(&name, entry, path) {
                Ok(server) => out.servers.push(server),
                Err(e) => {
                    eprintln!("mcp: {}: {name}: {e}", path.display());
                    out.problems.push(Problem {
                        path: path.clone(),
                        error: format!("{name}: {e}"),
                        blocked_global: false,
                    });
                }
            }
        }
    }
    out
}

/// The line a person reads for a config problem, on the transcript.
pub fn problem_notice(place: &Place, p: &Problem) -> String {
    let shown = p
        .path
        .strip_prefix(&place.path)
        .map(|r| r.display().to_string())
        .unwrap_or_else(|_| p.path.display().to_string());
    if p.blocked_global {
        format!(
            "MCP: {shown} does not parse ({}). Its servers are off, and no MCP file after it was read in its place — not the place's other files, not the machine's own MCP file — since a server this file meant to define would otherwise have come from another file of the same name, unnoticed. Fix the file and restart the kernel.",
            p.error.trim()
        )
    } else {
        format!("MCP: {shown}: {}; that server is off.", p.error.trim())
    }
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

#[cfg(test)]
mod config_walk_tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("arbos-mcp-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// qal-j31: one typo in the place's file discarded the whole file in
    /// silence and every name in it fell through to the machine's file.
    /// Now the broken place file is a problem, said, and the machine's
    /// file is not read in its stead; place files that do parse still
    /// load; a machine file that does not parse is a problem too, and the
    /// place's servers stand.
    #[test]
    fn a_place_file_that_does_not_parse_is_said_and_blocks_the_machines_file() {
        let d = dir("walk");
        let place_toml = d.join("mcp.toml");
        let place_json = d.join("mcp.json");
        let global = d.join("global.toml");
        std::fs::write(
            &place_toml,
            "[servers.notes]\ncommand = \"notes-mcp\"\nargs = [\n",
        )
        .unwrap();
        std::fs::write(
            &place_json,
            r#"{"mcpServers":{"files":{"command":"files-mcp"}}}"#,
        )
        .unwrap();
        std::fs::write(
            &global,
            "[servers.notes]\ncommand = \"global-notes\"\n[servers.web]\ncommand = \"web-mcp\"\n",
        )
        .unwrap();
        let loaded = load_from(&[place_toml.clone(), place_json.clone(), global.clone()], 2);
        let names: Vec<&str> = loaded.servers.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.is_empty(),
            "the walk stops at the broken place file: not the place's later file, not the machine's: {names:?}"
        );
        assert_eq!(loaded.problems.len(), 1, "{:?}", loaded.problems);
        let p = &loaded.problems[0];
        assert_eq!(p.path, place_toml);
        assert!(p.blocked_global, "{p:?}");
        assert!(!p.error.is_empty());

        // The machine's file broken instead: said, and the place stands.
        std::fs::write(&place_toml, "[servers.notes]\ncommand = \"notes-mcp\"\n").unwrap();
        std::fs::write(&global, "[servers.web\ncommand = 1\n").unwrap();
        let loaded = load_from(&[place_toml.clone(), place_json.clone(), global.clone()], 2);
        let names: Vec<&str> = loaded.servers.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["notes", "files"]);
        assert_eq!(loaded.problems.len(), 1);
        assert!(!loaded.problems[0].blocked_global);
        assert_eq!(loaded.problems[0].path, global);

        // All well: the place's name wins over the machine's, the rest joins.
        std::fs::write(
            &global,
            "[servers.notes]\ncommand = \"global-notes\"\n[servers.web]\ncommand = \"web-mcp\"\n",
        )
        .unwrap();
        let loaded = load_from(&[place_toml.clone(), place_json.clone(), global.clone()], 2);
        let names: Vec<&str> = loaded.servers.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["notes", "files", "web"]);
        assert!(loaded.problems.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }
}
