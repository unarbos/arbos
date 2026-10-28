//! `arbos-acp` — put an ACP agent (Claude Code through `claude-agent-acp`, Codex through
//! `codex-acp`, Goose, any agent speaking the Agent Client Protocol over stdio) into Arbos chats
//! as a connected member (#2143; design `docs/outside-agents-spec.md`).
//!
//! One process = one agent identity: it dials the relay's `/asp/socket` with the agent's
//! `arbk_…` key, receives every message of the agent's chats as `message.received` events,
//! prompts the ACP agent on the ones flagged `wake`, and streams the agent's text back as a live
//! bubble (`message.start` → `message.delta` → `message.end`), with typing and a status line from
//! the agent's tool calls. The relay writes the rows; this process holds no chat state beyond a
//! short context window per chat.
//!
//! ```text
//! Arbos relay ──WS /asp/socket──▶ arbos-acp ──stdio JSON-RPC (ACP)──▶ claude-agent-acp / codex-acp
//!             ◀── message.* ────           ◀── session/update ────────
//! ```
//!
//! Run: `ARBOS_AGENT_KEY=arbk_… arbos-acp --agent claude-agent-acp` (see `--help`).

use std::collections::{HashMap, VecDeque};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

/// ACP protocol version this client speaks (`initialize.protocolVersion`).
const ACP_PROTOCOL_VERSION: u64 = 1;
/// Messages kept per chat as context for the next prompt.
const CONTEXT_ROWS: usize = 12;
/// Deltas are flushed to the relay on this tick while the agent streams.
const FLUSH_EVERY: Duration = Duration::from_millis(150);
/// Heartbeat cadence (the relay's lease is 180 s).
const HEARTBEAT_EVERY: Duration = Duration::from_secs(60);

