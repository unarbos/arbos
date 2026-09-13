//! The plan is the wake queue.
//!
//! A [`Node`] is one goal an agent holds. It is stored; a [`crate::Wake`]
//! is derived from it when its moment comes. A user prompt, a `say`
//! request, a spawn brief, a cron firing, a callback — every one is a node
//! in the folder of the agent that does it.
//!
//! Two axes describe a node. [`When`] says when it becomes runnable.
//! [`Do`] says what discharges it. Almost every pair is meaningful.
//!
//! An [`Attempt`] is one execution. Attempts are append-only: a failed
//! attempt is knowledge, and the last one is the working memory the next
//! firing reads.
//!
//! Files, per agent:
//!
//! ```text
//! plan.jsonl      one Node per line; the last line for an id wins
//! attempts.jsonl  one Attempt per line, append-only; the last line for an id wins
//! plan.md         the human rendering, written by the kernel
//! ```

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::{BufRead, BufReader, Write},
    path::Path,
};

pub type NodeId = u64;

/// Floor on a recurrence. The kernel scans on a coarse tick, and a finer
/// period would be a runaway spend loop when the executor is the model.
pub const MIN_EVERY_MS: u64 = 30_000;
/// Reply budget a fresh agent-to-agent request carries.
pub const DEFAULT_HOPS: u8 = 3;
/// Most characters the rendered plan may take in a prompt.
pub const RENDER_BUDGET: usize = 2_400;
/// Most bytes of command output that ride into an outcome.
pub const TAIL_LIMIT: usize = 1_200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Active,
    Blocked,
    Done,
    Cancelled,
    Failed,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "pending" => Self::Pending,
            "active" => Self::Active,
            "blocked" => Self::Blocked,
            "done" => Self::Done,
            "cancelled" | "canceled" => Self::Cancelled,
            "failed" => Self::Failed,
            _ => return None,
        })
    }

    /// No further work is possible from here.
    pub fn terminal(self) -> bool {
        matches!(self, Self::Done | Self::Cancelled | Self::Failed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Success,
    Fail,
    Inconclusive,
}

/// The trigger axis: when the node becomes runnable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct When {
    /// Not ready before this instant. Cleared when it fires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_ms: Option<i64>,
    /// Recurrence period. A recurring node never terminates on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every_ms: Option<u64>,
    /// Next firing of a recurring node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_due_ms: Option<i64>,
    /// Fire a turn the moment the node is ready. Without it a ready agent
    /// task waits for a running turn to pick it up. The inbox is this flag:
    /// a user prompt, a request, a spawn brief all carry it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub wake: bool,
    /// A shell predicate checked on the `every` cadence. The node's `do`
    /// fires only when it exits 0.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub condition: String,
}

/// The executor axis: what discharges the node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Do {
    /// A model turn of the agent that holds the node.
    Agent,
    /// The kernel runs a command. Exit code is the verdict. No model.
    /// With `report`, the command's output is delivered to the node's
    /// origin after each successful run; `{output}` in the text is
    /// replaced by it, or the output follows the text.
    Shell {
        cmd: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        report: Option<String>,
    },
    /// The kernel delivers a message to the node's origin. No model.
    Notify { text: String },
    /// A question only the user can resolve. Parks until answered.
    Ask,
}

impl Default for Do {
    fn default() -> Self {
        Self::Agent
    }
}

impl Do {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Shell { .. } => "shell",
            Self::Notify { .. } => "notify",
            Self::Ask => "ask",
        }
    }

    /// The kernel can discharge it without a model turn.
    pub fn mechanical(&self) -> bool {
        matches!(self, Self::Shell { .. } | Self::Notify { .. })
    }
}

/// One goal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    /// 0 for a root.
    #[serde(default)]
    pub parent: NodeId,
    /// Order among siblings. Earlier siblings gate later ones. Equal seq
    /// is a parallel group.
    #[serde(default)]
    pub seq: u32,
    pub goal: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub check: String,
    #[serde(default)]
    pub when: When,
    #[serde(rename = "do", default)]
    pub do_: Do,
    pub status: Status,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub outcome: String,
    /// Who asked: `user`, `agent:<id>`, `node:<id>`, `kernel`.
    #[serde(default)]
    pub origin: String,
    /// Reply budget carried into the turn this node starts.
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub hops: u8,
    /// The attempt that holds it active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<String>,
    /// Files that ride along with an inbox node.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<String>,
    #[serde(default)]
    pub created_ms: i64,
    #[serde(default)]
    pub updated_ms: i64,
}

