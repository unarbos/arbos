use crate::{Event, Usage};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum Frame {
    Snapshot {
        tree: Vec<TreeNode>,
        focus: String,
        budget: Option<Usage>,
    },
    Event {
        agent: String,
        event: Event,
    },
    Tree {
        tree: Vec<TreeNode>,
    },
    Turn {
        agent: String,
        state: String,
        budget: Option<Usage>,
    },
    Ask {
        agent: String,
        question: String,
        options: Vec<String>,
    },
    Pty {
        agent: String,
        page: String,
        data: String,
    },
    PtyIn {
        agent: String,
        page: String,
        data: String,
    },
    Browser {
        agent: String,
        page: String,
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        screenshot: Option<String>,
    },
    User {
        agent: String,
        text: String,
        #[serde(default)]
        steer: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<String>,
    },
    Pause {
        agent: String,
        paused: bool,
    },
    Focus {
        path: String,
    },
    Stop {
        agent: String,
    },
    /// Summarise the oldest turns now, before the next model call.
    Compact {
        agent: String,
    },
    Answer {
        agent: String,
        text: String,
    },
    Approve {
        agent: String,
        call_id: String,
        allow: bool,
    },
    Undo {
        agent: String,
    },
    SetModel {
        agent: String,
        model: String,
    },
    /// auto | ask | plan — how much the agent may do without asking.
    SetMode {
        agent: String,
        mode: String,
    },
    VoiceStart,
    VoiceStop,
    Refresh,
    /// The kernel refused or failed something a client sent. `agent` when
    /// the frame named one. Clients that do not know it may ignore it.
    Error {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
        detail: String,
    },
    /// One agent's plan, whole, whenever it changes.
    Plan {
        agent: String,
        nodes: Vec<PlanNode>,
    },
    /// Move a node: `cancel`, `run` (fire now), `reopen`, `answer`.
    PlanOp {
        agent: String,
        node: u64,
        op: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        text: String,
    },
    /// New output from a detached job, as it arrives. `delta` is what was
    /// appended to the journal since the last frame (capped; a skip marker
    /// says when bytes were dropped). The last frame has `running: false`
    /// and the exit code, `None` when the process was killed.
    Job {
        agent: String,
        id: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        delta: String,
        running: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit: Option<i32>,
    },
    /// Open or close a desktop panel. The Mac app docks a terminal as a
    /// child of `owner` on the left of the chat.
    Board {
        owner: String,
        action: String,
        panel: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        terminal_ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        /// What the row is called: a process's command, a page's title.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// Where the panel points: a browser's URL, a process's log path.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
    },
}

/// A plan node as the window draws it. Strings are pre-rendered so the
/// window needs no clock or duration code of its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanNode {
    pub id: u64,
    pub parent: u64,
    pub goal: String,
    /// `pending`, `active`, `blocked`, `done`, `cancelled`, `failed`.
    pub status: String,
    /// `ready`, `due`, `fires 15:04`, `every 1h · next 15:04`, `waits`, or empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub when: String,
    /// `agent`, `shell`, `notify`, `ask`.
    pub do_kind: String,
    /// Last attempt's outcome, one line.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub origin: String,
    /// A recurring node: a standing obligation.
    #[serde(default)]
    pub standing: bool,
    /// The user's own message, already run. Drawn nowhere.
    #[serde(default)]
    pub inbox: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeNode {
    pub id: String,
    pub name: String,
    pub parent: Option<String>,
    pub paused: bool,
    pub model: String,
    pub kind: String,
    #[serde(default)]
    pub mode: String,
    /// Pull requests this agent and its descendants opened.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub prs: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}
