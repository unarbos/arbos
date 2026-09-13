//! What tools and doors reach the kernel through: other agents, the user,
//! the browser, and the plan.
//!
//! Every message into an agent is a plan node in that agent's folder — a
//! user prompt, a `say` request, a spawn brief. The clock in
//! [`crate::plan`] fires them. Nothing waits in memory.

use anyhow::{Result, bail};
use arbos_core::{
    Agent, AgentId, Event, EventKind, Node, NodeId, Place, Wake, append_event,
    files::Layout,
    inbox, list_agents,
    node::{self, DEFAULT_HOPS},
    validate_id,
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
    /// Pending questions by agent: the ask's id and the receiver.
    pub asks: Mutex<HashMap<String, (String, oneshot::Sender<String>)>>,
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
    /// Parsed `plan.jsonl` per agent, keyed by the file's (length, mtime).
    /// One turn's end reads the plan five or six times in a row; a plan
    /// that holds a large prompt made that a multi-second stall of the
    /// serve loop (QA bug qa-003).
    plans: Mutex<HashMap<String, PlanCache>>,
    /// Children on other machines, reached over SSH.
    pub remotes: crate::remote::RemoteHub,
}

struct PlanCache {
    len: u64,
    modified: Option<std::time::SystemTime>,
    nodes: Arc<Vec<Node>>,
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
            asks: Mutex::new(HashMap::new()),
            issued: Mutex::new(HashSet::new()),
            approve_seq: std::sync::atomic::AtomicU64::new(1),
            waits: Mutex::new(HashMap::new()),
            waited: Mutex::new(HashSet::new()),
            turn_lo: Mutex::new(HashMap::new()),
            approves: Mutex::new(HashMap::new()),
            browsers: BrowserHub::new(),
            plan_lock: Mutex::new(()),
            running: Mutex::new(HashSet::new()),
            in_flight: Arc::new(Mutex::new(HashMap::new())),
            sent: Mutex::new(HashMap::new()),
            spawn_lock: Mutex::new(()),
            inbox_retry: Mutex::new(HashMap::new()),
            plans: Mutex::new(HashMap::new()),
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

    /// A fresh child id from a brief, under the same caps as a local spawn.
    pub fn remote_child_id(&self, brief: &str) -> Result<String> {
        let _one_at_a_time = self.spawn_lock.lock().unwrap();
        let base = slug(brief);
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
    }

    pub fn turn_ended(&self, agent: &str) {
        self.running.lock().unwrap().remove(agent);
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

    pub fn live_children(&self, parent: &AgentId) -> usize {
        list_agents(&self.place)
            .unwrap_or_default()
            .into_iter()
            .filter(|a| a.parent.as_ref() == Some(parent))
            .count()
    }
}

// ── plan store ──────────────────────────────────────────────────────────

/// One node as the `plan` tool takes it.
#[derive(Debug, Clone, Default)]
pub struct NewNode {
    pub goal: String,
    pub check: String,
    pub after: Option<String>,
    pub every: Option<String>,
    pub wake: bool,
    pub condition: String,
    pub shell: String,
    pub notify: String,
    pub ask: bool,
    pub par: bool,
}

/// `"0"`, `"0s"`, `"0m"`: a duration a model writes to mean "none".
fn is_zero_duration(x: &str) -> bool {
    let t = x.trim().to_ascii_lowercase();
    let digits: String = t
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let unit = &t[digits.len()..];
    !digits.is_empty()
        && digits.chars().all(|c| c == '0' || c == '.')
        && matches!(
            unit.trim(),
            "" | "s"
                | "sec"
                | "secs"
                | "m"
                | "min"
                | "mins"
                | "h"
                | "hr"
                | "hrs"
                | "d"
                | "day"
                | "days"
                | "ms"
        )
}

impl NewNode {
    /// `{goal, check, when:{after, every, wake, condition}, do:{shell, notify, ask}, par}`.
    pub fn from_json(v: &Value) -> Result<Self> {
        let s = |k: &str, o: Option<&Value>| -> String {
            o.and_then(|o| o.get(k))
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim()
                .to_string()
        };
        let b = |k: &str, o: Option<&Value>| -> bool {
            o.and_then(|o| o.get(k))
                .map(|x| x.as_bool().unwrap_or_else(|| x.as_str() == Some("true")))
                .unwrap_or(false)
        };
        let when = v.get("when");
        let do_ = v.get("do");
        // Models fill every field: `after: "0s"` next to `every: "1h"` means
        // "no delay", not a second trigger. Zero is unset.
        let opt = |x: String| {
            if x.is_empty() || is_zero_duration(&x) {
                None
            } else {
                Some(x)
            }
        };
        Ok(Self {
            goal: s("goal", Some(v)),
            check: s("check", Some(v)),
            after: opt(s("after", when)),
            every: opt(s("every", when)),
            wake: b("wake", when) || b("onDeps", when) || b("on_deps", when),
            condition: s("condition", when),
            shell: s("shell", do_),
            notify: s("notify", do_),
            ask: b("ask", do_),
            par: b("par", Some(v)),
        })
    }

