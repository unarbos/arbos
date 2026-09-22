//! One kernel chat bridged into the UI.
//!
//! The connection is a WebSocket to the attached Arbos kernel. Dropping
//! [`ChatSession`] closes that socket; the kernel stays up. A foreground
//! pump drains events in coalesced batches with a 120ms frame floor while
//! streaming — one notify per frame, not per chunk.
//!
//! A session outlives its connection. [`ChatSession::flush`] writes the
//! transcript whenever a turn settles, and the kernel session id goes
//! with it, so a relaunch opens the same chat again.

use crate::{
    agent::acp::{self, Event, KernelSurface, Launch, Session},
    model::{
        attachment::{DescribedImage, MessageImage, Prompt, UserMessage},
        panel::OpenedBy,
        place::Place,
        record::{self, Record},
        settings,
        workspace::Workspace,
    },
    view::component::transcript,
};
use anyhow::anyhow;
use bezel::gpui::{Context, Task};
use cacp::schema::{
    ContentBlock, MaybeUndefined, SessionConfigKind, SessionConfigOption, SessionConfigOptionValue,
    SessionModeState, SessionUpdate, StopReason, ToolCallContent, ToolCallStatus, ToolKind,
};
use serde::{Deserialize, Serialize};
use std::{
    cell::Cell,
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const STREAM_FRAME: Duration = Duration::from_millis(32);

/// How long a delegate stays on the tree after its turn ends, so one
/// that is between turns does not flicker out.
pub const DELEGATE_GRACE: Duration = Duration::from_millis(1500);

/// The kernel tails transcripts on a 200 ms tick, so the last record of a
/// turn can land after `Turn idle`. Activity inside this window after the
/// end is that tail, not a new turn.
const TAIL_LAG: Duration = Duration::from_millis(1000);

/// How often the poll may read a transcript tail for one chat.
const PROBE_EVERY: Duration = Duration::from_secs(10);
/// The tail of the rewind notice while the kernel is still restoring files.
const RESTORING: &str = "restoring files\u{2026}";
/// The window's line for a turn that ended with nothing under the prompt.
const NO_REPLY: &str = "no reply from the kernel";

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ToolStatus {
    Running,
    Success,
    Failure,
}

pub use arbos_core::wire::PlanNode;

#[derive(Clone, Serialize, Deserialize)]
pub enum ChatItem {
    User(UserMessage),
    /// Someone else spoke into this chat: another session, or another door.
    From {
        who: String,
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<MessageImage>,
    },
    Agent(String),
    Thinking {
        text: String,
        done: bool,
        /// Seconds this thought streamed, once it finished. Missing on
        /// older transcripts and while tokens are still arriving.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        secs: Option<u32>,
    },
    Tool {
        id: String,
        kind: ToolKind,
        label: String,
        status: ToolStatus,
        output: String,
        /// Display diff from the kernel (`Details.diff`). The model never
        /// sees this; the transcript paints it as Cursor's edit card.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diff: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        child_session: Option<String>,
        /// How long the call ran, once it has. The "Worked for …" line
        /// under a settled turn adds these up.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        secs: Option<u32>,
        /// What the call does, in the model's few words (`bash`'s
        /// `description`, kept by the kernel as the record's `label`):
        /// "Ran List repo contents and recent commits" instead of
        /// "Ran 1 command". None when the model gave none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        desc: Option<String>,
    },
    /// Something the session has to say for itself: a stop reason, or a
    /// failure. `failed` picks which strip it paints as.
    Notice {
        text: String,
        failed: bool,
    },
    /// A kernel reminder addressed to the model, shown dim so the user
    /// knows why the next reply starts with a page update.
    Nudge(String),
    /// A wake that opened a turn with no prompt of the user's — a worker's
    /// report (`done`, `say`), a subscription (`serve`), a job. Cursor's
    /// Project chat draws each as its own "Worked Ns" segment with the
    /// coordinator's words under it; this item is the segment's boundary
    /// and carries its clock. Draws nothing itself.
    Wake {
        kind: String,
        text: Option<String>,
        /// Unix millis the kernel wrote the wake, when known.
        at: Option<i64>,
        /// Wall seconds from the wake to the turn's end, once stamped.
        secs: Option<u32>,
    },
    /// Files a tool produced for the user to look at: screenshots and
    /// screen recordings. One row per tool call; click opens the file.
    Artifacts(Vec<Artifact>),
    /// A question the agent asked, answered: the card folded to one line.
    /// An empty `answer` is a skip.
    Asked {
        question: String,
        answer: String,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Image,
    Video,
}

impl ArtifactKind {
    /// By file extension. Anything not a known video is an image: the
    /// kernel only lists pictures and clips here.
    pub fn of_path(path: &str) -> Self {
        let ext = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        match ext.as_str() {
            "mp4" | "mov" | "webm" | "mkv" | "m4v" | "avi" => Self::Video,
            _ => Self::Image,
        }
    }
}

/// One file on the artifacts row. `thumb` is the picture itself, or a
/// clip's poster frame; missing when the file could not be decoded.
#[derive(Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub kind: ArtifactKind,
    pub path: String,
    pub name: String,
    /// `5.0s · 166 KB`, from the tool's own report when it gave one.
    #[serde(default)]
    pub caption: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumb: Option<MessageImage>,
}

impl Artifact {
    /// The picture at `path`, or a clip at `path` with `poster` beside it.
    pub fn load(path: &str, poster: Option<&str>, caption: &str) -> Self {
        let kind = ArtifactKind::of_path(path);
        let picture = match kind {
            ArtifactKind::Image => Some(path),
            ArtifactKind::Video => poster,
        };
        let thumb = picture.and_then(|p| MessageImage::from_file(PathBuf::from(p)).ok());
        Self {
            kind,
            path: path.to_owned(),
            name: PathBuf::from(path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(path)
                .to_owned(),
            caption: caption.to_owned(),
            thumb,
        }
    }
}

/// How much of the context window the conversation has taken, as the agent
/// counts it.
///
/// Runtime only, and not written to the record: the file carries the
/// transcript, and whichever agent reads it back counts for itself.
#[derive(Clone, Copy)]
pub struct Usage {
    pub used: u64,
    pub size: u64,
    /// Dollars this chat has spent since it was opened, when the provider
    /// prices turns (OpenRouter does). Summed from every turn's report.
    pub spent: Option<f64>,
    /// The last finished turn's price.
    pub last_cost: Option<f64>,
}

impl Usage {
    /// How full, as a fraction. A window of no size is an agent that does not
    /// count, and there is nothing to draw for it.
    pub fn fraction(&self) -> Option<f32> {
        (self.size > 0).then(|| (self.used as f32 / self.size as f32).clamp(0., 1.))
    }
}

/// The turn in flight: when it started, and what the context had spent by then.
///
/// Runtime only, like [`Usage`], and for the same reason — the two numbers it
/// exists to count from are only read while a turn is running, and a relaunch
/// has none.
#[derive(Clone, Copy)]
struct Flight {
    at: SystemTime,
    used: u64,
}

/// One selectable answer on an ask-tool question.
#[derive(Clone)]
pub struct AskOption {
    pub id: String,
    pub label: String,
}

/// One multiple-choice question the kernel paused for.
#[derive(Clone)]
pub struct AskQuestion {
    pub id: String,
    pub prompt: String,
    pub options: Vec<AskOption>,
    pub allow_multiple: bool,
}

/// What the user picked on one question, ready to send back.
#[derive(Clone, Serialize)]
pub struct AskAnswer {
    pub question_id: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub selected_ids: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub other_text: String,
}

#[derive(Clone, Default)]
pub struct AskDraft {
    pub selected: Vec<String>,
    pub other: bool,
    pub other_text: String,
}

/// The ask tool's form, held until Continue or Skip.
pub struct AskPrompt {
    pub request_id: String,
    pub title: String,
    pub questions: Vec<AskQuestion>,
    pub page: usize,
    pub drafts: HashMap<String, AskDraft>,
}

impl AskPrompt {
    pub fn current(&self) -> Option<&AskQuestion> {
        self.questions.get(self.page)
    }

    pub fn draft(&self, id: &str) -> AskDraft {
        self.drafts.get(id).cloned().unwrap_or_default()
    }

    fn save_other(&mut self, held: &str) {
        let Some(question) = self.questions.get(self.page) else {
            return;
        };
        let held = held.trim();
        if held.is_empty() {
            return;
        }
        let draft = self.drafts.entry(question.id.clone()).or_default();
        if draft.other && draft.other_text.is_empty() {
            draft.other_text = held.to_owned();
        }
    }

    fn build_answers(&self, held: &str) -> (Vec<AskAnswer>, String) {
        let held = held.trim();
        let current_id = self.questions.get(self.page).map(|q| q.id.as_str());
        let mut used_as_other = false;
        // Words that name an option are that option, once: typed "alpha"
        // with alpha picked (or not) reached the model as "alphaalpha".
        let mut used_as_pick = false;
        let answers = self
            .questions
            .iter()
            .map(|question| {
                let mut draft = self.draft(question.id.as_str());
                if current_id == Some(question.id.as_str()) && !held.is_empty() {
                    let named = question.options.iter().find(|o| {
                        o.id.eq_ignore_ascii_case(held) || o.label.trim().eq_ignore_ascii_case(held)
                    });
                    if let Some(option) = named {
                        if !draft.selected.iter().any(|id| *id == option.id) {
                            if question.allow_multiple {
                                draft.selected.push(option.id.clone());
                            } else {
                                draft.selected = vec![option.id.clone()];
                            }
                        }
                        draft.other = false;
                        used_as_pick = true;
                    }
                }
                if draft.other
                    && draft.other_text.is_empty()
                    && current_id == Some(question.id.as_str())
                    && !held.is_empty()
                {
                    draft.other_text = held.to_owned();
                    used_as_other = true;
                }
                AskAnswer {
                    question_id: question.id.clone(),
                    selected_ids: draft.selected,
                    other_text: if draft.other {
                        draft.other_text
                    } else {
                        String::new()
                    },
                }
            })
            .collect();
        let details = if used_as_other || used_as_pick {
            String::new()
        } else {
            held.to_owned()
        };
        (answers, details)
    }
}

/// Whether the session has a live kernel socket.
/// One frame of the screen an agent works on, for Try Live.
#[derive(Clone)]
pub struct LiveScreen {
    pub image: Option<Arc<bezel::gpui::Image>>,
    pub machine: String,
    pub at: Instant,
    pub width: u32,
    pub height: u32,
    pub error: Option<String>,
}

/// The kernel's own account of its provider, from its `provider` frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KernelProvider {
    pub provider: String,
    pub model: String,
    pub key: bool,
    pub source: String,
}

pub enum Connection {
    /// No socket: read back from disk, archived, or given up on.
    Idle,
    Connecting,
    Live(Box<Session>),
    /// The connection could not be restored within the retry window.
    Lost,
}

/// A slash command the agent offers for this session — what to type, and what
/// it does. The description is required by the protocol, so a picker can
/// always say what a name does not.
#[derive(Clone, PartialEq, Eq)]
pub struct Command {
    pub name: String,
    pub description: String,
}

pub struct ChatSession {
    pub id: u64,
    pub entry: settings::Agent,
    /// The directory the agent runs in, which is the project's. Kept here so
    /// a session can reconnect and file itself without asking the workspace.
    pub cwd: PathBuf,
    /// ssh alias when the project is on another machine.
    pub host: Option<String>,
    /// Where `.arbos` extras are written. A remote place uses a local sidecar.
    pub store: PathBuf,
    pub connection: Connection,
    /// Which kernel answered on this socket, as its `hello` described it.
    /// `None` until the handshake and again as soon as the socket is gone, so
    /// it is never a build nothing is attached to. Read it through
    /// [`crate::model::project::Project::kernel_build`], which asks the
    /// connection rather than this field.
    pub kernel_build: Option<crate::kernel::KernelBuild>,
    pub items: Vec<ChatItem>,
    /// Hide every transcript item before this index. Runtime only: typing
    /// `clear` in the composer empties the view. The file on disk is
    /// unchanged, so the agent keeps the same context.
    pub hide_before: usize,
    /// The agent's plan as the kernel last sent it: every node, inbox
    /// rows included. The strip above the composer draws the open ones.
    pub plan: Vec<PlanNode>,
    /// A plan question the next composer send answers.
    pub answering: Option<u64>,
    pub questions: Option<AskPrompt>,
    pub commands: Vec<Command>,
    /// The prompt in flight was dictated: voice the answer when it lands.
    pub voice_reply: bool,
    /// Leftover ACP mode state. The kernel has no session modes; the
    /// composer chip is a model picker (`set_model`).
    pub modes: Option<SessionModeState>,
    /// The same for config options, which is where the model lives — see
    /// `SessionConfigOptionCategory::Model`.
    pub config: Vec<SessionConfigOption>,
    /// Last model this session was switched to, from the kernel picker.
    /// Empty until a pick or a catalog current is applied.
    pub model: Option<String>,
    /// Context spent, when the agent says.
    pub usage: Option<Usage>,
    /// What the turn in flight is counted from — see [`Self::elapsed`] and
    /// [`Self::spent`].
    flight: Option<Flight>,
    /// When the current thought started streaming. Runtime only.
    thought_at: Option<SystemTime>,
    /// The `Agent` item the current step's deltas are building, until the
    /// step's recorded line replaces it. Runtime only.
    streaming_agent: Option<usize>,
    /// The last question answered or skipped from this window, and when:
    /// the transcript tail repeats it, and that repeat is not a new card.
    answered_ask: Option<(String, Instant)>,
    /// The kernel's answer to a `feedback` ask: the material behind an
    /// in-app report, waiting for the review sheet to take it. Drained
    /// rather than kept — it is one sheet's worth, not session state.
    pub feedback: Option<Box<crate::feedback::Bundle>>,
    /// Why there will be no bundle, when the kernel has said so. Held for the
    /// sheet exactly like the bundle: a report's trouble is not a line of the
    /// conversation.
    pub feedback_error: Option<String>,
    /// Try Live (A-02): the latest screen frame from the agent's machine,
    /// and whether the live view is open (the poll runs while it is).
    pub live_screen: Option<LiveScreen>,
    pub live_open: bool,
    /// A Stop was asked from this window (button, stop word, Force):
    /// the turn ending without an answer is then not a kernel failure.
    stop_requested: bool,
    /// `draft` was set by the model (a follow-up taken back, a rewind) and
    /// the composer, which otherwise owns the text while bound, must take it.
    pub draft_pushed: bool,
    /// The kernel's last `working` heartbeat: seconds the model call had
    /// been silent, and when it was heard. Cleared by any real progress.
    /// Runtime only.
    pub working: Option<(u64, Instant)>,
    /// The kernel said it has no model key (`provider {key: false}`): the
    /// provider it wants one for. Cleared when a key arrives. Runtime only.
    pub provider_missing: Option<String>,
    /// What the kernel serving this chat last said about itself: provider,
    /// model, whether it holds a key, and where the key came from. Settings
    /// shows it beside this window's own reading of config.toml (ui-010).
    pub kernel_provider: Option<KernelProvider>,
    /// A "Rewind here" was sent for the turn whose prompt is this item;
    /// the kernel's `rewound` cuts the pane there. Runtime only.
    rewind_to: Option<usize>,
    /// Automatic reconnects to a remote kernel since the last good
    /// connection, and when the next one fires. Runtime only.
    pub reconnect_attempt: u32,
    pub reconnect_at: Option<Instant>,
    /// The connection generation the pending timer was armed for; a Send
    /// or Stop meanwhile starts a new generation, and its failure must arm
    /// a new timer rather than defer to the stale one.
    pub reconnect_gen: u64,
    /// Why the last connect failed, in plain words, for the bar to keep
    /// for as long as the state lasts ("ssh to ArbosLife refused the
    /// key"). Cleared by a good connection. Runtime only.
    pub connect_fault: Option<String>,
    /// The agent's own name for the session, from `SessionInfoUpdate`.
    pub title: String,
    /// The name you typed, which the agent never overwrites. Two fields rather
    /// than one and a flag: whose name it is *is* the state.
    pub name: Option<String>,
    /// When the session last had something to say. Wall clock, not `Instant`,
    /// because the file has to carry it across a launch.
    pub updated: SystemTime,
    /// The kernel's id for this chat — what `open` with `session_id` resumes.
    pub agent_session: Option<String>,
    /// Local parent, once resolved. A child agent sits under this session.
    pub parent: Option<u64>,
    /// The parent's kernel session id, filed so a relaunch can resolve
    /// [`Self::parent`] after every session is read back.
    pub parent_kernel: Option<String>,
    pub delegate_number: Option<u64>,
    /// Where the session is written, once it has anything to write.
    pub file: Option<PathBuf>,
    /// Sidebar place among siblings. Dragging writes this; a turn does not.
    pub rank: i64,
    /// Whether the user archived it. Typing into it clears this.
    pub closed: bool,
    pub streaming: bool,
    /// Prompts waiting for an agent to send them to: what was typed while a
    /// turn was in flight, and what a dispatched card opened the session with.
    pub queue: VecDeque<Prompt>,
    /// The last user bubble is already in the pane; the kernel has not
    /// seen it yet. Replay lands the card before the socket is back.
    pending_wire: bool,
    /// How many prompts at the queue's front already have their card on
    /// the pane — lines held while the place was unreachable (`af-03`).
    /// Their turn opens when each is sent; no second card lands.
    held_cards: usize,
    pub transcript: transcript::State,
    /// What is sitting in the composer for this session. Written with the
    /// transcript so a kill mid-type comes back with the same line.
    pub draft: String,
    /// Sub-agents and scheduled firings in flight for this chat. Runtime
    /// only — the kernel is the record.
    pub live: Vec<crate::kernel::LiveWork>,
    /// This chat's sub-agents as the transcript and the task rail show
    /// them. Runtime only: the workspace refreshes it before each draw.
    pub children: Vec<ChildSummary>,
    /// The kernel's notifications for this agent the user has not seen
    /// (#293): a reply, a question, a failure, a notice. The tab's badge
    /// counts them; opening the chat sends `seen` and empties this.
    pub unseen: Vec<Notification>,
    /// The highest notification id any client has seen.
    pub seen_through: u64,
    /// Live notifications not yet offered to the OS: the window decides
    /// (unfocused, or another project's tab) and drains this.
    pub to_notify: Vec<Notification>,
    /// This worker cannot write (`readonly: true` on its `agent.md`): the
    /// glyph after its name in the worker line and the roster.
    pub readonly: bool,
    /// The kind it was spawned from (`explore`, …), for the kind chip.
    pub agent_kind: Option<String>,
    /// When `live` last became non-empty, for the braille tick.
    pub live_since: Option<SystemTime>,
    /// Seconds a thought had before a later step reopened it; added back
    /// when it settles again.
    pub thought_carry: u32,
    /// This turn's streamed items by model step (`step` on the deltas):
    /// the settled line of a step replaces the item it opened, in place.
    /// Runtime only; cleared when a turn ends.
    pub step_items: HashMap<u64, usize>,
    pub step_thoughts: HashMap<u64, usize>,
    /// When the kernel last said anything on this socket. A TCP socket
    /// does not notice a cut for twenty seconds or more, so "connected"
    /// keyed on it lies; this is keyed on what the person cares about —
    /// whether the kernel is answering.
    pub last_frame_at: Instant,
    /// When the turn last made progress the person could see: a token, a
    /// tool row, a step, a line of a running command's output (`job`
    /// frames count — Jacob's Mac read "Nothing has arrived in 2m 26s …
    /// check the model key" while `bubble_sort.py` was running and the key
    /// was fine). Not a probe's answer, not `Alive`. The stall hint's clock.
    pub progress_at: Instant,
    /// When the last liveness probe (`list`) went out, so one goes per
    /// quiet window rather than every frame.
    probe_at: Cell<Option<Instant>>,
    /// The streamed text of each live reply item as it arrived, by item
    /// index: the deltas merge into this, and the item shows it with any
    /// tool-call markup cut (`crate::markup`). Runtime only.
    pub stream_raw: HashMap<usize, String>,
    /// Prompts this window sent whose `user` record has not come back yet,
    /// squashed. The kernel may hold a line in its inbox for minutes
    /// (a turn still ending, workers running) before it runs it and writes
    /// the record; the echo is then matched by these, not by a clock —
    /// a two-minute window doubled two prompts after a relaunch (F-94).
    awaiting_echo: VecDeque<String>,
    /// When this window asked the kernel for the kickoff turn on an empty
    /// root (Cursor's "Setting up environment"). Runtime only.
    pub kickoff_at: Option<SystemTime>,
    /// How long the kickoff turn took, once it ended: the turn has no
    /// prompt to stamp, so its "Worked Ns" lives here.
    pub kickoff_secs: Option<u32>,
    /// The kickoff is due as soon as the kernel reports a provider key.
    pub kickoff_wanted: bool,
    /// What the agent says it is doing right now (the kernel's `status`
    /// event); cleared when the turn ends.
    pub status: Option<String>,
    /// The kernel's "waiting on <worker> — <step>" for a parent whose
    /// worker is live (#366). Runtime only; the kernel clears it the
    /// moment no worker is live.
    pub waiting: Option<String>,
    /// The agent set its `status` while a worker of its was live: the step
    /// is about the workers, and goes when the last of them finishes —
    /// "Waiting on three sorting workers" stood over three Done lines for
    /// minutes (Jacob's report 2026-09-17-6; #432's author handed the
    /// drawing here). Runtime only.
    status_over_workers: bool,
    /// The highest record line (`seq`) this pane has taken from the kernel's
    /// transcript. A recorded line at or below it is one the pane already
    /// holds: a replay reaching a live pane appended the record's head a
    /// second time (F-135, cycle 33). Reset when the kernel rewinds, since
    /// the numbering starts again below. Runtime only; primed from the
    /// cards' own `seq` at load.
    record_seq: u64,
    /// Inside a recorded line the pane already holds: its events are
    /// dropped until `RecordLineEnd`. Runtime only.
    holding_line: bool,
    /// Models the kernel has said are "not available to this key" in this
    /// chat: dropped from the picker until the key changes. Runtime only.
    pub unavailable_models: HashSet<String>,
    /// The attached command this chat has running as a job, by title —
    /// set by the workspace before a draw, like `children`. Runtime only.
    pub running_job: Option<String>,
    /// When each running tool call began, by call id, so its finished
    /// item can say how long it took.
    tool_started: HashMap<String, Instant>,
    /// The turn's reply so far was only `status:` lines, kept out of the
    /// transcript — still a reply, not a kernel that never answered.
    status_only: bool,
    /// The kernel is mid-turn here without this window having asked: a
    /// delegate on its brief, a chat a `say` woke. Set by the first
    /// streamed token or tool, cleared by `Turn idle`. Runtime only.
    pub turn_open: bool,
    /// When the kernel last said this chat's turn ended (`Turn idle`).
    /// `None` while a turn runs, and before the first one ends. A
    /// finished delegate leaves the tree on this. Runtime only.
    pub turn_ended: Option<Instant>,
    /// When the poll last read the transcript tail to learn whether a
    /// turn is over — the relaunch case, where no `Turn idle` will come.
    probed_at: Option<Instant>,
    /// When this window last asked the kernel to compact (`/compact`): the
    /// kernel's "nothing to compact yet" is an answer to that, and to
    /// nothing else. Runtime only.
    compact_asked: Option<Instant>,
    /// Which attach attempt owns `_pump`. A newer resume must not let an
    /// older socket's EOF mark this chat lost.
    pub(crate) attach_gen: u64,
    _pump: Task<()>,
}

impl ChatSession {
    /// Open a kernel chat. `seed` is its first prompt, sent as soon as
    /// the socket is up — what a dispatched card rides in on.
    pub fn connect(
        id: u64,
        entry: settings::Agent,
        place: Place,
        seed: Option<String>,
        cx: &mut Context<Workspace>,
    ) -> Self {
        Self::connect_at(id, entry, place, None, seed, cx)
    }

    /// Point a new row at an existing kernel session — a child the kernel
    /// already minted.
    pub fn adopt(
        id: u64,
        entry: settings::Agent,
        place: Place,
        agent_session: String,
        parent: Option<u64>,
        parent_kernel: Option<String>,
        cx: &mut Context<Workspace>,
    ) -> Self {
        let mut chat = Self::connect_at(id, entry, place, Some(agent_session.clone()), None, cx);
        chat.agent_session = Some(agent_session);
        chat.parent = parent;
        chat.parent_kernel = parent_kernel;
        chat
    }

    fn connect_at(
        id: u64,
        entry: settings::Agent,
        place: Place,
        previous: Option<String>,
        seed: Option<String>,
        cx: &mut Context<Workspace>,
    ) -> Self {
        let attach_gen = 1;
        let pump = pump(id, &entry, place.clone(), previous, attach_gen, cx);
        Self {
            id,
            entry,
            store: place.store(),
            host: place.host.clone(),
            cwd: place.path,
            connection: Connection::Connecting,
            kernel_build: None,
            items: Vec::new(),
            hide_before: 0,
            plan: Vec::new(),
            answering: None,
            questions: None,
            commands: Vec::new(),
            voice_reply: false,
            modes: None,
            config: Vec::new(),
            model: None,
            usage: None,
            flight: None,
            thought_at: None,
            streaming_agent: None,
            answered_ask: None,
            feedback: None,
            feedback_error: None,
            live_screen: None,
            live_open: false,
            stop_requested: false,
            draft_pushed: false,
            working: None,
            provider_missing: None,
            kernel_provider: None,
            rewind_to: None,
            reconnect_attempt: 0,
            reconnect_at: None,
            reconnect_gen: 0,
            connect_fault: None,
            title: String::new(),
            name: None,
            updated: SystemTime::now(),
            agent_session: None,
            parent: None,
            parent_kernel: None,
            delegate_number: None,
            children: Vec::new(),
            file: None,
            rank: 0,
            closed: false,
            streaming: false,
            queue: seed.into_iter().map(Prompt::from).collect(),
            pending_wire: false,
            held_cards: 0,
            transcript: transcript::State::default(),
            draft: String::new(),
            live: Vec::new(),
            tool_started: HashMap::new(),
            status_only: false,
            live_since: None,
            thought_carry: 0,
            step_items: HashMap::new(),
            step_thoughts: HashMap::new(),
            last_frame_at: Instant::now(),
            progress_at: Instant::now(),
            probe_at: Cell::new(None),
            stream_raw: HashMap::new(),
            awaiting_echo: VecDeque::new(),
            kickoff_at: None,
            kickoff_secs: None,
            readonly: false,
            agent_kind: None,
            unseen: Vec::new(),
            seen_through: 0,
            to_notify: Vec::new(),
            kickoff_wanted: false,
            status: None,
            waiting: None,
            status_over_workers: false,
            record_seq: 0,
            holding_line: false,
            unavailable_models: HashSet::new(),
            running_job: None,
            turn_open: false,
            turn_ended: None,
            probed_at: None,
            compact_asked: None,
            attach_gen,
            _pump: pump,
        }
    }

    /// A session read back from disk. It starts idle. Launch reconnects
    /// every open chat that still has a kernel id — see
    /// [`crate::model::workspace::Workspace`]'s wake path. An archived
    /// one stays idle until it is opened again.
    pub fn restore(
        id: u64,
        file: PathBuf,
        place: Place,
        entry: settings::Agent,
        record: Record,
    ) -> Self {
        let updated = record.at();
        Self {
            id,
            entry,
            store: place.store(),
            host: place.host.clone(),
            cwd: place.path,
            connection: Connection::Idle,
            kernel_build: None,
            items: record.items,
            hide_before: 0,
            plan: Vec::new(),
            answering: None,
            questions: None,
            commands: Vec::new(),
            voice_reply: false,
            modes: None,
            config: Vec::new(),
            model: None,
            usage: None,
            flight: None,
            thought_at: None,
            streaming_agent: None,
            answered_ask: None,
            feedback: None,
            feedback_error: None,
            live_screen: None,
            live_open: false,
            stop_requested: false,
            draft_pushed: false,
            working: None,
            provider_missing: None,
            kernel_provider: None,
            rewind_to: None,
            reconnect_attempt: 0,
            reconnect_at: None,
            reconnect_gen: 0,
            connect_fault: None,
            title: record.title,
            name: record.name,
            updated,
            agent_session: record.session,
            parent: None,
            parent_kernel: record.parent,
            delegate_number: record.delegate_number,
            children: Vec::new(),
            file: Some(file),
            rank: record.rank,
            closed: record.closed,
            streaming: false,
            queue: VecDeque::new(),
            pending_wire: false,
            held_cards: 0,
            transcript: transcript::State::default(),
            draft: record.draft,
            live: Vec::new(),
            tool_started: HashMap::new(),
            status_only: false,
            live_since: None,
            thought_carry: 0,
            step_items: HashMap::new(),
            step_thoughts: HashMap::new(),
            last_frame_at: Instant::now(),
            progress_at: Instant::now(),
            probe_at: Cell::new(None),
            stream_raw: HashMap::new(),
            awaiting_echo: VecDeque::new(),
            kickoff_at: None,
            kickoff_secs: None,
            readonly: false,
            agent_kind: None,
            unseen: Vec::new(),
            seen_through: 0,
            to_notify: Vec::new(),
            kickoff_wanted: false,
            status: None,
            waiting: None,
            status_over_workers: false,
            record_seq: 0,
            holding_line: false,
            unavailable_models: HashSet::new(),
            running_job: None,
            turn_open: false,
            turn_ended: None,
            probed_at: None,
            compact_asked: None,
            attach_gen: 0,
            _pump: Task::ready(()),
        }
    }

    /// A kernel chat this window has not filed yet. Idle until the wake
    /// path attaches the socket, or the user opens it.
    pub fn from_kernel(
        id: u64,
        place: Place,
        entry: settings::Agent,
        kernel_id: String,
        title: String,
        name: Option<String>,
        items: Vec<ChatItem>,
        updated: SystemTime,
    ) -> Self {
        Self {
            id,
            entry,
            store: place.store(),
            host: place.host.clone(),
            cwd: place.path,
            connection: Connection::Idle,
            kernel_build: None,
            items,
            hide_before: 0,
            plan: Vec::new(),
            answering: None,
            questions: None,
            commands: Vec::new(),
            voice_reply: false,
            modes: None,
            config: Vec::new(),
            model: None,
            usage: None,
            flight: None,
            thought_at: None,
            streaming_agent: None,
            answered_ask: None,
            feedback: None,
            feedback_error: None,
            live_screen: None,
            live_open: false,
            stop_requested: false,
            draft_pushed: false,
            working: None,
            provider_missing: None,
            kernel_provider: None,
            rewind_to: None,
            reconnect_attempt: 0,
            reconnect_at: None,
            reconnect_gen: 0,
            connect_fault: None,
            title,
            name,
            updated,
            agent_session: Some(kernel_id),
            parent: None,
            parent_kernel: None,
            delegate_number: None,
            children: Vec::new(),
            file: None,
            rank: 0,
            closed: false,
            streaming: false,
            queue: VecDeque::new(),
            pending_wire: false,
            held_cards: 0,
            transcript: transcript::State::default(),
            draft: String::new(),
            live: Vec::new(),
            tool_started: HashMap::new(),
            status_only: false,
            live_since: None,
            thought_carry: 0,
            step_items: HashMap::new(),
            step_thoughts: HashMap::new(),
            last_frame_at: Instant::now(),
            progress_at: Instant::now(),
            probe_at: Cell::new(None),
            stream_raw: HashMap::new(),
            awaiting_echo: VecDeque::new(),
            kickoff_at: None,
            kickoff_secs: None,
            readonly: false,
            agent_kind: None,
            unseen: Vec::new(),
            seen_through: 0,
            to_notify: Vec::new(),
            kickoff_wanted: false,
            status: None,
            waiting: None,
            status_over_workers: false,
            record_seq: 0,
            holding_line: false,
            unavailable_models: HashSet::new(),
            running_job: None,
            turn_open: false,
            turn_ended: None,
            probed_at: None,
            compact_asked: None,
            attach_gen: 0,
            _pump: Task::ready(()),
        }
    }

    fn place(&self) -> Place {
        Place {
            host: self.host.clone(),
            path: self.cwd.clone(),
        }
    }

    pub fn touched(&self) -> u128 {
        self.updated
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    }

    fn to_record(&self) -> Record {
        Record {
            agent: self.entry.name.clone(),
            session: self.agent_session.clone(),
            parent: self.parent_kernel.clone(),
            delegate_number: self.delegate_number,
            title: self.title.clone(),
            name: self.name.clone(),
            updated: self
                .updated
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            closed: self.closed,
            rank: self.rank,
            items: self.items.clone(),
            draft: self.draft.clone(),
        }
    }

    /// Write the session extras out. The file is minted on the first write
    /// and not before — opening a project must not put an `.arbos/desktop/`
    /// in it.
    /// A problem report about the exchange whose prompt is transcript line
    /// `seq` has been written: leave its id on that prompt card.
    pub fn mark_reported(&mut self, seq: u64, report: &str) {
        let card = self.items.iter_mut().find_map(|item| match item {
            ChatItem::User(message) if message.seq == Some(seq) => Some(message),
            _ => None,
        });
        if let Some(card) = card {
            card.reported = Some(report.to_string());
            self.flush();
        }
    }

    pub fn flush(&mut self) {
        let worth = !self.items.is_empty()
            || self.is_delegate()
            || !self.draft.trim().is_empty()
            || self.agent_session.is_some();
        if !worth {
            return;
        }
        if self.file.is_none() {
            self.file = record::create(&self.store);
        }
        if let Some(file) = &self.file {
            record::write(file, self.to_record());
        }
    }

    /// Attach to the kernel again, opening the same chat when we have its id.
    pub fn resume(&mut self, cx: &mut Context<Workspace>) {
        self.attach_gen = self.attach_gen.saturating_add(1);
        self._pump = pump(
            self.id,
            &self.entry,
            self.place(),
            self.agent_session.clone(),
            self.attach_gen,
            cx,
        );
        self.connection = Connection::Connecting;
        self.closed = false;
    }

    /// Close the socket and keep the transcript. The kernel stays up.
    pub fn close(&mut self) {
        // Finish the in-flight turn before the socket goes, so the kernel
        // is not left waiting on a request this window will never answer.
        self.cancel();
        self.attach_gen = self.attach_gen.saturating_add(1);
        self.connection = Connection::Idle;
        self._pump = Task::ready(());
        self.streaming = false;
        self.turn_open = false;
        self.closed = true;
        self.pending_wire = false;
        self.held_cards = 0;
        self.queue.clear();
        self.flush();
    }

    /// The turn just ended: write its wall time on the prompt that started
    /// it, for the "Worked 21s" line. Once per turn; a late duplicate end
    /// leaves the first figure.
    fn stamp_worked(&mut self) {
        // No clock of this window's own when the turn opened before it
        // attached — a worker's brief is on the card before the worker's
        // chat is joined mid-turn — the opener's own stamp says when it
        // began (F-131, cycle 32: the worker tab read its summary phrase
        // where Cursor's says "Worked for 20s").
        let since_opener = || {
            let began = self.items.iter().rev().find_map(|item| match item {
                ChatItem::User(message) => Some(message.sent_at),
                ChatItem::Wake { at, .. } => Some(*at),
                _ => None,
            })??;
            let now = arbos_core::now_ms();
            (now > began).then(|| Duration::from_millis((now - began) as u64))
        };
        let Some(elapsed) = self.elapsed().or_else(since_opener) else {
            return;
        };
        // The kickoff turn has no prompt: its time is the chat's, measured
        // from the ask to the last turn end before the user's first line.
        if let Some(at) = self.kickoff_at
            && !self
                .items
                .iter()
                .any(|item| matches!(item, ChatItem::User(_)))
        {
            self.kickoff_secs = Some(at.elapsed().map(|d| d.as_secs() as u32).unwrap_or(0));
            return;
        }
        // A segment a wake opened (a worker's report) takes the stamp: it
        // is the turn that just ended, not the prompt before it.
        if let Some(ChatItem::Wake { secs, .. }) = self
            .items
            .iter_mut()
            .rev()
            .find(|item| matches!(item, ChatItem::User(_) | ChatItem::Wake { .. }))
            && secs.is_none()
        {
            *secs = Some(elapsed.as_secs().min(u32::MAX as u64) as u32);
            return;
        }
        let Some(ChatItem::User(message)) = self
            .items
            .iter_mut()
            .rev()
            .find(|item| matches!(item, ChatItem::User(_)))
        else {
            return;
        };
        if message.worked_secs.is_none() {
            message.worked_secs = Some(elapsed.as_secs().min(u32::MAX as u64) as u32);
        }
    }

    /// How long the turn in flight has been running.
    pub fn elapsed(&self) -> Option<Duration> {
        self.flight?.at.elapsed().ok()
    }

    /// How long the current thought has been streaming.
    pub fn thought_elapsed(&self) -> Option<Duration> {
        self.thought_at?.elapsed().ok()
    }

    /// What the turn in flight has spent, as the agent counts context.
    ///
    /// The difference rather than the total: `used` is the whole conversation,
    /// and what a running turn is costing is what it has added to it. `None`
    /// until an agent has counted at all — most do not until the first turn is
    /// answered, so a first turn shows its clock and nothing else.
    pub fn spent(&self) -> Option<u64> {
        Some(self.usage?.used.saturating_sub(self.flight?.used))
    }

    pub fn live(&self) -> bool {
        match &self.connection {
            Connection::Live(session) => !session.is_closed(),
            _ => false,
        }
    }

    /// The attach write side is gone. The UI may still hold a `Live` box;
    /// the next send would fail with "attach writer closed".
    pub(crate) fn reap_dead_socket(&mut self) {
        if !self.socket_dead() {
            return;
        }
        self.forget_socket();
    }

    fn socket_dead(&self) -> bool {
        match &self.connection {
            Connection::Live(session) => session.is_closed(),
            _ => false,
        }
    }

    fn forget_socket(&mut self) {
        self.connection = Connection::Lost;
        // The build belonged to that socket. Nothing is attached now, and
        // "nothing is attached" is an answer; last week's version is not.
        self.kernel_build = None;
        self.streaming = false;
        self.turn_open = false;
        self.flight = None;
        self.fail_running_tools();
    }

    /// A token or a tool arrived: the kernel is mid-turn here. Inside
    /// `TAIL_LAG` of the last `Turn idle` it is the tail catching up.
    fn turn_alive(&mut self) {
        // Real progress: the model is no longer just thinking in silence.
        self.working = None;
        // A record arriving after the turn's own end is that turn's tail
        // — however late (the kernel's settled records landed past the
        // 1 s lag and reopened an idle chat: "stop the turn before
        // rewinding" on an idle chat, F-122b, cycle 29). A new turn
        // announces itself first: a prompt sent here, another client's
        // line, a wake, or the kernel's thinking pulse — each clears
        // `turn_ended`, and only then does a token reopen the turn.
        if self.turn_ended.is_some() {
            return;
        }
        self.turn_open = true;
    }

    /// "Thinking for Ns": the heartbeat's count plus the time since it was
    /// heard, so the label ticks between heartbeats. None when the model
    /// is producing.
    pub fn thinking_for(&self) -> Option<Duration> {
        let (secs, at) = self.working?;
        Some(Duration::from_secs(secs) + at.elapsed())
    }

    /// The last turn ended less than `DELEGATE_GRACE` ago: a child between
    /// turns, not yet gone.
    pub fn recently_ended(&self) -> bool {
        self.turn_ended
            .is_some_and(|at| at.elapsed() < DELEGATE_GRACE)
    }

    /// Whether this delegate is done for the tree: its last turn ended
    /// `DELEGATE_GRACE` ago and nothing has run since. The parent's spawn
    /// call is the workspace's to check.
    pub fn delegate_done(&self) -> bool {
        self.is_delegate()
            && !self.closed
            && !self.busy()
            && self
                .turn_ended
                .is_some_and(|at| at.elapsed() >= DELEGATE_GRACE)
    }

    /// Whether the poll should read this chat's transcript tail: a chat
    /// no `Turn idle` has reached — a relaunch, a socket that was down, or
    /// a tail that lagged past `TAIL_LAG` — and not read in `PROBE_EVERY`.
    /// Every delegate, and any chat whose turn is open without a prompt
    /// of this window's in flight (`streaming` holds from the send to the
    /// end): a turn a wake opened, whose end the kernel told the socket
    /// once, at the Stop, and not again when the waited spawn came back
    /// (F-179; the root sat Working 55 s past its own `turn_complete`).
    /// A tool card still Running does not hold the probe off: a turn the
    /// kernel ended over a tool (a stop while its command ran) leaves that
    /// card without a result, and the card was the one thing keeping the
    /// tab Working.
    pub(crate) fn wants_probe(&self) -> bool {
        (self.is_delegate() || self.turn_open)
            && !self.closed
            && !self.streaming
            && self.live.is_empty()
            && self.turn_ended.is_none()
            && self.probed_at.is_none_or(|at| at.elapsed() >= PROBE_EVERY)
    }

    /// The tail was read. `ended` is a turn-ending record last in the file:
    /// the kernel is idle here whatever the stream said, and a tool it
    /// never answered is not coming back.
    pub(crate) fn probed(&mut self, ended: bool) {
        self.probed_at = Some(Instant::now());
        if ended && self.turn_ended.is_none() {
            self.turn_open = false;
            self.working = None;
            self.turn_ended = Some(Instant::now());
            self.fail_running_tools();
            self.flush();
        }
    }

    /// Whether the UI should treat this chat as in flight: a streamed
    /// turn, a tool that has not returned, or a sub-agent the kernel
    /// still lists as live. The composer Stop and the work header
    /// both read this, so a `delegate` that outlives `streaming` does
    /// not look idle.
    pub fn busy(&self) -> bool {
        self.streaming || self.turn_open || self.has_running_tool() || !self.live.is_empty()
    }

    pub(crate) fn has_running_tool(&self) -> bool {
        self.items.iter().any(|item| {
            matches!(
                item,
                ChatItem::Tool {
                    status: ToolStatus::Running,
                    ..
                }
            )
        })
    }

    /// How long since the turn last made visible progress (see
    /// [`Self::progress_at`]).
    pub fn quiet_for(&self) -> Duration {
        self.progress_at.elapsed()
    }

    /// A running command printed something: progress, even though the
    /// `job` frame is drawn by the process row and never reaches `apply`.
    pub fn mark_progress(&mut self) {
        self.progress_at = Instant::now();
    }

    /// The command the turn is waiting on, if the last thing the turn did
    /// was start one that has not returned: the stall hint names it rather
    /// than blaming the model key.
    pub fn running_command(&self) -> Option<&str> {
        let start = self
            .items
            .iter()
            .rposition(|item| matches!(item, ChatItem::User(_)))
            .unwrap_or(0);
        self.items[start..]
            .iter()
            .rev()
            .find_map(|item| match item {
                ChatItem::Tool {
                    label,
                    status: ToolStatus::Running,
                    ..
                } => Some(label.as_str()),
                _ => None,
            })
    }

    /// Push local bubbles the kernel never stored, then title from them.
    /// Lines the kernel recorded while no window was attached — a held
    /// follow-up that ran after the app was shut, its answer, a worker's
    /// report — are on the transcript and not in this window's record; the
    /// live stream only starts at the attach. The view then showed the
    /// answer with no prompt over it, or nothing (F-105, cycle 22: the
    /// follow-up ran between quit and relaunch, and only "restart" came
    /// through). On attach, every turn the kernel wrote after the newest
    /// prompt this record knows is taken in whole.
    pub(crate) fn adopt_kernel_tail(&mut self) {
        if self.host.is_some() {
            return;
        }
        let Some(sid) = self.agent_session.clone() else {
            return;
        };
        let Some(replay) = crate::kernel::session_history(&self.place(), &sid) else {
            return;
        };
        let squash = |s: &str| s.split_whitespace().collect::<String>();
        let is_user = |item: &ChatItem| matches!(item, ChatItem::User(_));
        // The record's newest prompt, by its transcript line when the card
        // kept one, else by its words.
        let Some(ChatItem::User(known)) = self.items.iter().rev().find(|item| is_user(item)) else {
            return;
        };
        let known_words = squash(&known.text);
        let known_at = match known.seq {
            Some(seq) => replay
                .items
                .iter()
                .rposition(|item| matches!(item, ChatItem::User(m) if m.seq == Some(seq))),
            None => replay.items.iter().rposition(
                |item| matches!(item, ChatItem::User(m) if squash(&m.text) == known_words),
            ),
        };
        let Some(known_at) = known_at else {
            return;
        };
        let Some(next_turn) = replay.items[known_at + 1..]
            .iter()
            .position(is_user)
            .map(|offset| known_at + 1 + offset)
        else {
            return;
        };
        // Whole turns only, and none this record already has: a prompt the
        // live stream delivered in the meantime is matched by its words.
        let fresh: Vec<ChatItem> = replay.items[next_turn..]
            .iter()
            .filter(|item| match item {
                ChatItem::User(m) => !self.items.iter().any(
                    |mine| matches!(mine, ChatItem::User(k) if squash(&k.text) == squash(&m.text) && k.sent_at == m.sent_at),
                ),
                _ => true,
            })
            .cloned()
            .collect();
        if fresh.iter().any(is_user) {
            self.items.extend(fresh);
            self.flush();
        }
    }

    pub(crate) fn sync_kernel_history(&mut self) {
        if let Some(sid) = self.agent_session.clone() {
            // A line typed while the socket was down has its card on the
            // transcript already and its words still in the queue: they go
            // as a frame when the queue drains, and the kernel records them
            // then. Seeded here as well, the kernel held the line twice —
            // once written by this window, once from the frame (QA,
            // 2026-09-16, after a kernel respawn on a fresh place).
            let unsent = self.unsent_cards();
            if unsent.is_empty() {
                crate::kernel::seed_transcript(&self.place(), &sid, &self.items);
            } else {
                let sent: Vec<ChatItem> = self
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(ix, _)| !unsent.contains(ix))
                    .map(|(_, item)| item.clone())
                    .collect();
                crate::kernel::seed_transcript(&self.place(), &sid, &sent);
            }
        }
        self.take_title_from_first_prompt();
    }

    /// The prompt cards whose words are still queued for the wire: the
    /// newest card for each queued prompt, matched by its words.
    fn unsent_cards(&self) -> HashSet<usize> {
        let squash = |s: &str| s.split_whitespace().collect::<String>();
        let mut taken = HashSet::new();
        for prompt in &self.queue {
            let words = squash(&prompt.text);
            let found = self
                .items
                .iter()
                .enumerate()
                .rev()
                .find(|(ix, item)| {
                    !taken.contains(ix)
                        && matches!(item, ChatItem::User(message) if squash(&message.text) == words)
                })
                .map(|(ix, _)| ix);
            if let Some(ix) = found {
                taken.insert(ix);
            }
        }
        taken
    }

    /// First user line → a short title. Skipped when they already named it.
    pub(crate) fn take_title_from_first_prompt(&mut self) {
        if self
            .name
            .as_deref()
            .is_some_and(|n| !arbos_core::chattitle::is_generic(n, self.agent_session.as_deref()))
        {
            return;
        }
        if !self.title.is_empty()
            && !arbos_core::chattitle::is_generic(&self.title, self.agent_session.as_deref())
        {
            return;
        }
        let Some(text) = first_user_text(&self.items) else {
            return;
        };
        let Some(title) = arbos_core::chattitle::from_prompt(text) else {
            return;
        };
        self.title = title;
        if let Some(sid) = &self.agent_session {
            crate::kernel::set_chat_title(&self.place(), sid, &self.title);
        }
    }

    /// Take the kernel's transcript when it is ahead: more items, or the
    /// same count with thoughts / write diffs the local cache dropped.
    pub fn adopt_history(&mut self, items: Vec<ChatItem>) {
        if history_beats(&items, &self.items) {
            // A view the person cleared stays cleared. The kernel's copy
            // of the same conversation is longer than the window's (it
            // keeps the thoughts and tool rows the cache drops), and a
            // mark left at the old count then sat in the middle of it and
            // put the hidden lines back on screen — `clear` undone by the
            // next merge (#629's regression, Jacob 09-18).
            let cleared = self.view_cleared();
            self.items = items;
            self.hide_before = if cleared {
                self.items.len()
            } else {
                self.hide_before.min(self.items.len())
            };
            self.transcript = transcript::State::default();
            self.take_title_from_first_prompt();
            self.flush();
            return;
        }
        self.merge_tool_diffs(&items);
        self.merge_turn_clocks(&items);
    }

    /// The kernel's clock on prompts this window typed: when its own copy of
    /// the transcript wins, the prompts it kept have no `sent_at` (the date
    /// line, "Just now") and a worked time measured live or not at all. The
    /// history's prompts, matched in order by their words, lend theirs.
    fn merge_turn_clocks(&mut self, incoming: &[ChatItem]) {
        let squash = |s: &str| s.split_whitespace().collect::<String>();
        let theirs: Vec<&crate::model::attachment::UserMessage> = incoming
            .iter()
            .filter_map(|item| match item {
                ChatItem::User(message) => Some(message),
                _ => None,
            })
            .collect();
        let mut next = 0;
        let mut changed = false;
        for item in &mut self.items {
            let ChatItem::User(mine) = item else {
                continue;
            };
            let Some(pos) = theirs[next.min(theirs.len())..]
                .iter()
                .position(|m| squash(&m.text) == squash(&mine.text))
            else {
                continue;
            };
            let found = theirs[next + pos];
            next += pos + 1;
            if mine.sent_at.is_none() && found.sent_at.is_some() {
                mine.sent_at = found.sent_at;
                changed = true;
            }
            if found.worked_secs.is_some() && mine.worked_secs != found.worked_secs {
                mine.worked_secs = found.worked_secs;
                changed = true;
            }
        }
        if changed {
            self.flush();
        }
    }

    fn merge_tool_diffs(&mut self, incoming: &[ChatItem]) {
        let diffs: HashMap<&str, &str> = incoming
            .iter()
            .filter_map(|item| match item {
                ChatItem::Tool {
                    id,
                    diff: Some(diff),
                    ..
                } if !diff.trim().is_empty() => Some((id.as_str(), diff.as_str())),
                _ => None,
            })
            .collect();
        if diffs.is_empty() {
            return;
        }
        let mut changed = false;
        for item in &mut self.items {
            let ChatItem::Tool { id, diff: held, .. } = item else {
                continue;
            };
            if held.as_ref().is_some_and(|diff| !diff.trim().is_empty()) {
                continue;
            }
            let Some(diff) = diffs.get(id.as_str()) else {
                continue;
            };
            *held = Some((*diff).to_owned());
            changed = true;
        }
        if changed {
            self.flush();
        }
    }

    pub fn connecting(&self) -> bool {
        matches!(self.connection, Connection::Connecting)
    }

    /// Whether sending to it would start an agent: it is not talking to one,
    /// and one is not already on the way.
    pub fn idle(&self) -> bool {
        matches!(self.connection, Connection::Idle | Connection::Lost)
    }

    /// Whether this chat can be pointed at the kernel again. The kernel is
    /// attached, not spawned from `entry.command`, so an empty command is
    /// not a reason to hide the composer.
    /// Whether a reconnect makes sense: the chat is open and, for a local
    /// place, its agent folder still exists. A child deleted under a live
    /// window is not attached to again and again.
    pub fn resumable(&self) -> bool {
        !self.closed && !self.agent_gone() && !self.place_gone()
    }

    /// A local place whose folder is no longer where the window knew it —
    /// renamed, moved or deleted under a running kernel (QA `af-03`). Not
    /// an archived agent: the kernel stops, the words must not.
    pub fn place_gone(&self) -> bool {
        self.host.is_none() && !self.cwd.as_os_str().is_empty() && !self.cwd.is_dir()
    }

    /// Keep a line typed while the place is unreachable: its card on the
    /// pane, the words in the queue for the next attach, no turn opened —
    /// nothing is coming until the folder is back, so no shimmer.
    pub fn hold_offline(&mut self, content: Prompt) {
        if content.is_empty() {
            return;
        }
        // The card is on the pane now; the kernel's own record of the line,
        // when the queue drains, is its echo and must not land twice.
        let squashed: String = content.text.split_whitespace().collect();
        if !squashed.is_empty() {
            self.awaiting_echo.push_back(squashed);
            while self.awaiting_echo.len() > 8 {
                self.awaiting_echo.pop_front();
            }
        }
        self.items.push(ChatItem::User(content.message()));
        self.updated = SystemTime::now();
        self.queue.push_back(content);
        self.held_cards += 1;
        self.flush();
    }

    /// The record's newest line at attach, from the kernel's replay or the
    /// file: history the pane holds, not news. Only for a pane with a
    /// record of its own — a worker's fresh tab has nothing yet and draws
    /// its first lines from the live frames.
    fn hold_record_through(&mut self, seq: u64) {
        if !self.items.is_empty() {
            self.record_seq = self.record_seq.max(seq);
        }
    }

    /// Whether a recorded line at `seq` is one this pane already holds. A
    /// line above the high-water mark moves it and is news; `0` (a live
    /// frame, an older kernel) is never a record and always passes.
    fn record_held(&mut self, seq: u64) -> bool {
        if seq == 0 {
            return false;
        }
        if self.record_seq == 0 {
            // Primed once, from the cards the record already matched.
            self.record_seq = self
                .items
                .iter()
                .filter_map(|item| match item {
                    ChatItem::User(message) => message.seq,
                    _ => None,
                })
                .max()
                .unwrap_or(0);
        }
        if seq <= self.record_seq {
            return true;
        }
        self.record_seq = seq;
        false
    }

    /// Lines held while the place was away are still waiting, the socket is
    /// back and nothing is in flight: a new line queues behind them so they
    /// go in the order they were typed.
    pub fn has_held_lines(&self) -> bool {
        self.held_cards > 0
            && self.live()
            && !self.streaming
            && !self.has_running_tool()
            && !self.queue.is_empty()
    }

    /// The workers this status was about have all finished: the status
    /// goes. Called by the workspace with the children's current states
    /// before a draw; the flag is set when a `status` lands while a child
    /// is working.
    pub fn settle_status_over_workers(&mut self, any_working: bool) {
        // A status that stood while a worker worked is about the workers,
        // whether it was set before the spawn or after it (the rig's
        // coordinator set "Waiting on one sorting worker" and then spawned;
        // Jacob's spawned and then set it).
        if any_working && self.status.is_some() {
            self.status_over_workers = true;
        }
        if self.status_over_workers && !any_working && !self.children.is_empty() {
            self.status = None;
            self.status_over_workers = false;
        }
    }

    /// Whether the newest notice already says the place is gone — one
    /// line per disappearance, however many lines are typed into it.
    pub fn has_place_gone_notice(&self) -> bool {
        self.items
            .iter()
            .rev()
            .find_map(|item| match item {
                ChatItem::Notice { text, .. } => Some(text.starts_with(PLACE_GONE)),
                _ => None,
            })
            .unwrap_or(false)
    }

    pub fn has_agent_gone_notice(&self) -> bool {
        self.items
            .iter()
            .rev()
            .take_while(|item| !matches!(item, ChatItem::Agent(_)))
            .any(|item| matches!(item, ChatItem::Notice { text, .. } if text.starts_with(AGENT_GONE)))
    }

    /// A local agent whose folder is no longer on disk while its place is:
    /// the kernel archived it (or someone removed it). A place that is gone
    /// takes every agent with it and is [`Self::place_gone`], not this.
    pub fn agent_gone(&self) -> bool {
        self.host.is_none()
            && !self.place_gone()
            && self.agent_session.as_deref().is_some_and(|sid| {
                !arbos_core::agent_exists(&arbos_core::Place::new(&self.cwd), sid)
            })
    }

    pub fn number_delegates(sessions: &mut [Self]) -> Vec<usize> {
        let mut next = sessions
            .iter()
            .filter_map(|chat| chat.delegate_number)
            .max()
            .unwrap_or(0);
        let mut missing: Vec<usize> = sessions
            .iter()
            .enumerate()
            .filter(|(_, chat)| chat.is_delegate() && chat.delegate_number.is_none())
            .map(|(index, _)| index)
            .collect();
        missing.sort_by(|&a, &b| {
            sessions[a]
                .agent_session
                .cmp(&sessions[b].agent_session)
                .then_with(|| sessions[a].id.cmp(&sessions[b].id))
        });
        for &index in &missing {
            next += 1;
            sessions[index].delegate_number = Some(next);
        }
        missing
    }

    pub fn is_delegate(&self) -> bool {
        self.parent.is_some() || self.parent_kernel.is_some()
    }
}

