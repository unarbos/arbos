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
//! Run: `ARBOS_AGENT_KEY=arbk_… arbos-acp` (see `--help`).
//!
//! # Asking before acting (inbox 1460)
//!
//! A tool call here is a command on the owner's own computer, run because somebody sent a chat
//! message. Three defaults used to compose into running it with nothing asked, and an outside
//! Claude read the code and refused to connect rather than accept them:
//!
//! 1. `--permissions` defaulted to `allow`, so the bridge answered every permission request
//!    itself, with yes.
//! 2. Nothing checked who sent the message — only the relay's `wake` flag, which is `true` for
//!    every message in a DM and for an `@handle` in a group. Any member of a shared chat, and
//!    anyone an invite link let into it, could therefore ask for a command.
//! 3. `~/.claude/settings.json` can carry `defaultMode: bypassPermissions`, which the Claude
//!    adapter honours; in that mode it never asks at all, so `--permissions reject` — the one
//!    documented mitigation — had nothing to answer.
//!
//! What replaces them, each failing closed:
//!
//! * **The owner is asked.** A permission request becomes an `approval.request` on the relay,
//!   which raises an approval card in the owner's own chat with the agent and accepts only the
//!   owner's answer. The bridge waits for that answer and **refuses on anything else** — the
//!   card could not be raised, the relay could not be reached, the owner did not answer in
//!   time. A refusal carries the reason it refused, so "the owner said no" and "the owner was
//!   never asked" are never the same answer. There is no path from an unreachable owner to yes.
//! * **A sender allowlist**, `--allow`, defaulting to the owner plus the chats this agent was
//!   added to. A message from outside it is kept as context and never prompts the agent.
//!   `--answer-all` widens the relay's wake flag and cannot widen this.
//! * **The bypass is refused, not warned about.** Every session is opened with
//!   `allowDangerouslySkipPermissions: false`, which the adapter documents as removing
//!   `bypassPermissions` from the session — so the owner's `~/.claude/settings.json` needs no
//!   editing. The bridge then **checks that the refusal took** and will not run a session that
//!   still offers the mode, and clamps any other non-asking mode to one that asks. Wanting the
//!   bypass takes `--allow-bypass-permissions`, which is a flag on the owner's own command line
//!   and reachable from nowhere else.

use std::collections::{HashMap, HashSet, VecDeque};
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
/// How often the bridge asks the relay whether the owner has answered a card.
const APPROVAL_POLL_EVERY: Duration = Duration::from_secs(2);
/// The ACP agent started when `--agent` is left alone.
const DEFAULT_AGENT: &str = "claude-agent-acp";
/// The npm package that provides it, **pinned**: a floating `@latest` would let an npm publish
/// change what runs on the owner's computer between two starts of the same command.
const DEFAULT_AGENT_PACKAGE: &str = "@agentclientprotocol/claude-agent-acp@0.100.0";
/// Permission modes that ask before acting. Everything else the Claude adapter offers —
/// `acceptEdits`, `dontAsk`, `auto`, `bypassPermissions` — runs some class of tool with no card,
/// which is the defect this bridge exists to close, so a session that starts in one of those is
/// moved to `default` or not used at all.
const ASKING_MODES: &[&str] = &["default", "plan"];

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
    #[arg(long, env = "ARBOS_ACP_AGENT", default_value = DEFAULT_AGENT)]
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
    /// This widens which messages wake the agent; it cannot widen `--allow`.
    #[arg(long)]
    answer_all: bool,
    /// What to answer when the agent asks permission for a tool: `ask` the owner on an approval
    /// card (default), `reject` everything, or `allow` everything without asking.
    #[arg(long, env = "ARBOS_ACP_PERMISSIONS", default_value = "ask")]
    permissions: Permissions,
    /// Who may make this agent act, repeatable. `owner` = the person who owns the agent;
    /// `chats` = anyone in a chat it was added to; `@handle` = one person; `chat:<id>` = one
    /// chat. Default: `owner` and `chats`. A message from outside the list is kept as context
    /// and never prompts the agent.
    #[arg(long = "allow", env = "ARBOS_ACP_ALLOW", value_delimiter = ',')]
    allow: Vec<String>,
    /// How long the owner has to answer an approval card before the tool is refused.
    #[arg(long, env = "ARBOS_ACP_APPROVAL_TIMEOUT_SECS", default_value_t = 300)]
    approval_timeout_secs: u64,
    /// Let the agent keep `bypassPermissions` — every tool runs with nothing asked.
    ///
    /// Deliberately a flag and nothing else: no environment variable, no config file, nothing
    /// the relay or a working directory can reach. Turning the asking off has to be something
    /// the owner typed on their own command line.
    #[arg(long)]
    allow_bypass_permissions: bool,
    /// Extra instructions prepended to every prompt (how this agent should behave in chat).
    #[arg(long, env = "ARBOS_ACP_INSTRUCTIONS")]
    instructions: Option<String>,
    /// ACP auth method id to use when the agent lists several (e.g. `api-key`, `chat-gpt`).
    #[arg(long, env = "ARBOS_ACP_AUTH_METHOD")]
    auth_method: Option<String>,
}

/// What the bridge answers when the agent asks permission for a tool.
#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
#[value(rename_all = "lower")]
enum Permissions {
    /// Put it to the owner on an approval card and refuse unless they say yes.
    Ask,
    /// Refuse everything. The agent can talk and cannot act.
    Reject,
    /// Approve everything without asking anybody.
    Allow,
}

/// One `--allow` rule: who may make this agent act.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Allow {
    /// The person who owns the agent, wherever they write.
    Owner,
    /// Anyone writing in a chat this agent is a member of.
    Chats,
    /// One person, by GitHub login.
    Handle(String),
    /// One chat, by id.
    Chat(String),
}