#[derive(Parser, Debug, Clone)]
#[command(
    name = "arbos-acp",
    version,
    about = "Connect an ACP agent to Arbos chats"
)]
struct Args {
    /// Relay base URL.
    #[arg(
        long,
        env = "ARBOS_RELAY_URL",
        default_value = "https://relay.arbos.life"
    )]
    relay: String,
    /// The agent key the Arbos app showed once (arbk_…).
    #[arg(long, env = "ARBOS_AGENT_KEY", hide_env_values = true)]
    key: String,
    /// The ACP agent to run (a command on PATH or a path).
    #[arg(long, env = "ARBOS_ACP_AGENT", default_value = "claude-agent-acp")]
    agent: String,
    /// Extra arguments for the agent, comma-separated (`--agent-args=acp,--foo`).
    #[arg(long, env = "ARBOS_ACP_AGENT_ARGS", value_delimiter = ',')]
    agent_args: Vec<String>,
    /// Working directory the agent's sessions open in (default: the current directory).
    #[arg(long, env = "ARBOS_ACP_CWD")]
    cwd: Option<String>,
    /// Start from the oldest stored event instead of now (replays the backlog).
    #[arg(long)]
    replay: bool,
    /// Answer every message in a group, not only mentions (the relay's `wake` flag is ignored).
    #[arg(long)]
    answer_all: bool,
    /// What to answer when the agent asks permission for a tool: allow (default) or reject.
    #[arg(long, env = "ARBOS_ACP_PERMISSIONS", default_value = "allow")]
    permissions: String,
    /// Extra instructions prepended to every prompt (how this agent should behave in chat).
    #[arg(long, env = "ARBOS_ACP_INSTRUCTIONS")]
    instructions: Option<String>,
    /// ACP auth method id to use when the agent lists several (e.g. `api-key`, `chat-gpt`).
    #[arg(long, env = "ARBOS_ACP_AUTH_METHOD")]
    auth_method: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "arbos_acp=info".into()),
        )
        .with_target(false)
        .init();
    let args = Args::parse();
    if !args.key.starts_with("arbk_") {
        bail!("ARBOS_AGENT_KEY must be the agent key the Arbos app showed (arbk_…)");
    }
    let relay = args.relay.trim_end_matches('/').to_owned();
    let http = reqwest::Client::builder()
        .user_agent(format!("arbos-acp/{}", env!("CARGO_PKG_VERSION")))
        .build()?;

    // Who am I on the relay?
    let me: Value = http
        .get(format!("{relay}/asp/me"))
        .bearer_auth(&args.key)
        .send()
        .await
        .context("relay unreachable")?
        .error_for_status()
        .context("the relay refused the key (revoked or mistyped?)")?
        .json()
        .await?;
    let agent_name = me["agent"]["name"].as_str().unwrap_or("agent").to_owned();
    let handle = me["agent"]["handle"].as_str().unwrap_or("").to_owned();
    let mut cursor = if args.replay {
        0
    } else {
        me["inbox_head"].as_i64().unwrap_or(0)
    };
    tracing::info!(
        name = %agent_name, handle = %handle, protocol = %me["protocol"], chats = me["chats"].as_array().map(|c| c.len()).unwrap_or(0),
        "connected to the relay"
    );

    // The ACP agent.
    let acp = Acp::spawn(&args).await?;
    let init = acp
        .request(
            "initialize",
            json!({
                "protocolVersion": ACP_PROTOCOL_VERSION,
                "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false }, "terminal": false },
                "clientInfo": { "name": "arbos-acp", "version": env!("CARGO_PKG_VERSION") },
            }),
        )
        .await
        .context("ACP initialize failed — is the agent command right?")?;
    let agent_info = json!({
        "name": args.agent,
        "acp": init["agentInfo"],
        "protocolVersion": init["protocolVersion"],
        "bridge": format!("arbos-acp/{}", env!("CARGO_PKG_VERSION")),
    });
    tracing::info!(agent = %args.agent, info = %init["agentInfo"], "ACP agent initialized");
    if let Some(methods) = init["authMethods"].as_array().filter(|m| !m.is_empty()) {
        let ids: Vec<&str> = methods.iter().filter_map(|m| m["id"].as_str()).collect();
        // Pick the method the environment supports: an API key when one is set, else the first
        // (a login the user already did on this machine). `--auth-method` overrides.
        let chosen = args.auth_method.clone().or_else(|| {
            let has_key = std::env::var("OPENAI_API_KEY").is_ok()
                || std::env::var("ANTHROPIC_API_KEY").is_ok();
            ids.iter()
                .find(|m| has_key && m.contains("api"))
                .or_else(|| ids.first())
                .map(|m| (*m).to_owned())
        });
        if let Some(method) = chosen {
            match acp
                .request("authenticate", json!({ "methodId": method }))
                .await
            {
                Ok(_) => tracing::info!(%method, "ACP authenticate ok"),
                Err(e) => {
                    tracing::warn!(%method, error = format!("{e:#}"), "ACP authenticate failed; continuing (the agent may already be signed in)")
                }
            }
        }
    }

    let bridge = Arc::new(Bridge {
        args: args.clone(),
        relay: relay.clone(),
        http,
        acp,
        agent_name,
        handle,
        sessions: Mutex::new(HashMap::new()),
        context: Mutex::new(HashMap::new()),
        queue: Mutex::new(VecDeque::new()),
        busy: Mutex::new(()),
        agent_info,
    });

    // Heartbeat.
    {
        let b = bridge.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(HEARTBEAT_EVERY);
            loop {
                tick.tick().await;
                let _ = b
                    .post(json!({ "type": "presence.heartbeat", "agent": b.agent_info }))
                    .await;
            }
        });
    }

    // Socket loop with reconnects.
    let mut backoff = Duration::from_secs(1);
    loop {
        match run_socket(&bridge, &mut cursor).await {
            Ok(()) => {
                tracing::warn!("socket closed; reconnecting");
                backoff = Duration::from_secs(1);
            }
            Err(e) => {
                tracing::warn!(error = %e, "socket error; retrying in {backoff:?}");
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(60));
    }
}