/// What a sub-agent is up to, as the parent's transcript and the task rail
/// say it. Derived from the child's own session state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildState {
    /// A turn is running.
    Working,
    /// Parked on a question for the user.
    Asking,
    /// Idle between turns; may be woken again.
    Waiting,
    /// Its last turn ended and nothing has run since.
    Done,
}

/// One sub-agent, as its parent shows it.
/// One kernel notification (#293).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub id: u64,
    pub ts: i64,
    /// `reply`, `ask`, `error`, `notice`.
    pub kind: String,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildSummary {
    pub id: u64,
    /// The kernel's agent id, which `say` lines name.
    pub kernel_id: Option<String>,
    pub title: String,
    pub state: ChildState,
    pub readonly: bool,
    pub agent_kind: Option<String>,
    /// What the worker is on right now: its `status` line from the kernel
    /// when it sent one, else the tool it is running or last ran.
    pub step: Option<String>,
}

impl ChatSession {
    /// The status line the agent named — unless it has since moved on to a
    /// tool of its own that is still running: then that tool is the truer
    /// line. Jacob's report 2026-09-17-6: "Waiting on three sorting
    /// workers" stood over three "Done" lines for minutes while the
    /// coordinator itself sat in `sleep 75`; the line that was true was
    /// "Running sleep 75; echo waited".
    pub fn live_status(&self) -> Option<String> {
        if let Some(job) = &self.running_job {
            return Some(step_label(&format!("bash {job}")));
        }
        for item in self.items.iter().rev() {
            if let ChatItem::Tool { label, status, .. } = item {
                if label.split_whitespace().next() == Some("status") {
                    break;
                }
                if *status == ToolStatus::Running {
                    return Some(step_label(label));
                }
            }
        }
        self.status.clone().filter(|s| !s.trim().is_empty())
    }

