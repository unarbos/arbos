//! Tools only the kernel can provide: other agents, the user, the browser.
//!
//! They touch nothing in the place, so they run beside anything — except
//! `ask`, which waits on a human and must run alone and in order. The
//! browser has one session per agent, so browser calls order among themselves.

use anyhow::Result;
use arbos_core::{Layout, wire::Frame};
use arbos_engine::{
    Access, BoxFuture, Plan, PlanCx, Resource, RunCx, Tool, ToolOut, opt_bool, opt_strings,
    simple_schema, typed_schema,
};
use base64::Engine;
use serde_json::Value;
use std::{path::PathBuf, sync::Arc};

use crate::hooks::{Isolate, KernelHooks, SayMode};
use crate::pty::PtyHub;

/// The one browser page an agent has. The desktop keys its row on this.
const BROWSER_PAGE: &str = "b1";

/// A panel open/close for the desktop tree. `ids` names the rows.
fn board(owner: &str, action: &str, panel: &str, ids: Vec<String>) -> Frame {
    Frame::Board {
        owner: owner.to_string(),
        action: action.into(),
        panel: panel.into(),
        terminal_ids: ids,
        cwd: None,
        title: None,
        url: None,
    }
}

pub struct Spawn(pub Arc<KernelHooks>);
pub struct Say(pub Arc<KernelHooks>);
pub struct Ask(pub Arc<KernelHooks>);
pub struct Browser(pub Arc<KernelHooks>);
pub struct Terminal {
    pub hooks: Arc<KernelHooks>,
    pub ptys: Arc<PtyHub>,
}

fn req<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing {key}"))
}

fn opt_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
}

