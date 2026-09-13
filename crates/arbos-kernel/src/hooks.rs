//! What tools and doors reach the kernel through: other agents, the user,
//! the browser, and the plan.
//!
//! Every message into an agent is a plan node in that agent's folder — a
//! user prompt, a `say` request, a spawn brief. The clock in
//! [`crate::plan`] fires them. Nothing waits in memory.

use anyhow::{Result, bail};
use arbos_core::{
    Agent, AgentId, Event, EventKind, NodeId, Place, Wake, append_event, files::Layout, inbox,
    list_agents, notes, store, subscription, validate_id, waiting,
};
use arbos_engine::TurnControl;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::sync::{mpsc, oneshot};

use crate::{
    attach::Frame,
    browser::{BrowserHub, BrowserOut},
    sched::{MAX_CHILDREN, MAX_DEPTH},
    worktree::{self, Worktree},
};

/// How a `say` reaches another agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SayMode {
    /// On their transcript for their next turn.
    Note,
    /// Queue a turn for them now; the reply comes back as a message.
    Request,
    /// Into their running turn at the next tool boundary; a turn if idle.
    Steer,
}

impl SayMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "note" => Some(Self::Note),
            "request" => Some(Self::Request),
            "steer" => Some(Self::Steer),
            _ => None,
        }
    }
}

/// Where a spawned child works.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Isolate {
    /// The parent's checkout (or the `cwd` it names).
    None,
    /// Its own git worktree of the place, on its own branch.
    Worktree,
}

impl Isolate {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "none" => Some(Self::None),
            "worktree" => Some(Self::Worktree),
            _ => None,
        }
    }
}

/// How big the agent tree may grow.
#[derive(Debug, Clone, Copy)]
pub struct Caps {
    pub depth: usize,
    pub children: usize,
}

impl Default for Caps {
    fn default() -> Self {
        Self {
            depth: MAX_DEPTH,
            children: MAX_CHILDREN,
        }
    }
}

impl Caps {
    /// From the host config; zero or absurd values fall back to the defaults.
    pub fn from_config(cfg: &arbos_core::HostConfig) -> Self {
        Self {
            depth: if (1..=8).contains(&cfg.max_depth) {
                cfg.max_depth
            } else {
                MAX_DEPTH
            },
            children: if (1..=64).contains(&cfg.max_children) {
                cfg.max_children
            } else {
                MAX_CHILDREN
            },
        }
    }
}

pub struct KernelHooks {
    pub place: Place,
    /// Housekeeping wakes only (`Serve`, `Compact`). Work goes through the plan.
    pub wakes: mpsc::UnboundedSender<Wake>,
    /// "Scan the plans now." Sent after every plan write.
    pub kick: mpsc::UnboundedSender<()>,
    pub frames: Mutex<Vec<mpsc::UnboundedSender<Frame>>>,
    /// Every ask/approve id this kernel issued. An answer that names one of
    /// these but not the pending one is late or a duplicate and is refused;
    /// an id the kernel never issued is an old client's guess (qa-021).
    pub issued: Mutex<HashSet<String>>,
    approve_seq: std::sync::atomic::AtomicU64,
    /// Pending allow/deny questions by agent: the tool asked about, and
    /// the receiver. One at a time per agent (approvals are interactive).
    pub approves: Mutex<HashMap<String, (String, String, oneshot::Sender<bool>)>>,
    /// Parents blocked in `spawn wait=true`, by child id: the child's first
    /// report (or the end of its first turn) resolves them.
    pub waits: Mutex<HashMap<String, (String, oneshot::Sender<String>)>>,
    /// Children whose turn end already reached a parent blocked in `wait`:
    /// no `done` message for that turn (it would say the same thing twice).
    pub waited: Mutex<HashSet<String>>,
    /// Transcript length when each running turn began, for the `done`
    /// message's summary of what the turn said.
    pub turn_lo: Mutex<HashMap<String, u64>>,
    /// `notes.md`'s (size, mtime) when each top-level turn began: the
    /// status page's `changed` frame goes out the moment the turn ends.
    notes_at_start: Mutex<HashMap<String, Option<(u64, i64)>>>,
    pub browsers: BrowserHub,
    /// Serialises plan file writes. One kernel per place holds the lock, so
    /// this is the whole claim story.
    pub plan_lock: Mutex<()>,
    /// Agents with a turn in flight. The serve loop keeps it current.
    pub running: Mutex<HashSet<String>>,
    /// The control handle of every turn in flight, shared with the
    /// scheduler. `say mode=steer` reaches a live turn through it.
    pub in_flight: Arc<Mutex<HashMap<String, TurnControl>>>,
    /// `(to, text)` already sent this turn, per agent. Cleared when a turn
    /// starts, so a model that loops on one message sends it once.
    sent: Mutex<HashMap<String, HashSet<String>>>,
    /// Serialises `spawn`: the child cap and the id check read the agents
    /// folder, so concurrent calls must not interleave.
    spawn_lock: Mutex<()>,
    /// Inbox files whose claim failed: name → when to try again (ms).
    inbox_retry: Mutex<HashMap<String, i64>>,
    /// Tree caps from `config.toml` (`max_depth`, `max_children`); the
    /// constants in `sched` are the defaults.
    pub caps: Caps,
    /// Children on other machines, reached over SSH.
    pub remotes: crate::remote::RemoteHub,
}

impl KernelHooks {
    pub fn new(
        place: Place,
        wakes: mpsc::UnboundedSender<Wake>,
        kick: mpsc::UnboundedSender<()>,
    ) -> Arc<Self> {
        Self::with_caps(place, wakes, kick, Caps::default())
    }

