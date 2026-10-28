//! The waker: turns wake candidates into at most one small nudge per debounce window and delivers
//! it through the configured plugin (spec §4, brakes from §12).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::config::{expand_home, read_secret_line, WakerConfig, WakerKind};
use crate::mirror::{now_ms, Mirror, WakeCandidate};
use crate::redact;

/// Why a delivery failed, and whether trying again makes sense. A timeout is not retried: the
/// agent process may have done the work already (Buzz #2175, L20) — the next event re-arms.
#[derive(Debug)]
pub struct DeliverError {
    pub message: String,
    pub retry: bool,
}

impl std::fmt::Display for DeliverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DeliverError {}

/// Marker inside an `anyhow::Error` for failures that must not be retried.
#[derive(Debug)]
struct NoRetry(String);

impl std::fmt::Display for NoRetry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NoRetry {}

fn no_retry(message: String) -> anyhow::Error {
    anyhow::Error::new(NoRetry(message))
}

/// How the nudge leaves the sidecar.
#[derive(Clone)]
pub enum Plugin {
    Webhook {
        url_file: Option<PathBuf>,
        auth_file: Option<PathBuf>,
        url: Option<String>,
    },
    Command {
        /// Plain argv (stateless agents, tests). Ignored when `templates` is set.
        argv: Vec<String>,
        cwd: Option<PathBuf>,
        timeout: Duration,
        env_allow: Vec<String>,
        /// Session-aware launch (spec §13.2): `new` / `resume` argv with `{session}`.
        templates: Option<crate::session::Templates>,
        resume_failure_regex: String,
        context_full_regex: String,
    },
    File {
        path: PathBuf,
    },
    /// Tests and `--waker none`: nudges go nowhere (or into a channel).
    Channel(Option<mpsc::UnboundedSender<Value>>),
}

impl Plugin {
    pub fn from_config(cfg: &WakerConfig, default_file: PathBuf) -> Result<Self> {
        Ok(match cfg.kind {
            WakerKind::Webhook => {
                if cfg.url_file.is_none() && cfg.url.is_none() {
                    bail!("webhook waker needs url_file (or url)");
                }
                Self::Webhook {
                    url_file: cfg.url_file.as_deref().map(expand_home),
                    auth_file: cfg.auth_file.as_deref().map(expand_home),
                    url: cfg.url.clone(),
                }
            }
            WakerKind::Command => {
                let templates = crate::session::templates(cfg);
                let argv = cfg
                    .command
                    .clone()
                    .or_else(|| templates.as_ref().map(|t| t.new.clone()))
                    .unwrap_or_default();
                if argv.is_empty() {
                    bail!("command waker needs a command (or --agent claude|codex|cursor|hermes)");
                }
                Self::Command {
                    argv,
                    cwd: cfg.cwd.as_deref().map(expand_home),
                    timeout: Duration::from_secs(cfg.timeout_s.max(1)),
                    env_allow: cfg
                        .env_allow
                        .iter()
                        .filter(|k| !k.starts_with("ARBOS"))
                        .cloned()
                        .collect(),
                    templates: if cfg.agent.is_some() || cfg.command_new.is_some() {
                        templates
                    } else {
                        None
                    },
                    resume_failure_regex: cfg.resume_failure_regex.clone(),
                    context_full_regex: cfg.context_full_regex.clone(),
                }
            }
            WakerKind::File => Self::File {
                path: cfg.path.as_deref().map(expand_home).unwrap_or(default_file),
            },
            WakerKind::None => Self::Channel(None),
        })
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Webhook { .. } => "webhook",
            Self::Command { .. } => "command",
            Self::File { .. } => "file",
            Self::Channel(_) => "none",
        }
    }

    /// Deliver one nudge. `Ok(status)` = delivered; `Err` = not delivered (with a retry flag).
    pub async fn deliver(&self, nudge: &Value) -> std::result::Result<String, DeliverError> {
        match self.deliver_inner(nudge).await {
            Ok(s) => Ok(s),
            Err(e) => match e.downcast_ref::<NoRetry>() {
                Some(n) => Err(DeliverError {
                    message: n.0.clone(),
                    retry: false,
                }),
                None => Err(DeliverError {
                    message: format!("{e:#}"),
                    retry: true,
                }),
            },
        }
    }

    async fn deliver_inner(&self, nudge: &Value) -> Result<String> {
        match self {
            Self::Webhook {
                url_file,
                auth_file,
                url,
            } => {
                let url = match (url_file, url) {
                    (Some(f), _) => read_secret_line(f)?,
                    (None, Some(u)) => u.clone(),
                    (None, None) => bail!("no webhook url"),
                };
                redact::protect(&url);
                let auth = match auth_file {
                    Some(f) => Some(read_secret_line(f)?),
                    None => None,
                };
                if !(url.starts_with("https://")
                    || url.starts_with("http://127.0.0.1")
                    || url.starts_with("http://localhost"))
                {
                    bail!("webhook url must be https:// (http only for localhost)");
                }
                let body = serde_json::to_vec(nudge)?;
                let client = reqwest::Client::builder()
                    .timeout(Duration::from_secs(5))
                    .build()?;
                let mut req = client
                    .post(&url)
                    .header("Content-Type", "application/json")
                    .header("X-Arbos-Agent", nudge["agent"]["id"].as_str().unwrap_or(""));
                if let Some(a) = &auth {
                    redact::protect(a);
                    req = req
                        .header("Authorization", a)
                        .header("X-Arbos-Signature", format!("sha256={}", sign(&body, a)));
                }
                let resp = req.body(body).send().await.context("webhook")?;
                let status = resp.status();
                if status.is_success() {
                    Ok(status.as_u16().to_string())
                } else if status.is_client_error() && status.as_u16() != 429 {
                    // A 4xx other than 429 is deterministic (bad URL, bad auth): retrying is spend
                    // with no hope (Buzz #2422, L22).
                    Err(no_retry(format!("webhook answered {status}")))
                } else {
                    Err(anyhow!("webhook answered {status}"))
                }
            }
            Self::Command {
                argv,
                cwd,
                timeout,
                env_allow,
                ..
            } => {
                let out =
                    run_command(argv, cwd.as_deref(), *timeout, env_allow, nudge, None).await?;
                if out.status_ok {
                    Ok("exit 0".into())
                } else {
                    Err(anyhow!("command exited with {}", out.status))
                }
            }
            Self::File { path } => {
                if let Some(d) = path.parent() {
                    std::fs::create_dir_all(d)?;
                }
                let mut f = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)?;
                use std::io::Write;
                f.write_all(serde_json::to_string(nudge)?.as_bytes())?;
                f.write_all(b"\n")?;
                Ok("appended".into())
            }
            Self::Channel(tx) => {
                if let Some(tx) = tx {
                    tx.send(nudge.clone())
                        .map_err(|_| no_retry("channel closed".into()))?;
                }
                Ok("noop".into())
            }
        }
    }
}

