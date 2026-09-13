//! Turns start from files. Each scan claims one waking inbox file per
//! idle agent (the rename into `turns/tNNNN/cause.md` is the claim) and
//! asks the watcher to fire due subscriptions, which write inbox files of
//! their own. When a turn ends its folder is closed and, for a child, the
//! parent gets a `done` message. There is no plan node and no second clock
//! (Cursor's model, decided 2026-09-13).

use arbos_core::{
    Agent, Event, EventKind, Wake, WakeKind, append_event, inbox, list_agents, load_transcript,
    notes, subscription,
    wire::PlanNode,
};
use std::sync::Arc;

use crate::hooks::KernelHooks;

/// Kernel start: a turn folder left open by a dead kernel is closed with a
/// note. The serve wake (`needs_serve`) continues the turn itself from the
/// transcript; the record just has to say what happened.
pub fn reclaim(hooks: &KernelHooks) {
    for agent in list_agents(&hooks.place).unwrap_or_default() {
        let id = agent.id.as_str();
        while close_turn_folder(hooks, id, Some("kernel restarted before this turn ended")) {}
        hooks.broadcast(hooks.plan_frame(id));
    }
}

/// One pass: fire what the watcher finds due, then claim one waking inbox
/// file for each idle agent and return the turns to start.
pub fn scan(hooks: &Arc<KernelHooks>) -> Vec<Wake> {
    let now = arbos_core::now_ms();
    crate::subs::tick(hooks, now);
    let mut wakes = Vec::new();
    for agent in list_agents(&hooks.place).unwrap_or_default() {
        if agent.paused {
            continue;
        }
        let id = agent.id.as_str();
        if hooks.is_running(id) {
            continue;
        }
        let Some(filed) = inbox::list(&hooks.place, id)
            .into_iter()
            .find(|f| f.msg.wake && !hooks.inbox_backing_off(&f.name, now))
        else {
            continue;
        };
        match inbox::claim(&hooks.place, id, &filed) {
            Ok(turn_dir) => {
                if let Some(wake) = wake_from_message(hooks, &agent, &filed.msg, &turn_dir) {
                    wakes.push(wake);
                    hooks.broadcast(hooks.plan_frame(id));
                }
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
            }
        }
    }
    wakes
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
/// verdict, and the outcome (the turn's last words, or why it stopped, or
/// `forced` when the kernel closes it for another reason). True when one
/// was closed.
fn close_turn_folder(hooks: &KernelHooks, agent: &str, forced: Option<&str>) -> bool {
    let turns = inbox::turns_dir(&hooks.place, agent);
    let Ok(rd) = std::fs::read_dir(&turns) else {
        return false;
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
        return false;
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
    let (outcome, ok) = match forced {
        Some(why) => (why.to_string(), false),
        None => turn_outcome(&events, lo),
    };
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
    true
}

fn toml_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The turn an inbox message starts. A user's words become the `user`
/// line the turn writes; a peer's request lands on the transcript here as
/// a `say`, and the turn wakes to read it; a brief is the child's mission
/// spelled out; the kernel's and a subscription's words are the prompt.
/// `turn_dir` is where the claimed file went.
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
                    "You were spawned by agent {parent} for this mission:\n\n{}\n\nDo it now. If it has several steps, write them as your checklist with plan set and work them. Standing work (\"every N\", \"keep watching\") is a subscription (subscribe add), never a loop held open. Report results to your parent with say to={parent} (mode request when you need an answer from it). Your own folder is .arbos/agents/{}/.{}",
                    msg.body,
                    agent.id,
                    worktree_note(hooks.place.path(), agent)
                )),
            )
        }
        (_, "user") | (_, "") => (WakeKind::User, Some(msg.body.clone())),
        (_, "kernel") => (WakeKind::Plan, Some(msg.body.clone())),
        (_, who) if who.starts_with("subscription:") => (WakeKind::Plan, Some(msg.body.clone())),
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
        hops: msg.hops,
    })
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

/// A turn ended: its folder closes with what the transcript says, and a
/// child's parent hears about it.
pub fn finish_turn(hooks: &KernelHooks, agent: &str) {
    notify_parent_done(hooks, agent);
    close_turn_folder(hooks, agent, None);
    hooks.broadcast(hooks.plan_frame(agent));
}

/// The turn's last words and whether it ended well, from the transcript
/// lines the turn wrote (`lo` onward).
pub fn turn_outcome(events: &[Event], lo: u64) -> (String, bool) {
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
            return (arbos_core::text::clip(&f, 300), false);
        }
        return ("(no reply)".into(), true);
    }
    (arbos_core::text::clip(&last_text, 300), true)
}

// ── the window's rows ───────────────────────────────────────────────────

/// Ids in the plan frame: subscriptions in `1 << 41 | id`, notes items in
/// `1 << 42 | n`, inbox files in `1 << 40 | hash` (`hooks::inbox_id`).
pub const SUB_ID_BIT: u64 = 1 << 41;
pub const NOTE_ID_BIT: u64 = 1 << 42;

/// The desktop's plan strip reads one list: standing subscriptions, open
/// notes items, and queued inbox messages, in the `PlanNode` shape it
/// already draws.
pub fn wire_rows(place: &arbos_core::Place, agent: &str) -> Vec<PlanNode> {
    let mut rows = Vec::new();
    for sub in subscription::list(place, agent) {
        rows.push(PlanNode {
            id: SUB_ID_BIT | u64::from(sub.id),
            parent: 0,
            goal: sub.label(),
            status: if sub.paused { "blocked".into() } else { "pending".into() },
            when: sub.when_line(),
            do_kind: match sub.kind.as_str() {
                "shell" if sub.deliver_to == "user" => "notify".into(),
                "shell" => "shell".into(),
                _ => "agent".into(),
            },
            last: sub.last.clone(),
            origin: format!("subscription:{}", sub.kind),
            standing: true,
            inbox: false,
        });
    }
    for item in notes::load(place, agent).items() {
        if item.done {
            continue;
        }
        rows.push(PlanNode {
            id: NOTE_ID_BIT | item.n as u64,
            parent: 0,
            goal: item.text.clone(),
            status: "pending".into(),
            when: String::new(),
            do_kind: "agent".into(),
            last: item.readout().unwrap_or("").to_string(),
            origin: if item.section.is_empty() { String::new() } else { item.section.clone() },
            standing: false,
            inbox: false,
        });
    }
    rows
}
