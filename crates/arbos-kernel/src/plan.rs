//! The clock. The one place where time starts work instead of answering it.
//!
//! Each scan reads every agent's `plan.jsonl`, finds what can fire, claims
//! it on disk, then acts: shell and notify nodes run here with no model;
//! gated nodes get their predicate checked; agent nodes become one
//! [`Wake`] each (one turn per agent at a time). Everything is written
//! before it runs, so a kernel that dies mid-firing leaves an honest
//! `active` row that the next start reclaims.

use arbos_core::{
    Agent, Attempt, Do, Event, EventKind, Node, NodeId, NodeStatus as Status, Verdict, Wake,
    WakeKind, append_event, inbox, list_agents, load_transcript, needs_serve,
    node::{self, WakeReason},
    wire::PlanNode,
};
use arbos_engine::JobsRoot;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::hooks::KernelHooks;

/// Most shell and condition runs at once across the place.
const MAX_MECH: usize = 8;
/// One kernel-run command may take this long.
const CMD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// A turn the clock started, so its end can close the node.
#[derive(Debug, Clone)]
pub struct TurnMeta {
    pub node: NodeId,
    pub attempt: String,
    /// First transcript line the turn may have written.
    pub lo: u64,
}

#[derive(Default)]
pub struct Clock {
    /// `agent#node` of shell/condition runs in flight.
    mech: Mutex<HashSet<String>>,
    /// Agent → the plan turn running for it.
    turns: Mutex<HashMap<String, TurnMeta>>,
    /// `agent#node` whose claim could not be written, and when to try
    /// again. Without it a node whose rewrite fails (disk full, size
    /// limit) is re-claimed every tick, each time leaving an open attempt
    /// (QA bug qa-018).
    backoff: Mutex<HashMap<String, i64>>,
}

/// How long a node waits after its claim could not be written.
const CLAIM_BACKOFF_MS: i64 = 60_000;

impl Clock {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn turn_for(&self, agent: &str) -> Option<TurnMeta> {
        self.turns.lock().unwrap().get(agent).cloned()
    }

    /// Anything of the plan's in flight: a shell/condition run or a turn.
    pub fn busy(&self) -> bool {
        !self.mech.lock().unwrap().is_empty() || !self.turns.lock().unwrap().is_empty()
    }
}

/// Kernel start: nodes left `active` by a dead kernel are settled. A node
/// whose turn had started (the transcript holds an unfinished wake) is
/// closed — the serve wake continues that turn from the transcript, and
/// refiring would repeat the message. Anything else goes back to pending.
/// Also folds each `plan.jsonl` to one line per id.
pub fn reclaim(hooks: &KernelHooks) {
    let now = arbos_core::now_ms();
    for agent in list_agents(&hooks.place).unwrap_or_default() {
        let id = agent.id.as_str();
        let layout = hooks.layout(id);
        let _ = node::compact_nodes(&layout.plan_jsonl());
        migrate_inbox_nodes(hooks, id, now);
        let continued = needs_serve(&hooks.place, id);
        let _g = hooks.plan_lock.lock().unwrap();
        let nodes = hooks.plan_nodes(id);
        let attempts = hooks.plan_attempts(id);
        let mut changed = false;
        for mut n in nodes {
            if n.status != Status::Active {
                continue;
            }
            let mid_turn = continued && matches!(n.do_, Do::Agent) && !n.gated();
            let outcome = if mid_turn {
                "kernel restarted mid-turn; the turn was continued from the transcript"
            } else {
                "kernel restarted before this finished"
            };
            for a in attempts.iter().filter(|a| a.node == n.id && a.running()) {
                let mut a = a.clone();
                a.ended_ms = Some(now);
                a.verdict = Some(Verdict::Inconclusive);
                a.outcome = outcome.into();
                let _ = node::save_attempt(&layout.attempts_jsonl(), &a);
            }
            n.status = if mid_turn && !n.recurring() {
                Status::Done
            } else {
                Status::Pending
            };
            n.attempt = None;
            n.outcome = outcome.into();
            n.updated_ms = now;
            let _ = node::save_node(&layout.plan_jsonl(), &n);
            changed = true;
        }
        drop(_g);
        if changed {
            hooks.plan_changed(agent.id.as_str());
        } else {
            let _ = hooks.plan_render(agent.id.as_str());
        }
    }
}