/// What a command waker run produced.
pub struct CommandOutput {
    pub status_ok: bool,
    pub status: String,
    pub stdout: String,
    pub stderr: String,
}

/// Run one agent command: `{prompt}`/`{json}`/`{chat}`/`{session}` filled in, the nudge JSON on
/// stdin, an allow-listed env (never `ARBOS_*`), its own process group, killed at `timeout`
/// (not retried — Buzz L20).
pub async fn run_command(
    argv: &[String],
    cwd: Option<&std::path::Path>,
    timeout: Duration,
    env_allow: &[String],
    nudge: &Value,
    session: Option<&str>,
) -> std::result::Result<CommandOutput, anyhow::Error> {
    let json = serde_json::to_string(nudge)?;
    let prompt = nudge["prompt_override"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| prompt_text(nudge));
    let chat = nudge["chats"][0]["id"].as_str().unwrap_or("").to_owned();
    let session = session.unwrap_or("");
    let fill = |s: &str| {
        s.replace("{json}", &json)
            .replace("{prompt}", &prompt)
            .replace("{chat}", &chat)
            .replace("{session}", session)
    };
    let mut cmd = tokio::process::Command::new(fill(&argv[0]));
    for a in &argv[1..] {
        cmd.arg(fill(a));
    }
    cmd.env_clear();
    for k in env_allow {
        if k.starts_with("ARBOS") {
            continue;
        }
        if let Ok(v) = std::env::var(k) {
            cmd.env(k, v);
        }
    }
    cmd.env("ARBOS_SIDECAR_WAKE", "1");
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    let mut child = cmd.spawn().context("starting the waker command")?;
    let pid = child.id().map(|p| p as i32);
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(json.as_bytes()).await;
        let _ = stdin.shutdown().await;
    }
    // Head and tail of each stream: the head carries `thread.started` (codex's session id), the
    // tail the agent's last words; the middle is the agent's business.
    let tail = |b: &[u8]| {
        let s = String::from_utf8_lossy(b);
        let s = s.trim();
        let chars: Vec<char> = s.chars().collect();
        if chars.len() <= 8192 {
            return s.to_owned();
        }
        let head: String = chars[..4096].iter().collect();
        let end: String = chars[chars.len() - 4096..].iter().collect();
        format!("{head}\n…\n{end}")
    };
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(out)) => {
            let stdout = tail(&out.stdout);
            let stderr = tail(&out.stderr);
            if !stdout.is_empty() {
                tracing::info!(waker = "command", stdout = %redact::redact(&stdout));
            }
            if !stderr.is_empty() {
                tracing::info!(waker = "command", stderr = %redact::redact(&stderr));
            }
            Ok(CommandOutput {
                status_ok: out.status.success(),
                status: out.status.to_string(),
                stdout,
                stderr,
            })
        }
        Ok(Err(e)) => Err(anyhow!("command failed: {e}")),
        Err(_) => {
            if let Some(pid) = pid {
                let _ = nix::sys::signal::killpg(
                    nix::unistd::Pid::from_raw(pid),
                    nix::sys::signal::Signal::SIGKILL,
                );
            }
            Err(no_retry(format!(
                "command timed out after {} s (killed with its process group; not retried — the agent may have answered already)",
                timeout.as_secs()
            )))
        }
    }
}

/// One wake through the agent's persistent session: resume first; a resume that cannot find
/// the session falls back to a new one (once) with the status line "starting a fresh session";
/// a session at its turn cap or asked to rotate starts new with the chats' briefs as handoff.
#[allow(clippy::too_many_arguments)]
async fn run_with_session(
    store: &crate::session::Store,
    t: &crate::session::Templates,
    cwd: Option<&std::path::Path>,
    timeout: Duration,
    env_allow: &[String],
    resume_failure_regex: &str,
    context_full_regex: &str,
    nudge: &Value,
    chats: &[String],
    side: Option<&mpsc::UnboundedSender<Side>>,
) -> std::result::Result<String, DeliverError> {
    use crate::session::{
        codex_thread_id, looks_like_context_full, looks_like_resume_failure, Attempt,
    };
    let log = |v: Value| {
        if let Some(tx) = side {
            let _ = tx.send(Side::Log(v));
        }
    };
    let agent_kind = t.new.first().cloned().unwrap_or_default();
    let cfg_like = WakerConfig {
        max_turns_per_session: store_cap(store),
        ..Default::default()
    };
    let (mut s, mut attempt) = store.plan(&cfg_like, &agent_kind);
    let mut fell_back = false;
    loop {
        let (argv, handoff) = match &attempt {
            Attempt::Resume => (t.resume.clone(), false),
            Attempt::New { handoff } => (t.new.clone(), *handoff),
        };
        if matches!(attempt, Attempt::New { .. }) {
            // Codex names its own thread; everyone else gets a sidecar-minted id.
            let id = if t.capture_codex_thread {
                String::new()
            } else {
                crate::session::new_id()
            };
            s = store.begin_new(&s, id).map_err(|e| DeliverError {
                message: format!("{e:#}"),
                retry: false,
            })?;
            log(
                json!({ "kind": "session.new", "session": s.id, "generation": s.generation,
                "handoff": handoff, "handoff_from": s.handoff_from }),
            );
        } else {
            log(
                json!({ "kind": "session.resume", "session": s.id, "generation": s.generation, "turns": s.turns }),
            );
        }
        let mut n = nudge.clone();
        if handoff {
            let prefix = store.handoff_prompt(chats);
            n["prompt_override"] = json!(format!("{prefix}{}", prompt_text(nudge)));
        }
        let started = std::time::Instant::now();
        let out = run_command(&argv, cwd, timeout, env_allow, &n, Some(&s.id)).await;
        match out {
            Ok(o) => {
                if t.capture_codex_thread && s.id.is_empty() {
                    if let Some(id) = codex_thread_id(&o.stdout) {
                        s.id = id;
                    }
                }
                let text = format!("{}\n{}", o.stdout, o.stderr);
                let resume_failed = attempt == Attempt::Resume
                    && (!o.status_ok || started.elapsed() < Duration::from_secs(15))
                    && looks_like_resume_failure(&text, resume_failure_regex);
                if resume_failed && !fell_back {
                    fell_back = true;
                    log(
                        json!({ "kind": "session.resume_failed", "session": s.id, "status": o.status }),
                    );
                    if let Some(tx) = side {
                        let _ = tx.send(Side::Status {
                            chats: chats.to_vec(),
                            text: "starting a fresh session".into(),
                        });
                    }
                    attempt = Attempt::New { handoff: true };
                    continue;
                }
                let context_full = looks_like_context_full(&text, context_full_regex);
                let _ = store.after_turn(&mut s, context_full);
                if context_full {
                    log(
                        json!({ "kind": "session.rotate", "session": s.id, "reason": "context_full" }),
                    );
                }
                if o.status_ok {
                    return Ok("exit 0".into());
                }
                return Err(DeliverError {
                    message: format!("command exited with {}", o.status),
                    retry: true,
                });
            }
            Err(e) => {
                return Err(match e.downcast_ref::<NoRetry>() {
                    Some(n) => DeliverError {
                        message: n.0.clone(),
                        retry: false,
                    },
                    None => DeliverError {
                        message: format!("{e:#}"),
                        retry: true,
                    },
                });
            }
        }
    }
}

