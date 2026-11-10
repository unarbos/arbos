//! The `HarnessAdapter` registry (design §4.4.2, D67): `PrimeAdapter` (default),
//! `PiAdapter` (fallback), `ClaudeCodeAdapter` (long coding tasks). Everything above
//! this module sees only `AgentEvent`s.

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
use crate::settings::{LiveModels, ModelRole};
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
    /// What the session still has queued for the model. `steering` falling to zero is how a
    /// correction handed to a job's worker is known to have reached it (bug AB, `steer_gate.rs`):
    /// pi dequeues a steer at a tool boundary and reports the queue it has left.
    QueueUpdate {
        steering: usize,
        follow_up: usize,
    },
    /// The model call behind this session failed, as the provider explained it: a refusal, a
    /// context overflow, a 5xx. pi 1.2 reports all three on `message_end` only, and reports them
    /// *without* ending the session, so this is not a `Done` — a session that recovers goes on
    /// streaming after it. Its only use is to give a run that then settles with nothing a true
    /// reason (bug AR).
    ModelError {
        detail: String,
        refusal: bool,
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

/// One worker this adapter still holds a child for, as a retention pass reads it.
///
/// `idle_ms` is the child's own clock ([`RpcChild::idle_ms`]), not a store column. See that
/// field for why a store-derived clock cannot answer this question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedWorker {
    pub session_id: String,
    /// The child's pid, for a reading of its size. `None` where the adapter never had one.
    pub pid: Option<u32>,
    pub idle_ms: u64,
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
    /// Does this adapter still hold a live child for this session?
    ///
    /// [`None`] is "I could not look", never a guess: the map of children is behind one mutex
    /// per adapter, and a read that cannot take it knows nothing about any session. The three
    /// answers are kept apart because a kill decision reads a `false` as evidence that a
    /// session is over, and a contended lock used to produce exactly that `false` — so a live
    /// worker's whole tree was collectable for as long as a sibling session took to stop. Every
    /// caller that decides whether to signal a process must treat `None` as alive.
    ///
    /// Cleanup owns an exact Starting/Stopping receipt outside the registry lock. That
    /// session remains unknown until cleanup completes; independent Ready siblings are
    /// readable. A genuinely contended registry also remains unknown. The timer/cancel
    /// regression keeps physical writers alive and names only the unread receipt; neither
    /// uncertainty may be turned into evidence that a process is dead (bug AQ).
    fn is_open(&self, session_id: &str) -> Option<bool>;
    /// The process group this session's child leads, so a cancel can reap everything the child
    /// spawned after the child itself is gone (bug Z). `None` for an adapter whose children are
    /// not group leaders; the cancel then falls back to the cgroup alone.
    async fn child_pgid(&self, _session_id: &str) -> Option<u32> {
        None
    }

    /// Every worker this adapter still holds a child for, with each child's own idle clock, for
    /// [`crate::agent::Agent::retire_idle_workers`].
    ///
    /// **An adapter that cannot read its own map answers with no candidates.** Same three-state
    /// discipline as [`Self::is_open`], collapsed the safe way: a read that could not look knows
    /// nothing about any session, and the only thing it is allowed to do with that is retire
    /// nothing. There is no variant for "I could not look" because every caller would have to
    /// treat it as an empty list anyway.
    ///
    /// The default is the empty list, which is a **keep**, so an adapter that does not implement
    /// this has every worker spared. That is the polarity
    /// [`crate::isolation::orphan_marked_procs`] argues for, pointing the other way: here the
    /// list is of candidates to close, so omission costs memory and never a process.
    fn retained(&self) -> Vec<RetainedWorker> {
        Vec::new()
    }

    /// Close this session's child, but only if it is **still** idle; `true` when it was closed.
    ///
    /// The re-check is the point and it happens under the same lock that guards the map, so a
    /// session something took between the caller's decision and this call keeps its child. This
    /// is [`crate::agent::Agent`]'s `pi_parked` double-check applied to workers: the park reads
    /// `pi_idle_ms` a second time inside `pi.lock()` for exactly this reason.
    ///
    /// The default refuses, for the same reason the default above is empty.
    async fn stop_if_idle(&self, _session_id: &str, _min_idle_ms: u64) -> bool {
        false
    }

    /// Hold a child this box's harness binary could not have spawned, for the retention cases.
    ///
    /// The child comes from the caller through [`RpcChild::spawn`], so the process, the
    /// `ARBOS_WORKER` mark and the cgroup leaf are production's; only the program differs. It
    /// goes into the map through the same [`RpcSessions::attach`] a real start uses.
    #[cfg(test)]
    async fn test_attach_child(&self, _session_id: &str, _child: Arc<RpcChild>) {}

    /// Put one held child's idle clock `ms` into the past; [`None`] when it is not held.
    ///
    /// The clock moves and nothing else, so a case using this still closes a real process through
    /// the real path and still exercises the comparison in `retire_idle_workers` — which a
    /// test-only override of the bound would not.
    #[cfg(test)]
    async fn test_age_idle_clock(&self, _session_id: &str, _ms: u64) -> Option<()> {
        None
    }

    /// Hold one captured RPC process handle, without holding its stdin or the registry.
    #[cfg(test)]
    async fn test_hold_process_until(
        &self,
        _session_id: &str,
        _ready: tokio::sync::oneshot::Sender<()>,
        _release: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<()> {
        Err(anyhow!("adapter has no RPC process fixture"))
    }

    /// Take the children map, say so on `ready`, and hold it until `release` fires, so a case can
    /// measure a genuinely contended registry read. Child cleanup itself runs outside
    /// this lock; independent Ready siblings remain readable during Stop.
    #[cfg(test)]
    async fn test_hold_children_until(
        &self,
        _ready: tokio::sync::oneshot::Sender<()>,
        _release: tokio::sync::oneshot::Receiver<()>,
    ) {
    }
}

/// The name [`RpcChild::spawn`] starts a worker under.
///
/// It is not decoration. The name becomes the `ARBOS_WORKER` value every descendant inherits
/// and the cgroup leaf the worker is attached to, so a cancel that wants either handle has to
/// spell it the same way — and a cancel only has the session id to rebuild it from.
///
/// Bug AI, found in `r17status`: `Agent::worker_tree` built the mark as the bare session id
/// while the environment held `prime:<session id>`, and looked the leaf up as `w-<session id>`
/// while `attach_leaf` had created `w-prime-<session id>`. Both missed, silently, on every
/// cancel the box has ever run. One definition now, used by the spawn and by the cancel, so a
/// second spelling cannot be introduced by editing only one of them.
pub fn worker_name(harness: &str, session_id: &str) -> String {
    format!("{harness}:{session_id}")
}

/// Does this harness spawn its workers through [`RpcChild`], and so mark them?
///
/// Claude Code builds its own [`Command`], so its children carry no mark and own no cgroup
/// leaf. A cancel must know that rather than construct a name for them: a mark that cannot
/// match is not a handle, and treating it as one is how a sweep reports a clean kill having
/// looked at nothing.
pub fn marks_its_children(harness: &str) -> bool {
    matches!(harness, "pi" | "prime")
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
        Event::QueueUpdate {
            steering,
            follow_up,
        } => vec![AgentEvent::QueueUpdate {
            steering: steering.len(),
            follow_up: follow_up.len(),
        }],
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
        // The one place a worker can learn that its model call failed. `AssistantEvent::Error`
        // above is the streamed form and pi 1.2 never sends it: a refusal, a context overflow or
        // a provider 5xx arrives only here, as `stopReason: error` on the finished assistant
        // message (`agent.rs`, where the main session has always read it).
        //
        // Bug AR, filed in Test5 round 21 and settled by round 22's session file: the arm this
        // replaces discarded `message_end`, so a worker the provider had refused produced no
        // text, no tool call and no error, and agentd reported the only thing left to say —
        // "worker ended without output (no reply, no tool call)". The fresh worker got the same
        // brief and was refused the same way four seconds later, and the job fell to the inline
        // agent with its real reason recorded nowhere.
        Event::MessageEnd { message } => {
            match crate::agent::ModelError::from_message(message)
                .filter(|_| message.get("role").and_then(Value::as_str) == Some("assistant"))
            {
                Some(m) => vec![AgentEvent::ModelError {
                    detail: m.detail,
                    refusal: m.refusal,
                }],
                None => vec![],
            }
        }
        Event::AgentStart
        | Event::AgentEnd { .. }
        | Event::TurnStart
        | Event::TurnEnd { .. }
        | Event::MessageStart { .. }
        | Event::ToolExecutionUpdate { .. }
        | Event::EntryAppended { .. }
        | Event::ExtensionUiRequest { .. }
        | Event::Other => vec![],
    }
}

/// Shared resource registry for both pi-schema worker adapters. Stream senders belong to the
/// same admitted resource as the child; a subscription alone never owns process publication.
struct RpcWorker {
    child: Arc<RpcChild>,
    stream: broadcast::Sender<AgentEvent>,
}
struct RpcSessions {
    state: Mutex<crate::worker_lifetime::State<Arc<RpcWorker>>>,
    changed: tokio::sync::Notify,
}