struct Bridge {
    args: Args,
    relay: String,
    http: reqwest::Client,
    acp: Acp,
    agent_name: String,
    handle: String,
    /// chat id → ACP session id.
    sessions: Mutex<HashMap<String, String>>,
    /// chat id → recent messages (who: text) for the prompt's context.
    context: Mutex<HashMap<String, VecDeque<String>>>,
    /// Wake events waiting while a prompt runs (one prompt at a time, like buzz-acp's queue).
    queue: Mutex<VecDeque<Value>>,
    busy: Mutex<()>,
    agent_info: Value,
}

impl Bridge {
    /// One event to the relay over HTTP (the socket is for receiving; posting over HTTP keeps the
    /// result handling simple and survives a socket reconnect mid-stream).
    async fn post(&self, event: Value) -> Result<Value> {
        let r: Value = self
            .http
            .post(format!("{}/asp/events", self.relay))
            .bearer_auth(&self.args.key)
            .json(&event)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let first = r["results"]
            .as_array()
            .and_then(|a| a.first())
            .cloned()
            .unwrap_or(Value::Null);
        if first["ok"].as_bool() != Some(true) {
            bail!("relay refused {}: {}", event["type"], first["message"]);
        }
        Ok(first)
    }

    async fn session_for(&self, chat_id: &str) -> Result<String> {
        if let Some(s) = self.sessions.lock().await.get(chat_id) {
            return Ok(s.clone());
        }
        let cwd = match &self.args.cwd {
            Some(c) => std::fs::canonicalize(c)?,
            None => std::env::current_dir()?,
        };
        let r = self
            .acp
            .request(
                "session/new",
                json!({ "cwd": cwd.to_string_lossy(), "mcpServers": [] }),
            )
            .await
            .context("ACP session/new failed")?;
        let id = r["sessionId"]
            .as_str()
            .ok_or_else(|| anyhow!("session/new returned no sessionId"))?
            .to_owned();
        tracing::info!(chat = %chat_id, session = %id, "ACP session opened");
        self.sessions
            .lock()
            .await
            .insert(chat_id.to_owned(), id.clone());
        Ok(id)
    }

    fn remember(&self, ctx: &mut HashMap<String, VecDeque<String>>, chat_id: &str, line: String) {
        let q = ctx.entry(chat_id.to_owned()).or_default();
        q.push_back(line);
        while q.len() > CONTEXT_ROWS {
            q.pop_front();
        }
    }

    /// Everything the agent needs to answer one message as a chat member.
    async fn prompt_text(&self, ev: &Value) -> String {
        let chat = &ev["chat"];
        let msg = &ev["message"];
        let kind = chat["kind"].as_str().unwrap_or("chat");
        let title = chat["title"].as_str().unwrap_or("");
        let from_name = msg["from"]["name"].as_str().unwrap_or("someone");
        let from_handle = msg["from"]["handle"].as_str().unwrap_or("");
        let text = msg["text"].as_str().unwrap_or("");
        let mut out = String::new();
        if let Some(i) = &self.args.instructions {
            out.push_str(i.trim());
            out.push_str("\n\n");
        }
        out.push_str(&format!(
            "You are {name} (@{handle}), a member of an Arbos chat. Everything you write back is posted \
             to the chat as your message: write plain prose a person would send in a chat, short, no \
             headings, no narration of tools or steps, no mention of being an AI assistant unless asked. \
             If you decide the message needs no answer from you, reply with exactly: [silent]\n\n",
            name = self.agent_name,
            handle = self.handle
        ));
        let where_ = match kind {
            "dm" => "a direct message".to_owned(),
            "group" => format!("the group chat \"{title}\""),
            _ => format!("your own chat with {from_name}"),
        };
        out.push_str(&format!("This is {where_}.\n"));
        let ctx = self.context.lock().await;
        if let Some(lines) = ctx.get(chat["id"].as_str().unwrap_or("")) {
            if lines.len() > 1 {
                out.push_str("Recent messages (oldest first):\n");
                for l in lines.iter().take(lines.len() - 1) {
                    out.push_str("  ");
                    out.push_str(l);
                    out.push('\n');
                }
            }
        }
        drop(ctx);
        let who = if from_handle.is_empty() {
            from_name.to_owned()
        } else {
            format!("{from_name} (@{from_handle})")
        };
        out.push_str(&format!("\nNew message from {who}:\n{text}\n\nYour reply:"));
        out
    }