fn is_zero_u8(n: &u8) -> bool {
    *n == 0
}

impl Node {
    pub fn new(goal: impl Into<String>) -> Self {
        let now = crate::now_ms();
        Self {
            id: 0,
            parent: 0,
            seq: 0,
            goal: goal.into(),
            check: String::new(),
            when: When::default(),
            do_: Do::Agent,
            status: Status::Pending,
            outcome: String::new(),
            origin: String::new(),
            hops: 0,
            attempt: None,
            attachments: Vec::new(),
            created_ms: now,
            updated_ms: now,
        }
    }

    /// An inbox node: fire a turn as soon as it is ungated.
    pub fn inbox(goal: impl Into<String>, origin: impl Into<String>) -> Self {
        let mut n = Self::new(goal);
        n.when.wake = true;
        n.origin = origin.into();
        n
    }

    pub fn recurring(&self) -> bool {
        self.when.every_ms.is_some()
    }

    pub fn gated(&self) -> bool {
        !self.when.condition.is_empty()
    }

    /// The clock owns a trigger on it.
    pub fn armed(&self) -> bool {
        self.recurring() || self.when.after_ms.is_some() || self.when.wake
    }

    pub fn terminal(&self) -> bool {
        self.status.terminal()
    }

    /// The instant a timed node became or becomes due.
    pub fn due_at(&self) -> i64 {
        if self.recurring() {
            self.when.next_due_ms.unwrap_or(i64::MAX)
        } else {
            self.when.after_ms.unwrap_or(0)
        }
    }
}

/// One execution of a node. Two lines per attempt: one when it starts, one
/// when it ends. The last line for an id wins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempt {
    pub id: String,
    pub node: NodeId,
    /// `agent`, `shell`, `notify`, `condition`.
    pub kind: String,
    pub started_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub outcome: String,
    /// `exit` for a process, `kernel` for a notify, `self` for the model.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub verified_by: String,
    /// Transcript lines this attempt wrote, 1-based inclusive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_lo: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_hi: Option<u64>,
    /// The job folder for a shell or condition run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
}

impl Attempt {
    pub fn running(&self) -> bool {
        self.ended_ms.is_none()
    }
}

// ── files ───────────────────────────────────────────────────────────────

fn append_line<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut buf = serde_json::to_vec(value)?;
    buf.push(b'\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    file.write_all(&buf)?;
    file.sync_data()?;
    Ok(())
}

fn fold_lines<T: for<'de> Deserialize<'de>>(
    path: &Path,
    key: impl Fn(&T) -> String,
) -> Result<Vec<T>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = std::fs::File::open(path)?;
    let mut order: Vec<String> = Vec::new();
    let mut latest: HashMap<String, T> = HashMap::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<T>(&line) else {
            continue;
        };
        let k = key(&value);
        if !latest.contains_key(&k) {
            order.push(k.clone());
        }
        latest.insert(k, value);
    }
    Ok(order
        .into_iter()
        .filter_map(|k| latest.remove(&k))
        .collect())
}

/// Every node, latest state per id, in first-seen order.
pub fn load_nodes(path: &Path) -> Result<Vec<Node>> {
    let mut nodes = fold_lines::<Node>(path, |n| n.id.to_string())?;
    nodes.sort_by_key(|n| (n.parent, n.seq, n.id));
    Ok(nodes)
}

/// Write a node's current state. An upsert: the file is a log, the id wins.
pub fn save_node(path: &Path, node: &Node) -> Result<()> {
    append_line(path, node)
}

