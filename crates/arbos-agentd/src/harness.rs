//! The `HarnessAdapter` registry (design §4.4.2, D67): `PrimeAdapter` (default),
//! `PiAdapter` (fallback), `ClaudeCodeAdapter` (long coding tasks). Everything above
//! this module sees only `AgentEvent`s.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::{broadcast, Mutex};

use arbos_core::route::Harness;
use arbos_core::rpc::{AssistantEvent, Command as RpcCommand, Event, StreamingBehavior};

use crate::config::Config;
use crate::rpc_child::RpcChild;
use crate::store::Store;

/// Harness-neutral stream (design §4.4.2).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum AgentEvent {
    TextDelta(String),
    ReasoningDelta(String),
    ToolCall {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        id: String,
        name: String,
        output: String,
        is_error: bool,
    },
    Usage {
        input: u64,
        output: u64,
        cache_read: u64,
        cache_write: u64,
        cost_usd: Option<f64>,
    },
    InputRequested {
        question: String,
    },
    CompactionStarted,
    CompactionEnded {
        summary: Option<String>,
    },
    Done {
        aborted: bool,
        error: Option<String>,
    },
}

#[allow(dead_code)]
pub struct StartRequest {
    pub session_id: String,
    pub cwd: PathBuf,
    pub system_prompt: String,
    pub model: String,
    pub thinking: String,
}

/// The full adapter surface from the design; M0 drives start/resume/prompt/stream/read_history.
#[allow(dead_code)]
#[async_trait]
pub trait HarnessAdapter: Send + Sync {
    fn kind(&self) -> Harness;
    async fn start(&self, req: StartRequest) -> Result<()>;
    async fn resume(&self, session_id: &str) -> Result<()>;
    async fn prompt(&self, session_id: &str, text: &str) -> Result<()>;
    async fn follow_up(&self, session_id: &str, text: &str) -> Result<()>;
    async fn steer(&self, session_id: &str, text: &str) -> Result<()>;
    async fn cancel(&self, session_id: &str) -> Result<()>;
    fn stream(&self, session_id: &str) -> Option<broadcast::Receiver<AgentEvent>>;
    async fn read_history(
        &self,
        session_id: &str,
        cursor: Option<String>,
    ) -> Result<(Vec<Value>, Option<String>)>;
    async fn stop(&self, session_id: &str) -> Result<()>;
    /// Orderly shutdown of every open child (close stdin, wait, kill) before the box stops.
    async fn shutdown_all(&self);
    fn is_open(&self, session_id: &str) -> bool;
}

/// Translate one pi/Prime RPC event into zero or more `AgentEvent`s.
pub fn translate_rpc_event(e: &Event) -> Vec<AgentEvent> {
    match e {
        Event::MessageUpdate {
            assistant_message_event,
            usage,
        } => {
            let mut out = Vec::new();
            match assistant_message_event {
                AssistantEvent::TextDelta { delta, .. } => {
                    out.push(AgentEvent::TextDelta(delta.clone()))
                }
                AssistantEvent::ThinkingDelta { delta, .. } => {
                    out.push(AgentEvent::ReasoningDelta(delta.clone()))
                }
                AssistantEvent::Error { error, .. } => out.push(AgentEvent::Done {
                    aborted: false,
                    error: Some(
                        error
                            .as_ref()
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "provider error".into()),
                    ),
                }),
                AssistantEvent::Start
                | AssistantEvent::TextStart { .. }
                | AssistantEvent::TextEnd { .. }
                | AssistantEvent::ThinkingStart { .. }
                | AssistantEvent::ThinkingEnd { .. }
                | AssistantEvent::ToolcallStart { .. }
                | AssistantEvent::ToolcallDelta { .. }
                | AssistantEvent::ToolcallEnd { .. }
                | AssistantEvent::Done { .. }
                | AssistantEvent::Unknown => {}
            }
            if let Some(u) = usage {
                if u.total_tokens > 0 {
                    out.push(AgentEvent::Usage {
                        input: u.input,
                        output: u.output,
                        cache_read: u.cache_read,
                        cache_write: u.cache_write,
                        cost_usd: u
                            .cost
                            .as_ref()
                            .and_then(|c| c.get("total"))
                            .and_then(Value::as_f64),
                    });
                }
            }
            out
        }
        Event::ToolExecutionStart {
            tool_call_id,
            tool_name,
            args,
            ..
        } => vec![AgentEvent::ToolCall {
            id: tool_call_id.clone(),
            name: tool_name.clone(),
            input: args.clone(),
        }],
        Event::ToolExecutionEnd {
            tool_call_id,
            tool_name,
            result,
            is_error,
            ..
        } => {
            let output = result
                .get("content")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|c| c.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_else(|| result.to_string());
            vec![AgentEvent::ToolResult {
                id: tool_call_id.clone(),
                name: tool_name.clone(),
                output,
                is_error: *is_error,
            }]
        }
        Event::CompactionStart { .. } => vec![AgentEvent::CompactionStarted],
        Event::CompactionEnd { result, .. } => vec![AgentEvent::CompactionEnded {
            summary: result
                .as_ref()
                .and_then(|r| r.get("summary"))
                .and_then(Value::as_str)
                .map(str::to_owned),
        }],
        Event::AgentSettled { aborted } => vec![AgentEvent::Done {
            aborted: *aborted,
            error: None,
        }],
        Event::AgentStart
        | Event::AgentEnd { .. }
        | Event::TurnStart
        | Event::TurnEnd { .. }
        | Event::MessageStart { .. }
        | Event::MessageEnd { .. }
        | Event::ToolExecutionUpdate { .. }
        | Event::QueueUpdate { .. }
        | Event::EntryAppended { .. }
        | Event::ExtensionUiRequest { .. }
        | Event::Other => vec![],
    }
}