    /// The one line that says what this chat is doing: the kernel's status
    /// event, else the running tool's title, else the last tool's.
    pub fn current_step(&self) -> Option<String> {
        if let Some(status) = &self.status {
            return Some(status.clone());
        }
        let mut last = None;
        for item in self.items.iter().rev() {
            if let ChatItem::Tool { label, status, .. } = item {
                if *status == ToolStatus::Running {
                    return Some(step_label(label));
                }
                if last.is_none() {
                    last = Some(step_label(label));
                }
            }
            if matches!(item, ChatItem::User(_)) {
                break;
            }
        }
        last
    }
}

/// A tool's title as a step: "Reading main.py", "Running python3 main.py",
/// "Searching for foo" — the verb Cursor's status lines use, from the tool
/// name the label starts with.
/// The router's own step, which the window does not repeat. The kernel
/// sends "Choosing the next step" before every routed turn; on the chat
/// face it is the router announcing itself over and over, which is the
/// noise Jacob asked to be rid of (09-18). Who is choosing is on the
/// model card instead, as one quiet line that does not move.
fn is_router_step(text: &str) -> bool {
    text.eq_ignore_ascii_case("Choosing the next step")
}

/// The kernel's notice for a line typed again while its first copy still
/// waits (#362): `Already queued: "run it" waits for the running step …`.
pub fn is_already_queued(text: &str) -> bool {
    text.trim_start().starts_with("Already queued:")
}

/// The command a running tool row holds, short enough for one line of the
/// stall hint: "cd iota && python3 bubble_sort.py"; a tool that is not a
/// shell is "The running step".
pub fn command_short(label: &str) -> String {
    let label = label.trim();
    let (name, rest) = label.split_once(' ').unwrap_or((label, ""));
    let rest = rest.split_whitespace().collect::<Vec<_>>().join(" ");
    if !matches!(name, "bash" | "terminal" | "run" | "exec") || rest.is_empty() {
        return "The running step".to_string();
    }
    const MAX: usize = 48;
    if rest.chars().count() > MAX {
        let short: String = rest.chars().take(MAX - 1).collect();
        format!("{}…", short.trim_end())
    } else {
        rest
    }
}

fn step_label(label: &str) -> String {
    let label = label.trim();
    let (name, rest) = label.split_once(' ').unwrap_or((label, ""));
    let verb = match name {
        "read" | "cat" => "Reading",
        "write" | "edit" | "apply_patch" => "Editing",
        "bash" | "terminal" | "run" | "exec" => "Running",
        "grep" | "tgrep" | "find" | "search" | "ls" | "list" => "Searching",
        "fetch" | "browser" => "Fetching",
        "spawn" => "Spawning",
        "say" => "Reporting",
        "plan" => "Planning",
        "ask" => "Asking",
        "screenshot" | "record" => "Capturing",
        _ => "",
    };
    match (verb.is_empty(), rest.is_empty()) {
        (true, _) => label.to_string(),
        (false, true) => verb.to_string(),
        (false, false) => format!("{verb} {}", rest.trim()),
    }
}

impl ChatSession {
    /// The state a parent shows for this chat.
    pub fn child_state(&self) -> ChildState {
        // "Working" is drawn from something: frames on a live socket, a
        // tool this window saw start, or the kernel's own activity list —
        // never a flag alone on a chat nothing is attached to (F-137).
        let evidence = self.live() || self.has_running_tool() || !self.live.is_empty();
        // A worker mid-tool when the window relaunches has no tool row yet
        // (the kernel files a tool record when it ends) and no frame yet,
        // so busy() is false — but its status.toml, read on attach, holds
        // the step the kernel is on, and the kernel clears that file when
        // the turn ends. Live socket plus a kernel step is the kernel's own
        // word that it is working (F-172, d15: five sleeping workers drew
        // as idle rows after a relaunch).
        let stepping = self.status.is_some() && self.live();
        if (self.busy() || stepping) && evidence {
            ChildState::Working
        } else if self.answering.is_some() || self.plan_open().any(|n| n.do_kind == "ask") {
            ChildState::Asking
        } else if self.closed || self.turn_ended.is_some() || self.agent_gone() {
            ChildState::Done
        } else {
            ChildState::Waiting
        }
    }

    /// The title a `say` line's `who` resolves to: a sub-agent's title
    /// when `who` is one of this chat's children, else `who` itself with
    /// the ` → user` tail kept.
    pub fn who_label(&self, who: &str) -> String {
        let (head, tail) = match who.split_once(" → ") {
            Some((h, t)) => (h.trim(), Some(t.trim())),
            None => (who.trim(), None),
        };
        let name = if self.agent_session.as_deref() == Some(head) {
            // The agent's own notice to the user, filed on its chat.
            self.label()
        } else {
            self.children
                .iter()
                .find(|c| c.kernel_id.as_deref() == Some(head))
                .map(|c| c.title.clone())
                .filter(|title| !title.trim().is_empty())
                // A worker the window has not stamped yet (its report can
                // land before the roster does) is named from its id as a
                // person would read it: `add-sources-to-project-context`
                // → "Add sources to project context" (F-170), never the
                // raw id Cursor never shows.
                .unwrap_or_else(|| humanize_id(head))
        };
        match tail {
            Some(t) => format!("{name} → {t}"),
            None => name,
        }
    }
}

impl ChatSession {
    /// Hide the transcript that is already on screen. The file is not
    /// touched. A later send shows only the new lines.
    pub fn clear_view(&mut self) {
        self.hide_before = self.items.len();
    }

    /// The composer `clear` hid every line that is on this chat so far.
    /// A chat that never had a line is empty, not cleared.
    pub fn view_cleared(&self) -> bool {
        self.hide_before > 0 && self.hide_before >= self.items.len()
    }

    /// Explicit names override the delegate identity or the agent's title.
    pub fn label(&self) -> String {
        if let Some(name) = self
            .name
            .as_deref()
            .filter(|n| !arbos_core::chattitle::is_generic(n, self.agent_session.as_deref()))
        {
            return if self.is_delegate() {
                delegate_label(name)
            } else {
                name.to_string()
            };
        }
        if self.is_delegate() {
            // The kernel names a worker's folder after its brief
            // ("math-docstrings"); that is the name Cursor would show, not
            // a number. Only an id with nothing in it falls back to one.
            if let Some(name) = self.agent_session.as_deref().and_then(worker_name) {
                return name;
            }
            return self
                .delegate_number
                .map_or_else(|| "Delegate".into(), |number| format!("Delegate {number}"));
        }
        if !self.title.is_empty()
            && !arbos_core::chattitle::is_generic(&self.title, self.agent_session.as_deref())
        {
            return self.title.clone();
        }
        if let Some(text) = first_user_text(&self.items)
            && let Some(title) = arbos_core::chattitle::from_prompt(text)
        {
            return title;
        }
        "New chat".into()
    }