    pub fn with_caps(
        place: Place,
        wakes: mpsc::UnboundedSender<Wake>,
        kick: mpsc::UnboundedSender<()>,
        caps: Caps,
    ) -> Arc<Self> {
        Arc::new(Self {
            place,
            wakes,
            kick,
            caps,
            frames: Mutex::new(Vec::new()),
            issued: Mutex::new(HashSet::new()),
            approve_seq: std::sync::atomic::AtomicU64::new(1),
            waits: Mutex::new(HashMap::new()),
            waited: Mutex::new(HashSet::new()),
            turn_lo: Mutex::new(HashMap::new()),
            notes_at_start: Mutex::new(HashMap::new()),
            approves: Mutex::new(HashMap::new()),
            browsers: BrowserHub::new(),
            plan_lock: Mutex::new(()),
            running: Mutex::new(HashSet::new()),
            in_flight: Arc::new(Mutex::new(HashMap::new())),
            sent: Mutex::new(HashMap::new()),
            spawn_lock: Mutex::new(()),
            inbox_retry: Mutex::new(HashMap::new()),
            remotes: crate::remote::RemoteHub::default(),
        })
    }

    pub fn broadcast(&self, frame: Frame) {
        let mut xs = self.frames.lock().unwrap();
        xs.retain(|tx| tx.send(frame.clone()).is_ok());
    }

    pub fn kick(&self) {
        let _ = self.kick.send(());
    }

    /// The agent tree to every client, after a folder appears or changes.
    pub fn broadcast_tree(&self) {
        let agents = list_agents(&self.place).unwrap_or_default();
        let prs = arbos_core::load_prs(&self.place);
        let tree = agents
            .iter()
            .map(|a| arbos_core::wire::TreeNode {
                id: a.id.to_string(),
                name: a.name.clone(),
                parent: a.parent.as_ref().map(|p| p.to_string()),
                paused: a.paused,
                model: a.model.clone(),
                kind: "agent".into(),
                mode: a.mode.as_str().into(),
                prs: arbos_core::prs::prs_of_tree(&prs, a.id.as_str(), &agents).len() as u32,
            })
            .collect();
        self.broadcast(Frame::Tree { tree });
    }

    /// A fresh child id from a worker's name (or its brief), under the same
    /// caps as a local spawn.
    pub fn remote_child_id(&self, name: Option<&str>, brief: &str) -> Result<String> {
        let _one_at_a_time = self.spawn_lock.lock().unwrap();
        let base = match name.map(str::trim).filter(|n| !n.is_empty()) {
            Some(name) => name_slug(name),
            None => slug(brief),
        };
        let mut id = base.clone();
        let mut n = 1;
        while self.place.agent_dir(&id).exists() {
            n += 1;
            id = format!("{}-{n}", base.chars().take(20).collect::<String>());
        }
        Ok(id)
    }

    pub fn is_running(&self, agent: &str) -> bool {
        self.running.lock().unwrap().contains(agent)
    }

    pub fn turn_started(&self, agent: &str) {
        self.running.lock().unwrap().insert(agent.to_string());
        self.sent.lock().unwrap().remove(agent);
        let lo = count_lines(&self.layout(agent).transcript());
        self.turn_lo.lock().unwrap().insert(agent.to_string(), lo);
        self.notes_at_start.lock().unwrap().insert(
            agent.to_string(),
            crate::watch::stat(&store::notes_path(&self.place)),
        );
    }

    pub fn turn_ended(&self, agent: &str) {
        self.running.lock().unwrap().remove(agent);
        // The status page moved during this turn: tell every window now,
        // not at the watch's next second. Root is the only writer, so a
        // change seen at a child's turn end is root's, and still worth
        // one frame.
        let before = self.notes_at_start.lock().unwrap().remove(agent).flatten();
        let notes = store::notes_path(&self.place);
        let after = crate::watch::stat(&notes);
        if before != after {
            let kind = match (before, after) {
                (None, Some(_)) => "created",
                (Some(_), None) => "removed",
                _ => "modified",
            };
            self.broadcast(Frame::Changed {
                path: store::NOTES.to_string(),
                kind: kind.into(),
                size: after.map(|(size, _)| size).unwrap_or(0),
            });
        }
        // A child whose turn ended without a report: its last words, or its
        // failure, are what the waiting parent gets.
        if let Some((_, tx)) = self.waits.lock().unwrap().remove(agent) {
            self.waited.lock().unwrap().insert(agent.to_string());
            let events =
                arbos_core::load_transcript(&self.layout(agent).transcript()).unwrap_or_default();
            let text = events
                .iter()
                .rev()
                .find_map(|e| match &e.kind {
                    EventKind::Assistant { text, .. } if !text.trim().is_empty() => {
                        Some(text.clone())
                    }
                    EventKind::Notice { text, failed: true } => {
                        Some(format!("(the child failed) {text}"))
                    }
                    _ => None,
                })
                .unwrap_or_else(|| "(the child's turn ended without a report)".into());
            let _ = tx.send(text);
        }
    }

    /// Register a parent waiting on `child`'s first report.
    pub fn wait_for(&self, parent: &str, child: &str) -> oneshot::Receiver<String> {
        let (tx, rx) = oneshot::channel();
        self.waits
            .lock()
            .unwrap()
            .insert(child.to_string(), (parent.to_string(), tx));
        rx
    }

    pub fn stop_waiting(&self, child: &str) {
        self.waits.lock().unwrap().remove(child);
    }

    /// Children of `parent` that are still doing something: a turn in
    /// flight, a message waiting to wake them, or a parked ask/approve.
    /// A finished worker is not one (the multitasking audit: counting
    /// every folder stalled a coordinator at the cap after eight spawns).
    pub fn live_children(&self, parent: &AgentId) -> usize {
        list_agents(&self.place)
            .unwrap_or_default()
            .into_iter()
            .filter(|a| a.parent.as_ref() == Some(parent))
            .filter(|a| self.is_live(a.id.as_str()))
            .count()
    }