/// Shared implementation for the two pi-schema RPC harnesses (pi, prime-agent).
struct RpcSessions {
    children: Mutex<HashMap<String, Arc<RpcChild>>>,
    streams: Mutex<HashMap<String, broadcast::Sender<AgentEvent>>>,
}

impl RpcSessions {
    fn new() -> Self {
        Self {
            children: Mutex::new(HashMap::new()),
            streams: Mutex::new(HashMap::new()),
        }
    }

    async fn attach(
        &self,
        session_id: &str,
        child: Arc<RpcChild>,
        settled_is_agent_end: bool,
    ) -> broadcast::Receiver<AgentEvent> {
        let (tx, rx) = broadcast::channel(4096);
        self.streams
            .lock()
            .await
            .insert(session_id.to_owned(), tx.clone());
        self.children
            .lock()
            .await
            .insert(session_id.to_owned(), child.clone());
        let mut events = child.subscribe();
        let child2 = child.clone();
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(e) => {
                        // Prime has no agent_settled: treat agent_end as Done (design §4.4.2).
                        if settled_is_agent_end {
                            if let Event::AgentEnd { will_retry, .. } = &e {
                                if !*will_retry {
                                    let _ = tx.send(AgentEvent::Done {
                                        aborted: false,
                                        error: None,
                                    });
                                }
                                continue;
                            }
                        }
                        for ae in translate_rpc_event(&e) {
                            let _ = tx.send(ae);
                        }
                        if matches!(e, Event::Other) && !child2.is_alive() {
                            let _ = tx.send(AgentEvent::Done {
                                aborted: true,
                                error: Some("child exited".into()),
                            });
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        rx
    }

    async fn child(&self, session_id: &str) -> Result<Arc<RpcChild>> {
        self.children
            .lock()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| anyhow!("session {session_id} is not open"))
    }
}

// ---- PrimeAdapter ------------------------------------------------------------------------------

pub struct PrimeAdapter {
    cfg: Arc<Config>,
    store: Store,
    sessions: RpcSessions,
}

impl PrimeAdapter {
    pub fn new(cfg: Arc<Config>, store: Store) -> Self {
        Self {
            cfg,
            store,
            sessions: RpcSessions::new(),
        }
    }

    fn envs(&self, session_id: &str) -> Vec<(String, String)> {
        let prime_dir = self.cfg.prime_dir();
        let mut envs = self.cfg.browser_env();
        envs.extend(vec![
            (
                "PRIME_AGENT_CODING_AGENT_DIR".into(),
                prime_dir.display().to_string(),
            ),
            (
                "PRIME_AGENT_SESSION_DIR".into(),
                prime_dir.join("sessions").display().to_string(),
            ),
            ("PRIME_AGENT_TELEMETRY".into(), "0".into()),
            ("DO_NOT_TRACK".into(), "1".into()),
            ("ARBOS_SESSION_ID".into(), session_id.to_owned()),
            (
                "ARBOS_STATE_DIR".into(),
                self.cfg.state_dir.display().to_string(),
            ),
            (
                "ARBOS_AUDIT_PATH".into(),
                self.cfg.audit_path().display().to_string(),
            ),
            ("ARBOS_MCP_TOKEN".into(), self.cfg.subagent_mcp_token()),
            ("ARBOS_MCP_URL".into(), self.cfg.mcp_url()),
            ("ARBOS_BOX_TOKEN".into(), self.cfg.box_token.clone()),
            (
                "ARBOS_MODEL_PROXY_URL".into(),
                format!("{}/v1", self.cfg.proxy_base()),
            ),
            ("ARBOS_TALKING_MODEL".into(), self.cfg.talking_model.clone()),
            // D2: Prime's `openrouter` provider is pointed at the loopback stub by models.json in the prime
            // dir; the box token is the only credential it ever sees.
            ("OPENROUTER_API_KEY".into(), self.cfg.box_token.clone()),
            (
                "HOME".into(),
                std::env::var("HOME").unwrap_or_else(|_| "/home/daytona".into()),
            ),
            (
                "PRIME_AGENT_KERNEL_VENV".into(),
                std::env::var("PRIME_AGENT_KERNEL_VENV")
                    .unwrap_or_else(|_| "/opt/prime-venv".into()),
            ),
            (
                "PRIME_AGENT_KERNEL_PYTHON".into(),
                std::env::var("PRIME_AGENT_KERNEL_PYTHON")
                    .unwrap_or_else(|_| "/opt/prime-venv/bin/python".into()),
            ),
            (
                "PRIME_AGENT_BASH_SHELL".into(),
                std::env::var("PRIME_AGENT_BASH_SHELL")
                    .unwrap_or_else(|_| "/usr/local/lib/arbos/arbos-shell".into()),
            ),
        ]);
        envs.extend(crate::vault::shell_envs(&self.cfg));
        envs
    }

