//! The built-in tools and the one path a call takes to run.

use anyhow::Result;
use arbos_core::AgentId;
use serde_json::Value;
use std::sync::Arc;

mod apply_patch;
mod bash;
mod editdiff;
pub(crate) mod file_hooks;
pub(crate) mod fs;
pub mod git;
pub mod git_guard;
mod hashline;
pub mod memory;
mod web;

pub use bash::{is_readonly_command, kill_job};
pub use fs::resolve;

pub use crate::tool::ToolOut;
use crate::tool::{BoxFuture, Plan, PlanCx, Registry, RunCx, View};

#[derive(Debug, Clone)]
pub struct GrepHit {
    pub path: String,
    pub line: usize,
    pub text: String,
}

pub trait Grep: Send + Sync {
    fn search(&self, pattern: &str, glob: Option<&str>) -> Result<Vec<GrepHit>>;
    fn ready(&self) -> bool {
        true
    }
}

/// What the engine needs from whoever runs it. Agents, the user, and the
/// browser are tools the host registers; this is only what a built-in tool
/// cannot do alone.
pub trait Hooks: Send + Sync {
    /// Ask the user to allow a command. Resolves when they answer; the
    /// caller races it against the turn's cancel token.
    fn approve(
        &self,
        agent: &AgentId,
        tool: &str,
        command: &str,
    ) -> BoxFuture<'static, Result<bool>>;

    /// Live event for the window. Not written to transcript.jsonl.
    fn emit(&self, _event: &arbos_core::Event) {}
    /// The model has been silent for `secs` seconds and the call is still
    /// open. Live only; never on the transcript.
    fn working(&self, _secs: u64) {}
    /// What the turn's first model call carries, in estimated tokens: the
    /// system prompt (contract, context, instance, plan), the tool schemas,
    /// and the conversation. Once per turn; for the kernel log.
    fn prompt_size(&self, _size: PromptSize) {}
    /// The model set its status in prose — a one-line reply `status: …`
    /// instead of the `status` tool. `step` is the words after the colon,
    /// clipped; the kernel shows them as the live line.
    fn spoke_status(&self, _step: &str) {}

    /// A file in another node's store, by address (`arbos://…`). The host
    /// reaches it through the hub; a host with no hub says so. Failure is
    /// an error, never empty text: an agent must not take a missing peer
    /// for an empty file.
    fn store_read(&self, address: &str) -> BoxFuture<'static, Result<StoreFile>> {
        let a = address.to_string();
        Box::pin(async move { anyhow::bail!("{a}: {NO_MESH}") })
    }

    /// The entries of a folder in another node's store, by address.
    fn store_list(
        &self,
        address: &str,
    ) -> BoxFuture<'static, Result<Vec<arbos_core::wire::Entry>>> {
        let a = address.to_string();
        Box::pin(async move { anyhow::bail!("{a}: {NO_MESH}") })
    }

    /// Write a file in another node's store, by address. `base_hash` is
    /// the hash of what was read (compare-and-swap); the receiving kernel
    /// applies its own store rules and answers with the new hash.
    fn store_write(
        &self,
        address: &str,
        _text: String,
        _base_hash: Option<String>,
    ) -> BoxFuture<'static, Result<StoreWritten>> {
        let a = address.to_string();
        Box::pin(async move { anyhow::bail!("{a}: {NO_MESH}") })
    }
}

/// Why a store address cannot be used on a host with no hub.
pub const NO_MESH: &str = "a store address (arbos://<machine>/<project>/<path>) needs a kernel registered on a hub, and this one is on none; use the local path";

/// A file read from another node's store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreFile {
    pub text: String,
    /// The whole file's size; `text` is shorter when `truncated`.
    pub size: u64,
    pub truncated: bool,
    /// sha-256 of `text`, the `base_hash` a following write carries.
    pub hash: String,
}

/// What the store said after a write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreWritten {
    pub size: u64,
    pub hash: String,
}

/// Token estimates (chars/4, calibrated against the provider's count once
/// it has reported) of the parts of one model call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptSize {
    /// The model this turn's calls go to, after every override.
    pub model: String,
    pub system: u64,
    pub tools: u64,
    pub conversation: u64,
}