    fn build(&self, now_ms: i64) -> Result<Node> {
        if self.goal.is_empty() {
            bail!("goal must not be empty");
        }
        let mut n = Node::new(self.goal.clone());
        n.check = self.check.clone();
        let mut execs = 0;
        if !self.shell.is_empty() {
            // shell + notify: run it, then deliver the output. The template
            // must place the output: a fixed message on a reading node
            // hides what was read.
            if !self.notify.is_empty() && !self.notify.contains("{output}") {
                bail!(
                    "do.notify on a shell node must contain {{output}} (where the command's output goes), got {:?}",
                    self.notify
                );
            }
            n.do_ = arbos_core::Do::Shell {
                cmd: self.shell.clone(),
                report: (!self.notify.is_empty()).then(|| self.notify.clone()),
            };
            execs += 1;
        } else if !self.notify.is_empty() {
            n.do_ = arbos_core::Do::Notify {
                text: self.notify.clone(),
            };
            execs += 1;
        }
        if self.ask {
            n.do_ = arbos_core::Do::Ask;
            execs += 1;
        }
        if execs > 1 {
            bail!("do: ask cannot combine with shell or notify (omit do for an agent task)");
        }
        let mut trig = 0;
        if let Some(after) = &self.after {
            let ms = node::parse_duration_ms(after).ok_or_else(|| {
                anyhow::anyhow!("when.after must be a duration like \"30m\", got {after:?}")
            })?;
            n.when.after_ms = Some(now_ms + ms as i64);
            trig += 1;
        }
        if let Some(every) = &self.every {
            let ms = node::parse_duration_ms(every).ok_or_else(|| {
                anyhow::anyhow!("when.every must be a duration like \"1h\", got {every:?}")
            })?;
            if ms < node::MIN_EVERY_MS {
                bail!(
                    "when.every must be at least {}; for a finer mechanical cadence use a background bash job",
                    node::human_ms(node::MIN_EVERY_MS)
                );
            }
            n.when.every_ms = Some(ms);
            n.when.next_due_ms = Some(now_ms + ms as i64);
            trig += 1;
        }
        if trig > 1 {
            bail!("when: choose one of after, every (or omit for ready)");
        }
        // `wake` says "fire a turn of me when this is ready". A timed node
        // already does; so does a mechanical one. It only adds meaning on a
        // plain agent node, so elsewhere it is accepted and implied.
        if self.wake && trig == 0 && matches!(n.do_, arbos_core::Do::Agent) {
            n.when.wake = true;
        }
        // A schedule written into the goal instead of `when` never fires.
        // Refuse it with the fix, rather than store a node that only looks
        // scheduled. A one-shot `after` does not make "every hour" true
        // either (QA bug qa-009): the node fires once and the user was told
        // it recurs.
        if n.when.every_ms.is_none() {
            let g = n.goal.to_ascii_lowercase();
            let words = [
                "every ",
                "each ",
                "hourly",
                "daily",
                "weekly",
                "per minute",
                "per hour",
            ];
            if words
                .iter()
                .any(|w| g.starts_with(w) || g.contains(&format!(" {w}")))
            {
                let fate = if n.when.after_ms.is_some() {
                    "when.after fires it once and then it is done"
                } else {
                    "it would never fire"
                };
                bail!(
                    "the goal reads like a recurring job but when.every is not set, so {fate}. Set when:{{every:\"1h\"}} (or the period you mean; omit after) and keep the goal as what each firing does"
                );
            }
            let deferred_wording = g.starts_with("after ")
                || (g.starts_with("in ")
                    && node::parse_duration_ms(
                        g[3..]
                            .split_whitespace()
                            .take(2)
                            .collect::<Vec<_>>()
                            .join("")
                            .as_str(),
                    )
                    .is_some());
            if trig == 0 && deferred_wording {
                bail!(
                    "the goal reads like a deferred task but when.after is not set, so it would never fire. Set when:{{after:\"30m\"}} (or the delay you mean)"
                );
            }
        }
        if !self.condition.is_empty() {
            if n.when.every_ms.is_none() {
                bail!("when.condition needs when.every (the poll period to re-check it on)");
            }
            if !matches!(n.do_, arbos_core::Do::Agent | arbos_core::Do::Notify { .. }) {
                bail!(
                    "when.condition fires an agent or notify do, not a {} node. For a command on a schedule drop when.condition and keep when.every (the shell runs each period; add do.notify to report its output). To run the command only when a check passes, put the check inside the command: shell:\"<check> && <command>\"",
                    n.do_.kind()
                );
            }
            n.when.condition = self.condition.clone();
        }
        Ok(n)
    }
}

/// `kind` spellings that mean "no custom definition".
pub fn is_no_kind(kind: &str) -> bool {
    matches!(
        kind.trim().to_ascii_lowercase().as_str(),
        "default" | "none" | "null" | "auto" | "standard" | "generic" | "inherit" | "-"
    )
}

impl KernelHooks {
    pub fn layout(&self, agent: &str) -> Layout {
        Layout::new(&self.place, agent)
    }