    /// Send now. A turn in flight is steered: the kernel takes the words at
    /// its next tool boundary, Cursor's default, and the card lands in the
    /// transcript at once. Nothing waits in this window's memory but a
    /// prompt typed before the socket is up, which goes the moment it is.
    pub fn send(&mut self, content: Prompt) {
        self.reap_dead_socket();
        if !self.live() {
            self.land_turn(&content);
            self.pending_wire = true;
            self.queue.push_back(content);
            return;
        }
        // The kickoff turn is not the user's to steer: words typed while it
        // sets the place up wait for it, as Cursor's "Send follow-up" does,
        // and open their own turn.
        if self.kickoff_running() {
            // The kernel holds a plain user frame for the next turn while
            // one runs; its own record of the line lands the card in order,
            // after the greeting, so nothing is drawn here now.
            let held = match &self.connection {
                Connection::Live(session) if !session.is_closed() => {
                    session.prompt(&content).is_ok()
                }
                _ => false,
            };
            if !held {
                self.queue.push_back(content);
            }
            self.flush();
            return;
        }
        if self.streaming || self.has_running_tool() {
            self.steer(content);
            return;
        }
        self.prompt(content);
    }

    /// Ask for the kickoff turn when it is wanted and the kernel has a
    /// provider key; once.
    fn send_kickoff_if_ready(&mut self) {
        if !self.kickoff_wanted || self.kickoff_at.is_some() || !self.items.is_empty() {
            return;
        }
        let keyed = self.kernel_provider.as_ref().is_some_and(|p| p.key);
        if !keyed {
            return;
        }
        if let Connection::Live(session) = &self.connection
            && session.kickoff().is_ok()
        {
            let now = SystemTime::now();
            self.kickoff_at = Some(now);
            self.kickoff_wanted = false;
            // The kickoff is a turn like any other for the clock: its
            // "Worked Ns" is stamped when it ends, from here.
            self.flight = Some(Flight {
                at: now,
                used: self.usage.map_or(0, |usage| usage.used),
            });
            self.new_turn_steps();
        }
    }

    /// The kickoff turn (asked for by this window, no prompt of the user's
    /// yet) is still running.
    pub fn kickoff_running(&self) -> bool {
        // Asked and not yet ended: the first model call can take twenty
        // seconds before anything arrives to make the chat look busy, and
        // a prompt typed in that quiet must still wait its turn (else its
        // card lands above the kickoff's work). A kickoff with no turn end
        // after long enough is given up on.
        let Some(at) = self.kickoff_at else {
            return false;
        };
        self.kickoff_secs.is_none()
            && (self.busy() || at.elapsed().is_ok_and(|d| d < KICKOFF_QUIET))
            && !self
                .items
                .iter()
                .any(|item| matches!(item, ChatItem::User(_)))
    }

    /// Hold the words for the next turn. The kernel keeps them as an inbox
    /// file — the follow-up row under the composer — and runs them when
    /// this turn ends, through a restart of this window too. On an idle
    /// chat this is a send.
    pub fn queue_next(&mut self, content: Prompt) {
        self.reap_dead_socket();
        if content.is_empty() {
            return;
        }
        if !self.live() {
            self.land_turn(&content);
            self.pending_wire = true;
            self.queue.push_back(content);
            return;
        }
        if !(self.streaming || self.has_running_tool()) {
            self.prompt(content);
            return;
        }
        let held = match &self.connection {
            Connection::Live(session) if !session.is_closed() => session.prompt(&content).is_ok(),
            _ => false,
        };
        if !held {
            self.queue.push_back(content);
            self.reap_dead_socket();
        }
        self.flush();
    }

    /// Send the next queued prompt, if there is one and nothing is in flight.
    pub fn drain(&mut self) {
        if self.streaming && !self.pending_wire {
            return;
        }
        if let Some(next) = self.queue.pop_front() {
            self.prompt(next);
        }
    }

    /// Words into the running turn. The card lands now, under the work in
    /// progress; the kernel's own record of the line carries the same words
    /// and is not drawn twice.
    fn steer(&mut self, content: Prompt) {
        let sent = match &self.connection {
            Connection::Live(session) if !session.is_closed() => session.steer(&content).is_ok(),
            _ => false,
        };
        if !sent {
            self.queue.push_back(content);
            self.reap_dead_socket();
            self.flush();
            return;
        }
        // The same words again while the first copy still waits to be
        // read — "run it" four times into a silent turn (Jacob's Mac,
        // 09-16). One bubble: the kernel files the line once and answers
        // the repeat with an "Already queued" notice, which lands under
        // that bubble (#362).
        if self.repeats_waiting_steer(&content.text) {
            self.updated = SystemTime::now();
            self.flush();
            return;
        }
        // The kernel records the steer as a `user` line a moment later;
        // that record is this card's echo. Matched only against the newest
        // card, a second steer typed before the first was recorded made
        // the first land twice (F-149, cycle 34: three lines typed fast
        // while the first ran, two of them doubled).
        let squashed: String = content.text.split_whitespace().collect();
        if !squashed.is_empty() {
            self.awaiting_echo.push_back(squashed);
            while self.awaiting_echo.len() > 8 {
                self.awaiting_echo.pop_front();
            }
        }
        let mut message = content.message();
        message.steer = true;
        self.items.push(ChatItem::User(message));
        self.updated = SystemTime::now();
        self.flush();
    }

    /// Whether `text` is the words of the newest steer card, and nothing
    /// but the kernel's "Already queued" notices has landed since — the
    /// steer has not been read yet, so a second card would be a repeat.
    fn repeats_waiting_steer(&self, text: &str) -> bool {
        let squash = |s: &str| s.split_whitespace().collect::<String>();
        let wanted = squash(text);
        if wanted.is_empty() {
            return false;
        }
        for item in self.items.iter().rev() {
            match item {
                ChatItem::Notice { text, .. } if is_already_queued(text) => continue,
                ChatItem::User(message) if message.steer => {
                    return squash(&message.text) == wanted;
                }
                _ => return false,
            }
        }
        false
    }

    fn prompt(&mut self, content: Prompt) {
        let sent = match &self.connection {
            Connection::Live(session) if !session.is_closed() => session.prompt(&content).is_ok(),
            _ => {
                self.queue.push_front(content);
                return;
            }
        };
        if !sent {
            self.queue.push_front(content);
            self.reap_dead_socket();
            self.flush();
            return;
        }
        if self.pending_wire {
            self.pending_wire = false;
            return;
        }
        if self.held_cards > 0 {
            self.held_cards -= 1;
            self.open_turn();
            return;
        }
        self.land_turn(&content);
    }

    /// The turn a card already on the pane now starts: the clock, the
    /// shimmer, the step table — everything `land_turn` does but the card.
    fn open_turn(&mut self) {
        self.new_turn_steps();
        self.turn_ended = None;
        self.flight = Some(Flight {
            at: SystemTime::now(),
            used: self.usage.map_or(0, |usage| usage.used),
        });
        self.updated = SystemTime::now();
        self.streaming = true;
        self.answered_ask = None;
        self.flush();
    }

    /// A prompt another client sent to this agent — the caller's words on
    /// a live call, the phone — as the kernel recorded it. This window's own
    /// prompt comes back the same way a moment after it was sent, so a line
    /// that matches the last card is the echo and is dropped. Anything else
    /// is a turn this window did not start: the card goes up, and the pane
    /// is working until the kernel says idle.
    fn foreign_prompt(
        &mut self,
        text: String,
        attachments: Vec<String>,
        ts: i64,
        seq: u64,
        channel: String,
    ) {
        self.new_turn_steps();
        self.turn_ended = None;
        let squash = |s: &str| s.split_whitespace().collect::<String>();
        // A line this window sent and is still waiting to see recorded:
        // its card is the newest with these words. However long the kernel
        // held it, this is the echo.
        if let Some(at) = self
            .awaiting_echo
            .iter()
            .position(|sent| *sent == squash(&text))
        {
            self.awaiting_echo.remove(at);
            let voice = if let Some(card) =
                self.items.iter_mut().rev().find_map(|item| match item {
                    ChatItem::User(message) if squash(&message.text) == squash(&text) => {
                        Some(message)
                    }
                    _ => None,
                }) {
                if ts > 0 {
                    card.sent_at = Some(ts);
                }
                if seq > 0 {
                    card.seq = Some(seq);
                }
                card.channel == "voice"
            } else {
                false
            };
            // A spoken card we already drew: the kernel's record is this
            // line. Small talk never reaches the kernel, so this path is
            // a voice-delegated turn. Open it so tool and agent rows are
            // not hidden behind a display-only spoken row.
            if voice {
                self.open_turn();
            } else {
                self.flush();
            }
            return;
        }
        let echo = self
            .items
            .iter_mut()
            .rev()
            .find_map(|item| match item {
                ChatItem::User(message) => Some(message),
                _ => None,
            })
            .filter(|last| {
                squash(&last.text) == squash(&text)
                    && last.sent_at.is_none_or(|sent| (ts - sent).abs() < 120_000)
            });
        if let Some(last) = echo {
            // The line this window typed, read back with the kernel's clock
            // on it: the date line and "Just now" want that stamp.
            if last.sent_at.is_none() && ts > 0 {
                last.sent_at = Some(ts);
            }
            if seq > 0 {
                last.seq = Some(seq);
            }
            self.flush();
            return;
        }
        let mut message = crate::model::attachment::UserMessage::from(text);
        message.seq = (seq > 0).then_some(seq);
        for path in &attachments {
            message.add_file_path(path);
        }
        message.sent_at = (ts > 0).then_some(ts);
        // Spoken, not typed: the kernel says so on the line (#99). A kernel
        // from before that wrote no channel; then a line this window did not
        // type while a call to this chat is live came through the microphone.
        message.channel = if !channel.is_empty() {
            channel
        } else if crate::voice_ws::in_call() {
            "voice".into()
        } else {
            String::new()
        };
        self.flight = Some(Flight {
            at: SystemTime::now(),
            used: self.usage.map_or(0, |usage| usage.used),
        });
        self.items.push(ChatItem::User(message));
        self.updated = SystemTime::now();
        self.streaming = true;
        self.take_title_from_first_prompt();
        self.flush();
    }

    /// The caller's spoken words, from the gateway's transcript, into the
    /// chat now as a `voice` user card. Nothing is sent from here: the
    /// gateway forwards the utterance to the agent as a voice message when
    /// it is one for the agent, and the kernel's record of that is this
    /// card's echo (`foreign_prompt` matches it by its words). An
    /// utterance the narrator or the speech model answered stays a card
    /// with no turn under it, which is the truth.
    pub fn voice_prompt(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let squashed: String = text.split_whitespace().collect();
        // The kernel's record may already be here (a slow poll).
        if self.items.iter().rev().take(4).any(|item| matches!(item, ChatItem::User(m) if m.text.split_whitespace().collect::<String>() == squashed)) {
            return;
        }
        self.awaiting_echo.push_back(squashed);
        while self.awaiting_echo.len() > 8 {
            self.awaiting_echo.pop_front();
        }
        let mut message = crate::model::attachment::UserMessage::from(text.to_string());
        message.channel = "voice".into();
        message.sent_at = Some(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
        );
        self.items.push(ChatItem::User(message));
        self.updated = SystemTime::now();
        self.flush();
    }

    /// A tool or agent frame from the call mirror, into this chat as a
    /// real row. Display only: nothing is sent to the kernel. Spoken-row
    /// filtering used to drop these; a voice-delegated turn must show
    /// them. A row the kernel already drew is skipped.
    pub fn apply_call_work(&mut self, kind: &str, agent: &str, text: &str) -> bool {
        match kind {
            "tool.call" => {
                let name = text.split_whitespace().next().unwrap_or("");
                let detail = text[name.len()..].trim();
                self.voice_tool_row(name, detail);
                true
            }
            "agent.event/tool" => {
                let name = text.split_whitespace().next().unwrap_or(text);
                let detail = text[name.len()..].trim();
                self.voice_tool_row(name, detail);
                true
            }
            "agent.event/say" | "agent.done" => {
                let (who, body) = call_work_speaker(agent, text);
                self.voice_from_row(&who, &body);
                true
            }
            "agent.turn" if text.trim() == "running" => {
                if !self.busy() {
                    self.open_turn();
                } else {
                    self.turn_alive();
                }
                true
            }
            "agent.turn" => true,
            _ => false,
        }
    }

    fn voice_tool_row(&mut self, name: &str, detail: &str) {
        let name = name.trim();
        if name.is_empty() || name == "delegate" {
            return;
        }
        let hint = detail
            .trim()
            .trim_matches(|c: char| c == '{' || c == '}')
            .trim();
        let hint = (!hint.is_empty()).then_some(hint);
        let label = crate::agent::acp::tool_title(name, hint);
        if self.items.iter().rev().take(16).any(|item| {
            matches!(
                item,
                ChatItem::Tool { label: held, .. }
                    if held == &label
                        || held.split_whitespace().next() == Some(name)
            )
        }) {
            return;
        }
        if !self.busy() {
            self.open_turn();
        } else {
            self.turn_alive();
        }
        let id = format!("voice-{name}-{}", self.items.len());
        self.tool_started.insert(id.clone(), Instant::now());
        let child = (name == "spawn")
            .then(|| hint.unwrap_or("").to_string())
            .filter(|s| !s.is_empty());
        self.items.push(ChatItem::Tool {
            id,
            kind: crate::agent::acp::tool_kind(name),
            label,
            status: ToolStatus::Running,
            output: String::new(),
            diff: None,
            child_session: child,
            secs: None,
            desc: hint.map(str::to_string),
        });
        self.updated = SystemTime::now();
        self.flush();
    }

    fn voice_from_row(&mut self, who: &str, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let who = if who.is_empty() || who == "root" {
            String::new()
        } else {
            who.to_string()
        };
        if self.items.iter().rev().take(8).any(|item| {
            matches!(
                item,
                ChatItem::From { who: held_who, text: held, .. }
                    if held.trim() == text && (who.is_empty() || held_who == &who)
            )
        }) {
            return;
        }
        if !self.busy() {
            self.open_turn();
        }
        self.items.push(ChatItem::From {
            who,
            text: text.to_string(),
            images: Vec::new(),
        });
        self.updated = SystemTime::now();
        self.flush();
    }

    /// The user is looking at this chat: every notification held for it is
    /// seen, here and on every other client.
    pub fn mark_seen(&mut self) {
        let Some(through) = self.unseen.iter().map(|n| n.id).max() else {
            return;
        };
        if let Connection::Live(session) = &self.connection {
            let _ = session.seen(through);
        }
        self.seen_through = self.seen_through.max(through);
        self.unseen.clear();
        self.to_notify.clear();
        self.flush();
    }

    /// How long the kernel has been silent while it owes this chat
    /// something — a reply to a prompt just sent, or the rest of a turn in
    /// flight — past the point where silence means the wire, not the model.
    /// `None` while nothing is owed or the silence is still ordinary.
    ///
    /// A first probe goes out after `QUIET_PROBE`; the kernel answers a
    /// `list` at once whatever the agent is doing, so silence past
    /// `QUIET_LOST` is a cut the socket has not noticed yet. Cleared by the
    /// next frame of any kind.
    pub fn not_answering(&self) -> Option<Duration> {
        let owed = self.busy() || self.pending_wire;
        if !owed {
            return None;
        }
        let Connection::Live(session) = &self.connection else {
            return None;
        };
        let quiet = self.last_frame_at.elapsed();
        if quiet >= QUIET_PROBE {
            let probed = self
                .probe_at
                .get()
                .is_some_and(|at| at > self.last_frame_at);
            if !probed {
                let _ = session.probe();
                self.probe_at.set(Some(Instant::now()));
            }
        }
        (quiet >= QUIET_LOST).then_some(quiet)
    }

    /// A turn boundary: the step numbers start again at 1, so the maps of
    /// the last turn's items go. Not at turn end — the settled records of
    /// the final step can arrive after `turn_complete`.
    fn new_turn_steps(&mut self) {
        self.step_items.clear();
        self.step_thoughts.clear();
        self.stream_raw.clear();
    }

    /// A streamed chunk lands on the reply item at `ix`: merged into the
    /// raw text, shown with tool-call markup cut.
    fn stream_into(&mut self, ix: usize, text: &str) {
        let busy = self.busy();
        let Some(ChatItem::Agent(body)) = self.items.get_mut(ix) else {
            return;
        };
        let raw = self.stream_raw.entry(ix).or_insert_with(|| body.clone());
        merge_stream_text(raw, text);
        let shown = crate::markup::strip_live(raw);
        // A `status` call streams as the words "status: <step>" before the
        // settled line takes them off the transcript. They are the live
        // step for as long as they stream — Jacob's "status: Delegating to
        // worker…" sat as a paragraph for the whole wait on the worker.
        if let Some(step) = status_line(&shown) {
            body.clear();
            if busy {
                self.status = Some(step);
            }
            return;
        }
        *body = shown;
    }

    /// Open a reply item for streamed text; `None` when the text shows
    /// nothing yet (markup only).
    fn open_stream(&mut self, text: String) -> Option<usize> {
        let mut shown = crate::markup::strip_live(&text);
        if shown.is_empty() && text.trim().is_empty() {
            return None;
        }
        if let Some(step) = status_line(&shown) {
            if self.busy() {
                self.status = Some(step);
            }
            shown.clear();
        }
        self.items.push(ChatItem::Agent(shown));
        let ix = self.items.len() - 1;
        self.stream_raw.insert(ix, text);
        Some(ix)
    }

    /// A step's settled line that says what the turn already said: the same
    /// paragraph as the reply's previous prose (a model that repeats itself
    /// across a tool call), or the words the agent already sent the user
    /// with `say`. Cursor shows a paragraph once; so does the window — the
    /// repeat goes (F-66, F-67). Exact match only; near-repeats are the
    /// kernel's to judge.
    fn dedupe_settled(&mut self, ix: usize) {
        let Some(ChatItem::Agent(text)) = self.items.get(ix) else {
            return;
        };
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        let mut same_say = None;
        let mut same_prose = false;
        for (at, item) in self.items[..ix].iter().enumerate().rev() {
            match item {
                ChatItem::User(_) => break,
                ChatItem::Agent(earlier) if earlier.trim() == text => {
                    same_prose = true;
                    break;
                }
                ChatItem::From {
                    who, text: said, ..
                } if said.trim() == text
                    && self
                        .agent_session
                        .as_deref()
                        .is_none_or(|own| who == own || who.is_empty()) =>
                {
                    same_say = Some(at);
                    break;
                }
                _ => {}
            }
        }
        if same_prose {
            self.drop_item(ix);
        } else if let Some(at) = same_say {
            self.drop_item(at);
        }
    }

    /// The settled line of a streamed reply is empty (a reply that was only
    /// markup, cut by the kernel): the item it opened goes, and every index
    /// held after it moves up.
    fn drop_item(&mut self, ix: usize) {
        self.items.remove(ix);
        self.stream_raw.remove(&ix);
        for at in self
            .step_items
            .values_mut()
            .chain(self.step_thoughts.values_mut())
        {
            if *at > ix {
                *at -= 1;
            }
        }
        self.step_items.retain(|_, at| *at != ix);
        self.step_thoughts.retain(|_, at| *at != ix);
        let moved: Vec<(usize, String)> = self
            .stream_raw
            .drain()
            .map(|(at, raw)| (if at > ix { at - 1 } else { at }, raw))
            .collect();
        self.stream_raw.extend(moved);
        if self.streaming_agent == Some(ix) {
            self.streaming_agent = None;
        } else if let Some(at) = self.streaming_agent.as_mut()
            && *at > ix
        {
            *at -= 1;
        }
    }

    /// Put the user card in the pane this frame — before the kernel
    /// answers, and before a reconnect finishes.
    fn land_turn(&mut self, content: &Prompt) {
        self.new_turn_steps();
        // A prompt of ours is a new turn: the last one's end no longer
        // holds back the tokens that follow (F-122b).
        self.turn_ended = None;
        let squashed: String = content.text.split_whitespace().collect();
        if !squashed.is_empty() {
            self.awaiting_echo.push_back(squashed);
            // A record that never comes back (a kernel from before the echo,
            // a line it dropped) must not match a later prompt with the
            // same words forever.
            while self.awaiting_echo.len() > 8 {
                self.awaiting_echo.pop_front();
            }
        }
        self.flight = Some(Flight {
            at: SystemTime::now(),
            used: self.usage.map_or(0, |usage| usage.used),
        });
        self.items.push(ChatItem::User(content.message()));
        self.updated = SystemTime::now();
        self.streaming = true;
        // A new turn's question is never a tail repeat of the last one.
        self.answered_ask = None;
        self.take_title_from_first_prompt();
        self.flush();
    }

    /// Cancel the in-flight turn. A pending permission request MUST be
    /// answered `Cancelled` per spec before `session/cancel` goes out.
    pub fn cancel(&mut self) {
        self.interrupt(true);
    }

    /// Stop the turn. `announce` is Stop in the transcript — Force
    /// interrupts without saying the turn was stopped, then sends.
    fn interrupt(&mut self, announce: bool) {
        if self.questions.is_some() {
            self.skip_ask();
        }
        let busy = self.streaming || self.has_running_tool();
        if let Connection::Live(session) = &self.connection
            && !session.is_closed()
            && session.cancel().is_ok()
        {
            self.stop_requested = true;
            return;
        }
        self.reap_dead_socket();
        self.streaming = false;
        self.flight = None;
        self.fail_running_tools();
        if announce && busy {
            self.notice(false, "Stopped.");
        }
        self.flush();
    }