    async fn spawn(
        &self,
        session_id: &str,
        extra: &[String],
        cwd: &std::path::Path,
    ) -> Result<Arc<RpcChild>> {
        crate::isolation::ensure_worker_dir(
            &self.cfg.prime_dir().join("sessions"),
            self.cfg.run_as.as_ref(),
        )?;
        let mut args: Vec<String> = vec![
            "--mode".into(),
            "rpc".into(),
            "--session-dir".into(),
            self.cfg.prime_dir().join("sessions").display().to_string(),
        ];
        args.extend(extra.iter().cloned());
        let log = self
            .cfg
            .state_dir
            .join("logs")
            .join(format!("prime-{session_id}.log"));
        std::fs::create_dir_all(log.parent().unwrap())?;
        RpcChild::spawn(
            &format!("prime:{session_id}"),
            &self.cfg.prime_bin,
            &args,
            &self.envs(session_id),
            cwd,
            &log,
            self.cfg.run_as.as_ref(),
        )
        .await
    }
}

#[async_trait]
impl HarnessAdapter for PrimeAdapter {
    fn kind(&self) -> Harness {
        Harness::Prime
    }

    async fn start(&self, req: StartRequest) -> Result<()> {
        // Prime has no --session-id: it mints a UUID; we keep subagent_id → uuid for --resume.
        let extra = vec![
            "--provider".into(),
            self.cfg.pi_provider(),
            "--model".into(),
            req.model.clone(),
            "--append-system-prompt".into(),
            req.system_prompt.clone(),
        ];
        let child = self.spawn(&req.session_id, &extra, &req.cwd).await?;
        // A child that never answers `get_state` (a thrashing box) is killed here, not left
        // running: the reader task holds the Arc, so dropping it would not reap the process.
        let state = match child
            .call_timeout(
                RpcCommand::GetState {
                    id: child.next_id("st"),
                },
                std::time::Duration::from_secs(60),
            )
            .await
        {
            Ok(r) => r.data.unwrap_or(Value::Null),
            Err(e) => {
                child.shutdown().await;
                return Err(e);
            }
        };
        let path = state
            .get("sessionFile")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if let Some(uuid) = state.get("sessionId").and_then(Value::as_str) {
            self.store
                .meta_set(&format!("prime_uuid:{}", req.session_id), uuid)?;
        }
        self.store.upsert_session(
            &req.session_id,
            Some(&req.session_id),
            path.as_deref(),
            &req.cwd.display().to_string(),
            &req.model,
            "prime",
            "subagent",
        )?;
        self.sessions.attach(&req.session_id, child, true).await;
        Ok(())
    }

    async fn resume(&self, session_id: &str) -> Result<()> {
        if self.is_open(session_id) {
            return Ok(());
        }
        let uuid = self
            .store
            .meta_get(&format!("prime_uuid:{session_id}"))?
            .unwrap_or_else(|| session_id.to_owned());
        let extra = vec![
            "--resume".into(),
            uuid,
            "--provider".into(),
            self.cfg.pi_provider(),
            "--model".into(),
            self.cfg.subagent_model.clone(),
        ];
        let child = self.spawn(session_id, &extra, &self.cfg.work_dir).await?;
        self.sessions.attach(session_id, child, true).await;
        Ok(())
    }

    async fn prompt(&self, session_id: &str, text: &str) -> Result<()> {
        let c = self.sessions.child(session_id).await?;
        c.call_ok(RpcCommand::Prompt {
            id: c.next_id("p"),
            message: text.to_owned(),
            // A worker that is still finishing a run queues the brief instead of refusing it (#2073).
            streaming_behavior: Some(StreamingBehavior::FollowUp),
            images: Vec::new(),
        })
        .await
        .map(|_| ())
    }

    async fn follow_up(&self, session_id: &str, text: &str) -> Result<()> {
        let c = self.sessions.child(session_id).await?;
        c.call_ok(RpcCommand::FollowUp {
            id: c.next_id("f"),
            message: text.to_owned(),
        })
        .await
        .map(|_| ())
    }