#[derive(Default)]
struct OwnedWorkers(Vec<Arc<RpcChild>>);
impl OwnedWorkers {
    fn own(&mut self, child: Arc<RpcChild>) {
        if !self.0.iter().any(|held| Arc::ptr_eq(held, &child)) {
            self.0.push(child);
        }
    }
    async fn close(&mut self) {
        while let Some(child) = self.0.last().cloned() {
            child.shutdown().await;
            self.0.pop();
        }
    }
}
impl Drop for OwnedWorkers {
    fn drop(&mut self) {
        for child in self.0.drain(..) {
            child.retire_owned();
        }
    }
}
struct WorkerBoot {
    sessions: Arc<RpcSessions>,
    id: String,
    ticket: crate::worker_lifetime::Ticket,
    owned: OwnedWorkers,
    armed: bool,
}
impl Drop for WorkerBoot {
    fn drop(&mut self) {
        use crate::worker_lifetime::Event;
        if !self.armed {
            return;
        }
        if let Ok(mut state) = self.sessions.state.try_lock() {
            if let crate::worker_lifetime::Action::Retired(resource) =
                state.transition(Event::Abort {
                    id: self.id.clone(),
                    ticket: self.ticket,
                })
            {
                drop(resource);
            }
            self.sessions.changed.notify_waiters();
        } else if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let sessions = self.sessions.clone();
            let id = self.id.clone();
            let ticket = self.ticket;
            runtime.spawn(async move {
                if let crate::worker_lifetime::Action::Retired(resource) = sessions
                    .state
                    .lock()
                    .await
                    .transition(Event::Abort { id, ticket })
                {
                    drop(resource);
                }
                sessions.changed.notify_waiters();
            });
        }
    }
}
struct WorkerStop {
    sessions: Arc<RpcSessions>,
    id: String,
    ticket: crate::worker_lifetime::Ticket,
    owned: Option<OwnedWorkers>,
}
impl WorkerStop {
    async fn finish(mut self) {
        if let Some(owned) = &mut self.owned {
            owned.close().await;
        }
        self.sessions.stopped(&self.id, self.ticket).await;
        self.owned.take();
    }
}
impl Drop for WorkerStop {
    fn drop(&mut self) {
        let Some(mut owned) = self.owned.take() else {
            return;
        };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let sessions = self.sessions.clone();
            let id = self.id.clone();
            let ticket = self.ticket;
            runtime.spawn(async move {
                owned.close().await;
                sessions.stopped(&id, ticket).await;
            });
        }
        // Without a runtime, OwnedWorkers falls back to captured-child retirement. A stopped
        // runtime cannot admit a new worker or promise a synchronous reap.
    }
}
impl RpcSessions {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(Default::default()),
            changed: tokio::sync::Notify::new(),
        })
    }
    async fn begin(self: &Arc<Self>, id: &str) -> Result<WorkerBoot> {
        use crate::worker_lifetime::{Action, Event};
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let action = self
                .state
                .lock()
                .await
                .transition(Event::Begin(id.to_owned()));
            match action {
                Action::Begun { ticket, retired } => {
                    let mut owned = OwnedWorkers::default();
                    if let Some(resource) = retired {
                        owned.own(resource.child.clone());
                    }
                    return Ok(WorkerBoot {
                        sessions: self.clone(),
                        id: id.to_owned(),
                        ticket,
                        owned,
                        armed: true,
                    });
                }
                Action::Wait => changed.await,
                Action::Closed(resources) => {
                    for resource in resources {
                        resource.child.clone().retire_owned();
                    }
                    return Err(anyhow!("worker adapter is shut down"));
                }
                _ => unreachable!("Begin returns admission, wait or terminal refusal"),
            }
        }
    }
    async fn capture(&self, boot: &mut WorkerBoot, child: Arc<RpcChild>) -> Result<()> {
        use crate::worker_lifetime::{Action, Event};
        // Capture before the first cancellable registry/boot await.
        boot.owned.own(child.clone());
        let (stream, _) = broadcast::channel(4096);
        let resource = Arc::new(RpcWorker { child, stream });
        let action = self.state.lock().await.transition(Event::Capture {
            id: boot.id.clone(),
            ticket: boot.ticket,
            resource: resource.clone(),
        });
        match action {
            Action::Accepted => Ok(()),
            Action::Rejected(refused) => {
                drop(refused);
                Err(anyhow!("worker admission was retired"))
            }
            _ => Err(anyhow!("worker admission was retired")),
        }
    }
    async fn publish(
        &self,
        boot: &mut WorkerBoot,
        settled_is_agent_end: bool,
        record: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        use crate::worker_lifetime::{Action, Event};
        let mut state = self.state.lock().await;
        if !state
            .entry(&boot.id)
            .is_some_and(|entry| entry.starting(boot.ticket))
        {
            return Err(anyhow!("worker admission was retired"));
        }
        record()?;
        if !matches!(
            state.transition(Event::Publish {
                id: boot.id.clone(),
                ticket: boot.ticket
            }),
            Action::Accepted
        ) {
            return Err(anyhow!("worker admission was retired"));
        }
        let resource = state
            .entry(&boot.id)
            .and_then(|entry| entry.ready())
            .expect("accepted Publish has a resource")
            .clone();
        boot.owned.0.clear();
        boot.armed = false;
        drop(state);
        let mut events = resource.child.subscribe();
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(e) => {
                        if settled_is_agent_end {
                            if let arbos_core::rpc::Event::AgentEnd { will_retry, .. } = &e {
                                if !*will_retry {
                                    let _ = resource.stream.send(AgentEvent::Done {
                                        aborted: false,
                                        error: None,
                                    });
                                }
                                continue;
                            }
                        }
                        for event in translate_rpc_event(&e) {
                            let _ = resource.stream.send(event);
                        }
                        if matches!(e, arbos_core::rpc::Event::Other) && !resource.child.is_alive()
                        {
                            let _ = resource.stream.send(AgentEvent::Done {
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
        Ok(())
    }
    async fn child(&self, id: &str) -> Result<Arc<RpcChild>> {
        self.state
            .lock()
            .await
            .entry(id)
            .and_then(|entry| entry.ready())
            .map(|resource| resource.child.clone())
            .ok_or_else(|| anyhow!("session {id} is not open"))
    }
    fn stream(&self, id: &str) -> Option<broadcast::Receiver<AgentEvent>> {
        self.state
            .try_lock()
            .ok()?
            .entry(id)?
            .ready()
            .map(|resource| resource.stream.subscribe())
    }
    fn is_open(&self, id: &str) -> Option<bool> {
        let state = self.state.try_lock().ok()?;
        if state.entry(id).is_some_and(|entry| entry.stopping()) {
            return None;
        }
        Some(
            state
                .entry(id)
                .and_then(|entry| entry.ready())
                .is_some_and(|resource| resource.child.is_alive()),
        )
    }
    async fn pgid(&self, id: &str) -> Option<u32> {
        self.child(id).await.ok()?.tree().pgid
    }
    fn retained(&self) -> Vec<RetainedWorker> {
        let Ok(state) = self.state.try_lock() else {
            return Vec::new();
        };
        state
            .ready()
            .filter(|(_, resource)| resource.child.is_alive())
            .map(|(id, resource)| RetainedWorker {
                session_id: id.to_owned(),
                pid: resource.child.pid(),
                idle_ms: resource.child.idle_ms(),
            })
            .collect()
    }
    async fn stopped(&self, id: &str, ticket: crate::worker_lifetime::Ticket) {
        let _ = self
            .state
            .lock()
            .await
            .transition(crate::worker_lifetime::Event::Stopped {
                id: id.to_owned(),
                ticket,
            });
        self.changed.notify_waiters();
    }
    async fn stop(self: &Arc<Self>, id: &str, min_idle_ms: Option<u64>) -> bool {
        use crate::worker_lifetime::{Action, Entry, Event};
        let mut waiting_for = None;
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let action = {
                let mut state = self.state.lock().await;
                // A stop that observed A's cleanup never gains authority over a later B.
                if let Some(waited) = waiting_for {
                    if !matches!(state.entry(id), Some(Entry::Stopping {ticket,..}) if *ticket == waited)
                    {
                        return true;
                    }
                }
                if let Some(min) = min_idle_ms {
                    if !state
                        .entry(id)
                        .and_then(|entry| entry.ready())
                        .is_some_and(|resource| resource.child.idle_ms() >= min)
                    {
                        return false;
                    }
                }
                if let Some(Entry::Stopping { ticket, .. }) = state.entry(id) {
                    waiting_for = Some(*ticket);
                }
                state.transition(Event::Stop(id.to_owned()))
            };
            match action {
                Action::Stopping { ticket, resource } => {
                    let mut owned = OwnedWorkers::default();
                    if let Some(resource) = resource {
                        owned.own(resource.child.clone());
                    }
                    WorkerStop {
                        sessions: self.clone(),
                        id: id.to_owned(),
                        ticket,
                        owned: Some(owned),
                    }
                    .finish()
                    .await;
                    return true;
                }
                Action::Wait => changed.await,
                Action::Missing | Action::Closed(_) => return false,
                _ => unreachable!("Stop returns cleanup, wait or absence"),
            }
        }
    }
    async fn shutdown(&self) {
        let action = self
            .state
            .lock()
            .await
            .transition(crate::worker_lifetime::Event::Shutdown);
        self.changed.notify_waiters();
        let mut owned = OwnedWorkers::default();
        if let crate::worker_lifetime::Action::Closed(resources) = action {
            for resource in resources {
                owned.own(resource.child.clone());
            }
        }
        owned.close().await;
    }
    async fn stop_if_idle(self: &Arc<Self>, id: &str, min_idle_ms: u64) -> bool {
        self.stop(id, Some(min_idle_ms)).await
    }
    #[cfg(test)]
    async fn attach(self: &Arc<Self>, id: &str, child: Arc<RpcChild>, settled_is_agent_end: bool) {
        let mut boot = match self.begin(id).await {
            Ok(boot) => boot,
            Err(_) => {
                child.retire_owned();
                return;
            }
        };
        boot.owned.close().await;
        if self.capture(&mut boot, child).await.is_ok() {
            let _ = self
                .publish(&mut boot, settled_is_agent_end, || Ok(()))
                .await;
        }
    }
    #[cfg(test)]
    async fn test_age_idle_clock(&self, id: &str, ms: u64) -> Option<()> {
        self.state
            .lock()
            .await
            .entry(id)?
            .ready()
            .map(|resource| resource.child.test_age_idle_clock(ms))
    }
    #[cfg(test)]
    async fn test_hold_children_until(
        &self,
        ready: tokio::sync::oneshot::Sender<()>,
        release: tokio::sync::oneshot::Receiver<()>,
    ) {
        let _held = self.state.lock().await;
        let _ = ready.send(());
        let _ = release.await;
    }
}

/// The model a session that is coming back runs on: the one it was started with when the store
/// knows it, else the model this agent's work runs on now. Never the box boot value: an agent
/// whose model was changed must not resume its workers on the old one (Jacob, 2026-11-04).
async fn resume_model(store: &Store, models: &LiveModels, session_id: &str) -> String {
    match store
        .session_model(session_id)
        .ok()
        .flatten()
        .filter(|m| !m.trim().is_empty())
    {
        Some(m) => m,
        None => {
            models
                .resolve(ModelRole::Worker {
                    job_kind: None,
                    research: None,
                })
                .await
        }
    }
}

// ---- PrimeAdapter ------------------------------------------------------------------------------

pub struct PrimeAdapter {
    cfg: Arc<Config>,
    store: Store,
    /// The agent's live model slots: a child is spawned with the model the agent has now, not
    /// with the one the box booted with (Jacob, 2026-11-04).
    models: LiveModels,
    sessions: Arc<RpcSessions>,
}

impl PrimeAdapter {
    pub fn new(cfg: Arc<Config>, store: Store, models: LiveModels) -> Self {
        Self {
            cfg,
            store,
            models,
            sessions: RpcSessions::new(),
        }
    }

    async fn envs(&self, session_id: &str) -> Vec<(String, String)> {
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
            (
                "ARBOS_TALKING_MODEL".into(),
                self.models.resolve(ModelRole::Talking).await,
            ),
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
            &worker_name(Harness::Prime.as_str(), session_id),
            &self.cfg.prime_bin,
            &args,
            &self.envs(session_id).await,
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
        let mut boot = self.sessions.begin(&req.session_id).await?;
        boot.owned.close().await;
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
        self.sessions.capture(&mut boot, child.clone()).await?;
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
        self.sessions
            .publish(&mut boot, true, || {
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
                Ok(())
            })
            .await?;

        Ok(())
    }

    async fn resume(&self, session_id: &str) -> Result<()> {
        // `unwrap_or(false)`, so a lock this could not take still means "resume it" — exactly
        // what it meant before. Deliberately unchanged: this is not a kill decision, and the
        // other reading (assume open, skip the resume) would leave a session unresumed.
        if self.is_open(session_id).unwrap_or(false) {
            return Ok(());
        }
        let mut boot = self.sessions.begin(session_id).await?;
        boot.owned.close().await;
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
            resume_model(&self.store, &self.models, session_id).await,
        ];
        let child = self.spawn(session_id, &extra, &self.cfg.work_dir).await?;
        self.sessions.capture(&mut boot, child).await?;
        self.sessions.publish(&mut boot, true, || Ok(())).await?;
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
        self.sessions.stream(session_id)
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

    /// Reusable stop retains unknown liveness until its owned cleanup completes.
    async fn stop(&self, session_id: &str) -> Result<()> {
        self.sessions.stop(session_id, None).await;
        Ok(())
    }

    async fn shutdown_all(&self) {
        self.sessions.shutdown().await;
    }

    fn is_open(&self, session_id: &str) -> Option<bool> {
        self.sessions.is_open(session_id)
    }

    async fn child_pgid(&self, session_id: &str) -> Option<u32> {
        self.sessions.pgid(session_id).await
    }

    fn retained(&self) -> Vec<RetainedWorker> {
        self.sessions.retained()
    }

    async fn stop_if_idle(&self, session_id: &str, min_idle_ms: u64) -> bool {
        self.sessions.stop_if_idle(session_id, min_idle_ms).await
    }
}

// ---- PiAdapter ---------------------------------------------------------------------------------

pub struct PiAdapter {
    cfg: Arc<Config>,
    store: Store,
    models: LiveModels,
    sessions: Arc<RpcSessions>,
}

impl PiAdapter {
    pub fn new(cfg: Arc<Config>, store: Store, models: LiveModels) -> Self {
        Self {
            cfg,
            store,
            models,
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
            &worker_name(Harness::Pi.as_str(), session_id),
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
        let mut boot = self.sessions.begin(&req.session_id).await?;
        boot.owned.close().await;
        let extra = vec!["--session-id".into(), req.session_id.clone()];
        let model = if req.model.trim().is_empty() {
            self.models
                .resolve(ModelRole::Worker {
                    job_kind: None,
                    research: None,
                })
                .await
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
        self.sessions.capture(&mut boot, child.clone()).await?;
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
        self.sessions
            .publish(&mut boot, false, || {
                self.store.upsert_session(
                    &req.session_id,
                    Some(&req.session_id),
                    path.as_deref(),
                    &req.cwd.display().to_string(),
                    &req.model,
                    "pi",
                    "subagent",
                )?;
                Ok(())
            })
            .await?;

        Ok(())
    }

    async fn resume(&self, session_id: &str) -> Result<()> {
        // `unwrap_or(false)`, so a lock this could not take still means "resume it" — exactly
        // what it meant before. Deliberately unchanged: this is not a kill decision, and the
        // other reading (assume open, skip the resume) would leave a session unresumed.
        if self.is_open(session_id).unwrap_or(false) {
            return Ok(());
        }
        let mut boot = self.sessions.begin(session_id).await?;
        boot.owned.close().await;
        let extra = vec!["--session".into(), session_id.to_owned()];
        let pack = std::fs::read_to_string(
            self.cfg
                .state_dir
                .join("packs")
                .join(format!("{session_id}.md")),
        )
        .unwrap_or_default();
        let model = resume_model(&self.store, &self.models, session_id).await;
        let child = self
            .spawn(session_id, &extra, &self.cfg.work_dir, &pack, &model)
            .await?;
        self.sessions.capture(&mut boot, child).await?;
        self.sessions.publish(&mut boot, false, || Ok(())).await?;
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
        self.sessions.stream(session_id)
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

    /// Reusable stop retains unknown liveness until its owned cleanup completes.
    async fn stop(&self, session_id: &str) -> Result<()> {
        self.sessions.stop(session_id, None).await;
        Ok(())
    }

    async fn shutdown_all(&self) {
        self.sessions.shutdown().await;
    }

    fn is_open(&self, session_id: &str) -> Option<bool> {
        self.sessions.is_open(session_id)
    }

    async fn child_pgid(&self, session_id: &str) -> Option<u32> {
        self.sessions.pgid(session_id).await
    }

    fn retained(&self) -> Vec<RetainedWorker> {
        self.sessions.retained()
    }

    async fn stop_if_idle(&self, session_id: &str, min_idle_ms: u64) -> bool {
        self.sessions.stop_if_idle(session_id, min_idle_ms).await
    }

    #[cfg(test)]
    async fn test_attach_child(&self, session_id: &str, child: Arc<RpcChild>) {
        self.sessions.attach(session_id, child, false).await;
    }

    #[cfg(test)]
    async fn test_age_idle_clock(&self, session_id: &str, ms: u64) -> Option<()> {
        self.sessions.test_age_idle_clock(session_id, ms).await
    }

    #[cfg(test)]
    async fn test_hold_process_until(
        &self,
        session_id: &str,
        ready: tokio::sync::oneshot::Sender<()>,
        release: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<()> {
        let child = self.sessions.child(session_id).await?;
        child.test_hold_process_until(ready, release).await;
        Ok(())
    }

    #[cfg(test)]
    async fn test_hold_children_until(
        &self,
        ready: tokio::sync::oneshot::Sender<()>,
        release: tokio::sync::oneshot::Receiver<()>,
    ) {
        self.sessions.test_hold_children_until(ready, release).await
    }
}

// ---- ClaudeCodeAdapter -------------------------------------------------------------------------

/// Headless `claude -p --output-format stream-json --input-format stream-json` child per session
/// (design §4.4.2; sandbox-image.md §2.3). Provider = the relay's Anthropic passthrough through the
/// loopback stub (`ANTHROPIC_BASE_URL`), so no Anthropic key enters the box.
/// Claude has a different wire protocol and immediate-kill policy, but uses the same
/// actual ticket/state transition boundary as the RPC adapters.
struct ClaudeWorker {
    child: Mutex<Option<tokio::process::Child>>,
    stdin: Mutex<Option<tokio::process::ChildStdin>>,
    stream: broadcast::Sender<AgentEvent>,
}
impl ClaudeWorker {
    async fn close(&self) {
        // A backpressured write owns stdin: kill must never wait for that writer.
        if let Some(mut child) = self.child.lock().await.take() {
            let _ = child.kill().await;
        }
        self.stdin.lock().await.take();
    }
    fn retire_owned(self: Arc<Self>) {
        struct Cleanup(Option<Arc<ClaudeWorker>>);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                if let Some(resource) = self.0.take() {
                    if let Ok(mut child) = resource.child.try_lock() {
                        if let Some(mut child) = child.take() {
                            let _ = child.start_kill();
                        }
                    };
                    if let Ok(mut stdin) = resource.stdin.try_lock() {
                        stdin.take();
                    };
                }
            }
        }
        let cleanup = Cleanup(Some(self));
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let mut cleanup = cleanup;
                if let Some(resource) = &cleanup.0 {
                    resource.close().await;
                }
                cleanup.0.take();
            });
        }
        // With no runtime, Cleanup uses only the captured handle. Reaping after runtime
        // teardown remains best effort; no ambient PID is turned into authority.
    }
}
#[derive(Default)]
struct OwnedClaudeWorkers(Vec<Arc<ClaudeWorker>>);
impl OwnedClaudeWorkers {
    fn own(&mut self, resource: Arc<ClaudeWorker>) {
        if !self.0.iter().any(|held| Arc::ptr_eq(held, &resource)) {
            self.0.push(resource);
        }
    }
    async fn close(&mut self) {
        while let Some(resource) = self.0.last().cloned() {
            resource.close().await;
            self.0.pop();
        }
    }
}
impl Drop for OwnedClaudeWorkers {
    fn drop(&mut self) {
        for resource in self.0.drain(..) {
            resource.retire_owned();
        }
    }
}
struct ClaudeSessions {
    state: Mutex<crate::worker_lifetime::State<Arc<ClaudeWorker>>>,
    changed: tokio::sync::Notify,
}
struct ClaudeBoot {
    sessions: Arc<ClaudeSessions>,
    id: String,
    ticket: crate::worker_lifetime::Ticket,
    owned: OwnedClaudeWorkers,
    armed: bool,
}
impl Drop for ClaudeBoot {
    fn drop(&mut self) {
        use crate::worker_lifetime::{Action, Event};
        if !self.armed {
            return;
        }
        if let Ok(mut state) = self.sessions.state.try_lock() {
            if let Action::Retired(resource) = state.transition(Event::Abort {
                id: self.id.clone(),
                ticket: self.ticket,
            }) {
                drop(resource);
            }
            self.sessions.changed.notify_waiters();
        } else if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let sessions = self.sessions.clone();
            let id = self.id.clone();
            let ticket = self.ticket;
            runtime.spawn(async move {
                if let Action::Retired(resource) = sessions
                    .state
                    .lock()
                    .await
                    .transition(Event::Abort { id, ticket })
                {
                    drop(resource);
                }
                sessions.changed.notify_waiters();
            });
        }
    }
}
struct ClaudeStop {
    sessions: Arc<ClaudeSessions>,
    id: String,
    ticket: crate::worker_lifetime::Ticket,
    owned: Option<OwnedClaudeWorkers>,
}
impl ClaudeStop {
    async fn finish(mut self) {
        if let Some(owned) = &mut self.owned {
            owned.close().await;
        }
        self.sessions.stopped(&self.id, self.ticket).await;
        self.owned.take();
    }
}
impl Drop for ClaudeStop {
    fn drop(&mut self) {
        let Some(mut owned) = self.owned.take() else {
            return;
        };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let sessions = self.sessions.clone();
            let id = self.id.clone();
            let ticket = self.ticket;
            runtime.spawn(async move {
                owned.close().await;
                sessions.stopped(&id, ticket).await;
            });
        }
    }
}
impl ClaudeSessions {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(Default::default()),
            changed: tokio::sync::Notify::new(),
        })
    }
    async fn begin(self: &Arc<Self>, id: &str) -> Result<ClaudeBoot> {
        use crate::worker_lifetime::{Action, Event};
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let action = self
                .state
                .lock()
                .await
                .transition(Event::Begin(id.to_owned()));
            match action {
                Action::Begun { ticket, retired } => {
                    let mut owned = OwnedClaudeWorkers::default();
                    if let Some(resource) = retired {
                        owned.own(resource);
                    }
                    return Ok(ClaudeBoot {
                        sessions: self.clone(),
                        id: id.to_owned(),
                        ticket,
                        owned,
                        armed: true,
                    });
                }
                Action::Wait => changed.await,
                Action::Closed(resources) => {
                    for resource in resources {
                        resource.retire_owned();
                    }
                    return Err(anyhow!("worker adapter is shut down"));
                }
                _ => unreachable!("Begin returns admission, wait or terminal refusal"),
            }
        }
    }
    async fn capture(&self, boot: &mut ClaudeBoot, resource: Arc<ClaudeWorker>) -> Result<()> {
        use crate::worker_lifetime::{Action, Event};
        boot.owned.own(resource.clone());
        match self.state.lock().await.transition(Event::Capture {
            id: boot.id.clone(),
            ticket: boot.ticket,
            resource,
        }) {
            Action::Accepted => Ok(()),
            Action::Rejected(resource) => {
                drop(resource);
                Err(anyhow!("worker admission was retired"))
            }
            _ => Err(anyhow!("worker admission was retired")),
        }
    }
    async fn publish(
        &self,
        boot: &mut ClaudeBoot,
        record: impl FnOnce() -> Result<()>,
    ) -> Result<Arc<ClaudeWorker>> {
        use crate::worker_lifetime::{Action, Event};
        let mut state = self.state.lock().await;
        if !state
            .entry(&boot.id)
            .is_some_and(|entry| entry.starting(boot.ticket))
        {
            return Err(anyhow!("worker admission was retired"));
        }
        record()?;
        if !matches!(
            state.transition(Event::Publish {
                id: boot.id.clone(),
                ticket: boot.ticket
            }),
            Action::Accepted
        ) {
            return Err(anyhow!("worker admission was retired"));
        }
        let resource = state
            .entry(&boot.id)
            .and_then(|entry| entry.ready())
            .expect("accepted Publish has a resource")
            .clone();
        boot.owned.0.clear();
        boot.armed = false;
        Ok(resource)
    }
    async fn resource(&self, id: &str) -> Result<Arc<ClaudeWorker>> {
        self.state
            .lock()
            .await
            .entry(id)
            .and_then(|entry| entry.ready())
            .cloned()
            .ok_or_else(|| anyhow!("session {id} is not open"))
    }
    fn stream(&self, id: &str) -> Option<broadcast::Receiver<AgentEvent>> {
        self.state
            .try_lock()
            .ok()?
            .entry(id)?
            .ready()
            .map(|resource| resource.stream.subscribe())
    }
    fn is_open(&self, id: &str) -> Option<bool> {
        let state = self.state.try_lock().ok()?;
        if state.entry(id).is_some_and(|entry| entry.stopping()) {
            return None;
        }
        Some(state.entry(id).and_then(|entry| entry.ready()).is_some())
    }
    async fn stopped(&self, id: &str, ticket: crate::worker_lifetime::Ticket) {
        let _ = self
            .state
            .lock()
            .await
            .transition(crate::worker_lifetime::Event::Stopped {
                id: id.to_owned(),
                ticket,
            });
        self.changed.notify_waiters();
    }
    async fn stop(self: &Arc<Self>, id: &str) {
        use crate::worker_lifetime::{Action, Entry, Event};
        let mut waiting_for = None;
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let action = {
                let mut state = self.state.lock().await;
                if let Some(waited) = waiting_for {
                    if !matches!(state.entry(id),Some(Entry::Stopping{ticket,..})if *ticket==waited)
                    {
                        return;
                    }
                }
                if let Some(Entry::Stopping { ticket, .. }) = state.entry(id) {
                    waiting_for = Some(*ticket);
                }
                state.transition(Event::Stop(id.to_owned()))
            };
            match action {
                Action::Stopping { ticket, resource } => {
                    let mut owned = OwnedClaudeWorkers::default();
                    if let Some(resource) = resource {
                        owned.own(resource);
                    }
                    ClaudeStop {
                        sessions: self.clone(),
                        id: id.to_owned(),
                        ticket,
                        owned: Some(owned),
                    }
                    .finish()
                    .await;
                    return;
                }
                Action::Wait => changed.await,
                Action::Missing | Action::Closed(_) => return,
                _ => unreachable!("Stop returns cleanup, wait or absence"),
            }
        }
    }
    async fn shutdown(&self) {
        let action = self
            .state
            .lock()
            .await
            .transition(crate::worker_lifetime::Event::Shutdown);
        self.changed.notify_waiters();
        let mut owned = OwnedClaudeWorkers::default();
        if let crate::worker_lifetime::Action::Closed(resources) = action {
            for resource in resources {
                owned.own(resource);
            }
        }
        owned.close().await;
    }
}