    /// Prompt the agent and stream its answer into the chat as one bubble.
    async fn answer(self: &Arc<Self>, ev: Value) -> Result<()> {
        let chat_id = ev["chat"]["id"].as_str().unwrap_or("").to_owned();
        let message_id = ev["message"]["id"].as_str().unwrap_or("").to_owned();
        let session = self.session_for(&chat_id).await?;
        let prompt = self.prompt_text(&ev).await;
        let _ = self
            .post(json!({ "type": "typing", "chat": chat_id, "on": true }))
            .await;

        // Stream handling: chunks from session/update land here; a ticker flushes them.
        let (tx, mut rx) = mpsc::unbounded_channel::<Update>();
        self.acp.subscribe(&session, tx).await;
        let this = self.clone();
        let chat_for_stream = chat_id.clone();
        let reply_to = message_id.clone();
        let streamer = tokio::spawn(async move {
            let mut open: Option<String> = None;
            let mut pending = String::new();
            let mut full = String::new();
            let mut last_status: Option<String> = None;
            let mut tick = tokio::time::interval(FLUSH_EVERY);
            loop {
                tokio::select! {
                    u = rx.recv() => match u {
                        Some(Update::Text(t)) => { pending.push_str(&t); full.push_str(&t); }
                        Some(Update::Status(s)) => {
                            if last_status.as_deref() != Some(&s) {
                                last_status = Some(s.clone());
                                let _ = this.post(json!({ "type": "status", "chat": chat_for_stream, "text": s })).await;
                            }
                        }
                        Some(Update::Done) | None => break,
                    },
                    _ = tick.tick() => {
                        if !pending.is_empty() {
                            flush(&this, &chat_for_stream, &reply_to, &mut open, &mut pending, &full).await;
                        }
                    }
                }
            }
            if !pending.is_empty() {
                flush(
                    &this,
                    &chat_for_stream,
                    &reply_to,
                    &mut open,
                    &mut pending,
                    &full,
                )
                .await;
            }
            (open, full)
        });

        let result = self
            .acp
            .request(
                "session/prompt",
                json!({ "sessionId": session, "prompt": [{ "type": "text", "text": prompt }] }),
            )
            .await;
        self.acp.unsubscribe(&session).await;
        let (open, full) = streamer.await.unwrap_or((None, String::new()));
        let stop = result
            .as_ref()
            .ok()
            .and_then(|r| r["stopReason"].as_str())
            .unwrap_or("error")
            .to_owned();
        let text = full.trim().to_owned();
        let silent = text.is_empty() || text == "[silent]" || text.starts_with("[silent]");
        match open {
            Some(msg) if silent => {
                // Nothing worth posting after all: close the empty row with what little there is.
                let _ = self
                    .post(json!({ "type": "message.end", "chat": chat_id, "message": msg, "text": " " }))
                    .await;
            }
            Some(msg) => {
                self.post(
                    json!({ "type": "message.end", "chat": chat_id, "message": msg, "text": text }),
                )
                .await?;
            }
            None if silent => {
                let _ = self
                    .post(json!({ "type": "typing", "chat": chat_id, "on": false }))
                    .await;
            }
            None => {
                self.post(json!({ "type": "message.post", "chat": chat_id, "text": text, "reply_to": message_id }))
                    .await?;
            }
        }
        if let Err(e) = &result {
            tracing::warn!(chat = %chat_id, error = format!("{e:#}"), "prompt failed");
        }
        tracing::info!(chat = %chat_id, stop = %stop, chars = text.len(), silent, "turn finished");
        if !silent {
            let mut ctx = self.context.lock().await;
            self.remember(
                &mut ctx,
                &chat_id,
                format!("{} (you): {}", self.agent_name, text),
            );
        }
        Ok(())
    }
}