    /// Whether an agent is running, about to run, or parked on the user.
    pub fn is_live(&self, agent: &str) -> bool {
        if self
            .in_flight
            .lock()
            .map(|m| m.contains_key(agent))
            .unwrap_or(false)
        {
            return true;
        }
        if !arbos_core::waiting::list(&self.place, agent).is_empty() {
            return true;
        }
        inbox::list(&self.place, agent).iter().any(|f| f.msg.wake)
    }
}

// ── plan store: notes, subscriptions, inbox ─────────────────────────────

impl KernelHooks {
    pub fn layout(&self, agent: &str) -> Layout {
        Layout::new(&self.place, agent)
    }

    /// What the window draws: standing subscriptions, open notes items,
    /// and queued inbox messages, as one list.
    pub fn plan_frame(&self, agent: &str) -> Frame {
        let mut nodes = crate::plan::wire_rows(&self.place, agent);
        // Inbox files ride along as the queued rows the window already
        // draws (`inbox: true`).
        for filed in inbox::list(&self.place, agent) {
            nodes.push(arbos_core::wire::PlanNode {
                id: inbox_id(&filed.name),
                parent: 0,
                goal: filed.msg.body.clone(),
                status: "pending".into(),
                when: if filed.msg.wake {
                    "ready".into()
                } else {
                    "waits".into()
                },
                do_kind: "agent".into(),
                last: String::new(),
                origin: filed.msg.from.clone(),
                standing: false,
                inbox: true,
            });
        }
        Frame::Plan {
            agent: agent.to_string(),
            nodes,
        }
    }

    /// After any write to notes, subscriptions, or the inbox: broadcast
    /// and scan.
    pub fn plan_changed(&self, agent: &str) {
        self.broadcast(self.plan_frame(agent));
        self.kick();
    }

    /// The agent's checklist, for the `plan` tool.
    pub fn notes(&self, agent: &str) -> notes::Notes {
        notes::load(&self.place, agent)
    }

    pub fn save_notes(&self, agent: &str, n: &notes::Notes) -> Result<()> {
        notes::save(&self.place, agent, n)?;
        self.plan_changed(agent);
        Ok(())
    }

    /// `subscribe add`: validated, numbered, saved.
    pub fn subscribe(
        &self,
        agent: &str,
        sub: subscription::Subscription,
        after: Option<&str>,
    ) -> Result<subscription::Subscription> {
        let sub = subscription::add(&self.place, agent, sub, after)?;
        self.plan_changed(agent);
        Ok(sub)
    }

    pub fn unsubscribe(&self, agent: &str, id: u32) -> Result<bool> {
        let gone = subscription::remove(&self.place, agent, id)?;
        if gone {
            self.plan_changed(agent);
        }
        Ok(gone)
    }

    /// A message file for `agent`, from whoever `msg.from` says. The one
    /// door every producer uses.
    pub fn deliver(&self, agent: &str, msg: &inbox::Message) -> Result<String> {
        if !arbos_core::agent_exists(&self.place, agent) {
            bail!("no agent {agent}");
        }
        // Nothing to say and nothing attached is not a message; storing it
        // would fire a model turn on an empty prompt (QA bug qa-008).
        if msg.body.trim().is_empty() && msg.attachments.is_empty() {
            bail!("empty prompt");
        }
        let name = inbox::deliver(&self.place, agent, msg)?;
        self.plan_changed(agent);
        Ok(name)
    }
    /// A user prompt (or another producer's words) as an inbox file.
    /// `from`: `user`, `user:<name>`, `agent:<id>`, `kernel`.
    pub fn inbox(
        &self,
        agent: &str,
        text: &str,
        from: &str,
        attachments: Vec<String>,
    ) -> Result<NodeId> {
        self.inbox_with(agent, text, from, attachments, "", "")
    }

    /// The user's own prompt, with where it came from: `channel` (voice |
    /// text) and `device`, as the `user` frame carries them (#99).
    pub fn inbox_user(
        &self,
        agent: &str,
        text: &str,
        attachments: Vec<String>,
        channel: &str,
        device: &str,
    ) -> Result<NodeId> {
        self.inbox_with(agent, text, "user", attachments, channel, device)
    }

    fn inbox_with(
        &self,
        agent: &str,
        text: &str,
        from: &str,
        attachments: Vec<String>,
        channel: &str,
        device: &str,
    ) -> Result<NodeId> {
        let (from, kind) = match from {
            o if o.starts_with("spawn:") => (format!("agent:{}", &o["spawn:".len()..]), "brief"),
            "" | "user" => ("user".to_string(), "request"),
            o => (o.to_string(), "request"),
        };
        let mut msg = inbox::Message::new(from, kind, text.to_string());
        msg.wake = true;
        msg.hops = inbox::DEFAULT_HOPS;
        msg.attachments = attachments;
        msg.channel = channel.to_string();
        msg.device = device.to_string();
        let name = self.deliver(agent, &msg)?;
        Ok(inbox_id(&name))
    }
    /// Note a failed claim. True the first time (say so), false while the
    /// minute's back-off still runs.
    pub fn inbox_backoff(&self, name: &str, now: i64) -> bool {
        let mut m = self.inbox_retry.lock().unwrap();
        m.retain(|_, until| *until > now - 3_600_000);
        let first = !m.contains_key(name);
        m.insert(name.to_string(), now + 60_000);
        first
    }

    pub fn inbox_backing_off(&self, name: &str, now: i64) -> bool {
        self.inbox_retry
            .lock()
            .unwrap()
            .get(name)
            .is_some_and(|until| *until > now)
    }

    pub fn inbox_files(&self, agent: &str) -> Vec<inbox::Filed> {
        inbox::list(&self.place, agent)
    }