/// The turn cap the store was built with (kept beside the record so `plan` sees one value).
fn store_cap(store: &crate::session::Store) -> u32 {
    store.max_turns()
}

/// `X-Arbos-Signature` value: hex HMAC-SHA256 of the body keyed with the Authorization value —
/// the same contract as the relay's Wake URL (#2185).
pub fn sign(body: &[u8], key: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("hmac key");
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

/// One-line text form for `{prompt}` wakers.
pub fn prompt_text(nudge: &Value) -> String {
    let mut parts = Vec::new();
    for c in nudge["chats"].as_array().into_iter().flatten() {
        let n = c["pending"].as_i64().unwrap_or(1);
        let title = c["title"].as_str().unwrap_or("");
        let kind = c["kind"].as_str().unwrap_or("chat");
        let from = c["last_from"]["name"].as_str().unwrap_or("");
        let what = if c["reason"] == "member.added" {
            format!("you were added to \"{title}\" ({kind})")
        } else {
            let label = if title.is_empty() {
                format!("a {kind}")
            } else {
                format!("\"{title}\" ({kind})")
            };
            format!(
                "{n} new message{} for you in {label}{}",
                if n == 1 { "" } else { "s" },
                if from.is_empty() {
                    String::new()
                } else {
                    format!(" — last from {from}")
                }
            )
        };
        parts.push(format!("{what} [chat {}]", c["id"].as_str().unwrap_or("")));
    }
    format!(
        "Arbos: {}. First, within seconds, call arbos_ack (or `arbos-sidecar ack <chat> <message> <one line>`) naming your first step. Then read with arbos_pending / arbos_read_chat (or `arbos-sidecar pending`), do the work, and answer once with arbos_reply (or `arbos-sidecar reply <chat> <message> <text>`) — it appears under your ack in the same bubble. Never a second ack. Reply only where you are addressed; post nothing if you decide not to answer.",
        parts.join("; ")
    )
}

#[derive(Debug, Clone, Default)]
struct PendingChat {
    title: String,
    kind: String,
    count: i64,
    last_from: Value,
    last_at: i64,
    events: Vec<i64>,
    reason: &'static str,
}

/// Why a candidate was not queued for a nudge.
#[derive(Debug, PartialEq)]
pub enum Skip {
    Stale,
    AgentLoop,
    AgentCooldown,
}

/// What the engine tells the daemon (typing, status line, log lines) — the engine holds no key.
#[derive(Debug, Clone)]
pub enum Side {
    /// A nudge just went out for these chats (`(chat id, newest event ts)`): typing on, log the
    /// wake, remember the timing for the ack/reply latencies.
    Nudged {
        chats: Vec<(String, i64)>,
        events: Vec<i64>,
        session: String,
        at: i64,
    },
    /// Put a working line under the composer of these chats ("starting a fresh session").
    Status { chats: Vec<String>, text: String },
    /// One `session.jsonl` entry.
    Log(Value),
}

/// What the daemon tells the engine.
pub enum Signal {
    /// A row (or membership change) that asks the agent to act (boxed: the unit variant is
    /// 0 B, the candidate ≈ 200 B).
    Event(Box<WakeCandidate>),
    /// The agent signalled "done": it posted, replied, reacted or marked a chat answered.
    Done,
}

const RETRY_DELAYS: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(5)];
const RETRY_LAST: Duration = Duration::from_secs(30);
const BREAKER_WINDOW: Duration = Duration::from_secs(60);
const BREAKER_OPEN: Duration = Duration::from_secs(300);
const BREAKER_THRESHOLD: usize = 3;
const AGENT_COOLDOWN: Duration = Duration::from_secs(600);
const SLOW_INTERVAL: Duration = Duration::from_secs(300);

/// Result of one delivery task: the nudge it carried, the outcome, and the attempt count.
type DeliveryOutcome = (Value, std::result::Result<String, DeliverError>, u32);

/// The nudge scheduler (spec §4, Jacob 16:32 UTC "sub-second first response, no flurry"):
///
/// - **Leading edge.** The first event with nothing in flight and no turn open fires at once.
/// - **Mid-turn.** While a nudge is being delivered, or the agent is in the turn it started
///   (no `Done` yet, less than `max_wait` since the nudge), new events only mark `dirty`.
/// - **Trailing.** When the agent says `Done` — or `max_wait` (5 s) passes without it — one
///   trailing nudge goes out if `dirty`, never sooner than `floor` (2 s) after the last one.
///
/// A burst of N messages therefore costs at most two wakes; `arbos_pending` carries the batch.
pub struct Engine {
    cfg: WakerConfig,
    plugin: Plugin,
    mirror: Mirror,
    agent: Value,
    pending: BTreeMap<String, PendingChat>,
    /// Events arrived since the last nudge went out.
    dirty: bool,
    last_sent: Option<Instant>,
    /// The agent finished the turn the last nudge started.
    done: bool,
    delivery: Option<tokio::task::JoinHandle<DeliveryOutcome>>,
    /// When to look again (floor, max wait, slow interval or breaker).
    deadline: Option<Instant>,
    sent: VecDeque<Instant>,
    failures: VecDeque<Instant>,
    open_until: Option<Instant>,
    half_open: bool,
    agent_sender_last: HashMap<String, Instant>,
    side: Option<mpsc::UnboundedSender<Side>>,
    sessions: Option<crate::session::Store>,
}

impl Engine {
    pub fn with_side(mut self, side: mpsc::UnboundedSender<Side>) -> Self {
        self.side = Some(side);
        self
    }

    pub fn with_sessions(mut self, store: crate::session::Store) -> Self {
        self.sessions = Some(store);
        self
    }

    fn side(&self, s: Side) {
        if let Some(tx) = &self.side {
            let _ = tx.send(s);
        }
    }

    pub fn new(cfg: WakerConfig, plugin: Plugin, mirror: Mirror, agent: Value) -> Self {
        Self {
            cfg,
            plugin,
            mirror,
            agent,
            pending: BTreeMap::new(),
            dirty: false,
            last_sent: None,
            done: true,
            delivery: None,
            deadline: None,
            sent: VecDeque::new(),
            failures: VecDeque::new(),
            open_until: None,
            half_open: false,
            agent_sender_last: HashMap::new(),
            side: None,
            sessions: None,
        }
    }