    /// No-op on the wire. The kernel has no ACP session modes.
    pub fn set_mode(&mut self, mode_id: &str) {
        let Connection::Live(session) = &self.connection else {
            return;
        };
        session.set_mode(mode_id);
        if let Some(modes) = &mut self.modes {
            modes.current_mode_id = mode_id.into();
        }
    }

    /// Switch the model for later turns (`set_model` on the kernel seam).
    pub fn set_model(&mut self, model: &str) {
        // Remembered whatever the connection state: a model picked before
        // the first message is applied when the session attaches (see the
        // go-live block in `connect`), not dropped.
        self.model = Some(model.to_string());
        if let Connection::Live(session) = &self.connection {
            session.set_model(model);
        }
    }

    /// Give the kernel this window's model key: the key belongs to the
    /// user, not the machine. `remember` writes it into the kernel's
    /// config; otherwise it lives in that kernel's memory only.
    pub fn offer_key(&mut self, remember: bool) {
        let Connection::Live(session) = &self.connection else {
            self.notice(true, "no live kernel connection to give the key to");
            return;
        };
        let Ok(host) = arbos_core::Host::load() else {
            self.notice(true, "this window has no model config of its own");
            return;
        };
        let Some(key) = host.api_key() else {
            self.notice(
                true,
                "this window has no model key of its own to give (Settings › Model)",
            );
            return;
        };
        let base = host.config.api_base().unwrap_or_default();
        session.configure(
            host.config.provider().as_str(),
            &base,
            &host.config.model(),
            &key,
            remember,
        );
    }

    /// Summarise the oldest turns now.
    pub fn compact(&mut self) {
        if let Connection::Live(session) = &self.connection {
            self.compact_asked = Some(Instant::now());
            session.compact();
        }
    }

    /// "Rewind here" on the turn whose prompt is item `ix`: the kernel cuts
    /// its transcript at that turn's start and, with `files`, puts the
    /// project back; the pane follows when `rewound` arrives. The prompt
    /// goes back into the composer so it can be sent again, changed.
    pub fn rewind(&mut self, ix: usize, files: bool) {
        let Connection::Live(session) = &self.connection else {
            self.notice(true, "rewind needs a live kernel connection");
            return;
        };
        // The chat's own turn, not its workers: a spawned worker's tool row
        // reads as running until the worker reports, and `busy()` counts it
        // — so an idle chat over a long-running worker refused every rewind
        // ("stop the turn before rewinding", gate cycle-22b). The kernel
        // checks the agent itself and refuses if it is running.
        if self.streaming || self.turn_open {
            // Which flag held, and how long since anything arrived: F-122
            // recurred once on cycle 28 after passing three times, and the
            // notice alone cannot say why (rig, cycle 29).
            eprintln!(
                "arbos: rewind refused: streaming={} turn_open={} working={} last_frame={}s ago progress={}s ago turn_ended={:?}",
                self.streaming,
                self.turn_open,
                self.working.is_some(),
                self.last_frame_at.elapsed().as_secs(),
                self.progress_at.elapsed().as_secs(),
                self.turn_ended.map(|at| at.elapsed().as_secs()),
            );
            self.notice(true, "stop the turn before rewinding");
            return;
        }
        // The footer may sit under a `From` block inside the turn; the
        // turn starts at the last user prompt at or before it.
        let Some(start) = self
            .items
            .iter()
            .take(ix + 1)
            .rposition(|item| matches!(item, ChatItem::User(_)))
        else {
            return;
        };
        let turn = self
            .items
            .iter()
            .take(start + 1)
            .filter(|item| matches!(item, ChatItem::User(_)))
            .count() as u32;
        // The prompt's own transcript line names the turn exactly; the
        // count of cards is the fallback for a card from before seqs were
        // kept. The two differ whenever the transcript holds `user` lines
        // the window folds — an ask's answer, a skipped question, a fork's
        // copied history — and the count then rewinds the wrong turn or
        // none (F-103).
        let line = match &self.items[start] {
            ChatItem::User(message) => message.seq,
            _ => None,
        };
        self.rewind_to = Some(start);
        session.rewind(turn, line, files);
    }

    /// Pause or resume the agent.
    pub fn set_paused(&mut self, paused: bool) {
        if let Connection::Live(session) = &self.connection {
            session.set_paused(paused);
        }
    }

    /// Restore the git checkpoint for this turn.
    pub fn undo_checkpoint(&mut self) {
        if let Connection::Live(session) = &self.connection {
            session.undo_checkpoint();
        }
    }

    /// Work map: tools in this transcript, as a view of the log.
    pub fn work_map(&self) -> String {
        let mut out = String::new();
        for item in &self.items {
            if let ChatItem::Tool {
                label,
                status,
                output,
                ..
            } = item
            {
                out.push_str(&format!(
                    "- {label} {:?}\n",
                    match status {
                        ToolStatus::Running => "run",
                        ToolStatus::Success => "ok",
                        ToolStatus::Failure => "err",
                    }
                ));
                if !output.is_empty() {
                    out.push_str(&format!("  {}\n", output.lines().next().unwrap_or("")));
                }
            }
        }
        if out.is_empty() {
            "(no tools yet)\n".into()
        } else {
            out
        }
    }

    /// No-op on the wire. The kernel has no ACP config options; the model
    /// is [`Self::set_model`].
    pub fn set_config(&mut self, config_id: &str, value: SessionConfigOptionValue) {
        let Connection::Live(session) = &self.connection else {
            return;
        };
        session.set_config_option(config_id, value.clone());
        let found = self
            .config
            .iter_mut()
            .find(|option| &*option.id == config_id);
        let Some(option) = found else {
            return;
        };
        match (&mut option.kind, value) {
            (SessionConfigKind::Select(select), SessionConfigOptionValue::ValueId { value }) => {
                select.current_value = value;
            }
            (SessionConfigKind::Boolean(flag), SessionConfigOptionValue::Boolean { value }) => {
                flag.current_value = value;
            }
            // The agent offers one shape and was asked for the other. Its own
            // update is the answer; nothing is guessed at here.
            _ => {}
        }
    }

    /// An ask that is really an approval — the kernel in `ask` mode puts
    /// "allow <call>" to the user with allow / deny options. Cursor draws
    /// that as its approval row, not a question. Returns the call's words
    /// and the two option ids.
    pub fn approval_ask(&self) -> Option<(String, String, String)> {
        let prompt = self.questions.as_ref()?;
        if prompt.questions.len() != 1 {
            return None;
        }
        let question = prompt.current()?;
        let words = if question.prompt.trim().is_empty() {
            prompt.title.as_str()
        } else {
            question.prompt.as_str()
        };
        let call = words
            .trim()
            .strip_prefix("allow ")
            .or_else(|| words.trim().strip_prefix("Allow "))?;
        let find = |want: &str| {
            question
                .options
                .iter()
                .find(|o| {
                    o.id.eq_ignore_ascii_case(want) || o.label.trim().eq_ignore_ascii_case(want)
                })
                .map(|o| o.id.clone())
        };
        Some((call.trim().to_string(), find("allow")?, find("deny")?))
    }

    /// Answer an approval-shaped ask: pick allow or deny and send it.
    pub fn answer_approval(&mut self, allow: bool) {
        let Some((_, allow_id, deny_id)) = self.approval_ask() else {
            return;
        };
        let Some(question_id) = self
            .questions
            .as_ref()
            .and_then(|p| p.current())
            .map(|q| q.id.clone())
        else {
            return;
        };
        let pick = if allow { allow_id } else { deny_id };
        if let Some(prompt) = self.questions.as_mut() {
            let draft = prompt.drafts.entry(question_id).or_default();
            draft.selected = vec![pick];
            draft.other = false;
        }
        self.answer_ask("", false);
    }

    pub fn toggle_ask_option(&mut self, question_id: &str, option_id: &str) {
        let Some(prompt) = self.questions.as_mut() else {
            return;
        };
        let Some(question) = prompt.questions.iter().find(|q| q.id == question_id) else {
            return;
        };
        let allow_multiple = question.allow_multiple;
        let draft = prompt.drafts.entry(question_id.to_owned()).or_default();
        if draft.selected.iter().any(|id| id == option_id) {
            draft.selected.retain(|id| id != option_id);
        } else if allow_multiple {
            draft.selected.push(option_id.to_owned());
        } else {
            draft.selected = vec![option_id.to_owned()];
            draft.other = false;
        }
    }

    pub fn toggle_ask_other(&mut self, question_id: &str) {
        let Some(prompt) = self.questions.as_mut() else {
            return;
        };
        let allow_multiple = prompt
            .questions
            .iter()
            .find(|q| q.id == question_id)
            .is_some_and(|q| q.allow_multiple);
        let draft = prompt.drafts.entry(question_id.to_owned()).or_default();
        draft.other = !draft.other;
        if draft.other && !allow_multiple {
            draft.selected.clear();
        }
    }

    pub fn turn_ask_page(&mut self, delta: isize, held: &str) {
        let Some(prompt) = self.questions.as_mut() else {
            return;
        };
        if prompt.questions.is_empty() {
            return;
        }
        prompt.save_other(held);
        let n = prompt.questions.len() as isize;
        prompt.page = ((prompt.page as isize + delta).rem_euclid(n)) as usize;
    }

