//! Runs the tool calls of one model step with maximum safe concurrency.
//!
//! Invariants:
//! 1. Outcomes come back in call order, whatever order the calls finished.
//! 2. Every call gets an outcome: ran, errored, or skipped with a reason.
//! 3. A call that writes never starts before the model response is committed.
//! 4. A later call never blocks an earlier one.
//! 5. `max_parallel = 1` reproduces sequential semantics exactly.
//!
//! The one scheduling rule: call `i` may start when every earlier call `j < i`
//! that conflicts with `i` is done, fewer than `max_parallel` calls are
//! running, and either the batch is committed or `i` is read-only.
//!
//! Calls arrive over a channel so the executor can start read-only work while
//! the model is still streaming the rest of its response.

use anyhow::Result;
use arbos_core::{Event, EventKind, ToolRec};
use std::collections::HashMap;
use tokio::{sync::mpsc, task::JoinSet};

use crate::{
    access::Access,
    control::TurnControl,
    provider::ToolCall,
    tool::{RunCx, ToolOut, View},
    tools::{self, Prepared},
};

#[derive(Debug, Clone, Copy)]
pub struct BatchCfg {
    pub max_parallel: usize,
    /// Start read-only calls before the response is committed.
    pub speculate: bool,
}

pub enum Msg {
    Call(ToolCall),
    /// The response stream ended cleanly. Writes may start.
    Commit,
    /// The response stream failed. Stop everything; the caller discards.
    Abort,
}

pub enum Outcome {
    Ran {
        out: Result<ToolOut>,
        started: i64,
        ended: i64,
    },
    Skipped(String),
}

impl Outcome {
    /// The transcript record for this call.
    pub fn into_event(self, call: &ToolCall) -> Event {
        let (body, paths, child, images, error, started, ended, diff) = match self {
            Outcome::Ran {
                out: Ok(out),
                started,
                ended,
            } => (
                out.body,
                out.paths,
                out.child,
                out.images,
                None,
                Some(started),
                Some(ended),
                out.diff,
            ),
            Outcome::Ran {
                out: Err(e),
                started,
                ended,
            } => (
                e.to_string(),
                vec![],
                None,
                vec![],
                Some(e.to_string()),
                Some(started),
                Some(ended),
                None,
            ),
            Outcome::Skipped(why) => (
                why.clone(),
                vec![],
                None,
                vec![],
                Some(why),
                None,
                None,
                None,
            ),
        };
        // The transcript is the record the model and the user read: no
        // granted secret, and not the kernel's own key, gets written into it.
        let secrets = crate::secrets::store();
        let (body, error) = if secrets.has_any() {
            (secrets.redact(&body), error.map(|e| secrets.redact(&e)))
        } else {
            (body, error)
        };
        let result_size = body.len() as u64;
        Event::new(EventKind::Tool(ToolRec {
            name: call.name.clone(),
            call_id: call.id.clone(),
            paths,
            started,
            ended,
            result_size: Some(result_size),
            error,
            body: Some(body),
            args: Some(call.arguments.clone()),
            child,
            images,
            diff,
            label: call_label(&call.arguments),
        }))
    }
}

/// The model's `description` of a call, trimmed to a line, when it gave
/// one worth showing.
pub(crate) fn call_label(args: &serde_json::Value) -> Option<String> {
    let d = args.get("description")?.as_str()?.trim();
    if d.is_empty() {
        return None;
    }
    let one: String = d.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(one.chars().take(120).collect())
}

/// One line naming a call for the allow/deny question: the path, the
/// command, or the arguments, cut short.
pub(crate) fn summarise_call(_name: &str, args: &serde_json::Value) -> String {
    let key = ["command", "path", "patch", "url", "text", "brief"]
        .iter()
        .find_map(|k| args.get(*k).and_then(serde_json::Value::as_str));
    let body = match key {
        Some(v) => v.to_string(),
        None => args.to_string(),
    };
    let body = body.replace('\n', " ");
    let cut: String = body.chars().take(160).collect();
    if cut.len() < body.len() {
        format!("{cut}…")
    } else {
        cut
    }
}

