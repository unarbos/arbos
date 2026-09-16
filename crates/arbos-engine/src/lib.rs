//! One function: [`turn`]. Working set, fold, compact, tool dispatch.

mod access;
mod batch;
pub mod blocked;
pub mod compact;
mod control;
pub mod describe;
pub mod envprobe;
mod evict;
mod host;
pub mod image;
pub mod intent;
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
mod tool;
mod tools;
mod turn;

pub use access::{Access, Resource};
pub use control::TurnControl;
pub use host::{Host, HostConfig, KeySource, ProviderKind};
pub use jobs::{
    JOB_TTL, JOURNAL_WINDOW, Job, JobsRoot, Meta as JobMeta, PidIdentity, Reaped,
    Status as JobStatus,
};
pub use provider::{
    ChatMessage, Interrupted, Provider, ProviderError, check_key, list_model_ids, warm,
};
pub use tool::{
    BoxFuture, Param, Plan, PlanCx, Registry, RunCx, Tool, ToolOut, opt_bool, opt_strings,
    simple_schema, typed_schema,
};
pub use tools::git;
pub use tools::{
    Grep, GrepHit, Hooks, NO_MESH, PromptSize, StoreFile, StoreWritten, is_readonly_command,
    kill_job,
};
pub use turn::{TurnOpts, brief_output_paths, turn};