    /// Continue or skip the ask form. `held` is the composer line: Other
    /// text on the current page, or optional details.
    pub fn answer_ask(&mut self, held: &str, skipped: bool) {
        let Some(prompt) = self.questions.take() else {
            return;
        };
        self.answered_ask = Some((prompt.title.clone(), Instant::now()));
        // The kernel's "Waiting for your answer" line was true until now;
        // the folded question and the answer under it say the rest.
        self.items.retain(|item| {
            !matches!(item, ChatItem::Notice { text, failed: false } if is_waiting_line(text))
        });
        let (answers, details) = if skipped {
            (Vec::new(), String::new())
        } else {
            prompt.build_answers(held)
        };
        // Words typed with no option picked are the answer in the user's
        // own terms, never a skip: a skip is the Skip button and nothing
        // else, so what was typed is not lost.
        let answered = answers
            .iter()
            .any(|answer| !answer.selected_ids.is_empty() || !answer.other_text.is_empty())
            || !details.trim().is_empty();
        let (answers, details, skipped) = if skipped || !answered {
            (Vec::new(), String::new(), true)
        } else {
            (answers, details, false)
        };
        match &self.connection {
            Connection::Live(session) => {
                if let Err(e) =
                    session.answer_questions(&prompt.request_id, &answers, &details, skipped)
                {
                    self.notice(true, &format!("could not answer: {e:#}"));
                    self.flush();
                    return;
                }
                // The answer is a line of yours in the conversation, under
                // the question it answers.
                let said = if !details.is_empty() {
                    details
                } else {
                    answers
                        .iter()
                        .flat_map(|a| a.selected_ids.iter().chain(Some(&a.other_text)))
                        .filter(|s| !s.is_empty())
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                // The card folds to one line that carries the answer, as
                // Cursor's does: no second bubble of yours under it.
                let question = prompt
                    .questions
                    .first()
                    .map(|q| q.prompt.clone())
                    .filter(|q| !q.trim().is_empty())
                    .unwrap_or_else(|| prompt.title.clone());
                self.items.push(ChatItem::Asked {
                    question,
                    answer: said.clone(),
                });
                self.updated = SystemTime::now();
                self.flush();
            }
            _ => self.notice(true, "the agent asked a question; not connected"),
        }
    }

    pub fn skip_ask(&mut self) {
        self.answer_ask("", true);
    }

    /// Drop the `ix`th waiting prompt — a steer taken back before the turn in
    /// flight got to it. `ix` counts as [`Self::waiting`] does.
    /// Take a queued follow-up back into the composer for editing (ui-006).
    pub fn edit_queued(&mut self, ix: usize) {
        let ix = ix + self.wired_head();
        if ix < self.queue.len() {
            let prompt = self.queue.remove(ix).unwrap_or_default();
            self.draft = prompt.text;
            self.draft_pushed = true;
        }
    }

    pub fn unqueue(&mut self, ix: usize) {
        let ix = ix + self.wired_head();
        if ix < self.queue.len() {
            self.queue.remove(ix);
        }
    }

    /// Prompts the strip should show as waiting. A prompt sent before the
    /// connection was up already sits in the transcript as the turn's
    /// message (`land_turn`) and only waits on the wire; showing it in the
    /// strip too made one message read as both queued and sent.
    pub fn waiting(&self) -> impl Iterator<Item = &Prompt> {
        self.queue.iter().skip(self.wired_head())
    }

    fn wired_head(&self) -> usize {
        usize::from(self.pending_wire && !self.queue.is_empty())
    }

    pub fn set_live(&mut self, live: Vec<crate::kernel::LiveWork>) -> bool {
        if self.live == live {
            return false;
        }
        if live.is_empty() {
            self.live.clear();
            self.live_since = None;
        } else {
            if self.live_since.is_none() {
                self.live_since = Some(SystemTime::now());
            }
            self.live = live;
        }
        true
    }

    fn apply(&mut self, event: Event) {
        self.updated = SystemTime::now();
        self.last_frame_at = Instant::now();
        // One transcript line's events, bracketed: held when the pane has
        // that line already, whatever kind it is.
        match event {
            Event::RecordLine(seq) => {
                self.holding_line = self.record_held(seq);
                return;
            }
            Event::RecordLineEnd => {
                self.holding_line = false;
                return;
            }
            _ if self.holding_line => return,
            _ => {}
        }
        // Progress the person could see. Not `Alive`, not a probe's answer,
        // not the roster, provider or store bookkeeping the kernel sends
        // between real frames — those would keep the stall hint away from
        // a turn that is truly stuck.
        if matches!(
            event,
            Event::TextDelta { .. }
                | Event::ThoughtDelta { .. }
                | Event::ThoughtFinal { .. }
                | Event::AssistantFinal { .. }
                | Event::Update(_)
                | Event::Status(_)
                | Event::Waiting(_)
                | Event::Incoming { .. }
                | Event::UserLine { .. }
                | Event::Woke { .. }
                | Event::Aside(_)
                | Event::Refused(_)
                | Event::Plan(_)
                | Event::NeedQuestion { .. }
                | Event::TurnDone(_)
        ) {
            self.progress_at = Instant::now();
        }
        match event {
            Event::Alive => {}
            Event::Artifacts(files) => {
                if !files.is_empty() {
                    self.items.push(ChatItem::Artifacts(files));
                    self.flush();
                }
            }
            Event::Update(update) => self.apply_update(update),
            Event::Incoming { who, text } => {
                self.items.push(ChatItem::From {
                    who,
                    text,
                    images: Vec::new(),
                });
                self.flush();
            }
            Event::UserLine {
                text,
                attachments,
                ts,
                seq,
                channel,
            } => self.foreign_prompt(text, attachments, ts, seq, channel),
            Event::Provider {
                provider,
                model,
                key,
                source,
            } => {
                self.kernel_provider = Some(KernelProvider {
                    provider: provider.clone(),
                    model,
                    key,
                    source: source.clone(),
                });
                let was = self.provider_missing.take();
                if key {
                    if was.is_some() {
                        self.notice(
                            false,
                            &format!("{provider} key in place on this kernel ({source})"),
                        );
                        self.flush();
                    }
                    self.send_kickoff_if_ready();
                } else {
                    self.provider_missing = Some(provider);
                }
            }
            Event::Rewound {
                dropped,
                restored,
                pending,
            } => {
                // The record is shorter now and numbers on from where it
                // was cut: the lines to come are new however low they sit.
                self.record_seq = 0;
                // The second frame of a rewind with files: the restore is
                // done. The chat was already cut on the first; only the
                // line changes.
                if let Some(r) = restored.as_deref()
                    && matches!(self.items.last(), Some(ChatItem::Notice { text, .. }) if text.starts_with("rewound:") && text.ends_with(RESTORING))
                {
                    self.items.pop();
                    let _ = r;
                    self.notice(false, &rewound_line(dropped, true));
                    self.flush();
                    return;
                }
                match self.rewind_to.take() {
                    Some(ix) if ix < self.items.len() => {
                        if let ChatItem::User(message) = &self.items[ix] {
                            self.draft = message.text.clone();
                            self.draft_pushed = true;
                        }
                        self.items.truncate(ix);
                    }
                    // Another window asked: take the kernel's transcript as
                    // it now is.
                    _ => {
                        if let Some(sid) = &self.agent_session
                            && let Some(replay) = crate::kernel::session_history(&self.place(), sid)
                        {
                            self.items = replay.items;
                        }
                    }
                }
                self.transcript = transcript::State::default();
                self.streaming_agent = None;
                self.questions = None;
                let what = match restored {
                    Some(_) => rewound_line(dropped, true),
                    None if pending => {
                        format!("rewound: {dropped} transcript lines cut; {RESTORING}")
                    }
                    None => rewound_line(dropped, false),
                };
                self.notice(false, &what);
                self.flush();
            }
            Event::RecordEnd(to) => self.hold_record_through(to),
            // Taken above, before anything else reads the event.
            Event::RecordLine(_) | Event::RecordLineEnd => {}
            Event::Handshake { protocol, build } => {
                let ok = protocol.is_some_and(|p| p >= crate::kernel::PROTOCOL);
                let kernel = build.version.clone();
                // Everything on the record at this moment is history: the
                // pane holds it (its cards, and `adopt_kernel_tail` for what
                // was written while it was away). A kernel whose tail
                // cursors start at line one broadcasts the whole file as
                // live frames to a window that attaches during its first
                // tick, and a record from before cards kept their `seq`
                // took every line as news — 167 cards became 266 on one
                // launch (F-180, cycle 39).
                if self.host.is_none()
                    && let Some(sid) = self.agent_session.as_deref()
                {
                    let lines = crate::kernel::transcript_lines(&self.place(), sid);
                    self.hold_record_through(lines);
                }
                // Which kernel is on the other end of this socket, in its own
                // words. Kept only while the socket is: `forget_socket` drops
                // it, so the field cannot outlive the connection it describes.
                self.kernel_build = ok.then_some(build);
                // A new project opens on an empty chat, the same as `clear`.
                // The kickoff splash is no longer asked for: the first turn
                // is the person's, not a setup greeting.
                if !ok {
                    let where_ = match &self.host {
                        Some(h) => format!("on {h}"),
                        None => "for this folder".into(),
                    };
                    let what = match protocol {
                        Some(p) => format!(
                            "speaks attach protocol {p}; this window needs {}",
                            crate::kernel::PROTOCOL
                        ),
                        None => "predates the attach handshake".into(),
                    };
                    let fix = if self.host.is_some() {
                        "Stop it there and reopen this place: the window puts its own arbos-kernel binary on the machine when none is running."
                    } else {
                        "Stop it and reopen this folder so the window starts its own."
                    };
                    self.notice(
                        true,
                        &format!(
                            "The arbos-kernel {where_}{} {what}. {fix}",
                            if kernel.is_empty() {
                                String::new()
                            } else {
                                format!(" ({kernel})")
                            }
                        ),
                    );
                    self.close();
                    self.flush();
                }
            }
            Event::Woke { kind, text, at } => {
                self.new_turn_steps();
                self.turn_ended = None;
                // The kernel writes `wake` then `user` for a prompt: the
                // user line is that turn's boundary. Every other kind opens
                // a segment of its own (F-62).
                if kind != "user" && kind != "kickoff" && kind != "compact" {
                    let dup = matches!(self.items.last(), Some(ChatItem::Wake { at: Some(prev), .. }) if Some(*prev) == at);
                    if !dup {
                        self.items.push(ChatItem::Wake {
                            kind,
                            text,
                            at,
                            secs: None,
                        });
                        // The kernel writes the worker's report (`say`) and
                        // then the wake it caused. Cursor's segment opens
                        // with its "Worked" header and the report under it:
                        // the wake goes first, the report inside.
                        let n = self.items.len();
                        if n >= 2 && matches!(self.items[n - 2], ChatItem::From { .. }) {
                            self.items.swap(n - 2, n - 1);
                        }
                        // The segment's clock: "Working" counts from here
                        // and "Worked Ns" is stamped from it at the end.
                        self.flight = Some(Flight {
                            at: SystemTime::now(),
                            used: self.usage.map_or(0, |usage| usage.used),
                        });
                        self.flush();
                    }
                }
            }
            Event::TextDelta { text, step } => {
                self.turn_alive();
                self.finish_thinking();
                let at = self
                    .step_items
                    .get(&step)
                    .copied()
                    .filter(|ix| matches!(self.items.get(*ix), Some(ChatItem::Agent(_))));
                match at {
                    Some(ix) => self.stream_into(ix, &text),
                    None => {
                        if let Some(ix) = self.open_stream(text) {
                            self.step_items.insert(step, ix);
                            self.streaming_agent = Some(ix);
                        }
                    }
                }
                self.updated = SystemTime::now();
            }
            Event::ThoughtDelta { text, step } => {
                self.turn_alive();
                let at = self.step_thoughts.get(&step).copied();
                match at.and_then(|ix| self.items.get_mut(ix)) {
                    Some(ChatItem::Thinking { text: body, .. }) => merge_stream_text(body, &text),
                    _ => {
                        if !text.is_empty() {
                            self.thought_at = Some(SystemTime::now());
                            self.items.push(ChatItem::Thinking {
                                text,
                                done: false,
                                secs: None,
                            });
                            self.step_thoughts.insert(step, self.items.len() - 1);
                        }
                    }
                }
                self.updated = SystemTime::now();
            }
            Event::ThoughtFinal { text, step, secs } => {
                // The settled thought of a step: its words whole and its
                // seconds, on the item its deltas opened; a new item only
                // when none streamed (attached late).
                let at = self.step_thoughts.get(&step).copied();
                match at.and_then(|ix| self.items.get_mut(ix)) {
                    Some(ChatItem::Thinking {
                        text: body,
                        done,
                        secs: held,
                    }) => {
                        if !text.trim().is_empty() {
                            *body = text;
                        }
                        *done = true;
                        *held = secs;
                    }
                    _ => {
                        // No item by step (a record from before the maps, a
                        // settled line arriving after the turn's end): the
                        // last thought of this turn with the same words takes
                        // the stamp; a new item only when there is none.
                        let same = self
                            .items
                            .iter()
                            .rposition(|item| matches!(item, ChatItem::Thinking { text: t, .. } if same_words(t, &text)));
                        let boundary = self
                            .items
                            .iter()
                            .rposition(|item| matches!(item, ChatItem::User(_)))
                            .unwrap_or(0);
                        match same
                            .filter(|ix| *ix >= boundary)
                            .and_then(|ix| self.items.get_mut(ix))
                        {
                            Some(ChatItem::Thinking {
                                done, secs: held, ..
                            }) => {
                                *done = true;
                                *held = secs;
                            }
                            _ => {
                                if !text.trim().is_empty() {
                                    self.items.push(ChatItem::Thinking {
                                        text,
                                        done: true,
                                        secs,
                                    });
                                    self.step_thoughts.insert(step, self.items.len() - 1);
                                }
                            }
                        }
                    }
                }
                self.flush();
            }
            Event::AssistantFinal { text, step } => {
                self.finish_thinking();
                // The kernel cuts tool markup from the settled line (#278);
                // the same cut here covers a kernel from before it.
                let text = crate::markup::strip_live(text.trim_matches('\n'));
                // The settled line of a numbered step replaces the item its
                // deltas built, wherever it sits; nothing is doubled and a
                // late final goes to its own step, not the last one.
                if step > 0
                    && let Some(ix) = self.step_items.get(&step).copied()
                    && let Some(ChatItem::Agent(body)) = self.items.get_mut(ix)
                {
                    if let Some(step_text) = status_line(&text) {
                        self.items.remove(ix);
                        self.step_items.remove(&step);
                        for at in self
                            .step_items
                            .values_mut()
                            .chain(self.step_thoughts.values_mut())
                        {
                            if *at > ix {
                                *at -= 1;
                            }
                        }
                        if self.streaming_agent == Some(ix) {
                            self.streaming_agent = None;
                        }
                        if self.busy() {
                            self.status = Some(step_text);
                        }
                        self.status_only = true;
                        self.flush();
                        return;
                    }
                    if text.is_empty() {
                        // The kernel cut the whole reply (tool markup written
                        // as prose): nothing to keep, no empty bubble.
                        self.drop_item(ix);
                        self.flush();
                        return;
                    }
                    *body = text;
                    self.stream_raw.remove(&ix);
                    if self.streaming_agent == Some(ix) {
                        self.streaming_agent = None;
                    }
                    self.dedupe_settled(ix);
                    self.flush();
                    return;
                }
                // The kernel records a `status` call as an assistant line
                // "status: <step>". It is the live step, shown in the worker
                // line and the panel, not a paragraph of the reply.
                if let Some(step) = status_line(&text) {
                    if let Some(ix) = self.streaming_agent.take()
                        && matches!(self.items.get(ix), Some(ChatItem::Agent(body)) if status_line(body).is_some())
                    {
                        self.items.remove(ix);
                    }
                    if self.busy() {
                        self.status = Some(step);
                    }
                    self.status_only = true;
                    self.flush();
                    return;
                }
                // The deltas of this step built an item: the recorded line is
                // the same words, whole. Replace, never append; an empty line
                // (a reply the kernel cut whole) takes the item with it.
                if let Some(ix) = self.streaming_agent.take() {
                    if let Some(ChatItem::Agent(body)) = self.items.get_mut(ix) {
                        if text.is_empty() {
                            self.drop_item(ix);
                        } else {
                            *body = text;
                            self.stream_raw.remove(&ix);
                        }
                        self.flush();
                        return;
                    }
                }
                // No deltas seen (an older kernel, a replay): the line stands
                // on its own, once.
                if text.is_empty() {
                    return;
                }
                let dup = matches!(self.items.last(), Some(ChatItem::Agent(body)) if body.trim() == text.trim());
                if !dup {
                    self.items.push(ChatItem::Agent(text));
                }
                self.flush();
            }
            Event::Aside(text) => {
                // The kernel says the repeated line was already queued
                // (#362): once under the bubble is the answer; the third
                // and fourth "run it" do not stack notices either.
                let repeat = is_already_queued(&text)
                    && matches!(self.items.last(), Some(ChatItem::Notice { text: last, .. }) if *last == text);
                if !repeat {
                    self.notice(false, &text);
                }
                self.flush();
            }
            Event::Nudge(text) => {
                // Once per idle period from the kernel; the same line twice
                // in a row (a tail replay) is not two rows.
                let dup = matches!(self.items.last(), Some(ChatItem::Nudge(t)) if *t == text);
                if !dup {
                    self.items.push(ChatItem::Nudge(text));
                }
                self.flush();
            }
            Event::ImageDescribed { path, model, text } => {
                // Inside the card that carried the image: the last user
                // message, which is this turn's. Never a transcript line.
                if let Some(ChatItem::User(message)) = self
                    .items
                    .iter_mut()
                    .rev()
                    .find(|item| matches!(item, ChatItem::User(_)))
                    && !message.described.iter().any(|d| d.path == path)
                {
                    message.described.push(DescribedImage { path, model, text });
                }
                self.flush();
            }
            Event::Notify {
                id,
                ts,
                kind,
                title,
                body,
                replayed,
            } => {
                if id <= self.seen_through || self.unseen.iter().any(|n| n.id == id) {
                    return;
                }
                let note = Notification {
                    id,
                    ts,
                    kind,
                    title,
                    body,
                };
                if !replayed {
                    self.to_notify.push(note.clone());
                }
                self.unseen.push(note);
                self.flush();
            }
            Event::Seen(through) => {
                self.seen_through = self.seen_through.max(through);
                self.unseen.retain(|n| n.id > through);
                self.to_notify.retain(|n| n.id > through);
                self.flush();
            }
            Event::Refused(detail) => {
                self.rewind_to = None;
                // A keyless kernel (#312) answers `kickoff` with this and no
                // turn: the setup bar under the composer is the cue, and
                // the kickoff is over — nothing waits behind it.
                if detail.starts_with(KICKOFF_NOT_STARTED) {
                    if self.kickoff_at.is_some() && self.kickoff_secs.is_none() {
                        self.kickoff_secs = Some(0);
                    }
                    self.flight = None;
                    self.streaming = false;
                    self.turn_open = false;
                    self.flush();
                    self.drain();
                    return;
                }
                // A keyless kernel kept the typed line in its inbox instead
                // of spending a turn on it (#312): no turn is coming, so the
                // card must not sit under a shimmer. The line says what is
                // missing and what became of the words (the kernel writes
                // the same words on the transcript; identical notices read
                // once); the pending row under the composer shows them
                // waiting, and the setup bar says where a key goes.
                if detail.contains(LINE_KEPT_FOR_KEY) {
                    self.flight = None;
                    self.streaming = false;
                    self.turn_open = false;
                    self.notice(true, &detail);
                    self.flush();
                    return;
                }
                if self.plan_op_gone(&detail) {
                    return;
                }
                self.notice(true, &detail);
                self.flush();
                // The kernel no longer has this agent: the row keeps its
                // words and stops asking for a socket.
                if detail.starts_with("no agent ") {
                    self.close();
                }
            }
            Event::NeedQuestion {
                request_id,
                title,
                questions,
            } => {
                self.turn_alive();
                // The transcript tail repeats a question the user already
                // answered or skipped a moment ago: not a new card (ui-004).
                if self
                    .answered_ask
                    .as_ref()
                    .is_some_and(|(t, at)| *t == title && at.elapsed() < Duration::from_secs(10))
                {
                    return;
                }
                if let Some(previous) = self.questions.take() {
                    // The same ask reaches the window twice: once as the live
                    // `ask` frame, again as the transcript line the tail
                    // reads. Skipping the "previous" one answered the kernel
                    // with "" before the user saw the card. A repeat of the
                    // open question is the same question: keep it.
                    if same_question(&previous, &title, &questions) {
                        self.questions = Some(previous);
                        return;
                    }
                    if let Connection::Live(session) = &self.connection {
                        let _ = session.skip_question(&previous.request_id);
                    }
                }
                self.questions = Some(AskPrompt {
                    request_id,
                    title,
                    questions,
                    page: 0,
                    drafts: HashMap::new(),
                });
            }
            Event::Working(secs) => {
                self.working = Some((secs, Instant::now()));
                // A heartbeat straggling in after the turn's own end must
                // not reopen it: the chat then looked idle everywhere but
                // refused "Rewind here" with "stop the turn before
                // rewinding" and drew no footer (F-122, cycle 24 gate).
                // Same lag rule as `turn_alive`.
                if !self.turn_ended.is_some_and(|at| at.elapsed() < TAIL_LAG) {
                    self.turn_open = true;
                    self.turn_ended = None;
                }
            }
            Event::Status(text) => {
                let text = text.trim().to_string();
                self.status_over_workers = self.waiting.is_some()
                    || self
                        .children
                        .iter()
                        .any(|child| matches!(child.state, ChildState::Working));
                self.status = (!text.is_empty() && !is_router_step(&text)).then_some(text);
            }
            Event::Waiting(line) => {
                // The kernel clears its waiting line the moment no worker
                // is live: a status the agent set over its workers has
                // lost its subject and goes with it. The kernel's own
                // derived steps (a checkpoint being saved) arrive as
                // status too and are told apart by nothing here — they
                // never outlive what they describe on the kernel's side.
                if line.is_none() && self.waiting.is_some() && self.status_over_workers {
                    self.status = None;
                    self.status_over_workers = false;
                }
                self.waiting = line;
            }
            Event::TurnEndedAt(ended) => {
                // The turn read back is the newest opener's: a prompt, or a
                // wake — a worker's whole first turn opens on its `plan`
                // wake and has no `user` line at all, so a replayed worker
                // tab read "Edited 2 files, …" where Cursor's says "Worked
                // for 46s" (F-131, cycle 32).
                let opener = self
                    .items
                    .iter_mut()
                    .rev()
                    .find(|item| matches!(item, ChatItem::User(_) | ChatItem::Wake { .. }));
                let stamp = |began: i64| ((ended - began) / 1000).min(u32::MAX as i64) as u32;
                match opener {
                    Some(ChatItem::User(message)) => {
                        if message.worked_secs.is_none()
                            && let Some(sent) = message.sent_at
                            && ended > sent
                        {
                            message.worked_secs = Some(stamp(sent));
                        }
                    }
                    Some(ChatItem::Wake { at, secs, .. }) => {
                        if secs.is_none()
                            && let Some(began) = *at
                            && ended > began
                        {
                            *secs = Some(stamp(began));
                        }
                    }
                    _ => {}
                }
            }
            Event::TurnDone(result) => {
                self.working = None;
                self.status = None;
                self.stamp_worked();
                self.voice_answer();
                // A question parks the turn: the kernel ends it and waits
                // for the answer as a file. The card stays; answering
                // starts the next turn. A stopped or failed turn drops it.
                if !matches!(result, Ok(StopReason::EndTurn)) {
                    self.questions = None;
                }
                self.finish_thinking();
                self.streaming = false;
                self.turn_open = false;
                self.turn_ended = Some(Instant::now());
                let stopped = std::mem::take(&mut self.stop_requested);
                let status_only = std::mem::take(&mut self.status_only);
                match result {
                    Ok(StopReason::EndTurn) => {
                        // Cursor just ends. A lone "done" line is extra.
                        // Only speak when the kernel never answered at all —
                        // and not when this window asked it to stop: the
                        // kernel's own `interrupted` line ("Stopped by you")
                        // follows on the tail.
                        if !stopped && !status_only && !self.busy() && ended_on_user(&self.items) {
                            // Kernel turn failed before any token (missing
                            // agent.md, bad model, no key). Idle used to
                            // clear the thinking row and leave a blank pane.
                            self.notice(true, NO_REPLY);
                        }
                    }
                    // The kernel spells a user stop `cancelled` or, since the
                    // stop word landed, `stop`. Its own "Stopped by you" line
                    // follows on the tail, so this window adds nothing when it
                    // asked for the stop.
                    Ok(StopReason::Cancelled) | Ok(StopReason::Other(_)) if stopped => {
                        self.fail_running_tools();
                    }
                    Ok(StopReason::Cancelled) => {
                        self.fail_running_tools();
                        self.notice(false, "Stopped.");
                    }
                    Ok(StopReason::Other(reason)) if reason == "stop" => {
                        self.fail_running_tools();
                        self.notice(false, "Stopped.");
                    }
                    Ok(StopReason::Refusal) => self.notice(false, "the agent refused to continue"),
                    Ok(StopReason::MaxTokens) => self.notice(false, "stopped: max tokens"),
                    Ok(StopReason::MaxTurnRequests) => self.notice(false, "stopped: max steps"),
                    Ok(StopReason::Other(reason)) => {
                        self.notice(false, &format!("stopped: {reason}"))
                    }
                    Err(e) => {
                        self.fail_running_tools();
                        self.notice(true, &format!("turn failed: {}", acp::error_text(&e)));
                    }
                }
                self.flush();
                self.drain();
            }
            Event::Closed => {
                self.questions = None;
                let busy = self.streaming || self.has_running_tool();
                let queued = !self.queue.is_empty();
                self.forget_socket();
                if busy && !queued {
                    self.notice(false, "Stopped.");
                }
                self.flush();
            }
            Event::Plan(nodes) => self.set_plan(nodes),
            Event::Show { .. }
            | Event::ChildSession { .. }
            | Event::Open { .. }
            | Event::Hide { .. }
            | Event::Browser { .. }
            | Event::Pty { .. }
            | Event::Job { .. }
            | Event::StoreChanged(_) => {}
            Event::Screen {
                machine,
                png,
                mime,
                width,
                height,
                error,
            } => {
                let format = if mime == "image/jpeg" {
                    bezel::gpui::ImageFormat::Jpeg
                } else {
                    bezel::gpui::ImageFormat::Png
                };
                let image = (!png.is_empty())
                    .then(|| Arc::new(bezel::gpui::Image::from_bytes(format, png)));
                self.live_screen = Some(LiveScreen {
                    image,
                    machine,
                    at: Instant::now(),
                    width,
                    height,
                    error,
                });
            }
            // Held for the review sheet to collect. Nothing is drawn in the
            // chat: a report is not part of the conversation.
            Event::Feedback(bundle) => self.feedback = Some(bundle),
            Event::FeedbackUnavailable(why) => self.feedback_error = Some(why),
            // The kernel's list of what it holds is about the project's rows,
            // not this chat's transcript: `pump` hands it to the workspace,
            // which owns the surfaces. Named rather than swept into a wildcard
            // so the next frame added has to be thought about here too.
            Event::Surfaces(_) => {}
        }
    }

    /// Whether this chat holds a live kernel socket right now.
    pub fn connected(&self) -> bool {
        matches!(self.connection, Connection::Live(_))
    }

    /// Try Live: ask the kernel for the agent's screen now.
    pub fn request_screen(&self) {
        if let Connection::Live(session) = &self.connection {
            session.request_screen();
        }
    }

    /// Ask the kernel for the material behind a report: the exchange holding
    /// `seq`, or the last one the user opened. Answered as `Event::Feedback`
    /// and held in [`Self::feedback`] for the review sheet.
    pub fn request_feedback(&self, seq: Option<u64>, tail: u32) {
        if let Connection::Live(session) = &self.connection {
            session.request_feedback(seq, None, tail, "");
        }
    }

    pub fn take_feedback(&mut self) -> Option<Box<crate::feedback::Bundle>> {
        self.feedback.take()
    }

    pub fn take_feedback_error(&mut self) -> Option<String> {
        self.feedback_error.take()
    }

    /// This chat as the window holds it: the facts behind the row it draws.
    ///
    /// Enough to explain a row the kernel has no agent for. F-137 was a
    /// `Delegate 1 · Working` line for an agent the kernel had never heard of,
    /// and it could not be diagnosed from a report — the window's own view had
    /// to be fetched from Jacob's machine by hand. Everything the label and its
    /// status are computed from is here, so the two sides can be compared
    /// without asking him for anything.
    pub fn row_facts(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "label": self.label(),
            "agent": self.agent_session,
            "parent": self.parent,
            "parent_kernel": self.parent_kernel,
            "delegate_number": self.delegate_number,
            "is_delegate": self.is_delegate(),
            "title": self.title,
            "closed": self.closed,
            "connected": self.connected(),
            "streaming": self.streaming,
            "running_tool": self.has_running_tool(),
            "live_work": self.live.len(),
            "items": self.items.len(),
            "status": self.status,
        })
    }

    /// What this window believes the chat holds, for a report to carry beside
    /// the kernel's transcript. When the two disagree the drawing is usually
    /// the wrong one, and that disagreement is the bug.
    ///
    /// `index` is the part a reader actually works with: one row per drawn
    /// item, in order, with whatever anchor the app holds for it — the
    /// transcript `seq` of a prompt card, the kernel's clock on a wake, a
    /// tool's `call_id`. Those are the three kinds that exist in both views,
    /// so they are enough to line the drawing up against the events.
    ///
    /// The other kinds carry no clock at all, and `anchor: null` says so
    /// rather than leaving a reader to wonder whether something was lost. The
    /// features agent asked for a `seq` and `ts` on every item; the honest
    /// answer is that the app does not have them, and this says which it does.
    pub fn drawn_view(&self) -> serde_json::Value {
        let index: Vec<serde_json::Value> = self
            .items
            .iter()
            .enumerate()
            .map(|(n, item)| {
                let (kind, anchor) = match item {
                    ChatItem::User(m) => (
                        "user",
                        serde_json::json!({"seq": m.seq, "sent_at": m.sent_at, "steer": m.steer}),
                    ),
                    ChatItem::Wake { kind, at, .. } => {
                        ("wake", serde_json::json!({"wake": kind, "at": at}))
                    }
                    ChatItem::Tool { id, status, .. } => (
                        "tool",
                        // Named here rather than derived, and with no wildcard,
                        // so a fourth status cannot slip into a report as a
                        // word nobody chose.
                        serde_json::json!({"call_id": id, "status": match status {
                            ToolStatus::Running => "running",
                            ToolStatus::Success => "success",
                            ToolStatus::Failure => "failure",
                        }}),
                    ),
                    ChatItem::Agent(_) => ("agent", serde_json::Value::Null),
                    ChatItem::From { who, .. } => ("from", serde_json::json!({"who": who})),
                    ChatItem::Thinking { done, .. } => {
                        ("thinking", serde_json::json!({"done": done}))
                    }
                    ChatItem::Notice { failed, .. } => {
                        ("notice", serde_json::json!({"failed": failed}))
                    }
                    ChatItem::Nudge(_) => ("nudge", serde_json::Value::Null),
                    _ => ("other", serde_json::Value::Null),
                };
                serde_json::json!({"n": n, "kind": kind, "anchor": anchor})
            })
            .collect();
        serde_json::json!({
            "items": serde_json::to_value(&self.items).unwrap_or(serde_json::Value::Null),
            "index": index,
            "agent": self.agent_session,
            // What the window thinks is running now. A count, not the work
            // itself: "it says one worker is going and the transcript says
            // none" is the whole question.
            "live_work": self.live.len(),
        })
    }

    /// The kernel sent the whole plan. Work the kernel is doing for this
    /// agent right now — a shell step, a gated watch — is what `live` shows.
    pub fn set_plan(&mut self, nodes: Vec<PlanNode>) {
        let live: Vec<crate::kernel::LiveWork> = nodes
            .iter()
            .filter(|n| n.status == "active" && !n.inbox && n.do_kind != "agent")
            .map(|n| crate::kernel::LiveWork {
                label: n.goal.clone(),
                running: true,
            })
            .collect();
        self.set_live(live);
        if let Some(id) = self.answering {
            let still = nodes.iter().any(|n| {
                n.id == id && n.do_kind == "ask" && n.status != "done" && n.status != "cancelled"
            });
            if !still {
                self.answering = None;
            }
        }
        self.plan = nodes;
    }

    /// Nodes the strip draws: open, not the agent's own inbox.
    /// The plan's open nodes, less the kernel's own housekeeping — a chore
    /// it runs for itself (the weekly `git gc`) is nothing the user asked
    /// for or should manage.
    pub fn plan_open(&self) -> impl Iterator<Item = &PlanNode> {
        self.plan.iter().filter(|n| {
            !n.inbox && n.status != "done" && n.status != "cancelled" && !kernel_chore(n)
        })
    }

    /// Prompts the kernel holds for this agent that have not run yet. A
    /// steer in the inbox is not one: the running turn takes it at its
    /// next step.
    pub fn plan_queued(&self) -> usize {
        self.plan
            .iter()
            .filter(|n| n.inbox && n.status == "pending" && is_user_followup(n))
            .count()
    }

    pub fn plan_op(&mut self, node: u64, op: &str, text: &str) {
        if let Connection::Live(session) = &self.connection {
            session.plan_op(node, op, text);
        }
        // The row goes as the click lands; the kernel's next plan frame is
        // the truth and puts it back if the cancel did not take.
        if op == "cancel"
            && let Some(n) = self.plan.iter_mut().find(|n| n.id == node)
        {
            n.status = "cancelled".into();
        }
    }

    /// The kernel's answer to a plan op on a message it no longer holds —
    /// the running turn took the queued line before the click (an
    /// attached `bash` yields to a queued line within seconds since #362).
    /// For a cancel that is the end state the person wanted: the row goes
    /// and nothing is said. For anything else, one calm line — the words
    /// went, they were not lost. Never a failure: nothing failed. Returns
    /// whether the detail was this case.
    fn plan_op_gone(&mut self, detail: &str) -> bool {
        let Some(rest) = detail.strip_prefix("plan op ") else {
            return false;
        };
        if !rest.contains("no longer in the inbox") {
            return false;
        }
        let mut words = rest.split_whitespace();
        let op = words.next().unwrap_or("");
        let node = words.next().and_then(|w| {
            w.trim_start_matches('#')
                .trim_end_matches(':')
                .parse::<u64>()
                .ok()
        });
        if let Some(id) = node {
            self.plan.retain(|n| n.id != id);
        }
        if op != "cancel" {
            self.notice(false, "That follow-up had already gone into the turn.");
        }
        self.flush();
        true
    }

    fn apply_update(&mut self, update: SessionUpdate) {
        if matches!(
            update,
            SessionUpdate::AgentMessageChunk(_)
                | SessionUpdate::AgentThoughtChunk(_)
                | SessionUpdate::ToolCall(_)
                | SessionUpdate::ToolCallUpdate(_)
        ) {
            self.turn_alive();
        }
        match update {
            SessionUpdate::AgentMessageChunk(chunk) => {
                self.finish_thinking();
                let text = content_text(&chunk.content);
                if matches!(self.items.last(), Some(ChatItem::Agent(_))) {
                    let ix = self.items.len() - 1;
                    self.stream_into(ix, &text);
                    self.streaming_agent = Some(ix);
                } else if let Some(ix) = self.open_stream(text) {
                    self.streaming_agent = Some(ix);
                }
            }
            SessionUpdate::AgentThoughtChunk(chunk) => {
                let text = content_text(&chunk.content);
                // A `status` call between two reasoning steps draws no row:
                // the new step continues the thought before it rather than
                // opening a second "Thought briefly".
                let shown = self.items.iter().rposition(|item| {
                    !matches!(item, ChatItem::Tool { label, .. }
                        if crate::view::component::transcript::is_status_call(label))
                });
                if let Some(ChatItem::Thinking {
                    text: body,
                    done: false,
                    ..
                }) = self.items.last_mut()
                {
                    merge_stream_text(body, &text);
                } else if let Some(ChatItem::Thinking {
                    text: body,
                    done,
                    secs,
                }) = shown.and_then(|at| self.items.get_mut(at))
                    && !text.is_empty()
                {
                    if !body.ends_with('\n') {
                        body.push_str("\n\n");
                    }
                    merge_stream_text(body, &text);
                    // The clock restarts for the new step and adds to the
                    // seconds already on the thought when it settles.
                    *done = false;
                    self.thought_carry = secs.take().unwrap_or(0);
                    self.thought_at = Some(SystemTime::now());
                } else if !text.is_empty() {
                    self.thought_at = Some(SystemTime::now());
                    self.items.push(ChatItem::Thinking {
                        text,
                        done: false,
                        secs: None,
                    });
                }
            }
            SessionUpdate::ToolCall(call) => {
                self.finish_thinking();
                let id = call.tool_call_id.to_string();
                let output = tool_content_text(&call.content);
                let diff = tool_diff_text(&call.content);
                let existing = self
                    .items
                    .iter()
                    .rposition(
                        |item| matches!(item, ChatItem::Tool { id: tool, .. } if *tool == id),
                    )
                    .or_else(|| {
                        // A call-mirror placeholder for this command: take
                        // the kernel's id so the next update lands here.
                        self.items.iter().rposition(|item| {
                            matches!(
                                item,
                                ChatItem::Tool {
                                    id: held_id,
                                    label,
                                    status: ToolStatus::Running,
                                    ..
                                } if held_id.starts_with("voice-")
                                    && (label == &call.title
                                        || label.split_whitespace().next()
                                            == call.title.split_whitespace().next())
                            )
                        })
                    });
                if let Some(ix) = existing {
                    if let ChatItem::Tool { id: held, .. } = &mut self.items[ix] {
                        *held = id.clone();
                    }
                    let ChatItem::Tool {
                        kind,
                        label,
                        status,
                        output: held,
                        diff: held_diff,
                        ..
                    } = &mut self.items[ix]
                    else {
                        unreachable!("rposition matched a Tool item");
                    };
                    *kind = call.kind;
                    if !call.title.is_empty() {
                        *label = call.title;
                    }
                    *status = tool_status(call.status);
                    if !output.is_empty() {
                        *held = output;
                    }
                    if diff.is_some() {
                        *held_diff = diff;
                    }
                } else {
                    self.tool_started.insert(id.clone(), Instant::now());
                    let desc = call
                        .meta
                        .as_ref()
                        .and_then(|meta| meta.get("label"))
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|d| !d.is_empty())
                        .map(str::to_string);
                    self.items.push(ChatItem::Tool {
                        id,
                        kind: call.kind,
                        label: call.title,
                        status: tool_status(call.status),
                        output,
                        diff,
                        child_session: None,
                        secs: None,
                        desc,
                    });
                }
            }
            SessionUpdate::ToolCallUpdate(update) => {
                let id = update.tool_call_id.to_string();
                let Some(ix) = self.items.iter().rposition(
                    |item| matches!(item, ChatItem::Tool { id: tool, .. } if *tool == id),
                ) else {
                    return;
                };
                let ChatItem::Tool {
                    kind,
                    label,
                    status,
                    output,
                    diff,
                    secs,
                    ..
                } = &mut self.items[ix]
                else {
                    unreachable!("rposition matched a Tool item");
                };
                if let Some(title) = update.fields.title {
                    *label = title;
                }
                if let Some(new_kind) = update.fields.kind {
                    *kind = new_kind;
                }
                if let Some(content) = update.fields.content {
                    let text = tool_content_text(&content);
                    if !text.is_empty() {
                        if !output.is_empty() {
                            output.push('\n');
                        }
                        output.push_str(&text);
                    }
                    if let Some(next) = tool_diff_text(&content) {
                        *diff = Some(next);
                    }
                }
                if let Some(new_status) = update.fields.status {
                    *status = tool_status(new_status);
                    if *status != ToolStatus::Running
                        && let Some(at) = self.tool_started.remove(&id)
                    {
                        *secs = Some(at.elapsed().as_secs().min(u32::MAX as u64) as u32);
                    }
                }
            }
            SessionUpdate::SessionInfoUpdate(info) => match info.title {
                MaybeUndefined::Value(title) => self.title = title,
                MaybeUndefined::Null => self.title.clear(),
                MaybeUndefined::Undefined => {}
            },
            SessionUpdate::AvailableCommandsUpdate(cmds) => {
                self.commands = cmds
                    .available_commands
                    .into_iter()
                    .map(|c| Command {
                        name: c.name,
                        description: c.description,
                    })
                    .collect();
            }
            // The kernel's plan comes as `Event::Plan`; ACP plan snapshots
            // are from agents this window no longer runs.
            SessionUpdate::Plan(_) => {}
            SessionUpdate::CurrentModeUpdate(update) => {
                if let Some(modes) = &mut self.modes {
                    modes.current_mode_id = update.current_mode_id;
                }
            }
            // Reported whole, like the plan: replace, don't merge.
            SessionUpdate::ConfigOptionUpdate(update) => self.config = update.config_options,
            SessionUpdate::UsageUpdate(update) => {
                let last_cost = update.cost.as_ref().map(|c| c.amount);
                let spent = match (self.usage.and_then(|u| u.spent), last_cost) {
                    (Some(a), Some(b)) => Some(a + b),
                    (a, b) => a.or(b),
                };
                self.usage = Some(Usage {
                    used: update.used,
                    size: update.size,
                    spent,
                    last_cost,
                });
            }
            // We echo the user's message locally.
            SessionUpdate::UserMessageChunk(_) => {}
            _ => {}
        }
    }

    /// A dictated prompt's turn just ended: read the answer aloud through
    /// the speech server. Once per turn; nothing when voice is not set up.
    fn voice_answer(&mut self) {
        if !std::mem::take(&mut self.voice_reply) || !crate::voice_ws::configured() {
            return;
        }
        let Some(ChatItem::Agent(text)) = self
            .items
            .iter()
            .rev()
            .find(|item| matches!(item, ChatItem::Agent(_)))
        else {
            return;
        };
        if let Err(e) = crate::voice_ws::speak(text) {
            self.notice(true, &format!("voice reply failed: {e:#}"));
        }
    }

    pub(crate) fn notice(&mut self, failed: bool, text: &str) {
        // "openai/gpt-6-astra-pro is not available to this key, so … answers
        // this turn": the model named first is one this key cannot use.
        if let Some(model) = unavailable_model_in(text) {
            self.unavailable_models.insert(model);
        }
        // The kernel's parked-ask line after the question is already
        // answered (a replay, a late frame) says nothing true.
        if !failed && is_waiting_line(text) && self.questions.is_none() {
            return;
        }
        // "nothing to compact yet: the whole working set is recent" answers
        // a `/compact` this window typed. The kernel on main writes it
        // after tool calls of its own accord — five times in one journey
        // run, the kickoff included — and Cursor's pane never carries a
        // compaction status (F-199, cycle 46). Without a recent ask it is
        // the kernel's housekeeping, not the person's line.
        if !failed
            && text.trim_start().starts_with("nothing to compact yet")
            && !self
                .compact_asked
                .is_some_and(|at| at.elapsed() < Duration::from_secs(60))
        {
            return;
        }
        // The kernel's page nudge is a standing state, not news each turn:
        // one line, at the latest turn it applies to. An earlier copy goes.
        if !failed && is_page_nudge(text) {
            self.items.retain(|item| {
                !matches!(item, ChatItem::Notice { text: t, failed: false } if is_page_nudge(t))
            });
        }
        // A retry replaces the retry before it: "retrying in 2.4s (attempt
        // 4/5)" is the same story one step on, and five copies with the
        // URL in each was Jacob's first screen on a new project (F-76).
        if is_retry_line(text)
            && let Some(ChatItem::Notice { text: t, failed: f }) = self.items.last_mut()
            && is_retry_line(t)
        {
            *t = text.to_string();
            *f = failed;
            self.updated = SystemTime::now();
            return;
        }
        // The same words twice in a row (a turn that failed the same way
        // again) read once; Cursor never stacks identical lines.
        if matches!(self.items.last(), Some(ChatItem::Notice { text: t, failed: f }) if t == text && *f == failed)
        {
            return;
        }
        // "no reply from the kernel" is what the window says when a turn
        // ends on the prompt with nothing under it; the kernel's own
        // reason lands a tail-tick later ("… did not accept the API key").
        // The reason is the line; the guess before it goes (F-189).
        if failed
            && text != NO_REPLY
            && matches!(self.items.last(), Some(ChatItem::Notice { text: t, .. }) if t == NO_REPLY)
        {
            self.items.pop();
        }
        self.items.push(ChatItem::Notice {
            text: text.to_owned(),
            failed,
        });
    }

    fn finish_thinking(&mut self) {
        // The open thought is the last item, or the one a later step
        // reopened across a `status` call.
        let open = self
            .items
            .iter()
            .rposition(|item| matches!(item, ChatItem::Thinking { done: false, .. }));
        if let Some(ChatItem::Thinking { done, secs, .. }) =
            open.and_then(|at| self.items.get_mut(at))
        {
            *done = true;
            let took = self
                .thought_at
                .and_then(|at| at.elapsed().ok())
                .map(|d| d.as_secs() as u32)
                .unwrap_or(0);
            *secs = Some(took.saturating_add(self.thought_carry));
        }
        self.thought_carry = 0;
        self.thought_at = None;
    }

    fn fail_running_tools(&mut self) {
        for item in &mut self.items {
            if let ChatItem::Tool { status, .. } = item
                && *status == ToolStatus::Running
            {
                *status = ToolStatus::Failure;
            }
        }
    }
}