/// Kernels before Phase 2 kept messages as plan nodes ("inbox nodes").
/// Each pending one becomes an inbox file with the same words and
/// sender, and the node closes as cancelled with a note saying where it
/// went, so nothing waiting is lost across the upgrade and no node ever
/// fires as a message again.
fn migrate_inbox_nodes(hooks: &KernelHooks, agent: &str, now: i64) {
    let _g = hooks.plan_lock.lock().unwrap();
    let nodes = hooks.plan_nodes(agent);
    let layout = hooks.layout(agent);
    let mut moved = 0;
    for mut n in nodes.clone() {
        if n.status != Status::Pending || !node::is_inbox(&nodes, &n) {
            continue;
        }
        let (from, kind) = match n.origin.as_str() {
            o if o.starts_with("spawn:") => (format!("agent:{}", &o["spawn:".len()..]), "brief"),
            "" | "user" => ("user".to_string(), "request"),
            o => (o.to_string(), "request"),
        };
        let mut msg = inbox::Message::new(from, kind, n.goal.clone());
        msg.hops = n.hops;
        msg.attachments = n.attachments.clone();
        msg.sent = inbox::rfc3339(if n.created_ms > 0 { n.created_ms } else { now });
        match inbox::deliver(&hooks.place, agent, &msg) {
            Ok(name) => {
                n.status = Status::Cancelled;
                n.outcome = format!("moved to inbox/{name} (messages are files now)");
                n.updated_ms = now;
                let _ = node::save_node(&layout.plan_jsonl(), &n);
                moved += 1;
            }
            Err(e) => crate::klog::warn(
                "inbox_migrate_failed",
                Some(agent),
                format!("node #{}: {e:#}", n.id),
            ),
        }
    }
    if moved > 0 {
        crate::klog::info(
            "inbox_migrated",
            Some(agent),
            format!("{moved} message node(s) became inbox files"),
        );
    }
}

/// One pass over every plan. Mechanical work is spawned here. Agent wakes
/// are claimed and returned for the serve loop to start.
pub fn scan(hooks: &Arc<KernelHooks>, clock: &Arc<Clock>) -> Vec<Wake> {
    let now = arbos_core::now_ms();
    let mut wakes = Vec::new();
    for agent in list_agents(&hooks.place).unwrap_or_default() {
        if agent.paused {
            continue;
        }
        let id = agent.id.as_str();
        let nodes = hooks.plan_nodes(id);
        // An empty plan still has an inbox.
        if nodes.is_empty() && inbox::list(&hooks.place, id).is_empty() {
            continue;
        }
        let fire = node::fireable(&nodes, now);
        for n in fire.mech.into_iter().chain(fire.conds.into_iter()) {
            let key = format!("{id}#{}", n.id);
            {
                let mut mech = clock.mech.lock().unwrap();
                if mech.len() >= MAX_MECH || mech.contains(&key) {
                    continue;
                }
                mech.insert(key.clone());
            }
            let Some((n, attempt)) = claim(hooks, clock, id, n, now) else {
                clock.mech.lock().unwrap().remove(&key);
                continue;
            };
            let hooks = Arc::clone(hooks);
            let clock = Arc::clone(clock);
            let agent = agent.clone();
            tokio::spawn(async move {
                let node_id = n.id;
                let goal: String = n.goal.chars().take(80).collect();
                if n.gated() {
                    run_condition(&hooks, &agent, n, attempt).await;
                } else {
                    run_mechanical(&hooks, &agent, n, attempt).await;
                }
                clock.mech.lock().unwrap().remove(&key);
                // A cron or shell node that ran without a turn is a commit too.
                crate::snapshot::commit_later(
                    &hooks.place,
                    format!("{} node #{node_id}: {goal}", agent.id.as_str()),
                );
                hooks.kick();
            });
        }
        if hooks.is_running(id) || clock.turn_for(id).is_some() {
            continue;
        }
        // Inbox first: a message that asks for a turn. The claim is the
        // rename into turns/tNNNN/cause.md; the wake carries its words.
        if let Some(filed) = inbox::list(&hooks.place, id)
            .into_iter()
            .find(|f| f.msg.wake && !hooks.inbox_backing_off(&f.name, now))
        {
            match inbox::claim(&hooks.place, id, &filed) {
                Ok(turn_dir) => {
                    if let Some(wake) = wake_from_message(hooks, &agent, &filed.msg, &turn_dir) {
                        wakes.push(wake);
                        hooks.broadcast(hooks.plan_frame(id));
                    }
                    continue;
                }
                Err(e) => {
                    // Told once, then left alone for a minute: a full disk or
                    // a read-only folder does not become a log storm.
                    if hooks.inbox_backoff(&filed.name, now) {
                        crate::klog::warn(
                            "inbox_claim_failed",
                            Some(id),
                            format!("{}: {e:#}", filed.name),
                        );
                        hooks.broadcast(arbos_core::wire::Frame::Error {
                            agent: Some(id.to_string()),
                            detail: format!(
                                "could not start the turn for your message ({}): {e:#}; will try again in a minute",
                                filed.name
                            ),
                        });
                    }
                    continue;
                }
            }
        }
        let Some((n, reason)) = fire.wakes.into_iter().next() else {
            continue;
        };
        let Some((n, attempt)) = claim(hooks, clock, id, n, now) else {
            continue;
        };
        let lo = load_transcript(&hooks.layout(id).transcript())
            .map(|e| e.len() as u64 + 1)
            .unwrap_or(1);
        clock.turns.lock().unwrap().insert(
            id.to_string(),
            TurnMeta {
                node: n.id,
                attempt: attempt.id.clone(),
                lo,
            },
        );
        wakes.push(wake_for(hooks.place.path(), &agent, &n, reason, ""));
        hooks.broadcast(hooks.plan_frame(id));
    }
    wakes
}