pub struct ClaudeCodeAdapter {
    cfg: Arc<Config>,
    store: Store,
    models: LiveModels,
    sessions: Arc<ClaudeSessions>,
}

impl ClaudeCodeAdapter {
    pub fn new(cfg: Arc<Config>, store: Store, models: LiveModels) -> Self {
        Self {
            cfg,
            store,
            models,
            sessions: ClaudeSessions::new(),
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
        let mut boot = self.sessions.begin(session_id).await?;
        boot.owned.close().await;
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
        let model = resume_model(&self.store, &self.models, session_id).await;
        let mut cmd = tokio::process::Command::new(&self.cfg.claude_bin);
        cmd.args(&args)
            .current_dir(cwd)
            .env("CLAUDE_CONFIG_DIR", self.cfg.state_dir.join("claude"))
            .env("ANTHROPIC_BASE_URL", self.cfg.proxy_base())
            .env("ANTHROPIC_AUTH_TOKEN", &self.cfg.box_token)
            // The relay's Anthropic passthrough may sit on OpenRouter, whose ids are `anthropic/<model>`.
            // Claude Code's own cheap lane is pointed at the same model: a worker of an agent set
            // to one model never quietly drops to another (Jacob, 2026-11-04).
            .env("ANTHROPIC_MODEL", &model)
            .env("ANTHROPIC_DEFAULT_SONNET_MODEL", &model)
            .env("ANTHROPIC_DEFAULT_HAIKU_MODEL", &model)
            .env("ARBOS_MCP_TOKEN", self.cfg.subagent_mcp_token())
            .env("ARBOS_AUDIT_PATH", self.cfg.audit_path())
            .env("DISABLE_AUTOUPDATER", "1")
            .env("DISABLE_TELEMETRY", "1")
            .env("DISABLE_ERROR_REPORTING", "1")
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
            .envs(crate::vault::shell_envs(&self.cfg))
            .envs(self.cfg.browser_env())
            .kill_on_drop(true)
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
        let resource = Arc::new(ClaudeWorker {
            child: Mutex::new(Some(child)),
            stdin: Mutex::new(Some(stdin)),
            stream: tx,
        });
        self.sessions.capture(&mut boot, resource).await?;
        let resource = self
            .sessions
            .publish(&mut boot, || {
                self.store.upsert_session(
                    session_id,
                    Some(session_id),
                    None,
                    &cwd.display().to_string(),
                    &model,
                    "claude_code",
                    "subagent",
                )?;
                Ok(())
            })
            .await?;
        let tx = resource.stream.clone();
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
        let resource = self.sessions.resource(session_id).await?;
        let msg = serde_json::json!({ "type": "user", "message": { "role": "user", "content": [{ "type": "text", "text": text }] } });
        let mut stdin = resource.stdin.lock().await;
        let s = stdin
            .as_mut()
            .ok_or_else(|| anyhow!("session {session_id} is not open"))?;
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
        // `unwrap_or(false)`, so a lock this could not take still means "resume it" — exactly
        // what it meant before. Deliberately unchanged: this is not a kill decision, and the
        // other reading (assume open, skip the resume) would leave a session unresumed.
        if self.is_open(session_id).unwrap_or(false) {
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
        self.sessions.stream(session_id)
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
        self.sessions.stop(session_id).await;
        Ok(())
    }

    async fn shutdown_all(&self) {
        self.sessions.shutdown().await;
    }

    fn is_open(&self, session_id: &str) -> Option<bool> {
        self.sessions.is_open(session_id)
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

#[cfg(test)]
mod worker_owner_regressions {
    use super::*;
    use std::future::Future;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    const SESSION: &str = "01WORKEROWNERSHIPFIXTURE00";
    const STUB: &str = r#"#!/usr/bin/env python3
import json, select, sys, time
from pathlib import Path
marker, release, exited, finish = Path(@MARKER@), Path(@RELEASE@), Path(@EXITED@), Path(@FINISH@)
try:
    while not finish.exists():
        if not select.select([sys.stdin], [], [], 0.01)[0]:
            continue
        line = sys.stdin.readline()
        if not line:
            if (marker.parent / "pause-close").exists():
                (marker.parent / "closing").write_text("closing")
                while not (marker.parent / "close-release").exists():
                    time.sleep(0.005)
            break
        command = json.loads(line)
        if command.get("type") == "get_state":
            marker.write_text("waiting")
            while not release.exists() and not finish.exists():
                if select.select([sys.stdin], [], [], 0.01)[0] and not sys.stdin.readline():
                    sys.exit(0)
            if finish.exists():
                break
        print(json.dumps({"type":"response", "command":command.get("type"), "id":command.get("id"), "success":True, "data":{}}), flush=True)
finally:
    exited.write_text("exited")
"#;

    struct Fixture {
        adapter: Arc<dyn HarnessAdapter>,
        store: Store,
        root: PathBuf,
        agent_root: PathBuf,
        marker: PathBuf,
        release: PathBuf,
        exited: PathBuf,
        finish: PathBuf,
    }
    impl Fixture {
        async fn new(prime: bool) -> Self {
            let root = std::env::temp_dir()
                .join(format!("arbos-worker-owned-{}", arbos_core::ids::ulid()));
            std::fs::create_dir_all(&root).unwrap();
            let script = root.join("worker.py");
            let marker = root.join("marker");
            let release = root.join("release");
            let exited = root.join("exited");
            let finish = root.join("finish");
            std::fs::write(
                &script,
                STUB.replace("@MARKER@", &format!("{marker:?}"))
                    .replace("@RELEASE@", &format!("{release:?}"))
                    .replace("@EXITED@", &format!("{exited:?}"))
                    .replace("@FINISH@", &format!("{finish:?}")),
            )
            .unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
            let (agent, agent_root) =
                crate::status::tests::test_agent_with_pi(script.to_str().unwrap()).await;
            let mut cfg = (*agent.cfg).clone();
            cfg.prime_bin = script.display().to_string();
            std::fs::create_dir_all(&cfg.work_dir).unwrap();
            let models = LiveModels::new(&cfg);
            let cfg = Arc::new(cfg);
            let adapter: Arc<dyn HarnessAdapter> = if prime {
                Arc::new(PrimeAdapter::new(cfg, agent.store.clone(), models))
            } else {
                Arc::new(PiAdapter::new(cfg, agent.store.clone(), models))
            };
            Self {
                adapter,
                store: agent.store.clone(),
                root,
                agent_root,
                marker,
                release,
                exited,
                finish,
            }
        }
        fn start(&self) -> tokio::task::JoinHandle<Result<()>> {
            let adapter = self.adapter.clone();
            let cwd = self.root.clone();
            tokio::spawn(async move {
                adapter
                    .start(StartRequest {
                        session_id: SESSION.into(),
                        cwd,
                        system_prompt: "synthetic fixture".into(),
                        model: "mock-sub".into(),
                        thinking: "off".into(),
                    })
                    .await
            })
        }
        async fn wait(path: &std::path::Path, bound: Duration) -> bool {
            tokio::time::timeout(bound, async {
                while !path.exists() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .is_ok()
        }
        async fn reached(&self) {
            assert!(
                Self::wait(&self.marker, Duration::from_secs(5)).await,
                "actual worker never reached its RPC barrier"
            );
        }
        fn release(&self) {
            std::fs::write(&self.release, "release").unwrap();
        }
        async fn cleanup(&self) {
            self.adapter.shutdown_all().await;
            std::fs::write(&self.finish, "fixture cleanup").unwrap();
            if self.marker.exists() {
                assert!(
                    Self::wait(&self.exited, Duration::from_secs(5)).await,
                    "owned fixture child did not exit during cleanup"
                );
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
            let _ = std::fs::remove_dir_all(&self.agent_root);
        }
    }

    async fn cancelled_boot(prime: bool) {
        let f = Fixture::new(prime).await;
        let task = f.start();
        f.reached().await;
        task.abort();
        let _ = task.await;
        let cleaned = Fixture::wait(&f.exited, Duration::from_secs(1)).await;
        f.cleanup().await;
        assert!(
            cleaned,
            "cancelled real adapter boot retained an unpublished RPC child"
        );
    }
    async fn late_attach(prime: bool, all: bool) {
        let f = Fixture::new(prime).await;
        let task = f.start();
        f.reached().await;
        if all {
            f.adapter.shutdown_all().await;
        } else {
            f.adapter.stop(SESSION).await.unwrap();
        }
        f.release();
        let result = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        let open = f.adapter.is_open(SESSION);
        f.cleanup().await;
        assert!(
            result.is_err(),
            "retired adapter admission published a late successful start"
        );
        assert_eq!(open, Some(false));
    }
    async fn post_shutdown(prime: bool) {
        let f = Fixture::new(prime).await;
        f.adapter.shutdown_all().await;
        f.release();
        let result = tokio::time::timeout(Duration::from_secs(5), f.start())
            .await
            .unwrap()
            .unwrap();
        let open = f.adapter.is_open(SESSION);
        f.cleanup().await;
        assert!(
            result.is_err(),
            "terminal adapter shutdown accepted new worker admission"
        );
        assert_eq!(open, Some(false));
    }
    async fn ordinary_start_and_stop(prime: bool) {
        let f = Fixture::new(prime).await;
        f.release();
        f.start().await.unwrap().unwrap();
        assert_eq!(f.adapter.is_open(SESSION), Some(true));
        assert!(f.adapter.stream(SESSION).is_some());
        f.adapter.stop(SESSION).await.unwrap();
        assert_eq!(f.adapter.is_open(SESSION), Some(false));
        f.cleanup().await;
    }
    #[tokio::test]
    async fn cancelled_prime_boot_retires_its_unpublished_child() {
        cancelled_boot(true).await;
    }
    #[tokio::test]
    async fn cancelled_pi_boot_retires_its_unpublished_child() {
        cancelled_boot(false).await;
    }
    #[tokio::test]
    async fn shutdown_refuses_late_prime_attach() {
        late_attach(true, true).await;
    }
    #[tokio::test]
    async fn shutdown_refuses_late_pi_attach() {
        late_attach(false, true).await;
    }
    #[tokio::test]
    async fn stop_refuses_late_prime_attach() {
        late_attach(true, false).await;
    }
    #[tokio::test]
    async fn stop_refuses_late_pi_attach() {
        late_attach(false, false).await;
    }
    #[tokio::test]
    async fn terminal_shutdown_refuses_new_prime_start() {
        post_shutdown(true).await;
    }
    #[tokio::test]
    async fn terminal_shutdown_refuses_new_pi_start() {
        post_shutdown(false).await;
    }
    #[tokio::test]
    async fn current_prime_start_stream_and_stop_work() {
        ordinary_start_and_stop(true).await;
    }
    #[tokio::test]
    async fn current_pi_start_stream_and_stop_work() {
        ordinary_start_and_stop(false).await;
    }
    async fn failed_session_record_retires_child(prime: bool) {
        let f = Fixture::new(prime).await;
        f.store.with(|c| c.execute_batch("CREATE TRIGGER fixture_session_failure BEFORE INSERT ON sessions BEGIN SELECT RAISE(ABORT, 'synthetic session write refusal'); END")).unwrap();
        f.release();
        let result = f.start().await.unwrap();
        let closed = Fixture::wait(&f.exited, Duration::from_secs(1)).await;
        let open = f.adapter.is_open(SESSION);
        f.cleanup().await;
        assert!(result.is_err());
        assert!(
            closed,
            "a local session-write error leaked its already spawned worker"
        );
        assert_eq!(open, Some(false));
    }
    async fn cancelled_stop_releases_all_resume_waiters(prime: bool) {
        let f = Fixture::new(prime).await;
        f.release();
        f.start().await.unwrap().unwrap();
        std::fs::write(f.root.join("pause-close"), "pause").unwrap();
        let stop = {
            let adapter = f.adapter.clone();
            tokio::spawn(async move { adapter.stop(SESSION).await })
        };
        assert!(Fixture::wait(&f.root.join("closing"), Duration::from_secs(5)).await);
        assert_eq!(
            f.adapter.is_open(SESSION),
            None,
            "the cleanup owner still holds an unknown live process"
        );
        let mut waiters = Vec::new();
        for _ in 0..3 {
            let adapter = f.adapter.clone();
            waiters.push(tokio::spawn(async move { adapter.resume(SESSION).await }));
        }
        tokio::task::yield_now().await;
        waiters.pop().unwrap().abort();
        stop.abort();
        let _ = stop.await;
        // Cancelling shutdown drops its captured kill_on_drop Child. Cleanup can already
        // finish and admit a resumed owner here; the pending-stop assertion above is the fence.
        std::fs::remove_file(f.root.join("pause-close")).unwrap();
        std::fs::write(f.root.join("close-release"), "release").unwrap();
        let mut admitted = 0;
        for waiter in waiters {
            if tokio::time::timeout(Duration::from_secs(5), waiter)
                .await
                .expect("resume waiter lost its notification")
                .unwrap()
                .is_ok()
            {
                admitted += 1;
            }
        }
        let open = f.adapter.is_open(SESSION);
        f.cleanup().await;
        assert!(admitted >= 1);
        assert_eq!(open, Some(true));
    }
    #[tokio::test]
    async fn prime_record_error_cleans_unpublished_child() {
        failed_session_record_retires_child(true).await;
    }
    #[tokio::test]
    async fn pi_record_error_cleans_unpublished_child() {
        failed_session_record_retires_child(false).await;
    }
    #[tokio::test]
    async fn cancelled_prime_stop_preserves_unknown_until_cleanup_and_wakes_resumers() {
        cancelled_stop_releases_all_resume_waiters(true).await;
    }
    #[tokio::test]
    async fn cancelled_pi_stop_preserves_unknown_until_cleanup_and_wakes_resumers() {
        cancelled_stop_releases_all_resume_waiters(false).await;
    }
    async fn old_stop_waiter_preserves_resumed_owner(prime: bool) {
        let f = Fixture::new(prime).await;
        f.release();
        f.start().await.unwrap().unwrap();
        std::fs::write(f.root.join("pause-close"), "pause").unwrap();
        let first = {
            let adapter = f.adapter.clone();
            tokio::spawn(async move { adapter.stop(SESSION).await })
        };
        assert!(Fixture::wait(&f.root.join("closing"), Duration::from_secs(5)).await);
        let second = f.adapter.stop(SESSION);
        tokio::pin!(second);
        std::future::poll_fn(|context| {
            assert!(
                second.as_mut().poll(context).is_pending(),
                "second actual stop must observe pending cleanup"
            );
            std::task::Poll::Ready(())
        })
        .await;
        // Keep the admitted second caller's continuation unpolled until a normal resume owns B.
        std::fs::remove_file(f.root.join("pause-close")).unwrap();
        std::fs::write(f.root.join("close-release"), "release").unwrap();
        first.await.unwrap().unwrap();
        f.adapter.resume(SESSION).await.unwrap();
        let replacement = f.adapter.child_pgid(SESSION).await;
        assert!(replacement.is_some());
        tokio::time::timeout(Duration::from_secs(5), &mut second)
            .await
            .unwrap()
            .unwrap();
        let retained = f.adapter.child_pgid(SESSION).await;
        let open = f.adapter.is_open(SESSION);
        f.cleanup().await;
        assert_eq!(
            retained, replacement,
            "a stop admitted while A was retiring stopped later resumed B"
        );
        assert_eq!(open, Some(true));
    }
    #[tokio::test]
    async fn old_stop_waiter_cannot_retire_resumed_prime() {
        old_stop_waiter_preserves_resumed_owner(true).await;
    }
    #[tokio::test]
    async fn old_stop_waiter_cannot_retire_resumed_pi() {
        old_stop_waiter_preserves_resumed_owner(false).await;
    }
    async fn sibling_stop_keeps_ready_observable(prime: bool) {
        let f = Fixture::new(prime).await;
        f.release();
        f.start().await.unwrap().unwrap();
        let sibling = arbos_core::ids::ulid();
        f.adapter
            .start(StartRequest {
                session_id: sibling.clone(),
                cwd: f.root.clone(),
                system_prompt: "synthetic sibling".into(),
                model: "mock-sub".into(),
                thinking: "off".into(),
            })
            .await
            .unwrap();
        assert_eq!(f.adapter.is_open(&sibling), Some(true));
        std::fs::write(f.root.join("pause-close"), "pause").unwrap();
        let stopping = {
            let adapter = f.adapter.clone();
            tokio::spawn(async move { adapter.stop(SESSION).await })
        };
        assert!(Fixture::wait(&f.root.join("closing"), Duration::from_secs(5)).await);
        let own = f.adapter.is_open(SESSION);
        let observed = f.adapter.is_open(&sibling);
        std::fs::remove_file(f.root.join("pause-close")).unwrap();
        std::fs::write(f.root.join("close-release"), "release").unwrap();
        stopping.await.unwrap().unwrap();
        f.cleanup().await;
        assert_eq!(own, None, "the actual retiring owner remains unknown");
        assert_eq!(
            observed,
            Some(true),
            "an independently readable Ready sibling remains observable and protected"
        );
    }
    #[tokio::test]
    async fn stopping_one_rpc_session_keeps_ready_sibling_observable_prime() {
        sibling_stop_keeps_ready_observable(true).await;
    }
    #[tokio::test]
    async fn stopping_one_rpc_session_keeps_ready_sibling_observable_pi() {
        sibling_stop_keeps_ready_observable(false).await;
    }
}

#[cfg(test)]
mod claude_worker_owner_regressions {
    use super::*;
    use std::future::Future;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    struct ReadPause {
        reached: AtomicBool,
        armed: AtomicBool,
        release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    }
    struct Trace {
        store: Store,
        pointer: *const ReadPause,
        data: Arc<ReadPause>,
        release: std::sync::mpsc::Sender<()>,
    }
    impl Trace {
        fn install(store: &Store) -> Self {
            unsafe extern "C" fn observed(
                event: u32,
                context: *mut std::ffi::c_void,
                statement: *mut std::ffi::c_void,
                _: *mut std::ffi::c_void,
            ) -> i32 {
                if event != rusqlite::ffi::SQLITE_TRACE_PROFILE as u32 {
                    return 0;
                }
                let sql = unsafe { rusqlite::ffi::sqlite3_sql(statement.cast()) };
                if sql.is_null() {
                    return 0;
                }
                let sql = unsafe { std::ffi::CStr::from_ptr(sql) }.to_bytes();
                if !sql
                    .windows(b"SELECT model FROM sessions".len())
                    .any(|s| s == b"SELECT model FROM sessions")
                {
                    return 0;
                }
                let pause = unsafe { &*context.cast::<ReadPause>() };
                if pause.armed.swap(false, Ordering::SeqCst) {
                    pause.reached.store(true, Ordering::SeqCst);
                    let _ = pause
                        .release
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(5));
                }
                0
            }
            let (release, receiver) = std::sync::mpsc::channel();
            let data = Arc::new(ReadPause {
                reached: AtomicBool::new(false),
                armed: AtomicBool::new(true),
                release: std::sync::Mutex::new(receiver),
            });
            let pointer = Arc::into_raw(data.clone());
            store
                .with(|c| {
                    let rc = unsafe {
                        rusqlite::ffi::sqlite3_trace_v2(
                            c.handle(),
                            rusqlite::ffi::SQLITE_TRACE_PROFILE as u32,
                            Some(observed),
                            pointer.cast_mut().cast(),
                        )
                    };
                    assert_eq!(rc, rusqlite::ffi::SQLITE_OK);
                    Ok(())
                })
                .unwrap();
            Self {
                store: store.clone(),
                pointer,
                data,
                release,
            }
        }
        async fn reached(&self) {
            assert!(tokio::time::timeout(Duration::from_secs(5), async {
                while !self.data.reached.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            })
            .await
            .is_ok());
        }
        fn release(&self) {
            let _ = self.release.send(());
        }
    }
    impl Drop for Trace {
        fn drop(&mut self) {
            self.release();
            self.store
                .with(|c| {
                    unsafe {
                        rusqlite::ffi::sqlite3_trace_v2(c.handle(), 0, None, std::ptr::null_mut());
                    }
                    Ok(())
                })
                .unwrap();
            unsafe {
                drop(Arc::from_raw(self.pointer));
            }
        }
    }

    const STUB: &str = r#"#!/usr/bin/env python3
import json,os,select,sys,time
from pathlib import Path
root=Path(@ROOT@)
(root/'pid').write_text(str(os.getpid()))
while not (root/'finish').exists():
    if (root/'no-read').exists():
        time.sleep(0.01)
        continue
    if select.select([sys.stdin],[],[],0.01)[0]:
        line=sys.stdin.readline()
        if line:
            with (root/'commands').open('a') as out:out.write(line)
            print(json.dumps({'type':'stream_event','event':{'type':'content_block_delta','delta':{'type':'text_delta','text':'fixture output'}}}),flush=True)
        else:
            (root/'stdin-closed').write_text('closed')
            time.sleep(0.01)
"#;
    struct Fixture {
        adapter: Arc<ClaudeCodeAdapter>,
        store: Store,
        root: PathBuf,
        agent_root: PathBuf,
        id: String,
    }
    impl Fixture {
        async fn new() -> Self {
            let root = std::env::temp_dir()
                .join(format!("arbos-claude-owned-{}", arbos_core::ids::ulid()));
            std::fs::create_dir_all(&root).unwrap();
            let script = root.join("cli.py");
            std::fs::write(&script, STUB.replace("@ROOT@", &format!("{root:?}"))).unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
            let (agent, agent_root) = crate::status::tests::test_agent().await;
            let mut cfg = (*agent.cfg).clone();
            cfg.claude_bin = script.display().to_string();
            std::fs::create_dir_all(&cfg.work_dir).unwrap();
            let models = LiveModels::new(&cfg);
            let store = agent.store.clone();
            let adapter = Arc::new(ClaudeCodeAdapter::new(Arc::new(cfg), store.clone(), models));
            Self {
                adapter,
                store,
                root,
                agent_root,
                id: arbos_core::ids::ulid(),
            }
        }
        fn start(&self) -> tokio::task::JoinHandle<Result<()>> {
            let adapter = self.adapter.clone();
            let id = self.id.clone();
            let cwd = self.root.clone();
            tokio::spawn(async move {
                adapter
                    .start(StartRequest {
                        session_id: id,
                        cwd,
                        system_prompt: "synthetic fixture".into(),
                        model: "mock-sub".into(),
                        thinking: "off".into(),
                    })
                    .await
            })
        }
        async fn reached(&self) {
            assert!(
                tokio::time::timeout(Duration::from_secs(5), async {
                    while !self.root.join("pid").exists() {
                        tokio::time::sleep(Duration::from_millis(2)).await;
                    }
                })
                .await
                .is_ok(),
                "owned fake CLI never spawned"
            );
        }
        fn pid(&self) -> i32 {
            std::fs::read_to_string(self.root.join("pid"))
                .unwrap()
                .parse()
                .unwrap()
        }
        fn alive(pid: i32) -> bool {
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
        }
        async fn gone(pid: i32) -> bool {
            tokio::time::timeout(Duration::from_secs(2), async {
                while Self::alive(pid) {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .is_ok()
        }
        async fn cleanup(&self) {
            self.adapter.shutdown_all().await;
            std::fs::write(self.root.join("finish"), "finish").unwrap();
            if self.root.join("pid").exists() {
                assert!(
                    Self::gone(self.pid()).await,
                    "owned fake CLI was not reaped during cleanup"
                );
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
            let _ = std::fs::remove_dir_all(&self.agent_root);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancelled_claude_publication_retires_its_spawned_child() {
        let f = Fixture::new().await;
        let trace = Trace::install(&f.store);
        let task = f.start();
        trace.reached().await;
        let held = f.adapter.sessions.state.lock().await;
        trace.release();
        f.reached().await;
        let pid = f.pid();
        assert!(Fixture::alive(pid));
        task.abort();
        let _ = task.await;
        drop(held);
        let cleaned = Fixture::gone(pid).await;
        f.cleanup().await;
        assert!(
            cleaned,
            "cancelling actual Claude publication leaked its owned child"
        );
    }
    async fn retired_publication(all: bool) {
        let f = Fixture::new().await;
        let trace = Trace::install(&f.store);
        let task = f.start();
        trace.reached().await;
        let held = f.adapter.sessions.state.lock().await;
        trace.release();
        f.reached().await;
        let retire = async {
            if all {
                f.adapter.shutdown_all().await;
                Ok(())
            } else {
                f.adapter.stop(&f.id).await
            }
        };
        tokio::pin!(retire);
        let completed = std::future::poll_fn(|cx| {
            std::task::Poll::Ready(matches!(
                retire.as_mut().poll(cx),
                std::task::Poll::Ready(_)
            ))
        })
        .await;
        drop(held);
        if !completed {
            retire.await.unwrap();
        }
        let result = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        let open = f.adapter.is_open(&f.id);
        f.cleanup().await;
        assert!(
            result.is_err(),
            "retired Claude admission published a late successful child"
        );
        assert_eq!(open, Some(false));
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn shutdown_refuses_late_claude_publication() {
        retired_publication(true).await;
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stop_refuses_late_claude_publication() {
        retired_publication(false).await;
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn terminal_shutdown_refuses_new_claude_start() {
        let f = Fixture::new().await;
        f.adapter.shutdown_all().await;
        let result = f.start().await.unwrap();
        if result.is_ok() {
            f.reached().await;
        }
        let open = f.adapter.is_open(&f.id);
        f.cleanup().await;
        assert!(result.is_err());
        assert_eq!(open, Some(false));
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn ordinary_claude_start_stream_stop_and_resume_remain_usable() {
        let f = Fixture::new().await;
        f.start().await.unwrap().unwrap();
        f.reached().await;
        let first = f.pid();
        assert!(Fixture::alive(first));
        assert!(f.adapter.stream(&f.id).is_some());
        f.adapter.stop(&f.id).await.unwrap();
        assert!(Fixture::gone(first).await);
        assert_eq!(f.adapter.is_open(&f.id), Some(false));
        std::fs::remove_file(f.root.join("pid")).unwrap();
        f.adapter.resume(&f.id).await.unwrap();
        f.reached().await;
        assert_ne!(f.pid(), first);
        assert!(Fixture::alive(f.pid()));
        f.cleanup().await;
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn claude_session_record_error_retires_captured_child() {
        let f = Fixture::new().await;
        f.store.with(|c|c.execute_batch("CREATE TRIGGER fixture_session_failure BEFORE INSERT ON sessions BEGIN SELECT RAISE(ABORT,'synthetic session failure'); END")).unwrap();
        let trace = Trace::install(&f.store);
        let task = f.start();
        trace.reached().await;
        let held = f.adapter.sessions.state.lock().await;
        trace.release();
        f.reached().await;
        let pid = f.pid();
        assert!(Fixture::alive(pid));
        drop(held);
        let result = task.await.unwrap();
        let cleaned = Fixture::gone(pid).await;
        let open = f.adapter.is_open(&f.id);
        f.cleanup().await;
        assert!(result.is_err());
        assert!(cleaned);
        assert_eq!(open, Some(false));
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn old_claude_stop_waiter_cannot_retire_resumed_owner() {
        let f = Fixture::new().await;
        f.start().await.unwrap().unwrap();
        f.reached().await;
        let original = f.adapter.sessions.resource(&f.id).await.unwrap();
        let held = original.child.lock().await;
        let first = {
            let adapter = f.adapter.clone();
            let id = f.id.clone();
            tokio::spawn(async move { adapter.stop(&id).await })
        };
        assert!(tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let stopping = matches!(
                    f.adapter.sessions.state.lock().await.entry(&f.id),
                    Some(crate::worker_lifetime::Entry::Stopping { .. })
                );
                if stopping {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .is_ok());
        assert_eq!(f.adapter.is_open(&f.id), None);
        let second = f.adapter.stop(&f.id);
        tokio::pin!(second);
        std::future::poll_fn(|cx| {
            assert!(second.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        drop(held);
        first.await.unwrap().unwrap();
        std::fs::remove_file(f.root.join("pid")).unwrap();
        f.adapter.resume(&f.id).await.unwrap();
        f.reached().await;
        let replacement = f.adapter.sessions.resource(&f.id).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), &mut second)
            .await
            .unwrap()
            .unwrap();
        let retained = f.adapter.sessions.resource(&f.id).await.unwrap();
        let alive = Fixture::alive(f.pid());
        f.cleanup().await;
        assert!(Arc::ptr_eq(&replacement, &retained));
        assert!(alive);
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn current_claude_resource_preserves_actual_stdin_stdout_protocol() {
        let f = Fixture::new().await;
        f.start().await.unwrap().unwrap();
        f.reached().await;
        let mut stream = f.adapter.stream(&f.id).unwrap();
        f.adapter.prompt(&f.id, "synthetic hello").await.unwrap();
        let output = tokio::time::timeout(Duration::from_secs(2), stream.recv())
            .await
            .unwrap()
            .unwrap();
        let command: Value = serde_json::from_str(
            std::fs::read_to_string(f.root.join("commands"))
                .unwrap()
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        f.cleanup().await;
        assert!(matches!(output,AgentEvent::TextDelta(text)if text=="fixture output"));
        assert_eq!(
            command,
            serde_json::json!({"type":"user","message":{"role":"user","content":[{"type":"text","text":"synthetic hello"}]}})
        );
    }
    async fn blocked_writer_does_not_prevent_immediate_kill(all: bool) {
        let f = Fixture::new().await;
        std::fs::write(f.root.join("no-read"), "pause stdin reads").unwrap();
        f.start().await.unwrap().unwrap();
        f.reached().await;
        let pid = f.pid();
        assert!(Fixture::alive(pid));
        let blocked = Arc::new(AtomicBool::new(false));
        let observed = blocked.clone();
        let adapter = f.adapter.clone();
        let id = f.id.clone();
        let writer = tokio::spawn(async move {
            let payload = "x".repeat(2 * 1024 * 1024);
            let future = adapter.prompt(&id, &payload);
            tokio::pin!(future);
            std::future::poll_fn(|cx| {
                let result = future.as_mut().poll(cx);
                if result.is_pending() {
                    observed.store(true, Ordering::SeqCst);
                }
                result
            })
            .await
        });
        assert!(
            tokio::time::timeout(Duration::from_secs(5), async {
                while !blocked.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            })
            .await
            .is_ok(),
            "actual write did not reach pipe backpressure"
        );
        assert!(
            !writer.is_finished(),
            "the no-read fixture must leave a real blocked writer"
        );
        let completed = tokio::time::timeout(Duration::from_secs(5), async {
            if all {
                f.adapter.shutdown_all().await;
            } else {
                f.adapter.stop(&f.id).await.unwrap();
            }
        })
        .await
        .is_ok();
        // Observe the kill before manually releasing/cancelling the writer during cleanup.
        let killed_before_cleanup = !Fixture::alive(pid);
        writer.abort();
        let _ = writer.await;
        f.cleanup().await;
        assert!(
            completed,
            "immediate child kill waited for a backpressured stdin writer"
        );
        assert!(
            killed_before_cleanup,
            "retirement did not kill its captured child before writer cleanup"
        );
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn blocked_claude_writer_cannot_block_stop_kill() {
        blocked_writer_does_not_prevent_immediate_kill(false).await;
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn blocked_claude_writer_cannot_block_shutdown_kill() {
        blocked_writer_does_not_prevent_immediate_kill(true).await;
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancelled_claude_retirement_keeps_kill_ownership_with_a_blocked_writer() {
        let f = Fixture::new().await;
        std::fs::write(f.root.join("no-read"), "pause stdin reads").unwrap();
        f.start().await.unwrap().unwrap();
        f.reached().await;
        let pid = f.pid();
        let blocked = Arc::new(AtomicBool::new(false));
        let observed = blocked.clone();
        let gate = Arc::new(tokio::sync::Notify::new());
        let resume = gate.clone();
        let adapter = f.adapter.clone();
        let id = f.id.clone();
        let writer = tokio::spawn(async move {
            let payload = "x".repeat(2 * 1024 * 1024);
            let mut future = Box::pin(adapter.prompt(&id, &payload));
            std::future::poll_fn(|cx| {
                assert!(future.as_mut().poll(cx).is_pending());
                std::task::Poll::Ready(())
            })
            .await;
            observed.store(true, Ordering::SeqCst);
            resume.notified().await;
            future.await
        });
        assert!(tokio::time::timeout(Duration::from_secs(5), async {
            while !blocked.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .is_ok());
        let stopping = {
            let adapter = f.adapter.clone();
            let id = f.id.clone();
            tokio::spawn(async move { adapter.stop(&id).await })
        };
        let killed = Fixture::gone(pid).await;
        let pending = !stopping.is_finished();
        stopping.abort();
        let _ = stopping.await;
        gate.notify_one();
        if killed {
            assert!(tokio::time::timeout(Duration::from_secs(5), writer)
                .await
                .unwrap()
                .unwrap()
                .is_err());
            std::fs::remove_file(f.root.join("pid")).unwrap();
            tokio::time::timeout(Duration::from_secs(5), f.adapter.resume(&f.id))
                .await
                .unwrap()
                .unwrap();
            f.reached().await;
        } else {
            writer.abort();
            let _ = writer.await;
        }
        let reusable = f.adapter.is_open(&f.id);
        f.cleanup().await;
        assert!(
            killed,
            "cleanup must kill its captured child before waiting for the writer"
        );
        assert!(
            pending,
            "the controlled writer must keep the actual cleanup future pending before cancellation"
        );
        assert_eq!(reusable, Some(true));
    }
}

#[cfg(test)]
mod rpc_blocked_cleanup_regressions {
    use super::*;
    use arbos_core::rpc::Command;
    use std::time::Duration;

    struct Fixture {
        root: PathBuf,
        child: Arc<RpcChild>,
        eof: std::path::PathBuf,
    }
    impl Fixture {
        async fn new(reads: bool) -> Self {
            let root =
                std::env::temp_dir().join(format!("arbos-rpc-cleanup-{}", arbos_core::ids::ulid()));
            std::fs::create_dir_all(&root).unwrap();
            let ready = root.as_path().join("ready");
            let eof = root.as_path().join("eof");
            let code = r#"import os, sys, time
from pathlib import Path
Path(os.environ['FIXTURE_READY']).write_text('ready')
if os.environ['FIXTURE_READS'] == 'yes':
    sys.stdin.buffer.read()
    Path(os.environ['FIXTURE_EOF']).write_text('eof')
else:
    time.sleep(60)
"#;
            let child = RpcChild::spawn(
                "synthetic-rpc-cleanup",
                "/usr/bin/python3",
                &["-I".into(), "-u".into(), "-c".into(), code.into()],
                &[
                    ("FIXTURE_READY".into(), ready.to_string_lossy().into_owned()),
                    ("FIXTURE_EOF".into(), eof.to_string_lossy().into_owned()),
                    (
                        "FIXTURE_READS".into(),
                        if reads { "yes" } else { "no" }.into(),
                    ),
                ],
                root.as_path(),
                &root.as_path().join("child.log"),
                None,
            )
            .await
            .unwrap();
            tokio::time::timeout(Duration::from_secs(5), async {
                while !ready.exists() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("the owned synthetic child starts");
            Self { root, child, eof }
        }
        async fn blocked_writer(
            &self,
        ) -> tokio::task::JoinHandle<Result<arbos_core::rpc::Response>> {
            let child = self.child.clone();
            let writing = tokio::spawn(async move {
                child
                    .call(Command::Prompt {
                        id: "synthetic-large-prompt".into(),
                        message: "synthetic ".repeat(128 * 1024),
                        streaming_behavior: None,
                        images: Vec::new(),
                    })
                    .await
            });
            tokio::time::timeout(Duration::from_secs(5), async {
                while !self.child.test_stdin_is_locked() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("actual write_all retains the stdin lock under backpressure");
            assert!(
                !writing.is_finished(),
                "the actual writer must still be pending"
            );
            writing
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.child.clone().retire_owned();
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    async fn blocked_cleanup(all: bool) {
        let fixture = Fixture::new(false).await;
        let sessions = RpcSessions::new();
        sessions
            .attach("synthetic-session", fixture.child.clone(), false)
            .await;
        assert_eq!(sessions.is_open("synthetic-session"), Some(true));
        let mut writing = fixture.blocked_writer().await;
        let owned = sessions.clone();
        let mut closing = tokio::spawn(async move {
            if all {
                owned.shutdown().await;
            } else {
                owned.stop("synthetic-session", None).await;
            }
        });
        // The original RPC policy grants five seconds for EOF; one extra second is test slack.
        let completed = tokio::time::timeout(Duration::from_secs(6), &mut closing)
            .await
            .is_ok();
        let writer_released = if completed {
            tokio::time::timeout(Duration::from_secs(1), &mut writing)
                .await
                .is_ok()
        } else {
            false
        };
        // Always clean the owned fixture before asserting a regression, even on original code.
        if !writer_released {
            writing.abort();
            let _ = writing.await;
        }
        if !completed {
            tokio::time::timeout(Duration::from_secs(7), closing)
                .await
                .expect("fixture cleanup after aborting its writer")
                .unwrap();
        }
        assert!(
            completed,
            "owned RPC cleanup waited beyond its five-second grace for a retained stdin writer"
        );
        assert!(
            writer_released,
            "cleanup must unblock the actual writer without its caller aborting it"
        );
        assert!(!fixture.child.is_alive());
        assert_eq!(sessions.is_open("synthetic-session"), Some(false));
    }
    #[tokio::test]
    async fn rpc_blocked_cleanup_stop_does_not_wait_forever_for_stdin() {
        blocked_cleanup(false).await;
    }
    #[tokio::test]
    async fn rpc_blocked_cleanup_shutdown_does_not_wait_forever_for_stdin() {
        blocked_cleanup(true).await;
    }
    #[tokio::test]
    async fn rpc_blocked_cleanup_cancel_retains_captured_process_cleanup() {
        let fixture = Fixture::new(false).await;
        let sessions = RpcSessions::new();
        sessions
            .attach("synthetic-cancel", fixture.child.clone(), false)
            .await;
        let mut writing = fixture.blocked_writer().await;
        let owned = sessions.clone();
        let closing = tokio::spawn(async move { owned.stop("synthetic-cancel", None).await });
        tokio::time::timeout(Duration::from_secs(1), async {
            while !fixture.child.test_process_is_captured() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("owned cleanup captures its real process before waiting for stdin");
        closing.abort();
        let _ = closing.await;
        let released = tokio::time::timeout(Duration::from_secs(1), &mut writing)
            .await
            .is_ok();
        if !released {
            writing.abort();
            let _ = writing.await;
        }
        tokio::time::timeout(Duration::from_secs(1), async {
            while fixture.child.is_alive() || sessions.is_open("synthetic-cancel") != Some(false) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancelled cleanup retires only its captured process and receipt");
        assert!(
            released,
            "aborting cleanup must still unblock the retained actual writer"
        );
    }

    #[tokio::test]
    async fn rpc_blocked_cleanup_preserves_graceful_eof() {
        let fixture = Fixture::new(true).await;
        let sessions = RpcSessions::new();
        sessions
            .attach("synthetic-eof", fixture.child.clone(), false)
            .await;
        tokio::time::timeout(Duration::from_secs(6), sessions.stop("synthetic-eof", None))
            .await
            .expect("ordinary EOF cleanup stays within the existing grace");
        assert!(
            fixture.eof.exists(),
            "the actual child must see EOF rather than forced termination"
        );
        assert!(!fixture.child.is_alive());
        assert_eq!(sessions.is_open("synthetic-eof"), Some(false));
    }
}