fn history_beats(next: &[ChatItem], held: &[ChatItem]) -> bool {
    if next.len() > held.len() {
        return true;
    }
    if next.len() < held.len() {
        return false;
    }
    if next.is_empty() {
        return false;
    }
    let thoughts = |items: &[ChatItem]| {
        items
            .iter()
            .filter(|item| matches!(item, ChatItem::Thinking { .. }))
            .count()
    };
    let diffs = |items: &[ChatItem]| {
        items
            .iter()
            .filter(|item| {
                matches!(
                    item,
                    ChatItem::Tool {
                        diff: Some(diff),
                        ..
                    } if !diff.trim().is_empty()
                )
            })
            .count()
    };
    thoughts(next) > thoughts(held) || diffs(next) > diffs(held)
}

fn ended_on_user(items: &[ChatItem]) -> bool {
    items
        .iter()
        .rev()
        .find(|item| !matches!(item, ChatItem::Notice { .. }))
        .is_some_and(|item| matches!(item, ChatItem::User(_)))
}

pub(crate) fn link_child(items: &mut [ChatItem], call_id: &str, session: &str) {
    if let Some(ChatItem::Tool { child_session, .. }) = items
        .iter_mut()
        .rev()
        .find(|item| matches!(item, ChatItem::Tool { id, .. } if id == call_id))
    {
        *child_session = Some(session.to_owned());
    }
}

fn tool_status(status: ToolCallStatus) -> ToolStatus {
    match status {
        ToolCallStatus::Completed => ToolStatus::Success,
        ToolCallStatus::Failed => ToolStatus::Failure,
        _ => ToolStatus::Running,
    }
}