/// Mark a node active and open its attempt. Disarms the clock on it: a
/// deferral clears, a recurrence advances from now (missed firings
/// coalesce into this one).
fn claim(
    hooks: &KernelHooks,
    clock: &Clock,
    agent: &str,
    mut n: Node,
    now: i64,
) -> Option<(Node, Attempt)> {
    let layout = hooks.layout(agent);
    let key = format!("{agent}#{}", n.id);
    if clock
        .backoff
        .lock()
        .unwrap()
        .get(&key)
        .is_some_and(|until| now < *until)
    {
        return None;
    }
    let _g = hooks.plan_lock.lock().unwrap();
    // Re-read: another writer may have moved it since the scan loaded.
    let fresh = hooks.plan_nodes(agent).into_iter().find(|x| x.id == n.id)?;
    if fresh.status != Status::Pending {
        return None;
    }
    n = fresh;
    let attempts = hooks.plan_attempts(agent);
    let kind = if n.gated() { "condition" } else { n.do_.kind() };
    let attempt = Attempt {
        id: node::next_attempt_id(&attempts),
        node: n.id,
        kind: kind.into(),
        started_ms: now,
        ended_ms: None,
        verdict: None,
        outcome: String::new(),
        verified_by: String::new(),
        transcript_lo: None,
        transcript_hi: None,
        job: None,
    };
    if let Err(e) = node::save_attempt(&layout.attempts_jsonl(), &attempt) {
        claim_failed(hooks, clock, &key, agent, &format!("attempt: {e:#}"), now);
        return None;
    }
    n.status = Status::Active;
    n.attempt = Some(attempt.id.clone());
    n.when.after_ms = None;
    if let Some(every) = n.when.every_ms {
        n.when.next_due_ms = Some(now + every as i64);
    }
    n.updated_ms = now;
    if let Err(e) = node::save_node(&layout.plan_jsonl(), &n) {
        // The attempt is on disk but the node is not: close the attempt so
        // the record says what happened, and leave the node alone for a while.
        let mut a = attempt;
        a.ended_ms = Some(now);
        a.verdict = Some(Verdict::Inconclusive);
        a.outcome = format!("could not write the plan: {e:#}");
        a.verified_by = "kernel".into();
        let _ = node::save_attempt(&layout.attempts_jsonl(), &a);
        claim_failed(hooks, clock, &key, agent, &format!("node: {e:#}"), now);
        return None;
    }
    Some((n, attempt))
}

fn claim_failed(hooks: &KernelHooks, clock: &Clock, key: &str, agent: &str, why: &str, now: i64) {
    clock
        .backoff
        .lock()
        .unwrap()
        .insert(key.to_string(), now + CLAIM_BACKOFF_MS);
    crate::klog::error(
        "claim_failed",
        Some(agent),
        format!("{key}: {why}; next try in {}s", CLAIM_BACKOFF_MS / 1000),
    );
    hooks.broadcast(crate::attach::Frame::Error {
        agent: Some(agent.to_string()),
        detail: format!("could not start {key}: {why}"),
    });
}

/// The line a spawned child gets when it works in its own worktree: where
/// it is, which branch, and that the main checkout is not its to edit.
fn worktree_note(place: &std::path::Path, agent: &Agent) -> String {
    let Some(cwd) = agent.cwd.as_deref() else {
        return String::new();
    };
    if !crate::worktree::is_worktree(place, cwd) {
        return String::new();
    }
    let branch = crate::worktree::branch_of(cwd).unwrap_or_else(|| "(unknown)".into());
    format!(
        " Your working directory is {} — your own git worktree of this repository on branch {branch}, cut from your parent's HEAD; uncommitted edits in the main checkout are not in it. Edit, build, and commit there; report the branch name with your results. The main checkout at {} belongs to your parent: do not edit it.",
        cwd.display(),
        place.display()
    )
}

