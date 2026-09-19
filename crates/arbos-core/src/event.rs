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
        /// The model that answered this turn's last step — the effective
        /// one, which a fallback or a blocked family makes different from
        /// the configured one. The window's chip names this, not the
        /// configuration; the transcript keeps who answered. None when no
        /// chat model was called (a Jev-only turn, a refusal).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
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
    /// A glance at the output for a small screen: the first ~400
    /// characters, or head and tail when the body is long, the error
    /// first when there is one. The phone folds tool calls into one line
    /// and could show only labels when opened (F14). `body` stays the
    /// full text; a client that has it needs nothing here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

impl ToolRec {
    /// The glance, from `error` and `body`.
    pub fn digest(&self) -> Option<String> {
        let text = match (&self.error, &self.body) {
            (Some(e), Some(b)) if b.trim() != e.trim() && !b.trim().is_empty() => {
                format!("{e}\n{b}")
            }
            (Some(e), _) => e.clone(),
            (None, Some(b)) => b.clone(),
            (None, None) => return None,
        };
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        Some(tool_digest(text))
    }

    /// `output` filled from the record itself when absent (a record from
    /// before the field existed, replayed).
    pub fn with_output(mut self) -> Self {
        if self.output.is_none() {
            self.output = self.digest();
        }
        self
    }
}

/// How much of a tool's output a glance shows.
pub const DIGEST_CHARS: usize = 400;

/// The first `DIGEST_CHARS` characters of `text`; past that, the first
/// half and the last half (whole lines where they fit) around a `…` line,
/// so both the start and the end of a long run are seen.
pub fn tool_digest(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= DIGEST_CHARS {
        return text.to_string();
    }
    let half = DIGEST_CHARS / 2 - 2;
    let head: String = text.chars().take(half).collect();
    let tail_start = text.chars().count() - half;
    let tail: String = text.chars().skip(tail_start).collect();
    // Cut on line ends where a line end is near, so the glance is not
    // two half-words.
    let head = match head.rfind('\n') {
        Some(i) if i > half / 2 => head[..i].to_string(),
        _ => head,
    };
    let tail = match tail.find('\n') {
        Some(i) if i < half / 2 => tail[i + 1..].to_string(),
        _ => tail,
    };
    format!("{}\n…\n{}", head.trim_end(), tail.trim_start())
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

#[cfg(test)]
mod digest_tests {
    use super::*;

    fn rec(body: Option<&str>, error: Option<&str>) -> ToolRec {
        ToolRec {
            name: "bash".into(),
            call_id: "c1".into(),
            step: 1,
            paths: vec![],
            started: None,
            ended: None,
            result_size: None,
            error: error.map(str::to_string),
            body: body.map(str::to_string),
            args: None,
            child: None,
            images: vec![],
            diff: None,
            label: None,
            output: None,
        }
    }

    #[test]
    fn a_short_output_is_itself_a_long_one_shows_head_and_tail_and_an_error_leads() {
        assert_eq!(rec(Some("ok\n"), None).digest().as_deref(), Some("ok"));
        assert_eq!(rec(None, None).digest(), None);
        let lines: Vec<String> = (1..=200).map(|i| format!("line {i} of the run")).collect();
        let long = lines.join("\n");
        let d = tool_digest(&long);
        assert!(
            d.chars().count() <= DIGEST_CHARS + 8,
            "{}",
            d.chars().count()
        );
        assert!(d.starts_with("line 1 of the run"), "{d}");
        assert!(d.ends_with("line 200 of the run"), "{d}");
        assert!(d.contains("\n…\n"), "{d}");
        assert!(!d.contains("line 100 "), "the middle is cut: {d}");
        let e = rec(Some("stdout so far"), Some("exit 1: no such file"))
            .digest()
            .unwrap();
        assert!(e.starts_with("exit 1: no such file\n"), "{e}");
        let filled = rec(Some("hello"), None).with_output();
        assert_eq!(filled.output.as_deref(), Some("hello"));
    }
}