    pub fn plan_nodes(&self, agent: &str) -> Vec<Node> {
        let path = self.layout(agent).plan_jsonl();
        let Ok(meta) = std::fs::metadata(&path) else {
            self.plans.lock().unwrap().remove(agent);
            return Vec::new();
        };
        let (len, modified) = (meta.len(), meta.modified().ok());
        if let Some(c) = self.plans.lock().unwrap().get(agent) {
            if c.len == len && c.modified == modified {
                return c.nodes.as_ref().clone();
            }
        }
        let nodes = node::load_nodes(&path).unwrap_or_default();
        self.plans.lock().unwrap().insert(
            agent.to_string(),
            PlanCache {
                len,
                modified,
                nodes: Arc::new(nodes.clone()),
            },
        );
        nodes
    }

    pub fn plan_attempts(&self, agent: &str) -> Vec<arbos_core::Attempt> {
        node::load_attempts(&self.layout(agent).attempts_jsonl()).unwrap_or_default()
    }

    /// Write one node and tell the window. Callers hold `plan_lock`.
    fn write_node(&self, agent: &str, n: &Node) -> Result<()> {
        node::save_node(&self.layout(agent).plan_jsonl(), n)
    }

    /// The plan as text. Also refreshes `plan.md`.
    pub fn plan_render(&self, agent: &str) -> String {
        let nodes = self.plan_nodes(agent);
        let last = node::last_attempts(&self.plan_attempts(agent));
        let text = node::render(&nodes, &last, arbos_core::now_ms());
        if !nodes.is_empty() {
            let _ = std::fs::write(self.layout(agent).plan_md(), format!("{text}\n"));
        }
        text
    }

