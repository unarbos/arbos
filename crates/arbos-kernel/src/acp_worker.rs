//! An outside agent as a worker (P-14): Claude Code, Codex, Gemini CLI — any
//! program that speaks the Agent Client Protocol (ACP: JSON-RPC over
//! stdio, newline-delimited). An agent kind whose definition has an `acp`
//! command line runs its turns here instead of in the engine: the kernel
//! is the ACP *client*, the outside program is the agent.
//!
//! The folder contract holds: the worker has an agent folder, a transcript
//! (`user`, `assistant`, `thinking`, `tool`, `turn_complete` lines the
//! desktop already renders), an inbox, and a parent that gets the `done`
//! message like any child. Permission requests follow the agent's mode:
//! `auto` allows, `ask` puts the allow/deny to the user, `plan` allows
//! reads only. File reads and writes the agent routes through the client
//! (`fs/*`) are served from the worker's cwd.
//!
//! Aligned with the mesh's worker model (`docs/arbos-mesh-design.md`): a
//! worker starts a whole agent where the checkout is; here that agent is
//! someone else's program, started per turn in the worker's cwd. The
//! session is reloaded across turns when the program can (`loadSession`);
//! otherwise each turn is a fresh session and the prompt carries the
//! transcript's recent lines.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use arbos_core::{
    Agent, Event, EventKind, Mode, Place, ToolRec, Wake, WakeKind, append_event, load_transcript,
};
use arbos_engine::TurnControl;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
    sync::{mpsc, oneshot},
};

use crate::hooks::KernelHooks;

/// A turn may take this long before the program is killed.
const TURN_TIMEOUT: Duration = Duration::from_secs(60 * 60);
/// Transcript lines carried into a fresh session's prompt.
const CONTEXT_LINES: usize = 40;

/// Is `agent` run by an outside ACP program? The command line, when so.
pub fn command_for(place: &Place, agent: &Agent) -> Option<String> {
    if agent.kind.is_empty() {
        return None;
    }
    arbos_core::find_def(place, &agent.kind).and_then(|d| d.acp.filter(|c| !c.trim().is_empty()))
}

/// The session the program gave us last time, so it can be loaded again.
fn session_file(place: &Place, agent: &Agent) -> PathBuf {
    place.agent_dir(agent.id.as_str()).join("acp.json")
}

struct Rpc {
    stdin: tokio::process::ChildStdin,
    next_id: u64,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, Value>>>>>,
}

impl Rpc {
    async fn send(&mut self, v: &Value) -> Result<()> {
        let mut line = serde_json::to_string(v)?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn request(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<oneshot::Receiver<Result<Value, Value>>> {
        self.next_id += 1;
        let id = self.next_id;
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await?;
        Ok(rx)
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .await
    }

    async fn reply(&mut self, id: &Value, result: Value) -> Result<()> {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "result": result}))
            .await
    }

    async fn reply_error(&mut self, id: &Value, code: i64, message: &str) -> Result<()> {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}))
            .await
    }
}

/// What the reader thread hands the turn: a request or notification from
/// the agent (responses go straight to their waiters).
enum Incoming {
    Message(Value),
    Closed,
}

/// The per-turn state: text streamed so far, open tool calls.
#[derive(Default)]
struct TurnState {
    assistant: String,
    thinking: String,
    /// toolCallId → (title, started ms, raw input)
    tools: HashMap<String, (String, i64, Option<Value>)>,
}