/// The newest `turns/tNNNN/meta.toml` without `ended` gets `ended`, the
/// verdict, and the outcome (the turn's last words, or why it stopped).
fn close_turn_folder(hooks: &KernelHooks, agent: &str) {
    let turns = inbox::turns_dir(&hooks.place, agent);
    let Ok(rd) = std::fs::read_dir(&turns) else {
        return;
    };
    let mut open: Vec<std::path::PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let meta = p.join("meta.toml");
            std::fs::read_to_string(&meta).is_ok_and(|t| !t.contains("\nended = "))
                && p.join("cause.md").exists()
        })
        .collect();
    open.sort();
    let Some(dir) = open.pop() else {
        return;
    };
    let events = load_transcript(&hooks.layout(agent).transcript()).unwrap_or_default();
    let lo = std::fs::read_to_string(dir.join("meta.toml"))
        .ok()
        .and_then(|t| {
            t.lines().find_map(|l| {
                l.strip_prefix("transcript_lo = ")?
                    .trim()
                    .parse::<u64>()
                    .ok()
            })
        })
        .unwrap_or(0);
    let (outcome, ok) = turn_outcome(&events, lo);
    let verdict = if ok { "success" } else { "failed" };
    let line: String = outcome
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(200)
        .collect();
    let mut text = std::fs::read_to_string(dir.join("meta.toml")).unwrap_or_default();
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&format!(
        "ended = \"{}\"\nverdict = \"{verdict}\"\noutcome = {}\ntranscript_hi = {}\n",
        inbox::rfc3339(arbos_core::now_ms()),
        toml_string(&line),
        events.len()
    ));
    let tmp = dir.join(format!(".meta.toml.tmp-{}", std::process::id()));
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, dir.join("meta.toml"));
    }
}

fn toml_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The turn an inbox message starts. A user's words become the `user`
/// line the turn writes; a peer's request lands on the transcript here as
/// a `say`, and the turn wakes to read it; a brief is the child's mission
/// spelled out; the kernel's own words (a failed command, a condition)
/// are the prompt. `turn_dir` is where the claimed file went.
fn wake_from_message(
    hooks: &KernelHooks,
    agent: &Agent,
    msg: &inbox::Message,
    turn_dir: &std::path::Path,
) -> Option<Wake> {
    let from = msg.from.as_str();
    let (kind, text) = match (msg.kind.as_str(), from) {
        ("brief", _) => {
            let parent = from.strip_prefix("agent:").unwrap_or(from);
            (
                WakeKind::Plan,
                Some(format!(
                    "You were spawned by agent {parent} for this mission:\n\n{}\n\nDo it now. If it has several steps, decompose it with plan add and work them. Standing work (\"every N\", \"keep doing\") is a plan node with when.every, never a loop held open. Report results to your parent with say to={parent} (mode request when you need an answer from it). Your own folder is .arbos/agents/{}/.{}",
                    msg.body,
                    agent.id,
                    worktree_note(hooks.place.path(), agent)
                )),
            )
        }
        (_, "user") | (_, "") => (WakeKind::User, Some(msg.body.clone())),
        (_, "kernel") => (WakeKind::Plan, Some(msg.body.clone())),
        (_, who) if who.starts_with("user:") => (WakeKind::User, Some(msg.body.clone())),
        (_, who) => {
            // A peer's words go on the transcript now; the turn reads them.
            let from_id = who.strip_prefix("agent:").unwrap_or(who).to_string();
            if let Err(e) = append_event(
                &hooks.layout(agent.id.as_str()).transcript(),
                &Event::new(EventKind::Say {
                    from: from_id,
                    text: msg.body.clone(),
                }),
            ) {
                crate::klog::warn(
                    "inbox_say_failed",
                    Some(agent.id.as_str()),
                    format!("{e:#}"),
                );
                return None;
            }
            (WakeKind::Say, None)
        }
    };
    let lo = load_transcript(&hooks.layout(agent.id.as_str()).transcript())
        .map(|e| e.len() as u64)
        .unwrap_or(0);
    let _ = std::fs::write(
        turn_dir.join("meta.toml"),
        format!(
            "started = \"{}\"\nfrom = \"{}\"\nkind = \"{}\"\ntranscript_lo = {lo}\n",
            inbox::rfc3339(arbos_core::now_ms()),
            msg.from,
            msg.kind
        ),
    );
    Some(Wake {
        agent: agent.id.clone(),
        kind,
        text,
        attachments: msg.attachments.clone(),
        steer: false,
        node: None,
        hops: msg.hops,
        channel: msg.channel.clone(),
        device: msg.device.clone(),
    })
}

