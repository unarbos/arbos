use crate::{Event, Usage};
use serde::{Deserialize, Serialize};

/// The most one `put` may carry, decoded (the desktop's attachment cap).
pub const PUT_MAX_BYTES: usize = 20 * 1024 * 1024;
/// Where a client's uploaded files land under `.arbos/`.
pub const ATTACHMENTS_DIR: &str = "attachments";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum Frame {
    /// First frame on every connection: what the kernel speaks and how
    /// much of the focused agent's transcript it replays right after the
    /// snapshot. `protocol` 1 = hello + replay + history + deltas.
    /// Client → kernel, first frame from a peer that is not on this
    /// machine: a token from `.arbos/access.toml`. A WebSocket client may
    /// send it as `Authorization: Bearer` or `?token=` instead. Loopback
    /// peers never need it.
    Auth {
        token: String,
    },
    Hello {
        /// The place's face (name, glyph, colour from `project.toml`),
        /// so a client draws the same identity as the desktop's tab in
        /// the first round trip. `changed project.toml` follows a rewrite.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        identity: Option<crate::project::ProjectIdentity>,
        /// This node's store as every node on the hub addresses it,
        /// `arbos://<machine>/<project>/`; absent when the kernel is on no
        /// hub. A client hands it to others (a brief, a link) instead of
        /// a machine-local path.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        store: Option<String>,
        protocol: u32,
        kernel: String,
        tail: u32,
        focus: String,
    },
    /// Client → kernel: replay `agent`'s transcript lines, oldest first,
    /// at most `limit` (capped), `history_end` after the last one. With
    /// `before`: the `limit` lines with `seq < before` nearest to it — the
    /// page above what a client holds (the phone scrolling to the top of
    /// the replayed tail, M-54). Else: lines with `seq > since`, the page
    /// after.
    History {
        agent: String,
        #[serde(default)]
        since: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<u64>,
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
    /// Client → kernel: one file under `.arbos/`, as text. `path` is
    /// relative to `.arbos/` (`agents/root/plan.md`). Answered with `file`;
    /// a file past the cap comes back `truncated` and is paged with `tail`.
    /// For clients that cannot read the folder (a phone); the desktop
    /// reads the files itself.
    Read {
        path: String,
    },
    /// Kernel → client: the file, or the reason it was refused.
    File {
        path: String,
        #[serde(default)]
        text: String,
        size: u64,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        truncated: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Client → kernel: bytes `from..from+limit` of a file under `.arbos/`,
    /// cut back to a line boundary. For the open transcript segment: keep
    /// `to` and ask again from it.
    Tail {
        path: String,
        #[serde(default)]
        from: u64,
        #[serde(default)]
        limit: u64,
    },
    /// Kernel → client: the bytes as text, and where they sit in the file.
    Chunk {
        path: String,
        from: u64,
        to: u64,
        size: u64,
        #[serde(default)]
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Kernel → every client, about once a second while something moved:
    /// a file under `.arbos/` (path relative to it) was `created`,
    /// `modified`, or `removed`; `size` is its length now. A client that
    /// mirrors a file asks for the part it lacks with `tail` or `read`.
    /// Only a fixed set is watched: each agent's `agent.md`, `notes.md`
    /// (its checklist), `transcript.jsonl`, `feedback.jsonl`, `checkpoints.jsonl`,
    /// `instructions.md`, and the place's `focus`, `user.md`, `memory.md`,
    /// `kernel.json`, `project.toml`, `notes.md`, `archived.md`,
    /// `docs/project-context.md`. `notes.md` is also announced the moment
    /// a turn that changed it ends.
    Changed {
        path: String,
        kind: String,
        size: u64,
    },
    /// Client → kernel: write one file under `.arbos/`, whole. A peer on
    /// the mesh writing by address (`arbos://…`); the receiving kernel
    /// applies the same rules as a local write (root-owned pages and
    /// protected files are refused; only the store's shared folders).
    /// `base_hash` is the sha-256 of the content the writer last read:
    /// the write happens only if the file still has it (`""` = must not
    /// exist yet); absent = write regardless. Answered with `written`.
    Put {
        path: String,
        #[serde(default)]
        text: String,
        /// The file's bytes, base64 (standard alphabet, padding optional):
        /// a photo or a file from a client with no path on this machine
        /// (the phone). With `data`, `text` is ignored. Lands under
        /// `.arbos/attachments/…` (or a shared folder), at most
        /// `PUT_MAX_BYTES`; the following `user` frame names the same
        /// relative path in `attachments`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base_hash: Option<String>,
    },
    /// Kernel → client: the outcome of a `put`. `hash` is the sha-256 of
    /// what is on disk now; on a refusal or a conflict `error` says why
    /// and `hash` is still the current file's, so the writer can re-read.
    Written {
        path: String,
        #[serde(default)]
        size: u64,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        hash: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Client → kernel: the entries of a folder under `.arbos/`.
    List {
        #[serde(default)]
        path: String,
    },
    /// Kernel → client: the folder's entries, sorted by name.
    Listing {
        path: String,
        #[serde(default)]
        entries: Vec<Entry>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Kernel → client, every few seconds while a model call has produced
    /// nothing yet: the call is alive and has run `secs` seconds. A
    /// thinking model can be silent for a minute; without this the turn
    /// looks dead. Never on the transcript.
    Working {
        agent: String,
        secs: u64,
    },
    /// Kernel → client, after `hello` and after every change: which model
    /// provider this kernel talks to and whether it holds a key for it.
    /// `source` is where the key comes from — `config`, `env:VAR`,
    /// `secrets:NAME`, `memory` — or `none`. Never the key itself.
    Provider {
        provider: String,
        model: String,
        key: bool,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        source: String,
    },
    /// Client (owner only) → kernel: use this provider and key from now
    /// on. The model key belongs to the user, not the machine: a window
    /// that holds one hands it to a kernel that has none, over the
    /// connection it already authenticated. `remember` writes it to the
    /// kernel's `config.toml` (owner-readable); otherwise it lives in the
    /// kernel's memory and dies with it. Never echoed, never logged.
    Configure {
        provider: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        api_base: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        model: String,
        api_key: String,
        #[serde(default)]
        remember: bool,
    },
    /// Client → kernel: put `agent` back to the start of its `turn`-th user
    /// turn (1-based, counting `user` transcript lines). The transcript is
    /// cut there (the cut lines are archived beside it) and, with `files`,
    /// the project's tracked files go back to that turn's checkpoint. The
    /// agent must be idle. Answered with `rewound`, or an `error`.
    Rewind {
        agent: String,
        turn: u32,
        #[serde(default)]
        files: bool,
    },
    /// Kernel → every client: the transcript of `agent` now ends before
    /// `line`; `dropped` lines went to the archive; `restored` names the
    /// project state put back, when files were. Reload the chat from the
    /// files or cut your own copy — the lines will not be replayed.
    /// Kernel → every client: what `agent` is doing now, in a few words —
    /// the agent's own `status` line (`source: "agent"`) or the kernel's
    /// guess from the tool in flight (`"derived"`). An empty `step` means
    /// idle: the line goes.
    Status {
        agent: String,
        step: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        since: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        source: String,
    },
    Rewound {
        agent: String,
        line: u64,
        dropped: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        restored: Option<String>,
        /// The file restore is still running: a second `rewound` with
        /// `restored` (or an `error`) follows. The cut itself is done.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        pending: bool,
    },
    /// Live text as the model streams it, one frame per chunk. The whole
    /// step arrives later as an `event` with a `seq` (the transcript line).
    AssistantDelta {
        agent: String,
        text: String,
        /// The model step within the turn, 1-based; the settled
        /// `assistant` event of the same step carries the same number.
        #[serde(default)]
        step: u64,
    },
    ThinkingDelta {
        agent: String,
        text: String,
        #[serde(default)]
        step: u64,
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
    /// A question for the user. `id` names it: the ask tool's `call_id`, or
    /// `approve-N` for an allow/deny prompt. An `answer` or `approve` that
    /// carries the id resolves that question and nothing else (qa-021).
    Ask {
        agent: String,
        question: String,
        options: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
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
    /// Try Live (A-02): a client asks for the screen the agent works on.
    /// For an agent on another machine the kernel forwards the request
    /// over its link and relays the answer back.
    Screen {
        agent: String,
    },
    /// The answer: one PNG of the screen, base64, with where it came from.
    /// `error` instead when no backend could capture (headless machine).
    Screenshot {
        agent: String,
        machine: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        png: String,
        /// `image/png` or `image/jpeg` (scaled for the wire).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        mime: String,
        #[serde(default)]
        width: u32,
        #[serde(default)]
        height: u32,
        at_ms: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// A person's words for `agent`. `channel` says where they came from:
    /// `voice` (a call, through the voice gateway) or `text` (typed). Absent
    /// means typed: the desktop and the CLI send no channel. `device` is
    /// the client that carried them: `phone`, `desktop`, `cli`. The kernel
    /// writes both into the inbox file and onto the transcript's `user`
    /// line so the record shows how each line arrived.
    User {
        agent: String,
        text: String,
        #[serde(default)]
        steer: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<String>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        channel: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        device: String,
        /// Run this turn on `model` instead of the agent's own; the next
        /// turn is back on the agent's. For "switch to <vision model> for
        /// this turn" when the composer holds an image the model cannot
        /// see. Empty or absent: the agent's model.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        model: String,
    },
    /// Client → kernel: a place was just opened for the first time; run
    /// root's kickoff turn (read the folder, seed the context file and
    /// the page, greet in two lines, no spawns, no questions). A no-op
    /// once root has any turn on record, so a second open sends nothing
    /// twice. Cursor's "Setting up environment" turn on a new Project.
    Kickoff {
        #[serde(default = "root_agent")]
        agent: String,
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
        /// The `ask` frame's `id`. Without it the answer is accepted only
        /// when exactly one question is pending in the kernel.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
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
    /// A frame type this build does not know. A kernel newer than the
    /// client (or the reverse) adds frames; an old reader must skip them,
    /// not drop the connection. Never sent on purpose.
    #[serde(other)]
    Unknown,
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
    /// What the agent is doing right now, in a few words (`status.toml`):
    /// its own `status` line, or the kernel's guess from the tool in
    /// flight. Absent when idle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// One folder entry in a `listing`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dir: bool,
    #[serde(default)]
    pub size: u64,
    /// Unix millis of the last write, when the file system says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<i64>,
}

fn root_agent() -> String {
    crate::ROOT_ID.to_string()
}
