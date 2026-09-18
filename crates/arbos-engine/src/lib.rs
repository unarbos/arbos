//! One function: [`turn`]. Working set, fold, compact, tool dispatch.

mod access;
pub mod apology;
mod batch;
pub mod blocked;
pub mod compact;
mod control;
pub mod describe;
pub mod envprobe;
mod evict;
mod host;
pub mod image;
pub mod inflight;
pub mod intent;
mod jev;
mod jobs;
pub mod markup;
pub mod mechanism;
pub mod pdf;
pub mod project;
mod prompt;
mod provider;
pub mod repeat;
pub mod replay;
pub mod repro;
mod retry;
pub mod sandbox;
pub mod secrets;
mod step;
pub mod summarise;
pub mod title;
mod tool;
mod tools;
mod turn;

pub use access::{Access, Resource};
pub use control::TurnControl;
pub use host::{Host, HostConfig, KeySource, ProviderKind};
pub use jobs::{
    JOB_TTL, JOURNAL_WINDOW, Job, JobsRoot, LEASH_POINTERS, Meta as JobMeta, PidIdentity, Reaped,
    Status as JobStatus, kill_reason, parent_pid, repoint_leash, set_kill_reason,
    sweep_leash_pointers,
};
pub use provider::{
    ChatMessage, Interrupted, Provider, ProviderError, check_key, context_window, list_model_ids,
    warm,
};
pub use tool::{
    BoxFuture, Param, Plan, PlanCx, Registry, RunCx, Tool, ToolOut, opt_bool, opt_strings,
    simple_schema, typed_schema,
};
pub use tools::git;
pub use tools::git_guard::GitRules;
pub use tools::{
    Claim, Grep, GrepHit, Hooks, NO_MESH, PromptSize, StoreFile, StoreWritten, is_readonly_command,
    kill_job, reap_by_pid,
};
pub use turn::{TurnOpts, brief_output_paths, turn, written_this_turn};

/// What an agent's prompt costs before any conversation: the system
/// prefix (contract, project, page, peers) and the tool schemas, in
/// estimated tokens. A configured `window_tokens` smaller than this
/// leaves every turn over budget with nothing to compact; the kernel
/// checks at start so a stale pin is loud once, not thirteen times a
/// turn (JB-4, 2026-09-16).
pub fn standing_tokens(
    place: &arbos_core::Place,
    agent: &arbos_core::Agent,
    registry: &Registry,
) -> StandingTokens {
    let skills = prompt::skill_names(place);
    let proj = project::project(place, agent, &[], &skills, project::STEP_BYTES);
    let view = registry.view(agent);
    let tools = evict::estimate_tokens(&serde_json::to_string(view.schemas()).unwrap_or_default());
    StandingTokens {
        system: proj.base_tokens,
        tools,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct StandingTokens {
    pub system: u64,
    pub tools: u64,
}

impl StandingTokens {
    pub fn total(&self) -> u64 {
        self.system + self.tools
    }
    /// The smallest window this prompt can work in: the standing part plus
    /// room for a step's reply and a few turns of conversation.
    pub fn needed_window(&self) -> u64 {
        self.total() * 2
    }
}