impl PromptSize {
    pub fn total(&self) -> u64 {
        self.system + self.tools + self.conversation
    }
}

/// Every tool the engine ships. Hosts add theirs with [`Registry::with`].
pub fn builtin() -> Registry {
    Registry::new()
        .with(fs::Ls)
        .with(fs::Read)
        .with(fs::Find)
        .with(fs::GrepTool)
        .with(fs::Write)
        .with(fs::Edit)
        .with(fs::Delete)
        .with(apply_patch::ApplyPatch)
        .with(bash::Bash)
        .with(bash::Await)
        .with(bash::Jobs)
        .with(web::Fetch)
        .with(web::Search)
        .with(git::Changes)
        .with(git::Undo)
        .with(memory::Remember)
}

/// A call after preflight: allowlist checked, file hooks applied, planned.
pub struct Prepared {
    pub tool: Arc<dyn crate::tool::Tool>,
    pub args: Value,
    pub plan: Plan,
    /// A before-tool hook wants the user asked first; the question.
    pub ask: Option<String>,
    /// Text before-tool hooks add to the result the model reads.
    pub context: Vec<String>,
    /// Hook trouble worth a notice (a failed hook, bad `hooks.toml`).
    pub notices: Vec<String>,
}

/// Allowlist → `before-tool` hook (may rewrite) → allowlist again → `plan`.
/// Runs the hook scripts on the blocking pool; they are user processes.
/// Marker the tool result uses to say the path was a guess.
pub const INFERRED_PATH: &str = "__arbos_inferred_path";

/// The file the agent last read or edited in the current turn, relative to
/// `cwd` when it is under it. None when the turn has not touched a file.
fn last_touched_path(
    place: &arbos_core::Place,
    agent: &arbos_core::Agent,
    cwd: &std::path::Path,
) -> Option<String> {
    let layout = arbos_core::files::Layout::new(place, agent.id.as_str());
    let events = arbos_core::load_transcript(&layout.transcript()).ok()?;
    for ev in events.iter().rev() {
        match &ev.kind {
            arbos_core::EventKind::TurnComplete { .. }
            | arbos_core::EventKind::Interrupted { .. } => return None,
            arbos_core::EventKind::Tool(rec)
                if matches!(rec.name.as_str(), "edit" | "write" | "read")
                    && rec.error.is_none() =>
            {
                if let Some(p) = rec.paths.first() {
                    let path = std::path::Path::new(p);
                    if path.is_dir() {
                        continue;
                    }
                    return Some(
                        path.strip_prefix(cwd)
                            .map(|r| r.display().to_string())
                            .unwrap_or_else(|_| p.clone()),
                    );
                }
            }
            _ => {}
        }
    }
    None
}

/// The protected entry a call would write, if any: from the plan's write
/// paths (file tools), or from the command text for `bash`/`terminal`.
fn protected_target(
    root: &std::path::Path,
    tool: &str,
    plan: &Plan,
    args: &Value,
) -> Option<String> {
    for r in &plan.access.writes {
        if let crate::access::Resource::Path(p) = r
            && let Some(entry) = arbos_core::store::protected_by(root, p)
        {
            return Some(entry.to_string());
        }
    }
    if matches!(tool, "bash" | "terminal")
        && let Some(cmd) = crate::tool::opt_str(args, "command")
    {
        return arbos_core::store::bash_writes_protected(cmd).map(str::to_string);
    }
    None
}