/// Longest tool body kept on a transcript line. The transcript is the
/// full-body store, but one 100 MB line makes every reader (the kernel's
/// tail, the desktop, compaction) parse 100 MB per tick. Over the cap the
/// body goes to a side file the model can `read` in pieces.
pub const BODY_CAP: usize = 1024 * 1024;
const BODY_HEAD: usize = 64 * 1024;

/// A body larger than the model's view of it (the eviction limits) is
/// written whole to `<agent dir>/results/<call_id>.txt`, so the model can
/// `read` it in slices by the path the evicted view cites; the transcript
/// keeps it whole too, up to `BODY_CAP`. Past that the transcript line
/// holds a head plus the path instead.
pub fn cap_body(cx: &RunCx, tool: &str, call_id: &str, body: String) -> String {
    // A `read` result already has a file behind it — the one it read; the
    // evicted view cites that path, so no copy is kept.
    if tool == "read" && body.len() <= BODY_CAP {
        return body;
    }
    if !crate::evict::spills(&body) {
        return body;
    }
    let dir = arbos_core::Layout::new(&cx.place, cx.agent.id.as_str())
        .dir
        .join("results");
    let path = dir.join(crate::evict::spill_name(call_id));
    let spilled = std::fs::create_dir_all(&dir)
        .and_then(|_| std::fs::write(&path, &body))
        .is_ok();
    if body.len() <= BODY_CAP {
        return body;
    }
    let cut = body
        .char_indices()
        .map(|(i, _)| i)
        .take_while(|&i| i <= BODY_HEAD)
        .last()
        .unwrap_or(0);
    let mut head = body[..cut].to_string();
    head.push_str(&format!(
        "\n[… {} MB more{}]",
        body.len() / (1024 * 1024),
        if spilled {
            format!(" in {}; read it in pieces", path.display())
        } else {
            String::new()
        }
    ));
    head
}

enum State {
    /// Arrived; not yet through preflight.
    New,
    Ready(Prepared),
    Running,
    Done(Outcome),
}

struct Slot {
    call: ToolCall,
    access: Access,
    state: State,
}

impl Slot {
    fn is_done(&self) -> bool {
        matches!(self.state, State::Done(_))
    }
}

