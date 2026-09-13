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
            "Start a child agent with a brief. The child owns its own plan and schedule: do not add plan nodes of your own that mirror its job. Its reports arrive here as messages from it. isolate=worktree gives it a git worktree of this repository (.arbos/worktrees/<id>, branch arbos/<id>, cut from HEAD) so it can edit, build, and commit without touching your checkout — use it for any child that changes code while you or another child also do. kind picks a custom agent definition (see Kinds in your prompt): its model, tools, and standing instructions apply to the child.",
            &[
                ("brief", "What the child should do.", true, "string"),
                (
                    "kind",
                    "Name of an agent definition from .arbos/agents-defs/ (Kinds in your prompt).",
                    false,
                    "string",
                ),
                (
                    "model",
                    "inherit or a model id. Beats the kind's model.",
                    false,
                    "string",
                ),
                ("readonly", "If true, no writes.", false, "boolean"),
                ("cwd", "Child cwd. Overrides isolate.", false, "string"),
                ("isolate", "none (default) or worktree.", false, "string"),
                (
                    "host",
                    "A machine name from ~/.config/arbos/machines.toml (ssh; the child runs in its own synced copy of this project) or from the hub roster in .arbos/machines/ (a machine with a worker; the child runs in a worktree of that machine's own checkout of this project). See Machines in your prompt. It reports back here.",
                    false,
                    "string",
                ),
                (
                    "wait",
                    "true: block until the child's first report (its say to you, or its first turn's last words) and return it as this result. Use it for a quick sub-task whose answer you need before going on; leave it off for parallel workers.",
                    false,
                    "boolean",
                ),
                (
                    "wait_secs",
                    "With wait: how long to wait before returning \"still working\" (default 600). The child keeps going; its report then arrives as a message.",
                    false,
                    "integer",
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
            let brief = req(&args, "brief")?;
            let model = opt_str(&args, "model");
            let readonly = opt_bool(&args, "readonly").unwrap_or(false);
            let cwd = opt_str(&args, "cwd").map(PathBuf::from);
            let wait = opt_bool(&args, "wait").unwrap_or(false);
            let wait_secs = args
                .get("wait_secs")
                .and_then(Value::as_u64)
                .unwrap_or(600)
                .clamp(1, 6 * 3600);
            if let Some(host) = opt_str(&args, "host") {
                let (id, where_) = crate::remote::spawn_remote(
                    Arc::clone(&hooks),
                    cx.agent.clone(),
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
            let kind = opt_str(&args, "kind");
            let kind_owned = kind.map(str::to_string);
            let raw = opt_str(&args, "isolate").unwrap_or("none");
            let isolate = Isolate::parse(raw).ok_or_else(|| {
                anyhow::anyhow!("spawn: isolate must be none or worktree, not {raw:?}")
            })?;
            // git runs on the blocking pool: a large checkout takes seconds.
            let agent = cx.agent.clone();
            let brief_owned = brief.to_string();
            let model_owned = model.map(str::to_string);
            let spawner = Arc::clone(&hooks);
            let (id, worktree) = tokio::task::spawn_blocking(move || {
                spawner.spawn_isolated(
                    &agent,
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
            let mut body = match kind {
                Some(k) => format!("spawned {id} (kind {k}): {brief}"),
                None => format!("spawned {id}: {brief}"),
            };
            let mut paths = vec![format!(".arbos/agents/{id}")];
            if let Some(w) = &worktree {
                body.push_str(&format!(
                    "\nIt works in its own worktree {} on branch {} (cut from {}). Your checkout is untouched. When its branch is merged or abandoned, remove it: {}",
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
            "Send a message to someone outside this conversation: another agent (by id or name — see <<peers>>) or the user. To an agent it lands in their transcript as a message from you. mode note (default) waits for their next turn; mode request queues a turn for them and their reply arrives here as a message; mode steer reaches an agent that is running now — your words land in its current turn at its next tool step, so it changes course without restarting (if it is idle, a turn starts). Use steer to add a constraint, redirect, or stop a worker mid-task. To 'user' it is a durable notice in this chat. Then end your turn; never read another agent's transcript to see whether they answered.",
            &[
                (
                    "to",
                    "Agent id, agent name, 'user', or an agent on another machine of the hub as <machine>/<agent> (or <machine>/<project>/<agent>; see Hub machines in your prompt).",
                    true,
                ),
                (
                    "text",
                    "The message. Short and self-contained: the recipient sees only this.",
                    true,
                ),
                ("mode", "note (default), request, or steer.", false),
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
                None => anyhow::bail!("say: mode must be note, request, or steer, not {raw:?}"),
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
                "description": "Your checklist, kept in your notes.md (root: the project page .arbos/notes.md). One call writes it: {\"op\":\"set\",\"items\":[{\"section\":\"Profile\",\"text\":\"[time the sort](bench.py) — not started\"},\"count comparisons\"]}. items may also be a markdown checklist string (\"## Phase\\n- [ ] item\") or nested goals ({\"goal\":\"…\",\"children\":[…]}: a parent with children becomes a ## section). op:add appends one item (text); op:check marks item n done with a readout (done:false reopens); op:update rewrites item n's text; op:remove drops item n; op:show prints it. Items read `[label](target) — status readout`, rewritten fresh on every touch. It schedules nothing: anything timed or event-driven is a subscription (subscribe). Survives restarts and compaction; trust it over conversation memory.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "op": {"type": "string", "enum": ["set", "add", "check", "update", "remove", "show"]},
                        "items": {
                            "type": "array",
                            "description": "set: the whole list, in order. A string, or {section, text} to start a ## section.",
                            "items": {"anyOf": [{"type": "string"}, {"type": "object", "properties": {"section": {"type": "string"}, "text": {"type": "string"}}, "required": ["text"]}]}
                        },
                        "section": {"type": "string", "description": "add: the ## section to append under (created when new)."},
                        "text": {"type": "string", "description": "add/update: the item text, `[label](target) — readout`."},
                        "n": {"type": "integer", "description": "check/update/remove: the item number from show."},
                        "done": {"type": "boolean", "description": "check: false to reopen (default true)."},
                        "readout": {"type": "string", "description": "check: the fresh status readout written after the dash."}
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
                    let item = notes.check(k, done, opt_str(&args, "readout"))?;
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
                "description": "The only clock. op:add creates a standing request to be woken: kind timer (every or after, with prompt), shell (cmd on every; wakes you only on failure — with deliver_to user and notify \"…{output}\" the output goes to the user after each run, no model turn), github_pr / github_ci (repo, pr: woken with a [github] message when the PR or its checks change), inbox (path, every: woken when new files land in a folder). op:list shows yours; op:remove id ends one; op:pause / op:resume id. A firing arrives as a message from subscription:N. Never sleep or poll in bash instead.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "op": {"type": "string", "enum": ["add", "list", "remove", "pause", "resume"]},
                        "id": {"type": "integer", "description": "remove/pause/resume: the subscription id from list."},
                        "kind": {"type": "string", "enum": ["timer", "shell", "github_pr", "github_ci", "inbox"]},
                        "prompt": {"type": "string", "description": "What you are told when it fires (timer, shell, inbox); the note appended to a github message."},
                        "every": {"type": "string", "description": "Period, e.g. \"1h\", \"10m\" (min 30s). Recurring."},
                        "after": {"type": "string", "description": "One-shot: fire once this long from now, e.g. \"30m\"."},
                        "at": {"type": "string", "description": "Wall clock (UTC) the due moment aligns to: \"09:00\" daily, \":15\" each hour."},
                        "cmd": {"type": "string", "description": "shell: the command the kernel runs as a job."},
                        "deliver_to": {"type": "string", "enum": ["agent", "user", "none"], "description": "shell: user sends the output to the user with no model turn; none is a quiet chore (nothing on success). Failures always wake you. Default agent."},
                        "notify": {"type": "string", "description": "shell + deliver_to user: the line sent, must contain {output}."},
                        "repo": {"type": "string", "description": "github_*: owner/name."},
                        "pr": {"type": "integer", "description": "github_*: pull request number."},
                        "path": {"type": "string", "description": "inbox: the folder to watch (relative to the place or absolute)."},
                        "expires": {"type": "string", "description": "RFC 3339 instant after which it is removed."}
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
                        deliver_to: opt_str(&args, "deliver_to").unwrap_or("agent").to_string(),
                        notify: opt_str(&args, "notify").map(str::to_string),
                        expires: opt_str(&args, "expires").map(str::to_string),
                        paused: false,
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
            let subs = arbos_core::subscription::list(&hooks.place, agent);
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
                ("question", "Question.", true, "string"),
                (
                    "options",
                    "Optional choices the user can pick from.",
                    false,
                    "array",
                ),
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
            "Open an interactive terminal attached to this chat. The user sees it as a sub-terminal on the left of the chat. action 'open' starts a new shell (optional cwd) and returns its id. Use this when the user asks for a terminal in Arbos — not bash 'open -a Terminal' and not another editor's terminal.",
            &[
                ("action", "open.", true),
                (
                    "cwd",
                    "Directory the new shell starts in (default: working directory).",
                    false,
                ),
            ],
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
            "Navigate, click, type, or screenshot a page. The user sees the page as a browser row under this chat; close removes it. Use screenshot whenever the user asks to see a page or a result: the image is shown to the user and to you.",
            &[
                (
                    "action",
                    "navigate|click|type|screenshot|snapshot|close",
                    true,
                ),
                ("url", "For navigate.", false),
                ("ref", "Element ref.", false),
                ("text", "Text to type.", false),
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
            let changes_page = matches!(action.as_str(), "navigate" | "click" | "type");
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