    async fn steer(&self, session_id: &str, text: &str) -> Result<()> {
        let c = self.sessions.child(session_id).await?;
        c.call_ok(RpcCommand::Steer {
            id: c.next_id("s"),
            message: text.to_owned(),
        })
        .await
        .map(|_| ())
    }

    async fn cancel(&self, session_id: &str) -> Result<()> {
        let c = self.sessions.child(session_id).await?;
        c.call_ok(RpcCommand::Abort { id: c.next_id("a") })
            .await
            .map(|_| ())
    }

    fn stream(&self, session_id: &str) -> Option<broadcast::Receiver<AgentEvent>> {
        self.sessions
            .streams
            .try_lock()
            .ok()
            .and_then(|s| s.get(session_id).map(|tx| tx.subscribe()))
    }

    /// Prime has no `get_entries`: tail the JSONL by byte offset (design §4.4.2).
    async fn read_history(
        &self,
        session_id: &str,
        cursor: Option<String>,
    ) -> Result<(Vec<Value>, Option<String>)> {
        let path = match self.store.session_path(session_id)? {
            Some(p) => PathBuf::from(p),
            None => find_session_file(&self.cfg.prime_dir().join("sessions"), session_id)
                .ok_or_else(|| anyhow!("no session file for {session_id}"))?,
        };
        tail_jsonl(&path, cursor)
    }

    async fn stop(&self, session_id: &str) -> Result<()> {
        if let Some(c) = self.sessions.children.lock().await.remove(session_id) {
            c.shutdown().await;
        }
        Ok(())
    }

    async fn shutdown_all(&self) {
        let children: Vec<Arc<RpcChild>> = self
            .sessions
            .children
            .lock()
            .await
            .drain()
            .map(|(_, c)| c)
            .collect();
        for c in children {
            c.shutdown().await;
        }
    }

    fn is_open(&self, session_id: &str) -> bool {
        self.sessions
            .children
            .try_lock()
            .map(|c| c.get(session_id).map(|ch| ch.is_alive()).unwrap_or(false))
            .unwrap_or(false)
    }
}

// ---- PiAdapter ---------------------------------------------------------------------------------

pub struct PiAdapter {
    cfg: Arc<Config>,
    store: Store,
    sessions: RpcSessions,
}

impl PiAdapter {
    pub fn new(cfg: Arc<Config>, store: Store) -> Self {
        Self {
            cfg,
            store,
            sessions: RpcSessions::new(),
        }
    }

    pub fn envs(cfg: &Config, session_id: &str, role: &str) -> Vec<(String, String)> {
        let mut envs = cfg.browser_env();
        envs.extend(vec![
            (
                "PI_CODING_AGENT_DIR".into(),
                cfg.pi_dir().display().to_string(),
            ),
            ("PI_TELEMETRY".into(), "0".into()),
            ("PI_SKIP_VERSION_CHECK".into(), "1".into()),
            ("ARBOS_SESSION_ID".into(), session_id.to_owned()),
            ("ARBOS_SESSION_ROLE".into(), role.to_owned()),
            (
                "ARBOS_STATE_DIR".into(),
                cfg.state_dir.display().to_string(),
            ),
            (
                "ARBOS_AUDIT_PATH".into(),
                cfg.audit_path().display().to_string(),
            ),
            (
                "ARBOS_BLOB_DIR".into(),
                cfg.blobs_dir().display().to_string(),
            ),
            ("ARBOS_HOOK_URL".into(), cfg.hook_base()),
            ("ARBOS_MCP_TOKEN".into(), cfg.mcp_token_for_role(role)),
            ("ARBOS_MCP_URL".into(), cfg.mcp_url()),
            ("ARBOS_BOX_TOKEN".into(), cfg.box_token.clone()),
            // D86: the extension applies the script guardrails (default deadline) when on.
            (
                "ARBOS_CODE_MODE".into(),
                if cfg.code_mode() { "on" } else { "off" }.into(),
            ),
            // D87: the extension puts the working set, skills and memory in the turn, not the prompt.
            (
                "ARBOS_STABLE_PREFIX".into(),
                if cfg.stable_prefix() { "on" } else { "off" }.into(),
            ),
        ]);
        // Approved vault grants (F8): placeholders only, usable through the egress tool.
        envs.extend(crate::vault::shell_envs(cfg));
        envs
    }

    pub fn common_args(
        cfg: &Config,
        session_dir: &std::path::Path,
        model: &str,
        system_prompt_path: &std::path::Path,
    ) -> Vec<String> {
        vec![
            "--mode".into(),
            "rpc".into(),
            "--session-dir".into(),
            session_dir.display().to_string(),
            "--provider".into(),
            cfg.pi_provider(),
            "--model".into(),
            model.to_owned(),
            "--thinking".into(),
            "off".into(),
            "--approve".into(),
            "--extension".into(),
            cfg.pi_ext_path.display().to_string(),
            "--append-system-prompt".into(),
            system_prompt_path.display().to_string(),
        ]
    }