impl Tool for Spawn {
    fn name(&self) -> &'static str {
        "spawn"
    }
    fn schema(&self) -> Value {
        typed_schema(
            "spawn",
            "Start a worker with a kickoff: name + task, read_first, do, rules, output, report (or a raw brief). Pass existing content as paths. Its reports and a [done] arrive here as messages. isolate=worktree for a worker that edits code beside another. kind = an agent definition; host = another machine.",
            &[
                (
                    "name",
                    "Short imperative label, ~5 words; becomes the id.",
                    false,
                    "string",
                ),
                (
                    "task",
                    "This worker's own piece of the ask, in the user's terms (or give brief). Not the whole request.",
                    false,
                    "string",
                ),
                (
                    "read_first",
                    "Paths to read first (default: project-context.md, notes.md).",
                    false,
                    "string",
                ),
                ("do", "Numbered steps.", false, "string"),
                (
                    "rules",
                    "Repo/base branch, no merging; a default covers the usual.",
                    false,
                    "string",
                ),
                (
                    "output",
                    "Exact output paths under .arbos/docs|internal|media.",
                    false,
                    "string",
                ),
                ("report", "What to report back.", false, "string"),
                (
                    "brief",
                    "Raw brief when the fields do not fit.",
                    false,
                    "string",
                ),
                ("kind", "Leave out unless a Kind fits.", false, "string"),
                ("isolate", "Leave out, or worktree.", false, "string"),
                (
                    "host",
                    "Leave out to run here. Else a name from Machines.",
                    false,
                    "string",
                ),
                (
                    "wait",
                    "Block until its first report and return it (quick sub-tasks only).",
                    false,
                    "boolean",
                ),
            ],
        )
    }
    fn plan(&self, _cx: &PlanCx, _args: &Value) -> Result<Plan> {
        Ok(Plan::access(Access::none()))
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        let hooks = Arc::clone(&self.0);
        Box::pin(async move {
            let name = opt_str(&args, "name")
                .map(str::trim)
                .filter(|s| !s.is_empty());
            // The user's "show me" travels with the brief: a kickoff that
            // says "run … and report" lost it, and no image was made.
            // Judged on the user's words that opened this turn, unless
            // the brief already asks for an image itself.
            let show = arbos_core::store::turn_user_text(&cx.place, cx.agent.id.as_str())
                .is_some_and(|t| arbos_core::store::asks_to_see(&t))
                && !["task", "do", "brief"]
                    .iter()
                    .filter_map(|k| opt_str(&args, k))
                    .any(|t| {
                        let t = t.to_ascii_lowercase();
                        t.contains("screenshot") || t.contains("image") || t.contains("capture")
                    });
            // The template wins when a task is given; a raw brief is the
            // fallback. Neither is an error the model can act on.
            let rendered = match (opt_str(&args, "task"), opt_str(&args, "brief")) {
                (Some(task), _) => arbos_core::store::Kickoff {
                    read_first: opt_str(&args, "read_first"),
                    task,
                    do_: opt_str(&args, "do"),
                    rules: opt_str(&args, "rules"),
                    output: opt_str(&args, "output"),
                    report: opt_str(&args, "report"),
                    base_branch: arbos_core::store::current_branch(cx.place.path()),
                    show,
                }
                .render(),
                (None, Some(brief)) if show => {
                    format!("{brief}\n\nShow: {}\n", arbos_core::store::KICKOFF_SHOW)
                }
                (None, Some(brief)) => brief.to_string(),
                (None, None) => {
                    anyhow::bail!("spawn: give `task` (with the template fields) or a raw `brief`")
                }
            };
            let brief = rendered.as_str();
            let model = opt_str(&args, "model");
            let readonly = opt_bool(&args, "readonly").unwrap_or(false);
            let cwd = opt_str(&args, "cwd").map(PathBuf::from);
            let wait = opt_bool(&args, "wait").unwrap_or(false);
            let wait_secs = args
                .get("wait_secs")
                .and_then(Value::as_u64)
                .unwrap_or(600)
                .clamp(1, 6 * 3600);
            let mut host_note = None;
            let host = match opt_str(&args, "host") {
                Some(host) => match crate::remote::choose_host(&hooks.place, host) {
                    crate::remote::HostChoice::Local { note } => {
                        host_note = note;
                        None
                    }
                    crate::remote::HostChoice::Remote => Some(host),
                },
                None => None,
            };
            if let Some(host) = host {
                let (id, where_) = crate::remote::spawn_remote(
                    Arc::clone(&hooks),
                    cx.agent.clone(),
                    name.map(str::to_string),
                    brief.to_string(),
                    host.to_string(),
                )
                .await?;
                return Ok(ToolOut {
                    body: format!("spawned {id} {where_}: {brief}"),
                    paths: vec![format!(".arbos/agents/{id}")],
                    child: Some(id.to_string()),
                    images: vec![],
                    diff: None,
                    park: None,
                });
            }
            let mut ran_here = String::new();
            let kind = opt_str(&args, "kind");
            let kind_owned = kind.map(str::to_string);
            let raw = opt_str(&args, "isolate")
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("none");
            let mut isolate = Isolate::parse(raw).ok_or_else(|| {
                anyhow::anyhow!("spawn: isolate must be none or worktree, not {raw:?}")
            })?;
            // A worktree protects the parent's checkout; a place that is no
            // repository has none to protect, so the child runs in place.
            if isolate == Isolate::Worktree && !crate::worktree::is_repo(&hooks.place.path) {
                isolate = Isolate::None;
                ran_here.push_str(" (not a git repository, so no worktree: it works in place)");
            }
            // git runs on the blocking pool: a large checkout takes seconds.
            let agent = cx.agent.clone();
            let brief_owned = brief.to_string();
            let name_owned = name.map(str::to_string);
            let model_owned = model.map(str::to_string);
            let spawner = Arc::clone(&hooks);
            let (id, worktree) = tokio::task::spawn_blocking(move || {
                spawner.spawn_named(
                    &agent,
                    name_owned.as_deref(),
                    &brief_owned,
                    model_owned.as_deref(),
                    None,
                    readonly,
                    cwd,
                    isolate,
                    kind_owned.as_deref(),
                )
            })
            .await
            .map_err(|e| anyhow::anyhow!("spawn task: {e}"))??;
            let shown = match name {
                Some(n) => format!("{n}\n{brief}"),
                None => brief.to_string(),
            };
            let mut body = match kind {
                Some(k) => format!("spawned {id} (kind {k}){ran_here}: {shown}"),
                None => format!("spawned {id}{ran_here}: {shown}"),
            };
            if let Some(note) = &host_note {
                body.push_str(&format!("\nNote: {note}."));
            }
            let mut paths = vec![format!(".arbos/agents/{id}")];
            if let Some(w) = &worktree {
                body.push_str(&format!(
                    "\nIt works in its own worktree {} on branch {} (cut from {}). Your checkout is untouched. The worktree is removed when the worker is archived and has nothing uncommitted (its commits stay on the branch); to take it down yourself: {}",
                    w.path.display(),
                    w.branch,
                    w.base,
                    w.removal()
                ));
                paths.push(w.path.display().to_string());
            }
            if wait {
                let rx = hooks.wait_for(cx.agent.id.as_str(), id.as_str());
                let report = tokio::select! {
                    r = rx => r.ok(),
                    _ = tokio::time::sleep(std::time::Duration::from_secs(wait_secs)) => None,
                    _ = cx.cancel.cancelled() => {
                        hooks.stop_waiting(id.as_str());
                        anyhow::bail!("interrupted while waiting for {id}; it keeps working")
                    }
                };
                match report {
                    Some(text) => body = format!("{id} reports:\n{text}"),
                    None => {
                        hooks.stop_waiting(id.as_str());
                        body.push_str(&format!(
                            "\n{id} is still working after {wait_secs}s; its report will arrive here as a message from it."
                        ));
                    }
                }
            }
            Ok(ToolOut {
                body,
                paths,
                child: Some(id.to_string()),
                images: vec![],
                diff: None,
                park: None,
            })
        })
    }
}