    fn floor(&self) -> Duration {
        Duration::from_secs(self.cfg.floor_s)
    }

    fn max_wait(&self) -> Duration {
        Duration::from_secs(self.cfg.debounce_s.max(self.cfg.floor_s))
    }

    /// Queue a candidate for the next nudge, or say why not.
    pub fn offer(&mut self, c: WakeCandidate) -> std::result::Result<(), Skip> {
        let now = Instant::now();
        let age_ms = now_ms() - c.ts;
        if age_ms > (self.cfg.stale_wake_s as i64) * 1000 {
            return Err(Skip::Stale);
        }
        if c.from_agent {
            // Belt for agent↔agent ping-pong (Buzz L10): the relay already stops re-forwarding
            // (one-hop rule); here an agent-sent wake nudges at most once per chat per 10 min
            // and never when the last rows are agents talking to agents with no human between.
            let rows = self.mirror.rows(&c.chat.id);
            let tail: Vec<&Value> = rows.iter().rev().take(3).collect();
            if tail.len() == 3 && tail.iter().all(|r| r["from"]["kind"] == "agent") {
                return Err(Skip::AgentLoop);
            }
            if let Some(last) = self.agent_sender_last.get(&c.chat.id) {
                if now.duration_since(*last) < AGENT_COOLDOWN {
                    return Err(Skip::AgentCooldown);
                }
            }
            self.agent_sender_last.insert(c.chat.id.clone(), now);
        }
        let p = self.pending.entry(c.chat.id.clone()).or_default();
        if !c.chat.title.is_empty() {
            p.title = c.chat.title.clone();
        }
        if !c.chat.kind.is_empty() {
            p.kind = c.chat.kind.clone();
        }
        p.count += 1;
        p.last_from = c.from.clone();
        p.last_at = c.ts;
        p.events.push(c.event_id);
        p.reason = c.reason;
        self.dirty = true;
        Ok(())
    }

    fn build_nudge(pending: &BTreeMap<String, PendingChat>, agent: &Value) -> Value {
        let mut chats: Vec<(&String, &PendingChat)> = pending.iter().collect();
        chats.sort_by_key(|a| std::cmp::Reverse(a.1.last_at));
        let events: Vec<i64> = chats.iter().flat_map(|(_, p)| p.events.clone()).collect();
        let first = chats.first().map(|(id, _)| id.as_str()).unwrap_or("");
        json!({
            "type": "wake",
            "v": 1,
            "at": now_ms(),
            "agent": agent,
            "chats": chats.iter().map(|(id, p)| json!({
                "id": id, "kind": p.kind, "title": p.title, "pending": p.count,
                "last_from": p.last_from, "last_at": p.last_at, "reason": p.reason,
            })).collect::<Vec<_>>(),
            "events": events,
            "read": format!("arbos-sidecar read {first}   |   MCP: arbos_read_chat {{\"chat\":\"{first}\"}}"),
        })
    }

    fn sent_last_hour(&mut self, now: Instant) -> usize {
        while let Some(t) = self.sent.front() {
            if now.duration_since(*t) > Duration::from_secs(3600) {
                self.sent.pop_front();
            } else {
                break;
            }
        }
        self.sent.len()
    }

    fn breaker_allows(&mut self, now: Instant) -> bool {
        if let Some(until) = self.open_until {
            if now < until {
                return false;
            }
            // Half-open: one probe.
            self.open_until = None;
            self.half_open = true;
        }
        true
    }

    fn record_failure(&mut self, now: Instant) {
        if self.half_open {
            self.half_open = false;
            self.open_until = Some(now + BREAKER_OPEN);
            self.failures.clear();
            return;
        }
        self.failures.push_back(now);
        while let Some(t) = self.failures.front() {
            if now.duration_since(*t) > BREAKER_WINDOW {
                self.failures.pop_front();
            } else {
                break;
            }
        }
        if self.failures.len() >= BREAKER_THRESHOLD {
            self.open_until = Some(now + BREAKER_OPEN);
            self.failures.clear();
        }
    }

    fn record(&self, nudge: &Value, ok: bool, status: &str, error: Option<&str>, attempt: u32) {
        let rec = json!({
            "at": now_ms(),
            "chats": nudge["chats"].as_array().map(|a| a.iter().map(|c| c["id"].clone()).collect::<Vec<_>>()),
            "events": nudge["events"],
            "waker": self.plugin.name(),
            "ok": ok,
            "status": status,
            "error": error.map(redact::redact),
            "attempt": attempt,
        });
        if let Err(e) = self.mirror.append_wake(&rec) {
            tracing::warn!(error = %e, "wakes.jsonl not written");
        }
    }

    fn arm(&mut self, at: Instant) {
        self.deadline = Some(match self.deadline {
            Some(d) if d <= at => d,
            _ => at,
        });
    }