    /// Messages with `wake = false` waiting for this agent: onto its
    /// transcript now (a `say` from a peer, a notice from the kernel), and
    /// out of the inbox. Called as a turn starts, whatever caused it.
    pub fn take_notes(&self, agent: &str) -> usize {
        let mut taken = 0;
        for filed in inbox::list(&self.place, agent) {
            if filed.msg.wake {
                continue;
            }
            let event = match filed.msg.from.as_str() {
                "kernel" => EventKind::Notice {
                    text: filed.msg.body.clone(),
                    failed: false,
                },
                from => EventKind::Say {
                    from: from.strip_prefix("agent:").unwrap_or(from).to_string(),
                    text: filed.msg.body.clone(),
                },
            };
            if append_event(&self.layout(agent).transcript(), &Event::new(event)).is_ok() {
                let _ = std::fs::remove_file(&filed.path);
                taken += 1;
            }
        }
        if taken > 0 {
            self.broadcast(self.plan_frame(agent));
        }
        taken
    }

    pub fn descendants(&self, agent: &str) -> Vec<String> {
        let agents = list_agents(&self.place).unwrap_or_default();
        let mut out = vec![agent.to_string()];
        let mut i = 0;
        while i < out.len() {
            let cur = out[i].clone();
            for a in agents
                .iter()
                .filter(|a| a.parent.as_ref().is_some_and(|p| p.as_str() == cur))
            {
                if !out.iter().any(|x| *x == a.id.as_str()) {
                    out.push(a.id.to_string());
                }
            }
            i += 1;
        }
        out
    }

    /// The user pressed stop on `agent`: every standing or scheduled node
    /// of it and its children goes to blocked (run ▶ resumes one), and
    /// their running jobs are killed. Turns are the scheduler's to stop.
    pub fn stop_work(&self, agent: &str) -> Vec<String> {
        let ids = self.descendants(agent);
        for id in &ids {
            let root = arbos_engine::JobsRoot::for_agent(&self.place, &AgentId::new(id));
            for job in root.list() {
                root.kill(&job);
            }
            let mut changed = false;
            // A queued prompt that has not run is dropped with the stop; a
            // standing subscription pauses until someone presses run.
            for filed in inbox::list(&self.place, id) {
                if filed.msg.wake && std::fs::remove_file(&filed.path).is_ok() {
                    changed = true;
                }
            }
            for mut sub in subscription::list(&self.place, id) {
                if !sub.paused {
                    sub.paused = true;
                    sub.last = "stopped by the user — press run to resume".into();
                    let _ = subscription::save(&self.place, id, &sub);
                    changed = true;
                }
            }
            if changed {
                self.plan_changed(id);
            }
        }
        ids
    }
    /// A window action on a row of the plan frame: an inbox message
    /// (cancel, run), a subscription (cancel, run, pause, reopen), or a
    /// notes item (check, uncheck, cancel).
    pub fn plan_op(&self, agent: &str, id: NodeId, op: &str, text: &str) -> Result<()> {
        if is_inbox_id(id) {
            let Some(filed) = inbox::list(&self.place, agent)
                .into_iter()
                .find(|f| inbox_id(&f.name) == id)
            else {
                bail!("that message is no longer in the inbox");
            };
            match op {
                "cancel" => inbox::remove(&self.place, agent, &filed.name)?,
                "run" => {
                    let mut filed = filed;
                    filed.msg.wake = true;
                    inbox::rewrite(&filed)?;
                }
                other => bail!("{other}: not an operation on an inbox message (cancel, run)"),
            }
            self.plan_changed(agent);
            return Ok(());
        }
        if id & crate::plan::SUB_ID_BIT != 0 {
            let sid = (id & !crate::plan::SUB_ID_BIT) as u32;
            let Some(mut sub) = subscription::get(&self.place, agent, sid) else {
                bail!("no subscription #{sid}");
            };
            match op {
                "cancel" => {
                    self.unsubscribe(agent, sid)?;
                    return Ok(());
                }
                "pause" => sub.paused = true,
                "reopen" | "resume" => {
                    sub.paused = false;
                    if sub.next_due.is_none() {
                        sub.next_due = Some(inbox::rfc3339(arbos_core::now_ms()));
                    }
                }
                "run" => {
                    sub.paused = false;
                    sub.next_due = Some(inbox::rfc3339(arbos_core::now_ms()));
                }
                other => bail!(
                    "{other}: not an operation on a subscription (cancel, run, pause, reopen)"
                ),
            }
            subscription::save(&self.place, agent, &sub)?;
            self.plan_changed(agent);
            return Ok(());
        }
        if id & crate::plan::NOTE_ID_BIT != 0 {
            let n = (id & !crate::plan::NOTE_ID_BIT) as usize;
            let mut notes = self.notes(agent);
            match op {
                // The window's ✓ / "run" on a checklist row marks it done.
                "check" | "answer" | "run" => {
                    notes.check(n, true, Some(text))?;
                }
                "uncheck" | "reopen" => {
                    notes.check(n, false, None)?;
                }
                "cancel" => {
                    notes.remove(n)?;
                }
                other => {
                    bail!("{other}: not an operation on a notes item (check, uncheck, cancel)")
                }
            }
            return self.save_notes(agent, &notes);
        }
        bail!("unknown plan row #{id}")
    }
}
/// A `kind` a model writes to mean "the default one".
pub fn is_no_kind(kind: &str) -> bool {
    matches!(
        kind.trim().to_ascii_lowercase().as_str(),
        "default" | "none" | "null" | "auto" | "standard" | "generic" | "inherit" | "-"
    )
}

// ── agents ──────────────────────────────────────────────────────────────