/// Rewrite the log with only the latest line per id. Run at kernel start.
pub fn compact_nodes(path: &Path) -> Result<()> {
    let nodes = load_nodes(path)?;
    if nodes.is_empty() {
        return Ok(());
    }
    let tmp = path.with_extension("jsonl.tmp");
    {
        let mut file = std::fs::File::create(&tmp)?;
        for n in &nodes {
            serde_json::to_writer(&mut file, n)?;
            file.write_all(b"\n")?;
        }
        file.sync_data()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn load_attempts(path: &Path) -> Result<Vec<Attempt>> {
    fold_lines::<Attempt>(path, |a| a.id.clone())
}

pub fn save_attempt(path: &Path, attempt: &Attempt) -> Result<()> {
    append_line(path, attempt)
}

/// The most recent attempt per node.
pub fn last_attempts(attempts: &[Attempt]) -> HashMap<NodeId, Attempt> {
    let mut out: HashMap<NodeId, Attempt> = HashMap::new();
    for a in attempts {
        let newer = out
            .get(&a.node)
            .is_none_or(|prev| a.started_ms >= prev.started_ms);
        if newer {
            out.insert(a.node, a.clone());
        }
    }
    out
}

pub fn next_node_id(nodes: &[Node]) -> NodeId {
    nodes.iter().map(|n| n.id).max().unwrap_or(0) + 1
}

pub fn next_attempt_id(attempts: &[Attempt]) -> String {
    let n = attempts
        .iter()
        .filter_map(|a| a.id.strip_prefix('a')?.parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    format!("a{}", n + 1)
}

// ── pure rules ──────────────────────────────────────────────────────────

/// Sibling slots for nodes appended under one parent. `existing_max` is
/// the parent's current max seq, or `None` with no children yet. A `par`
/// node shares the previous slot: three `par` then one plain reads
/// `1,1,1,2` — fan out, then join.
pub fn assign_seqs(existing_max: Option<u32>, par: &[bool]) -> Vec<u32> {
    let mut out = Vec::with_capacity(par.len());
    let mut next = existing_max.map(|m| m + 1).unwrap_or(0);
    let mut prev = existing_max;
    for &p in par {
        let seq = match (p, prev) {
            (true, Some(prev)) => prev,
            _ => next,
        };
        out.push(seq);
        prev = Some(seq);
        next = seq + 1;
    }
    out
}

/// The legal status graph. `done -> pending` reopens a goal (done is a
/// cached claim). Nothing leaves `cancelled`.
pub fn can_transition(n: &Node, to: Status) -> Result<()> {
    use Status::*;
    if to == n.status {
        bail!("node #{} is already {}", n.id, to.as_str());
    }
    if n.recurring() && matches!(to, Done | Failed) {
        bail!(
            "node #{} recurs: it has no terminal success or failure. Record a recurrence with an outcome-only update; cancel it when its scope ends",
            n.id
        );
    }
    let ok = match n.status {
        Pending => matches!(to, Active | Blocked | Done | Failed | Cancelled),
        Active => matches!(to, Done | Failed | Blocked | Pending | Cancelled),
        Blocked => matches!(to, Pending | Active | Cancelled),
        Done => matches!(to, Pending),
        Failed => matches!(to, Pending | Active | Cancelled),
        Cancelled => false,
    };
    if !ok {
        bail!(
            "node #{} cannot go {} -> {}",
            n.id,
            n.status.as_str(),
            to.as_str()
        );
    }
    Ok(())
}

/// An earlier one-shot sibling is still open. Recurring siblings never
/// gate: standing obligations run beside the work. Roots never gate each
/// other: each root is its own plan, and the inbox is a list of roots.
pub fn gated_by_sibling(all: &[Node], n: &Node) -> bool {
    if n.parent == 0 {
        return false;
    }
    all.iter().any(|s| {
        s.parent == n.parent && s.seq < n.seq && !s.recurring() && !s.terminal() && s.id != n.id
    })
}

/// The node can be worked on now.
pub fn ready(n: &Node, gated: bool, now_ms: i64) -> bool {
    if n.status != Status::Pending || matches!(n.do_, Do::Ask) || gated {
        return false;
    }
    if n.when.after_ms.is_some_and(|t| now_ms < t) {
        return false;
    }
    if n.recurring() {
        return n.when.next_due_ms.is_some_and(|t| {
            // Due, or scheduled further out than one period: the clock moved
            // back (or the file was edited) and that instant is unreachable
            // on the current clock. Firing now re-arms it from now (qa-013).
            now_ms >= t || t - now_ms > n.when.every_ms.unwrap_or(0) as i64
        });
    }
    true
}

/// Why the model is summoned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeReason {
    Due,
    Ready,
    Condition,
    CmdFailed,
}

/// What the kernel can advance right now, for one agent's nodes.
#[derive(Debug, Default)]
pub struct Fireable {
    /// Shell and notify nodes: the kernel runs them, no model.
    pub mech: Vec<Node>,
    /// Gated nodes: evaluate the predicate first.
    pub conds: Vec<Node>,
    /// Agent nodes the clock fires, due order then seq then id.
    pub wakes: Vec<(Node, WakeReason)>,
}

pub fn fireable(nodes: &[Node], now_ms: i64) -> Fireable {
    let mut out = Fireable::default();
    for n in nodes {
        if !ready(n, gated_by_sibling(nodes, n), now_ms) {
            continue;
        }
        if n.gated() {
            out.conds.push(n.clone());
            continue;
        }
        if n.do_.mechanical() {
            out.mech.push(n.clone());
            continue;
        }
        if n.recurring() || n.when.after_ms.is_some() {
            out.wakes.push((n.clone(), WakeReason::Due));
        } else if n.when.wake {
            out.wakes.push((n.clone(), WakeReason::Ready));
        }
    }
    out.wakes
        .sort_by_key(|(n, _)| (n.due_at(), n.parent, n.seq, n.id));
    out
}

/// Parse `30m`, `2h`, `90s`, `1d`, or plain seconds, into milliseconds.
pub fn parse_duration_ms(s: &str) -> Option<u64> {
    let s = s.trim().to_ascii_lowercase();
    if s.is_empty() {
        return None;
    }
    let (num, unit) = match s.find(|c: char| c.is_ascii_alphabetic()) {
        Some(i) => s.split_at(i),
        None => (s.as_str(), "s"),
    };
    let n: f64 = num.trim().parse().ok()?;
    if n <= 0.0 {
        return None;
    }
    let mult = match unit.trim() {
        "ms" => 1.0,
        "s" | "sec" | "secs" => 1_000.0,
        "m" | "min" | "mins" => 60_000.0,
        "h" | "hr" | "hrs" => 3_600_000.0,
        "d" | "day" | "days" => 86_400_000.0,
        _ => return None,
    };
    Some((n * mult) as u64)
}

pub fn human_ms(ms: u64) -> String {
    let s = ms / 1000;
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86_400)
    }
}