    async fn spawn(
        &self,
        session_id: &str,
        extra: &[String],
        cwd: &std::path::Path,
        system_prompt: &str,
        model: &str,
    ) -> Result<Arc<RpcChild>> {
        let sessions_dir = self.cfg.pi_dir().join("subagent-sessions");
        crate::isolation::ensure_worker_dir(&sessions_dir, self.cfg.run_as.as_ref())?;
        let prompt_path = self
            .cfg
            .state_dir
            .join("packs")
            .join(format!("{session_id}.md"));
        crate::isolation::ensure_worker_dir(
            prompt_path.parent().unwrap(),
            self.cfg.run_as.as_ref(),
        )?;
        std::fs::write(&prompt_path, system_prompt)?;
        let mut args = Self::common_args(&self.cfg, &sessions_dir, model, &prompt_path);
        args.extend(extra.iter().cloned());
        let log = self
            .cfg
            .state_dir
            .join("logs")
            .join(format!("pi-sub-{session_id}.log"));
        std::fs::create_dir_all(log.parent().unwrap())?;
        RpcChild::spawn(
            &format!("pi:{session_id}"),
            &self.cfg.pi_bin,
            &args,
            &Self::envs(&self.cfg, session_id, "subagent"),
            cwd,
            &log,
            self.cfg.run_as.as_ref(),
        )
        .await
    }
}

#[async_trait]
impl HarnessAdapter for PiAdapter {
    fn kind(&self) -> Harness {
        Harness::Pi
    }

    async fn start(&self, req: StartRequest) -> Result<()> {
        let extra = vec!["--session-id".into(), req.session_id.clone()];
        let model = if req.model.trim().is_empty() {
            self.cfg.subagent_model.clone()
        } else {
            req.model.clone()
        };
        let child = self
            .spawn(
                &req.session_id,
                &extra,
                &req.cwd,
                &req.system_prompt,
                &model,
            )
            .await?;
        let state = child
            .call_ok(RpcCommand::GetState {
                id: child.next_id("st"),
            })
            .await
            .unwrap_or(Value::Null);
        let path = state
            .get("sessionFile")
            .and_then(Value::as_str)
            .map(str::to_owned);
        self.store.upsert_session(
            &req.session_id,
            Some(&req.session_id),
            path.as_deref(),
            &req.cwd.display().to_string(),
            &req.model,
            "pi",
            "subagent",
        )?;
        self.sessions.attach(&req.session_id, child, false).await;
        Ok(())
    }

    async fn resume(&self, session_id: &str) -> Result<()> {
        if self.is_open(session_id) {
            return Ok(());
        }
        let extra = vec!["--session".into(), session_id.to_owned()];
        let pack = std::fs::read_to_string(
            self.cfg
                .state_dir
                .join("packs")
                .join(format!("{session_id}.md")),
        )
        .unwrap_or_default();
        // A resumed worker keeps the model its session was started with when the store knows it.
        let model = self
            .store
            .session_model(session_id)
            .ok()
            .flatten()
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| self.cfg.subagent_model.clone());
        let child = self
            .spawn(session_id, &extra, &self.cfg.work_dir, &pack, &model)
            .await?;
        self.sessions.attach(session_id, child, false).await;
        Ok(())
    }

    async fn prompt(&self, session_id: &str, text: &str) -> Result<()> {
        let c = self.sessions.child(session_id).await?;
        c.call_ok(RpcCommand::Prompt {
            id: c.next_id("p"),
            message: text.to_owned(),
            // A worker that is still finishing a run queues the brief instead of refusing it (#2073).
            streaming_behavior: Some(StreamingBehavior::FollowUp),
            images: Vec::new(),
        })
        .await
        .map(|_| ())
    }

    async fn follow_up(&self, session_id: &str, text: &str) -> Result<()> {
        let c = self.sessions.child(session_id).await?;
        c.call_ok(RpcCommand::FollowUp {
            id: c.next_id("f"),
            message: text.to_owned(),
        })
        .await
        .map(|_| ())
    }

    async fn steer(&self, session_id: &str, text: &str) -> Result<()> {
        let c = self.sessions.child(session_id).await?;
        c.call_ok(RpcCommand::Steer {
            id: c.next_id("s"),
            message: text.to_owned(),
        })
        .await
        .map(|_| ())
    }

    async fn cancel(&self, session_id: &str) -> Result<()> {
        let c = self.sessions.child(session_id).await?;
        c.call_ok(RpcCommand::Abort { id: c.next_id("a") })
            .await
            .map(|_| ())
    }

    fn stream(&self, session_id: &str) -> Option<broadcast::Receiver<AgentEvent>> {
        self.sessions
            .streams
            .try_lock()
            .ok()
            .and_then(|s| s.get(session_id).map(|tx| tx.subscribe()))
    }

