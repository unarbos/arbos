//! Turns start from files. Each scan claims one waking inbox file per
//! idle agent (the rename into `turns/tNNNN/cause.md` is the claim) and
//! asks the watcher to fire due subscriptions, which write inbox files of
//! their own. When a turn ends its folder is closed and, for a child, the
//! parent gets a `done` message. There is no plan node and no second clock
//! (Cursor's model, decided 2026-09-13).

use arbos_core::{
    Agent, Event, EventKind, Wake, WakeKind, append_event, inbox, list_agents, load_transcript,
    notes, subscription, wire::PlanNode,
};
use std::sync::Arc;

use crate::hooks::KernelHooks;

/// Kernel start: a turn folder left open by a dead kernel is closed with a
/// note. The serve wake (`needs_serve`) continues the turn itself from the
/// transcript; the record just has to say what happened.
pub fn reclaim(hooks: &KernelHooks) {
    crate::subs::ensure_chores(&hooks.place);
    for agent in list_agents(&hooks.place).unwrap_or_default() {
        let id = agent.id.as_str();
        // Tool calls the last kernel died in the middle of: each becomes
        // a `tool` line that says so, before the serve wake continues the
        // turn — else the model, seeing no record, runs the command again
        // (qal-j02: a side effect twice).
        let now = arbos_core::now_ms();
        let cut: Vec<arbos_core::Event> = arbos_engine::inflight::take(&hooks.place, &agent.id)
            .into_iter()
            .map(|rec| {
                crate::klog::warn(
                    "tool_cut_by_restart",
                    Some(id),
                    format!("{} {}", rec.name, rec.call_id),
                );
                arbos_core::Event::new(arbos_core::EventKind::Tool(
                    arbos_engine::inflight::cut_record(rec, now),
                ))
            })
            .collect();
        if !cut.is_empty() {
            let _ = arbos_core::append_events(&hooks.layout(id).transcript(), &cut);
        }
        while close_turn_folder(hooks, id, Some("kernel restarted before this turn ended")) {}
        // A blocking allow/deny prompt does not outlive its turn; a parked
        // question does, and stays.
        arbos_core::waiting::clear_approves(&hooks.place, id);
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
        // No model key: the words stay in the inbox — the pending row
        // under the composer — and run when `configure` lands one, which
        // kicks this scan. A turn would only burn them on a failure
        // notice. Said once per agent, on the transcript where the
        // window shows it.
        if let Some(hint) = crate::serve::keyless(&hooks.place) {
            if keyless_told(id) {
                let text = format!(
                    "{hint} Your message is kept and runs once a key is in place: {}",
                    arbos_core::text::clip(filed.msg.body.trim(), 120)
                );
                let _ = arbos_core::append_event(
                    &hooks.layout(id).transcript(),
                    &arbos_core::Event::new(arbos_core::EventKind::Notice {
                        text: text.clone(),
                        failed: true,
                    }),
                );
                hooks.broadcast(arbos_core::wire::Frame::Error {
                    agent: Some(id.to_string()),
                    detail: text,
                });
                crate::klog::warn("turn_held_keyless", Some(id), &filed.name);
                hooks.broadcast(hooks.plan_frame(id));
            }
            continue;
        }
        keyless_forget(id);
        match inbox::claim(&hooks.place, id, &filed) {
            Ok(turn_dir) => {
                if let Some(mut wake) = wake_from_message(hooks, &agent, &filed.msg, &turn_dir) {
                    // Every other finished worker's done file joins this
                    // turn: one wake, one model call, one message to the
                    // user ("3 workers finished"), not one turn per child.
                    if filed.msg.kind == "done" {
                        let mut reported = vec![filed.msg.from.clone()];
                        reported.extend(batch_done_files(hooks, &agent, &filed, &turn_dir, now));
                        let still = still_working(hooks, &agent, &reported);
                        wake.kind = WakeKind::Done;
                        wake.text = Some(done_line(&reported, &still));
                        archive_finished(hooks, &reported);
                    }
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

/// Agents told once that their words wait for a key. `keyless_told` is
/// true the first time only; `keyless_forget` clears the agent when a key
/// arrives, so a later loss of the key is told again.
static KEYLESS_TOLD: std::sync::Mutex<Option<std::collections::HashSet<String>>> =
    std::sync::Mutex::new(None);

fn keyless_told(agent: &str) -> bool {
    KEYLESS_TOLD
        .lock()
        .unwrap()
        .get_or_insert_with(Default::default)
        .insert(agent.to_string())
}

fn keyless_forget(agent: &str) {
    if let Some(set) = KEYLESS_TOLD.lock().unwrap().as_mut() {
        set.remove(agent);
    }
}

/// This agent's other workers that are still running, queued, or parked
/// on a question — the ones whose reports are yet to come.
fn still_working(hooks: &KernelHooks, agent: &Agent, reported: &[String]) -> Vec<String> {
    list_agents(&hooks.place)
        .unwrap_or_default()
        .into_iter()
        .filter(|a| a.parent.as_ref().is_some_and(|p| p == &agent.id) && !a.paused)
        .map(|a| a.id.to_string())
        .filter(|id| {
            !reported
                .iter()
                .any(|r| r.strip_prefix("agent:").unwrap_or(r) == id)
        })
        .filter(|id| hooks.is_live(id))
        .collect()
}

/// The kernel line that opens a done wake: who reported, who is still
/// working, and what the turn owes. Cursor's coordinator folds worker
/// reports into one answer and says nothing in between (cold-p5, cold-pp2).
fn done_line(reported: &[String], still: &[String]) -> String {
    let names = |ids: &[String]| {
        ids.iter()
            .map(|r| r.strip_prefix("agent:").unwrap_or(r).to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let who = names(reported);
    let plural = if reported.len() == 1 {
        "Report"
    } else {
        "Reports"
    };
    if still.is_empty() {
        format!(
            "{plural} from {who} above — the last of your workers. If the user is owed an answer, give it once now, combined, in the answer shape; do not relay each report. If your last message already covered it, or the user asked for nothing further, end with no message (an empty reply is right here)."
        )
    } else {
        format!(
            "{plural} from {who} above. Still working: {}. Hold the combined answer until the last report; end this turn with no message unless the user is owed something now (an empty reply is right here). Do not narrate the report that just came in.",
            names(still)
        )
    }
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
    // A timer with continuity: the words this turn ended with ride on its
    // next firing.
    if let Ok(cause) = std::fs::read_to_string(dir.join("cause.md"))
        && let Ok(msg) = inbox::Message::parse(&cause)
        && let Some(n) = msg
            .from
            .strip_prefix("subscription:")
            .and_then(|n| n.parse::<u32>().ok())
        && let Some(mut sub) = arbos_core::subscription::get(&hooks.place, agent, n)
        && sub.continuity
        && sub.kind == "timer"
    {
        sub.seen = Some(arbos_core::text::clip(outcome.trim(), 4000));
        let _ = arbos_core::subscription::save(&hooks.place, agent, &sub);
    }
    let verdict = if ok { "success" } else { "failed" };
    let line: String = outcome
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(200)
        .collect();
    // Confirmed: an unreadable meta.toml is not rewritten as its tail
    // (arbos_core::record); the close is logged instead.
    let mut text = match arbos_core::record::read_text(&dir.join("meta.toml")).confirmed() {
        Ok(t) => t.unwrap_or_default(),
        Err(e) => {
            crate::klog::warn("turn_meta_unread", None, format!("{e:#}"));
            return true;
        }
    };
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
                    "You were spawned by agent {parent} for this mission:\n\n{}\n\nDo it now. Say what you are doing with status at each major step (a verb phrase, six words or less): it is the line {parent} and the user see beside your name. If it has several steps, write them as your checklist with plan set and work them. Standing work (\"every N\", \"keep watching\") is a subscription (subscribe add), never a loop held open. When your turn ends, {parent} is told your last words automatically: end with a short report (outcome, paths, open questions) as your final words, and do not also say it to {parent}. Use say to={parent} mode request only for a question you need answered mid-task. The project context is in your prompt; project status is .arbos/notes.md; earlier workers' transcripts (live or archived) are greppable with grep scope=history. Your own folder is .arbos/agents/{}/.{}",
                    msg.body,
                    agent.id,
                    worktree_note(hooks.place.path(), agent)
                )),
            )
        }
        (_, "user") | (_, "") => {
            // The user's new words re-arm the notes nudge for the turns
            // that follow (once per idle period).
            hooks.notes_nudge.lock().unwrap().remove(agent.id.as_str());
            (WakeKind::User, Some(msg.body.clone()))
        }
        ("kickoff", "kernel") => (WakeKind::Kickoff, Some(msg.body.clone())),
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
    let title_line = if msg.title.is_empty() {
        String::new()
    } else {
        format!("title = {}\n", toml_string(&msg.title))
    };
    let _ = std::fs::write(
        turn_dir.join("meta.toml"),
        format!(
            "started = \"{}\"\nfrom = \"{}\"\nkind = \"{}\"\n{title_line}transcript_lo = {lo}\n",
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
        channel: msg.channel.clone(),
        device: msg.device.clone(),
        model: msg.model.clone(),
        title: msg.title.clone(),
        brief: if msg.kind == "brief" {
            msg.body.clone()
        } else {
            String::new()
        },
    })
}

/// The other `done` files waiting for `agent` go into the same turn folder
/// (`cause-2.md`, `cause-3.md`, …) and onto the transcript as one `say`
/// line each, so the turn that `first` opened reads them all.
fn batch_done_files(
    hooks: &KernelHooks,
    agent: &Agent,
    first: &inbox::Filed,
    turn_dir: &std::path::Path,
    now: i64,
) -> Vec<String> {
    let mut senders = Vec::new();
    let mut n = 1;
    for other in inbox::list(&hooks.place, agent.id.as_str()) {
        if other.name == first.name
            || other.msg.kind != "done"
            || !other.msg.wake
            || hooks.inbox_backing_off(&other.name, now)
        {
            continue;
        }
        n += 1;
        senders.push(other.msg.from.clone());
        let dest = turn_dir.join(format!("cause-{n}.md"));
        if let Err(e) = std::fs::rename(&other.path, &dest) {
            crate::klog::warn(
                "done_batch_failed",
                Some(agent.id.as_str()),
                format!("{}: {e:#}", other.name),
            );
            continue;
        }
        let from = other.msg.from.as_str();
        let from_id = from.strip_prefix("agent:").unwrap_or(from).to_string();
        if let Err(e) = append_event(
            &hooks.layout(agent.id.as_str()).transcript(),
            &Event::new(EventKind::Say {
                from: from_id,
                text: other.msg.body.clone(),
            }),
        ) {
            crate::klog::warn(
                "inbox_say_failed",
                Some(agent.id.as_str()),
                format!("{e:#}"),
            );
        }
    }
    if n > 1 {
        crate::klog::info(
            "done_batched",
            Some(agent.id.as_str()),
            format!("{n} done files in one turn"),
        );
    }
    senders
}

/// Unless project.toml says `[root] archive_children = false`: a worker
/// whose done message its parent has just read, and that is not live (no
/// turn, no waiting message, no parked ask, no live children of its own),
/// moves to `.arbos/archive/agents/<id>/`. The tree frame tells every
/// window.
fn archive_finished(hooks: &KernelHooks, reported: &[String]) {
    let parents: std::collections::BTreeSet<String> = reported
        .iter()
        .filter_map(|r| {
            let id = r.strip_prefix("agent:").unwrap_or(r);
            arbos_core::load_agent(&hooks.place, &arbos_core::AgentId::new(id))
                .ok()
                .and_then(|a| a.parent.map(|p| p.to_string()))
        })
        .collect();
    archive_finished_inner(hooks, reported);
    for p in parents {
        hooks.refresh_waiting(&p);
    }
}

fn archive_finished_inner(hooks: &KernelHooks, reported: &[String]) {
    if !arbos_core::project::load(&hooks.place)
        .root
        .archives_children()
    {
        return;
    }
    let mut moved = false;
    for from in reported {
        let Some(id) = from.strip_prefix("agent:") else {
            continue;
        };
        if hooks.is_live(id) || hooks.live_children(&arbos_core::AgentId::new(id)) > 0 {
            continue;
        }
        let src = hooks.layout(id).dir.clone();
        if !src.join("agent.md").exists() {
            continue;
        }
        let remote = arbos_core::load_agent(&hooks.place, &arbos_core::AgentId::new(id))
            .ok()
            .is_some_and(|a| a.remote.is_some());
        let dir = arbos_core::project::archive_agents_dir(&hooks.place);
        let dest = dir.join(id);
        // The child's running jobs move with its folder. Each leash reads
        // its cap from the folder's path, so it is told the new one
        // before the move; a job that was blind to its own cap for the
        // life of a moved folder is how a log grew to 164 GB. The jobs
        // themselves go on: a server a worker started on purpose survives
        // being archived.
        let running: Vec<(u32, String)> =
            arbos_engine::JobsRoot::for_agent(&hooks.place, &arbos_core::AgentId::new(id))
                .list()
                .into_iter()
                .filter(|j| j.running())
                .map(|j| (j.meta.pid, j.id.clone()))
                .collect();
        let store = hooks.place.arbos();
        for (pid, job) in &running {
            let _ = arbos_engine::repoint_leash(&store, *pid, &dest.join("jobs").join(job));
        }
        let result = std::fs::create_dir_all(&dir).and_then(|_| {
            if dest.exists() {
                return Err(std::io::Error::other("already archived"));
            }
            std::fs::rename(&src, &dest)
        });
        if result.is_err() {
            for (pid, _) in &running {
                let _ = std::fs::remove_file(
                    store
                        .join(arbos_engine::LEASH_POINTERS)
                        .join(pid.to_string()),
                );
            }
        } else if !running.is_empty() {
            crate::klog::info(
                "jobs_repointed",
                Some(id),
                format!(
                    "{} running job(s) follow the folder to the archive: {}",
                    running.len(),
                    running
                        .iter()
                        .map(|(pid, job)| format!("{job} (pid {pid})"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
        }
        match result {
            Ok(()) => {
                moved = true;
                crate::klog::info("child_archived", Some(id), dest.display().to_string());
                retire_page_rows(hooks, id, &dest);
                // A child on another machine: its kernel there stops and
                // its record goes, or every spawn left one running (qa-038).
                if remote {
                    crate::remote::forget(hooks, id);
                }
                // Its worktree goes with it when nothing would be lost
                // (K-01c); commits stay on the branch.
                match crate::worktree::remove_if_clean(hooks.place.path(), id) {
                    Ok(crate::worktree::Removed::Nothing) => {}
                    Ok(crate::worktree::Removed::Removed {
                        branch,
                        ahead,
                        branch_kept,
                    }) => crate::klog::info(
                        "worktree_removed",
                        Some(id),
                        if branch_kept {
                            format!("{branch} keeps {ahead} commit(s)")
                        } else {
                            format!("{branch} had no commits; deleted")
                        },
                    ),
                    Ok(crate::worktree::Removed::KeptDirty { path, dirty }) => crate::klog::warn(
                        "worktree_kept",
                        Some(id),
                        format!(
                            "{} has {dirty} uncommitted path(s); left as is",
                            path.display()
                        ),
                    ),
                    Err(e) => {
                        crate::klog::warn("worktree_remove_failed", Some(id), format!("{e:#}"))
                    }
                }
                // Two `changed` frames for clients that mirror folders
                // (the watcher sees files, not a folder rename); the
                // `tree` frame below is the list itself.
                hooks.broadcast(arbos_core::wire::Frame::Changed {
                    path: format!("agents/{id}"),
                    kind: "removed".into(),
                    size: 0,
                });
                hooks.broadcast(arbos_core::wire::Frame::Changed {
                    path: format!("archive/agents/{id}"),
                    kind: "created".into(),
                    size: 0,
                });
            }
            Err(e) => crate::klog::warn("child_archive_failed", Some(id), format!("{e:#}")),
        }
    }
    if moved {
        hooks.broadcast_tree();
    }
}

/// Where a worktree worker's changes are when its turn ends, for the done
/// message. A worker that edited in its own worktree and never committed
/// left the user's checkout untouched; its parent said "renamed
/// everywhere" all the same (symmetry loop, cycle 4). The done line
/// makes the state plain so the answer can be.
fn worktree_state(hooks: &KernelHooks, agent: &str) -> Option<String> {
    let left = crate::worktree::leftover(hooks.place.path(), agent)?;
    if left.dirty == usize::MAX || left.ahead == usize::MAX {
        // git could not say; better no line than a wrong one.
        return None;
    }
    let rel = left
        .path
        .strip_prefix(hooks.place.path())
        .unwrap_or(&left.path)
        .display()
        .to_string();
    Some(match (left.dirty, left.ahead) {
        (0, 0) => format!(
            "Worktree {rel} (branch {}): no changes committed or pending; the checkout is as it was. If its report says it committed or wrote files, the work went somewhere else (another repository, a copied folder) — check before relaying it as done.",
            left.branch
        ),
        (0, ahead) => format!(
            "Its changes are {ahead} commit(s) on branch {} (worktree {rel}), not in the user's checkout until merged.",
            left.branch
        ),
        (dirty, 0) => format!(
            "Its changes are UNCOMMITTED: {dirty} path(s) in worktree {rel} (branch {}); the user's checkout is unchanged. Not done until committed and merged.",
            left.branch
        ),
        (dirty, ahead) => format!(
            "Its changes: {ahead} commit(s) on branch {} plus {dirty} uncommitted path(s) in worktree {rel}; the user's checkout is unchanged until merged.",
            left.branch
        ),
    })
}

/// The project page's rows for an archived worker stop saying "worker
/// running". While a worker runs its row targets `agents/<id>`; once the
/// folder has moved that link is dead and the readout is stale. A turn
/// that ended well checks the row (readout: the worker's last words); a
/// stopped or failed one stays open and says so. Either way the link
/// follows the folder into the archive. Root rewrites the row again when
/// it files the result (`plan check n readout target`).
fn retire_page_rows(hooks: &KernelHooks, id: &str, archived: &std::path::Path) {
    let mut page = arbos_core::notes::load(&hooks.place, arbos_core::ROOT_ID);
    let rows = page.worker_items(id);
    if rows.is_empty() {
        return;
    }
    let events = load_transcript(&archived.join("transcript.jsonl")).unwrap_or_default();
    let lo = events
        .iter()
        .rev()
        .find(|e| matches!(e.kind, EventKind::Wake { .. }))
        .map_or(0, |e| e.seq);
    let (outcome, ok) = turn_outcome(&events, lo);
    let readout = if ok {
        format!("worker finished: {}", arbos_core::text::clip(&outcome, 100))
    } else {
        format!("worker stopped: {}", arbos_core::text::clip(&outcome, 100))
    };
    // A worker's code change shows its PR when it opened one, else the
    // worker (its archive); never both (Cursor's page rule).
    let target = arbos_core::load_prs(&hooks.place)
        .into_iter()
        .rev()
        .find(|pr| pr.agent == id)
        .map(|pr| pr.url)
        .unwrap_or_else(|| format!("archive/agents/{id}"));
    // Numbers shift as rows are checked (done ones sink), so each pass
    // looks the row up again by its target.
    for _ in 0..rows.len() {
        let Some(row) = page.worker_items(id).into_iter().next() else {
            break;
        };
        if let Err(e) = page.check_with_target(row.n, ok, Some(&readout), Some(&target)) {
            crate::klog::warn("page_row_retire_failed", Some(id), format!("{e:#}"));
            return;
        }
    }
    match hooks.save_notes(arbos_core::ROOT_ID, &page) {
        Ok(()) => crate::klog::info(
            "page_rows_retired",
            Some(id),
            format!("{} row(s): {readout}", rows.len()),
        ),
        Err(e) => crate::klog::warn("page_row_retire_failed", Some(id), format!("{e:#}")),
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
        // The parent has the report as its tool result; no done file, so
        // the archive step must come from here — once the parent's turn
        // ends (it may still read the folder), or now if it already has.
        if hooks.is_running(parent.as_str()) {
            hooks
                .archive_after
                .lock()
                .unwrap()
                .insert(agent.to_string(), parent.to_string());
        } else {
            archive_finished(hooks, &[format!("agent:{agent}")]);
        }
        return;
    }
    if !arbos_core::agent_exists(&hooks.place, parent.as_str()) {
        return;
    }
    let lo = hooks.turn_lo.lock().unwrap().remove(agent).unwrap_or(0);
    let events = load_transcript(&hooks.layout(agent).transcript()).unwrap_or_default();
    let (outcome, ok) = turn_outcome(&events, lo);
    // The user's Stop paused this worker; it did not fail. Its report is
    // a note, not a wake: the parent hears it with the user's next line
    // ("Continue where you stopped") and picks the work back up, instead
    // of waking at once to call the stop a failure and spawn nothing
    // (F-96, journey J09).
    let user_stop = outcome.starts_with(USER_STOPPED);
    let capped = outcome.starts_with(arbos_core::spend::TURN_CAP_PREFIX);
    let status = if ok {
        "ended"
    } else if user_stop {
        "stopped by the user"
    } else if capped {
        "stopped at the per-turn cap"
    } else {
        "ended badly"
    };
    let mut body = format!(
        "Turn {status}. Last words: {outcome}\n(transcript: .arbos/agents/{agent}/transcript.jsonl)"
    );
    if user_stop {
        body.push_str(&format!(
            "\nThis is the user pausing, not a failure. Its folder and transcript are intact: when the user says to continue, `say to={agent} mode=request` with what to pick up, or spawn afresh with what is left."
        ));
    }
    if let Some(note) = worktree_state(hooks, agent) {
        body.push('\n');
        body.push_str(&note);
    }
    let msg = inbox::Message {
        from: format!("agent:{agent}"),
        kind: "done".into(),
        wake: !user_stop,
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

/// A top-level agent's turn ended: the user should hear about it even
/// when no window is open — the answer as a `reply`, a failure as an
/// `error`. A worker's end reaches the user through its parent's reply,
/// so a child notifies nothing; a turn that ended in silence (a done
/// wake with nothing owed) notifies nothing either.
fn notify_reply(hooks: &KernelHooks, agent: &str) {
    let Ok(me) = arbos_core::load_agent(&hooks.place, &arbos_core::AgentId::new(agent)) else {
        return;
    };
    if me.parent.is_some() {
        return;
    }
    let lo = hooks
        .turn_lo
        .lock()
        .unwrap()
        .get(agent)
        .copied()
        .unwrap_or(0);
    let events = load_transcript(&hooks.layout(agent).transcript()).unwrap_or_default();
    let turn: Vec<&Event> = events.iter().filter(|e| e.seq >= lo).collect();
    // A question parked the turn: the ask already notified.
    if turn.iter().any(|e| matches!(e.kind, EventKind::Ask { .. })) {
        return;
    }
    let reply_at = turn.iter().rposition(
        |e| matches!(&e.kind, EventKind::Assistant { text, .. } if !text.trim().is_empty()),
    );
    let failed_at = turn
        .iter()
        .rposition(|e| matches!(&e.kind, EventKind::Notice { failed: true, .. }));
    let capped_at = turn.iter().rposition(|e| {
        matches!(&e.kind, EventKind::Notice { text, failed: false } if text.starts_with(arbos_core::spend::TURN_CAP_PREFIX))
    });
    let name = if me.name.is_empty() {
        me.id.to_string()
    } else {
        me.name.clone()
    };
    let text_of = |i: usize| match &turn[i].kind {
        EventKind::Assistant { text, .. } => text.trim().to_string(),
        EventKind::Notice { text, .. } => text.clone(),
        _ => String::new(),
    };
    // A turn that ended on a failure — a cost cap, a provider down — is
    // told as one, even when it said something earlier: "step one" is
    // not what the user needs to hear about a turn that stopped on money.
    // The per-turn cap closing the turn: the rule working, told as a
    // notice with the numbers and the fix — not a failure.
    if let Some(c) = capped_at
        && reply_at.is_none_or(|r| c > r)
    {
        hooks.notify(
            agent,
            "notice",
            &format!("{name} stopped at the per-turn cap"),
            &text_of(c),
        );
        return;
    }
    match (reply_at, failed_at) {
        (Some(r), Some(f)) if f > r => hooks.notify(
            agent,
            "error",
            &format!("{name}: turn stopped"),
            &text_of(f),
        ),
        (Some(r), _) => hooks.notify(agent, "reply", &format!("{name} replied"), &text_of(r)),
        (None, Some(f)) => {
            hooks.notify(agent, "error", &format!("{name}: turn failed"), &text_of(f))
        }
        (None, None) => {}
    }
}

/// A turn ended: its folder closes with what the transcript says, and a
/// child's parent hears about it.
pub fn finish_turn(hooks: &KernelHooks, agent: &str) {
    crate::chatdoor::reply_if_door_turn(hooks, agent);
    notify_reply(hooks, agent);
    notify_parent_done(hooks, agent);
    verify_reply_links(hooks, agent);
    record_spend(hooks, agent);
    close_turn_folder(hooks, agent, None);
    hooks.broadcast(hooks.plan_frame(agent));
    // Waited-for children that finished during this turn go to the
    // archive now that the parent is done with them.
    let waited: Vec<String> = {
        let mut map = hooks.archive_after.lock().unwrap();
        let ids: Vec<String> = map
            .iter()
            .filter(|(_, parent)| parent.as_str() == agent)
            .map(|(child, _)| child.clone())
            .collect();
        for id in &ids {
            map.remove(id);
        }
        ids.into_iter().map(|id| format!("agent:{id}")).collect()
    };
    if !waited.is_empty() {
        archive_finished(hooks, &waited);
    }
}

/// The turn's cost joins the place's spend (`spend.toml`); the user hears
/// once at 80 % of the cap and once at the cap (Cursor's "spend caps
/// checked mid-run; stop at the cap and report").
fn record_spend(hooks: &KernelHooks, agent: &str) {
    let _one_at_a_time = hooks.spend_lock.lock().unwrap_or_else(|p| p.into_inner());
    let events = load_transcript(&hooks.layout(agent).transcript()).unwrap_or_default();
    let lo = hooks
        .turn_lo
        .lock()
        .unwrap()
        .get(agent)
        .copied()
        .unwrap_or(0);
    let cost = events
        .iter()
        .filter(|e| e.seq >= lo)
        .rev()
        .find_map(|e| match &e.kind {
            EventKind::TurnComplete { usage } => usage.as_ref().and_then(|u| u.cost),
            _ => None,
        })
        .unwrap_or(0.0);
    let (spend, crossed) = match arbos_core::spend::add_turn(&hooks.place, cost) {
        Ok(x) => x,
        Err(e) => {
            crate::klog::warn("spend_record_failed", Some(agent), format!("{e:#}"));
            return;
        }
    };
    let Some(cap) = arbos_core::spend::cap_usd(&hooks.place) else {
        return;
    };
    let note = match crossed {
        Some(true) => format!(
            "Spend cap reached: ${:.2} of ${cap:.2} over {} turns. Workers, subscriptions, and spawns are refused until cap_usd under [spend] in .arbos/project.toml is raised or removed; your own messages to the main chat still run.",
            spend.spent_usd, spend.turns
        ),
        Some(false) => format!(
            "Spend is at ${:.2} of the ${cap:.2} cap ({:.0} %); work stops at the cap.",
            spend.spent_usd,
            spend.spent_usd / cap * 100.0
        ),
        None => return,
    };
    crate::klog::info("spend_mark", Some(agent), note.clone());
    // From the agent whose turn crossed the mark: the notice lands on its
    // top-level chat, where the user reads.
    if let Err(e) = hooks.notify_user(agent, &note) {
        crate::klog::warn("spend_notice_failed", Some(agent), format!("{e:#}"));
    }
}

/// The head of the kernel's own note about links to missing files; a
/// turn opened by that note is not checked again, so a reply that keeps
/// pointing at a file that never comes cannot loop.
pub const MISSING_LINKS: &str = "[kernel] your reply links files that do not exist";

/// Cursor's rule: verify an artifact before showing it. When a top-level
/// agent's turn ends, every local path its last reply links (`[x](p)`,
/// `![x](p)`) is checked against the store and the place; a missing one
/// goes back to the agent as a wake so it fixes the path or makes the
/// file, and the user is told the link is not there yet. Workers report
/// to a parent, not the user, so only top-level agents are checked.
fn verify_reply_links(hooks: &KernelHooks, agent: &str) {
    let Ok(me) = arbos_core::load_agent(&hooks.place, &arbos_core::AgentId::new(agent)) else {
        return;
    };
    if me.parent.is_some() {
        return;
    }
    let events = load_transcript(&hooks.layout(agent).transcript()).unwrap_or_default();
    let lo = hooks
        .turn_lo
        .lock()
        .unwrap()
        .get(agent)
        .copied()
        .unwrap_or(0);
    let turn: Vec<&Event> = events.iter().filter(|e| e.seq >= lo).collect();
    // A turn the kernel opened for this very reason is not checked again.
    let opened_by_us = turn.iter().any(|e| match &e.kind {
        EventKind::Wake { text: Some(t), .. } => t.starts_with(MISSING_LINKS),
        EventKind::User { text, .. } => text.starts_with(MISSING_LINKS),
        _ => false,
    });
    if opened_by_us {
        return;
    }
    let Some(reply) = turn.iter().rev().find_map(|e| match &e.kind {
        EventKind::Assistant { text, .. } if !text.trim().is_empty() => Some(text.clone()),
        _ => None,
    }) else {
        return;
    };
    let missing = missing_links(&hooks.place, &reply);
    if missing.is_empty() {
        return;
    }
    let list = missing
        .iter()
        .map(|p| format!("`{p}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let body = format!(
        "{MISSING_LINKS}: {list}. The user cannot open them. Read each path you meant, make the file or fix the link, then tell the user again in one line — or say plainly that the artifact is not ready."
    );
    let mut msg = inbox::Message::new("kernel", "wake", body);
    msg.wake = true;
    match inbox::deliver(&hooks.place, agent, &msg) {
        Ok(_) => {
            hooks.plan_changed(agent);
            crate::klog::info("reply_links_missing", Some(agent), missing.join(","));
        }
        Err(e) => crate::klog::warn("reply_links_check_failed", Some(agent), format!("{e:#}")),
    }
}

/// Local paths in `text`'s markdown links and images that exist neither
/// under the store (`.arbos/`), nor under the place, nor as given. URLs,
/// anchors, mailto, and `agents/…` (a worker, not a file) are not checked.
pub fn missing_links(place: &arbos_core::Place, text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("](") {
        let after = &rest[open + 2..];
        let Some(close) = after.find(')') else {
            break;
        };
        let raw = after[..close]
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim();
        rest = &after[close + 1..];
        if raw.is_empty()
            || raw.contains("://")
            || raw.starts_with("mailto:")
            || raw.starts_with('#')
            || raw.starts_with("agents/")
            || raw.starts_with(".arbos/agents/")
            || raw.starts_with("archive/agents/")
        {
            continue;
        }
        let path = raw.split('#').next().unwrap_or(raw);
        let candidates = [
            std::path::PathBuf::from(path),
            place.arbos().join(path.trim_start_matches("./")),
            place.path.join(path.trim_start_matches("./")),
        ];
        if candidates.iter().any(|c| c.exists()) {
            continue;
        }
        if !out.iter().any(|p| p == path) {
            out.push(path.to_string());
        }
    }
    out
}

/// The turn's last words and whether it ended well, from the transcript
/// lines the turn wrote (`lo` onward).
/// How a turn the user stopped reads in its report to the parent.
pub const USER_STOPPED: &str = "stopped by the user (Stop)";

/// The `interrupted` detail of the user's own Stop — the button, a stop
/// word, a pause — is `stop`, sometimes with where it landed ("stop
/// during model call"). A parent's `say mode=stop` says "stopped by
/// <who>"; the kernel's own says "kernel stopping".
fn user_stopped(detail: &str) -> bool {
    let d = detail.trim();
    d == "stop" || d.starts_with("stop during")
}

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
            // The per-turn cap's closing line: a stop the user's own rule
            // made, carried whole so the parent reads the numbers.
            EventKind::Notice {
                text,
                failed: false,
            } if text.starts_with(arbos_core::spend::TURN_CAP_PREFIX) => {
                stopped = Some(text.clone())
            }
            _ => {}
        }
    }
    if let Some(why) = stopped {
        // What it had before the stop rides along: a parent that stopped a
        // worker early wants the partial result, not only the reason.
        // The user's own Stop is a pause, not a failure, and reads so.
        let mut head = if user_stopped(&why) {
            USER_STOPPED.to_string()
        } else if why.starts_with(arbos_core::spend::TURN_CAP_PREFIX) {
            why.clone()
        } else {
            format!("stopped: {why}")
        };
        // The kernel's own reason for the stop (a cost cap, with the
        // numbers and what to change) rides along: the parent reads the
        // report, not the child's transcript.
        if let Some(f) = &failed {
            head.push_str(" — ");
            head.push_str(&arbos_core::text::clip(f, 300));
        }
        return (
            if last_text.is_empty() {
                head
            } else {
                format!(
                    "{head}\nLast words before the stop: {}",
                    arbos_core::text::clip(&last_text, 300)
                )
            },
            false,
        );
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
    for sub in subscription::list_visible(place, agent) {
        rows.push(PlanNode {
            id: SUB_ID_BIT | u64::from(sub.id),
            parent: 0,
            goal: sub.label(),
            status: if sub.paused {
                "blocked".into()
            } else {
                "pending".into()
            },
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
            origin: if item.section.is_empty() {
                String::new()
            } else {
                item.section.clone()
            },
            standing: false,
            inbox: false,
        });
    }
    rows
}