/// Drive one batch to completion. Returns `(call, outcome)` in call order.
pub async fn run(
    view: View,
    cx: RunCx,
    control: TurnControl,
    cfg: BatchCfg,
    mut rx: mpsc::UnboundedReceiver<Msg>,
) -> Vec<(ToolCall, Outcome)> {
    let max_parallel = cfg.max_parallel.max(1);
    let mut slots: Vec<Slot> = Vec::new();
    let mut set: JoinSet<(usize, Outcome)> = JoinSet::new();
    let mut task_slot: HashMap<tokio::task::Id, usize> = HashMap::new();
    let mut committed = false;
    let mut aborted = false;
    let mut rx_open = true;

    loop {
        // Preflight arrivals in order. Hooks are user scripts; order matters.
        #[allow(clippy::needless_range_loop)]
        for i in 0..slots.len() {
            if !matches!(slots[i].state, State::New) {
                continue;
            }
            let call_cx = RunCx {
                call_id: slots[i].call.id.clone(),
                ..cx.clone()
            };
            match tools::preflight(
                &view,
                &call_cx,
                &slots[i].call.name,
                &slots[i].call.arguments,
            )
            .await
            {
                Ok(prepared) => {
                    let mut access = prepared.plan.access.clone();
                    if prepared.plan.interactive {
                        access.exclusive = true;
                    }
                    slots[i].access = access;
                    slots[i].state = State::Ready(prepared);
                }
                Err(e) => {
                    let now = arbos_core::now_ms();
                    slots[i].state = State::Done(Outcome::Ran {
                        out: Err(e),
                        started: now,
                        ended: now,
                    });
                }
            }
        }

        let stopped = control.is_stopped() || aborted;
        // A steer waits for the tool boundary; the calls the model already
        // decided on run (Cursor's rule, and the multitasking audit: a
        // steer before a batch skipped the user's own spawn by 30 s). Only
        // a steer that says stop cancels what has not started.
        let steered = arbos_core::inbox::has_stop_steer(&cx.place, cx.agent.id.as_str());
        let running = slots
            .iter()
            .filter(|s| matches!(s.state, State::Running))
            .count();

        if stopped {
            set.abort_all();
            for s in &mut slots {
                if !s.is_done() {
                    s.state = State::Done(Outcome::Skipped(if aborted {
                        "skipped: model response failed".into()
                    } else {
                        "skipped: interrupted".into()
                    }));
                }
            }
        } else if steered && running == 0 {
            // The user said stop: nothing more starts; the words land at
            // the boundary and the turn takes it from there.
            for s in &mut slots {
                if !s.is_done() {
                    s.state = State::Done(Outcome::Skipped("skipped: user said stop".into()));
                }
            }
        } else if !steered {
            let mut running = running;
            for i in 0..slots.len() {
                if running >= max_parallel {
                    break;
                }
                if !matches!(slots[i].state, State::Ready(_)) {
                    continue;
                }
                let early_ok = cfg.speculate && slots[i].access.is_readonly();
                if !committed && !early_ok {
                    continue;
                }
                let blocked = (0..i)
                    .any(|j| !slots[j].is_done() && slots[j].access.conflicts(&slots[i].access));
                if blocked {
                    continue;
                }
                let State::Ready(prepared) = std::mem::replace(&mut slots[i].state, State::Running)
                else {
                    unreachable!()
                };
                running += 1;
                let call = slots[i].call.clone();
                let call_cx = RunCx {
                    call_id: call.id.clone(),
                    ..cx.clone()
                };
                let started = arbos_core::now_ms();
                cx.hooks.emit(&Event::new(EventKind::Tool(ToolRec {
                    name: call.name.clone(),
                    call_id: call.id.clone(),
                    paths: vec![],
                    started: Some(started),
                    ended: None,
                    result_size: None,
                    error: None,
                    body: None,
                    args: Some(call.arguments.clone()),
                    child: None,
                    images: vec![],
                    diff: None,
                    label: call_label(&call.arguments),
                })));
                for note in &prepared.notices {
                    hook_notice(&cx, note);
                }
                let handle = set.spawn(async move {
                    let out = run_with_hooks(prepared, &call_cx, &call).await;
                    (
                        i,
                        Outcome::Ran {
                            out,
                            started,
                            ended: arbos_core::now_ms(),
                        },
                    )
                });
                task_slot.insert(handle.id(), i);
            }
        }

        let all_done = slots.iter().all(Slot::is_done);
        if all_done && (!rx_open || stopped) {
            break;
        }

        tokio::select! {
            biased;
            _ = control.cancel().cancelled(), if !stopped => {}
            msg = rx.recv(), if rx_open => match msg {
                Some(Msg::Call(call)) => slots.push(Slot { call, access: Access::none(), state: State::New }),
                Some(Msg::Commit) => committed = true,
                Some(Msg::Abort) => aborted = true,
                None => { rx_open = false; committed = true; }
            },
            joined = set.join_next(), if !set.is_empty() => {
                match joined {
                    Some(Ok((i, outcome))) => slots[i].state = State::Done(outcome),
                    Some(Err(e)) => {
                        // A panicking or aborted tool task.
                        if let Some(&i) = task_slot.get(&e.id()) {
                            if matches!(slots[i].state, State::Running) {
                                let now = arbos_core::now_ms();
                                slots[i].state = State::Done(Outcome::Ran {
                                    out: Err(anyhow::anyhow!("tool task: {e}")),
                                    started: now,
                                    ended: now,
                                });
                            }
                        }
                    }
                    None => {}
                }
            }
            else => break,
        }
    }

    let outcomes: Vec<(ToolCall, Outcome)> = slots
        .into_iter()
        .map(|s| {
            let outcome = match s.state {
                State::Done(o) => o,
                _ => Outcome::Skipped("skipped: batch ended".into()),
            };
            (s.call, outcome)
        })
        .collect();
    log_speedup(&cx.agent.id, &outcomes);
    outcomes
}