    /// Fire now if the rules allow; otherwise arm the deadline for the earliest moment they may.
    fn try_fire(&mut self) {
        self.deadline = None;
        if self.pending.is_empty() || self.delivery.is_some() {
            return;
        }
        let now = Instant::now();
        if let Some(last) = self.last_sent {
            let since = now.duration_since(last);
            // Floor between nudges.
            if since < self.floor() {
                self.arm(last + self.floor());
                return;
            }
            // The agent is still in the turn the last nudge started.
            if !self.done && since < self.max_wait() {
                self.arm(last + self.max_wait());
                return;
            }
            // Rate cap (Buzz L16): past the hourly cap, one nudge per 5 min.
            let n = self.sent_last_hour(now);
            if n as u32 >= self.cfg.max_nudges_per_hour && since < SLOW_INTERVAL {
                tracing::warn!(sent_last_hour = n, "nudge rate cap reached — slowing down");
                self.arm(last + SLOW_INTERVAL);
                return;
            }
        }
        if !self.breaker_allows(now) {
            let nudge = Self::build_nudge(&self.pending, &self.agent);
            self.record(&nudge, false, "skipped", Some("circuit_open"), 0);
            if let Some(until) = self.open_until {
                self.arm(until);
            }
            return;
        }
        let batch = std::mem::take(&mut self.pending);
        let mut nudge = Self::build_nudge(&batch, &self.agent);
        self.dirty = false;
        self.done = false;
        self.last_sent = Some(now);
        // The stable session field for receivers that keep their own thread (webhook / file).
        let generation = self
            .sessions
            .as_ref()
            .map(|s| s.load().generation)
            .unwrap_or(0);
        let agent_id = self.agent["id"].as_str().unwrap_or("").to_owned();
        let session_label = crate::session::external_session(&agent_id, generation);
        nudge["session"] = json!(session_label);
        // Typing on, log the wake, start the latency clock — before the agent is even started.
        self.side(Side::Nudged {
            chats: batch
                .iter()
                .map(|(id, p)| (id.clone(), p.last_at))
                .collect(),
            events: batch.values().flat_map(|p| p.events.clone()).collect(),
            session: session_label.clone(),
            at: now_ms(),
        });
        let plugin = self.plugin.clone();
        let sessions = self.sessions.clone();
        let side = self.side.clone();
        let chats: Vec<String> = batch.keys().cloned().collect();
        self.delivery = Some(tokio::spawn(async move {
            // Session-aware command wakers resume the agent's own session (spec §13.2).
            if let (
                Plugin::Command {
                    templates: Some(t),
                    cwd,
                    timeout,
                    env_allow,
                    resume_failure_regex,
                    context_full_regex,
                    ..
                },
                Some(store),
            ) = (&plugin, &sessions)
            {
                let r = run_with_session(
                    store,
                    t,
                    cwd.as_deref(),
                    *timeout,
                    env_allow,
                    resume_failure_regex,
                    context_full_regex,
                    &nudge,
                    &chats,
                    side.as_ref(),
                )
                .await;
                return (nudge, r, 1);
            }
            let mut attempt = 0u32;
            loop {
                attempt += 1;
                match plugin.deliver(&nudge).await {
                    Ok(status) => return (nudge, Ok(status), attempt),
                    Err(e) if !e.retry => return (nudge, Err(e), attempt),
                    Err(e) => {
                        tracing::warn!(attempt, error = %redact::redact(&e.message), "nudge not delivered");
                        if attempt <= RETRY_DELAYS.len() as u32 {
                            tokio::time::sleep(RETRY_DELAYS[attempt as usize - 1]).await;
                        } else if attempt == RETRY_DELAYS.len() as u32 + 1 {
                            tokio::time::sleep(RETRY_LAST).await;
                        } else {
                            return (nudge, Err(e), attempt);
                        }
                    }
                }
            }
        }));
        // Whatever happens, look again when the turn window closes.
        self.arm(now + self.max_wait());
    }

    fn on_delivered(&mut self, outcome: DeliveryOutcome) {
        let (nudge, result, attempt) = outcome;
        self.delivery = None;
        match result {
            Ok(status) => {
                self.record(&nudge, true, &status, None, attempt);
                self.sent.push_back(Instant::now());
                self.half_open = false;
                self.failures.clear();
                tracing::info!(attempt, status = %status, "nudge delivered");
                // A command waker's exit *is* the end of the agent's turn.
                if matches!(self.plugin, Plugin::Command { .. }) {
                    self.done = true;
                }
            }
            Err(e) => {
                self.record(&nudge, false, "failed", Some(&e.message), attempt);
                self.side(Side::Log(json!({
                    "kind": "error", "step": "waker", "error": e.message,
                    "chat": nudge["chats"][0]["id"],
                })));
                self.record_failure(Instant::now());
                // Put the batch back so the next event (or the next look) carries it.
                for c in nudge["chats"].as_array().into_iter().flatten() {
                    let id = c["id"].as_str().unwrap_or("").to_owned();
                    let p = self.pending.entry(id).or_default();
                    p.title = c["title"].as_str().unwrap_or("").to_owned();
                    p.kind = c["kind"].as_str().unwrap_or("").to_owned();
                    p.count += c["pending"].as_i64().unwrap_or(1);
                    p.last_from = c["last_from"].clone();
                    p.last_at = c["last_at"].as_i64().unwrap_or(0);
                    p.reason = if c["reason"] == "member.added" {
                        "member.added"
                    } else {
                        "message"
                    };
                }
                for e in nudge["events"].as_array().into_iter().flatten() {
                    if let (Some(id), Some((_, p))) = (e.as_i64(), self.pending.iter_mut().next()) {
                        p.events.push(id);
                    }
                }
                self.dirty = true;
                self.done = true;
            }
        }
        self.try_fire();
    }