/// `HH:MM` local time for a unix millisecond stamp.
pub fn clock(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let offset = local_offset_secs();
    let day = (secs + offset).rem_euclid(86_400);
    format!("{:02}:{:02}", day / 3600, (day % 3600) / 60)
}

fn local_offset_secs() -> i64 {
    // The kernel runs where the user sits; the TZ offset is what `date +%z`
    // says. Read once.
    use std::sync::OnceLock;
    static OFF: OnceLock<i64> = OnceLock::new();
    *OFF.get_or_init(|| {
        std::process::Command::new("date")
            .arg("+%z")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| {
                let s = s.trim();
                let sign = if s.starts_with('-') { -1 } else { 1 };
                let digits = s.trim_start_matches(['+', '-']);
                let h: i64 = digits.get(0..2)?.parse().ok()?;
                let m: i64 = digits.get(2..4)?.parse().ok()?;
                Some(sign * (h * 3600 + m * 60))
            })
            .unwrap_or(0)
    })
}

// ── render ──────────────────────────────────────────────────────────────

pub const NO_PLAN: &str = "(no plan)";

/// The plan as the model and the human read it. Roots first with a done
/// count; open nodes depth-first; standing obligations; questions for the
/// user. Each open node shows its last attempt's outcome: that is the
/// working memory across firings.
pub fn render(nodes: &[Node], last: &HashMap<NodeId, Attempt>, now_ms: i64) -> String {
    let open: Vec<&Node> = nodes.iter().filter(|n| !is_inbox(nodes, n)).collect();
    if open.is_empty() {
        return NO_PLAN.into();
    }
    let mut children: HashMap<NodeId, Vec<&Node>> = HashMap::new();
    let mut roots: Vec<&Node> = Vec::new();
    for n in &open {
        if n.parent == 0 {
            roots.push(n);
        } else {
            children.entry(n.parent).or_default().push(n);
        }
    }
    let mut b = String::new();
    for root in roots {
        // A finished one-shot root with no open children is history.
        if root.terminal() && !subtree_open(root.id, &children) {
            continue;
        }
        render_plan(&mut b, root, &children, last, now_ms);
        if b.len() > RENDER_BUDGET {
            b.push_str("… (plan truncated; use plan op:show for the rest)\n");
            break;
        }
    }
    let out = b.trim_end().to_string();
    if out.is_empty() { NO_PLAN.into() } else { out }
}