    async fn read_history(
        &self,
        session_id: &str,
        cursor: Option<String>,
    ) -> Result<(Vec<Value>, Option<String>)> {
        let c = self.sessions.child(session_id).await?;
        let data = c
            .call_ok(RpcCommand::GetEntries {
                id: c.next_id("ge"),
                since: cursor,
            })
            .await?;
        let entries = data
            .get("entries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let leaf = data
            .get("leafId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        Ok((entries, leaf))
    }

    async fn stop(&self, session_id: &str) -> Result<()> {
        if let Some(c) = self.sessions.children.lock().await.remove(session_id) {
            c.shutdown().await;
        }
        Ok(())
    }

    async fn shutdown_all(&self) {
        let children: Vec<Arc<RpcChild>> = self
            .sessions
            .children
            .lock()
            .await
            .drain()
            .map(|(_, c)| c)
            .collect();
        for c in children {
            c.shutdown().await;
        }
    }

    fn is_open(&self, session_id: &str) -> bool {
        self.sessions
            .children
            .try_lock()
            .map(|c| c.get(session_id).map(|ch| ch.is_alive()).unwrap_or(false))
            .unwrap_or(false)
    }
}

// ---- ClaudeCodeAdapter -------------------------------------------------------------------------

/// Headless `claude -p --output-format stream-json --input-format stream-json` child per session
/// (design §4.4.2; sandbox-image.md §2.3). Provider = the relay's Anthropic passthrough through the
/// loopback stub (`ANTHROPIC_BASE_URL`), so no Anthropic key enters the box.
type ClaudeChild = (
    Arc<Mutex<tokio::process::Child>>,
    Arc<Mutex<tokio::process::ChildStdin>>,
);

pub struct ClaudeCodeAdapter {
    cfg: Arc<Config>,
    store: Store,
    children: Mutex<HashMap<String, ClaudeChild>>,
    streams: Mutex<HashMap<String, broadcast::Sender<AgentEvent>>>,
}

impl ClaudeCodeAdapter {
    pub fn new(cfg: Arc<Config>, store: Store) -> Self {
        Self {
            cfg,
            store,
            children: Mutex::new(HashMap::new()),
            streams: Mutex::new(HashMap::new()),
        }
    }

    async fn spawn(
        &self,
        session_id: &str,
        resume: bool,
        system_prompt: &str,
        cwd: &std::path::Path,
    ) -> Result<()> {
        use std::process::Stdio;
        use tokio::io::{AsyncBufReadExt, BufReader};
        // No `--bare`: the seed settings (PreToolUse deny/audit hook, bypassPermissions) must load.
        let mut args: Vec<String> = vec![
            "-p".into(),
            "--permission-mode".into(),
            "bypassPermissions".into(),
            "--output-format".into(),
            "stream-json".into(),
            "--input-format".into(),
            "stream-json".into(),
            "--verbose".into(),
            "--include-partial-messages".into(),
            "--mcp-config".into(),
            "/etc/arbos/claude-mcp.json".into(),
        ];
        // Claude Code insists on a UUID session id; derive one from the subagent ULID (same 128 bits).
        let claude_id = claude_session_uuid(session_id);
        if resume {
            args.extend(["--resume".into(), claude_id]);
        } else {
            args.extend([
                "--session-id".into(),
                claude_id,
                "--append-system-prompt".into(),
                system_prompt.to_owned(),
            ]);
        }
        let log = self
            .cfg
            .state_dir
            .join("logs")
            .join(format!("claude-{session_id}.log"));
        std::fs::create_dir_all(log.parent().unwrap())?;
        let mut cmd = tokio::process::Command::new(&self.cfg.claude_bin);
        cmd.args(&args)
            .current_dir(cwd)
            .env("CLAUDE_CONFIG_DIR", self.cfg.state_dir.join("claude"))
            .env("ANTHROPIC_BASE_URL", self.cfg.proxy_base())
            .env("ANTHROPIC_AUTH_TOKEN", &self.cfg.box_token)
            // The relay's Anthropic passthrough may sit on OpenRouter, whose ids are `anthropic/<model>`.
            .env("ANTHROPIC_MODEL", &self.cfg.subagent_model)
            .env("ANTHROPIC_DEFAULT_SONNET_MODEL", &self.cfg.subagent_model)
            .env("ANTHROPIC_DEFAULT_HAIKU_MODEL", &self.cfg.talking_model)
            .env("ARBOS_MCP_TOKEN", self.cfg.subagent_mcp_token())
            .env("ARBOS_AUDIT_PATH", self.cfg.audit_path())
            .env("DISABLE_AUTOUPDATER", "1")
            .env("DISABLE_TELEMETRY", "1")
            .env("DISABLE_ERROR_REPORTING", "1")
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
            .envs(crate::vault::shell_envs(&self.cfg))
            .envs(self.cfg.browser_env())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(std::fs::File::create(&log)?));
        if let Some(ra) = &self.cfg.run_as {
            cmd.uid(ra.uid).gid(ra.gid);
            for (k, v) in crate::isolation::Isolation::env(ra) {
                cmd.env(k, v);
            }
            let _ = std::os::unix::fs::chown(&log, Some(ra.uid), Some(ra.gid));
        }
        let mut child = cmd.spawn()?;
        if let (Some(ra), Some(pid)) = (&self.cfg.run_as, child.id()) {
            crate::isolation::Isolation::attach(ra, pid);
        }
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        let (tx, _) = broadcast::channel(4096);
        self.streams
            .lock()
            .await
            .insert(session_id.to_owned(), tx.clone());
        self.children.lock().await.insert(
            session_id.to_owned(),
            (Arc::new(Mutex::new(child)), Arc::new(Mutex::new(stdin))),
        );
        self.store.upsert_session(
            session_id,
            Some(session_id),
            None,
            &cwd.display().to_string(),
            &self.cfg.subagent_model,
            "claude_code",
            "subagent",
        )?;
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(v) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                for ev in translate_claude_event(&v) {
                    let _ = tx.send(ev);
                }
            }
            let _ = tx.send(AgentEvent::Done {
                aborted: true,
                error: Some("claude exited".into()),
            });
        });
        Ok(())
    }

    async fn write_user(&self, session_id: &str, text: &str) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        let (_, stdin) = self
            .children
            .lock()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| anyhow!("session {session_id} is not open"))?;
        let msg = serde_json::json!({ "type": "user", "message": { "role": "user", "content": [{ "type": "text", "text": text }] } });
        let mut s = stdin.lock().await;
        s.write_all(format!("{msg}\n").as_bytes()).await?;
        s.flush().await?;
        Ok(())
    }
}

