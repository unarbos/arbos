//! `arbos-sidecar` — the Arbos sidecar for outside agents (spec: `docs/outside-agents-sidecar-spec.md`
//! in the project store; relay side #2189, this crate #2190).
//!
//! One small program on the agent's own machine: `run` holds the relay connection and mirrors
//! chats to files, nudges the agent through a pluggable waker, serves a control socket; `mcp`
//! is a stdio MCP server over the mirror; the other verbs are the same tools from a shell.

mod config;
mod control;
mod daemon;
mod log;
mod mcp;
mod mirror;
mod redact;
mod relay;
mod service;
mod session;
mod turns;
mod waker;

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};

use config::{Config, Paths, WakerConfig, WakerKind};

#[derive(Parser)]
#[command(
    name = "arbos-sidecar",
    version,
    about = "Keep your agent connected to Arbos"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Store the key (asked for with echo off), write config.toml, pick the waker.
    Init {
        #[arg(long, default_value = "https://relay.arbos.life")]
        relay: String,
        /// webhook | command | file | none
        #[arg(long, default_value = "none")]
        waker: String,
        #[arg(long)]
        webhook_url_file: Option<String>,
        #[arg(long)]
        webhook_auth_file: Option<String>,
        /// Non-secret webhook URL (local catchers); prefer --webhook-url-file.
        #[arg(long)]
        webhook_url: Option<String>,
        /// Command waker, one string: 'claude -p {prompt}' (stateless; prefer --agent)
        #[arg(long)]
        command: Option<String>,
        /// Command waker with a persistent session: claude | codex | cursor | hermes
        #[arg(long)]
        agent: Option<String>,
        /// Custom session templates ({session} is the id): 'mybot --new {session} {prompt}'
        #[arg(long)]
        command_new: Option<String>,
        #[arg(long)]
        command_resume: Option<String>,
        #[arg(long)]
        timeout_s: Option<u64>,
        /// Read the key from stdin instead of the terminal.
        #[arg(long)]
        key_stdin: bool,
        /// Adopt an existing 0600 key file instead of writing one.
        #[arg(long)]
        key_file: Option<PathBuf>,
        /// Rows the relay attaches to wake events (0–20).
        #[arg(long)]
        context: Option<usize>,
    },
    /// The daemon: connect, mirror, nudge, serve the control socket.
    Run,
    /// Is the daemon up, what is mirrored, when was the last nudge.
    Status,
    /// Send one test nudge through the configured waker now.
    TestWake,
    /// Install the user service (systemd --user / launchd) and start it.
    InstallService,
    /// Stop and remove the service; --purge also deletes the key and the mirror.
    Uninstall {
        #[arg(long)]
        purge: bool,
    },
    /// Serve MCP over stdio.
    Mcp,
    /// Open your reply bubble with one line naming your first step (then `reply`).
    Ack {
        chat: String,
        message: String,
        text: String,
    },
    /// The owner-facing session log (newest first; no message text).
    Log {
        #[arg(long)]
        chat: Option<String>,
        /// wake | ack | reply | error | session
        #[arg(long)]
        kind: Option<String>,
        #[arg(short = 'n', long, default_value_t = 50)]
        limit: u64,
    },
    /// The agent session record; `session rotate` starts a new one on the next wake.
    Session {
        #[arg(value_parser = ["show", "rotate"], default_value = "show")]
        action: String,
    },
    /// Your chats.
    Chats,
    /// Wake rows you have not answered.
    Pending,
    /// Rows of one chat, oldest first.
    Read {
        chat: String,
        #[arg(long, default_value_t = 40)]
        limit: u64,
        #[arg(long)]
        before: Option<String>,
    },
    /// Rows matching a phrase.
    Search {
        query: String,
        #[arg(long)]
        chat: Option<String>,
    },
    /// Who is in a chat.
    Members { chat: String },
    /// The chat's brief.
    Brief { chat: String },
    /// Download an attachment to the mirror.
    File { chat: String, blob: String },
    /// Say something.
    Post { chat: String, text: String },
    /// Answer one message.
    Reply {
        chat: String,
        message: String,
        text: String,
    },
    /// React to a message.
    React {
        chat: String,
        message: String,
        emoji: String,
        #[arg(long)]
        off: bool,
    },
    /// Show typing.
    Typing {
        chat: String,
        #[arg(long)]
        off: bool,
    },
    /// Set the working line (≤ 40 chars).
    StatusLine { chat: String, text: String },
    /// Who you are, your scopes, sidecar health.
    Me,
}