/// One turn of an ACP-run agent.
pub async fn turn(
    place: Place,
    agent: Agent,
    wake: Wake,
    hooks: Arc<KernelHooks>,
    control: TurnControl,
) -> Result<()> {
    let cmd = command_for(&place, &agent)
        .ok_or_else(|| anyhow!("no acp command for kind {}", agent.kind))?;
    let cwd = agent.work_dir(&place.path);
    let transcript = place.agent_dir(agent.id.as_str()).join("transcript.jsonl");
    let agent_id = agent.id.to_string();

    // The prompt: the user's words on the transcript first, as the engine
    // does; a peer's words are already there as `say` lines.
    let mut prompt = wake.text.clone().unwrap_or_default();
    if wake.kind == WakeKind::User && !prompt.is_empty() {
        emit(
            &hooks,
            &agent_id,
            &transcript,
            Event::new(EventKind::User {
                text: prompt.clone(),
                attachments: wake.attachments.clone(),
                channel: wake.channel.clone(),
                device: wake.device.clone(),
            }),
        )?;
    }
    if prompt.trim().is_empty() {
        prompt = recent_context(&transcript, CONTEXT_LINES, true);
        if prompt.trim().is_empty() {
            prompt = "Continue.".into();
        }
    }

    let mut child = Command::new("sh")
        .arg("-lc")
        .arg(&cmd)
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("start acp agent: {cmd}"))?;
    let stdin = child.stdin.take().context("acp stdin")?;
    let stdout = child.stdout.take().context("acp stdout")?;
    let stderr = child.stderr.take().context("acp stderr")?;
    let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, Value>>>>> =
        Default::default();
    let mut rpc = Rpc {
        stdin,
        next_id: 0,
        pending: Arc::clone(&pending),
    };

    // Reader: responses to their waiters, everything else to the turn.
    let (in_tx, mut in_rx) = mpsc::unbounded_channel::<Incoming>();
    {
        let pending = Arc::clone(&pending);
        let in_tx = in_tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(v) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let is_response = v.get("id").is_some() && v.get("method").is_none();
                if is_response {
                    let id = v["id"].as_u64().unwrap_or(0);
                    if let Some(tx) = pending.lock().unwrap().remove(&id) {
                        let _ = tx.send(match v.get("error") {
                            Some(e) => Err(e.clone()),
                            None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
                        });
                    }
                } else {
                    let _ = in_tx.send(Incoming::Message(v));
                }
            }
            let _ = in_tx.send(Incoming::Closed);
        });
    }
    // Stderr: kept as the program's own log, one notice at the end if it
    // said anything and the turn failed.
    let stderr_tail: Arc<Mutex<String>> = Default::default();
    {
        let tail = Arc::clone(&stderr_tail);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut t = tail.lock().unwrap();
                t.push_str(&line);
                t.push('\n');
                if t.len() > 8_000 {
                    let cut = t.len() - 8_000;
                    t.drain(..cut);
                }
            }
        });
    }

    let result = drive(
        &mut rpc,
        &mut in_rx,
        &mut child,
        &place,
        &agent,
        &cwd,
        &transcript,
        &hooks,
        &control,
        prompt,
    )
    .await;

    let _ = rpc.notify("session/cancel", json!({})).await;
    let _ = child.kill().await;
    match result {
        Ok(()) => Ok(()),
        Err(e) => {
            let tail = stderr_tail.lock().unwrap().clone();
            let text = if tail.trim().is_empty() {
                format!("acp agent ({}) failed: {e:#}", agent.kind)
            } else {
                format!(
                    "acp agent ({}) failed: {e:#}\n{}",
                    agent.kind,
                    arbos_core::text::tail(&tail)
                )
            };
            let _ = emit(
                &hooks,
                &agent_id,
                &transcript,
                Event::new(EventKind::Notice { text, failed: true }),
            );
            let _ = emit(
                &hooks,
                &agent_id,
                &transcript,
                Event::new(EventKind::TurnComplete { usage: None }),
            );
            Err(e)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn drive(
    rpc: &mut Rpc,
    in_rx: &mut mpsc::UnboundedReceiver<Incoming>,
    child: &mut Child,
    place: &Place,
    agent: &Agent,
    cwd: &Path,
    transcript: &Path,
    hooks: &Arc<KernelHooks>,
    control: &TurnControl,
    prompt: String,
) -> Result<()> {
    let agent_id = agent.id.to_string();
    let deadline = tokio::time::Instant::now() + TURN_TIMEOUT;

    // initialize
    let init = rpc
        .request(
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientCapabilities": {"fs": {"readTextFile": true, "writeTextFile": true}, "terminal": false},
                "clientInfo": {"name": "arbos-kernel", "version": env!("CARGO_PKG_VERSION")}
            }),
        )
        .await?;
    let init = wait_response(
        init,
        in_rx,
        rpc,
        child,
        place,
        agent,
        cwd,
        transcript,
        hooks,
        control,
        deadline,
        &mut TurnState::default(),
    )
    .await
    .map_err(|e| anyhow!("initialize: {e}"))?;
    let can_load = init
        .pointer("/agentCapabilities/loadSession")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    // session: load the last one when the program can, else new.
    let session_path = session_file(place, agent);
    let remembered: Option<String> = std::fs::read_to_string(&session_path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v["sessionId"].as_str().map(str::to_string));
    let mut state = TurnState::default();
    let mut session_id: Option<String> = None;
    let mut fresh = true;
    if can_load && let Some(id) = remembered.clone() {
        let rx = rpc
            .request(
                "session/load",
                json!({"sessionId": id, "cwd": cwd.display().to_string(), "mcpServers": []}),
            )
            .await?;
        // A load replays the history as updates; they are not new words,
        // so they go to a scratch state.
        let mut scratch = TurnState::default();
        if wait_response(
            rx,
            in_rx,
            rpc,
            child,
            place,
            agent,
            cwd,
            transcript,
            hooks,
            control,
            deadline,
            &mut scratch,
        )
        .await
        .is_ok()
        {
            session_id = Some(id);
            fresh = false;
        }
    }
    if session_id.is_none() {
        let rx = rpc
            .request(
                "session/new",
                json!({"cwd": cwd.display().to_string(), "mcpServers": []}),
            )
            .await?;
        let v = wait_response(
            rx, in_rx, rpc, child, place, agent, cwd, transcript, hooks, control, deadline,
            &mut state,
        )
        .await
        .map_err(|e| anyhow!("session/new: {e}"))?;
        let id = v["sessionId"]
            .as_str()
            .context("session/new returned no sessionId")?
            .to_string();
        let _ = std::fs::write(
            &session_path,
            json!({"sessionId": id, "command": command_for(place, agent)}).to_string(),
        );
        session_id = Some(id);
    }
    let session_id = session_id.unwrap();

    // The prompt. A fresh session gets the folder contract and the recent
    // transcript, so the program knows where it is and what came before.
    let mut text = String::new();
    if fresh {
        text.push_str(&format!(
            "You are agent {} in an Arbos place at {}. Your folder is .arbos/agents/{}/ (transcript.jsonl is the record of earlier turns). Work in {}. When you finish, end with a short report of what you did; it reaches the agent that spawned you.\n\n",
            agent.id, place.path.display(), agent.id, cwd.display()
        ));
        let recent = recent_context(transcript, CONTEXT_LINES, false);
        if !recent.trim().is_empty() {
            text.push_str("Recent transcript:\n");
            text.push_str(&recent);
            text.push_str("\n\n");
        }
    }
    text.push_str(&prompt);
    let rx = rpc
        .request(
            "session/prompt",
            json!({"sessionId": session_id, "prompt": [{"type": "text", "text": text}]}),
        )
        .await?;
    let result = wait_response(
        rx, in_rx, rpc, child, place, agent, cwd, transcript, hooks, control, deadline, &mut state,
    )
    .await;
    // Whatever streamed lands on the transcript before the verdict.
    flush(&mut state, hooks, &agent_id, transcript)?;
    match result {
        Ok(v) => {
            let stop = v["stopReason"].as_str().unwrap_or("end_turn").to_string();
            if stop != "end_turn" && stop != "cancelled" {
                emit(
                    hooks,
                    &agent_id,
                    transcript,
                    Event::new(EventKind::Notice {
                        text: format!("acp agent stopped: {stop}"),
                        failed: stop == "refusal",
                    }),
                )?;
            }
            let mut batch = Vec::new();
            if stop == "cancelled" {
                batch.push(Event::new(EventKind::Interrupted {
                    detail: control.stop_reason(),
                }));
            }
            batch.push(Event::new(EventKind::TurnComplete { usage: None }));
            for e in batch {
                emit(hooks, &agent_id, transcript, e)?;
            }
            Ok(())
        }
        Err(e) => bail!("session/prompt: {e}"),
    }
}