impl Tool for Say {
    fn name(&self) -> &'static str {
        "say"
    }
    fn schema(&self) -> Value {
        simple_schema(
            "say",
            "Message another agent (id or name, see <<peers>>) or the user. mode note waits for their next turn; request queues a turn and their reply arrives here; steer lands in a running turn at its next tool step (redirect or stop a worker). Then end your turn.",
            &[
                ("to", "Agent id or name, user, or <machine>/<agent>.", true),
                ("text", "The message; the recipient sees only this.", true),
                (
                    "mode",
                    "note (default), request, steer, or stop (end a worker's turn now; its done brings what it had).",
                    false,
                ),
            ],
        )
    }
    fn plan(&self, _cx: &PlanCx, _args: &Value) -> Result<Plan> {
        Ok(Plan::access(Access::none()))
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        let hooks = Arc::clone(&self.0);
        Box::pin(async move {
            let to = req(&args, "to")?;
            let text = req(&args, "text")?;
            let raw = opt_str(&args, "mode").unwrap_or("note");
            let mode = match SayMode::parse(raw) {
                Some(m) => m,
                // The old wire: wake:true meant request.
                None if opt_bool(&args, "wake") == Some(true) => SayMode::Request,
                None => {
                    anyhow::bail!("say: mode must be note, request, steer, or stop, not {raw:?}")
                }
            };
            // `<machine>/<agent>`: an agent on another machine of the hub.
            // Only when that machine is on the roster; a local agent may
            // be named with a slash in a brief, and it stays local.
            if let Some(target) = arbos_core::MeshTarget::parse(to)
                && let Some(info) = arbos_core::hub::roster_machine(&hooks.place, &target.machine)
            {
                let cfg = crate::hub_link::config_from_env()?.ok_or_else(|| {
                    anyhow::anyhow!(
                        "say: {} is on the hub but this kernel has no hub configured (start it with --hub)",
                        info.name
                    )
                })?;
                let from = format!("{}/{}", cfg.machine, cx.agent.id);
                let receipt = crate::hub_link::deliver(
                    &cfg,
                    &info.name,
                    target.project.as_deref(),
                    &target.agent,
                    &from,
                    text,
                )
                .await?;
                return Ok(ToolOut::text(receipt));
            }
            let receipt = hooks.say(&cx.agent.id, to, text, mode, cx.hops)?;
            Ok(ToolOut::text(receipt))
        })
    }
}