fn tool_content_text(content: &[ToolCallContent]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            ToolCallContent::Content { content } => {
                let text = content_text(content);
                (!text.is_empty()).then_some(text)
            }
            ToolCallContent::Terminal { .. } => Some("[terminal]".to_owned()),
            ToolCallContent::Diff(_) => None,
            ToolCallContent::Other(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The kernel's display diff (`Details.diff`), carried as ACP `Diff.new_text`.
fn tool_diff_text(content: &[ToolCallContent]) -> Option<String> {
    content.iter().find_map(|c| match c {
        ToolCallContent::Diff(diff) if !diff.new_text.is_empty() => Some(diff.new_text.clone()),
        _ => None,
    })
}

/// Whether `title`/`questions` describe the question `open` already shows:
/// the same prompt text and the same option labels, in order.
fn same_question(open: &AskPrompt, title: &str, questions: &[AskQuestion]) -> bool {
    if open.title != title || open.questions.len() != questions.len() {
        return false;
    }
    open.questions.iter().zip(questions).all(|(a, b)| {
        a.prompt == b.prompt
            && a.allow_multiple == b.allow_multiple
            && a.options.len() == b.options.len()
            && a.options
                .iter()
                .zip(&b.options)
                .all(|(x, y)| x.id == y.id && x.label == y.label)
    })
}

fn content_text(block: &ContentBlock) -> String {
    match block {
        ContentBlock::Text(text) => text.text.clone(),
        _ => "[non-text content]".to_owned(),
    }
}

/// Live deltas and the persisted full line can both arrive. Do not
/// print the reply twice.
/// How long a kickoff may stay silent before a typed prompt stops waiting
/// for it.
const KICKOFF_QUIET: Duration = Duration::from_secs(90);

/// Silence from the kernel while a turn is owed: when the liveness probe
/// goes out, and when the silence is called what it is. Ten seconds is the
/// phone's figure for "the kernel has not echoed the typed line".
const QUIET_PROBE: Duration = Duration::from_secs(10);
const QUIET_LOST: Duration = Duration::from_secs(20);

/// The streamed and the settled copy of one thought: the same words, or
/// one the other with the trailing whitespace the stream had.
fn same_words(a: &str, b: &str) -> bool {
    let a = a.trim();
    let b = b.trim();
    !a.is_empty() && (a == b || a.starts_with(b) || b.starts_with(a))
}

fn merge_stream_text(body: &mut String, text: &str) {
    if text.is_empty() || body == text || body.starts_with(text) {
        return;
    }
    if text.starts_with(body.as_str()) {
        body.push_str(&text[body.len()..]);
        return;
    }
    // The whole message, sent again after its deltas, and not quite what the
    // deltas added up to — a chunk lost on the way, a space reshaped. It
    // opens the way the body opens; a delta never does. It is the message,
    // so it replaces the body rather than following it as a second copy.
    if text.len() >= RESEND_PREFIX
        && body.len() >= RESEND_PREFIX
        && text.as_bytes()[..RESEND_PREFIX] == body.as_bytes()[..RESEND_PREFIX]
    {
        *body = text.to_owned();
        return;
    }
    body.push_str(text);
}

/// How much of a message's opening two texts must share before one is taken
/// to be the other, sent whole.
const RESEND_PREFIX: usize = 64;

/// Drain the kernel event channel into the session, for as long as there is one.
///
/// `previous` is the kernel chat id: with it set, the same chat is opened
/// rather than a new one.
fn pump(
    id: u64,
    _entry: &settings::Agent,
    place: Place,
    previous: Option<String>,
    attach_gen: u64,
    cx: &mut Context<Workspace>,
) -> Task<()> {
    let conn = acp::runtime().spawn(async move {
        Session::spawn(Launch {
            previous,
            ..Launch::new(place)
        })
        .await
    });

    cx.spawn(async move |this, cx| {
        let opened = conn
            .await
            .unwrap_or_else(|e| Err(anyhow!("the connection task panicked: {e}")));
        let (session, mut events) = match opened {
            Ok(pair) => pair,
            Err(e) => {
                // A start that lost the race — two rows spawning the same
                // kernel, one exits on the lock; the socket not yet open;
                // a remote tunnel that dropped — is tried again with
                // backoff until hello. Only a fault no retry can mend (no
                // kernel binary, a path that is not a directory) stops here.
                let again = transient_connect_error(&e);
                let _ = this.update(cx, |workspace, cx| {
                    workspace.with_session(id, cx, |chat| {
                        if chat.attach_gen != attach_gen {
                            return;
                        }
                        chat.connection = Connection::Lost;
                        // The reason, in the words a person reads, with
                        // the bootstrap step it died on when there was one
                        // ("installing arbos-kernel 0.2.1 failed: ArbosLife
                        // refused permission: …"). Jacob read
                        // "Connection failed." and a counter for three
                        // hours while the reason sat behind Details.
                        let reason = connect_fault_words(chat.host.as_deref(), &chat.cwd, &e);
                        let news = chat.connect_fault.as_deref() != Some(reason.as_str());
                        chat.connect_fault = Some(reason.clone());
                        // A fault is news the first time it is seen; a
                        // retry in progress is not, and a race the next
                        // try wins is not worth a line.
                        if (!again && news)
                            || chat.reconnect_attempt >= 1 && chat.reconnect_attempt % 5 == 0
                        {
                            chat.notice(true, &format!("connection failed: {reason}"));
                        }
                    });
                    let retry = workspace
                        .session(id)
                        .is_some_and(|chat| chat.attach_gen == attach_gen && chat.resumable());
                    if retry {
                        // A fault no retry mends (no binary, a bad path)
                        // is still re-checked, slowly: the machine may be
                        // fixed from the other side, and a tab that never
                        // looks again is a dead tab.
                        workspace.schedule_reconnect(id, !again, cx);
                    }
                });
                return;
            }
        };

        let attached = this.update(cx, |workspace, cx| {
            let ours = workspace
                .session(id)
                .is_some_and(|chat| chat.attach_gen == attach_gen);
            if !ours {
                return false;
            }
            workspace.with_session(id, cx, |chat| {
                chat.agent_session = Some(session.session_id.clone());
                // The kernel's permission modes, current from agent.md.
                let current = crate::kernel::agent_mode(&chat.place(), &session.session_id)
                    .unwrap_or_else(|| "auto".into());
                chat.modes = Some(crate::agent::acp::Session::modes(&current));
                chat.config = Vec::new();
                // The kernel remembers the agent's model in agent.md. That
                // is the truth on attach; a model picked while offline is
                // pushed only when the agent has none of its own yet.
                let kept = (chat.host.is_none())
                    .then(|| {
                        crate::kernel::agent_model(
                            &arbos_core::Place::new(&chat.cwd),
                            &session.session_id,
                        )
                    })
                    .flatten();
                match (kept, chat.model.clone()) {
                    (Some(kept), _) => chat.model = Some(kept),
                    (None, Some(model)) => session.set_model(&model),
                    (None, None) => {}
                }
                chat.connection = Connection::Live(Box::new(session));
            });
            workspace.session_connected(id, cx);
            true
        });
        if !matches!(attached, Ok(true)) {
            return;
        }
        // Attached: ask this kernel what it holds. Every attach, not only a
        // reconnect — the kernel answering now may not be the one that opened
        // the rows this window is drawing, and after a relaunch it certainly
        // is not. The answer comes back as `Event::Surfaces` below.
        let _ = this.update(cx, |workspace, _| workspace.ask_surfaces(id));

        while let Some(event) = events.recv().await {
            let mut batch = vec![event];
            while let Ok(event) = events.try_recv() {
                batch.push(event);
            }
            let streaming = this.update(cx, |workspace, cx| {
                let mut shown = Vec::new();
                let mut surfaces = Vec::new();
                let mut children = Vec::new();
                let mut ended = false;
                let mut store_moved = false;
                // What the kernel says it holds, in answer to the ask above.
                let mut listed: Option<Vec<KernelSurface>> = None;
                workspace.with_session(id, cx, |chat| {
                    for event in batch {
                        ended |= matches!(event, Event::TurnDone(_));
                        match event {
                            Event::Show { path, title, kind } => {
                                shown.push((path, title, kind));
                            }
                            Event::StoreChanged(_) => store_moved = true,
                            Event::Surfaces(list) => listed = Some(list),
                            // Kernel rows come and go in order; keep it.
                            event @ (Event::Open { .. }
                            | Event::Hide { .. }
                            | Event::Browser { .. }
                            | Event::Pty { .. }
                            | Event::Job { .. }) => surfaces.push(event),
                            Event::ChildSession { call_id, session } => {
                                link_child(&mut chat.items, &call_id, &session);
                                chat.flush();
                                children.push(session);
                            }
                            event => chat.apply(event),
                        }
                    }
                });
                if let Some(list) = listed {
                    workspace.reconcile_surfaces(id, &list, cx);
                }
                for (path, title, kind) in shown {
                    workspace.open_shown(id, path, title, kind, None, None, OpenedBy::Agent, cx);
                }
                for event in surfaces {
                    match event {
                        // `by` (user | agent) is on the event for the drawer's
                        // rule — open when the person asked, stay quiet when
                        // the agent did; the drawer reads it when it lands.
                        Event::Open {
                            path,
                            title,
                            kind,
                            cwd,
                            url,
                            by,
                        } => workspace.open_shown(
                            id,
                            path,
                            title,
                            kind,
                            cwd,
                            url,
                            OpenedBy::from_frame(&by),
                            cx,
                        ),
                        Event::Hide { path, kind } => {
                            if kind == "process" {
                                workspace.finish_shown_process(id, &path, cx)
                            } else {
                                workspace.close_shown(id, &path, cx)
                            }
                        }
                        Event::Browser {
                            page,
                            url,
                            screenshot,
                        } => workspace.browser_moved(id, page, url, screenshot, cx),
                        Event::Pty { page, data } => workspace.pty_output(id, page, data, cx),
                        Event::Job {
                            id: job,
                            delta,
                            running,
                            exit,
                        } => workspace.job_output(id, job, delta, running, exit, cx),
                        _ => {}
                    }
                }
                for sid in children {
                    workspace.ensure_child_agent(id, sid, cx);
                }
                // The kernel says the project page moved: re-read the
                // store now, ahead of (or instead of, for a remote place)
                // the file watch's knock.
                if store_moved && let Some(root) = workspace.project_root_of(id) {
                    workspace.reload_project(&root, cx);
                }
                workspace.settle_delegate(id, ended, cx);
                workspace.session(id).is_some_and(|chat| chat.busy())
            });
            match streaming {
                Ok(true) => cx.background_executor().timer(STREAM_FRAME).await,
                Ok(false) => {}
                Err(_) => return,
            }
        }

        let _ = this.update(cx, |workspace, cx| {
            let should_resume = if let Some(chat) = workspace.session_mut(id) {
                if chat.attach_gen != attach_gen {
                    false
                } else if matches!(chat.connection, Connection::Live(_)) {
                    let busy = chat.streaming || chat.has_running_tool();
                    let queued = !chat.queue.is_empty();
                    chat.forget_socket();
                    // The kernel stops itself when its folder moves out
                    // from under it: say what happened and where the
                    // window looked, at the moment it happens (QA `af-03`).
                    if chat.place_gone() {
                        if !chat.has_place_gone_notice() {
                            let text = format!(
                                "{PLACE_GONE}: expected {}. The kernel stopped; a line typed here is kept until the folder is back or the project is reopened.",
                                chat.cwd.display()
                            );
                            chat.notice(true, &text);
                        }
                        chat.flush();
                    } else if busy && !queued {
                        chat.notice(false, "Stopped.");
                        chat.flush();
                    }
                    chat.resumable() && (queued || busy)
                } else {
                    chat.resumable() && chat.idle() && !chat.queue.is_empty()
                }
            } else {
                false
            };
            if should_resume && let Some(chat) = workspace.session_mut(id) {
                if chat.idle() && chat.resumable() {
                    chat.resume(cx);
                }
            }
            // A socket that dropped and was not resumed at once comes back
            // on its own, with backoff — a remote tunnel, or a local kernel
            // that stopped and gets started again.
            let lost = workspace.session(id).is_some_and(|chat| {
                chat.attach_gen == attach_gen
                    && chat.resumable()
                    && matches!(chat.connection, Connection::Lost)
            });
            if lost {
                workspace.schedule_reconnect(id, false, cx);
            }
            cx.notify();
        });
    })
}

/// The kernel's turn-end reminder that `.arbos/notes.md` did not change
/// after a worker started or reported.
pub fn is_page_nudge(text: &str) -> bool {
    text.trim_start().starts_with("project page not updated")
}

/// The model a kernel notice says this key cannot use: the id before
/// " is not available to this key".
pub fn unavailable_model_in(text: &str) -> Option<String> {
    let (head, _) = text.split_once(" is not available to this key")?;
    let model = head
        .trim()
        .rsplit(' ')
        .next()?
        .trim_matches(|c| c == '`' || c == '"');
    (!model.is_empty() && model.contains('/')).then(|| model.to_string())
}

/// The kernel's "… — retrying in 2.4s (attempt 4/5)" line.
pub fn is_retry_line(text: &str) -> bool {
    text.contains("retrying in") && text.contains("(attempt ")
}

/// An inbox node the user put there — a follow-up typed while the turn
/// ran. A worker's report (`say`, `done`), an answer, a subscription
/// firing wait in the same inbox and are the kernel's to deliver: they are
/// not the user's queue, and "Send now / Edit / Remove" would be wrong on
/// them (F-68).
pub fn is_user_followup(node: &PlanNode) -> bool {
    // The kernel files every non-steer inbox message as `agent`; who put
    // it there is in `origin`.
    node.do_kind != "steer" && (node.origin.is_empty() || node.origin == "user")
}

/// A standing node the kernel keeps for itself: it marks the prompt so and
/// answers to nobody (`deliver_to = none`).
pub fn kernel_chore(node: &PlanNode) -> bool {
    node.standing && node.goal.contains("(kernel chore)")
}

/// Whether a failed connect is worth another try. A kernel that exited on
/// start (the lock race between two rows spawning it), a port not yet
/// listening, a timeout, a tunnel that dropped: yes. No kernel binary, a
/// path that is not a directory, a URL that is not one: no retry mends it.
fn transient_connect_error(e: &anyhow::Error) -> bool {
    let text = format!("{e:#}");
    ![
        "failed to start",
        "is not a file",
        "is not a directory",
        "bad kernel url",
    ]
    .iter()
    .any(|fault| text.contains(fault))
}

/// Why a connect failed, in one plain clause with the machine named, for
/// the notice and the bar: the hub or the transport has usually said
/// exactly why, and the person should read it rather than "Connection
/// failed." A remote bootstrap that died mid-step names the step first
/// ("installing arbos-kernel 0.2.1 on ArbosLife failed: …").
pub(crate) fn connect_fault_words(host: Option<&str>, cwd: &Path, e: &anyhow::Error) -> String {
    let text = format!("{e:#}");
    let machine = host.unwrap_or("this machine");
    let step = host
        .and_then(|h| crate::kernel::remote_progress(h, cwd))
        .and_then(|p| match p {
            arbos_core::remote_kernel::Progress::Failed { step, .. } => Some(step),
            _ => None,
        })
        .filter(|s| !s.eq_ignore_ascii_case("connecting"))
        .map(|s| {
            let mut c = s.chars();
            let lower = c
                .next()
                .map(|f| f.to_lowercase().collect::<String>() + c.as_str())
                .unwrap_or_default();
            // The reason names the machine; the step need not again.
            format!("{lower} failed: ")
        })
        .unwrap_or_default();
    format!("{step}{}", plain_transport_words(machine, &text))
}

/// The transport's or the kernel's reason in the reader's words. Patterns
/// first; else the most specific link of the error chain, whole.
fn plain_transport_words(machine: &str, text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let http = lower
        .find("http ")
        .or_else(|| lower.find("status "))
        .and_then(|at| lower[at..].split_whitespace().nth(1))
        .and_then(|code| {
            code.trim_matches(|c: char| !c.is_ascii_digit())
                .parse::<u16>()
                .ok()
        })
        .filter(|code| (400..600).contains(code));
    if lower.contains("publickey") {
        return format!("ssh to {machine} refused the key");
    }
    if lower.contains("permission denied") {
        let what = text.rsplit(": ").next().unwrap_or(text).trim();
        return format!("{machine} refused permission: {}", shorten_words(what, 80));
    }
    if lower.contains("could not resolve hostname")
        || lower.contains("name or service not known")
        || lower.contains("nodename nor servname")
    {
        return format!("{machine} is not a name this machine can resolve");
    }
    if lower.contains("no route to host") || lower.contains("network is unreachable") {
        return format!("no route to {machine}");
    }
    if lower.contains("connection refused") {
        return format!("{machine} refused the connection (nothing listening)");
    }
    if lower.contains("timed out") || lower.contains("did not answer") || lower.contains("timeout")
    {
        return format!("{machine} did not answer (timeout)");
    }
    if let Some(code) = http {
        return match code {
            404 => format!("{machine} answered HTTP 404 — not an Arbos kernel or hub"),
            502 | 503 | 504 => format!("{machine} is not answering (HTTP {code})"),
            401 | 403 => format!("{machine} refused this window (HTTP {code})"),
            _ => format!("{machine} answered HTTP {code}"),
        };
    }
    if lower.contains("failed to start") {
        let bin = text
            .split("failed to start ")
            .nth(1)
            .and_then(|rest| rest.split([':', ' ']).next())
            .unwrap_or("arbos-kernel");
        return format!("no kernel binary at {bin} on {machine}");
    }
    if lower.contains("is not a directory") {
        return "the project folder is gone or was moved".into();
    }
    if lower.contains("is not a file") {
        return format!("ARBOS_KERNEL_BIN does not point at a file on {machine}");
    }
    if lower.contains("bad kernel url") {
        return format!("the kernel on {machine} announced an address this window cannot read");
    }
    // The chain's most specific link: after the last ": " when the links
    // read as context, else the whole text, bounded.
    let last = text.rsplit(": ").next().unwrap_or(text).trim();
    let words = if last.len() >= 12 { last } else { text.trim() };
    shorten_words(words, 140)
}

fn shorten_words(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// An agent id as a person reads it: dashes and underscores to spaces, the
/// first letter up. `add-sources-to-project-context` → "Add sources to
/// project context". An id that is already words (a name) comes back as is.
pub(crate) fn humanize_id(id: &str) -> String {
    let id = id.trim();
    if id.contains(' ') || id.is_empty() {
        return id.to_string();
    }
    let spaced = id.replace(['-', '_'], " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => spaced,
    }
}

/// A child's row name from the brief the kernel named it after: the lead
/// clause, before a colon, a parenthesis, or the first sentence's end —
/// "Echo agent: reply to your parent (…" reads as "Echo agent".
fn delegate_label(name: &str) -> String {
    let name = name.trim();
    let cut = [": ", " (", ". ", " — ", " - "]
        .iter()
        .filter_map(|sep| name.find(sep))
        .filter(|&at| at >= 6)
        .min()
        .unwrap_or(name.len());
    let lead = name[..cut].trim_end_matches(['.', ':', ',']).trim();
    if lead.is_empty() {
        name.to_string()
    } else {
        lead.to_string()
    }
}

/// First thing someone said in this transcript. Used to bind a local file
/// that never stored the kernel id back to the kernel chat it came from.
pub(crate) fn first_user_text(items: &[ChatItem]) -> Option<&str> {
    items.iter().find_map(|item| match item {
        ChatItem::User(message) => {
            let text = message.text.trim();
            (!text.is_empty()).then_some(text)
        }
        ChatItem::From { text, .. } => {
            let text = text.trim();
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    })
}

/// Who spoke a call-mirror agent row, and the words. `agent.event/say`
/// arrives as `from: text` when the kernel named a worker.
fn call_work_speaker(agent: &str, text: &str) -> (String, String) {
    let text = text.trim();
    if let Some((who, body)) = text.split_once(':') {
        let who = who.trim();
        let body = body.trim();
        if !who.is_empty() && !body.is_empty() && !who.contains(' ') {
            return (who.to_string(), body.to_string());
        }
    }
    (agent.trim().to_string(), text.to_string())
}

/// Fold a transcript into a key that two copies of the same chat share:
/// the first spoken line, else the title.
pub(crate) fn conversation_key(items: &[ChatItem], title: &str) -> Option<String> {
    if let Some(text) = first_user_text(items) {
        let folded: String = text
            .chars()
            .flat_map(char::to_lowercase)
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let clipped: String = folded.chars().take(200).collect();
        return (!clipped.is_empty()).then_some(clipped);
    }
    let title = title.trim();
    (!title.is_empty()).then(|| format!("title:{}", title.to_lowercase()))
}

/// What the pane says for an `interrupted` transcript line. `stop` (the
/// button, a stop word) is the user's own hand; anything else names the
/// cause the kernel gave.
pub fn interrupt_label(detail: &str) -> String {
    let d = detail.trim();
    let lower = d.to_ascii_lowercase();
    // The kernel says where the stop landed ("stop during model call",
    // "stop during compaction"); the user does not need to know.
    if lower.is_empty() || lower == "user" || lower.starts_with("stop") {
        return STOPPED_BY_YOU.to_owned();
    }
    match lower.as_str() {
        "force" | "steer" | "follow-up" => "Interrupted for your follow-up".to_owned(),
        _ => format!("Interrupted: {d}"),
    }
}

/// The notice text for a turn the user stopped; the fold line keys on it.
pub const STOPPED_BY_YOU: &str = "Stopped by you";
/// The head of the notice for a place whose folder moved under the window
/// (QA `af-03`); the path it expected follows.
pub const PLACE_GONE: &str = "This project's folder is gone or was moved";
/// The agent's folder is missing while its place is (F-165).
pub const AGENT_GONE: &str = "this agent's folder is gone";

/// The kernel's line for a turn that ended at the user's own per-turn
/// spend cap ("Stopped at the per-turn cap: this turn spent $… over the $…
/// you allow"; older kernels: "…over the $… cap").
pub fn is_cap_notice(text: &str) -> bool {
    let lower = text.trim().to_ascii_lowercase();
    lower.starts_with("stopped at the per-turn cap")
        || (lower.contains("over the $") && lower.contains(" cap"))
}

/// A model's short apology or refusal for a stop the kernel imposed —
/// "I'm sorry, I cannot complete your request." after the cap line. The
/// kernel's own line says what happened; the model's regret about it is
/// noise (qal-j05), the same family as the empty-reply apology the engine
/// trims. Short and about not doing the thing; a real answer that happens
/// to open with "Sorry, the tests fail" is longer or says more.
pub fn is_cap_apology(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() || t.chars().count() > 240 {
        return false;
    }
    let lower = t.to_ascii_lowercase();
    let regret = [
        "sorry",
        "apolog",
        "unfortunately",
        "i cannot",
        "i can't",
        "i am unable",
        "i'm unable",
    ]
    .iter()
    .any(|w| lower.contains(w));
    let about_stopping = [
        "cannot complete",
        "can't complete",
        "unable to complete",
        "cannot continue",
        "can't continue",
        "unable to continue",
        "cannot proceed",
        "can't proceed",
        "unable to proceed",
        "cannot fulfill",
        "cannot fulfil",
        "can't fulfill",
        "cap",
        "budget",
        "limit",
        "cost",
    ]
    .iter()
    .any(|w| lower.contains(w));
    regret && about_stopping
}

/// Whether a notice marks the end of an interrupted turn.
pub fn is_interrupt_notice(text: &str) -> bool {
    text == STOPPED_BY_YOU || text.starts_with("Interrupted")
}

/// The kernel's answer to `kickoff` on a place with no model key (#312):
/// no turn follows.
const KICKOFF_NOT_STARTED: &str = "kickoff not started:";

/// In the kernel's line when it kept a typed prompt in its inbox for want
/// of a key (#312): "… Your message is kept and runs once a key is in
/// place: <the words>".
pub(crate) const LINE_KEPT_FOR_KEY: &str = "Your message is kept and runs once a key is in place";

/// The kernel's notice while an `ask` is parked with the user.
fn is_waiting_line(text: &str) -> bool {
    text.trim() == "Waiting for your answer"
}

/// A `status` call written as a line of the reply, in any of the forms a
/// model produces: the kernel's own record `status: <step>`, the prompt's
/// example `status "<step>"` (QA qal-j01: three of them over the kickoff
/// greeting, drawn as bubbles), `status 'step'`, `status(step)`,
/// `status = step`, with or without markdown around the word. One line
/// only. The step, or `None` for prose — "Status quo is fine" is prose.
pub(crate) fn status_line(text: &str) -> Option<String> {
    let line = text.trim();
    if line.contains('\n') {
        return None;
    }
    let line = line.trim_matches(|c: char| matches!(c, '*' | '_' | '`'));
    let rest = ["status", "Status", "STATUS"]
        .iter()
        .find_map(|word| line.strip_prefix(word))?;
    let rest = rest.trim_start_matches(|c: char| matches!(c, '*' | '_' | '`'));
    let rest = rest.trim_start();
    let step = if let Some(r) = rest.strip_prefix(':').or_else(|| rest.strip_prefix('=')) {
        r.trim().to_string()
    } else if let Some(r) = rest.strip_prefix('(') {
        r.trim_end_matches(|c: char| matches!(c, '*' | '_' | '`'))
            .strip_suffix(')')
            .unwrap_or(r)
            .trim()
            .to_string()
    } else if rest.starts_with(['"', '\'', '“', '‘']) {
        rest.trim_end_matches(|c: char| matches!(c, '*' | '_' | '`' | '.'))
            .trim_matches(|c: char| matches!(c, '"' | '\'' | '“' | '”' | '‘' | '’'))
            .trim()
            .to_string()
    } else {
        return None;
    };
    let step = step
        .trim_matches(|c: char| matches!(c, '"' | '\'' | '“' | '”' | '‘' | '’' | '*' | '`' | '_'))
        .trim();
    (!step.is_empty()).then(|| step.to_string())
}

#[cfg(test)]
mod clear_tests {
    use super::{ChatItem, ChatSession, is_router_step};
    use crate::model::testing::Scratch;
    use std::time::SystemTime;

    fn chat(scratch: &Scratch, items: Vec<ChatItem>) -> ChatSession {
        ChatSession::from_kernel(
            1,
            crate::model::place::Place::local(scratch.path()),
            crate::model::settings::kernel_agent(),
            "root".into(),
            "Sorting out the panel".into(),
            None,
            items,
            SystemTime::now(),
        )
    }

    fn said(lines: &[&str]) -> Vec<ChatItem> {
        lines
            .iter()
            .map(|line| ChatItem::Agent((*line).to_string()))
            .collect()
    }

    #[test]
    fn a_cleared_view_survives_the_kernels_longer_copy() {
        let scratch = Scratch::new("clear-adopt");
        let mut chat = chat(&scratch, said(&["one", "two"]));
        chat.clear_view();
        assert!(chat.view_cleared());
        // The kernel keeps more of the same conversation than the window
        // cached. Before the fix the mark stayed at 2 and the hidden
        // lines came back.
        chat.adopt_history(said(&["one", "two", "three", "four"]));
        assert!(chat.view_cleared(), "clear was undone by the merge");
    }

    #[test]
    fn a_view_that_was_not_cleared_keeps_its_place() {
        let scratch = Scratch::new("clear-untouched");
        let mut chat = chat(&scratch, said(&["one", "two"]));
        chat.adopt_history(said(&["one", "two", "three"]));
        assert_eq!(chat.hide_before, 0);
        assert!(!chat.view_cleared());
    }

    #[test]
    fn the_routers_own_step_is_not_the_chats() {
        assert!(is_router_step("Choosing the next step"));
        assert!(!is_router_step("Reading main.py"));
        assert!(!is_router_step("Saving a checkpoint of the working tree"));
    }
}

#[cfg(test)]
mod status_line_tests {
    use super::status_line;

    #[test]
    fn every_form_a_model_writes_is_the_step() {
        assert_eq!(
            status_line("status: Reading the file"),
            Some("Reading the file".into())
        );
        assert_eq!(
            status_line("Status: Setting plan"),
            Some("Setting plan".into())
        );
        assert_eq!(
            status_line("status \"Looking around the new place\""),
            Some("Looking around the new place".into())
        );
        assert_eq!(
            status_line("status 'Writing project context'"),
            Some("Writing project context".into())
        );
        assert_eq!(
            status_line("status(\"Spawning worker\")"),
            Some("Spawning worker".into())
        );
        assert_eq!(
            status_line("**status:** Running tests"),
            Some("Running tests".into())
        );
        assert_eq!(
            status_line("`status \"Setting plan\"`"),
            Some("Setting plan".into())
        );
    }

    #[test]
    fn prose_is_not_a_step() {
        assert_eq!(status_line("Status quo is fine."), None);
        assert_eq!(status_line("status"), None);
        assert_eq!(status_line("The status of the build is green."), None);
        assert_eq!(status_line("status: Reading\nthe file"), None);
    }
}

/// A worker's folder name as a label: "math-docstrings" → "math docstrings",
/// "sleep-done-2" → "sleep done 2". `None` for an id that is only a
/// counter — "chat-1789394544546", "agent-3", digits — which says nothing.
pub(crate) fn worker_name(id: &str) -> Option<String> {
    let id = id.trim();
    if id.is_empty() || id.eq_ignore_ascii_case("root") {
        return None;
    }
    let words: Vec<&str> = id.split(['-', '_']).filter(|w| !w.is_empty()).collect();
    let said = words.iter().any(|w| {
        w.chars().any(|c| c.is_ascii_alphabetic())
            && !matches!(*w, "chat" | "agent" | "worker" | "delegate")
    });
    if !said {
        return None;
    }
    Some(words.join(" "))
}

/// The line under a rewind, in the reader's words: what came back, not
/// the commit hashes the kernel reports (they are in the kernel's log).
fn rewound_line(dropped: u64, files: bool) -> String {
    let lines = if dropped == 1 {
        "1 line".to_string()
    } else {
        format!("{dropped} lines")
    };
    if files {
        format!("Rewound to before this prompt: {lines} of chat cut, files restored")
    } else {
        format!("Rewound to before this prompt: {lines} of chat cut, files untouched")
    }
}
