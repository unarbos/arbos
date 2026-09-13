//! File kernel types and the place store.
//!
//! Five types: [`Place`], [`Agent`], [`Page`], [`Event`], [`Node`].
//! A [`Wake`] is derived from a node whose moment came.
//! The tree is the directory. The log is JSONL.

mod agent;
pub mod chattitle;
mod event;
pub mod files;
mod lock;
pub mod machines;
pub mod node;
mod page;
mod place;
mod wake;
pub mod wire;

pub use agent::validate_id;
pub use agent::{ALL_TOOLS, Agent, AgentId};
pub use event::{Event, EventKind, ToolRec, Usage};
pub use files::{
    Layout, ROOT_ID, append_event, append_events, bootstrap, create_chat, list_agents, load_agent,
    load_transcript, needs_serve, read_focus, write_focus,
};
pub use lock::PlaceLock;
pub use machines::{Machine, Machines};
pub use node::{Attempt, Do, Node, NodeId, Status as NodeStatus, Verdict, When};
pub use page::{Page, PageKind};
pub use place::Place;
pub use wake::{Wake, WakeKind};

/// `~/.config/arbos` (or `$XDG_CONFIG_HOME/arbos`): the host's own files.
pub fn host_dir() -> std::path::PathBuf {
    if let Some(base) = std::env::var_os("XDG_CONFIG_HOME") {
        return std::path::PathBuf::from(base).join("arbos");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return std::path::PathBuf::from(home).join(".config").join("arbos");
    }
    std::path::PathBuf::from(".arbos-host")
}

/// Unix millis.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