pub struct PlanTool(pub Arc<KernelHooks>);

impl Tool for PlanTool {
    fn name(&self) -> &'static str {
        "plan"
    }
    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "plan",
                "description": "Your checklist (notes.md). set items:[\"plain\", {\"section\":\"Phase\",\"text\":\"[label](target) — readout\"}] replaces it (a markdown checklist string works too); add text; check n readout [target]; update n text; remove n; show. Schedules nothing.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "op": {"type": "string", "enum": ["set", "add", "check", "update", "remove", "show"]},
                        "items": {
                            "type": "array",
                            "description": "set: strings or {section, text}.",
                            "items": {}
                        },
                        "section": {"type": "string", "description": "add."},
                        "text": {"type": "string", "description": "add/update."},
                        "n": {"type": "integer", "description": "check/update/remove."},
                        "readout": {"type": "string", "description": "check: fresh status."},
                        "target": {"type": "string", "description": "check: the deliverable link."}
                    },
                    "required": ["op"]
                }
            }
        })
    }
    fn plan(&self, _cx: &PlanCx, _args: &Value) -> Result<Plan> {
        Ok(Plan::access(Access::none()))
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        let hooks = Arc::clone(&self.0);
        Box::pin(async move {
            let agent = cx.agent.id.as_str();
            // What was meant when `op` is missing or spelt another way: a
            // list under any of the usual keys is a set; text alone is a
            // set too when it holds a checklist, else an add.
            let list_key = [
                "items",
                "goals",
                "nodes",
                "steps",
                "tasks",
                "checklist",
                "plan",
                "list",
            ]
            .into_iter()
            .find(|k| args.get(*k).is_some_and(|v| !v.is_null()));
            let op_raw = opt_str(&args, "op")
                .or_else(|| opt_str(&args, "action"))
                .unwrap_or("");
            let op = match op_raw.trim().to_ascii_lowercase().as_str() {
                "set" | "replace" | "write" | "create" | "new" => "set",
                "add" | "append" | "push" => "add",
                "check" | "done" | "complete" | "finish" | "tick" => "check",
                "update" | "edit" | "rename" => "update",
                "remove" | "delete" | "drop" | "cancel" => "remove",
                "show" | "list" | "get" | "read" | "view" => "show",
                "" if list_key.is_some() => "set",
                "" if opt_str(&args, "text")
                    .or_else(|| opt_str(&args, "markdown"))
                    .is_some_and(|t| t.contains("\n")) =>
                {
                    "set"
                }
                "" if opt_str(&args, "text").is_some() => "add",
                "" if args.as_object().is_none_or(|o| o.is_empty()) => "show",
                other => anyhow::bail!(
                    "plan: op {other:?} is not one of set, add, check, update, remove, show. {}",
                    arbos_core::notes::SHAPES
                ),
            };
            let mut notes = hooks.notes(agent);
            let n = || -> Result<usize> {
                ["n", "item", "index", "id", "number"]
                    .iter()
                    .find_map(|k| args.get(*k))
                    .and_then(|v| {
                        v.as_u64()
                            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
                    })
                    .map(|v| v as usize)
                    .ok_or_else(|| {
                        anyhow::anyhow!("plan {op}: n (the item number from show) is required")
                    })
            };
            let ack = match op {
                "set" => {
                    let source = list_key
                        .and_then(|k| args.get(k))
                        .or_else(|| args.get("text"))
                        .or_else(|| args.get("markdown"))
                        .or_else(|| args.get("content"))
                        .ok_or_else(|| {
                            anyhow::anyhow!("plan set: no items. {}", arbos_core::notes::SHAPES)
                        })?;
                    let items = arbos_core::notes::items_from_json(source).map_err(|e| {
                        anyhow::anyhow!("plan set: {e:#}. {}", arbos_core::notes::SHAPES)
                    })?;
                    notes.set(&items);
                    hooks.save_notes(agent, &notes)?;
                    format!("Set {} item(s).", items.len())
                }
                "add" => {
                    let text = opt_str(&args, "text")
                        .or_else(|| opt_str(&args, "item"))
                        .or_else(|| opt_str(&args, "label"))
                        .or_else(|| opt_str(&args, "goal"))
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "plan add: text is required. {}",
                                arbos_core::notes::SHAPES
                            )
                        })?;
                    let k = notes.add(opt_str(&args, "section").unwrap_or(""), text);
                    hooks.save_notes(agent, &notes)?;
                    format!("Added item {k}.")
                }
                "check" => {
                    let k = n()?;
                    let done = args.get("done").and_then(|v| v.as_bool()).unwrap_or(true);
                    let item = notes.check_with_target(
                        k,
                        done,
                        opt_str(&args, "readout"),
                        opt_str(&args, "target"),
                    )?;
                    hooks.save_notes(agent, &notes)?;
                    format!(
                        "{} {}: {}. Items are renumbered after a check (done ones sink); use the numbers in this list.",
                        if done { "Checked" } else { "Reopened" },
                        k,
                        item.text
                    )
                }
                "update" => {
                    let k = n()?;
                    notes.update(k, req(&args, "text")?)?;
                    hooks.save_notes(agent, &notes)?;
                    format!("Updated item {k}.")
                }
                "remove" => {
                    let k = n()?;
                    let item = notes.remove(k)?;
                    hooks.save_notes(agent, &notes)?;
                    format!("Removed: {}", item.text)
                }
                _ => String::new(),
            };
            let shown = hooks.notes(agent).show();
            Ok(ToolOut::text(if ack.is_empty() {
                shown
            } else {
                format!("{ack}\n\n{shown}")
            }))
        })
    }
}