    pub fn plan_frame(&self, agent: &str) -> Frame {
        let mut nodes =
            crate::plan::wire_nodes(&self.plan_nodes(agent), &self.plan_attempts(agent));
        // Inbox files ride along as the queued rows the window already
        // draws (`inbox: true`), never as plan nodes.
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

    /// After any plan write: settle parents, render, broadcast, scan.
    pub fn plan_changed(&self, agent: &str) {
        self.settle_parents(agent);
        let _ = self.plan_render(agent);
        self.broadcast(self.plan_frame(agent));
        self.kick();
    }

    /// A goal whose every step is done is done — when it has no check of
    /// its own and nobody is working it. A failed or blocked step leaves
    /// the parent open so the failure stays in view.
    fn settle_parents(&self, agent: &str) {
        use arbos_core::NodeStatus as S;
        let _g = self.plan_lock.lock().unwrap();
        let nodes = self.plan_nodes(agent);
        let now = arbos_core::now_ms();
        let mut changed = true;
        let mut nodes = nodes;
        while changed {
            changed = false;
            let snapshot = nodes.clone();
            for n in nodes.iter_mut() {
                if n.status != S::Pending
                    || !n.check.is_empty()
                    || !matches!(n.do_, arbos_core::Do::Agent)
                {
                    continue;
                }
                let kids: Vec<&Node> = snapshot.iter().filter(|k| k.parent == n.id).collect();
                if kids.is_empty() {
                    continue;
                }
                let all_done = kids
                    .iter()
                    .all(|k| matches!(k.status, S::Done | S::Cancelled) || k.recurring());
                let any_open_standing = kids.iter().any(|k| k.recurring() && !k.terminal());
                if !all_done || any_open_standing {
                    continue;
                }
                n.status = S::Done;
                n.outcome = "every step finished".into();
                n.updated_ms = now;
                let _ = self.write_node(agent, n);
                changed = true;
            }
        }
    }

    /// Append nodes under `parent` (0 = new roots). Returns the ids.
    pub fn plan_add(
        &self,
        agent: &str,
        parent: NodeId,
        new: &[NewNode],
        origin: &str,
    ) -> Result<Vec<NodeId>> {
        if new.is_empty() {
            bail!("plan add: nodes must not be empty");
        }
        let now = arbos_core::now_ms();
        let _g = self.plan_lock.lock().unwrap();
        let nodes = self.plan_nodes(agent);
        if parent != 0 && !nodes.iter().any(|n| n.id == parent) {
            bail!("plan add: no node #{parent}");
        }
        let mut next = node::next_node_id(&nodes);
        let mut ids = Vec::with_capacity(new.len());
        let mut built = Vec::with_capacity(new.len());
        // parent 0 with several nodes starts a plan: the first is the
        // mission root, the rest hang under it in order.
        let (root_spec, rest, under) = if parent == 0 && new.len() > 1 {
            (Some(&new[0]), &new[1..], next)
        } else {
            (None, new, parent)
        };
        if let Some(spec) = root_spec {
            let mut n = spec
                .build(now)
                .map_err(|e| anyhow::anyhow!("plan add: nodes[0]: {e}"))?;
            n.id = next;
            n.parent = 0;
            n.seq = nodes
                .iter()
                .filter(|x| x.parent == 0)
                .map(|x| x.seq + 1)
                .max()
                .unwrap_or(0);
            n.origin = origin.to_string();
            next += 1;
            ids.push(n.id);
            built.push(n);
        }
        let existing_max = nodes
            .iter()
            .filter(|n| n.parent == under)
            .map(|n| n.seq)
            .max();
        let par: Vec<bool> = rest.iter().map(|n| n.par).collect();
        let seqs = node::assign_seqs(existing_max, &par);
        for (i, spec) in rest.iter().enumerate() {
            let mut n = spec.build(now).map_err(|e| {
                anyhow::anyhow!("plan add: nodes[{}]: {e}", i + root_spec.is_some() as usize)
            })?;
            n.id = next;
            n.parent = under;
            n.seq = seqs[i];
            n.origin = origin.to_string();
            next += 1;
            ids.push(n.id);
            built.push(n);
        }
        for n in &built {
            self.write_node(agent, n)?;
        }
        drop(_g);
        self.plan_changed(agent);
        Ok(ids)
    }

    /// A message into `agent`: one root node that fires a turn when ready.
    /// Put a message in an agent's inbox. It is a file
    /// (`inbox/<time>-<from>-<seq>.md`, see `arbos_core::inbox`), not a
    /// plan node: plan nodes are goals. The node passed in is the message
    /// as the callers still build it — goal, origin, attachments, hops —
    /// and is translated. The id returned names the file for the window's
    /// rows (`plan_op cancel|run` on it removes or wakes the file).
    pub fn inbox(&self, agent: &str, n: Node) -> Result<NodeId> {
        if !arbos_core::agent_exists(&self.place, agent) {
            bail!("no agent {agent}");
        }
        // Nothing to say and nothing attached is not a message; storing it
        // would fire a model turn on an empty prompt (QA bug qa-008).
        if n.goal.trim().is_empty() && n.attachments.is_empty() {
            bail!("empty prompt");
        }
        let (from, kind) = match n.origin.as_str() {
            o if o.starts_with("spawn:") => (format!("agent:{}", &o["spawn:".len()..]), "brief"),
            "" | "user" => ("user".to_string(), "request"),
            o => (o.to_string(), "request"),
        };
        let mut msg = inbox::Message::new(from, kind, n.goal.clone());
        msg.wake = true;
        msg.hops = n.hops;
        msg.attachments = n.attachments.clone();
        msg.channel = n.channel.clone();
        let name = inbox::deliver(&self.place, agent, &msg)?;
        self.plan_changed(agent);
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

    /// Move a node through its life. `status` None on a recurring node
    /// records one recurrence. Returns the acknowledgement line.
    pub fn plan_update(
        &self,
        agent: &str,
        id: NodeId,
        status: Option<arbos_core::NodeStatus>,
        outcome: &str,
        by: &str,
    ) -> Result<String> {
        use arbos_core::NodeStatus as S;
        let now = arbos_core::now_ms();
        let outcome = outcome.trim();
        let _g = self.plan_lock.lock().unwrap();
        let nodes = self.plan_nodes(agent);
        let Some(mut n) = nodes.iter().find(|n| n.id == id).cloned() else {
            bail!("plan update: no node #{id}");
        };
        let attempts_path = self.layout(agent).attempts_jsonl();
        let Some(to) = status else {
            if !n.recurring() {
                bail!("plan update: node #{id}: status is required for a one-shot node");
            }
            if outcome.is_empty() {
                bail!("plan update: node #{id}: a recurrence needs an outcome");
            }
            let attempts = self.plan_attempts(agent);
            let a = arbos_core::Attempt {
                id: node::next_attempt_id(&attempts),
                node: id,
                kind: "agent".into(),
                started_ms: now,
                ended_ms: Some(now),
                verdict: Some(arbos_core::Verdict::Success),
                outcome: outcome.to_string(),
                verified_by: by.to_string(),
                transcript_lo: None,
                transcript_hi: None,
                job: None,
            };
            node::save_attempt(&attempts_path, &a)?;
            n.status = S::Pending;
            n.outcome = outcome.to_string();
            n.attempt = None;
            n.updated_ms = now;
            self.write_node(agent, &n)?;
            drop(_g);
            self.plan_changed(agent);
            return Ok(format!("Recorded recurrence of #{id}."));
        };
        node::can_transition(&n, to).map_err(|e| anyhow::anyhow!("plan update: {e}"))?;
        if matches!(to, S::Done | S::Failed | S::Blocked) && outcome.is_empty() {
            bail!(
                "plan update: node #{id}: {} needs an outcome — say what happened or what is needed",
                to.as_str()
            );
        }
        if !outcome.is_empty() {
            n.outcome = outcome.to_string();
        }
        n.status = to;
        n.updated_ms = now;
        if matches!(to, S::Done | S::Failed | S::Cancelled | S::Pending) {
            n.attempt = None;
        }
        if to == S::Done || to == S::Failed {
            let attempts = self.plan_attempts(agent);
            // Close the running attempt if the model is finishing its own
            // node mid-turn; otherwise record a fresh one.
            let open = attempts
                .iter()
                .find(|a| a.node == id && a.running())
                .cloned();
            let mut a = open.unwrap_or(arbos_core::Attempt {
                id: node::next_attempt_id(&attempts),
                node: id,
                kind: "agent".into(),
                started_ms: now,
                ended_ms: None,
                verdict: None,
                outcome: String::new(),
                verified_by: String::new(),
                transcript_lo: None,
                transcript_hi: None,
                job: None,
            });
            a.ended_ms = Some(now);
            a.verdict = Some(if to == S::Done {
                arbos_core::Verdict::Success
            } else {
                arbos_core::Verdict::Fail
            });
            a.outcome = n.outcome.clone();
            a.verified_by = by.to_string();
            node::save_attempt(&attempts_path, &a)?;
        }
        self.write_node(agent, &n)?;
        drop(_g);
        self.plan_changed(agent);
        Ok(format!("#{id} -> {}.", to.as_str()))
    }

    /// `agent` and everything under it, depth-first.
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
        use arbos_core::NodeStatus as S;
        let now = arbos_core::now_ms();
        let ids = self.descendants(agent);
        for id in &ids {
            let root = arbos_engine::JobsRoot::for_agent(&self.place, &AgentId::new(id));
            for job in root.list() {
                root.kill(&job);
            }
            let layout = self.layout(id);
            let _g = self.plan_lock.lock().unwrap();
            let mut changed = false;
            for mut n in self.plan_nodes(id) {
                let scheduled = n.armed() || n.gated();
                if !scheduled || n.terminal() || n.status == S::Blocked {
                    continue;
                }
                // An inbox message that has not run yet is a queued prompt;
                // stopping the agent drops it too.
                if node::is_inbox(&[], &n) && n.status == S::Pending && n.parent == 0 {
                    n.status = S::Cancelled;
                } else {
                    n.status = S::Blocked;
                }
                n.attempt = None;
                n.outcome = "stopped by the user — press run to resume".into();
                n.updated_ms = now;
                let _ = node::save_node(&layout.plan_jsonl(), &n);
                changed = true;
            }
            drop(_g);
            if changed {
                self.plan_changed(id);
            }
        }
        ids
    }

    /// A window action on a node.
    pub fn plan_op(&self, agent: &str, id: NodeId, op: &str, text: &str) -> Result<()> {
        use arbos_core::NodeStatus as S;
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
        match op {
            "cancel" => {
                let _ = self.plan_update(
                    agent,
                    id,
                    Some(S::Cancelled),
                    "cancelled from the window",
                    "user",
                );
            }
            "reopen" => {
                let _ = self.plan_update(
                    agent,
                    id,
                    Some(S::Pending),
                    "reopened from the window",
                    "user",
                );
            }
            "run" => {
                let now = arbos_core::now_ms();
                let _g = self.plan_lock.lock().unwrap();
                let nodes = self.plan_nodes(agent);
                let Some(mut n) = nodes.iter().find(|n| n.id == id).cloned() else {
                    bail!("no node #{id}");
                };
                if n.status != S::Pending {
                    n.status = S::Pending;
                    n.attempt = None;
                    n.outcome.clear();
                }
                n.when.after_ms = None;
                if n.recurring() {
                    n.when.next_due_ms = Some(now);
                }
                if matches!(n.do_, arbos_core::Do::Agent) {
                    n.when.wake = true;
                }
                n.updated_ms = now;
                self.write_node(agent, &n)?;
                drop(_g);
                self.plan_changed(agent);
            }
            "answer" => {
                let text = text.trim();
                if text.is_empty() {
                    bail!("an answer needs text");
                }
                let _ = self.plan_update(agent, id, Some(S::Done), text, "user");
            }
            other => bail!("unknown plan op {other}"),
        }
        Ok(())
    }
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
        // Two children with the same brief get distinct ids (`-2`, `-3`, …)
        // rather than the second one failing.
        let base = slug(brief);
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
        child.name = brief.chars().take(48).collect();
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
        // The brief is the child's first node: its mission root. It fires
        // a turn now with the mission spelled out; the child decomposes
        // under it with plan add and reports back with say.
        let mut n = Node::inbox(brief, format!("spawn:{}", parent.id));
        n.hops = DEFAULT_HOPS;
        self.inbox(&id, n)?;
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
                DEFAULT_HOPS
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
            DEFAULT_HOPS
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
        let mut n = Node::inbox(text, format!("agent:{from}"));
        n.hops = hops;
        self.inbox(tid, n)?;
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
            node::clock(arbos_core::now_ms()),
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
    pub fn ask(
        &self,
        agent: &AgentId,
        question: &str,
        options: &[String],
        call_id: &str,
    ) -> Result<oneshot::Receiver<String>> {
        let (tx, rx) = oneshot::channel();
        let id = if call_id.is_empty() {
            format!("ask-{}", arbos_core::now_ms())
        } else {
            call_id.to_string()
        };
        self.issued.lock().unwrap().insert(id.clone());
        self.asks
            .lock()
            .unwrap()
            .insert(agent.to_string(), (id.clone(), tx));
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
            id: Some(id),
        });
        Ok(rx)
    }

