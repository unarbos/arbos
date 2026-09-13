use serde::{Deserialize, Serialize};

/// One JSONL line. The only persisted projection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Identity: the 1-based physical line in transcript.jsonl. Not
    /// written (the position is the identity); set by `load_transcript`,
    /// 0 on an event not yet on disk. Stable because the file is
    /// append-only and a bad line still occupies its line. On the wire
    /// when non-zero, so a client can tell a transcript line (replayed or
    /// tailed) from a live emit; never written to the file itself.
    #[serde(default, skip_serializing_if = "seq_is_zero")]
    pub seq: u64,
    pub ts: i64,
    #[serde(flatten)]
    pub kind: EventKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventKind {
    Wake {
        wake: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    User {
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<String>,
    },
    Assistant {
        text: String,
        /// Provider reasoning blocks that must go back with this message on
        /// the next call (Gemini 3 thought signatures, Anthropic thinking
        /// blocks via OpenRouter). Opaque; never rendered.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_details: Option<serde_json::Value>,
    },
    Thinking {
        text: String,
    },
    Tool(ToolRec),
    Ask {
        question: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        options: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
    },
    Answer {
        text: String,
    },
    Say {
        from: String,
        text: String,
    },
    Approval {
        call_id: String,
        tool: String,
        allowed: bool,
    },
    TurnComplete {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
    },
    /// Stage one of context management. Tool bodies on lines <= `through`
    /// render to the model as a one-line cite instead of their text. No
    /// model call; nothing is lost on disk.
    Fold {
        through: u64,
        tokens: u64,
    },
    /// Stage two. Events on lines `lo..=hi` are replaced in the model's
    /// view by `summary`, rendered at the position of `lo`. The events stay
    /// verbatim in transcript.jsonl.
    Compaction {
        lo: u64,
        hi: u64,
        summary: String,
        tokens_before: u64,
        tokens_after: u64,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        model: String,
    },
    Interrupted {
        detail: String,
    },
    Notice {
        text: String,
        #[serde(default)]
        failed: bool,
    },
    /// Older transcripts hid everything before this line. No longer written;
    /// `compact::visible` treats one as a compaction of lines `1..seq-1`
    /// with a fixed summary, so those logs still project the same way.
    WindowReset {},
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRec {
    pub name: String,
    pub call_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Full body on disk. The model sees an evicted tail plus this cite.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child: Option<String>,
    /// Image files this call produced or read. Projected to the model as
    /// real image parts, not text. Paths only: the bytes stay on disk.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
    /// Display diff for the chat card. The model never sees this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Usage {
    pub used: u64,
    pub size: u64,
}

fn seq_is_zero(n: &u64) -> bool {
    *n == 0
}

impl Event {
    pub fn new(kind: EventKind) -> Self {
        Self {
            seq: 0,
            ts: crate::now_ms(),
            kind,
        }
    }

    /// A turn ends here: the model finished, or was stopped.
    pub fn ends_turn(&self) -> bool {
        matches!(
            self.kind,
            EventKind::TurnComplete { .. } | EventKind::Interrupted { .. }
        )
    }

    pub fn is_turn_complete(&self) -> bool {
        matches!(self.kind, EventKind::TurnComplete { .. })
    }

    pub fn is_wake(&self) -> bool {
        matches!(self.kind, EventKind::Wake { .. })
    }

    pub fn user_text(&self) -> Option<&str> {
        match &self.kind {
            EventKind::User { text, .. }
            | EventKind::Wake {
                text: Some(text), ..
            } => Some(text),
            EventKind::Answer { text } => Some(text),
            _ => None,
        }
    }
}