pub struct SubscribeTool(pub Arc<KernelHooks>);

impl Tool for SubscribeTool {
    fn name(&self) -> &'static str {
        "subscribe"
    }
    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "subscribe",
                "description": "The only clock; a firing arrives as a message from subscription:N. add kind: timer (every|after, prompt); shell (cmd, every: no model turn, wakes you on failure; deliver_to user + notify \"…{output}\" sends the reading to the user); github_pr (repo, pr); github_ci (repo, pr | branch: a branch's workflow runs); inbox (path, every). list; remove|pause|resume id.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "op": {"type": "string", "enum": ["add", "list", "remove", "pause", "resume"]},
                        "id": {"type": "integer", "description": "remove/pause/resume."},
                        "kind": {"type": "string", "enum": ["timer", "shell", "github_pr", "github_ci", "inbox"]},
                        "prompt": {"type": "string", "description": "what you are told."},
                        "every": {"type": "string", "description": "e.g. 1h, 10m."},
                        "after": {"type": "string", "description": "once, e.g. 30m."},
                        "cmd": {"type": "string", "description": "shell command."},
                        "deliver_to": {"type": "string", "enum": ["agent", "user", "none"], "description": "shell: default agent."},
                        "notify": {"type": "string", "description": "deliver_to user: line with {output}."},
                        "repo": {"type": "string", "description": "owner/name."},
                        "pr": {"type": "integer", "description": "PR number."},
                        "branch": {"type": "string", "description": "github_ci: watch this branch's runs instead of a PR."},
                        "path": {"type": "string", "description": "inbox: folder."}
                    },
                    "required": ["op"]
                }
            }
        })
    }
    fn plan(&self, _cx: &PlanCx, _args: &Value) -> Result<Plan> {
        Ok(Plan::access(Access::none()))
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        let hooks = Arc::clone(&self.0);
        Box::pin(async move {
            let agent = cx.agent.id.as_str();
            let op = opt_str(&args, "op").unwrap_or("add");
            let id = || -> Result<u32> {
                args.get("id")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32)
                    .ok_or_else(|| anyhow::anyhow!("subscribe {op}: id is required (see list)"))
            };
            let text = match op {
                "add" => {
                    let kind = opt_str(&args, "kind").unwrap_or("timer").to_string();
                    let sub = arbos_core::subscription::Subscription {
                        id: 0,
                        kind,
                        prompt: opt_str(&args, "prompt").unwrap_or("").to_string(),
                        every: opt_str(&args, "every").map(str::to_string),
                        at: opt_str(&args, "at").map(str::to_string),
                        once: false,
                        cmd: opt_str(&args, "cmd").map(str::to_string),
                        path: opt_str(&args, "path").map(str::to_string),
                        repo: opt_str(&args, "repo").map(str::to_string),
                        pr: args.get("pr").and_then(|v| v.as_u64()).filter(|n| *n > 0),
                        branch: opt_str(&args, "branch")
                            .map(str::trim)
                            .filter(|b| !b.is_empty())
                            .map(str::to_string),
                        deliver_to: opt_str(&args, "deliver_to").unwrap_or("agent").to_string(),
                        notify: opt_str(&args, "notify").map(str::to_string),
                        expires: opt_str(&args, "expires").map(str::to_string),
                        paused: false,
                        internal: false,
                        created: String::new(),
                        next_due: None,
                        last_fired: None,
                        last: String::new(),
                        error: None,
                        seen: None,
                    };
                    let sub = hooks.subscribe(agent, sub, opt_str(&args, "after"))?;
                    format!(
                        "Subscribed #{} ({} · {}). It fires as a message from subscription:{}; end the turn — you are woken when it does.",
                        sub.id,
                        sub.kind,
                        sub.when_line(),
                        sub.id
                    )
                }
                "remove" => {
                    let k = id()?;
                    if hooks.unsubscribe(agent, k)? {
                        format!("Removed subscription #{k}.")
                    } else {
                        format!("No subscription #{k}.")
                    }
                }
                "pause" | "resume" => {
                    let k = id()?;
                    hooks.plan_op(agent, crate::plan::SUB_ID_BIT | u64::from(k), op, "")?;
                    format!("Subscription #{k} {op}d.")
                }
                "list" => String::new(),
                other => anyhow::bail!(
                    "subscribe: unknown op {other:?} (add, list, remove, pause, resume)"
                ),
            };
            let subs = arbos_core::subscription::list_visible(&hooks.place, agent);
            let listing = if subs.is_empty() {
                "(no subscriptions)".to_string()
            } else {
                subs.iter()
                    .map(|s| {
                        let mut line =
                            format!("#{} {} — {} · {}", s.id, s.kind, s.label(), s.when_line());
                        if !s.last.is_empty() {
                            line.push_str(&format!(" · last: {}", s.last));
                        }
                        line
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            Ok(ToolOut::text(if text.is_empty() {
                listing
            } else {
                format!("{text}\n\n{listing}")
            }))
        })
    }
}

impl Tool for Ask {
    fn name(&self) -> &'static str {
        "ask"
    }
    fn schema(&self) -> Value {
        typed_schema(
            "ask",
            "Ask the user a question.",
            &[
                ("question", "", true, "string"),
                ("options", "Choices.", false, "array"),
            ],
        )
    }
    fn plan(&self, _cx: &PlanCx, _args: &Value) -> Result<Plan> {
        Ok(Plan::access(Access::exclusive()).interactive())
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        let hooks = Arc::clone(&self.0);
        Box::pin(async move {
            let q = req(&args, "question")?;
            let options = opt_strings(&args, "options");
            // Park: the question is a file, the turn ends after this call,
            // and the user's answer starts the next turn as a message
            // (Cursor's shape; restart-safe).
            let id = hooks.ask(&cx.agent.id, q, &options, &cx.call_id)?;
            Ok(ToolOut::parked(
                format!(
                    "Question {id} is with the user. This turn ends here; their answer arrives as your next message."
                ),
                "Waiting for your answer",
            ))
        })
    }
}