    /// Post an allow/deny prompt. The receiver resolves when the user answers.
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
        self.broadcast(Frame::Ask {
            agent: agent.to_string(),
            question: format!("allow {tool}: {command}"),
            options: vec!["allow".into(), "deny".into()],
            id: Some(id),
        });
        rx
    }

    /// Whether an answer may resolve `agent`'s pending question. `given` is
    /// the id the client sent (empty = none). Ok(pending id) or the reason.
    pub fn answer_allowed(
        &self,
        agent: &str,
        given: &str,
        pending: Option<&str>,
        pending_total: usize,
    ) -> std::result::Result<(), String> {
        let Some(pending) = pending else {
            return Err(format!("no question is pending for {agent}"));
        };
        // Blind: no id, or the agent id (what the desktop's approve sent
        // before asks had ids). Only safe when there is exactly one
        // question it could mean.
        if given.is_empty() || given == agent {
            if pending_total == 1 {
                return Ok(());
            }
            return Err(format!(
                "answer without an ask id while {pending_total} questions are pending; send id {pending:?}"
            ));
        }
        if !self.issued.lock().unwrap().contains(given) {
            // An id this kernel never issued is a client's mistake, not a
            // late answer to another question. With one question pending
            // it can only mean that one: take it rather than leave the user
            // with no card and a stuck turn (ui-004).
            if pending_total == 1 {
                crate::klog::warn(
                    "answer_id_unknown",
                    Some(agent),
                    format!(
                        "answer names ask {given:?} (never issued); taken for the one pending question {pending:?}"
                    ),
                );
                return Ok(());
            }
            return Err(format!(
                "answer names ask {given:?}, which this kernel never issued; the pending question is {pending:?}"
            ));
        }
        if given != pending {
            return Err(format!(
                "answer names ask {given:?} but the pending question is {pending:?}; a late or duplicate answer resolves nothing"
            ));
        }
        Ok(())
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