/// Push buffered text to the relay: the first flush opens the bubble.
async fn flush(
    b: &Arc<Bridge>,
    chat_id: &str,
    reply_to: &str,
    open: &mut Option<String>,
    pending: &mut String,
    full: &str,
) {
    if open.is_none() {
        // Don't open a bubble for a "[silent]" answer.
        if full.trim_start().starts_with('[') && full.len() < 12 {
            return;
        }
        match b
            .post(json!({ "type": "message.start", "chat": chat_id, "reply_to": reply_to }))
            .await
        {
            Ok(r) => *open = r["message"].as_str().map(str::to_owned),
            Err(e) => {
                tracing::warn!(error = %e, "message.start refused");
                return;
            }
        }
    }
    if let Some(msg) = open.as_deref() {
        let delta = std::mem::take(pending);
        if let Err(e) = b
            .post(
                json!({ "type": "message.delta", "chat": chat_id, "message": msg, "text": delta }),
            )
            .await
        {
            tracing::warn!(error = %e, "message.delta refused");
        }
    }
}

async fn run_socket(bridge: &Arc<Bridge>, cursor: &mut i64) -> Result<()> {
    let ws_url = format!(
        "{}/asp/socket?since={cursor}",
        bridge
            .relay
            .replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1)
    );
    let mut req = ws_url.as_str().into_client_request()?;
    req.headers_mut().insert(
        "authorization",
        format!("Bearer {}", bridge.args.key).parse()?,
    );
    let (ws, _) = tokio_tungstenite::connect_async(req).await?;
    let (mut sink, mut stream) = ws.split();
    tracing::info!(since = *cursor, "socket open");
    while let Some(msg) = stream.next().await {
        let msg = msg?;
        match msg {
            Message::Text(t) => {
                let ev: Value = match serde_json::from_str(&t) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if let Some(id) = ev["id"].as_i64() {
                    *cursor = id;
                }
                match ev["type"].as_str().unwrap_or("") {
                    "hello" => {}
                    "bye" => bail!("the relay said goodbye: {}", ev["reason"]),
                    "message.received" => on_message(bridge, ev).await,
                    "member.added" | "member.removed" => {
                        tracing::info!(chat = %ev["chat"]["title"], kind = %ev["type"], "membership changed");
                    }
                    "result" => {}
                    other => tracing::debug!(kind = %other, "event ignored"),
                }
            }
            Message::Ping(p) => {
                let _ = sink.send(Message::Pong(p)).await;
            }
            Message::Close(_) => return Ok(()),
            _ => {}
        }
    }
    Ok(())
}

async fn on_message(bridge: &Arc<Bridge>, ev: Value) {
    let chat_id = ev["chat"]["id"].as_str().unwrap_or("").to_owned();
    let from = ev["message"]["from"]["name"].as_str().unwrap_or("someone");
    let text = ev["message"]["text"].as_str().unwrap_or("");
    {
        let mut ctx = bridge.context.lock().await;
        bridge.remember(&mut ctx, &chat_id, format!("{from}: {text}"));
    }
    let wake = ev["wake"].as_bool().unwrap_or(false) || bridge.args.answer_all;
    if !wake {
        tracing::debug!(chat = %chat_id, "message kept as context (no wake)");
        return;
    }
    tracing::info!(chat = %ev["chat"]["title"], from = %from, "wake");
    bridge.queue.lock().await.push_back(ev);
    let b = bridge.clone();
    tokio::spawn(async move {
        // One prompt at a time; the queue drains in order.
        let _guard = b.busy.lock().await;
        loop {
            let next = b.queue.lock().await.pop_front();
            let Some(ev) = next else { break };
            if let Err(e) = b.answer(ev).await {
                tracing::warn!(error = format!("{e:#}"), "answer failed");
            }
        }
    });
}