impl Tool for Terminal {
    fn name(&self) -> &'static str {
        "terminal"
    }
    fn schema(&self) -> Value {
        simple_schema(
            "terminal",
            "Open a shell the user sees as a sub-terminal of this chat (never Terminal.app or an editor's terminal).",
            &[("action", "open", true), ("cwd", "Start directory.", false)],
        )
    }
    fn plan(&self, _cx: &PlanCx, _args: &Value) -> Result<Plan> {
        Ok(Plan::access(Access::none()))
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        let hooks = Arc::clone(&self.hooks);
        let ptys = Arc::clone(&self.ptys);
        Box::pin(async move {
            let action = req(&args, "action")?;
            if !action.eq_ignore_ascii_case("open") {
                anyhow::bail!("terminal: action must be open (got {action:?})");
            }
            let dir = match opt_str(&args, "cwd") {
                Some(cwd) => {
                    let path = std::path::PathBuf::from(cwd);
                    if path.is_absolute() {
                        path
                    } else {
                        cx.cwd.join(cwd)
                    }
                }
                None => cx.cwd.clone(),
            };
            if !dir.is_dir() {
                anyhow::bail!("terminal: open: {} is not a directory", dir.display());
            }
            let id = ptys.next_id();
            ptys.spawn_shell(&id, &dir, cx.agent.id.as_str())?;
            hooks.broadcast(Frame::Board {
                owner: cx.agent.id.to_string(),
                action: "open".into(),
                panel: "terminal".into(),
                terminal_ids: vec![id.clone()],
                cwd: Some(dir.display().to_string()),
                title: None,
                url: None,
            });
            Ok(ToolOut::text(format!(
                "Opened terminal {id} in {} as a sub-terminal on the left of this chat.",
                dir.display()
            )))
        })
    }
}