fn init_logging(paths: &Paths, to_file: bool) {
    let file = if to_file {
        std::fs::create_dir_all(&paths.data_dir).ok();
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(paths.log_file())
            .ok()
    } else {
        None
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(redact::RedactingWriter::new(file))
        .with_target(false)
        .try_init();
}

fn print(v: &Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
}

#[allow(clippy::too_many_arguments)]
fn init(
    paths: &Paths,
    relay: String,
    waker: String,
    webhook_url_file: Option<String>,
    webhook_auth_file: Option<String>,
    webhook_url: Option<String>,
    command: Option<String>,
    agent: Option<String>,
    command_new: Option<String>,
    command_resume: Option<String>,
    timeout_s: Option<u64>,
    key_stdin: bool,
    key_file: Option<PathBuf>,
    context: Option<usize>,
) -> Result<()> {
    paths.ensure()?;
    let kind: WakerKind = waker.parse()?;
    let mut w = WakerConfig {
        kind: kind.clone(),
        ..Default::default()
    };
    match kind {
        WakerKind::Webhook => {
            if webhook_url_file.is_none() && webhook_url.is_none() {
                bail!("--waker webhook needs --webhook-url-file (and usually --webhook-auth-file)");
            }
            if let Some(f) = &webhook_url_file {
                config::check_private(&config::expand_home(f))?;
            }
            if let Some(f) = &webhook_auth_file {
                config::check_private(&config::expand_home(f))?;
            }
            w.url_file = webhook_url_file;
            w.auth_file = webhook_auth_file;
            w.url = webhook_url;
        }
        WakerKind::Command => {
            if let Some(a) = &agent {
                if session::preset(a).is_none() {
                    bail!("--agent must be claude | codex | cursor | hermes (or use --command-new/--command-resume)");
                }
                w.agent = Some(a.clone());
            } else if let (Some(n), Some(r)) = (&command_new, &command_resume) {
                w.command_new = Some(shell_words(n));
                w.command_resume = Some(shell_words(r));
            } else {
                let c = command.context(
                    "--waker command needs --agent claude|codex|cursor|hermes, or --command 'claude -p {prompt}'",
                )?;
                let argv = shell_words(&c);
                if argv.is_empty() {
                    bail!("empty --command");
                }
                w.command = Some(argv);
            }
        }
        WakerKind::File | WakerKind::None => {}
    }
    if let Some(t) = timeout_s {
        w.timeout_s = t;
    }
    let existing = Config::load(paths).ok();
    let mut mirror = existing.map(|c| c.mirror).unwrap_or_default();
    if let Some(c) = context {
        mirror.context = c.min(20);
    }
    let key_path = paths.key_file();
    let cfg = Config {
        relay: relay.trim_end_matches('/').to_owned(),
        key_file: key_file.as_ref().map(|p| p.display().to_string()),
        waker: w,
        mirror,
    };
    match &key_file {
        Some(p) => {
            config::read_key(p)?;
        }
        None => {
            if key_path.exists() && !key_stdin && std::env::var_os("ARBOS_SIDECAR_REKEY").is_none()
            {
                config::read_key(&key_path)?;
                eprintln!("Keeping the stored key (set ARBOS_SIDECAR_REKEY=1 to replace it).");
            } else {
                let key = if key_stdin {
                    config::read_key_from_stdin()?
                } else {
                    config::prompt_key_from_tty()?
                };
                config::store_key(&key_path, &key)?;
                eprintln!("Key stored in {} (mode 600).", key_path.display());
            }
        }
    }
    cfg.save(paths)?;
    eprintln!(
        "Config written to {} — waker: {}{}.",
        paths.config_file().display(),
        waker,
        agent
            .map(|a| format!(" ({a} session preset)"))
            .unwrap_or_default()
    );
    Ok(())
}

/// Split a command line on spaces, honouring single and double quotes.
fn shell_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut had = false;
    for c in s.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '\'') | (None, '"') => {
                quote = Some(c);
                had = true;
            }
            (None, ' ') | (None, '\t') => {
                if had || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    had = false;
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if had || !cur.is_empty() {
        out.push(cur);
    }
    out
}