// ---- ACP over stdio (JSON-RPC 2.0, one message per line) ---------------------------------------

enum Update {
    Text(String),
    Status(String),
    Done,
}

struct Acp {
    stdin: Arc<Mutex<ChildStdin>>,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>,
    streams: Arc<Mutex<HashMap<String, mpsc::UnboundedSender<Update>>>>,
    _child: Mutex<Child>,
}

impl Acp {
    async fn spawn(args: &Args) -> Result<Acp> {
        let mut cmd = Command::new(&args.agent);
        cmd.args(args.agent_args.iter().filter(|a| !a.is_empty()))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        if let Some(c) = &args.cwd {
            cmd.current_dir(c);
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("could not start the ACP agent `{}`", args.agent))?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let streams: Arc<Mutex<HashMap<String, mpsc::UnboundedSender<Update>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let stdin = Arc::new(Mutex::new(stdin));
        let acp = Acp {
            stdin: stdin.clone(),
            next_id: AtomicU64::new(1),
            pending: pending.clone(),
            streams: streams.clone(),
            _child: Mutex::new(child),
        };
        // Reader: responses settle pending requests; notifications feed the stream; requests from
        // the agent (permissions, fs) get answered inline.
        let permissions = args.permissions.clone();
        let writer = writer_task(stdin);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let v: Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => {
                        tracing::debug!(line = %line, "agent wrote a non-JSON line");
                        continue;
                    }
                };
                if let Some(id) = v.get("id").and_then(|i| i.as_u64()) {
                    if v.get("method").is_none() {
                        // A response to one of ours.
                        if let Some(tx) = pending.lock().await.remove(&id) {
                            let r = if let Some(e) = v.get("error") {
                                Err(anyhow!("{} ({})", e["message"], e["code"]))
                            } else {
                                Ok(v["result"].clone())
                            };
                            let _ = tx.send(r);
                        }
                        continue;
                    }
                    // A request from the agent.
                    let reply = handle_agent_request(&v, &permissions);
                    let _ = writer
                        .send(json!({ "jsonrpc": "2.0", "id": id, "result": reply }))
                        .await;
                    continue;
                }
                match v["method"].as_str() {
                    Some("session/update") => {
                        let sid = v["params"]["sessionId"].as_str().unwrap_or("");
                        let update = &v["params"]["update"];
                        let kind = update["sessionUpdate"].as_str().unwrap_or("");
                        let out = match kind {
                            "agent_message_chunk" => update["content"]["text"]
                                .as_str()
                                .map(|t| Update::Text(t.to_owned())),
                            "tool_call" | "tool_call_update" => update["title"]
                                .as_str()
                                .filter(|t| !t.trim().is_empty())
                                .map(|t| Update::Status(status_words(t))),
                            _ => None,
                        };
                        if let Some(u) = out {
                            if let Some(tx) = streams.lock().await.get(sid) {
                                let _ = tx.send(u);
                            }
                        }
                    }
                    Some(other) => tracing::debug!(method = %other, "agent notification ignored"),
                    None => {}
                }
            }
            tracing::error!("the ACP agent closed its stdout; exiting so a supervisor restarts us");
            std::process::exit(2);
        });
        Ok(acp)
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        let mut line =
            json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string();
        line.push('\n');
        self.stdin.lock().await.write_all(line.as_bytes()).await?;
        rx.await
            .map_err(|_| anyhow!("agent went away during {method}"))?
    }

    async fn subscribe(&self, session: &str, tx: mpsc::UnboundedSender<Update>) {
        self.streams.lock().await.insert(session.to_owned(), tx);
    }

    async fn unsubscribe(&self, session: &str) {
        if let Some(tx) = self.streams.lock().await.remove(session) {
            let _ = tx.send(Update::Done);
        }
    }
}