impl Tool for Browser {
    fn name(&self) -> &'static str {
        "browser"
    }
    fn schema(&self) -> Value {
        simple_schema(
            "browser",
            "Drive a page the user sees under this chat. snapshot lists text and numbered [n] refs; click/type/fill/hover/select/scroll take a ref; press a key (Enter, Tab, Escape, ArrowDown); wait for text or a ref (≤30 s); eval a JS expression; console shows the page's console and errors; screenshot shows the image to both of you.",
            &[
                (
                    "action",
                    "navigate|snapshot|screenshot|click|type|fill|press|hover|select|scroll|back|forward|wait|eval|console|close",
                    true,
                ),
                ("url", "navigate", false),
                ("ref", "[n] from snapshot", false),
                ("text", "type/fill: the text; wait: text to wait for", false),
                ("key", "press", false),
                ("value", "select: option text or value", false),
                ("direction", "scroll: down|up|top|bottom", false),
                ("amount", "scroll: pixels", false),
                ("ms", "wait: at most (5000)", false),
                ("expression", "eval: JavaScript", false),
            ],
        )
    }
    fn plan(&self, _cx: &PlanCx, _args: &Value) -> Result<Plan> {
        Ok(Plan::access(Access::write_resource(Resource::Browser)))
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        let hooks = Arc::clone(&self.0);
        Box::pin(async move {
            let action = req(&args, "action")?.to_ascii_lowercase();
            let owner = cx.agent.id.to_string();
            if action == "close" {
                let had = hooks.browsers.close(&owner);
                hooks.broadcast(board(&owner, "close", "browser", vec![BROWSER_PAGE.into()]));
                return Ok(ToolOut::text(if had {
                    "browser closed"
                } else {
                    "no browser page was open"
                }));
            }
            if hooks.browsers.touch(&owner) {
                hooks.broadcast(Frame::Board {
                    owner: owner.clone(),
                    action: "open".into(),
                    panel: "browser".into(),
                    terminal_ids: vec![BROWSER_PAGE.into()],
                    cwd: None,
                    title: None,
                    url: opt_str(&args, "url").map(str::to_string),
                });
            }
            let agent = cx.agent.id.clone();
            let blocking = Arc::clone(&hooks);
            let changes_page = matches!(
                action.as_str(),
                "navigate"
                    | "click"
                    | "type"
                    | "fill"
                    | "press"
                    | "select"
                    | "scroll"
                    | "back"
                    | "forward"
            );
            let out = tokio::task::spawn_blocking(move || blocking.browser(&agent, &action, &args))
                .await
                .map_err(|e| anyhow::anyhow!("browser task: {e}"))??;
            // Every action refreshes the desktop's browser row: the URL
            // always, and a picture — the one this action took, or, after a
            // navigate/click/type, a preview so the row shows the page
            // without the model having to ask for a screenshot.
            let preview = if out.png.is_none() && changes_page {
                let previews = Arc::clone(&hooks);
                tokio::task::spawn_blocking(move || previews.browsers.preview())
                    .await
                    .ok()
                    .flatten()
            } else {
                None
            };
            hooks.broadcast(Frame::Browser {
                agent: owner,
                page: BROWSER_PAGE.into(),
                url: hooks.browsers.url(cx.agent.id.as_str()),
                screenshot: out
                    .png
                    .as_deref()
                    .or(preview.as_deref())
                    .map(|png| base64::engine::general_purpose::STANDARD.encode(png)),
            });
            let Some(png) = out.png else {
                return Ok(ToolOut::text(out.text));
            };
            // Screenshots live under the agent folder so the transcript
            // cite survives and the UI can open them.
            let dir = Layout::new(&cx.place, cx.agent.id.as_str()).images();
            std::fs::create_dir_all(&dir)?;
            let file = dir.join(format!("shot-{}.png", arbos_core::now_ms()));
            std::fs::write(&file, &png)?;
            let shown = file.display().to_string();
            let dims = arbos_engine::image::dimensions(&png)
                .map(|(w, h)| format!(", {w}x{h}"))
                .unwrap_or_default();
            Ok(ToolOut {
                body: format!(
                    "screenshot {shown} (image/png{dims}, {} KB) — attached below",
                    png.len().div_ceil(1024)
                ),
                paths: vec![shown.clone()],
                child: None,
                images: vec![shown],
                diff: None,
                park: None,
            })
        })
    }
}

