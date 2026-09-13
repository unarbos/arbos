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
pub mod node;
mod page;
mod place;
pub mod prs;
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
pub use node::{Attempt, Do, Node, NodeId, Status as NodeStatus, Verdict, When};
pub use page::{Page, PageKind};
pub use place::Place;
pub use prs::{PrRec, load_prs, record_pr};
pub use wake::{Wake, WakeKind};

/// Unix millis.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