impl KernelHooks {
    pub fn spawn(
        &self,
        parent: &Agent,
        brief: &str,
        model: Option<&str>,
        allowlist: Option<Vec<String>>,
        readonly: bool,
        cwd: Option<PathBuf>,
        kind: Option<&str>,
    ) -> Result<AgentId> {
        self.spawn_isolated(
            parent,
            brief,
            model,
            allowlist,
            readonly,
            cwd,
            Isolate::None,
            kind,
        )
        .map(|(id, _)| id)
    }

    /// `spawn` with a working directory of the child's own. `Isolate::Worktree`
    /// cuts a git worktree of the place for it; an explicit `cwd` wins.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_isolated(
        &self,
        parent: &Agent,
        brief: &str,
        model: Option<&str>,
        allowlist: Option<Vec<String>>,
        readonly: bool,
        cwd: Option<PathBuf>,
        isolate: Isolate,
        kind: Option<&str>,
    ) -> Result<(AgentId, Option<Worktree>)> {
        self.spawn_named(
            parent, None, brief, model, allowlist, readonly, cwd, isolate, kind,
        )
    }

    /// `spawn_isolated` with the worker's name given (the coordinator
    /// protocol's short imperative label). The name makes the child's id
    /// and its row; without one both come from the brief's first words.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_named(
        &self,
        parent: &Agent,
        name: Option<&str>,
        brief: &str,
        model: Option<&str>,
        allowlist: Option<Vec<String>>,
        readonly: bool,
        cwd: Option<PathBuf>,
        isolate: Isolate,
        kind: Option<&str>,
    ) -> Result<(AgentId, Option<Worktree>)> {
        // A definition fills in what the call left out; the call's own
        // model wins, the def's readonly cannot be switched off. Models fill
        // every optional field: `kind: "default"` (or none/null/auto) is not
        // a request for a definition, it is the built-in child (QA qa-019).
        let kind = kind
            .map(str::trim)
            .filter(|k| !k.is_empty() && !is_no_kind(k));
        let def = match kind {
            None => None,
            Some(k) => match arbos_core::find_def(&self.place, k) {
                Some(d) => Some(d),
                None => {
                    let known: Vec<String> = arbos_core::load_defs(&self.place)
                        .into_iter()
                        .map(|d| d.name)
                        .collect();
                    if known.is_empty() {
                        // Nothing to choose from: the name cannot mean a
                        // definition, so it is the built-in child. Refusing
                        // here left a fresh place unable to spawn at all.
                        crate::klog::warn(
                            "spawn_kind_ignored",
                            Some(parent.id.as_str()),
                            format!(
                                "kind {k:?}: no agent definitions exist here; spawning the built-in child"
                            ),
                        );
                        None
                    } else {
                        bail!(
                            "spawn: no agent definition named {k:?}. Kinds here: {}",
                            known.join(", ")
                        );
                    }
                }
            },
        };
        let model = model.or(def
            .as_ref()
            .map(|d| d.model.as_str())
            .filter(|m| !m.is_empty()));
        let allowlist = allowlist.or(def
            .as_ref()
            .filter(|d| !d.allowlist.is_empty())
            .map(|d| d.allowlist.clone()));
        let readonly = readonly || def.as_ref().is_some_and(|d| d.readonly);
        let cwd = cwd.or(def.as_ref().and_then(|d| d.cwd.clone()));
        // One spawn at a time: the model runs parallel tool calls, and the
        // cap and the id check both read the agents folder, so without the
        // lock ten calls all see zero children and all pass.
        let _one_at_a_time = self.spawn_lock.lock().unwrap();
        let depth = parent.depth(&list_agents(&self.place).unwrap_or_default());
        if depth >= self.caps.depth {
            bail!(
                "spawn: the agent tree is capped at {} levels below the root and you are on level {depth}; do this work yourself or ask your parent to spawn. `max_depth` in config.toml changes the cap.",
                self.caps.depth
            );
        }
        let live = self.live_children(&parent.id);
        if live >= self.caps.children {
            bail!(
                "spawn: you already have {live} live children, the cap (`max_children` in config.toml is {}); wait for one to report, or give an existing child the work with say.",
                self.caps.children
            );
        }
        // Two children with the same name get distinct ids (`-2`, `-3`, …)
        // rather than the second one failing.
        let label = name.map(str::trim).filter(|n| !n.is_empty());
        let base = match label {
            Some(label) => name_slug(label),
            None => slug(brief),
        };
        let mut id = base.clone();
        let mut n = 1;
        while self.place.agent_dir(&id).exists() {
            n += 1;
            id = format!("{}-{n}", base.chars().take(20).collect::<String>());
        }
        validate_id(&id)?;
        // The worktree comes first: if git refuses, no agent folder is
        // left behind for a child that never existed.
        let worktree = match (&cwd, isolate) {
            (Some(_), Isolate::Worktree) | (_, Isolate::None) => None,
            (None, Isolate::Worktree) => Some(worktree::create(self.place.path(), &id)?),
        };
        // The parent's list as saved, not as narrowed for this turn: a
        // coordinator's children are the ones that edit.
        let saved_parent =
            arbos_core::load_agent(&self.place, &parent.id).unwrap_or_else(|_| parent.clone());
        let mut child = Agent::root(&id);
        child.name = label.unwrap_or(brief).chars().take(48).collect();
        child.parent = Some(parent.id.clone());
        child.model = model.unwrap_or("inherit").to_string();
        if let Some(list) = allowlist {
            child.allowlist = list;
        } else {
            child.allowlist = saved_parent.allowlist.clone();
        }
        child.readonly = readonly;
        child.cwd = cwd.or_else(|| worktree.as_ref().map(|w| w.path.clone()));
        if let Some(d) = &def {
            child.kind = d.name.clone();
        }
        child.restrict_allowlist(&saved_parent);
        child.save(&self.place.agent_dir(&id))?;
        let layout = Layout::new(&self.place, &id);
        std::fs::create_dir_all(layout.jobs())?;
        if let Some(d) = def.as_ref().filter(|d| !d.body.is_empty()) {
            std::fs::write(layout.instructions(), format!("{}\n", d.body))?;
        }
        // The brief is the child's first message: its mission, as a
        // `kind = "brief"` inbox file that fires a turn now.
        self.inbox(&id, brief, &format!("spawn:{}", parent.id), Vec::new())?;
        Ok((AgentId::new(id), worktree))
    }

    /// Who `to` means. Exact id, exact name, then a unique substring of a
    /// name. Zero or several hits send nothing.
    fn resolve(&self, from: &AgentId, to: &str) -> Result<Agent> {
        let agents = list_agents(&self.place)?;
        let q = to.trim();
        if let Some(a) = agents.iter().find(|a| a.id.as_str() == q) {
            return self.check_target(from, a.clone());
        }
        if let Some(a) = agents.iter().find(|a| a.name == q) {
            return self.check_target(from, a.clone());
        }
        let lq = q.to_ascii_lowercase();
        let hits: Vec<&Agent> = agents
            .iter()
            .filter(|a| a.id != *from && a.name.to_ascii_lowercase().contains(&lq))
            .collect();
        match hits.len() {
            1 => self.check_target(from, hits[0].clone()),
            0 => bail!(
                "say: no agent is named {q:?}. Agents here: {}",
                roster(&agents, from)
            ),
            _ => bail!(
                "say: {q:?} matches several; nothing sent. Use the exact id of one:\n{}",
                hits.iter()
                    .map(|a| format!("  {} — {}", a.id, a.name))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        }
    }

    fn check_target(&self, from: &AgentId, a: Agent) -> Result<Agent> {
        if a.id == *from {
            bail!("say: that is this agent; reply here instead");
        }
        Ok(a)
    }

    /// Send to another agent or the user. Returns the receipt the sender reads.
    ///
    /// `Request` queues a turn for them and carries a reply budget; a
    /// `Note` lands on their transcript for their next turn; a `Steer` goes
    /// into their live turn at its next tool boundary, or queues a turn
    /// when they are idle. `hops_in` is the budget this turn was started
    /// with.
    pub fn say(
        &self,
        from: &AgentId,
        to: &str,
        text: &str,
        mode: SayMode,
        hops_in: u8,
    ) -> Result<String> {
        let text = text.trim();
        if text.is_empty() {
            bail!("say: text must not be empty");
        }
        if to.trim().eq_ignore_ascii_case("user") {
            self.dedupe(from, "user", text)?;
            self.notify_user(from.as_str(), text)?;
            return Ok(
                "Sent to the user as a notice in this chat: now if it is open, otherwise when they next open it."
                    .into(),
            );
        }
        let target = self.resolve(from, to)?;
        let tid = target.id.as_str();
        self.dedupe(from, tid, text)?;
        if let Some(remote) = &target.remote {
            // The child lives on another machine: its kernel gets the words.
            self.remotes
                .forward(self, tid, from.as_str(), text, false)?;
            return Ok(format!(
                "Sent to {} ({tid}) on {}; its reply will arrive here as a message from it.",
                target.name,
                remote.split(':').next().unwrap_or(remote)
            ));
        }
        let label = format!("{} ({})", target.name, tid);
        // A steer is an inbox file of kind `steer`: a running turn takes it
        // at its next tool boundary; an idle one wakes on it.
        if mode == SayMode::Steer {
            let mut msg = inbox::Message::new(format!("agent:{from}"), "steer", text);
            msg.hops = if hops_in > 0 {
                hops_in - 1
            } else {
                inbox::DEFAULT_HOPS
            };
            inbox::deliver(&self.place, tid, &msg)?;
            self.plan_changed(tid);
            return Ok(if self.is_running(tid) {
                format!(
                    "Sent to {label} as a steer: it is running now and reads this at its next tool boundary, in the same turn. Its reply, if any, arrives here as a message from it."
                )
            } else {
                format!(
                    "Sent to {label} as a steer: it was idle, so a turn starts for it now. Its reply, if any, arrives here as a message from it."
                )
            });
        }
        // A parent blocked in spawn wait=true gets this as the tool result.
        let waiting_parent = self
            .waits
            .lock()
            .unwrap()
            .get(from.as_str())
            .is_some_and(|(parent, _)| parent == tid);
        if waiting_parent && let Some((_, tx)) = self.waits.lock().unwrap().remove(from.as_str()) {
            if tx.send(text.to_string()).is_ok() {
                // The parent has its report; the done file at this turn's
                // end would be the same words a second time.
                self.waited.lock().unwrap().insert(from.to_string());
                append_event(
                    &self.layout(tid).transcript(),
                    &Event::new(EventKind::Say {
                        from: from.to_string(),
                        text: text.to_string(),
                    }),
                )?;
                return Ok(format!(
                    "Delivered to {} ({tid}) as the result of the spawn call it is waiting on; no turn needed.",
                    target.name
                ));
            }
        }
        let busy = self.is_running(tid);
        let request = match mode {
            SayMode::Note => false,
            SayMode::Request => true,
            // Idle, so there is no turn to steer: start one.
            SayMode::Steer => true,
        };
        // A note is an inbox file the peer reads at the start of its next
        // turn; a request is one that starts a turn. Nothing is written into
        // the peer's transcript from here: its own turn does that.
        let mut note = inbox::Message::new(format!("agent:{from}"), "message", text);
        note.wake = false;
        if !request {
            inbox::deliver(&self.place, tid, &note)?;
            self.broadcast(self.plan_frame(tid));
            return Ok(format!(
                "Sent to {label} as a note; it will read it at its next turn{}.",
                if busy {
                    " (it is running now; use mode steer to reach the current turn)"
                } else {
                    ""
                }
            ));
        }
        let hops = if hops_in > 0 {
            hops_in - 1
        } else {
            inbox::DEFAULT_HOPS
        };
        if hops == 0 {
            inbox::deliver(&self.place, tid, &note)?;
            self.broadcast(self.plan_frame(tid));
            return Ok(format!(
                "Sent to {label} as a note: this exchange's request budget is spent, so no turn is queued. It will read it at its next turn."
            ));
        }
        if target.paused {
            inbox::deliver(&self.place, tid, &note)?;
            self.broadcast(self.plan_frame(tid));
            return Ok(format!(
                "Sent to {label} as a note: it is paused, so no turn is queued."
            ));
        }
        let mut msg = inbox::Message::new(format!("agent:{from}"), "request", text.to_string());
        msg.wake = true;
        msg.hops = hops;
        self.deliver(tid, &msg)?;
        Ok(match (mode, busy) {
            (SayMode::Steer, _) => format!(
                "Sent to {label} as a steer; it was idle, so a turn starts for it now. Its reply will arrive here as a message from it."
            ),
            (SayMode::Request | SayMode::Note, true) => format!(
                "Sent to {label} as a request; it is running now, so a turn is queued after its current one. Its reply will arrive here as a message from it."
            ),
            (SayMode::Request | SayMode::Note, false) => format!(
                "Sent to {label} as a request; a turn is queued for it. Its reply will arrive here as a message from it."
            ),
        })
    }

    fn dedupe(&self, from: &AgentId, to: &str, text: &str) -> Result<()> {
        let key = format!("{to}\u{0}{text}");
        let mut sent = self.sent.lock().unwrap();
        let set = sent.entry(from.to_string()).or_default();
        if !set.insert(key) {
            bail!(
                "say: you already sent exactly this to {to} in this turn; it was delivered once. Wait for the reply or say something new"
            );
        }
        Ok(())
    }

    /// A durable notice for the person. It lands in `user.md` and on the
    /// transcript of the top-level chat the sender belongs to — the user
    /// reads the parent's chat, not a child's — and on the sender's own
    /// when that is a different agent.
    pub fn notify_user(&self, from: &str, text: &str) -> Result<()> {
        let mut inbox = std::fs::read_to_string(self.place.user_md()).unwrap_or_default();
        inbox.push_str(&format!(
            "- {} [{from}] {}\n",
            subscription::clock(arbos_core::now_ms()),
            text.replace('\n', " ")
        ));
        std::fs::write(self.place.user_md(), inbox)?;
        let event = Event::new(EventKind::Say {
            from: format!("{from} → user"),
            text: text.to_string(),
        });
        let top = self.top_ancestor(from);
        append_event(&self.layout(&top).transcript(), &event)?;
        if top != from {
            append_event(&self.layout(from).transcript(), &event)?;
        }
        Ok(())
    }

    /// The root of `agent`'s parent chain: the chat the user opens.
    fn top_ancestor(&self, agent: &str) -> String {
        let agents = list_agents(&self.place).unwrap_or_default();
        let mut cur = agent.to_string();
        for _ in 0..32 {
            let Some(a) = agents.iter().find(|a| a.id.as_str() == cur) else {
                break;
            };
            match &a.parent {
                Some(p) if agents.iter().any(|x| x.id == *p) => cur = p.to_string(),
                _ => break,
            }
        }
        cur
    }

    /// Post a question. The receiver resolves when the user answers.
    /// Park a question: `waiting/ask-<id>.toml`, the `ask` transcript line,
    /// and the live frame. No channel — the turn ends after this call, and
    /// the answer arrives as an inbox file that starts the next turn. A
    /// kernel restart leaves the question standing.
    pub fn ask(
        &self,
        agent: &AgentId,
        question: &str,
        options: &[String],
        call_id: &str,
    ) -> Result<String> {
        let id = if call_id.is_empty() {
            format!("ask-{}", arbos_core::now_ms())
        } else {
            call_id.to_string()
        };
        self.issued.lock().unwrap().insert(id.clone());
        waiting::write(
            &self.place,
            agent.as_str(),
            &waiting::Waiting {
                kind: "ask".into(),
                id: id.clone(),
                question: question.to_string(),
                options: options.to_vec(),
                tool: String::new(),
                asked: inbox::rfc3339(arbos_core::now_ms()),
            },
        )?;
        // The same id on the transcript line, so a client that sees the live
        // frame and then the line knows they are one question.
        append_event(
            &Layout::new(&self.place, agent.as_str()).transcript(),
            &Event::new(EventKind::Ask {
                question: question.to_string(),
                options: options.to_vec(),
                call_id: Some(id.clone()),
            }),
        )?;
        self.broadcast(Frame::Ask {
            agent: agent.to_string(),
            question: question.to_string(),
            options: options.to_vec(),
            id: Some(id.clone()),
        });
        Ok(id)
    }

    /// The parked questions of `agent`, oldest first.
    pub fn pending_asks(&self, agent: &str) -> Vec<waiting::Waiting> {
        waiting::asks(&self.place, agent)
    }

    /// Take the answer to a parked question: the waiting file goes, the
    /// `answer` line is written, and an inbox file of `kind = "answer"`
    /// starts the agent's next turn with the words. An empty answer is the
    /// Skip button and says so to the model.
    pub fn answer(&self, agent: &str, ask_id: &str, text: &str) -> Result<()> {
        waiting::remove(&self.place, agent, "ask", ask_id);
        append_event(
            &self.layout(agent).transcript(),
            &Event::new(EventKind::Answer {
                text: text.to_string(),
            }),
        )?;
        let body = if text.trim().is_empty() {
            "The user skipped this question without answering. Choose a sensible default yourself, say which you chose, and continue.".to_string()
        } else {
            text.to_string()
        };
        let mut msg = inbox::Message::new("user", "answer", body);
        msg.wake = true;
        msg.reply_to = ask_id.to_string();
        self.deliver(agent, &msg)?;
        Ok(())
    }

    /// Post an allow/deny prompt. The receiver resolves when the user
    /// answers; `waiting/approve-N.toml` mirrors it meanwhile.
    pub fn approve(&self, agent: &AgentId, tool: &str, command: &str) -> oneshot::Receiver<bool> {
        let (tx, rx) = oneshot::channel();
        let n = self
            .approve_seq
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let id = format!("approve-{n}");
        self.issued.lock().unwrap().insert(id.clone());
        self.approves
            .lock()
            .unwrap()
            .insert(agent.to_string(), (tool.to_string(), id.clone(), tx));
        let question = format!("allow {tool}: {command}");
        let _ = waiting::write(
            &self.place,
            agent.as_str(),
            &waiting::Waiting {
                kind: "approve".into(),
                id: id.clone(),
                question: question.clone(),
                options: vec!["allow".into(), "deny".into()],
                tool: tool.to_string(),
                asked: inbox::rfc3339(arbos_core::now_ms()),
            },
        );
        self.broadcast(Frame::Ask {
            agent: agent.to_string(),
            question,
            options: vec!["allow".into(), "deny".into()],
            id: Some(id),
        });
        rx
    }

    /// Whether an answer may resolve one of `agent`'s pending questions.
    /// `given` is the id the client sent (empty = none); `pending` the ids
    /// waiting. Ok(the id it resolves) or the reason.
    pub fn answer_allowed(
        &self,
        agent: &str,
        given: &str,
        pending: &[String],
    ) -> std::result::Result<String, String> {
        let Some(first) = pending.first() else {
            return Err(format!("no question is pending for {agent}"));
        };
        // Blind: no id, or the agent id (what the desktop's approve sent
        // before asks had ids). Only safe when there is exactly one
        // question it could mean.
        if given.is_empty() || given == agent {
            if pending.len() == 1 {
                return Ok(first.clone());
            }
            return Err(format!(
                "answer without an ask id while {} questions are pending; send id {first:?}",
                pending.len()
            ));
        }
        if pending.iter().any(|p| p == given) {
            return Ok(given.to_string());
        }
        if !self.issued.lock().unwrap().contains(given)
            && !waiting::list(&self.place, agent)
                .iter()
                .any(|w| w.id == given)
        {
            // An id this kernel never issued is a client's mistake, not a
            // late answer to another question. With one question pending
            // it can only mean that one: take it rather than leave the user
            // with no card and a stuck turn (ui-004).
            if pending.len() == 1 {
                crate::klog::warn(
                    "answer_id_unknown",
                    Some(agent),
                    format!(
                        "answer names ask {given:?} (never issued); taken for the one pending question {first:?}"
                    ),
                );
                return Ok(first.clone());
            }
            return Err(format!(
                "answer names ask {given:?}, which this kernel never issued; the pending question is {first:?}"
            ));
        }
        Err(format!(
            "answer names ask {given:?} but the pending question is {first:?}; a late or duplicate answer resolves nothing"
        ))
    }

    pub fn browser(&self, agent: &AgentId, action: &str, args: &Value) -> Result<BrowserOut> {
        self.browsers.act(agent.as_str(), action, args)
    }
}