/// A message into the agent — a user prompt, a peer's request, a kernel
/// summons — is transcript, not plan, unless the agent hung children under
/// it (then it is a mission root).
pub fn is_inbox(all: &[Node], n: &Node) -> bool {
    n.parent == 0
        && n.when.wake
        && !n.recurring()
        && n.check.is_empty()
        && matches!(n.do_, Do::Agent)
        && (n.origin == "user"
            || n.origin == "kernel"
            || n.origin.starts_with("agent:")
            || n.origin.starts_with("spawn:"))
        && !all.iter().any(|k| k.parent == n.id)
}

fn subtree_open(id: NodeId, children: &HashMap<NodeId, Vec<&Node>>) -> bool {
    children.get(&id).is_some_and(|kids| {
        kids.iter()
            .any(|k| !k.terminal() || subtree_open(k.id, children))
    })
}

fn render_plan(
    b: &mut String,
    root: &Node,
    children: &HashMap<NodeId, Vec<&Node>>,
    last: &HashMap<NodeId, Attempt>,
    now_ms: i64,
) {
    let (done, total) = count(root.id, children);
    b.push_str(&format!("#{} {}", root.id, clip(&root.goal, 140)));
    if total > 0 {
        b.push_str(&format!("  ({done}/{total} done)"));
    }
    if root.status != Status::Pending && root.status != Status::Active {
        b.push_str(&format!("  [{}]", root.status.as_str()));
    }
    if root.recurring() && root.status == Status::Pending {
        b.push_str(&due_suffix(root, now_ms));
    }
    if matches!(root.status, Status::Blocked | Status::Failed) && !root.outcome.is_empty() {
        b.push_str(&format!("  — {}", clip(&root.outcome, 100)));
    }
    let childless = children.get(&root.id).is_none_or(|k| k.is_empty());
    // A lone goal with nothing to fire it is pull mode: it runs only when a
    // turn works it. Say so, or it reads as scheduled.
    if childless
        && root.status == Status::Pending
        && matches!(root.do_, Do::Agent)
        && !root.armed()
        && root.when.condition.is_empty()
    {
        b.push_str(
            "  (no trigger — runs only when you work it; use when.every/after/wake to schedule)",
        );
    }
    b.push('\n');
    if childless {
        write_last(b, last, root.id, "  ");
    }
    let mut standing = Vec::new();
    let mut human = Vec::new();
    render_open(
        b,
        root.id,
        children,
        last,
        now_ms,
        1,
        &mut standing,
        &mut human,
    );
    if !standing.is_empty() {
        b.push_str("  standing:\n");
        for n in standing {
            b.push_str(&format!(
                "    #{} {}{}{}\n",
                n.id,
                clip(&n.goal, 110),
                do_suffix(n),
                due_suffix(n, now_ms)
            ));
            write_last(b, last, n.id, "      ");
        }
    }
    if !human.is_empty() {
        b.push_str("  waiting on the user:\n");
        for n in human {
            b.push_str(&format!("    #{} {}\n", n.id, clip(&n.goal, 110)));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_open<'a>(
    b: &mut String,
    parent: NodeId,
    children: &HashMap<NodeId, Vec<&'a Node>>,
    last: &HashMap<NodeId, Attempt>,
    now_ms: i64,
    depth: usize,
    standing: &mut Vec<&'a Node>,
    human: &mut Vec<&'a Node>,
) {
    let Some(sibs) = children.get(&parent) else {
        return;
    };
    let all: Vec<Node> = sibs.iter().map(|n| (*n).clone()).collect();
    for n in sibs {
        if n.recurring() {
            if !n.terminal() {
                standing.push(n);
            }
            continue;
        }
        if matches!(n.do_, Do::Ask) {
            if !n.terminal() {
                human.push(n);
            }
            continue;
        }
        // Done and cancelled fold into the parent's count. A failure stays
        // visible until someone reopens or cancels it.
        if matches!(n.status, Status::Done | Status::Cancelled) {
            continue;
        }
        let mut marker = n.status.as_str().to_string();
        if ready(n, gated_by_sibling(&all, n), now_ms) {
            marker = if n.when.after_ms.is_some() {
                "due"
            } else {
                "ready"
            }
            .into();
        }
        let indent = "  ".repeat(depth);
        b.push_str(&format!(
            "{indent}#{} [{marker}] {}",
            n.id,
            clip(&n.goal, 120)
        ));
        if n.status == Status::Pending {
            if let Some(t) = n.when.after_ms.filter(|t| now_ms < *t) {
                b.push_str(&format!("  (fires ~{})", clock(t)));
            }
        }
        b.push_str(&do_suffix(n));
        if n.when.wake && matches!(n.do_, Do::Agent) && n.origin.is_empty() {
            b.push_str("  (callback)");
        }
        if !n.when.condition.is_empty() {
            b.push_str(&format!("  (if: {})", clip(&n.when.condition, 48)));
        }
        if !n.check.is_empty() {
            b.push_str(&format!("  (check: {})", clip(&n.check, 60)));
        }
        if matches!(n.status, Status::Blocked | Status::Failed) && !n.outcome.is_empty() {
            b.push_str(&format!("  — {}", clip(&n.outcome, 100)));
        }
        b.push('\n');
        write_last(b, last, n.id, &format!("{indent}    "));
        render_open(b, n.id, children, last, now_ms, depth + 1, standing, human);
    }
}

fn do_suffix(n: &Node) -> String {
    match &n.do_ {
        Do::Shell { cmd, report } => format!(
            "  (shell: {}{})",
            clip(cmd, 48),
            if report.is_some() {
                " → reports output"
            } else {
                ""
            }
        ),
        Do::Notify { text } => format!("  (notify: {})", clip(text, 48)),
        _ => String::new(),
    }
}

fn write_last(b: &mut String, last: &HashMap<NodeId, Attempt>, id: NodeId, indent: &str) {
    let Some(a) = last.get(&id) else {
        return;
    };
    if a.outcome.is_empty() {
        return;
    }
    let suffix = match a.verdict {
        Some(Verdict::Success) | None => String::new(),
        Some(v) => format!(" — {}", verdict_str(v)),
    };
    b.push_str(&format!(
        "{indent}last: {} ({}{suffix})\n",
        clip(&a.outcome, 110),
        clock(a.ended_ms.unwrap_or(a.started_ms))
    ));
}

pub fn verdict_str(v: Verdict) -> &'static str {
    match v {
        Verdict::Success => "success",
        Verdict::Fail => "fail",
        Verdict::Inconclusive => "inconclusive",
    }
}

fn count(parent: NodeId, children: &HashMap<NodeId, Vec<&Node>>) -> (usize, usize) {
    let mut done = 0;
    let mut total = 0;
    if let Some(kids) = children.get(&parent) {
        for n in kids {
            if !n.recurring() && !matches!(n.do_, Do::Ask) {
                total += 1;
                if n.status == Status::Done {
                    done += 1;
                }
            }
            let (d, t) = count(n.id, children);
            done += d;
            total += t;
        }
    }
    (done, total)
}

fn due_suffix(n: &Node, now_ms: i64) -> String {
    match n.when.next_due_ms {
        None => String::new(),
        Some(t) if now_ms >= t => " · due now".into(),
        Some(t) => format!(" · next {}", clock(t)),
    }
}

/// One line, at most `n` chars, with an ellipsis.
pub fn clip(s: &str, n: usize) -> String {
    let first = s.lines().next().unwrap_or("");
    let more = s.lines().nth(1).is_some();
    let mut out: String = first.chars().take(n).collect();
    if first.chars().count() > n || more {
        out.push('…');
    }
    out
}

/// The tail of a command's output that rides into an outcome.
pub fn tail(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() <= TAIL_LIMIT {
        return s.to_string();
    }
    let skip = s.chars().count() - TAIL_LIMIT;
    format!("…{}", s.chars().skip(skip).collect::<String>())
}