fn wake_for(
    place: &std::path::Path,
    agent: &Agent,
    n: &Node,
    reason: WakeReason,
    detail: &str,
) -> Wake {
    // A node is a goal; messages are inbox files (`wake_from_message`).
    let _ = place;
    let (kind, text) = (WakeKind::Plan, Some(wake_prompt(n, reason, detail)));
    Wake {
        agent: agent.id.clone(),
        kind,
        text,
        attachments: n.attachments.clone(),
        steer: false,
        node: Some(n.id),
        hops: n.hops,
        channel: n.channel.clone(),
        device: n.device.clone(),
    }
}

/// What the model is told when the clock summons it.
pub fn wake_prompt(n: &Node, reason: WakeReason, detail: &str) -> String {
    let detail = if detail.is_empty() {
        "(no output captured)"
    } else {
        detail
    };
    match reason {
        WakeReason::CmdFailed => format!(
            "Kernel-run command failed: node #{} — {}. Command: `{}`. Output tail:\n{detail}\nDiagnose and act: fix the cause and reopen the node (plan update, status pending) so the kernel retries, adjust its shell command by cancelling it and adding a new node, or record it failed/blocked with an outcome.",
            n.id,
            n.goal,
            match &n.do_ {
                Do::Shell { cmd, .. } => cmd.as_str(),
                _ => "",
            }
        ),
        WakeReason::Ready => format!(
            "Callback: node #{} is now ready — {}. Its earlier siblings finished (see <<plan>> for their outcomes). Do what it says, then finish it with plan update (status done, with a one-line outcome). If you end the turn without updating it, the kernel marks it done with your last reply as the outcome.",
            n.id, n.goal
        ),
        WakeReason::Due if n.recurring() => format!(
            "Scheduled firing: standing obligation #{} is due — {}. Do it now and keep to this one obligation. When this turn ends the kernel records the recurrence with your last reply as the outcome; write that reply as a message to whoever continues the work (values, readings, conclusions the next firing must compare against).",
            n.id, n.goal
        ),
        WakeReason::Due => format!(
            "Scheduled firing: deferred task #{} is now due — {}. Do it now. When this turn ends the kernel marks it done with your last reply as the outcome; use plan update yourself if it failed or is blocked.",
            n.id, n.goal
        ),
        WakeReason::Condition => format!(
            "Condition met: the watch on node #{} held — {}. Predicate: `{}`. Its latest output:\n{detail}\nThe kernel keeps polling this node; do not manage its status. Act on the goal now.",
            n.id, n.goal, n.when.condition
        ),
    }
}

/// A claimed wake that never became a turn. The node goes back to pending
/// so the next scan fires it again.
pub fn abandon(hooks: &KernelHooks, clock: &Clock, agent: &str) {
    let Some(meta) = clock.turns.lock().unwrap().remove(agent) else {
        return;
    };
    let layout = hooks.layout(agent);
    let _g = hooks.plan_lock.lock().unwrap();
    if let Some(mut n) = hooks
        .plan_nodes(agent)
        .into_iter()
        .find(|n| n.id == meta.node)
    {
        if n.status == Status::Active {
            n.status = Status::Pending;
            n.attempt = None;
            n.updated_ms = arbos_core::now_ms();
            let _ = node::save_node(&layout.plan_jsonl(), &n);
        }
    }
}

/// Cursor's completion notification, on disk: when a child's turn ends the
/// kernel writes a `kind = "done"` inbox file to its parent with the turn's
/// last words (or how it failed) and where the transcript is. The child no
/// longer has to remember to `say`. Skipped when the parent was blocked in
/// `spawn wait=true` for this turn: it already got the words as the tool
/// result.
fn notify_parent_done(hooks: &KernelHooks, agent: &str) {
    let Ok(child) = arbos_core::load_agent(&hooks.place, &arbos_core::AgentId::new(agent)) else {
        return;
    };
    let Some(parent) = child.parent.as_ref() else {
        return;
    };
    if hooks.waited.lock().unwrap().remove(agent) {
        return;
    }
    if !arbos_core::agent_exists(&hooks.place, parent.as_str()) {
        return;
    }
    let lo = hooks.turn_lo.lock().unwrap().remove(agent).unwrap_or(0);
    let events = load_transcript(&hooks.layout(agent).transcript()).unwrap_or_default();
    let (outcome, ok) = turn_outcome(&events, lo);
    let status = if ok { "ended" } else { "ended badly" };
    let body = format!(
        "Turn {status}. Last words: {outcome}\n(transcript: .arbos/agents/{agent}/transcript.jsonl)"
    );
    let msg = inbox::Message {
        from: format!("agent:{agent}"),
        kind: "done".into(),
        wake: true,
        hops: 0,
        body,
        ..inbox::Message::default()
    };
    if let Err(e) = inbox::deliver(&hooks.place, parent.as_str(), &msg) {
        crate::klog::warn("done_notice_failed", Some(agent), format!("{e:#}"));
    } else {
        hooks.plan_changed(parent.as_str());
    }
}

