use crate::agent::AgentId;

/// Why a turn should run. Idle agents have none.
///
/// Made from an inbox file whose moment came (a prompt, a peer's words, a
/// subscription firing), or by the kernel for housekeeping (`Serve`,
/// `Compact`).
#[derive(Debug, Clone)]
pub struct Wake {
    pub agent: AgentId,
    pub kind: WakeKind,
    pub text: Option<String>,
    pub attachments: Vec<String>,
    /// Inject into a live job at the next tool boundary.
    pub steer: bool,
    /// Reply budget: how many agent-to-agent requests may chain from this
    /// turn before they fall back to notes.
    pub hops: u8,
    /// For a person's words: how they arrived (`voice` | `text`) and from
    /// which client (`phone` | `desktop` | `cli`). Written onto the
    /// transcript's `user` line. Empty otherwise.
    pub channel: String,
    pub device: String,
    /// A model for this turn only (the composer's "switch to <vision
    /// model> for this turn"). Empty: the agent's own.
    pub model: String,
    /// The sender's label for this turn (`say title=`), or empty.
    pub title: String,
    /// A spawned child's mission as the parent wrote it — the brief alone,
    /// without the kernel's framing around it in `text`. Empty otherwise.
    pub brief: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeKind {
    User,
    Say,
    Plan,
    Serve,
    /// A detached bash job finished.
    Job,
    /// Compact the transcript, then stop. No model step.
    Compact,
    /// Root's first turn in a fresh place: read the folder, seed the
    /// context file and the page, greet. No spawns, no questions.
    Kickoff,
}

impl WakeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Say => "say",
            Self::Plan => "plan",
            Self::Serve => "serve",
            Self::Job => "job",
            Self::Compact => "compact",
            Self::Kickoff => "kickoff",
        }
    }
}

impl Wake {
    pub fn new(agent: impl Into<String>, kind: WakeKind, text: Option<String>) -> Self {
        Self {
            agent: AgentId::new(agent),
            kind,
            text,
            attachments: Vec::new(),
            steer: false,
            hops: 0,
            channel: String::new(),
            device: String::new(),
            model: String::new(),
            title: String::new(),
            brief: String::new(),
        }
    }

    pub fn user(agent: impl Into<String>, text: impl Into<String>) -> Self {
        Self::new(agent, WakeKind::User, Some(text.into()))
    }

    pub fn serve(agent: impl Into<String>) -> Self {
        Self::new(agent, WakeKind::Serve, None)
    }

    pub fn compact(agent: impl Into<String>) -> Self {
        Self::new(agent, WakeKind::Compact, None)
    }
}
