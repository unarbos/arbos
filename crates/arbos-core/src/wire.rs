use crate::{Event, Usage};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum Frame {
    /// First frame on every connection: what the kernel speaks and how
    /// much of the focused agent's transcript it replays right after the
    /// snapshot. `protocol` 1 = hello + replay + history + deltas.
    Hello {
        protocol: u32,
        kernel: String,
        tail: u32,
        focus: String,
    },
    /// Client → kernel: replay `agent`'s transcript lines with `seq` >
    /// `since`, oldest first, at most `limit` (capped). `history_end`
    /// follows the last one.
    History {
        agent: String,
        #[serde(default)]
        since: u64,
        #[serde(default)]
        limit: u32,
    },
    /// Kernel → client: one transcript line replayed on attach or for a
    /// `history` request. Its own frame so a client that already holds the
    /// transcript (the desktop reads the files) can ignore replays while
    /// still taking live `event`s.
    Replayed {
        agent: String,
        event: Event,
    },
    /// Kernel → client: the replay is complete. `from`/`to` are the first
    /// and last `seq` sent (equal to `since` when nothing was), `total` the
    /// transcript's length now.
    HistoryEnd {
        agent: String,
        from: u64,
        to: u64,
        total: u64,
    },
    /// Live text as the model streams it, one frame per chunk. The whole
    /// step arrives later as an `event` with a `seq` (the transcript line).
    AssistantDelta {
        agent: String,
        text: String,
    },
    ThinkingDelta {
        agent: String,
        text: String,
    },
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
    VoiceStart,
    VoiceStop,
    Refresh,
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
}
