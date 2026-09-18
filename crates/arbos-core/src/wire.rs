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
        /// Which build answered: the short git sha and the build time
        /// (`YYYY-MM-DDTHH:MMZ`). Semver moves rarely; these tell this
        /// morning's kernel from last week's, which is what a client (or
        /// the self-updater) needs to see skew at all. `built_at`, not
        /// `build`: the update feed's `build` is a commit count, and a
        /// count compared with a timestamp is a silent wrong answer.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        git_sha: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        built_at: String,
        /// The file this kernel was started from is gone (replaced or
        /// moved): it runs an old image, and a restart would run what is on
        /// disk now. Only ever sent when true; the one-word reason for a
        /// "restart needed" beside the version. See `arbos_core::binary_gone`.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        binary_gone: bool,
        /// No `git` on the kernel's machine: checkpoints, rewind and undo
        /// are off there (said once on root's transcript; also in
        /// `kernel.json`, which a phone attaching over the hub cannot
        /// read). Only ever sent when true.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        git_missing: bool,
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
    /// Client → kernel: the material for a feedback report about one
    /// exchange of `agent` — everything that happened because the user
    /// asked one thing, from a `user`/`kickoff` wake to the next such
    /// wake (the `done`, `job`, `serve` wakes inside stay inside). The
    /// exchange is the one holding `seq` (a line the user is looking at)
    /// or the tool call `call_id` (a tool line the user clicked, carried
    /// whole); absent both, the last the user opened. `tail` (default 0)
    /// adds the last N lines of the agent's transcript whatever exchange
    /// they fall in, slimmed and redacted the same — for "it keeps doing
    /// this" and for reproducing a behaviour bug; the wake lines in them
    /// show the turn structure. Answered with `feedback_bundle`:
    /// the lines slimmed (tool bodies budgeted by outcome — a glance for
    /// a call that went fine, the error uncut plus a tail-weighted 8 KB
    /// for one that failed or the span's last, whole for the named
    /// call; fat arguments glanced with their true length recorded), the
    /// children spawned in the span, the kernel log for its span, and
    /// which build answered — all redacted of credentials and bounded,
    /// so a client can send it as it is. `note` is the user's own words,
    /// redacted the same way and carried back.
    Feedback {
        agent: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seq: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
        #[serde(default, skip_serializing_if = "is_zero_u32")]
        tail: u32,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        note: String,
    },
    /// Kernel → client: the answer to `feedback`. `events` are the anchor
    /// exchange's lines; `tail` the last N transcript lines asked for
    /// (those already in `events` left out); `children` the spawned
    /// agents' lines since the exchange began,
    /// each `{agent, events}`; `log` kernel.log lines for the span (and
    /// the log's newest few). All JSON as the files hold them, after
    /// redaction and slimming. `redacted` counts what went; `truncated`
    /// says the cap cut something (the tail's oldest lines first, then
    /// children, then log lines to a floor, then the anchor's middle);
    /// `bytes` is this frame's size.
    FeedbackBundle {
        agent: String,
        turn: serde_json::Value,
        events: Vec<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tail: Vec<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        children: Vec<serde_json::Value>,
        log: Vec<serde_json::Value>,
        kernel: serde_json::Value,
        /// The place's settings that decide behaviour: permission mode,
        /// spend caps and spend so far, the window pin, `max_children`,
        /// fallback models, whether a key is in reach (a keyless kernel
        /// holds every waking line — a worker's brief reads as "Starting
        /// forever").
        #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
        place: serde_json::Value,
        /// Every agent of the place at the moment of the report — live
        /// ones with `running`, the live `step`, `pending_asks`, and the
        /// `inbox` (kind, from, wake) — and the anchor's archived children.
        /// What a roster shows, and what it does not.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        agents: Vec<serde_json::Value>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        note: String,
        redacted: serde_json::Value,
        truncated: bool,
        bytes: u64,
    },
    /// Client → kernel: one tool call's whole body, redacted and capped —
    /// the companion to a report that carried its glance, for whoever is
    /// fixing the bug to pull later. Answered with `tool_body`.
    ToolBody {
        agent: String,
        call_id: String,
    },
    /// Kernel → client: the answer to `tool_body`. `size` is the body's
    /// true length; `truncated` says the cap cut the middle.
    ToolBodyReply {
        agent: String,
        call_id: String,
        body: String,
        size: u64,
        truncated: bool,
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
    /// transcript's length now. `archived`: the agent has finished and its
    /// folder moved to `archive/agents/<id>/`; the lines came from there
    /// (M-27: a Done worker read as "Nothing on record yet"). `path` is
    /// that transcript, relative to `.arbos/`, for a client that reads
    /// files.
    HistoryEnd {
        agent: String,
        from: u64,
        to: u64,
        total: u64,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        archived: bool,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        path: String,
        /// The folder id the request resolved to when `agent` was a name
        /// (or another spelling) rather than the id; empty when the same.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        id: String,
        /// No agent, live or archived, by that id or name: `total` is 0
        /// because there is no record, not because the record is empty.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        unknown: bool,
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
        /// The transcript line of the prompt to rewind to, when the client
        /// knows it. Preferred over `turn`: a window's count of prompt
        /// cards can differ from the transcript's count of `user` lines
        /// (ask answers, skipped questions, a fork's copied history), and a
        /// count that is off by one restores the wrong turn or none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line: Option<u64>,
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
    /// Kernel → client: something the user should hear about even when
    /// no window is open — a top-level agent's reply finished (`reply`),
    /// a question or approval waits (`ask`), a turn failed (`error`), a
    /// line was posted to the user (`notice`). Sent live to every client;
    /// the unseen ones are replayed after `hello` with `replayed: true`,
    /// so a client that was away shows what it missed (a badge, an OS
    /// notification, a phone push from the client's side). Kept in
    /// `.arbos/notifications.jsonl`.
    Notify {
        id: u64,
        ts: i64,
        agent: String,
        kind: String,
        title: String,
        body: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        replayed: bool,
    },
    /// Client → kernel: the user has seen every notification with id ≤
    /// `through` (the chat was opened, the banner tapped). Kernel →
    /// client: the same, broadcast, so every window clears its badge.
    Seen {
        through: u64,
    },
    Snapshot {
        tree: Vec<TreeNode>,
        focus: String,
        budget: Option<Usage>,
        /// What this kernel holds at the moment of the attach — its jobs,
        /// shells and browser pages, as `surface_list` reports them — so a
        /// window rebuilds its rows from the kernel's record rather than
        /// its own memory, without having to know to ask. Absent from a
        /// kernel before this field: ask with `surfaces`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        surfaces: Vec<Surface>,
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
    /// End the agent's turn and its standing work — the stop button.
    /// With `reason: "superseded"` it is not a person stopping anything:
    /// a client is replacing the message this turn answers with a fuller
    /// one (the speech gateway, when a caller pauses mid-sentence). Only
    /// the turn ends; nothing is held or blocked; and a turn that had
    /// done nothing yet is cut from the record so one utterance is one
    /// line, not two lines and a stop notice.
    Stop {
        agent: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
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
        /// Who asked for this panel: `user` (a `shell` frame from a client,
        /// or a `terminal` tool call the agent marked as the user's request)
        /// or `agent` (the agent's own work: its jobs, its browser page, a
        /// terminal it opened for itself). A window opens its drawer for
        /// `user` and stays quiet for `agent`. Empty on frames from a kernel
        /// before this field: read as unknown, not as `user`.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        by: String,
    },
    /// A client asks for a shell of its own — the person's `$SHELL`,
    /// interactive, in `cwd` (absolute, or relative to the place; the place
    /// itself when absent). The kernel answers with a `board` frame for
    /// panel `terminal` with `by: user`, then `pty` output on the new page;
    /// a directory that does not exist is an `error` frame. `owner` is the
    /// agent the row docks under (`root` when absent).
    Shell {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
    },
    /// Client → kernel: what surfaces does this kernel hold — its jobs, its
    /// terminals, its browser pages, with their states — so a window that
    /// reattaches after a kernel died (and a replacement answered) can
    /// reconcile its rows against the kernel's record instead of its own
    /// memory. A job row ticking for a job no kernel runs, a terminal with
    /// no shell behind it, are what this frame ends. `agent` scopes the
    /// answer to one owner; absent, every agent of the place.
    Surfaces {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
    },
    /// Kernel → client: the answer to `surfaces`. Every row the kernel has
    /// and nothing it does not: a row a window holds that is not here has
    /// no process behind it. Answered on the asking connection only.
    SurfaceList {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
        surfaces: Vec<Surface>,
        at_ms: i64,
    },
    /// Client → kernel: what each turn of `agent` changed in the working
    /// tree — the rewind checkpoints' diff, readable at last (side-panels
    /// handover 5). Turn N's files are the difference between the tree
    /// saved at its start and the tree saved at the next turn's start; the
    /// newest turn is measured against the working tree as it is now.
    /// `limit` newest turns (0: the kernel's default). Answered on the
    /// asking connection only, as `turn_change_list`; read now, never
    /// from a cache.
    TurnChanges {
        agent: String,
        #[serde(default)]
        limit: u64,
    },
    /// Kernel → client: the answer to `turn_changes`, oldest turn first.
    TurnChangeList {
        agent: String,
        turns: Vec<TurnChange>,
        at_ms: i64,
    },
    /// A frame type this build does not know. A kernel newer than the
    /// client (or the reverse) adds frames; an old reader must skip them,
    /// not drop the connection. Never sent on purpose.
    #[serde(other)]
    Unknown,
}

/// One turn's mark on the working tree, for a files panel.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnChange {
    /// The transcript line the turn began on — the same number `rewind`
    /// and the checkpoint carry, so a row can name the turn.
    pub line: u64,
    /// When the turn began, Unix millis.
    pub ts: i64,
    /// A later turn has begun: this one's files are final. False for the
    /// newest turn, measured against the tree as it is now — a row that
    /// may still grow.
    pub ended: bool,
    /// Files whose contents differ between the turn's start and its end.
    /// Paths are relative to the place, as git prints them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<FileChange>,
    /// Paths the turn's tool calls said they touched (`edit`, `write`,
    /// `apply_patch`, a `bash` that dirtied tracked files), whether or not
    /// the file differs at the end — an edit undone in the same turn is
    /// here and not in `files`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub touched: Vec<String>,
    /// Why `files` could not be measured: the turn's tree was not saved
    /// (the record says why), or the place is not a git repository. Empty
    /// when `files` is the truth (an empty `files` then means no change).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub unmeasured: String,
}

/// One file between two trees, as `git diff --numstat` counts it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    /// `added` | `modified` | `deleted` | `renamed` (then `from` is set) |
    /// `typechange`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub from: String,
    /// Lines added and removed; both zero with `binary: true`.
    #[serde(default)]
    pub added: u64,
    #[serde(default)]
    pub removed: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub binary: bool,
}