async fn status(paths: &Paths) -> Result<()> {
    let cfg = Config::load(paths).ok();
    let m = mirror::Mirror::new(paths.clone());
    let state = m.state();
    let alive = daemon::daemon_alive(paths).await;
    let me = if alive {
        control::call(paths, &json!({ "op": "me" })).await.ok()
    } else {
        None
    };
    let chats = m.chats();
    let last = m.last_wake();
    let waker = cfg.as_ref().map(|c| c.waker.clone()).unwrap_or_default();
    let w = json!({
        "kind": waker.kind,
        "url_file": waker.url_file.as_ref().map(|_| "set"),
        "auth_file": waker.auth_file.as_ref().map(|_| "set"),
        "command": waker.command.as_ref().map(|c| c.first().cloned().unwrap_or_default()),
        "debounce_s": waker.debounce_s,
    });
    print(&json!({
        "running": alive,
        "relay": cfg.as_ref().map(|c| c.relay.clone()),
        "agent": me.as_ref().map(|v| v["agent"].clone()).unwrap_or(state.agent),
        "scopes": me.as_ref().map(|v| v["scopes"].clone()).unwrap_or(Value::Null),
        "cursor": state.cursor,
        "last_event_at": if state.updated > 0 { Some(state.updated) } else { None },
        "chats": chats.len(),
        "rows": chats.iter().map(|c| m.rows(&c.id).len()).sum::<usize>(),
        "pending": m.pending(waker.stale_wake_s as i64 * 1000).len(),
        "waker": w,
        "last_wake": last,
        "session": cfg.as_ref().filter(|c| c.waker.agent.is_some() || c.waker.command_new.is_some()).map(|c| session::Store::new(m.clone(), c.waker.max_turns_per_session).json()),
        "log": log::SessionLog::new(paths).summary(),
        "paths": { "config": paths.config_dir, "data": paths.data_dir, "log": paths.log_file() },
    }));
    Ok(())
}

async fn test_wake(paths: &Paths, cfg: &Config) -> Result<()> {
    let m = mirror::Mirror::new(paths.clone());
    let plugin = waker::Plugin::from_config(&cfg.waker, paths.wakes_file())?;
    let agent = m.state().agent;
    let engine = waker::Engine::new(cfg.waker.clone(), plugin, m, agent);
    match engine.test_wake().await {
        Ok(s) => {
            println!("test nudge delivered ({s})");
            Ok(())
        }
        Err(e) => bail!(
            "test nudge not delivered: {}",
            redact::redact(&format!("{e:#}"))
        ),
    }
}

fn tool_out(v: Value) -> Result<()> {
    if v["isError"] == true {
        bail!("{}", v["content"][0]["text"].as_str().unwrap_or("failed"));
    }
    let text = v["content"][0]["text"].as_str().unwrap_or("");
    match serde_json::from_str::<Value>(text) {
        Ok(j) => print(&j),
        Err(_) => println!("{text}"),
    }
    Ok(())
}

fn main() {
    let code = match real_main() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("arbos-sidecar: {}", redact::redact(&format!("{e:#}")));
            1
        }
    };
    std::process::exit(code);
}

