use serde::{Deserialize, Serialize};

/// One JSONL line. The only persisted projection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Identity: the 1-based physical line in transcript.jsonl. Not
    /// written to the file (the position is the identity); set by
    /// `load_transcript`, 0 on an event not yet on disk. Stable because the
    /// file is append-only and a bad line still occupies its line.
    ///
    /// On the wire it is written when nonzero, so a client can tell a
    /// transcript line (the record) from a live emit (streaming text, a
    /// tool starting) that has no line yet. A line re-read from disk
    /// carries the number the reader gave it, so the field is never
    /// authoritative in the file.
    #[serde(default, skip_serializing_if = "is_zero")]
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
        /// On a spawned child's first wake: the mission as the parent
        /// wrote it, alone. `text` has it inside the kernel's framing
        /// ("You were spawned by … for this mission: …"); a client shows
        /// this one as the prompt, no string cut.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        brief: Option<String>,
    },
    User {
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<String>,
        /// How the words arrived: `voice` (a call) or `text` (typed).
        /// Absent on lines from before the key existed.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        channel: String,
        /// The client: `phone` | `desktop` | `cli`.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        device: String,
    },
    Assistant {
        text: String,
        /// The model step within the turn that said it, 1-based: one step
        /// is one model call (its thinking, its text, its tool calls). A
        /// window pairs the streamed `assistant_delta`s tagged N with the
        /// settled line tagged N (M-14). 0 on lines from before the field:
        /// pair by adjacency.
        #[serde(default, skip_serializing_if = "is_zero_u64")]
        step: u64,
        /// Provider reasoning blocks that must go back with this message on
        /// the next call (Gemini 3 thought signatures, Anthropic thinking
        /// blocks via OpenRouter). Opaque; never rendered.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_details: Option<serde_json::Value>,
    },
    Thinking {
        text: String,
        /// On a settled record (one per model step that reasoned): how
        /// long the thinking streamed, from its first token to its last.
        /// Absent on a live delta and on lines from before the key.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        secs: Option<u64>,
        /// The model step within the turn, as on `Assistant`.
        #[serde(default, skip_serializing_if = "is_zero_u64")]
        step: u64,
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
    /// A kernel reminder for the model, written between turns ("project
    /// page not updated last turn"). For the model it reads like a kernel
    /// notice; a window draws it dim, as an aside, never as a failure.
    Nudge {
        text: String,
        /// The nudge in a few words ("project page not updated"), for a
        /// client's one-line notice; `text` is the full reminder the model
        /// reads. Absent on lines from before the key existed.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        reason: String,
    },
    /// An attached image the selected model could not see, described in
    /// words by `model` (a vision-capable one) so the turn went on with the
    /// user's picture in text. `path` is the attachment as the `user` or
    /// `tool` line names it. Projections show `text` in place of the
    /// pixels; clients draw it inside the message card, never as a line.
    ImageDescribed {
        path: String,
        model: String,
        text: String,
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
    /// The model step within the turn that made the call, as on
    /// `Assistant`: a window attaching mid-turn puts a late final in order.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub step: u64,
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
    /// What the call does, in the model's few words (`bash`'s
    /// `description`: "List repo contents and recent commits"), for the
    /// line a window shows instead of "Ran 1 command". Absent when the
    /// model gave none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Usage {
    pub used: u64,
    pub size: u64,
    /// What the turn cost in US dollars, summed over its model calls, when
    /// the provider reports it (OpenRouter does).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    /// Prompt tokens read from the provider's cache over the turn's model
    /// calls (`usage.prompt_tokens_details.cached_tokens`), when reported.
    /// Zero with a cache-capable model means the breakpoints did not hit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached: Option<u64>,
}

fn is_zero(n: &u64) -> bool {
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

fn is_zero_u64(n: &u64) -> bool {
    *n == 0
}