/// A small channel so the reader task can answer the agent's requests without holding stdin
/// across an await of its own.
fn writer_task(stdin: Arc<Mutex<ChildStdin>>) -> mpsc::Sender<Value> {
    let (tx, mut rx) = mpsc::channel::<Value>(32);
    tokio::spawn(async move {
        while let Some(v) = rx.recv().await {
            let mut line = v.to_string();
            line.push('\n');
            let mut s = stdin.lock().await;
            if s.write_all(line.as_bytes()).await.is_err() {
                break;
            }
        }
    });
    tx
}

/// Answer a request the agent makes of its client. Permissions follow `--permissions`; file and
/// terminal access were not advertised, so those come back as "method not found".
fn handle_agent_request(v: &Value, permissions: &str) -> Value {
    match v["method"].as_str().unwrap_or("") {
        "session/request_permission" => {
            let options = v["params"]["options"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let want = if permissions == "reject" {
                "reject_once"
            } else {
                "allow_once"
            };
            let pick = options
                .iter()
                .find(|o| o["kind"] == want)
                .or_else(|| {
                    options.iter().find(|o| {
                        o["kind"].as_str().is_some_and(|k| {
                            k.starts_with(if permissions == "reject" {
                                "reject"
                            } else {
                                "allow"
                            })
                        })
                    })
                })
                .or_else(|| options.first());
            let title = v["params"]["toolCall"]["title"]
                .as_str()
                .unwrap_or("a tool");
            match pick {
                Some(o) => {
                    tracing::info!(tool = %title, option = %o["name"], "permission answered");
                    json!({ "outcome": { "outcome": "selected", "optionId": o["optionId"] } })
                }
                None => json!({ "outcome": { "outcome": "cancelled" } }),
            }
        }
        other => {
            tracing::debug!(method = %other, "agent request not supported");
            json!({ "error": { "code": -32601, "message": "not supported by arbos-acp" } })
        }
    }
}

/// "Read file src/main.rs" → "reading src/main.rs": a status line in present tense, ≤ 40 chars.
fn status_words(title: &str) -> String {
    let t = title.trim();
    let lower = t.to_ascii_lowercase();
    let mapped = [
        ("read", "reading"),
        ("search", "searching"),
        ("grep", "searching"),
        ("list", "listing"),
        ("edit", "editing"),
        ("write", "writing"),
        ("run", "running"),
        ("execute", "running"),
        ("fetch", "fetching"),
        ("web", "searching the web"),
        ("think", "thinking"),
    ]
    .iter()
    .find(|(k, _)| lower.starts_with(k))
    .map(|(k, v)| {
        let rest =
            t[k.len()..].trim_start_matches(|c: char| !c.is_alphanumeric() && c != '/' && c != '.');
        if rest.is_empty() {
            (*v).to_owned()
        } else {
            format!("{v} {rest}")
        }
    })
    .unwrap_or_else(|| lower);
    mapped
        .chars()
        .take(40)
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_lines_read_well() {
        assert_eq!(
            status_words("Read file src/main.rs"),
            "reading file src/main.rs"
        );
        assert_eq!(status_words("Search: refunds"), "searching refunds");
        assert_eq!(status_words("Bash: cargo test"), "bash: cargo test");
        assert!(status_words(&"x".repeat(100)).len() <= 40);
    }

    #[test]
    fn permission_picks_allow_once() {
        let req = json!({ "method": "session/request_permission", "params": { "toolCall": { "title": "Edit" },
            "options": [ { "optionId": "r", "kind": "reject_once", "name": "No" }, { "optionId": "a", "kind": "allow_once", "name": "Yes" } ] } });
        let r = handle_agent_request(&req, "allow");
        assert_eq!(r["outcome"]["optionId"], "a");
        let r = handle_agent_request(&req, "reject");
        assert_eq!(r["outcome"]["optionId"], "r");
    }
}
