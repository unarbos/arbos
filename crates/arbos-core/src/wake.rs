use crate::agent::AgentId;

/// Why a turn should run. Idle agents have none.
///
/// Derived from a plan node whose moment came, or made by the kernel for
/// housekeeping (`Serve`, `Compact`).
#[derive(Debug, Clone)]
pub struct Wake {
    pub agent: AgentId,
    pub kind: WakeKind,
    pub text: Option<String>,
    pub attachments: Vec<String>,
    /// Inject into a live job at the next tool boundary.
    pub steer: bool,
    /// The plan node this turn discharges.
    pub node: Option<crate::NodeId>,
    /// Reply budget: how many agent-to-agent requests may chain from this
    /// turn before they fall back to notes.
    pub hops: u8,
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
            node: None,
            hops: 0,
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
