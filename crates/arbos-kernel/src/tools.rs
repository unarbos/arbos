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

use crate::hooks::{Isolate, KernelHooks, NewNode, SayMode};
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
                    "A machine name from ~/.config/arbos/machines.toml (see Machines in your prompt): the child runs there, in its own synced copy of this project, and reports back here.",
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
                ("to", "Agent id, agent name, or 'user'.", true),
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
                "description": "Maintain your durable plan: the tree of goals you hold. op:add appends nodes under a parent (0 = new roots), each a when × do pair; op:update moves one node (active, done, failed, blocked, cancelled, pending — or no status on a recurring node to record one recurrence); op:show renders it. Survives restarts and compaction; trust it over conversation memory.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "op": {"type": "string", "enum": ["add", "update", "show"]},
                        "parent": {"type": "integer", "description": "add: parent node id. 0 starts a new plan: the first node becomes the mission root and the rest its children, in order."},
                        "nodes": {
                            "type": "array",
                            "description": "add: nodes to append, in execution order.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "goal": {"type": "string", "description": "What done looks like, stated so it can be checked. For an ask, the question."},
                                    "check": {"type": "string", "description": "How to verify it (a command, a test, a criterion)."},
                                    "when": {
                                        "type": "object",
                                        "description": "When it becomes runnable. Omit for ready (after earlier siblings).",
                                        "properties": {
                                            "after": {"type": "string", "description": "Defer by a duration from now, e.g. \"30m\". One-shot."},
                                            "every": {"type": "string", "description": "Recur on this period, e.g. \"1h\". Never terminates; cancel it when done. Min 30s."},
                                            "wake": {"type": "boolean", "description": "Fire a turn of you the moment this node is ready (earlier siblings finished). The callback. Agent nodes only."},
                                            "condition": {"type": "string", "description": "A shell predicate (exit 0 = holds) checked every poll period; do fires only when it holds. Needs every. Agent or notify do."}
                                        }
                                    },
                                    "do": {
                                        "type": "object",
                                        "description": "What discharges it. Omit for a turn of you.",
                                        "properties": {
                                            "shell": {"type": "string", "description": "A command the kernel runs as a job — no model turn. Exit 0 = done; otherwise you are woken with the log tail. Its output alone goes nowhere: add notify to deliver it."},
                                            "notify": {"type": "string", "description": "A message the kernel delivers to whoever asked (the user, or the agent that spawned you) — no model turn. With shell: sent after each successful run, with {output} replaced by the command's output (e.g. shell:\"curl -s …/spot | jq -r .data.amount\", notify:\"BTC: ${output}\"). This is how a scheduled reading reaches the user with no model turn."},
                                            "ask": {"type": "boolean", "description": "A question only the user can answer; parks until they do."}
                                        }
                                    },
                                    "par": {"type": "boolean", "description": "Run beside the previous node instead of after it."}
                                },
                                "required": ["goal"]
                            }
                        },
                        "node": {"type": "integer", "description": "update: target node id."},
                        "status": {"type": "string", "description": "update: active, done, failed, blocked, cancelled, or pending. Omit on a recurring node to record a recurrence."},
                        "outcome": {"type": "string", "description": "update: what happened or was learned — required for done, failed, blocked."}
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
            let op = req(&args, "op")?;
            let ack = match op {
                "add" => {
                    let parent = args.get("parent").and_then(|v| v.as_u64()).unwrap_or(0);
                    let specs: Vec<NewNode> = args
                        .get("nodes")
                        .and_then(|v| v.as_array())
                        .map(|xs| {
                            xs.iter()
                                .map(NewNode::from_json)
                                .collect::<Result<Vec<_>>>()
                        })
                        .transpose()?
                        .unwrap_or_default();
                    let ids = hooks.plan_add(agent, parent, &specs, "")?;
                    format!(
                        "Added {}.",
                        ids.iter()
                            .map(|i| format!("#{i}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
                "update" => {
                    let node = args
                        .get("node")
                        .and_then(|v| v.as_u64())
                        .ok_or_else(|| anyhow::anyhow!("plan update: node is required"))?;
                    let status =
                        match opt_str(&args, "status") {
                            None => None,
                            Some(s) => Some(arbos_core::NodeStatus::parse(s).ok_or_else(|| {
                                anyhow::anyhow!("plan update: unknown status {s:?}")
                            })?),
                        };
                    let outcome = opt_str(&args, "outcome").unwrap_or("");
                    hooks.plan_update(agent, node, status, outcome, "self")?
                }
                "show" => String::new(),
                other => anyhow::bail!("plan: unknown op {other:?} (use add, update, or show)"),
            };
            let forest = hooks.plan_render(agent);
            Ok(ToolOut::text(if ack.is_empty() {
                forest
            } else {
                format!("{ack}\n\n{forest}")
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
            let rx = hooks.ask(&cx.agent.id, q, &options, &cx.call_id)?;
            let answer = tokio::select! {
                r = rx => r.ok(),
                _ = cx.cancel.cancelled() => anyhow::bail!("interrupted while waiting for the user"),
            };
            // An empty answer is the Skip button: say so, so the model
            // carries on with a default instead of reporting an empty reply.
            let answer = answer.map(|a| {
                if a.trim().is_empty() {
                    "The user skipped this question without answering. Choose a sensible default yourself, say which you chose, and continue.".to_string()
                } else {
                    a
                }
            });
            Ok(ToolOut::text(
                answer.unwrap_or_else(|| "(waiting for user)".into()),
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