/// One tool of an MCP server, offered to the model as
/// `mcp__<server>__<tool>`. The call runs on the blocking pool: a stdio
/// server is spawned per request, an HTTP one is posted to.
pub struct McpTool {
    name: &'static str,
    remote: String,
    description: String,
    input_schema: Value,
    server: Arc<crate::mcp::Server>,
}

impl McpTool {
    /// Prefix every name so it can never shadow a builtin, and give the
    /// `Tool` trait the `'static` name it wants (one leak per tool, once).
    pub fn new(server: Arc<crate::mcp::Server>, spec: crate::mcp::ToolSpec) -> Self {
        let name: &'static str =
            Box::leak(crate::mcp::tool_name(&server.name, &spec.name).into_boxed_str());
        Self {
            name,
            remote: spec.name,
            description: spec.description,
            input_schema: spec.input_schema,
            server,
        }
    }
}

impl Tool for McpTool {
    fn name(&self) -> &'static str {
        self.name
    }
    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": format!("MCP tool `{}` on server {}: {}", self.remote, self.server.name, self.description),
                "parameters": self.input_schema,
            }
        })
    }
    fn plan(&self, _cx: &PlanCx, _args: &Value) -> Result<Plan> {
        // An MCP tool may do anything; run it alone and count it as a write
        // so a readonly agent cannot reach it.
        Ok(Plan::access(Access::exclusive()))
    }
    fn run(&self, _cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        let server = Arc::clone(&self.server);
        let remote = self.remote.clone();
        Box::pin(async move {
            let text = tokio::task::spawn_blocking(move || server.call(&remote, args))
                .await
                .map_err(|e| anyhow::anyhow!("mcp task: {e}"))??;
            Ok(ToolOut::text(text))
        })
    }
}