pub async fn preflight(view: &View, cx: &RunCx, name: &str, args: &Value) -> Result<Prepared> {
    let Some(_) = view.get(name) else {
        anyhow::bail!("tool {name} not on allowlist");
    };
    if let Some(why) = args.get(crate::provider::BAD_ARGS).and_then(Value::as_str) {
        let shown: String = why.chars().take(600).collect();
        anyhow::bail!(
            "{name} was not run: {shown}. Send the call again with a complete JSON object."
        );
    }
    let decided = {
        let place = cx.place.clone();
        let agent = cx.agent.clone();
        let name = name.to_string();
        let args = args.clone();
        tokio::task::spawn_blocking(move || file_hooks::before_tool(&place, &agent, &name, &args))
            .await
            .map_err(|e| anyhow::anyhow!("hook task: {e}"))??
    };
    let Some(tool) = view.get(&decided.tool) else {
        anyhow::bail!("tool {} not on allowlist", decided.tool);
    };
    let mut decided = decided;
    // Small models drop `path` on the second edit to the same file. The
    // file they touched last in this turn is what they mean; the result
    // says so, so a wrong guess is visible and cheap to correct.
    if matches!(decided.tool.as_str(), "edit" | "write")
        && crate::tool::opt_str(&decided.args, "path").is_none()
        && decided.args.is_object()
    {
        let place = cx.place.clone();
        let agent = cx.agent.clone();
        let cwd = cx.cwd.clone();
        if let Ok(Some(p)) =
            tokio::task::spawn_blocking(move || last_touched_path(&place, &agent, &cwd)).await
        {
            decided.args[INFERRED_PATH] = serde_json::json!(true);
            decided.args["path"] = serde_json::json!(p);
        }
    }
    let mut plan = tool.plan(
        &PlanCx {
            root: cx.root(),
            cwd: &cx.cwd,
            agent: &cx.agent,
        },
        &decided.args,
    )?;
    // Readonly is a property of the footprint, not of the tool name. This
    // catches a hook that rewrites a read into a write, and a bash command
    // that is not on the read-only list.
    let writes = !plan.access.is_readonly();
    if writes && cx.agent.mode == arbos_core::Mode::Plan && decided.tool != "ask" {
        anyhow::bail!(
            "plan mode: {} would write. Put the change in your plan and your reply, and ask the user to switch you to ask or auto to carry it out.",
            decided.tool
        );
    }
    if cx.agent.readonly && writes {
        anyhow::bail!("readonly agent: {} would write", decided.tool);
    }
    // Ask mode: a write waits for the user. Interactive, so writes are
    // asked one at a time and never race a read of the same file.
    // `ask` is exclusive because it waits on the user, not because it
    // writes; asking permission to ask would be absurd. A before-tool
    // hook's own question wins when it set one.
    let asking = cx.agent.mode == arbos_core::Mode::Ask;
    // An agent's own bookkeeping — its checklist, its todo, its status
    // line, its memory — is never something to ask the user about, in
    // any mode (a coordinator in ask mode put "allow plan: …" to the user
    // as a card; Cursor never does).
    let bookkeeping = matches!(
        decided.tool.as_str(),
        "ask" | "plan" | "todo" | "status" | "remember"
    );
    let ask_first = writes && asking && !bookkeeping;
    // The default mode asks nothing: "go go go" (Jacob, 2026-09-15; a
    // read-only question on his Mac drew an allow-bash card). A write
    // into a file that shapes how agents behave (T3-10) and a command
    // that reaches past this machine (T3-06) ask only in ask mode; auto
    // keeps the hard refusals — the git guard, containment's fetch of a
    // metadata URL — and nothing that waits on a card. `remember` owns
    // memory.md and is never asked.
    let protected = (asking && writes && decided.tool != "remember")
        .then(|| protected_target(cx.root(), &decided.tool, &plan, &decided.args))
        .flatten();
    let reach = (asking && matches!(decided.tool.as_str(), "bash" | "terminal"))
        .then(|| crate::tool::opt_str(&decided.args, "command"))
        .flatten()
        .and_then(arbos_core::containment::risk_of)
        .filter(|risk| {
            !(risk.starts_with("the cloud metadata") && crate::sandbox::metadata_allowed(&cx.place))
        });
    let ask = decided
        .ask
        .or_else(|| {
            protected
                .as_deref()
                .map(|entry| arbos_core::store::protected_question(&decided.tool, entry))
        })
        .or_else(|| reach.map(|risk| arbos_core::containment::question(&decided.tool, risk)))
        .or_else(|| ask_first.then(|| crate::batch::summarise_call(&decided.tool, &decided.args)));
    let plan = if ask.is_some() {
        plan.interactive()
    } else {
        plan
    };
    Ok(Prepared {
        tool: Arc::clone(tool),
        args: decided.args,
        plan,
        ask,
        context: decided.context,
        notices: decided.notices,
    })
}