    /// Drive the engine until the signal channel closes.
    pub async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Signal>) {
        loop {
            let deadline = self.deadline;
            tokio::select! {
                sig = rx.recv() => match sig {
                    Some(Signal::Event(c)) => {
                        match self.offer(*c) {
                            Ok(()) => self.try_fire(),
                            Err(skip) => tracing::debug!(?skip, "wake candidate skipped"),
                        }
                    }
                    Some(Signal::Done) => {
                        self.done = true;
                        self.try_fire();
                    }
                    None => {
                        if let Some(h) = self.delivery.take() {
                            if let Ok(o) = h.await { self.on_delivered(o); }
                        }
                        return;
                    }
                },
                outcome = async { match self.delivery.as_mut() { Some(h) => h.await, None => std::future::pending().await } } => {
                    match outcome {
                        Ok(o) => self.on_delivered(o),
                        Err(e) => {
                            tracing::warn!(error = %e, "delivery task failed");
                            self.delivery = None;
                            self.done = true;
                            self.try_fire();
                        }
                    }
                }
                _ = async { match deadline { Some(d) => tokio::time::sleep_until(d).await, None => std::future::pending().await } } => {
                    self.try_fire();
                }
            }
        }
    }

    /// `arbos-sidecar test-wake`: one nudge now, no debounce, no retry.
    pub async fn test_wake(&self) -> Result<String> {
        let nudge = json!({
            "type": "wake.test", "v": 1, "at": now_ms(), "agent": self.agent,
            "chats": [], "events": [],
            "read": "this is a test nudge from arbos-sidecar test-wake",
        });
        let r = self.plugin.deliver(&nudge).await;
        match &r {
            Ok(s) => self.record(&nudge, true, s, None, 1),
            Err(e) => self.record(&nudge, false, "failed", Some(&e.message), 1),
        }
        r.map_err(|e| anyhow!("{}", e.message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mirror::tests::{received, temp_mirror};
    use crate::mirror::ChatMeta;

    fn cand(chat: &str, event_id: i64, from_agent: bool) -> WakeCandidate {
        WakeCandidate {
            chat: ChatMeta {
                id: chat.into(),
                kind: "group".into(),
                title: "Launch".into(),
                ..Default::default()
            },
            event_id,
            ts: now_ms(),
            from: json!({ "kind": if from_agent { "agent" } else { "user" }, "name": "Jacob" }),
            from_agent,
            reason: "message",
        }
    }

    fn engine(cfg: WakerConfig) -> (tempfile::TempDir, Engine, mpsc::UnboundedReceiver<Value>) {
        let (dir, m) = temp_mirror();
        let (tx, rx) = mpsc::unbounded_channel();
        let e = Engine::new(
            cfg,
            Plugin::Channel(Some(tx)),
            m,
            json!({ "id": "a1", "name": "Bot", "handle": "bot" }),
        );
        (dir, e, rx)
    }

    fn drain(out: &mut mpsc::UnboundedReceiver<Value>) -> Vec<Value> {
        let mut v = Vec::new();
        while let Ok(n) = out.try_recv() {
            v.push(n);
        }
        v
    }

    /// Jacob 16:32: an isolated mention reaches the webhook in well under a second (real clock,
    /// real HTTP, in-process catcher).
    #[tokio::test]
    async fn isolated_mention_reaches_webhook_under_one_second() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (hit_tx, mut hit_rx) = mpsc::unbounded_channel::<std::time::Instant>();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let at = std::time::Instant::now();
                let mut buf = vec![0u8; 65536];
                let _ = sock.read(&mut buf).await;
                let _ = sock
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .await;
                let _ = hit_tx.send(at);
            }
        });
        let (_d, m) = temp_mirror();
        let e = Engine::new(
            WakerConfig::default(),
            Plugin::Webhook {
                url_file: None,
                auth_file: None,
                url: Some(format!("http://127.0.0.1:{port}/hook")),
            },
            m,
            json!({ "id": "a1" }),
        );
        let (tx, rx) = mpsc::unbounded_channel();
        let h = tokio::spawn(e.run(rx));
        let t0 = std::time::Instant::now();
        tx.send(Signal::Event(Box::new(cand("c1", 1, false))))
            .unwrap();
        let at = tokio::time::timeout(Duration::from_secs(1), hit_rx.recv())
            .await
            .expect("webhook hit within 1 s")
            .unwrap();
        let latency = at.duration_since(t0);
        eprintln!("event → webhook latency: {latency:?}");
        assert!(latency < Duration::from_millis(1000), "{latency:?}");
        drop(tx);
        h.await.unwrap();
    }

    /// Jacob 16:32: a 20-message burst gives exactly two nudges — the first at once, one
    /// trailing nudge carrying the other 19 once the turn window closes.
    #[tokio::test(start_paused = true)]
    async fn burst_of_twenty_gives_exactly_two_nudges() {
        let (_d, e, mut out) = engine(WakerConfig::default());
        let (tx, rx) = mpsc::unbounded_channel();
        let h = tokio::spawn(e.run(rx));
        for i in 0..20 {
            tx.send(Signal::Event(Box::new(cand("c1", i, false))))
                .unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
        let first = drain(&mut out);
        assert_eq!(first.len(), 1, "leading nudge only");
        assert_eq!(first[0]["chats"][0]["pending"], 1);
        // No done signal: the trailing nudge waits for the 5 s window, not a second longer.
        tokio::time::sleep(Duration::from_secs(5)).await;
        let trailing = drain(&mut out);
        assert_eq!(trailing.len(), 1, "one trailing nudge");
        assert_eq!(trailing[0]["chats"][0]["pending"], 19);
        assert_eq!(trailing[0]["events"].as_array().unwrap().len(), 19);
        tokio::time::sleep(Duration::from_secs(30)).await;
        assert!(drain(&mut out).is_empty(), "nothing else");
        drop(tx);
        h.await.unwrap();
    }

    /// Jacob 16:32: messages that arrive while the agent is mid-turn only mark dirty; when the
    /// agent says done (after the 2 s floor) exactly one trailing nudge follows.
    #[tokio::test(start_paused = true)]
    async fn burst_during_turn_gives_one_trailing_nudge_after_done() {
        let (_d, e, mut out) = engine(WakerConfig::default());
        let (tx, rx) = mpsc::unbounded_channel();
        let h = tokio::spawn(e.run(rx));
        tx.send(Signal::Event(Box::new(cand("c1", 1, false))))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(drain(&mut out).len(), 1, "leading");
        // Mid-turn burst across two chats.
        for i in 2..12 {
            tx.send(Signal::Event(Box::new(cand(
                if i % 2 == 0 { "c1" } else { "c2" },
                i,
                false,
            ))))
            .unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(drain(&mut out).is_empty(), "no per-message nudges mid-turn");
        // The agent finishes at ~3 s (past the 2 s floor): one trailing nudge, at once.
        tokio::time::sleep(Duration::from_millis(1900)).await;
        tx.send(Signal::Done).unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        let trailing = drain(&mut out);
        assert_eq!(trailing.len(), 1);
        assert_eq!(trailing[0]["chats"].as_array().unwrap().len(), 2);
        assert_eq!(trailing[0]["events"].as_array().unwrap().len(), 10);
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert!(drain(&mut out).is_empty());
        drop(tx);
        h.await.unwrap();
    }

    /// Done before the floor: the trailing nudge waits until 2 s after the last one.
    #[tokio::test(start_paused = true)]
    async fn done_inside_floor_waits_for_the_floor() {
        let (_d, e, mut out) = engine(WakerConfig::default());
        let (tx, rx) = mpsc::unbounded_channel();
        let h = tokio::spawn(e.run(rx));
        tx.send(Signal::Event(Box::new(cand("c1", 1, false))))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(drain(&mut out).len(), 1);
        tx.send(Signal::Event(Box::new(cand("c1", 2, false))))
            .unwrap();
        tx.send(Signal::Done).unwrap();
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(drain(&mut out).is_empty(), "inside the floor");
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(drain(&mut out).len(), 1, "at the floor");
        drop(tx);
        h.await.unwrap();
    }

    /// Buzz L4/L5: an offline catch-up of 1 000 events still costs at most two nudges.
    #[tokio::test(start_paused = true)]
    async fn catch_up_after_offline_is_one_nudge() {
        let (_d, e, mut out) = engine(WakerConfig::default());
        let (tx, rx) = mpsc::unbounded_channel();
        let h = tokio::spawn(e.run(rx));
        for i in 0..1000 {
            tx.send(Signal::Event(Box::new(cand(
                if i % 2 == 0 { "c1" } else { "c2" },
                i,
                false,
            ))))
            .unwrap();
        }
        tokio::time::sleep(Duration::from_secs(6)).await;
        let nudges = drain(&mut out);
        assert!(nudges.len() <= 2, "{}", nudges.len());
        let total: usize = nudges
            .iter()
            .map(|n| n["events"].as_array().unwrap().len())
            .sum();
        assert_eq!(total, 1000);
        assert!(nudges[0]["read"]
            .as_str()
            .unwrap()
            .contains("arbos_read_chat"));
        drop(tx);
        h.await.unwrap();
    }

    /// Buzz L4: a stale row never nudges.
    #[test]
    fn stale_candidates_are_skipped() {
        let (_d, mut e, _rx) = engine(WakerConfig::default());
        let mut c = cand("c1", 1, false);
        c.ts = now_ms() - 7 * 3600 * 1000;
        assert_eq!(e.offer(c), Err(Skip::Stale));
        assert!(e.pending.is_empty());
    }

    /// Buzz L8: nothing arrives → nothing is sent, however long we wait.
    #[tokio::test(start_paused = true)]
    async fn idle_sidecar_sends_no_nudge() {
        let (_d, e, mut out) = engine(WakerConfig::default());
        let (tx, rx) = mpsc::unbounded_channel::<Signal>();
        let h = tokio::spawn(e.run(rx));
        tokio::time::sleep(Duration::from_secs(600)).await;
        assert!(out.try_recv().is_err());
        drop(tx);
        h.await.unwrap();
    }

    /// Buzz L10: agent-sent wakes nudge once per chat per 10 min and never for an all-agent tail.
    #[tokio::test(start_paused = true)]
    async fn agent_to_agent_pingpong_stops_after_one_hop() {
        let (_d, mut e, _rx) = engine(WakerConfig::default());
        assert!(e.offer(cand("c1", 1, true)).is_ok());
        assert_eq!(e.offer(cand("c1", 2, true)), Err(Skip::AgentCooldown));
        tokio::time::sleep(Duration::from_secs(601)).await;
        assert!(e.offer(cand("c1", 3, true)).is_ok());
        for i in 0..3 {
            let mut ev = received(10 + i, "c2", &format!("m{i}"), "ping", true);
            ev["message"]["from"] = json!({ "kind": "agent", "id": "other", "name": "Other bot" });
            e.mirror.apply_event(&ev).unwrap();
        }
        assert_eq!(e.offer(cand("c2", 20, true)), Err(Skip::AgentLoop));
        e.mirror
            .apply_event(&received(30, "c2", "m30", "hey bot", true))
            .unwrap();
        assert!(e.offer(cand("c2", 30, true)).is_ok());
    }

    /// Buzz L16: past the hourly cap, nudges coalesce to one per 5 min; nothing is lost.
    #[tokio::test(start_paused = true)]
    async fn nudge_rate_cap_coalesces() {
        let (_d, e, mut out) = engine(WakerConfig {
            max_nudges_per_hour: 2,
            ..Default::default()
        });
        let (tx, rx) = mpsc::unbounded_channel();
        let h = tokio::spawn(e.run(rx));
        for i in 0..2 {
            tx.send(Signal::Event(Box::new(cand("c1", i, false))))
                .unwrap();
            tx.send(Signal::Done).unwrap();
            tokio::time::sleep(Duration::from_secs(3)).await;
            assert_eq!(drain(&mut out).len(), 1);
        }
        tx.send(Signal::Event(Box::new(cand("c1", 10, false))))
            .unwrap();
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert!(drain(&mut out).is_empty(), "capped");
        tx.send(Signal::Event(Box::new(cand("c1", 11, false))))
            .unwrap();
        tokio::time::sleep(Duration::from_secs(300)).await;
        let n = drain(&mut out);
        assert_eq!(n.len(), 1);
        assert_eq!(n[0]["events"].as_array().unwrap().len(), 2);
        drop(tx);
        h.await.unwrap();
    }

    /// Buzz L7: three attempts (1 s, 5 s, 30 s) then the failure is recorded and the batch is
    /// kept for the next event.
    #[tokio::test(start_paused = true)]
    async fn waker_retries_three_times_then_rearms() {
        let (_d, m) = temp_mirror();
        let bad = Plugin::Webhook {
            url_file: None,
            auth_file: None,
            url: Some("http://127.0.0.1:9/never".into()),
        };
        let e = Engine::new(
            WakerConfig::default(),
            bad,
            m.clone(),
            json!({ "id": "a1" }),
        );
        let (tx, rx) = mpsc::unbounded_channel();
        let h = tokio::spawn(e.run(rx));
        tx.send(Signal::Event(Box::new(cand("c1", 1, false))))
            .unwrap();
        tokio::time::sleep(Duration::from_secs(60)).await;
        let last = m.last_wake().unwrap();
        assert_eq!(last["ok"], false);
        assert_eq!(last["attempt"], 4);
        assert_eq!(last["status"], "failed");
        drop(tx);
        h.await.unwrap();
    }

    /// Buzz L17: three failed nudges in a minute open the circuit for five minutes; one probe
    /// after that.
    #[tokio::test(start_paused = true)]
    async fn command_waker_circuit_breaker() {
        let (_d, m) = temp_mirror();
        let mut e = Engine::new(
            WakerConfig::default(),
            Plugin::Channel(None),
            m,
            json!({ "id": "a1" }),
        );
        let now = Instant::now();
        assert!(e.breaker_allows(now));
        for _ in 0..3 {
            e.record_failure(now);
        }
        assert!(!e.breaker_allows(now + Duration::from_secs(10)));
        assert!(e.breaker_allows(now + BREAKER_OPEN + Duration::from_secs(1)));
        assert!(e.half_open);
        e.record_failure(now + BREAKER_OPEN + Duration::from_secs(2));
        assert!(!e.breaker_allows(now + BREAKER_OPEN + Duration::from_secs(3)));
    }

    /// Buzz L6/L20: a hung command waker is killed after `timeout_s` with its process group; a
    /// timeout is never retried.
    #[tokio::test]
    async fn stuck_command_waker_is_killed_and_rearmed() {
        let p = Plugin::Command {
            argv: vec!["sh".into(), "-c".into(), "sleep 30; exit 0".into()],
            cwd: None,
            timeout: Duration::from_secs(1),
            env_allow: vec!["PATH".into()],
            templates: None,
            resume_failure_regex: String::new(),
            context_full_regex: String::new(),
        };
        let t = std::time::Instant::now();
        let r = p.deliver(&json!({ "chats": [], "agent": {} })).await;
        assert!(r.is_err());
        assert!(t.elapsed() < Duration::from_secs(5));
        let err = r.unwrap_err();
        assert!(err.message.contains("timed out"));
        assert!(!err.retry, "a timeout is never retried");
    }

    /// Buzz #8033 / #8586 (L19): a nudge that fails for good is recorded, and the rows stay
    /// visible in `pending` — nothing is silently discarded.
    #[tokio::test(start_paused = true)]
    async fn failed_nudge_keeps_pending_visible() {
        let (_d, m) = temp_mirror();
        m.apply_event(&received(1, "c1", "m1", "@bot hi", true))
            .unwrap();
        let (tx_dead, rx_dead) = mpsc::unbounded_channel::<Value>();
        drop(rx_dead);
        let e = Engine::new(
            WakerConfig::default(),
            Plugin::Channel(Some(tx_dead)),
            m.clone(),
            json!({ "id": "a1" }),
        );
        let (tx, rx) = mpsc::unbounded_channel();
        let h = tokio::spawn(e.run(rx));
        tx.send(Signal::Event(Box::new(cand("c1", 1, false))))
            .unwrap();
        tokio::time::sleep(Duration::from_secs(5)).await;
        let last = m.last_wake().unwrap();
        assert_eq!(last["ok"], false);
        assert_eq!(last["attempt"], 1, "a non-retryable failure is not retried");
        assert_eq!(m.pending(i64::MAX).len(), 1, "the row is still pending");
        drop(tx);
        h.await.unwrap();
    }

    /// Buzz #5327 (L25): event kinds the sidecar does not know never wake anyone.
    #[test]
    fn unknown_event_types_never_wake() {
        let (_d, m) = temp_mirror();
        for ty in ["typing", "presence.heartbeat", "reaction", "x.vendor.thing"] {
            let ev = json!({ "id": 5, "type": ty, "ts": now_ms(),
                "chat": { "id": "c1", "kind": "group", "title": "T" }, "wake": true });
            assert!(m.apply_event(&ev).unwrap().is_none(), "{ty}");
        }
        assert!(m.rows("c1").is_empty());
    }

    /// Buzz L13: the key is never on argv and never in the child's environment.
    #[tokio::test]
    async fn key_never_on_argv_or_in_child_env() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("env.txt");
        std::env::set_var("ARBOS_AGENT_KEY", format!("arbk_{}", "d".repeat(40)));
        std::env::set_var("ARBOS_SIDECAR_HOME", dir.path());
        let p = Plugin::Command {
            argv: vec![
                "sh".into(),
                "-c".into(),
                format!("env > {}; echo \"$@\" >> {}", out.display(), out.display()),
                "_".into(),
                "{prompt}".into(),
            ],
            cwd: None,
            timeout: Duration::from_secs(10),
            env_allow: vec!["PATH".into(), "ARBOS_AGENT_KEY".into()],
            templates: None,
            resume_failure_regex: String::new(),
            context_full_regex: String::new(),
        };
        p.deliver(&json!({ "chats": [{ "id": "c1", "title": "T", "kind": "group", "pending": 1 }], "agent": { "id": "a1" } }))
            .await
            .unwrap();
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(!text.contains("arbk_"), "{text}");
        assert!(!text.contains("ARBOS_AGENT_KEY"), "{text}");
        assert!(!text.contains("ARBOS_SIDECAR_HOME"), "{text}");
        assert!(text.contains("ARBOS_SIDECAR_WAKE=1"));
        assert!(text.contains("Arbos: 1 new message"));
        std::env::remove_var("ARBOS_AGENT_KEY");
        std::env::remove_var("ARBOS_SIDECAR_HOME");
    }

    /// Spec §13.2: the first wake creates a session, the next resumes it with the same id; a
    /// resume the agent cannot find falls back to a new session once (status line "starting a
    /// fresh session", handoff prompt), generation climbs, and every step is in the log.
    #[tokio::test]
    async fn sessions_resume_and_fall_back_once() {
        let (dir, m) = temp_mirror();
        let script = dir.path().join("agent.sh");
        let marks = dir.path().join("marks");
        std::fs::create_dir_all(&marks).unwrap();
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nmode=$1; sid=$2\n\
                 echo \"$mode $sid\" >> {marks}/calls\n\
                 if [ \"$mode\" = resume ] && [ ! -f {marks}/ok-$sid ]; then echo 'No conversation found' >&2; exit 1; fi\n\
                 touch {marks}/ok-$sid\n\
                 cat > {marks}/stdin-$mode\n\
                 echo fine\n",
                marks = marks.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let t = crate::session::Templates {
            new: vec![
                script.display().to_string(),
                "new".into(),
                "{session}".into(),
                "{prompt}".into(),
            ],
            resume: vec![
                script.display().to_string(),
                "resume".into(),
                "{session}".into(),
                "{prompt}".into(),
            ],
            capture_codex_thread: false,
        };
        let store = crate::session::Store::new(m.clone(), 50);
        let (side_tx, mut side_rx) = mpsc::unbounded_channel();
        let run = |n: Value| {
            let (store, t, side_tx) = (store.clone(), t.clone(), side_tx.clone());
            async move {
                run_with_session(
                    &store,
                    &t,
                    None,
                    Duration::from_secs(10),
                    &["PATH".to_string()],
                    "No conversation|not found",
                    "context window",
                    &n,
                    &["c1".to_string()],
                    Some(&side_tx),
                )
                .await
            }
        };
        let nudge = json!({ "chats": [{ "id": "c1", "title": "T", "kind": "group", "pending": 1 }], "agent": { "id": "a1" } });
        // 1. first wake → new session.
        run(nudge.clone()).await.unwrap();
        let s1 = store.load();
        assert_eq!(s1.generation, 1);
        assert_eq!(s1.turns, 1);
        assert!(!s1.id.is_empty());
        // 2. second wake → resume with the same id (the script has seen it, so it succeeds).
        run(nudge.clone()).await.unwrap();
        let s2 = store.load();
        assert_eq!(s2.id, s1.id);
        assert_eq!(s2.turns, 2);
        // 3. simulate a lost session: forget the agent-side mark → resume fails → new once.
        std::fs::remove_file(marks.join(format!("ok-{}", s2.id))).unwrap();
        run(nudge.clone()).await.unwrap();
        let s3 = store.load();
        assert_ne!(s3.id, s2.id);
        assert_eq!(s3.generation, 2);
        assert_eq!(s3.handoff_from.as_deref(), Some(s2.id.as_str()));
        let calls = std::fs::read_to_string(marks.join("calls")).unwrap();
        assert_eq!(calls.lines().count(), 4, "{calls}");
        let handoff = std::fs::read_to_string(marks.join("stdin-new")).unwrap();
        assert!(handoff.contains("\"chats\""));
        let mut kinds = Vec::new();
        let mut statuses = Vec::new();
        while let Ok(s) = side_rx.try_recv() {
            match s {
                Side::Log(v) => kinds.push(v["kind"].as_str().unwrap().to_owned()),
                Side::Status { text, .. } => statuses.push(text),
                Side::Nudged { .. } => {}
            }
        }
        assert_eq!(
            kinds,
            vec![
                "session.new",
                "session.resume",
                "session.resume",
                "session.resume_failed",
                "session.new"
            ]
        );
        assert_eq!(statuses, vec!["starting a fresh session"]);
    }

    /// The webhook signature matches the relay's Wake URL contract (#2185).
    #[test]
    fn signature_is_hmac_sha256_hex() {
        let s = sign(b"{}", "secret");
        assert_eq!(s.len(), 64);
        assert_eq!(s, sign(b"{}", "secret"));
        assert_ne!(s, sign(b"{}", "other"));
    }

    /// Buzz L11: chat text is data; "!shutdown" is mirrored, never obeyed.
    #[test]
    fn chat_text_is_never_a_command() {
        let (_d, m) = temp_mirror();
        let w = m
            .apply_event(&received(1, "c1", "m1", "!shutdown", true))
            .unwrap();
        assert!(w.is_some());
        assert_eq!(m.rows("c1")[0]["text"], "!shutdown");
    }
}