/// One line per multi-call batch: how long the tools took end to end versus
/// added up. The ratio is the parallel speedup.
fn log_speedup(agent: &arbos_core::AgentId, outcomes: &[(ToolCall, Outcome)]) {
    let spans: Vec<(i64, i64)> = outcomes
        .iter()
        .filter_map(|(_, o)| match o {
            Outcome::Ran { started, ended, .. } => Some((*started, *ended)),
            Outcome::Skipped(_) => None,
        })
        .collect();
    if spans.len() < 2 {
        return;
    }
    let wall =
        spans.iter().map(|s| s.1).max().unwrap_or(0) - spans.iter().map(|s| s.0).min().unwrap_or(0);
    let sum: i64 = spans.iter().map(|(a, b)| b - a).sum();
    eprintln!(
        "tools {agent}: {} calls, wall {wall}ms, sum {sum}ms, speedup {:.1}x",
        spans.len(),
        if wall > 0 {
            sum as f64 / wall as f64
        } else {
            1.0
        }
    );
}

/// The call itself, between its hooks: a before-tool `ask` goes to the user
/// first (deny = tool error), then the tool runs, then after-tool hooks see
/// the result and may add context for the model.
async fn run_with_hooks(prepared: Prepared, cx: &RunCx, call: &ToolCall) -> Result<ToolOut> {
    let name = call.name.clone();
    if let Some(question) = &prepared.ask {
        let allowed = tokio::select! {
            r = cx.hooks.approve(&cx.agent.id, &name, question) => r.unwrap_or(false),
            _ = cx.cancel.cancelled() => false,
        };
        if !allowed {
            anyhow::bail!(
                "the user did not allow {name} ({question}). Do not retry it unchanged; say what you wanted to do and why, and go on with what is allowed."
            );
        }
    }
    let args = prepared.args.clone();
    // The first edit of a task states its mechanism or does not run.
    let recorded = crate::mechanism::gate(&cx.place, &cx.agent.id, &name, &args)?;
    let taken = crate::repro::gate(&cx.place, &cx.agent.id, &name)?;
    let result = prepared
        .tool
        .run(cx.clone(), prepared.args)
        .await
        .map(|mut out| {
            out.body = cap_body(cx, &name, &call.id, std::mem::take(&mut out.body));
            if let Some(line) = &recorded {
                out.body.push_str("\n\nMechanism recorded for this task: ");
                out.body.push_str(line);
            }
            if let Some(note) = &taken {
                out.body.push_str("\n\n");
                out.body.push_str(note);
            }
            out
        });
    let (body, error, paths) = match &result {
        Ok(out) => (out.body.clone(), None, out.paths.clone()),
        Err(e) => (String::new(), Some(e.to_string()), Vec::new()),
    };
    let after = {
        let place = cx.place.clone();
        let agent = cx.agent.clone();
        let name = name.clone();
        let cwd = cx.cwd.clone();
        tokio::task::spawn_blocking(move || {
            let mut after = tools::file_hooks::after_tool(
                &place,
                &agent,
                &name,
                &args,
                &body,
                error.as_deref(),
                &paths,
            );
            // A source edit reports which existing tests name what it
            // changed; "none" is the wrong-layer signal (see git::coverage_note).
            if error.is_none() && matches!(name.as_str(), "edit" | "write" | "apply_patch") {
                if let Some(note) = tools::git::coverage_note(&cwd, &paths) {
                    after.context.push(note);
                }
            }
            after
        })
        .await
        .unwrap_or_default()
    };
    for note in &after.notices {
        hook_notice(cx, note);
    }
    let context: Vec<String> = prepared.context.into_iter().chain(after.context).collect();
    match result {
        Ok(mut out) => {
            for c in context {
                out.body.push_str("\n\n[hook] ");
                out.body.push_str(&c);
            }
            Ok(out)
        }
        Err(e) => Err(e),
    }
}

/// Hook trouble goes on the transcript: the user should see that a hook
/// failed or that `hooks.toml` is wrong, and it should survive the turn.
fn hook_notice(cx: &RunCx, text: &str) {
    let path = arbos_core::Layout::new(&cx.place, cx.agent.id.as_str()).transcript();
    let _ = arbos_core::append_event(
        &path,
        &Event::new(EventKind::Notice {
            text: text.to_string(),
            failed: false,
        }),
    );
}
