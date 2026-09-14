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
    agent::acp::{self, Event, Launch, Reply, Session},
    model::{
        attachment::{DescribedImage, MessageImage, Prompt, UserMessage},
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
    ContentBlock, MaybeUndefined, PermissionOptionKind, RequestPermissionRequest,
    RequestPermissionResponse, SessionConfigKind, SessionConfigOption, SessionConfigOptionValue,
    SessionModeState, SessionUpdate, StopReason, ToolCallContent, ToolCallStatus, ToolKind,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
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
    /// Files a tool produced for the user to look at: screenshots and
    /// screen recordings. One row per tool call; click opens the file.
    Artifacts(Vec<Artifact>),
    /// A question the agent asked, answered: the card folded to one line.
    /// An empty `answer` is a skip.
    Asked { question: String, answer: String },
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

/// One way to answer a permission request. `kind` is what decides how the
/// button paints — allow and reject must not look alike.
pub struct Choice {
    pub id: String,
    pub name: String,
    pub kind: PermissionOptionKind,
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
        let answers = self
            .questions
            .iter()
            .map(|question| {
                let mut draft = self.draft(question.id.as_str());
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
        let details = if used_as_other {
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
    Reconnecting(Box<Session>),
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

pub struct PermissionPrompt {
    pub title: String,
    pub options: Vec<Choice>,
    /// Whether the answer should stand for every call like this one, rather
    /// than for this one alone — the checkbox beside the buttons. It picks
    /// between the `*Once` and `*Always` forms of whichever button is pressed,
    /// which is what lets two buttons carry four options.
    pub always: bool,
    reply: Reply<RequestPermissionResponse>,
    /// Set when the kernel asked, so the answer goes back as an intent.
    kernel_approval: Option<String>,
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
    pub items: Vec<ChatItem>,
    /// The agent's plan as the kernel last sent it: every node, inbox
    /// rows included. The strip above the composer draws the open ones.
    pub plan: Vec<PlanNode>,
    /// A plan question the next composer send answers.
    pub answering: Option<u64>,
    pub permission: Option<PermissionPrompt>,
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
    /// When `live` last became non-empty, for the braille tick.
    pub live_since: Option<SystemTime>,
    /// When each running tool call began, by call id, so its finished
    /// item can say how long it took.
    tool_started: HashMap<String, Instant>,
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
            items: Vec::new(),
            plan: Vec::new(),
            answering: None,
            permission: None,
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
            transcript: transcript::State::default(),
            draft: String::new(),
            live: Vec::new(),
            tool_started: HashMap::new(),
            live_since: None,
            turn_open: false,
            turn_ended: None,
            probed_at: None,
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
            items: record.items,
            plan: Vec::new(),
            answering: None,
            permission: None,
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
            transcript: transcript::State::default(),
            draft: record.draft,
            live: Vec::new(),
            tool_started: HashMap::new(),
            live_since: None,
            turn_open: false,
            turn_ended: None,
            probed_at: None,
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
            items,
            plan: Vec::new(),
            answering: None,
            permission: None,
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
            transcript: transcript::State::default(),
            draft: String::new(),
            live: Vec::new(),
            tool_started: HashMap::new(),
            live_since: None,
            turn_open: false,
            turn_ended: None,
            probed_at: None,
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
        self.queue.clear();
        self.flush();
    }

    /// The turn just ended: write its wall time on the prompt that started
    /// it, for the "Worked 21s" line. Once per turn; a late duplicate end
    /// leaves the first figure.
    fn stamp_worked(&mut self) {
        let Some(elapsed) = self.elapsed() else {
            return;
        };
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
            Connection::Live(session) | Connection::Reconnecting(session) => session.is_closed(),
            _ => false,
        }
    }

    fn forget_socket(&mut self) {
        self.connection = Connection::Lost;
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
        if self.turn_ended.is_some_and(|at| at.elapsed() < TAIL_LAG) {
            return;
        }
        self.turn_ended = None;
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

    /// Whether the poll should read this chat's transcript tail: a delegate
    /// no `Turn idle` has reached — a relaunch, a socket that was down, or
    /// a tail that lagged past `TAIL_LAG` — and not read in `PROBE_EVERY`.
    /// Nothing this window sent is in flight.
    pub(crate) fn wants_probe(&self) -> bool {
        self.is_delegate()
            && !self.closed
            && !self.streaming
            && !self.has_running_tool()
            && self.live.is_empty()
            && self.turn_ended.is_none()
            && self.probed_at.is_none_or(|at| at.elapsed() >= PROBE_EVERY)
    }

    /// The tail was read. `ended` is a turn-ending record last in the file:
    /// the kernel is idle here whatever the stream said.
    pub(crate) fn probed(&mut self, ended: bool) {
        self.probed_at = Some(Instant::now());
        if ended && self.turn_ended.is_none() {
            self.turn_open = false;
            self.turn_ended = Some(Instant::now());
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

    fn has_running_tool(&self) -> bool {
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

    /// Push local bubbles the kernel never stored, then title from them.
    pub(crate) fn sync_kernel_history(&mut self) {
        if let Some(sid) = self.agent_session.clone() {
            crate::kernel::seed_transcript(&self.place(), &sid, &self.items);
        }
        self.take_title_from_first_prompt();
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
            self.items = items;
            self.transcript = transcript::State::default();
            self.take_title_from_first_prompt();
            self.flush();
            return;
        }
        self.merge_tool_diffs(&items);
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
        matches!(
            self.connection,
            Connection::Connecting | Connection::Reconnecting(_)
        )
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
        !self.closed && !self.agent_gone()
    }

    /// A local agent whose folder is no longer on disk.
    pub fn agent_gone(&self) -> bool {
        self.host.is_none()
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildSummary {
    pub id: u64,
    /// The kernel's agent id, which `say` lines name.
    pub kernel_id: Option<String>,
    pub title: String,
    pub state: ChildState,
}

impl ChatSession {
    /// The state a parent shows for this chat.
    pub fn child_state(&self) -> ChildState {
        if self.busy() {
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
                .unwrap_or_else(|| head.to_string())
        };
        match tail {
            Some(t) => format!("{name} → {t}"),
            None => name,
        }
    }
}

impl ChatSession {
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
        if self.streaming || self.has_running_tool() {
            self.steer(content);
            return;
        }
        self.prompt(content);
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
        self.items.push(ChatItem::User(content.message()));
        self.updated = SystemTime::now();
        self.flush();
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
        self.land_turn(&content);
    }

    /// A prompt another client sent to this agent — the caller's words on
    /// a live call, the phone — as the kernel recorded it. This window's own
    /// prompt comes back the same way a moment after it was sent, so a line
    /// that matches the last card is the echo and is dropped. Anything else
    /// is a turn this window did not start: the card goes up, and the pane
    /// is working until the kernel says idle.
    fn foreign_prompt(&mut self, text: String, attachments: Vec<String>, ts: i64, channel: String) {
        let squash = |s: &str| s.split_whitespace().collect::<String>();
        let echo = self
            .items
            .iter()
            .rev()
            .find_map(|item| match item {
                ChatItem::User(message) => Some(message),
                _ => None,
            })
            .is_some_and(|last| {
                squash(&last.text) == squash(&text)
                    && last.sent_at.is_none_or(|sent| (ts - sent).abs() < 120_000)
            });
        if echo {
            return;
        }
        let mut message = crate::model::attachment::UserMessage::from(text);
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

    /// Put the user card in the pane this frame — before the kernel
    /// answers, and before a reconnect finishes.
    fn land_turn(&mut self, content: &Prompt) {
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
        if let Some(prompt) = self.permission.take() {
            self.dismiss_permission(prompt, false);
        }
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
        if self.busy() {
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
        self.rewind_to = Some(start);
        session.rewind(turn, files);
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

    /// Answer the pending permission prompt with the chosen option id.
    pub fn respond_permission(&mut self, option_id: String) {
        let Some(prompt) = self.permission.take() else {
            return;
        };
        if let Some(request_id) = prompt.kernel_approval {
            let approved = option_id.contains("allow");
            if let Connection::Live(session) = &self.connection
                && let Err(e) = session.approval(&request_id, approved)
            {
                self.notice(true, &format!("approval failed: {e:#}"));
                self.flush();
            }
            return;
        }
        prompt
            .reply
            .send(RequestPermissionResponse::selected(option_id));
    }

    /// Flip whether the pending prompt is answered once or for good.
    pub fn toggle_permission_always(&mut self) {
        if let Some(prompt) = &mut self.permission {
            prompt.always = !prompt.always;
        }
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
                // The card folds to one line; the answer is a line of yours
                // under it.
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
                if !said.is_empty() {
                    self.items.push(ChatItem::User(UserMessage::from(said)));
                }
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
        match event {
            Event::History(replay) => {
                self.adopt_history(replay.items);
                if let Some(model) = replay.model {
                    self.model = Some(model);
                }
            }
            Event::Images(images) => {
                self.items.push(ChatItem::From {
                    who: String::new(),
                    text: String::new(),
                    images,
                });
                self.flush();
            }
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
                channel,
            } => self.foreign_prompt(text, attachments, ts, channel),
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
                } else {
                    self.provider_missing = Some(provider);
                }
            }
            Event::Rewound {
                dropped,
                restored,
                pending,
            } => {
                // The second frame of a rewind with files: the restore is
                // done. The chat was already cut on the first; only the
                // line changes.
                if let Some(r) = restored.as_deref()
                    && matches!(self.items.last(), Some(ChatItem::Notice { text, .. }) if text.starts_with("rewound:") && text.ends_with(RESTORING))
                {
                    self.items.pop();
                    self.notice(
                        false,
                        &format!("rewound: {dropped} transcript lines cut; project back to {r}"),
                    );
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
                    Some(r) => {
                        format!("rewound: {dropped} transcript lines cut; project back to {r}")
                    }
                    None if pending => {
                        format!("rewound: {dropped} transcript lines cut; {RESTORING}")
                    }
                    None => format!("rewound: {dropped} transcript lines cut; files untouched"),
                };
                self.notice(false, &what);
                self.flush();
            }
            Event::Handshake { protocol, kernel } => {
                let ok = protocol.is_some_and(|p| p >= crate::kernel::PROTOCOL);
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
            Event::AssistantFinal(text) => {
                self.finish_thinking();
                let text = text.trim_matches('\n').to_string();
                // The deltas of this step built an item: the recorded line is
                // the same words, whole. Replace, never append.
                if let Some(ix) = self.streaming_agent.take() {
                    if let Some(ChatItem::Agent(body)) = self.items.get_mut(ix) {
                        if !text.is_empty() {
                            *body = text;
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
                self.notice(false, &text);
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
            Event::Refused(detail) => {
                self.rewind_to = None;
                self.notice(true, &detail);
                self.flush();
                // The kernel no longer has this agent: the row keeps its
                // words and stops asking for a socket.
                if detail.starts_with("no agent ") {
                    self.close();
                }
            }
            Event::NeedApproval { request_id, title } => {
                self.turn_alive();
                self.open_kernel_approval(request_id, title);
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
            Event::Citations(sources) => self.bind_sources(sources),
            Event::Permission(request, reply) => self.open_permission(request, reply),
            Event::Working(secs) => {
                self.working = Some((secs, Instant::now()));
                self.turn_open = true;
                self.turn_ended = None;
            }
            Event::TurnDone(result) => {
                self.working = None;
                self.stamp_worked();
                self.voice_answer();
                if let Some(prompt) = self.permission.take() {
                    self.dismiss_permission(prompt, false);
                }
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
                match result {
                    Ok(StopReason::EndTurn) => {
                        // Cursor just ends. A lone "done" line is extra.
                        // Only speak when the kernel never answered at all —
                        // and not when this window asked it to stop: the
                        // kernel's own `interrupted` line ("Stopped by you")
                        // follows on the tail.
                        if !stopped && !self.busy() && ended_on_user(&self.items) {
                            // Kernel turn failed before any token (missing
                            // agent.md, bad model, no key). Idle used to
                            // clear the thinking row and leave a blank pane.
                            self.notice(true, "no reply from the kernel");
                        }
                    }
                    Ok(StopReason::Cancelled) => {
                        self.fail_running_tools();
                        self.notice(false, "Stopped.");
                    }
                    Ok(StopReason::Refusal) => self.notice(false, "the agent refused to continue"),
                    Ok(StopReason::MaxTokens) => self.notice(false, "stopped: max tokens"),
                    Ok(StopReason::MaxTurnRequests) => self.notice(false, "stopped: max steps"),
                    Ok(other) => self.notice(false, &format!("stopped: {other:?}")),
                    Err(e) => {
                        self.fail_running_tools();
                        self.notice(true, &format!("turn failed: {}", acp::error_text(&e)));
                    }
                }
                self.flush();
                self.drain();
            }
            Event::Reconnecting => {
                self.connection = match std::mem::replace(&mut self.connection, Connection::Lost) {
                    Connection::Live(session) => Connection::Reconnecting(session),
                    other => other,
                };
            }
            Event::Reconnected => {
                self.connection = match std::mem::replace(&mut self.connection, Connection::Lost) {
                    Connection::Reconnecting(session) => Connection::Live(session),
                    other => other,
                };
                self.drain();
            }
            Event::Closed => {
                if let Some(prompt) = self.permission.take() {
                    prompt.reply.send(RequestPermissionResponse::cancelled());
                }
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
        }
    }

    /// Try Live: ask the kernel for the agent's screen now.
    pub fn request_screen(&self) {
        if let Connection::Live(session) = &self.connection {
            session.request_screen();
        }
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
            .filter(|n| n.inbox && n.status == "pending" && n.do_kind != "steer")
            .count()
    }

    pub fn plan_op(&mut self, node: u64, op: &str, text: &str) {
        if let Connection::Live(session) = &self.connection {
            session.plan_op(node, op, text);
        }
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
                if let Some(ChatItem::Agent(body)) = self.items.last_mut() {
                    merge_stream_text(body, &text);
                    self.streaming_agent = Some(self.items.len() - 1);
                } else if !text.is_empty() {
                    self.items.push(ChatItem::Agent(text));
                    self.streaming_agent = Some(self.items.len() - 1);
                }
            }
            SessionUpdate::AgentThoughtChunk(chunk) => {
                let text = content_text(&chunk.content);
                if let Some(ChatItem::Thinking {
                    text: body,
                    done: false,
                    ..
                }) = self.items.last_mut()
                {
                    merge_stream_text(body, &text);
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
                if let Some(ix) = self.items.iter().rposition(
                    |item| matches!(item, ChatItem::Tool { id: tool, .. } if *tool == id),
                ) {
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
                    self.items.push(ChatItem::Tool {
                        id,
                        kind: call.kind,
                        label: call.title,
                        status: tool_status(call.status),
                        output,
                        diff,
                        child_session: None,
                        secs: None,
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

    fn open_permission(
        &mut self,
        request: RequestPermissionRequest,
        reply: Reply<RequestPermissionResponse>,
    ) {
        let options: Vec<Choice> = request
            .options
            .into_iter()
            .map(|opt| Choice {
                id: opt.option_id.to_string(),
                name: opt.name,
                kind: opt.kind,
            })
            .collect();
        if options.is_empty() {
            reply.send(RequestPermissionResponse::cancelled());
            return;
        }
        // A replaced prompt must still be answered — an unanswered
        // reply hangs the agent.
        if let Some(previous) = self.permission.take() {
            self.dismiss_permission(previous, false);
        }
        let title = request
            .tool_call
            .fields
            .title
            .clone()
            .unwrap_or_else(|| "Permission required".to_owned());
        self.permission = Some(PermissionPrompt {
            title,
            options,
            always: false,
            reply,
            kernel_approval: None,
        });
    }

    fn open_kernel_approval(&mut self, request_id: String, title: String) {
        if let Some(previous) = self.permission.take() {
            self.dismiss_permission(previous, false);
        }
        self.permission = Some(PermissionPrompt {
            title,
            options: vec![
                Choice {
                    id: "reject".into(),
                    name: "Don't allow".into(),
                    kind: PermissionOptionKind::RejectOnce,
                },
                Choice {
                    id: "allow".into(),
                    name: "Allow".into(),
                    kind: PermissionOptionKind::AllowOnce,
                },
            ],
            always: false,
            reply: Reply::ignore(),
            kernel_approval: Some(request_id),
        });
    }

    fn dismiss_permission(&mut self, prompt: PermissionPrompt, approved: bool) {
        if let Some(request_id) = prompt.kernel_approval {
            if let Connection::Live(session) = &self.connection {
                let _ = session.approval(&request_id, approved);
            }
            return;
        }
        prompt.reply.send(RequestPermissionResponse::cancelled());
    }

    fn bind_sources(&mut self, sources: Vec<acp::Citation>) {
        let Some(ChatItem::Agent(text)) = self
            .items
            .iter_mut()
            .rev()
            .find(|item| matches!(item, ChatItem::Agent(_)))
        else {
            return;
        };
        for source in sources {
            write_source(text, &source.url, &source.title);
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
        // The kernel's page nudge is a standing state, not news each turn:
        // one line, at the latest turn it applies to. An earlier copy goes.
        if !failed && is_page_nudge(text) {
            self.items.retain(|item| {
                !matches!(item, ChatItem::Notice { text: t, failed: false } if is_page_nudge(t))
            });
        }
        self.items.push(ChatItem::Notice {
            text: text.to_owned(),
            failed,
        });
    }

    fn finish_thinking(&mut self) {
        if let Some(ChatItem::Thinking { done, secs, .. }) = self.items.last_mut() {
            if !*done {
                *done = true;
                *secs = self
                    .thought_at
                    .and_then(|at| at.elapsed().ok())
                    .map(|d| d.as_secs() as u32);
            }
        }
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

fn write_source(text: &mut String, url: &str, title: &str) {
    if url.is_empty() || text.contains(url) {
        return;
    }
    let label = if title.is_empty() {
        host_of(url)
    } else {
        title.to_string()
    };
    let open = format!("[{label}](");
    if let Some(start) = text.find(&open) {
        let dest = start + open.len();
        if let Some(end) = text[dest..].find(')') {
            let dest_end = dest + end;
            if !markdown::is_url(&text[dest..dest_end]) {
                text.replace_range(dest..dest_end, url);
            }
            return;
        }
    }
    if let Some(at) = text
        .rfind("**Sources:**")
        .or_else(|| text.rfind("Sources:"))
    {
        if let Some(nl) = text[at..].find('\n') {
            text.insert_str(at + nl, &format!(" · [{label}]({url})"));
        } else {
            text.push_str(&format!(" · [{label}]({url})"));
        }
        return;
    }
    text.push_str(&format!("\n\n**Sources:** [{label}]({url})"));
}

fn host_of(url: &str) -> String {
    url.trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.")
        .split('/')
        .next()
        .filter(|host| !host.is_empty())
        .unwrap_or(url)
        .to_string()
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
                        // A fault is news; a retry in progress is not, and
                        // a race the next try wins is not worth a line.
                        if !again || chat.reconnect_attempt >= 1 && chat.reconnect_attempt % 5 == 0 {
                            chat.notice(true, &format!("connection failed: {e:#}"));
                        }
                    });
                    let retry = workspace
                        .session(id)
                        .is_some_and(|chat| chat.attach_gen == attach_gen && chat.resumable());
                    if again && retry {
                        workspace.schedule_reconnect(id, cx);
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
                workspace.with_session(id, cx, |chat| {
                    for event in batch {
                        ended |= matches!(event, Event::TurnDone(_));
                        match event {
                            Event::Show { path, title, kind } => {
                                shown.push((path, title, kind));
                            }
                            Event::StoreChanged(_) => store_moved = true,
                            // Kernel rows come and go in order; keep it.
                            event @ (Event::Open { .. }
                            | Event::Hide { .. }
                            | Event::Browser { .. }
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
                for (path, title, kind) in shown {
                    workspace.open_shown(id, path, title, kind, None, None, cx);
                }
                for event in surfaces {
                    match event {
                        Event::Open {
                            path,
                            title,
                            kind,
                            cwd,
                            url,
                        } => workspace.open_shown(id, path, title, kind, cwd, url, cx),
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
                } else if matches!(
                    chat.connection,
                    Connection::Live(_) | Connection::Reconnecting(_)
                ) {
                    let busy = chat.streaming || chat.has_running_tool();
                    let queued = !chat.queue.is_empty();
                    chat.forget_socket();
                    if busy && !queued {
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
                workspace.schedule_reconnect(id, cx);
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
    !["failed to start", "is not a file", "is not a directory", "bad kernel url"]
        .iter()
        .any(|fault| text.contains(fault))
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

/// Whether a notice marks the end of an interrupted turn.
pub fn is_interrupt_notice(text: &str) -> bool {
    text == STOPPED_BY_YOU || text.starts_with("Interrupted")
}