/// Claude Code stream-json → `AgentEvent` (assistant deltas, tool_use, tool_result, result).
pub fn translate_claude_event(v: &Value) -> Vec<AgentEvent> {
    let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
    match ty {
        "stream_event" => {
            let ev = v.get("event").cloned().unwrap_or(Value::Null);
            if ev.get("type").and_then(Value::as_str) == Some("content_block_delta") {
                let d = ev.get("delta").cloned().unwrap_or(Value::Null);
                return match d.get("type").and_then(Value::as_str) {
                    Some("text_delta") => vec![AgentEvent::TextDelta(
                        d.get("text")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                    )],
                    Some("thinking_delta") => vec![AgentEvent::ReasoningDelta(
                        d.get("thinking")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                    )],
                    _ => vec![],
                };
            }
            vec![]
        }
        "assistant" => {
            let mut out = Vec::new();
            if let Some(content) = v.pointer("/message/content").and_then(Value::as_array) {
                for c in content {
                    if c.get("type").and_then(Value::as_str) == Some("tool_use") {
                        out.push(AgentEvent::ToolCall {
                            id: c.get("id").and_then(Value::as_str).unwrap_or("").to_owned(),
                            name: c
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_owned(),
                            input: c.get("input").cloned().unwrap_or(Value::Null),
                        });
                    }
                }
            }
            if let Some(u) = v.pointer("/message/usage") {
                out.push(AgentEvent::Usage {
                    input: u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
                    output: u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
                    cache_read: u
                        .get("cache_read_input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    cache_write: u
                        .get("cache_creation_input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    cost_usd: None,
                });
            }
            out
        }
        "user" => {
            let mut out = Vec::new();
            if let Some(content) = v.pointer("/message/content").and_then(Value::as_array) {
                for c in content {
                    if c.get("type").and_then(Value::as_str) == Some("tool_result") {
                        let output = match c.get("content") {
                            Some(Value::String(s)) => s.clone(),
                            Some(Value::Array(a)) => a
                                .iter()
                                .filter_map(|x| x.get("text").and_then(Value::as_str))
                                .collect::<Vec<_>>()
                                .join("\n"),
                            _ => String::new(),
                        };
                        out.push(AgentEvent::ToolResult {
                            id: c
                                .get("tool_use_id")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_owned(),
                            name: String::new(),
                            output,
                            is_error: c.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                        });
                    }
                }
            }
            out
        }
        "result" => vec![AgentEvent::Done {
            aborted: false,
            error: v
                .get("is_error")
                .and_then(Value::as_bool)
                .filter(|b| *b)
                .map(|_| {
                    v.get("result")
                        .and_then(Value::as_str)
                        .unwrap_or("error")
                        .to_owned()
                }),
        }],
        _ => vec![],
    }
}

#[async_trait]
impl HarnessAdapter for ClaudeCodeAdapter {
    fn kind(&self) -> Harness {
        Harness::ClaudeCode
    }

    async fn start(&self, req: StartRequest) -> Result<()> {
        self.spawn(&req.session_id, false, &req.system_prompt, &req.cwd)
            .await
    }

    async fn resume(&self, session_id: &str) -> Result<()> {
        if self.is_open(session_id) {
            return Ok(());
        }
        self.spawn(session_id, true, "", &self.cfg.work_dir).await
    }

    async fn prompt(&self, session_id: &str, text: &str) -> Result<()> {
        self.write_user(session_id, text).await
    }

    async fn follow_up(&self, session_id: &str, text: &str) -> Result<()> {
        self.write_user(session_id, text).await
    }

    async fn steer(&self, session_id: &str, text: &str) -> Result<()> {
        self.write_user(session_id, text).await
    }

    async fn cancel(&self, session_id: &str) -> Result<()> {
        self.stop(session_id).await
    }

    fn stream(&self, session_id: &str) -> Option<broadcast::Receiver<AgentEvent>> {
        self.streams
            .try_lock()
            .ok()
            .and_then(|s| s.get(session_id).map(|tx| tx.subscribe()))
    }

    async fn read_history(
        &self,
        session_id: &str,
        cursor: Option<String>,
    ) -> Result<(Vec<Value>, Option<String>)> {
        let root = self.cfg.state_dir.join("claude").join("projects");
        let path = find_session_file(&root, session_id)
            .ok_or_else(|| anyhow!("no claude session file for {session_id}"))?;
        tail_jsonl(&path, cursor)
    }

    async fn stop(&self, session_id: &str) -> Result<()> {
        if let Some((child, _)) = self.children.lock().await.remove(session_id) {
            let _ = child.lock().await.kill().await;
        }
        Ok(())
    }

    async fn shutdown_all(&self) {
        let children: Vec<_> = self
            .children
            .lock()
            .await
            .drain()
            .map(|(_, (c, _))| c)
            .collect();
        for c in children {
            let _ = c.lock().await.kill().await;
        }
    }

    fn is_open(&self, session_id: &str) -> bool {
        self.children
            .try_lock()
            .map(|c| c.contains_key(session_id))
            .unwrap_or(false)
    }
}

/// A ULID's 128 bits in UUID text form (Claude Code only accepts UUIDs as session ids).
pub fn claude_session_uuid(id: &str) -> String {
    let bits: u128 = ulid::Ulid::from_string(id)
        .map(u128::from)
        .unwrap_or_else(|_| {
            use sha2::Digest;
            let h = sha2::Sha256::digest(id.as_bytes());
            u128::from_be_bytes(h[..16].try_into().expect("16 bytes"))
        });
    let hex = format!("{bits:032x}");
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

// ---- shared JSONL helpers ----------------------------------------------------------------------

/// Find `<…>_<session_id>.jsonl` or `<session_id>.jsonl` under `root` (recursive, two levels).
pub fn find_session_file(root: &std::path::Path, session_id: &str) -> Option<PathBuf> {
    fn walk(dir: &std::path::Path, session_id: &str, depth: u8) -> Option<PathBuf> {
        let rd = std::fs::read_dir(dir).ok()?;
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() && depth > 0 {
                if let Some(f) = walk(&p, session_id, depth - 1) {
                    return Some(f);
                }
            } else if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                if name.ends_with(".jsonl")
                    && (name == format!("{session_id}.jsonl")
                        || name.ends_with(&format!("_{session_id}.jsonl")))
                {
                    return Some(p);
                }
            }
        }
        None
    }
    walk(root, session_id, 3)
}

/// Read JSONL entries after a byte-offset cursor; returns `(entries, new_cursor)`.
pub fn tail_jsonl(
    path: &std::path::Path,
    cursor: Option<String>,
) -> Result<(Vec<Value>, Option<String>)> {
    use std::io::{BufRead, Seek};
    let offset: u64 = cursor.as_deref().and_then(|c| c.parse().ok()).unwrap_or(0);
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    let offset = offset.min(len);
    f.seek(std::io::SeekFrom::Start(offset))?;
    let mut reader = std::io::BufReader::new(f);
    let mut pos = offset;
    let mut out = Vec::new();
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            break;
        }
        if !line.ends_with('\n') {
            break; // partial trailing line: wait for the writer
        }
        pos += n as u64;
        if let Ok(v) = serde_json::from_str::<Value>(line.trim_end()) {
            out.push(v);
        }
    }
    Ok((out, Some(pos.to_string())))
}

/// Text extracted from one pi-schema JSONL entry for T3 indexing (`message` entries only).
pub fn entry_text(entry: &Value) -> Option<(String, String)> {
    if entry.get("type").and_then(Value::as_str) != Some("message") {
        return None;
    }
    let msg = entry.get("message")?;
    let role = msg
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if role == "system" {
        return None;
    }
    let text = match msg.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|c| match c.get("type").and_then(Value::as_str) {
                Some("text") => c.get("text").and_then(Value::as_str).map(str::to_owned),
                Some("toolCall") => Some(format!(
                    "[tool call {} {}]",
                    c.get("name").and_then(Value::as_str).unwrap_or(""),
                    c.get("arguments")
                        .map(|a| a.to_string())
                        .unwrap_or_default()
                )),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    };
    if text.trim().is_empty() {
        return None;
    }
    Some((role, text))
}