/// `id — name (paused)` per agent other than `me`.
pub fn roster(agents: &[Agent], me: &AgentId) -> String {
    let lines: Vec<String> = agents
        .iter()
        .filter(|a| a.id != *me)
        .map(|a| {
            format!(
                "{}{}{}",
                a.id,
                if a.name.is_empty() || a.name == a.id.as_str() {
                    String::new()
                } else {
                    format!(" — {}", a.name)
                },
                if a.paused { " (paused)" } else { "" }
            )
        })
        .collect();
    if lines.is_empty() {
        "(none)".into()
    } else {
        lines.join("; ")
    }
}

/// A worker's name as an id: words joined by `-`, so "Write river poem"
/// reads as `write-river-poem` in the panel and in `say to=`.
fn name_slug(name: &str) -> String {
    let words: Vec<String> = name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    let mut s = String::new();
    for w in words {
        if s.len() + w.len() + usize::from(!s.is_empty()) > 32 {
            break;
        }
        if !s.is_empty() {
            s.push('-');
        }
        s.push_str(&w);
    }
    if s.is_empty() {
        return slug(name);
    }
    if s.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        s = format!("a{s}");
    }
    s
}

fn slug(brief: &str) -> String {
    let mut s: String = brief
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(24)
        .collect();
    if s.is_empty() {
        s = format!("a{}", &uuid::Uuid::new_v4().to_string()[..8]);
    }
    if s.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        s = format!("a{s}");
    }
    s.to_ascii_lowercase()
}

/// The id a window sees for an inbox file: bit 40 set, then a hash of the
/// name. Plan node ids are small integers, so the two never collide.
pub fn inbox_id(name: &str) -> NodeId {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    (1u64 << 40) | (h & 0xffff_ffff)
}

pub fn is_inbox_id(id: NodeId) -> bool {
    id & (1u64 << 40) != 0
}

/// Lines in a file, cheaply (no parse): the transcript's event count.
fn count_lines(path: &std::path::Path) -> u64 {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else {
        return 0;
    };
    let mut buf = [0u8; 64 * 1024];
    let mut n = 0u64;
    while let Ok(read) = f.read(&mut buf) {
        if read == 0 {
            break;
        }
        n += buf[..read].iter().filter(|b| **b == b'\n').count() as u64;
    }
    n
}