/// The turn the clock started for `agent` ended. Close its node and attempt
/// from what the transcript says, unless the model already moved the node.
pub fn finish_turn(hooks: &KernelHooks, clock: &Clock, agent: &str) {
    notify_parent_done(hooks, agent);
    let Some(meta) = clock.turns.lock().unwrap().remove(agent) else {
        // Not a plan node's turn: an inbox message's. Its record is the
        // turn folder; close it.
        close_turn_folder(hooks, agent);
        return;
    };
    let now = arbos_core::now_ms();
    let layout = hooks.layout(agent);
    let events = load_transcript(&layout.transcript()).unwrap_or_default();
    let hi = events.len() as u64;
    let (outcome, ok) = turn_outcome(&events, meta.lo);
    let _g = hooks.plan_lock.lock().unwrap();
    let nodes = hooks.plan_nodes(agent);
    let Some(mut n) = nodes.into_iter().find(|n| n.id == meta.node) else {
        return;
    };
    let attempts = hooks.plan_attempts(agent);
    let mut a = attempts
        .iter()
        .find(|a| a.id == meta.attempt)
        .cloned()
        .unwrap_or(Attempt {
            id: meta.attempt.clone(),
            node: n.id,
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
    a.transcript_lo = Some(meta.lo);
    a.transcript_hi = Some(hi);
    if a.running() {
        a.ended_ms = Some(now);
        // The model moved the node itself: its status is the verdict.
        let self_moved = n.status != Status::Active || n.attempt.as_deref() != Some(&meta.attempt);
        if self_moved {
            a.verdict = Some(match n.status {
                Status::Done | Status::Pending => Verdict::Success,
                Status::Failed => Verdict::Fail,
                _ => Verdict::Inconclusive,
            });
            a.outcome = if n.outcome.is_empty() {
                outcome.clone()
            } else {
                n.outcome.clone()
            };
            a.verified_by = "self".into();
        } else {
            a.verdict = Some(if ok { Verdict::Success } else { Verdict::Fail });
            a.outcome = outcome.clone();
            a.verified_by = "kernel".into();
            n.outcome = outcome;
            n.attempt = None;
            n.status = if n.recurring() {
                Status::Pending
            } else if ok {
                Status::Done
            } else {
                Status::Failed
            };
            n.updated_ms = now;
            let _ = node::save_node(&layout.plan_jsonl(), &n);
        }
        let _ = node::save_attempt(&layout.attempts_jsonl(), &a);
    }
    drop(_g);
    hooks.plan_changed(agent);
}

/// What a turn said, read from the lines it wrote: the last assistant text,
/// or why it stopped. `ok` is false for a stop or a failed step.
fn turn_outcome(events: &[Event], lo: u64) -> (String, bool) {
    let mut last_text = String::new();
    let mut stopped: Option<String> = None;
    let mut failed: Option<String> = None;
    for e in events.iter().filter(|e| e.seq >= lo) {
        match &e.kind {
            EventKind::Assistant { text, .. } if !text.trim().is_empty() => {
                last_text = text.trim().to_string();
            }
            EventKind::Interrupted { detail } => stopped = Some(detail.clone()),
            EventKind::Notice { text, failed: true } => failed = Some(text.clone()),
            _ => {}
        }
    }
    if let Some(why) = stopped {
        return (format!("stopped: {why}"), false);
    }
    if last_text.is_empty() {
        if let Some(f) = failed {
            return (node::clip(&f, 300), false);
        }
        return ("(no reply)".into(), true);
    }
    (node::clip(&last_text, 300), true)
}

// ── mechanical executors ───────────────────────────────────────────────

async fn run_job(hooks: &KernelHooks, agent: &Agent, cmd: &str) -> (Option<String>, i32, String) {
    let cwd = agent
        .cwd
        .clone()
        .unwrap_or_else(|| hooks.place.path.clone());
    let root = JobsRoot::for_agent(&hooks.place, &agent.id);
    let (job, mut child) = match root.spawn(cmd, &cwd, Some(CMD_TIMEOUT.as_millis() as u64), None) {
        Ok(x) => x,
        Err(e) => return (None, -1, format!("could not start: {e}")),
    };
    let id = job.id.clone();
    let timed_out = tokio::time::timeout(CMD_TIMEOUT, child.wait())
        .await
        .is_err();
    if timed_out {
        if let Ok(j) = root.load(&id) {
            root.kill(&j);
        }
    }
    let code = match root.load(&id) {
        Ok(j) => match j.status {
            arbos_engine::JobStatus::Exited(c) => c,
            _ => -1,
        },
        Err(_) => -1,
    };
    let out = journal_tail(&job.journal(), 256 * 1024);
    let mut tail = node::tail(&out);
    if timed_out {
        tail = format!("timed out after {}s\n{tail}", CMD_TIMEOUT.as_secs());
    }
    (Some(id), code, tail)
}

/// The last `max` bytes of a job's journal. Never the whole file: a
/// runaway job's log is capped only by the leash's poll.
fn journal_tail(path: &std::path::Path, max: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return String::new();
    };
    let size = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = size.saturating_sub(max);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut buf = Vec::with_capacity((size - start) as usize);
    let _ = f.take(size - start).read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

fn close(
    hooks: &KernelHooks,
    agent: &str,
    n: &mut Node,
    mut a: Attempt,
    status: Status,
    verdict: Verdict,
    outcome: String,
    by: &str,
    job: Option<String>,
) {
    let now = arbos_core::now_ms();
    let layout = hooks.layout(agent);
    let _g = hooks.plan_lock.lock().unwrap();
    a.ended_ms = Some(now);
    a.verdict = Some(verdict);
    a.outcome = outcome.clone();
    a.verified_by = by.into();
    a.job = job;
    let _ = node::save_attempt(&layout.attempts_jsonl(), &a);
    n.status = status;
    n.outcome = outcome;
    n.attempt = None;
    n.updated_ms = now;
    let _ = node::save_node(&layout.plan_jsonl(), n);
    drop(_g);
    hooks.plan_changed(agent);
}

/// A shell or notify node: the kernel does the work, no model turn. A
/// failed command summons the model with the log tail — the one turn a
/// healthy pipeline never spends.
async fn run_mechanical(hooks: &Arc<KernelHooks>, agent: &Agent, mut n: Node, a: Attempt) {
    let id = agent.id.as_str();
    match n.do_.clone() {
        Do::Shell { cmd, report } => {
            let (job, code, tail) = run_job(hooks, agent, &cmd).await;
            // A node that exists to report a reading has nothing to report
            // when the command printed nothing: that is a failure too
            // (`curl | jq` with a dead endpoint exits 0 on some shells).
            let silent = report.is_some() && tail.trim().is_empty();
            let ok = code == 0 && !silent;
            // The output is the outcome: it is what the next firing, and
            // the window's `last:` line, need to see.
            let mut outcome = format!("exit {code}");
            if silent && code == 0 {
                outcome.push_str(", no output for the report");
            }
            if !tail.is_empty() {
                outcome.push_str(" — ");
                outcome.push_str(&node::clip(&tail, 400));
            }
            let status = if n.recurring() {
                Status::Pending
            } else if ok {
                Status::Done
            } else {
                Status::Failed
            };
            let verdict = if ok { Verdict::Success } else { Verdict::Fail };
            close(hooks, id, &mut n, a, status, verdict, outcome, "exit", job);
            if ok {
                if let Some(tpl) = report {
                    let out = tail.trim();
                    let text = if tpl.contains("{output}") {
                        tpl.replace("{output}", out)
                    } else if out.is_empty() {
                        tpl.clone()
                    } else {
                        format!("{tpl}\n{out}")
                    };
                    let _ = deliver(hooks, id, &n, &text);
                }
            }
            if !ok {
                let detail = if silent && code == 0 {
                    "the command exited 0 but printed nothing, so there was nothing to report"
                        .to_string()
                } else {
                    tail.clone()
                };
                let mut wake =
                    Node::inbox(wake_prompt(&n, WakeReason::CmdFailed, &detail), "kernel");
                wake.check = format!("node #{}", n.id);
                let _ = hooks.inbox(id, wake);
            }
        }
        Do::Notify { text } => {
            let (ok, outcome) = deliver(hooks, id, &n, &text);
            let status = if n.recurring() {
                Status::Pending
            } else if ok {
                Status::Done
            } else {
                Status::Failed
            };
            let verdict = if ok { Verdict::Success } else { Verdict::Fail };
            close(
                hooks, id, &mut n, a, status, verdict, outcome, "kernel", None,
            );
        }
        Do::Agent | Do::Ask => {}
    }
}

/// Speak into whoever asked for the node. The user by default; a peer when
/// the origin names one.
fn deliver(hooks: &KernelHooks, agent: &str, n: &Node, text: &str) -> (bool, String) {
    let peer = n
        .origin
        .strip_prefix("agent:")
        .or_else(|| n.origin.strip_prefix("spawn:"));
    let r = match peer {
        Some(peer) if !peer.is_empty() => hooks
            .say(
                &arbos_core::AgentId::new(agent),
                peer,
                text,
                crate::hooks::SayMode::Note,
                0,
            )
            .map(|_| ()),
        _ => hooks.notify_user(agent, text),
    };
    match r {
        Ok(()) => (true, "notified".into()),
        Err(e) => (false, format!("notify failed: {e}")),
    }
}

/// A gated node: run the predicate; only when it holds does the node's
/// `do` fire. A miss re-arms quietly — no attempt, no model.
async fn run_condition(hooks: &Arc<KernelHooks>, agent: &Agent, mut n: Node, a: Attempt) {
    let id = agent.id.as_str();
    let (job, code, tail) = run_job(hooks, agent, &n.when.condition).await;
    let held = code == 0;
    let status = if n.recurring() {
        Status::Pending
    } else if held {
        Status::Done
    } else {
        Status::Pending
    };
    if !held {
        // Quiet miss: close the attempt without noise in the outcome.
        let layout = hooks.layout(id);
        let _g = hooks.plan_lock.lock().unwrap();
        let mut a = a;
        a.ended_ms = Some(arbos_core::now_ms());
        a.verdict = Some(Verdict::Inconclusive);
        a.outcome = String::new();
        a.verified_by = "exit".into();
        a.job = job;
        let _ = node::save_attempt(&layout.attempts_jsonl(), &a);
        n.status = status;
        n.attempt = None;
        n.updated_ms = arbos_core::now_ms();
        let _ = node::save_node(&layout.plan_jsonl(), &n);
        drop(_g);
        hooks.broadcast(hooks.plan_frame(id));
        return;
    }
    let mut outcome = "condition held".to_string();
    if !tail.is_empty() {
        outcome.push_str(": ");
        outcome.push_str(&tail);
    }
    close(
        hooks,
        id,
        &mut n,
        a,
        status,
        Verdict::Success,
        outcome,
        "exit",
        job,
    );
    match &n.do_ {
        Do::Notify { text } => {
            let _ = deliver(hooks, id, &n, text);
        }
        _ => {
            let mut wake = Node::inbox(wake_prompt(&n, WakeReason::Condition, &tail), "kernel");
            wake.check = format!("node #{}", n.id);
            let _ = hooks.inbox(id, wake);
        }
    }
}

// ── wire ────────────────────────────────────────────────────────────────

/// Depth-first, siblings by (seq, id): the order the window draws.
fn tree_order(nodes: &[Node]) -> Vec<&Node> {
    fn walk<'a>(parent: NodeId, nodes: &'a [Node], out: &mut Vec<&'a Node>, depth: usize) {
        if depth > 32 {
            return;
        }
        let mut kids: Vec<&Node> = nodes.iter().filter(|n| n.parent == parent).collect();
        kids.sort_by_key(|n| (n.seq, n.id));
        for k in kids {
            out.push(k);
            walk(k.id, nodes, out, depth + 1);
        }
    }
    let mut out = Vec::with_capacity(nodes.len());
    walk(0, nodes, &mut out, 0);
    // Orphans (a parent that was never written) still show.
    for n in nodes {
        if !out.iter().any(|x| x.id == n.id) {
            out.push(n);
        }
    }
    out
}

pub fn wire_nodes(nodes: &[Node], attempts: &[Attempt]) -> Vec<PlanNode> {
    let last = node::last_attempts(attempts);
    let now = arbos_core::now_ms();
    tree_order(nodes)
        .into_iter()
        .map(|n| {
            let gated = node::gated_by_sibling(nodes, n);
            let when = if n.status != Status::Pending {
                String::new()
            } else if matches!(n.do_, Do::Ask) {
                "waits on you".into()
            } else if let Some(every) = n.when.every_ms {
                let next = n
                    .when
                    .next_due_ms
                    .map(|t| {
                        if now >= t {
                            "due now".to_string()
                        } else {
                            format!("next {}", node::clock(t))
                        }
                    })
                    .unwrap_or_default();
                format!("every {} · {next}", node::human_ms(every))
            } else if let Some(t) = n.when.after_ms.filter(|t| now < *t) {
                format!("fires {}", node::clock(t))
            } else if gated {
                "after earlier steps".into()
            } else if n.when.wake || n.do_.mechanical() {
                "next".into()
            } else {
                "ready".into()
            };
            let inbox = node::is_inbox(nodes, n);
            PlanNode {
                id: n.id,
                parent: n.parent,
                goal: node::clip(&n.goal, 160),
                status: n.status.as_str().into(),
                when,
                do_kind: n.do_.kind().into(),
                last: last
                    .get(&n.id)
                    .filter(|a| !a.outcome.is_empty())
                    .map(|a| node::clip(&a.outcome, 160))
                    .unwrap_or_default(),
                origin: n.origin.clone(),
                standing: n.recurring(),
                inbox,
            }
        })
        .collect()
}
