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
    let plan = tool.plan(
        &PlanCx {
            root: cx.place.path(),
            cwd: &cx.cwd,
            agent: &cx.agent,
        },
        &decided.args,
    )?;
    // Readonly is a property of the footprint, not of the tool name. This
    // catches a hook that rewrites a read into a write, and a bash command
    // that is not on the read-only list.
    if cx.agent.readonly && !plan.access.is_readonly() {
        anyhow::bail!("readonly agent: {} would write", decided.tool);
    }
    Ok(Prepared {
        tool: Arc::clone(tool),
        args: decided.args,
        plan,
    })
}