/// Wait for one response while serving the agent's requests and
/// notifications; stop or time out kills the program.
#[allow(clippy::too_many_arguments)]
async fn wait_response(
    mut rx: oneshot::Receiver<Result<Value, Value>>,
    in_rx: &mut mpsc::UnboundedReceiver<Incoming>,
    rpc: &mut Rpc,
    child: &mut Child,
    place: &Place,
    agent: &Agent,
    cwd: &Path,
    transcript: &Path,
    hooks: &Arc<KernelHooks>,
    control: &TurnControl,
    deadline: tokio::time::Instant,
    state: &mut TurnState,
) -> std::result::Result<Value, String> {
    loop {
        tokio::select! {
            r = &mut rx => {
                // Notifications the agent sent before its answer are already
                // queued (the reader forwards in reading order): take them
                // now, so the last streamed words are not left behind.
                while let Ok(Incoming::Message(v)) = in_rx.try_recv() {
                    if let Err(e) = handle_incoming(v, rpc, place, agent, cwd, transcript, hooks, state).await {
                        crate::klog::warn("acp_incoming", Some(agent.id.as_str()), format!("{e:#}"));
                    }
                }
                return match r {
                    Ok(Ok(v)) => Ok(v),
                    Ok(Err(e)) => Err(e["message"].as_str().unwrap_or("error").to_string()),
                    Err(_) => Err("the agent closed without answering".into()),
                };
            }
            msg = in_rx.recv() => {
                match msg {
                    Some(Incoming::Message(v)) => {
                        if let Err(e) = handle_incoming(v, rpc, place, agent, cwd, transcript, hooks, state).await {
                            crate::klog::warn("acp_incoming", Some(agent.id.as_str()), format!("{e:#}"));
                        }
                    }
                    Some(Incoming::Closed) | None => {
                        let _ = child.wait().await;
                        return Err("the agent exited".into());
                    }
                }
            }
            _ = control.cancel().cancelled() => {
                let _ = rpc.notify("session/cancel", json!({})).await;
                let _ = child.kill().await;
                return Ok(json!({"stopReason": "cancelled"}));
            }
            _ = tokio::time::sleep_until(deadline) => {
                let _ = child.kill().await;
                return Err(format!("no answer after {}s", TURN_TIMEOUT.as_secs()));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_incoming(
    v: Value,
    rpc: &mut Rpc,
    place: &Place,
    agent: &Agent,
    cwd: &Path,
    transcript: &Path,
    hooks: &Arc<KernelHooks>,
    state: &mut TurnState,
) -> Result<()> {
    let method = v["method"].as_str().unwrap_or("");
    let params = v.get("params").cloned().unwrap_or(Value::Null);
    let id = v.get("id").cloned();
    let agent_id = agent.id.to_string();
    match method {
        "session/update" => {
            let update = &params["update"];
            match update["sessionUpdate"].as_str().unwrap_or("") {
                "agent_message_chunk" => {
                    if let Some(t) = update.pointer("/content/text").and_then(Value::as_str) {
                        state.assistant.push_str(t);
                        hooks.broadcast(arbos_core::wire::Frame::AssistantDelta {
                            agent: agent_id.clone(),
                            text: t.to_string(),
                        });
                    }
                }
                "agent_thought_chunk" => {
                    if let Some(t) = update.pointer("/content/text").and_then(Value::as_str) {
                        state.thinking.push_str(t);
                        hooks.broadcast(arbos_core::wire::Frame::ThinkingDelta {
                            agent: agent_id.clone(),
                            text: t.to_string(),
                        });
                    }
                }
                "tool_call" => {
                    // Words before a tool call are their own assistant line,
                    // as the engine writes one per step.
                    flush(state, hooks, &agent_id, transcript)?;
                    let call_id = update["toolCallId"].as_str().unwrap_or("").to_string();
                    let title = update["title"].as_str().unwrap_or("tool").to_string();
                    state.tools.insert(
                        call_id,
                        (title, arbos_core::now_ms(), update.get("rawInput").cloned()),
                    );
                }
                "tool_call_update" => {
                    let call_id = update["toolCallId"].as_str().unwrap_or("").to_string();
                    let status = update["status"].as_str().unwrap_or("");
                    if matches!(status, "completed" | "failed") {
                        let (title, started, input) = state
                            .tools
                            .remove(&call_id)
                            .unwrap_or_else(|| ("tool".into(), arbos_core::now_ms(), None));
                        let body = content_text(update.get("content"))
                            .or_else(|| update.get("rawOutput").map(|o| o.to_string()));
                        let paths: Vec<String> = update["locations"]
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(|l| l["path"].as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let name = title
                            .split_whitespace()
                            .next()
                            .unwrap_or("tool")
                            .to_ascii_lowercase();
                        emit(
                            hooks,
                            &agent_id,
                            transcript,
                            Event::new(EventKind::Tool(ToolRec {
                                name: format!("acp:{name}"),
                                call_id,
                                paths,
                                started: Some(started),
                                ended: Some(arbos_core::now_ms()),
                                result_size: body.as_ref().map(|b| b.len() as u64),
                                error: (status == "failed")
                                    .then(|| body.clone().unwrap_or_else(|| "failed".into())),
                                body: body.map(|b| arbos_core::text::tail(&b)),
                                args: input.or_else(|| Some(json!({"title": title}))),
                                child: None,
                                images: Vec::new(),
                                diff: None,
                                label: None,
                            })),
                        )?;
                    }
                }
                _ => {}
            }
            Ok(())
        }
        "session/request_permission" => {
            let Some(id) = id else { return Ok(()) };
            let options = params["options"].as_array().cloned().unwrap_or_default();
            let pick = |kind: &str| {
                options
                    .iter()
                    .find(|o| o["kind"].as_str() == Some(kind))
                    .and_then(|o| o["optionId"].as_str().map(str::to_string))
            };
            let title = params
                .pointer("/toolCall/title")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_string();
            let kind = params
                .pointer("/toolCall/kind")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            // The permission request names the call; an update may be all
            // that follows, so this is where its title is learnt.
            if let Some(call_id) = params
                .pointer("/toolCall/toolCallId")
                .and_then(Value::as_str)
            {
                state.tools.entry(call_id.to_string()).or_insert_with(|| {
                    (
                        title.clone(),
                        arbos_core::now_ms(),
                        params.pointer("/toolCall/rawInput").cloned(),
                    )
                });
            }
            let reads_only = matches!(kind.as_str(), "read" | "search" | "fetch" | "think");
            let allow = match agent.mode {
                Mode::Auto => true,
                Mode::Plan => reads_only,
                Mode::Ask => {
                    if reads_only {
                        true
                    } else {
                        hooks
                            .approve(&agent.id, "acp", &title)
                            .await
                            .unwrap_or(false)
                    }
                }
            };
            let option = if allow {
                pick("allow_once").or_else(|| pick("allow_always"))
            } else {
                pick("reject_once").or_else(|| pick("reject_always"))
            };
            match option {
                Some(option_id) => {
                    rpc.reply(
                        &id,
                        json!({"outcome": {"outcome": "selected", "optionId": option_id}}),
                    )
                    .await
                }
                None => {
                    rpc.reply(&id, json!({"outcome": {"outcome": "cancelled"}}))
                        .await
                }
            }
        }
        "fs/read_text_file" => {
            let Some(id) = id else { return Ok(()) };
            let path = params["path"].as_str().unwrap_or("");
            match read_within(
                cwd,
                place,
                path,
                params["line"].as_u64(),
                params["limit"].as_u64(),
            ) {
                Ok(content) => rpc.reply(&id, json!({"content": content})).await,
                Err(e) => rpc.reply_error(&id, -32000, &format!("{e:#}")).await,
            }
        }
        "fs/write_text_file" => {
            let Some(id) = id else { return Ok(()) };
            if agent.readonly || agent.mode == Mode::Plan {
                return rpc
                    .reply_error(
                        &id,
                        -32000,
                        "this agent is read-only (mode plan / readonly)",
                    )
                    .await;
            }
            let path = params["path"].as_str().unwrap_or("");
            let content = params["content"].as_str().unwrap_or("");
            match write_within(cwd, place, path, content) {
                Ok(()) => rpc.reply(&id, json!({})).await,
                Err(e) => rpc.reply_error(&id, -32000, &format!("{e:#}")).await,
            }
        }
        other => {
            if let Some(id) = id {
                rpc.reply_error(&id, -32601, &format!("method not found: {other}"))
                    .await
            } else {
                Ok(())
            }
        }
    }
}

/// Streamed thinking and text so far become transcript lines.
fn flush(
    state: &mut TurnState,
    hooks: &Arc<KernelHooks>,
    agent_id: &str,
    transcript: &Path,
) -> Result<()> {
    if !state.thinking.trim().is_empty() {
        let text = std::mem::take(&mut state.thinking);
        emit(
            hooks,
            agent_id,
            transcript,
            Event::new(EventKind::Thinking { text, secs: None }),
        )?;
    }
    if !state.assistant.trim().is_empty() {
        let text = std::mem::take(&mut state.assistant);
        emit(
            hooks,
            agent_id,
            transcript,
            Event::new(EventKind::Assistant {
                text,
                reasoning_details: None,
            }),
        )?;
    }
    Ok(())
}

/// Append to the transcript and tell attached clients.
fn emit(hooks: &Arc<KernelHooks>, agent_id: &str, transcript: &Path, event: Event) -> Result<()> {
    append_event(transcript, &event)?;
    hooks.broadcast(arbos_core::wire::Frame::Event {
        agent: agent_id.to_string(),
        event,
    });
    Ok(())
}

fn content_text(content: Option<&Value>) -> Option<String> {
    let arr = content?.as_array()?;
    let mut out = String::new();
    for c in arr {
        if let Some(t) = c
            .pointer("/content/text")
            .and_then(Value::as_str)
            .or_else(|| c["text"].as_str())
        {
            out.push_str(t);
            out.push('\n');
        } else if let (Some(old), Some(new)) = (c["oldText"].as_str(), c["newText"].as_str()) {
            out.push_str(&format!(
                "--- {}\n-{}\n+{}\n",
                c["path"].as_str().unwrap_or(""),
                old.trim_end(),
                new.trim_end()
            ));
        }
    }
    (!out.trim().is_empty()).then(|| out.trim_end().to_string())
}

/// The transcript's last lines as text: what a fresh session needs to know.
fn recent_context(transcript: &Path, lines: usize, only_since_last_turn: bool) -> String {
    let events = load_transcript(transcript).unwrap_or_default();
    let start = if only_since_last_turn {
        events
            .iter()
            .rposition(|e| matches!(e.kind, EventKind::TurnComplete { .. }))
            .map(|i| i + 1)
            .unwrap_or(0)
    } else {
        events.len().saturating_sub(lines)
    };
    let mut out = String::new();
    for e in &events[start..] {
        let line = match &e.kind {
            EventKind::User { text, .. } => format!("user: {text}"),
            EventKind::Say { from, text } => format!("[{from}]: {text}"),
            EventKind::Assistant { text, .. } => {
                format!("you: {}", arbos_core::text::clip(text, 300))
            }
            EventKind::Notice { text, .. } => format!("kernel: {text}"),
            EventKind::Tool(rec) => format!(
                "tool {}: {}",
                rec.name,
                arbos_core::text::clip(rec.body.as_deref().unwrap_or(""), 120)
            ),
            _ => continue,
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// A path the agent asked to read: inside the place (or its cwd).
fn resolve_within(cwd: &Path, place: &Place, path: &str) -> Result<PathBuf> {
    let p = Path::new(path);
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    };
    let canon = abs.canonicalize().unwrap_or(abs.clone());
    let root = place
        .path
        .canonicalize()
        .unwrap_or_else(|_| place.path.clone());
    let cwd_c = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    if !(canon.starts_with(&root) || canon.starts_with(&cwd_c)) {
        bail!("{} is outside the place", abs.display());
    }
    Ok(abs)
}

fn read_within(
    cwd: &Path,
    place: &Place,
    path: &str,
    line: Option<u64>,
    limit: Option<u64>,
) -> Result<String> {
    let p = resolve_within(cwd, place, path)?;
    let text = std::fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?;
    match (line, limit) {
        (None, None) => Ok(text),
        (l, n) => {
            let start = l.unwrap_or(1).saturating_sub(1) as usize;
            let take = n.map(|n| n as usize).unwrap_or(usize::MAX);
            Ok(text
                .lines()
                .skip(start)
                .take(take)
                .collect::<Vec<_>>()
                .join("\n"))
        }
    }
}

fn write_within(cwd: &Path, place: &Place, path: &str, content: &str) -> Result<()> {
    let p = resolve_within(cwd, place, path)?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&p, content).with_context(|| format!("write {}", p.display()))
}