/// One surface a kernel holds, as `surface_list` reports it. The first
/// seven fields are the `board` frame's, so a window keys the row the same
/// way; the rest is what a row needs to be titled honestly (side panels,
/// kernel handover 3): who ran it, since when, how it ended, whether its
/// journal is still on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Surface {
    /// The agent the row docks under.
    pub owner: String,
    /// `process` (a job) | `terminal` (a shell) | `browser` (a page).
    pub panel: String,
    /// The row's id: the job id, the pty page, the browser page.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// A job's command line, a page's title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// A page's URL, a job's journal path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// `user` | `agent`: who asked for it. Empty when not recorded.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub by: String,
    /// A process is behind it now: the job's process is alive, the shell
    /// is alive, the page is registered.
    pub running: bool,
    /// The kernel's own words for its state: a job's status line
    /// ("running for 41s (pid 4812)", "exited with code 0 after 41s"),
    /// "shell alive (pid N)" / "shell gone", "page open".
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    /// A job's output journal: `present` | `gone`. Absent for other panels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal: Option<String>,
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
    /// The chat's label when nobody named it: the model's summary after
    /// the first turn (F-156), or a client's own cut of the first prompt
    /// until that lands. Empty when the agent has a real `name`, or before
    /// any label exists. A window prefers `name`, then this.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
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
    /// The definition the agent was spawned from (`spawn kind=explore`),
    /// when one was; the window shows a worker's kind on its line (F-56).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent_kind: String,
    /// The agent cannot write (a read-only kind, or `readonly: true`): a
    /// glyph on the worker line says so while it runs, not after.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub readonly: bool,
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

fn is_zero_u32(n: &u32) -> bool {
    *n == 0
}