impl Allow {
    fn parse(s: &str) -> Result<Allow> {
        let s = s.trim();
        Ok(match s {
            "owner" => Allow::Owner,
            "chats" => Allow::Chats,
            _ => {
                if let Some(h) = s.strip_prefix('@') {
                    Allow::Handle(h.to_ascii_lowercase())
                } else if let Some(c) = s.strip_prefix("chat:") {
                    Allow::Chat(c.to_owned())
                } else {
                    bail!("--allow {s}: expected `owner`, `chats`, `@handle` or `chat:<id>`");
                }
            }
        })
    }

    /// The rules in force: what the owner asked for, or the default.
    fn rules(args: &Args) -> Result<Vec<Allow>> {
        if args.allow.is_empty() {
            return Ok(vec![Allow::Owner, Allow::Chats]);
        }
        args.allow.iter().map(|s| Allow::parse(s)).collect()
    }

    fn describe(rules: &[Allow]) -> String {
        rules
            .iter()
            .map(|r| match r {
                Allow::Owner => "owner".to_owned(),
                Allow::Chats => "chats".to_owned(),
                Allow::Handle(h) => format!("@{h}"),
                Allow::Chat(c) => format!("chat:{c}"),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Why a message was or was not allowed to make the agent act. The refusal carries its reason
/// so the log says which rule was missing rather than only that something was dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SenderVerdict {
    Allowed(&'static str),
    Refused(String),
}

/// Is this message from somebody the owner allowed to make the agent act?
///
/// Fails closed in the one case that matters: a message the relay did not attribute — no sender
/// kind, or no sender id — matches no rule and is refused, rather than falling through to the
/// chat rules on the strength of the chat alone.
fn sender_verdict(
    rules: &[Allow],
    owner_id: Option<&str>,
    known_chats: &HashSet<String>,
    ev: &Value,
) -> SenderVerdict {
    let from = &ev["message"]["from"];
    let kind = from["kind"].as_str().unwrap_or_default();
    let id = from["id"].as_str().unwrap_or_default();
    let handle = from["handle"].as_str().unwrap_or_default().to_ascii_lowercase();
    let chat = ev["chat"]["id"].as_str().unwrap_or_default();
    if kind.is_empty() || id.is_empty() {
        return SenderVerdict::Refused(
            "the relay did not say who sent it, so no rule can allow it".to_owned(),
        );
    }
    for rule in rules {
        match rule {
            Allow::Owner if kind == "user" && Some(id) == owner_id => {
                return SenderVerdict::Allowed("owner")
            }
            Allow::Chats if known_chats.contains(chat) => return SenderVerdict::Allowed("chats"),
            Allow::Handle(h) if !handle.is_empty() && &handle == h => {
                return SenderVerdict::Allowed("handle")
            }
            Allow::Chat(c) if c == chat => return SenderVerdict::Allowed("chat"),
            _ => {}
        }
    }
    SenderVerdict::Refused(format!(
        "no --allow rule covers {} in chat {chat} (rules: {})",
        if handle.is_empty() {
            format!("{kind}:{id}")
        } else {
            format!("@{handle}")
        },
        Allow::describe(rules),
    ))
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
    let owner_id = me["agent"]["owner"]["id"].as_str().map(str::to_owned);
    let owner_handle = me["agent"]["owner"]["handle"]
        .as_str()
        .unwrap_or("")
        .to_owned();
    let known_chats: HashSet<String> = me["chats"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| c["id"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let rules = Allow::rules(&args)?;
    // An `owner` rule the relay cannot fill is a rule that allows nobody. Say so at startup
    // rather than letting every message be refused for a reason nobody reads.
    if rules.contains(&Allow::Owner) && owner_id.is_none() {
        bail!(
            "the relay did not name this agent's owner, so `--allow owner` can never match. \
             Rotate the key in the Arbos app, or name who may use the agent with `--allow @handle`."
        );
    }
    let mut cursor = if args.replay {
        0
    } else {
        me["inbox_head"].as_i64().unwrap_or(0)
    };
    tracing::info!(
        name = %agent_name, handle = %handle, protocol = %me["protocol"], chats = known_chats.len(),
        owner = %owner_handle, permissions = ?args.permissions, allow = %Allow::describe(&rules),
        "connected to the relay"
    );
    if args.permissions == Permissions::Allow {
        tracing::warn!(
            "--permissions allow: every tool this agent asks for runs with nobody asked. \
             Anyone who can get a message to it can run commands here."
        );
    }

    // The ACP agent. Permission requests cannot be answered inside the reader task — answering
    // one means asking the owner and waiting — so they come out on this channel instead.
    let (asks_tx, mut asks_rx) = mpsc::unbounded_channel::<PermissionAsk>();
    let acp = Acp::spawn(&args, asks_tx).await?;
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
        owner_id,
        rules,
        known_chats: Mutex::new(known_chats),
        sessions: Mutex::new(HashMap::new()),
        context: Mutex::new(HashMap::new()),
        queue: Mutex::new(VecDeque::new()),
        busy: Mutex::new(()),
        turn: Mutex::new(None),
        agent_info,
    });

    // Permission requests. One task drains the channel and gives each ask its own task, so an
    // owner who takes a minute to read a card does not stall the agent's output stream.
    {
        let b = bridge.clone();
        tokio::spawn(async move {
            while let Some(ask) = asks_rx.recv().await {
                let b = b.clone();
                tokio::spawn(async move {
                    let result = b.answer_permission(&ask.params).await;
                    b.acp.reply(ask.id, result).await;
                });
            }
        });
    }

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
    /// The person the relay says owns this agent; `--allow owner` means them.
    owner_id: Option<String>,
    /// `--allow` as parsed, or its default. Fixed at startup: nothing the relay sends can
    /// change who is allowed to make the agent act.
    rules: Vec<Allow>,
    /// Chats the agent is a member of, from `/asp/me` and kept fresh by `member.added`.
    known_chats: Mutex<HashSet<String>>,
    /// chat id → ACP session id.
    sessions: Mutex<HashMap<String, String>>,
    /// chat id → recent messages (who: text) for the prompt's context.
    context: Mutex<HashMap<String, VecDeque<String>>>,
    /// Wake events waiting while a prompt runs (one prompt at a time, like buzz-acp's queue).
    queue: Mutex<VecDeque<Value>>,
    busy: Mutex<()>,
    /// The message being answered. A permission request arrives mid-prompt with no reference to
    /// the chat, and the owner's card has to say which chat and which person it came from —
    /// "someone wants to run `rm -rf`" is not a question anybody can answer. One slot is
    /// enough: `busy` runs one prompt at a time.
    turn: Mutex<Option<Turn>>,
    agent_info: Value,
}

/// The message a prompt is answering, for the owner's card.
#[derive(Clone)]
struct Turn {
    chat: String,
    chat_title: String,
    from: String,
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
                session_params(&cwd.to_string_lossy(), self.args.allow_bypass_permissions),
            )
            .await
            .context("ACP session/new failed")?;
        let id = r["sessionId"]
            .as_str()
            .ok_or_else(|| anyhow!("session/new returned no sessionId"))?
            .to_owned();
        self.settle_mode(&id, &r).await?;
        tracing::info!(chat = %chat_id, session = %id, cwd = %cwd.display(), "ACP session opened");
        self.sessions
            .lock()
            .await
            .insert(chat_id.to_owned(), id.clone());
        Ok(id)
    }

    /// Check that the session really does ask before acting, and put it in a mode that does.
    ///
    /// Two separate things, because warning about a defence is not the same as having one:
    ///
    /// * `bypassPermissions` still in the mode catalogue after the bridge asked for it to be
    ///   dropped means the agent ignored the request. The session is refused — a bridge that
    ///   cannot promise every tool asks should not run one that does not.
    /// * A session that starts in any other non-asking mode (`acceptEdits`, `dontAsk`, `auto`,
    ///   from the owner's `~/.claude/settings.json`) is moved to `default`. If it will not
    ///   move, the session is refused.
    async fn settle_mode(&self, session: &str, new_session: &Value) -> Result<()> {
        match mode_verdict(self.args.allow_bypass_permissions, &new_session["modes"])? {
            ModeVerdict::Asks(note) => {
                tracing::info!(agent = %self.args.agent, note, "the session asks before acting");
                Ok(())
            }
            ModeVerdict::Clamp(current) => {
                tracing::warn!(
                    mode = %current,
                    "the agent started in a mode that does not ask before acting; moving it to `default`"
                );
                self.acp
                    .request(
                        "session/set_mode",
                        json!({ "sessionId": session, "modeId": "default" }),
                    )
                    .await
                    .with_context(|| {
                        format!(
                            "the agent started in `{current}`, which does not ask before acting, \
                             and would not change to `default`"
                        )
                    })?;
                Ok(())
            }
        }
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

    /// Answer one `session/request_permission`, and never answer it with yes by accident.
    ///
    /// Returns the JSON-RPC result for the request. Every path that is not the owner saying yes
    /// ends at [`Self::refuse`], which picks a reject option and cancels when there is none.
    async fn answer_permission(&self, params: &Value) -> Value {
        let title = params["toolCall"]["title"]
            .as_str()
            .unwrap_or("a tool")
            .to_owned();
        match self.args.permissions {
            Permissions::Allow => {
                let options = params["options"].as_array().cloned().unwrap_or_default();
                match option_of_kind(&options, "allow") {
                    Some(o) => {
                        tracing::warn!(tool = %title, option = %o["name"], "--permissions allow: approved without asking");
                        selected(o)
                    }
                    // Not even an allow option to pick: cancel. The old code fell back to
                    // `options.first()` here, which in reject mode could select an *allow*.
                    None => self.refuse(params, &title, "the agent offered no allow option"),
                }
            }
            Permissions::Reject => {
                self.refuse(params, &title, "--permissions reject is set on this bridge")
            }
            Permissions::Ask => match self.ask_owner(&title, params).await {
                Ok(option) => {
                    tracing::info!(tool = %title, "the owner approved it");
                    option
                }
                Err(why) => self.refuse(params, &title, &format!("{why:#}")),
            },
        }
    }

    /// Refuse a tool, saying why. The reason is the point: a bridge that refuses with only a
    /// status cannot tell the owner apart from the relay being unreachable, and those two must
    /// never be the same answer.
    ///
    /// When the agent offers no reject-shaped option the outcome is `cancelled`. It is never
    /// "whichever option came first", which is what the old picker did.
    fn refuse(&self, params: &Value, title: &str, why: &str) -> Value {
        let options = params["options"].as_array().cloned().unwrap_or_default();
        match option_of_kind(&options, "reject") {
            Some(o) => {
                tracing::info!(tool = %title, option = %o["name"], reason = %why, "tool refused");
                selected(o)
            }
            None => {
                tracing::info!(tool = %title, reason = %why, "tool cancelled: the agent offered no reject option");
                json!({ "outcome": { "outcome": "cancelled" } })
            }
        }
    }

    /// Put one tool call to the owner on an approval card and wait for their answer.
    ///
    /// `Ok` only when the owner approved it. Everything else is an `Err` whose message says what
    /// went wrong, and the caller turns that into a refusal: no card could be raised, the relay
    /// could not be reached, the owner said no, the owner did not answer in time. There is no
    /// branch from "could not ask" to "approved".
    async fn ask_owner(&self, title: &str, params: &Value) -> Result<Value> {
        let options = params["options"].as_array().cloned().unwrap_or_default();
        // Find the yes *before* asking: approving a card and then having nothing to answer the
        // agent with would run the tool's permission request into the ground after the owner
        // had already said yes.
        let allow = option_of_kind(&options, "allow")
            .cloned()
            .ok_or_else(|| anyhow!("the agent offered no option that would allow it"))?;
        let turn = self
            .turn
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow!("there is no chat to raise the owner's card from"))?;
        let timeout = Duration::from_secs(self.args.approval_timeout_secs.max(1));
        let call = &params["toolCall"];
        let r = self
            .post(json!({
                "type": "approval.request",
                "chat": turn.chat,
                "title": title,
                "summary": summary_for_owner(&turn, title, call),
                "risk": risk_of(call),
                "payload": call,
                "timeout_ms": timeout.as_millis() as i64,
            }))
            .await
            .context("the owner's card could not be raised")?;
        let id = r["approval"]
            .as_str()
            .ok_or_else(|| anyhow!("the relay accepted the request but raised no card"))?
            .to_owned();
        tracing::info!(tool = %title, approval = %id, chat = %turn.chat, "waiting for the owner");

        let deadline = tokio::time::Instant::now() + timeout;
        let mut last_error: Option<String> = None;
        loop {
            if tokio::time::Instant::now() >= deadline {
                // The card expires at the same instant on the relay, so nothing is left for
                // the owner to approve after this.
                return Err(match last_error {
                    Some(e) => anyhow!("the owner was not reached in time ({e})"),
                    None => anyhow!(
                        "the owner did not answer within {}s",
                        timeout.as_secs()
                    ),
                });
            }
            tokio::time::sleep(APPROVAL_POLL_EVERY).await;
            match self.approval_state(&id).await {
                Ok((state, reason)) => match card_answer(&state, reason) {
                    CardAnswer::Approved => return Ok(selected(&allow)),
                    CardAnswer::Refused(why) => return Err(anyhow!("{why}")),
                    // A `reason` while still pending is the relay reporting that somebody who
                    // is not the owner tried to answer; it is not a decision.
                    CardAnswer::Waiting(note) => {
                        if let Some(n) = note {
                            tracing::warn!(approval = %id, note = %n, "card still pending");
                        }
                    }
                },
                // Transient by assumption: keep asking until the deadline, which is the thing
                // that guarantees this ends, and ends in a refusal.
                Err(e) => {
                    let e = format!("{e:#}");
                    if last_error.as_deref() != Some(e.as_str()) {
                        tracing::warn!(approval = %id, error = %e, "could not read the card; still refusing until the owner is reached");
                    }
                    last_error = Some(e);
                }
            }
        }
    }

    /// `GET /asp/approvals/{id}` → `(state, reason)`.
    async fn approval_state(&self, approval_id: &str) -> Result<(String, Option<String>)> {
        let v: Value = self
            .http
            .get(format!("{}/asp/approvals/{approval_id}", self.relay))
            .bearer_auth(&self.args.key)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let state = v["state"]
            .as_str()
            .ok_or_else(|| anyhow!("the relay answered without a state"))?
            .to_owned();
        Ok((state, v["reason"].as_str().map(str::to_owned)))
    }

    /// Prompt the agent and stream its answer into the chat as one bubble.
    async fn answer(self: &Arc<Self>, ev: Value) -> Result<()> {
        let chat_id = ev["chat"]["id"].as_str().unwrap_or("").to_owned();
        let message_id = ev["message"]["id"].as_str().unwrap_or("").to_owned();
        *self.turn.lock().await = Some(Turn {
            chat: chat_id.clone(),
            chat_title: ev["chat"]["title"].as_str().unwrap_or("").to_owned(),
            from: who_sent(&ev),
        });
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
        // A permission request that somehow arrives after the prompt returned has no message to
        // name on a card, and `ask_owner` refuses without one rather than guessing a chat.
        *self.turn.lock().await = None;
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

/// The ACP result that picks one option the agent offered.
fn selected(option: &Value) -> Value {
    json!({ "outcome": { "outcome": "selected", "optionId": option["optionId"] } })
}

/// The one option of a given shape — `allow_once` before any other `allow…`, `reject_once`
/// before any other `reject…` — or `None`.
///
/// `None` matters more than the match does. The old picker ended in `options.first()`, and ACP
/// does not order options by safety: the Claude adapter lists the allow options first for its
/// elevated modes, so "the first option" in a refusal was an approval. Nothing here falls back
/// to an option of the wrong kind; the caller handles `None` by cancelling.
fn option_of_kind<'a>(options: &'a [Value], want: &str) -> Option<&'a Value> {
    let exact = format!("{want}_once");
    options
        .iter()
        .find(|o| o["kind"] == exact.as_str())
        .or_else(|| {
            options
                .iter()
                .find(|o| o["kind"].as_str().is_some_and(|k| k.starts_with(want)))
        })
}

/// `session/new` parameters. The bypass refusal is unconditional on everything except the
/// owner's own `--allow-bypass-permissions` flag.
///
/// `allowDangerouslySkipPermissions: false` is the Claude adapter's documented way for a client
/// to drop `bypassPermissions` from a session. It is one term of a conjunction with the settings
/// files, so once the client passes `false` no `settings.json` on any path — including one in
/// the working directory, which a repository the owner cloned can carry — puts the mode back.
/// That is why the refusal lives here and not in a file the owner has to edit.
fn session_params(cwd: &str, allow_bypass: bool) -> Value {
    let mut params = json!({ "cwd": cwd, "mcpServers": [] });
    if !allow_bypass {
        params["_meta"] = json!({
            "claudeCode": { "options": { "allowDangerouslySkipPermissions": false } }
        });
    }
    params
}

/// Whether a new session will ask before it acts.
#[derive(Debug)]
enum ModeVerdict {
    /// It asks. The string says how that was established, for the log.
    Asks(&'static str),
    /// It starts in a mode that does not ask; move it to `default`. Carries the mode it is in.
    Clamp(String),
}

/// Read a new session's mode catalogue and decide whether it can be used.
///
/// An `Err` refuses the session outright, and there is exactly one cause: `bypassPermissions`
/// still on offer after the client asked for it to be dropped. That is a self-check, and an
/// exact one — the adapter lists the mode if and only if it decided to allow it — so an agent
/// too old to know the flag is caught here instead of being silently trusted.
fn mode_verdict(allow_bypass: bool, modes: &Value) -> Result<ModeVerdict> {
    if allow_bypass {
        return Ok(ModeVerdict::Asks("--allow-bypass-permissions is set"));
    }
    let Some(available) = modes["availableModes"].as_array() else {
        // No modes concept at all (codex-acp, Goose). Nothing to clamp, and nothing missing:
        // the ACP contract still routes a tool through `session/request_permission`, and that
        // request is what the owner's card answers.
        return Ok(ModeVerdict::Asks(
            "the agent advertises no permission modes, so the owner's card is the only gate",
        ));
    };
    if available.iter().any(|m| m["id"] == "bypassPermissions") {
        bail!(
            "this agent still offers `bypassPermissions` after being asked to drop it, so \
             `allowDangerouslySkipPermissions: false` had no effect and the bridge cannot \
             promise that every tool asks. Update the ACP agent (arbos-acp expects \
             {DEFAULT_AGENT_PACKAGE} or newer), or pass --allow-bypass-permissions to accept \
             it deliberately."
        );
    }
    let current = modes["currentModeId"].as_str().unwrap_or_default();
    if current.is_empty() {
        return Ok(ModeVerdict::Asks("the agent named no current mode"));
    }
    if ASKING_MODES.contains(&current) {
        return Ok(ModeVerdict::Asks("the session started in an asking mode"));
    }
    Ok(ModeVerdict::Clamp(current.to_owned()))
}

/// What the relay's answer about a card means to the waiting bridge.
#[derive(Debug, PartialEq, Eq)]
enum CardAnswer {
    Approved,
    Refused(String),
    /// No decision yet. Carries anything the relay wanted noted.
    Waiting(Option<String>),
}

/// Map one `GET /asp/approvals/{id}` state to a decision.
///
/// Only the exact string `approved` approves. Every other state — including one this build has
/// never heard of — is `Waiting`, and waiting runs out at the caller's deadline and refuses. So
/// a relay that grows a new state cannot grow a new way to say yes.
fn card_answer(state: &str, reason: Option<String>) -> CardAnswer {
    match state {
        "approved" => CardAnswer::Approved,
        "rejected" => {
            CardAnswer::Refused(reason.unwrap_or_else(|| "the owner rejected it".to_owned()))
        }
        "expired" => CardAnswer::Refused(
            reason.unwrap_or_else(|| "the card expired unanswered".to_owned()),
        ),
        _ => CardAnswer::Waiting(reason),
    }
}

/// Who sent the message being answered, as a person would name them.
fn who_sent(ev: &Value) -> String {
    let from = &ev["message"]["from"];
    let name = from["name"].as_str().unwrap_or("someone");
    match from["handle"].as_str().filter(|h| !h.is_empty()) {
        Some(h) => format!("{name} (@{h})"),
        None => name.to_owned(),
    }
}

/// The card's body. The owner is being asked to let a command run on their own computer, so it
/// says what the command is and **who asked for it** — in a shared chat that is the whole
/// question, and "something wants to run `rm -rf`" is not one anybody can answer.
fn summary_for_owner(turn: &Turn, title: &str, call: &Value) -> String {
    let what = call["rawInput"]["command"]
        .as_str()
        .or_else(|| call["rawInput"]["file_path"].as_str())
        .unwrap_or(title);
    let where_ = if turn.chat_title.is_empty() {
        "a chat".to_owned()
    } else {
        format!("\"{}\"", turn.chat_title)
    };
    format!("{what}\n\nAsked by {} in {where_}.", turn.from)
}

/// The risk class for the card, from the ACP tool kind. An outside agent's tools run on the
/// owner's own computer, so `computer` is the floor rather than `other`.
fn risk_of(call: &Value) -> &'static str {
    match call["kind"].as_str().unwrap_or_default() {
        "delete" => "destructive",
        "fetch" => "send",
        _ => "computer",
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
                        // `--allow chats` means the chats the agent is in, so the set has to
                        // follow the agent in and out of them.
                        if let Some(id) = ev["chat"]["id"].as_str() {
                            let mut chats = bridge.known_chats.lock().await;
                            if ev["type"] == "member.added" {
                                chats.insert(id.to_owned());
                            } else {
                                chats.remove(id);
                            }
                        }
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
    // Asked before the wake flag, and deliberately: `--answer-all` widens which messages wake
    // the agent and must not be able to widen who may make it act.
    let allowed = {
        let chats = bridge.known_chats.lock().await;
        sender_verdict(&bridge.rules, bridge.owner_id.as_deref(), &chats, &ev)
    };
    match allowed {
        SenderVerdict::Allowed(rule) => tracing::debug!(chat = %chat_id, rule, "sender allowed"),
        SenderVerdict::Refused(why) => {
            // Kept as context above, never prompted: the agent can read what was said next time
            // the owner talks to it, and it will not act on it.
            tracing::info!(chat = %chat_id, from = %from, reason = %why, "message not answered: sender is not allowed");
            return;
        }
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

/// A `session/request_permission` on its way out of the reader task. It cannot be answered in
/// there: answering means asking the owner and waiting for them, and the reader has to keep
/// reading the agent's output while that happens.
struct PermissionAsk {
    id: u64,
    params: Value,
}

struct Acp {
    stdin: Arc<Mutex<ChildStdin>>,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>,
    streams: Arc<Mutex<HashMap<String, mpsc::UnboundedSender<Update>>>>,
    /// Answers to the agent's own requests go back through here.
    writer: mpsc::Sender<Value>,
    _child: Mutex<Child>,
}

/// Is this a runnable program on `PATH`? Hand-rolled to avoid a new dependency: adding one
/// would move `Cargo.lock`, which is both a Mac and an iOS release path.
fn on_path(program: &str) -> bool {
    if program.contains('/') {
        return std::path::Path::new(program).is_file();
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(program).is_file())
}

/// The command to start, as `(program, leading arguments)`.
///
/// When the default agent is not installed, start it with `npx` rather than failing. The global
/// npm install was a whole step of the connect instructions and one of two ecosystem installs,
/// and an outside agent that will not follow a four-command setup will not follow a
/// three-command one either. The package is pinned, so this fetches a known version rather than
/// whatever npm publishes next.
///
/// Only the default is resolved this way. An explicit `--agent` is taken literally — the owner
/// naming a program and getting a different one would be its own surprise.
fn resolve_agent(args: &Args) -> (String, Vec<String>) {
    if args.agent != DEFAULT_AGENT || on_path(&args.agent) {
        return (args.agent.clone(), Vec::new());
    }
    tracing::info!(
        package = DEFAULT_AGENT_PACKAGE,
        "`{DEFAULT_AGENT}` is not on PATH; starting it with npx"
    );
    (
        "npx".to_owned(),
        vec!["--yes".to_owned(), DEFAULT_AGENT_PACKAGE.to_owned()],
    )
}

impl Acp {
    async fn spawn(args: &Args, asks: mpsc::UnboundedSender<PermissionAsk>) -> Result<Acp> {
        let (program, lead) = resolve_agent(args);
        let mut cmd = Command::new(&program);
        cmd.args(lead.iter().chain(args.agent_args.iter().filter(|a| !a.is_empty())))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        if let Some(c) = &args.cwd {
            cmd.current_dir(c);
        }
        let mut child = cmd.spawn().with_context(|| {
            if program == "npx" {
                format!("could not start `{DEFAULT_AGENT}` with npx — is Node installed?")
            } else {
                format!("could not start the ACP agent `{program}`")
            }
        })?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let streams: Arc<Mutex<HashMap<String, mpsc::UnboundedSender<Update>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let stdin = Arc::new(Mutex::new(stdin));
        let writer = writer_task(stdin.clone());
        let acp = Acp {
            stdin: stdin.clone(),
            next_id: AtomicU64::new(1),
            pending: pending.clone(),
            streams: streams.clone(),
            writer: writer.clone(),
            _child: Mutex::new(child),
        };
        // Reader: responses settle pending requests; notifications feed the stream; a permission
        // request goes out to the owner on `asks`; anything else the agent asks of us is
        // answered inline as unsupported.
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
                    if v["method"] == "session/request_permission" {
                        let ask = PermissionAsk {
                            id,
                            params: v["params"].clone(),
                        };
                        if let Err(e) = asks.send(ask) {
                            // Nobody is left to ask the owner. Refusing is the only safe answer;
                            // answering on the owner's behalf is the bug this bridge fixed.
                            tracing::error!(error = %e, "no permission handler is listening; refusing the tool");
                            let _ = writer
                                .send(json!({ "jsonrpc": "2.0", "id": id,
                                    "result": { "outcome": { "outcome": "cancelled" } } }))
                                .await;
                        }
                        continue;
                    }
                    tracing::debug!(method = %v["method"], "agent request not supported");
                    let _ = writer
                        .send(json!({ "jsonrpc": "2.0", "id": id, "result": {
                            "error": { "code": -32601, "message": "not supported by arbos-acp" } } }))
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

    /// Answer one request the agent made of us.
    async fn reply(&self, id: u64, result: Value) {
        if let Err(e) = self
            .writer
            .send(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
            .await
        {
            tracing::warn!(error = %e, "could not answer the agent's request");
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

    /// `--permissions` is a choice between three things, and the default is the one that asks.
    /// A regression that put the default back to `allow` would restore the reported bug, so the
    /// default is asserted rather than assumed.
    #[test]
    fn asking_is_the_default() {
        use clap::Parser;
        let args = Args::parse_from(["arbos-acp", "--key", "arbk_x"]);
        assert_eq!(args.permissions, Permissions::Ask);
        assert!(
            !args.allow_bypass_permissions,
            "the bypass is refused unless the owner asks for it"
        );
        assert_eq!(args.approval_timeout_secs, 300);
    }

    /// `--allow-bypass-permissions` has to come from the owner's own command line. It is not an
    /// environment variable and not a config value, so nothing the relay sends and no file in
    /// the working directory can set it.
    #[test]
    fn the_bypass_opt_in_is_reachable_only_from_the_command_line() {
        use clap::CommandFactory;
        let arg = Args::command()
            .get_arguments()
            .find(|a| a.get_id() == "allow_bypass_permissions")
            .expect("the flag exists")
            .clone();
        assert!(
            arg.get_env().is_none(),
            "no environment variable may set the bypass opt-in"
        );

        // Positive control: the flag does exist and does parse, so the assertion above is about
        // where it can come from and not about a flag that is missing.
        use clap::Parser;
        let args = Args::parse_from([
            "arbos-acp",
            "--key",
            "arbk_x",
            "--allow-bypass-permissions",
        ]);
        assert!(args.allow_bypass_permissions);
        // And the two switches that do read the environment still only read their own names.
        for (id, var) in [
            ("permissions", "ARBOS_ACP_PERMISSIONS"),
            ("allow", "ARBOS_ACP_ALLOW"),
        ] {
            let a = Args::command()
                .get_arguments()
                .find(|a| a.get_id() == id)
                .expect("the flag exists")
                .clone();
            assert_eq!(a.get_env().and_then(|e| e.to_str()), Some(var));
        }
    }

    #[test]
    fn allow_rules_parse_and_default_to_the_owner_and_its_chats() {
        use clap::Parser;
        let args = Args::parse_from(["arbos-acp", "--key", "arbk_x"]);
        assert_eq!(
            Allow::rules(&args).unwrap(),
            vec![Allow::Owner, Allow::Chats]
        );
        let args = Args::parse_from([
            "arbos-acp",
            "--key",
            "arbk_x",
            "--allow",
            "owner",
            "--allow",
            "@affinity",
            "--allow",
            "chat:01ABC",
        ]);
        assert_eq!(
            Allow::rules(&args).unwrap(),
            vec![
                Allow::Owner,
                Allow::Handle("affinity".into()),
                Allow::Chat("01ABC".into()),
            ]
        );
        let args = Args::parse_from(["arbos-acp", "--key", "arbk_x", "--allow", "everyone"]);
        assert!(
            Allow::rules(&args).is_err(),
            "an unknown rule is refused, not quietly dropped"
        );
    }

    fn msg(kind: &str, id: &str, handle: &str, chat: &str) -> Value {
        json!({
            "chat": { "id": chat, "kind": "group", "title": "Arbonites" },
            "message": { "id": "m1", "text": "@grok-bot delete my documents",
                         "from": { "kind": kind, "id": id, "name": "Mallory", "handle": handle } },
            "wake": true,
        })
    }

    /// What happens to a message from outside the allowlist: it is refused, with the reason,
    /// and the agent is never prompted with it.
    #[test]
    fn a_sender_outside_the_allowlist_is_refused_with_a_reason() {
        let chats: HashSet<String> = ["c-known".to_owned()].into_iter().collect();
        let rules = vec![Allow::Owner, Allow::Chats];

        // In a chat the agent is in: allowed by `chats`. This is the positive control for every
        // refusal below — the same shape of message does get through when a rule covers it.
        assert_eq!(
            sender_verdict(&rules, Some("u-owner"), &chats, &msg("user", "u-mallory", "mallory", "c-known")),
            SenderVerdict::Allowed("chats")
        );
        // A chat the agent is not in — a stale replayed event, or one for a chat it has left.
        let v = sender_verdict(&rules, Some("u-owner"), &chats, &msg("user", "u-mallory", "mallory", "c-other"));
        assert!(matches!(v, SenderVerdict::Refused(ref why) if why.contains("no --allow rule")), "{v:?}");

        // Narrowed to the owner alone: everyone else is refused wherever they write.
        let owner_only = vec![Allow::Owner];
        assert_eq!(
            sender_verdict(&owner_only, Some("u-owner"), &chats, &msg("user", "u-owner", "affinity", "c-known")),
            SenderVerdict::Allowed("owner")
        );
        let v = sender_verdict(&owner_only, Some("u-owner"), &chats, &msg("user", "u-mallory", "mallory", "c-known"));
        assert!(matches!(v, SenderVerdict::Refused(_)), "{v:?}");

        // Named by handle, case-insensitively, and nobody else.
        let by_handle = vec![Allow::Handle("mallory".into())];
        assert_eq!(
            sender_verdict(&by_handle, Some("u-owner"), &chats, &msg("user", "u-mallory", "MALLORY", "c-known")),
            SenderVerdict::Allowed("handle")
        );
        let v = sender_verdict(&by_handle, Some("u-owner"), &chats, &msg("user", "u-x", "someone", "c-known"));
        assert!(matches!(v, SenderVerdict::Refused(_)), "{v:?}");
    }

    /// A message the relay did not attribute is refused outright. It must not fall through to
    /// the chat rules, which would let an unattributed message in on the strength of its chat.
    #[test]
    fn an_unattributed_sender_is_refused_even_in_a_known_chat() {
        let chats: HashSet<String> = ["c-known".to_owned()].into_iter().collect();
        let rules = vec![Allow::Owner, Allow::Chats];
        for bad in [
            msg("", "u-mallory", "mallory", "c-known"),
            msg("user", "", "mallory", "c-known"),
        ] {
            let v = sender_verdict(&rules, Some("u-owner"), &chats, &bad);
            assert!(
                matches!(v, SenderVerdict::Refused(ref why) if why.contains("did not say who sent it")),
                "{v:?}"
            );
        }
        // Positive control: the same chat and the same rules admit an attributed sender, so the
        // refusals above are about the missing attribution.
        assert_eq!(
            sender_verdict(&rules, Some("u-owner"), &chats, &msg("user", "u-mallory", "mallory", "c-known")),
            SenderVerdict::Allowed("chats")
        );
    }

    /// The `owner` rule cannot be satisfied by an agent whose owner the relay did not name.
    #[test]
    fn an_unnamed_owner_matches_nobody() {
        let chats = HashSet::new();
        let v = sender_verdict(&[Allow::Owner], None, &chats, &msg("user", "u-owner", "affinity", "c"));
        assert!(matches!(v, SenderVerdict::Refused(_)), "{v:?}");
    }

    /// The card says what would run and who asked for it. Without the asker, a card raised from
    /// a shared chat is unanswerable.
    #[test]
    fn the_card_names_the_command_and_who_asked() {
        let turn = Turn {
            chat: "c1".into(),
            chat_title: "Arbonites".into(),
            from: "Mallory (@mallory)".into(),
        };
        let call = json!({ "title": "Bash", "kind": "execute",
                           "rawInput": { "command": "rm -rf ~/Documents" } });
        let s = summary_for_owner(&turn, "Bash", &call);
        assert!(s.contains("rm -rf ~/Documents"), "{s}");
        assert!(s.contains("Mallory (@mallory)"), "{s}");
        assert!(s.contains("Arbonites"), "{s}");
        assert_eq!(risk_of(&call), "computer");
        assert_eq!(risk_of(&json!({ "kind": "delete" })), "destructive");
        // A tool kind nobody mapped is still a command on the owner's computer.
        assert_eq!(risk_of(&json!({ "kind": "something-new" })), "computer");
    }

    /// A refusal never picks an option of the wrong kind. The old picker ended in
    /// `options.first()`, and because the Claude adapter lists allow options first, that was a
    /// path from "refuse this" to "run it".
    #[test]
    fn a_refusal_cancels_rather_than_picking_the_first_option() {
        let allow_first = vec![
            json!({ "optionId": "yes", "kind": "allow_once", "name": "Yes" }),
            json!({ "optionId": "always", "kind": "allow_always", "name": "Yes, always" }),
        ];
        assert!(
            option_of_kind(&allow_first, "reject").is_none(),
            "no reject option exists, so none may be returned"
        );
        // Positive control: the same list does yield its allow option, so the `None` above is
        // about the kind asked for and not about a picker that never matches anything.
        assert_eq!(
            option_of_kind(&allow_first, "allow").unwrap()["optionId"],
            "yes"
        );
        // `allow_once` is preferred over `allow_always` even when it is listed second, and the
        // same for the reject side.
        let mixed = vec![
            json!({ "optionId": "always", "kind": "allow_always" }),
            json!({ "optionId": "once", "kind": "allow_once" }),
            json!({ "optionId": "never", "kind": "reject_always" }),
            json!({ "optionId": "no", "kind": "reject_once" }),
        ];
        assert_eq!(option_of_kind(&mixed, "allow").unwrap()["optionId"], "once");
        assert_eq!(option_of_kind(&mixed, "reject").unwrap()["optionId"], "no");
        // Only a `reject_always` on offer: it is still a refusal, so it is used.
        let only_always = vec![json!({ "optionId": "never", "kind": "reject_always" })];
        assert_eq!(
            option_of_kind(&only_always, "reject").unwrap()["optionId"],
            "never"
        );
    }

    /// Every session refuses the bypass, and the refusal is in the request rather than in a
    /// file. A `.claude/settings.json` inside the working directory is attacker-reachable — it
    /// comes with any repository the owner cloned — and cannot undo this, because the adapter
    /// ands the client's `false` with the settings.
    #[test]
    fn every_session_refuses_the_bypass_unless_the_owner_opted_in() {
        let p = session_params("/home/affinity/work", false);
        assert_eq!(
            p["_meta"]["claudeCode"]["options"]["allowDangerouslySkipPermissions"],
            json!(false)
        );
        assert_eq!(p["cwd"], "/home/affinity/work");
        // Positive control: the opt-in is the only thing that removes it, so the assertion
        // above is about the default and not about a key that is always present.
        let p = session_params("/home/affinity/work", true);
        assert!(p.get("_meta").is_none());
    }

    /// The bridge checks that its own refusal took, instead of trusting it. An agent that still
    /// offers `bypassPermissions` is one whose sessions might run a tool with nothing asked, so
    /// no session is opened on it at all.
    #[test]
    fn a_session_that_still_offers_the_bypass_is_refused() {
        let offers_bypass = json!({
            "currentModeId": "default",
            "availableModes": [ { "id": "default" }, { "id": "bypassPermissions" } ],
        });
        let e = mode_verdict(false, &offers_bypass)
            .expect_err("a session that can bypass asking is not usable");
        let e = format!("{e:#}");
        assert!(e.contains("still offers `bypassPermissions`"), "{e}");
        assert!(
            e.contains("--allow-bypass-permissions"),
            "the refusal says what the owner would do instead: {e}"
        );

        // Positive control: the same catalogue without the mode is accepted, so the refusal is
        // about the bypass and not about every catalogue.
        let asks = json!({
            "currentModeId": "default",
            "availableModes": [ { "id": "default" }, { "id": "plan" } ],
        });
        assert!(matches!(
            mode_verdict(false, &asks).unwrap(),
            ModeVerdict::Asks(_)
        ));

        // A session that starts in a mode which skips cards for some tools — `acceptEdits` and
        // `auto` come straight from the owner's `~/.claude/settings.json` — is moved to one
        // that asks. Dropping the bypass alone would have left these.
        for mode in ["acceptEdits", "auto", "dontAsk"] {
            let v = mode_verdict(
                false,
                &json!({ "currentModeId": mode, "availableModes": [ { "id": "default" }, { "id": mode } ] }),
            )
            .unwrap();
            assert!(
                matches!(v, ModeVerdict::Clamp(ref m) if m == mode),
                "{mode} does not ask, so it is clamped"
            );
        }

        // An agent with no modes at all (codex-acp, Goose) is fine: the permission request is
        // still the gate.
        assert!(matches!(
            mode_verdict(false, &json!({})).unwrap(),
            ModeVerdict::Asks(_)
        ));
    }

    /// Only the exact word `approved` is a yes. A state this build does not know keeps the
    /// bridge waiting, and waiting ends at the deadline in a refusal.
    #[test]
    fn only_an_explicit_approval_is_a_yes() {
        assert_eq!(card_answer("approved", None), CardAnswer::Approved);
        assert_eq!(
            card_answer("rejected", Some("the owner said no".into())),
            CardAnswer::Refused("the owner said no".into())
        );
        // A refusal always carries a reason, even when the relay sends none, so "the owner said
        // no" and "nobody was asked" are never the same answer in a log.
        assert_eq!(
            card_answer("rejected", None),
            CardAnswer::Refused("the owner rejected it".into())
        );
        assert_eq!(
            card_answer("expired", None),
            CardAnswer::Refused("the card expired unanswered".into())
        );
        for other in ["pending", "Approved", "approved ", "ok", "", "allowed"] {
            assert_eq!(
                card_answer(other, None),
                CardAnswer::Waiting(None),
                "{other:?} is not an approval"
            );
        }
    }

    /// An explicit `--agent` is taken literally; only the default falls back to `npx`, and the
    /// fallback pins a version rather than fetching whatever npm has published.
    #[test]
    fn only_the_default_agent_falls_back_to_npx() {
        use clap::Parser;
        let args = Args::parse_from(["arbos-acp", "--key", "arbk_x", "--agent", "codex-acp"]);
        assert_eq!(resolve_agent(&args), ("codex-acp".to_owned(), vec![]));

        let args = Args::parse_from(["arbos-acp", "--key", "arbk_x"]);
        let (program, lead) = resolve_agent(&args);
        if on_path(DEFAULT_AGENT) {
            assert_eq!((program.as_str(), lead.as_slice()), (DEFAULT_AGENT, &[][..]));
        } else {
            assert_eq!(program, "npx");
            assert_eq!(lead, vec!["--yes".to_owned(), DEFAULT_AGENT_PACKAGE.to_owned()]);
            assert!(
                DEFAULT_AGENT_PACKAGE.contains('@') && !DEFAULT_AGENT_PACKAGE.ends_with("latest"),
                "the fallback pins a version: {DEFAULT_AGENT_PACKAGE}"
            );
        }
    }
}