fn real_main() -> Result<i32> {
    let cli = Cli::parse();
    let paths = Paths::resolve();
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        match cli.cmd {
            Cmd::Init {
                relay,
                waker,
                webhook_url_file,
                webhook_auth_file,
                webhook_url,
                command,
                agent,
                command_new,
                command_resume,
                timeout_s,
                key_stdin,
                key_file,
                context,
            } => {
                init(
                    &paths,
                    relay,
                    waker,
                    webhook_url_file,
                    webhook_auth_file,
                    webhook_url,
                    command,
                    agent,
                    command_new,
                    command_resume,
                    timeout_s,
                    key_stdin,
                    key_file,
                    context,
                )?;
                Ok(0)
            }
            Cmd::Run => {
                init_logging(&paths, true);
                let cfg = Config::load(&paths)?;
                daemon::run(paths, cfg).await
            }
            Cmd::Status => {
                status(&paths).await?;
                Ok(0)
            }
            Cmd::TestWake => {
                init_logging(&paths, false);
                let cfg = Config::load(&paths)?;
                test_wake(&paths, &cfg).await?;
                Ok(0)
            }
            Cmd::InstallService => {
                Config::load(&paths)?;
                println!("{}", service::install(&paths)?);
                Ok(0)
            }
            Cmd::Uninstall { purge } => {
                println!("{}", service::uninstall(&paths, purge)?);
                Ok(0)
            }
            Cmd::Mcp => {
                let cfg = Config::load(&paths).ok();
                mcp::serve(paths, cfg).await?;
                Ok(0)
            }
            Cmd::Session { action } => {
                let op = if action == "rotate" {
                    "session_rotate"
                } else {
                    "session"
                };
                match control::call(&paths, &json!({ "op": op })).await {
                    Ok(v) => print(&v),
                    Err(_) => {
                        let m = mirror::Mirror::new(paths.clone());
                        let cfg = Config::load(&paths)?;
                        let st = session::Store::new(m, cfg.waker.max_turns_per_session);
                        if action == "rotate" {
                            st.request_rotation()?;
                        }
                        print(&st.json());
                    }
                }
                Ok(0)
            }
            other => {
                let cfg = Config::load(&paths).ok();
                let (name, args) = match other {
                    Cmd::Chats => ("arbos_chats", json!({})),
                    Cmd::Pending => ("arbos_pending", json!({})),
                    Cmd::Ack {
                        chat,
                        message,
                        text,
                    } => (
                        "arbos_ack",
                        json!({ "chat": chat, "message": message, "text": text }),
                    ),
                    Cmd::Log { chat, kind, limit } => (
                        "arbos_log",
                        json!({ "chat": chat, "kind": kind, "limit": limit }),
                    ),
                    Cmd::Read {
                        chat,
                        limit,
                        before,
                    } => (
                        "arbos_read_chat",
                        json!({ "chat": chat, "limit": limit, "before": before }),
                    ),
                    Cmd::Search { query, chat } => {
                        ("arbos_search", json!({ "query": query, "chat": chat }))
                    }
                    Cmd::Members { chat } => ("arbos_members", json!({ "chat": chat })),
                    Cmd::Brief { chat } => ("arbos_brief", json!({ "chat": chat })),
                    Cmd::File { chat, blob } => {
                        ("arbos_fetch_file", json!({ "chat": chat, "blob": blob }))
                    }
                    Cmd::Post { chat, text } => {
                        ("arbos_post", json!({ "chat": chat, "text": text }))
                    }
                    Cmd::Reply {
                        chat,
                        message,
                        text,
                    } => (
                        "arbos_reply",
                        json!({ "chat": chat, "message": message, "text": text }),
                    ),
                    Cmd::React {
                        chat,
                        message,
                        emoji,
                        off,
                    } => (
                        "arbos_react",
                        json!({ "chat": chat, "message": message, "emoji": emoji, "on": !off }),
                    ),
                    Cmd::Typing { chat, off } => {
                        ("arbos_typing", json!({ "chat": chat, "on": !off }))
                    }
                    Cmd::StatusLine { chat, text } => {
                        ("arbos_status", json!({ "chat": chat, "text": text }))
                    }
                    Cmd::Me => ("arbos_me", json!({})),
                    _ => unreachable!(),
                };
                tool_out(mcp::call_tool(&paths, cfg.as_ref(), name, &args).await)?;
                Ok(0)
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_words_handles_quotes() {
        assert_eq!(
            shell_words("claude -p '{prompt}'"),
            vec!["claude", "-p", "{prompt}"]
        );
        assert_eq!(
            shell_words("sh -c \"echo hi there\" x"),
            vec!["sh", "-c", "echo hi there", "x"]
        );
        assert_eq!(shell_words("  a   b "), vec!["a", "b"]);
    }

    #[test]
    fn mirror_config_default_context_is_off() {
        assert_eq!(config::MirrorConfig::default().context, 0);
    }
}
