//! The project agent driver: one `pi --mode rpc` child per Project (D65), one pi
//! session per chat, the `route` tool validated here, streams folded into the
//! chat doc, subagents through the `HarnessAdapter` registry. Hard limits that
//! remain (turn deadline, context window, spend, tool timeouts) are announced
//! in the chat — never a silent stop.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use tokio::sync::{broadcast, Mutex, Semaphore};

use arbos_core::approval::ApprovalOutcome;
use arbos_core::doc::{CommandStatus, CommandView, MessageStatus};
use arbos_core::ids::ulid;
use arbos_core::route::{pick_harness, validate, Harness, ValidRoute};
use arbos_core::rpc::{AssistantEvent, Command as RpcCommand, Event};
use arbos_core::widget::Widget;
use arbos_core::{TOOL_OUTPUT_SUMMARY_MAX, TURN_DEADLINE_SECS};

use crate::config::Config;
use crate::harness::{
    AgentEvent, ClaudeCodeAdapter, HarnessAdapter, PiAdapter, PrimeAdapter, StartRequest,
};
use crate::llm::{extract_return_contract, Llm};
use crate::memory::Memory;
use crate::rpc_child::RpcChild;
use crate::skills::{self, SkillsIndex};
use crate::store::Store;
use crate::sync::{SharedHost, SyncHub};

/// Streams one string field (`reply`, `ack`) out of the `route` tool's partial JSON arguments.
pub struct ReplyExtractor {
    key: &'static str,
    buf: String,
    pos: usize,
    in_string: bool,
    done: bool,
    pending_escape: Option<String>,
}

impl Default for ReplyExtractor {
    fn default() -> Self {
        Self::for_key("reply")
    }
}

impl ReplyExtractor {
    pub fn for_key(key: &'static str) -> Self {
        Self {
            key,
            buf: String::new(),
            pos: 0,
            in_string: false,
            done: false,
            pending_escape: None,
        }
    }

    /// The string closed (or was null): nothing more will come.
    pub fn is_done(&self) -> bool {
        self.done
    }

    /// The `action` value seen so far in the same arguments, when it has streamed already.
    pub fn action_seen(&self) -> Option<&str> {
        let k = self.buf.find("\"action\"")?;
        let after = &self.buf[k + 8..];
        let colon = after.find(':')?;
        let rest = after[colon + 1..].trim_start();
        let rest = rest.strip_prefix('"')?;
        let end = rest.find('"')?;
        Some(&rest[..end])
    }

    /// Feed one `toolcall_delta`; returns newly decoded text of the field.
    pub fn feed(&mut self, delta: &str) -> String {
        if self.done {
            return String::new();
        }
        self.buf.push_str(delta);
        let mut out = String::new();
        if !self.in_string {
            let needle = format!("\"{}\"", self.key);
            let Some(k) = self.buf.find(&needle) else {
                return out;
            };
            let after = &self.buf[k + needle.len()..];
            let Some(colon) = after.find(':') else {
                return out;
            };
            let rest = &after[colon + 1..];
            let trimmed = rest.trim_start();
            if trimmed.is_empty() {
                return out;
            }
            if !trimmed.starts_with('"') {
                self.done = true; // null reply
                return out;
            }
            let q = self.buf.len() - trimmed.len();
            self.pos = q + 1;
            self.in_string = true;
        }
        let bytes = self.buf.as_bytes();
        let mut i = self.pos;
        let mut esc = self.pending_escape.take().unwrap_or_default();
        while i < bytes.len() {
            let c = bytes[i] as char;
            if !esc.is_empty() {
                esc.push(c);
                if esc == "\\u" || (esc.starts_with("\\u") && esc.len() < 6) {
                    i += 1;
                    continue;
                }
                match esc.as_str() {
                    "\\n" => out.push('\n'),
                    "\\t" => out.push('\t'),
                    "\\r" => out.push('\r'),
                    "\\\"" => out.push('"'),
                    "\\\\" => out.push('\\'),
                    "\\/" => out.push('/'),
                    s if s.starts_with("\\u") && s.len() == 6 => {
                        if let Some(ch) = u32::from_str_radix(&s[2..], 16)
                            .ok()
                            .and_then(char::from_u32)
                        {
                            out.push(ch);
                        }
                    }
                    other => out.push_str(&other[1..]),
                }
                esc.clear();
                i += 1;
                continue;
            }
            if c == '\\' {
                esc.push('\\');
                i += 1;
                continue;
            }
            if c == '"' {
                self.done = true;
                i += 1;
                break;
            }
            // Multi-byte UTF-8: copy whole chars.
            let ch_len = utf8_len(bytes[i]);
            if i + ch_len > bytes.len() {
                break;
            }
            out.push_str(&self.buf[i..i + ch_len]);
            i += ch_len;
        }
        self.pos = i;
        if !esc.is_empty() {
            self.pending_escape = Some(esc);
        }
        out
    }
}

/// Cheap post-check on main-agent prose as it streams into the doc (demo review v2): drops
/// leading boilerplate lines ("Called 3 tools", "Running the tests…", "Route accepted: …"),
/// leading thought-aloud sentences ("The user is asking…", "I should reply…"), trailing
/// tool-narration ("Checking the latest…"), glued ".Word" → ". Word", and whole
/// ```json fences. Text is held back only until a line boundary, so streaming stays live.
#[derive(Default)]
pub struct ProseFilter {
    pending: String,
    in_fence: bool,
    fence_hidden: bool,
    started: bool,
}

impl ProseFilter {
    /// First-line boilerplate: narration about the mechanics, never an answer.
    fn is_boilerplate(line: &str) -> bool {
        let l = line
            .trim()
            .trim_start_matches(['*', '_', '#', '>', '-', ' '])
            .to_ascii_lowercase();
        if l.is_empty() {
            return true;
        }
        let starts = |p: &[&str]| p.iter().any(|x| l.starts_with(x));
        let tool_count = {
            // "called 3 tools", "i ran 2 tool calls.", "(4 tool calls)"
            let t = l
                .trim_start_matches(['(', 'i', ' '])
                .trim_start_matches("i ");
            let t = t
                .trim_start_matches("called ")
                .trim_start_matches("ran ")
                .trim_start_matches("used ")
                .trim_start_matches("made ")
                .trim_start_matches("executed ");
            let mut it = t.splitn(2, ' ');
            matches!((it.next().map(|n| n.parse::<u32>().is_ok()), it.next()), (Some(true), Some(rest)) if rest.starts_with("tool"))
        };
        let mechanics = starts(&[
            "tool call",
            "tool calls",
            "tools called",
            "route accepted",
            "routing:",
            "route:",
        ]);
        // Verb-led narration is short ("Running the tests…"); a real sentence that happens to
        // start with the same word runs long and stays.
        let narration = l.chars().count() <= 90
            && starts(&[
                "running ",
                "executing ",
                "calling ",
                "invoking ",
                "let me check",
                "let me run",
                "let me look",
                "i'll check",
                "i will check",
                "i'll run",
                "i will run",
                "checking ",
                "reading the file",
                "first, i",
                "first i",
                "now i'll",
                "now i will",
            ]);
        tool_count || mechanics || narration
    }

    /// A sentence that is the model talking to itself, not to the user.
    fn is_thought_sentence(sentence: &str) -> bool {
        let l = sentence
            .trim()
            .trim_start_matches(['*', '_', '#', '>', '-', ' '])
            .trim_end_matches(['.', '!', '?', '…'])
            .to_ascii_lowercase();
        if l.is_empty() {
            return true;
        }
        let starts = |p: &[&str]| p.iter().any(|x| l.starts_with(x));
        // Keep these narrow: a real reply can start with "I should…" or "I need to…".
        starts(&[
            "the user is",
            "the user asked",
            "the user wants",
            "the user would",
            "i should reply",
            "i should answer",
            "i will reply",
            "i'll reply",
            "i will answer",
            "i'll answer",
            "let me think",
            "thinking:",
            "this is a simple",
            "this is a factual",
            "this is straightforward",
            "this is an easy",
            // Routing / plan-aloud (clone and similar act turns).
            "that's real work",
            "the instructions say",
            "the instructions",
            "prefer act over",
            "a git clone",
            "i'll act",
            "i will act",
            "i should not use",
            "ack first",
            "clone into the project",
        ])
    }

    /// Split off the first sentence. Treats ".Austin" (period, no space, capital) as a boundary
    /// so a glued thought+answer still separates. Skips `e.g.` / `i.e.`-style abbreviations.
    fn split_first_sentence(s: &str) -> Option<(&str, &str)> {
        let bytes = s.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            if b == b'.' || b == b'!' || b == b'?' {
                // "e.g." / "i.e.": period after a lowercase letter that itself follows a period.
                if b == b'.' && i >= 2 && bytes[i - 1].is_ascii_lowercase() && bytes[i - 2] == b'.'
                {
                    i += 1;
                    continue;
                }
                let after = &s[i + 1..];
                if after.is_empty() {
                    return None;
                }
                if after.starts_with(' ') || after.starts_with('\n') || after.starts_with('\r') {
                    return Some((&s[..=i], after));
                }
                if let Some(c) = after.chars().next() {
                    if c.is_uppercase() {
                        return Some((&s[..=i], after));
                    }
                }
            }
            i += utf8_len(bytes[i]);
        }
        None
    }

    /// Drop leading thought-aloud sentences; keep the user-facing answer.
    fn strip_thought_prefix(text: &str) -> String {
        let (body, nl) = match text.strip_suffix('\n') {
            Some(b) => match b.strip_suffix('\r') {
                Some(b2) => (b2, "\r\n"),
                None => (b, "\n"),
            },
            None => (text, ""),
        };
        let mut rest = body.trim_start();
        while let Some((sentence, after)) = Self::split_first_sentence(rest) {
            if !Self::is_thought_sentence(sentence) {
                break;
            }
            rest = after.trim_start();
        }
        if rest.is_empty() {
            return String::new();
        }
        if Self::is_thought_sentence(rest) && Self::split_first_sentence(rest).is_none() {
            // Whole remainder is one thought sentence (no answer after).
            return String::new();
        }
        format!("{rest}{nl}")
    }

    /// Drop trailing tool-narration / boilerplate sentences ("Checking the latest…").
    fn strip_trailing_narration(text: &str) -> String {
        let (body, nl) = match text.strip_suffix('\n') {
            Some(b) => match b.strip_suffix('\r') {
                Some(b2) => (b2, "\r\n"),
                None => (b, "\n"),
            },
            None => (text, ""),
        };
        if body.trim().is_empty() {
            return String::new();
        }
        let mut sentences: Vec<String> = Vec::new();
        let mut rest = body.trim_start();
        while let Some((sentence, after)) = Self::split_first_sentence(rest) {
            sentences.push(sentence.trim().to_string());
            rest = after.trim_start();
        }
        if !rest.is_empty() {
            sentences.push(rest.trim().to_string());
        }
        while sentences
            .last()
            .is_some_and(|s| Self::is_boilerplate(s) || Self::is_thought_sentence(s))
        {
            sentences.pop();
        }
        if sentences.is_empty() {
            return String::new();
        }
        let mut out = String::new();
        for s in sentences {
            if !out.is_empty() && !out.ends_with([' ', '\n', '\r']) {
                out.push(' ');
            }
            out.push_str(&s);
        }
        format!("{out}{nl}")
    }

    /// True when `s` is (or is still typing toward) a short tool-narration sentence.
    fn could_be_narration_prefix(s: &str) -> bool {
        let l = s
            .trim()
            .trim_start_matches(['*', '_', '#', '>', '-', ' '])
            .to_ascii_lowercase();
        if l.is_empty() {
            return false;
        }
        if Self::is_boilerplate(s) {
            return true;
        }
        const STARTERS: &[&str] = &[
            "running ",
            "executing ",
            "calling ",
            "invoking ",
            "let me check",
            "let me run",
            "let me look",
            "i'll check",
            "i will check",
            "i'll run",
            "i will run",
            "checking ",
            "reading the file",
            "first, i",
            "first i",
            "now i'll",
            "now i will",
        ];
        STARTERS
            .iter()
            .any(|st| st.starts_with(&l) || l.starts_with(st))
    }

    /// Split after the last sentence-ending punctuation (including glued ".Word").
    /// `None` when no sentence has finished yet.
    fn split_at_last_sentence_end(s: &str) -> Option<(&str, &str)> {
        let bytes = s.as_bytes();
        let mut last: Option<usize> = None;
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            if b == b'.' || b == b'!' || b == b'?' {
                if b == b'.' && i >= 2 && bytes[i - 1].is_ascii_lowercase() && bytes[i - 2] == b'.'
                {
                    i += 1;
                    continue;
                }
                let after = &s[i + 1..];
                if after.is_empty()
                    || after.starts_with(' ')
                    || after.starts_with('\n')
                    || after.starts_with('\r')
                    || after.chars().next().is_some_and(|c| c.is_uppercase())
                {
                    last = Some(i + 1);
                }
            }
            i += utf8_len(bytes[i]);
        }
        let end = last?;
        Some((&s[..end], &s[end..]))
    }

    /// Insert a space after `.` / `!` / `?` when the next character is an uppercase letter
    /// ("have.Checking" → "have. Checking"). Skips `e.g.` / `i.e.`-style abbreviations.
    fn fix_glued_spaces(text: &str) -> String {
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::with_capacity(text.len() + 8);
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            out.push(c);
            if matches!(c, '.' | '!' | '?') {
                let abbrev =
                    c == '.' && i >= 2 && chars[i - 1].is_ascii_lowercase() && chars[i - 2] == '.';
                if !abbrev {
                    if let Some(next) = chars.get(i + 1) {
                        if next.is_uppercase() {
                            out.push(' ');
                        }
                    }
                }
            }
            i += 1;
        }
        out
    }

    /// Strip thought-aloud and trailing tool-narration; fix glued sentence spacing.
    pub fn clean_prose(text: &str) -> String {
        let stripped = Self::strip_thought_prefix(text);
        if stripped.trim().is_empty() {
            return String::new();
        }
        let without_tail = Self::strip_trailing_narration(&stripped);
        if without_tail.trim().is_empty() {
            return String::new();
        }
        Self::fix_glued_spaces(&without_tail)
    }

    fn take_line(&mut self, line: &str) -> Option<String> {
        let t = line.trim_start();
        if self.in_fence {
            if t.starts_with("```") {
                self.in_fence = false;
                let hidden = self.fence_hidden;
                self.fence_hidden = false;
                return if hidden { None } else { Some(line.to_owned()) };
            }
            return if self.fence_hidden {
                None
            } else {
                Some(line.to_owned())
            };
        }
        if t.starts_with("```") {
            let lang = t.trim_start_matches('`').trim().to_ascii_lowercase();
            self.in_fence = true;
            self.fence_hidden = lang.starts_with("json");
            return if self.fence_hidden {
                None
            } else {
                Some(line.to_owned())
            };
        }
        if Self::is_boilerplate(line) {
            return None;
        }
        let cleaned = Self::clean_prose(line);
        if cleaned.trim().is_empty() {
            return None;
        }
        self.started = true;
        Some(cleaned)
    }

    /// Feed a delta; returns the text that may enter the doc now.
    pub fn feed(&mut self, delta: &str) -> String {
        self.pending.push_str(delta);
        let mut out = String::new();
        while let Some(i) = self.pending.find('\n') {
            let line: String = self.pending.drain(..=i).collect();
            if let Some(l) = self.take_line(&line) {
                out.push_str(&l);
            }
        }
        // A partial line streams at once when it cannot be boilerplate or a fence any more.
        let p = self.pending.trim_start();
        let could_be_fence = p.is_empty() || "```".starts_with(p) || p.starts_with("```");
        if self.started && !self.in_fence && !could_be_fence {
            // Emit through the last finished sentence (cleaned); hold a trailing fragment
            // that still looks like tool-narration so "Checking…" never lands mid-stream.
            let raw = std::mem::take(&mut self.pending);
            if let Some((complete, rest)) = Self::split_at_last_sentence_end(&raw) {
                out.push_str(&Self::clean_prose(complete));
                if !rest.is_empty() && Self::could_be_narration_prefix(rest) {
                    self.pending = rest.to_string();
                } else if !rest.is_empty() {
                    let rest = Self::fix_glued_spaces(rest.trim_start());
                    if !rest.is_empty() {
                        if !out.is_empty()
                            && !out.ends_with([' ', '\n', '\r'])
                            && out.ends_with(['.', '!', '?'])
                            && rest.chars().next().is_some_and(|c| c.is_uppercase())
                        {
                            out.push(' ');
                        }
                        out.push_str(&rest);
                    }
                }
            } else if Self::could_be_narration_prefix(&raw) {
                self.pending = raw;
            } else {
                out.push_str(&Self::fix_glued_spaces(&raw));
            }
        } else if !self.started && !self.in_fence && self.pending.len() > 160 {
            // Long first line: real prose. Judge it once and let it through.
            if Self::is_boilerplate(&self.pending) {
                self.pending.clear();
            } else {
                let cleaned = Self::clean_prose(&self.pending);
                self.pending.clear();
                if cleaned.trim().is_empty() {
                    // Entire buffer was thought-aloud / narration; keep waiting for real prose.
                } else {
                    self.started = true;
                    out.push_str(&cleaned);
                }
            }
        }
        out
    }

    /// End of a text run (tool call or turn end): judge what is still held back.
    pub fn flush(&mut self) -> String {
        let rest = std::mem::take(&mut self.pending);
        if rest.is_empty() {
            return rest;
        }
        if self.in_fence {
            // An unterminated fence at the end: drop it when hidden, else let it through.
            self.in_fence = false;
            let hidden = self.fence_hidden;
            self.fence_hidden = false;
            return if hidden { String::new() } else { rest };
        }
        self.take_line(&rest).unwrap_or_default()
    }
}

fn utf8_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else {
        4
    }
}

/// Subagents running at once per box; further spawns wait in FIFO order (demo review item 2).
pub const SUBAGENT_MAX_CONCURRENT: usize = 4;

/// Pack section for every subagent: an unknown service, API or tool is researched, not guessed
/// at and not handed back to the user. Mirrors the main agent's rule in `prompts/router.md`.
const RESEARCH_SECTION: &str = "## Unknown services, APIs and tools\n\nWhen the task involves a service, API, CLI or library you do not know, or a call fails (not found, unknown endpoint, auth error, unexpected shape), find the answer yourself; guessing more than one or two URLs is not research. Until one path works: `web_search` for the service's API reference or developer docs and `web_fetch` them (the docs site, its OpenAPI/Swagger spec at `/openapi.json`, `/swagger.json`, `/docs`, `/redoc`, `/llms.txt`, the README); find the official SDK or CLI, install it (`uv tool install`, `pip install`, `npm i -g`, a release binary) and read its `--help` and, when needed, its source for the real base URL, routes and auth header; read the service's GitHub repositories and examples; read the error body. Read what you fetched before the next call. Authenticated calls go through `egress` with the secret's placeholder in a header or the body.\n\nReport `blocked` or `needs_input` only for a real blocker: a credential or account that does not exist, a choice only the user can make between real options you found, a spend decision, or a service that is down after you checked it against its docs. Never ask for docs, an API reference, an endpoint, or for the user to install something. Money: anything that charges the user beyond what the brief states (renting, buying, subscribing, paying) needs their go-ahead first: show the chosen option with its price (and a cheaper alternative when one exists) and ask with `ask_user`; listing options and quoting prices is free.\n\nSecret notes: each secret the owner stored comes with their note (`use_notes` in `secrets_list`, comments in your shell's secrets.env); the note is the policy for that key — for `egress` and for any CLI, SDK or script that reads the real `$NAME`. Spending guidance in it binds you: a limit (\"may spend up to $X without asking\") lets you proceed within it and no further; \"confirm before charging\" or \"read-only\" means ask first or never charge, whatever the brief says. A note without spending guidance leaves the money rule above in force. Never pass a confirmation-skipping flag (`--yes`, `-y`, `--force`, `--no-confirm` and the like) to a command that charges unless the note allows that spend.\n\n";

/// Why the main pi session is being prompted.
#[derive(Debug, Clone)]
pub enum TurnKind {
    /// A user message (`send` / `answer`).
    User,
    /// A finished subagent's result, relayed to the user in prose by the main agent.
    SubagentResult { subagent_id: String, name: String },
    /// An approval card was decided after its turn ended (feature `approve`): the main agent
    /// dispatches or acknowledges, in prose.
    ApprovalOutcome { approval_id: String, title: String },
}

/// What one main-agent turn produced, for the caller's follow-ups (memory pass, hand-offs).
pub struct TurnOutcome {
    pub assistant_msg_id: String,
    pub route_name: &'static str,
    pub turn: TurnState,
    pub result: Result<()>,
}

#[derive(Debug, Clone, Default)]
pub struct TurnMetrics {
    pub ttft_ms: Option<i64>,
    pub route_ms: Option<i64>,
    pub tool_calls: i64,
    pub usage: Option<Value>,
    pub route_missing: bool,
    pub route_corrected: bool,
}

pub struct TurnState {
    pub chat_id: String,
    pub user_msg_id: String,
    pub assistant_msg_id: String,
    pub text_part_id: String,
    pub started: Instant,
    pub route: Option<ValidRoute>,
    pub route_attempts: u32,
    pub route_failed: bool,
    pub escalate: bool,
    pub asked: Option<String>,
    pub metrics: TurnMetrics,
    pub streamed_text: String,
    pub act_transcript: Vec<String>,
    /// call id → args from `tool_execution_start`, so the tool part records the real input.
    pub tool_inputs: HashMap<String, Value>,
    pub reply_extractor: ReplyExtractor,
    /// Streams `route.ack` into the first row of the turn (agent-flow contract §1).
    pub ack_extractor: ReplyExtractor,
    /// The acknowledgement sentence written so far.
    pub ack_text: String,
    /// The ack row was closed; `assistant_msg_id` now names the result row (or none, see `row_closed`).
    pub ack_split: bool,
    /// No live row right now (a `new`/`wake` turn ended on its ack): the next text opens one.
    pub row_closed: bool,
    /// Progress bubbles sent with `send_message` this turn.
    pub progress_texts: Vec<String>,
    /// The `activity` currently on the live row, so unchanged labels are not rewritten.
    pub activity: Option<String>,
    /// Boilerplate filter on everything the main agent says in this turn.
    pub prose: ProseFilter,
    /// Act turns: prose is held here and only the text after the last tool call reaches the doc;
    /// anything the model said between tool calls is narration and goes to T3 only.
    pub act_buffer: String,
    pub route_call_id: Option<String>,
    pub text_part_dirty: bool,
    /// Set by an out-of-band `stop`: the turn ends as it is and no hand-off follows.
    pub stopped: bool,
    /// The text pi was prompted with; the skills selector matches against it (F5).
    pub prompt_text: String,
    /// Skills injected into this turn's working set (recorded once, in the Details drawer).
    pub skills_used: Vec<String>,
}

pub struct Agent {
    pub cfg: Arc<Config>,
    pub store: Store,
    pub sync: SyncHub,
    pub llm: Arc<Llm>,
    pub memory: Arc<Memory>,
    /// Memory tree (D77): read-only side index over T3, walked by the decision model per turn.
    pub tree: Arc<crate::tree::Tree>,
    pi: Mutex<Option<Arc<RpcChild>>>,
    current_chat: Mutex<Option<String>>,
    pub turn: Arc<Mutex<Option<TurnState>>>,
    adapters: HashMap<Harness, Arc<dyn HarnessAdapter>>,
    pub turn_running: AtomicBool,
    /// Subagent turns in flight (keep-alive must cover them too, design §4.6).
    pub subagent_turns: std::sync::atomic::AtomicU32,
    pub pi_ready: AtomicBool,
    pub ext_loaded: AtomicBool,
    /// Set by `shutdown`: the pi supervisor must not respawn a child we are stopping (an orphaned
    /// pi dies with `write EPIPE` at its next reply once agentd is gone).
    pub shutting_down: AtomicBool,
    turn_lock: Mutex<()>,
    /// FIFO gate on concurrent subagent runs in this workspace (`SUBAGENT_MAX_CONCURRENT`).
    subagent_slots: Arc<Semaphore>,
    /// Box-wide gate shared by every workspace (`workspace::BOX_MAX_SUBAGENTS`).
    box_slots: Arc<Semaphore>,
    http: reqwest::Client,
    pub settings: crate::settings::Settings,
    /// Cached skills index (F5): shared + this Project's own, bodies included.
    pub skills: SkillsIndex,
    /// Pending approval cards and short-lived grants (feature `approve`).
    pub approvals: crate::approval::Approvals,
    /// Running subagents → (chat id, progress message id), so a file a subagent delivers lands
    /// on its own card (F10 `deliver_file`).
    pub subagent_msgs: Mutex<HashMap<String, (String, String)>>,
    /// Group-chat bookkeeping (chat kinds from the registry, this box's "Projects ⇄" tab).
    pub groups: crate::groups::GroupState,
    pub people: crate::people::PeopleState,
    /// Image inputs for the next pi prompt (set by `handle_send` from the message's attachments).
    pub turn_images: std::sync::Mutex<Vec<Value>>,
}

impl Agent {
    pub async fn pi_child(&self) -> Result<Arc<RpcChild>> {
        self.pi().await
    }

    pub fn http_client(&self) -> &reqwest::Client {
        &self.http
    }

    /// Relay blob mirror slot for a chat (`PUT/GET /blob/{slot}/{key}`).
    /// People chats use `people:{chat_id}`; project chats use this project's id.
    pub fn blob_slot(&self, chat_id: &str) -> String {
        if self.people.is_people_chat(chat_id) {
            format!("people:{chat_id}")
        } else {
            self.cfg.project_id.clone()
        }
    }

    pub fn new(
        cfg: Arc<Config>,
        store: Store,
        sync: SyncHub,
        box_slots: Arc<Semaphore>,
    ) -> Arc<Self> {
        let cfg_for_settings = cfg.clone();
        let llm = Arc::new(Llm::new(&cfg));
        let memory = Arc::new(Memory::new(cfg.clone(), store.clone(), llm.clone()));
        let tree = Arc::new(crate::tree::Tree::new(&cfg, store.clone(), llm.clone()));
        let mut adapters: HashMap<Harness, Arc<dyn HarnessAdapter>> = HashMap::new();
        adapters.insert(
            Harness::Prime,
            Arc::new(PrimeAdapter::new(cfg.clone(), store.clone())),
        );
        adapters.insert(
            Harness::Pi,
            Arc::new(PiAdapter::new(cfg.clone(), store.clone())),
        );
        adapters.insert(
            Harness::ClaudeCode,
            Arc::new(ClaudeCodeAdapter::new(cfg.clone(), store.clone())),
        );
        Arc::new(Self {
            cfg,
            store,
            sync,
            llm,
            memory,
            tree,
            pi: Mutex::new(None),
            current_chat: Mutex::new(None),
            turn: Arc::new(Mutex::new(None)),
            adapters,
            turn_running: AtomicBool::new(false),
            subagent_turns: std::sync::atomic::AtomicU32::new(0),
            pi_ready: AtomicBool::new(false),
            ext_loaded: AtomicBool::new(false),
            shutting_down: AtomicBool::new(false),
            turn_lock: Mutex::new(()),
            subagent_slots: Arc::new(Semaphore::new(SUBAGENT_MAX_CONCURRENT)),
            box_slots,
            http: reqwest::Client::new(),
            settings: crate::settings::Settings::new(&cfg_for_settings),
            skills: SkillsIndex::new(),
            approvals: crate::approval::Approvals::default(),
            subagent_msgs: Mutex::new(HashMap::new()),
            groups: crate::groups::GroupState::default(),
            people: crate::people::PeopleState::default(),
            turn_images: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// Refetch the skills index from the relay (60 s timer, `skills_rev` change, after a save).
    pub async fn refresh_skills(&self) {
        match self
            .skills
            .refresh(
                &self.http,
                &self.cfg.relay_url,
                &self.cfg.box_token,
                &self.cfg.project_id,
            )
            .await
        {
            Ok(n) => {
                tracing::debug!(skills = n, rev = %self.skills.rev(), "skills index refreshed")
            }
            Err(e) => tracing::debug!(error = %e, "skills index refresh"),
        }
    }

    /// Live model slots (registry/doc writes land here; `cfg` only holds the boot values).
    pub(crate) async fn live_models(&self) -> (String, String) {
        let m = self.settings.models.read().await;
        (m.talking.id.clone(), m.subagent.id.clone())
    }

    pub fn adapter(&self, h: Harness) -> Arc<dyn HarnessAdapter> {
        self.adapters.get(&h).cloned().expect("registered adapter")
    }

    // ---- pi child -----------------------------------------------------------------------------

    fn system_prompt_path(&self) -> std::path::PathBuf {
        self.cfg.state_dir.join("pi").join("system-prompt.md")
    }

    /// Router prompt (byte-stable, first) + project instructions + T0/T1 (design §6.9 layout).
    pub fn write_system_prompt(&self) -> Result<()> {
        let mut s = String::from(include_str!("../prompts/router.md"));
        let talking = self
            .settings
            .models
            .try_read()
            .map(|m| m.talking.id.clone())
            .unwrap_or_else(|_| self.cfg.talking_model.clone());
        // One box, many workspaces (D70): the project's files live in its own folder, not /work.
        s.push_str(&format!(
            "\n\n## This project\n\nProject \"{}\". Its folder is `{}`: every file you create or read for this project goes there (relative paths resolve there; do not use `/work` itself, other folders under `/work` belong to other projects). You run on model `{}`.\n",
            self.cfg.project_name,
            self.cfg.work_dir.display(),
            talking
        ));
        s.push('\n');
        s.push_str(&crate::media::prompt_section(&self.cfg));
        if !self.cfg.instructions.trim().is_empty() {
            s.push_str("\n\n## Project instructions\n\n");
            s.push_str(self.cfg.instructions.trim());
            s.push('\n');
        }
        let core = self.memory.core_memory_text();
        if !core.trim().is_empty() {
            s.push_str("\n\n## Project memory\n\n");
            s.push_str(&core);
        }
        std::fs::create_dir_all(self.system_prompt_path().parent().unwrap())?;
        std::fs::write(self.system_prompt_path(), s)?;
        Ok(())
    }

    /// Deterministic pi session file for a chat: `<sessions>/<ts>_<chatId>.jsonl` (one session per chat).
    fn session_file_for(&self, chat_id: &str) -> Result<std::path::PathBuf> {
        if let Some(p) = self.store.chat(chat_id)?.and_then(|c| c.pi_session_path) {
            if std::path::Path::new(&p).exists() {
                return Ok(p.into());
            }
        }
        let dir = self.cfg.pi_sessions_dir();
        std::fs::create_dir_all(&dir)?;
        if let Some(existing) = crate::harness::find_session_file(&dir, chat_id) {
            self.store
                .set_chat_session_path(chat_id, &existing.display().to_string())?;
            return Ok(existing);
        }
        let ts = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S-%3fZ");
        let path = dir.join(format!("{ts}_{chat_id}.jsonl"));
        let header = json!({ "type": "session", "version": 3, "id": chat_id, "timestamp": chrono::Utc::now().to_rfc3339(), "cwd": self.cfg.work_dir });
        std::fs::write(&path, format!("{header}\n"))?;
        // pi appends to this file as the workspace user; agentd (root) created it.
        if let Some(ra) = &self.cfg.run_as {
            let _ = std::os::unix::fs::chown(&path, Some(ra.uid), Some(ra.gid));
        }
        self.store
            .set_chat_session_path(chat_id, &path.display().to_string())?;
        Ok(path)
    }

    pub async fn start_pi(self: &Arc<Self>, initial_chat: &str) -> Result<()> {
        self.write_system_prompt()?;
        let session = self.session_file_for(initial_chat)?;
        let (talking_model, _) = self.live_models().await;
        let mut args = PiAdapter::common_args(
            &self.cfg,
            &self.cfg.pi_sessions_dir(),
            &talking_model,
            &self.system_prompt_path(),
        );
        args.extend(["--session".into(), session.display().to_string()]);
        let log = self.cfg.state_dir.join("logs").join("pi-main.log");
        std::fs::create_dir_all(log.parent().unwrap())?;
        std::fs::create_dir_all(&self.cfg.work_dir)?;
        let t0 = Instant::now();
        let child = RpcChild::spawn(
            "pi-main",
            &self.cfg.pi_bin,
            &args,
            &PiAdapter::envs(&self.cfg, initial_chat, "main"),
            &self.cfg.work_dir,
            &log,
            self.cfg.run_as.as_ref(),
        )
        .await?;
        let state = child
            .call_timeout(
                RpcCommand::GetState {
                    id: "boot-state".into(),
                },
                Duration::from_secs(30),
            )
            .await?;
        if !state.success {
            return Err(anyhow!("pi get_state failed: {:?}", state.error));
        }
        tracing::info!(ms = t0.elapsed().as_millis() as i64, "pi main agent ready");
        *self.pi.lock().await = Some(child);
        *self.current_chat.lock().await = Some(initial_chat.to_owned());
        self.pi_ready.store(true, Ordering::SeqCst);
        self.store.upsert_session(
            initial_chat,
            None,
            Some(&session.display().to_string()),
            &self.cfg.work_dir.display().to_string(),
            &talking_model,
            "pi",
            "main",
        )?;
        let _ = self
            .post_timing("pi_boot_ms", t0.elapsed().as_millis() as i64, None)
            .await;
        // The extension writes an `extension_loaded` audit line on load.
        if let Ok(s) = std::fs::read_to_string(self.cfg.audit_path()) {
            if s.contains("\"extension_loaded\"") {
                self.ext_loaded.store(true, Ordering::SeqCst);
            }
        }
        // Supervise: respawn on exit (sandbox-image.md §3.3).
        let me = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let alive = me
                    .pi
                    .lock()
                    .await
                    .as_ref()
                    .map(|c| c.is_alive())
                    .unwrap_or(false);
                if me.shutting_down.load(Ordering::SeqCst) {
                    return;
                }
                if !alive && !me.turn_running.load(Ordering::SeqCst) {
                    me.pi_ready.store(false, Ordering::SeqCst);
                    let chat = me.current_chat.lock().await.clone().unwrap_or_default();
                    tracing::warn!("pi main child is gone; respawning");
                    if let Err(e) = me.respawn_pi(&chat).await {
                        tracing::error!(error = %e, "pi respawn failed");
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        });
        Ok(())
    }

    /// Respawn the main pi child on the chat it was last on (empty → a fresh
    /// session file). Used when an in-session `set_model` is refused so the
    /// next turn boots with `live_models()`.
    pub(crate) async fn respawn_pi_current(self: &Arc<Self>) -> Result<()> {
        let chat = self.current_chat.lock().await.clone().unwrap_or_default();
        self.respawn_pi(&chat).await
    }

    async fn respawn_pi(self: &Arc<Self>, chat_id: &str) -> Result<()> {
        if let Some(old) = self.pi.lock().await.take() {
            old.shutdown().await;
        }
        let session = self.session_file_for(chat_id)?;
        let (talking_model, _) = self.live_models().await;
        let mut args = PiAdapter::common_args(
            &self.cfg,
            &self.cfg.pi_sessions_dir(),
            &talking_model,
            &self.system_prompt_path(),
        );
        args.extend(["--session".into(), session.display().to_string()]);
        let log = self.cfg.state_dir.join("logs").join("pi-main.log");
        let child = RpcChild::spawn(
            "pi-main",
            &self.cfg.pi_bin,
            &args,
            &PiAdapter::envs(&self.cfg, chat_id, "main"),
            &self.cfg.work_dir,
            &log,
            self.cfg.run_as.as_ref(),
        )
        .await?;
        child
            .call_timeout(
                RpcCommand::GetState {
                    id: "boot-state".into(),
                },
                Duration::from_secs(30),
            )
            .await?;
        *self.pi.lock().await = Some(child);
        *self.current_chat.lock().await = Some(chat_id.to_owned());
        self.pi_ready.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Wait briefly for the supervisor to respawn pi (it polls every 2 s). A send that
    /// lands between death and respawn must not permanently Fail with no assistant bubble.
    async fn pi(&self) -> Result<Arc<RpcChild>> {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(c) = self.pi.lock().await.clone() {
                return Ok(c);
            }
            if self.shutting_down.load(Ordering::SeqCst) || Instant::now() >= deadline {
                return Err(anyhow!("pi main agent not running"));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    async fn ensure_chat_session(&self, chat_id: &str) -> Result<()> {
        if self.current_chat.lock().await.as_deref() == Some(chat_id) {
            return Ok(());
        }
        let path = self.session_file_for(chat_id)?;
        let pi = self.pi().await?;
        let t0 = Instant::now();
        pi.call_ok(RpcCommand::SwitchSession {
            id: pi.next_id("sw"),
            session_path: path.display().to_string(),
        })
        .await?;
        tracing::info!(
            chat_id,
            ms = t0.elapsed().as_millis() as i64,
            "switched pi session"
        );
        *self.current_chat.lock().await = Some(chat_id.to_owned());
        let (talking_model, _) = self.live_models().await;
        self.store.upsert_session(
            chat_id,
            None,
            Some(&path.display().to_string()),
            &self.cfg.work_dir.display().to_string(),
            &talking_model,
            "pi",
            "main",
        )?;
        Ok(())
    }

    // ---- commands -----------------------------------------------------------------------------

    /// Mark-before-execute, then `done`/`failed` in the doc. The result is also returned for
    /// callers that report it elsewhere (routine runs, channel replies); the command loop ignores it.
    pub async fn handle_command(self: &Arc<Self>, chat_id: &str, cmd: CommandView) -> Result<()> {
        let Some(host) = self.sync.host(chat_id).await else {
            return Err(anyhow!("no room for chat {chat_id}"));
        };
        // People chats route by mention (or, in a group, the decision model): a message not
        // for this agent is left pending for whoever it is for; humans' own sends are theirs.
        // Shared addresses ("hey agents", several @agents) stay `taken` until each addressed
        // agent has replied — see `people_shared_send_open` / sync `new_pending`.
        let people_msg_id = if cmd.kind == "send" && self.people.is_people_chat(chat_id) {
            let msg_id = cmd
                .payload
                .get("messageId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            if !self.people_wants(chat_id, &host, &msg_id).await {
                return Ok(());
            }
            if self.people_already_replied(&host, &msg_id).await {
                return Ok(());
            }
            Some(msg_id)
        } else {
            None
        };
        {
            host.lock()
                .await
                .doc
                .set_command_status(&cmd.id, CommandStatus::Taken, None);
        }
        let res = match cmd.kind.as_str() {
            "send" => self.handle_send(chat_id, &host, &cmd).await,
            "stop" => self.handle_stop(chat_id).await,
            // Guide: inject into the running turn (no turn_lock). Same as stop — mid-task.
            "steer" | "guide" => self.handle_steer(chat_id, &host, &cmd).await,
            "answer" => self.handle_send(chat_id, &host, &cmd).await,
            "retitle" => {
                let title = cmd
                    .payload
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_owned();
                self.retitle(chat_id, &title).await
            }
            "retry" => self.handle_retry(chat_id, &host, &cmd).await,
            "approve" | "reject" => {
                let approval_id = cmd
                    .payload
                    .get("approvalId")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let outcome = if cmd.kind == "approve" {
                    ApprovalOutcome::Approved
                } else {
                    ApprovalOutcome::Rejected {
                        reason: cmd
                            .payload
                            .get("reason")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|r| !r.is_empty())
                            .map(str::to_owned),
                    }
                };
                self.decide_approval(&approval_id, outcome, &cmd.issued_by)
                    .await
            }
            other => Err(anyhow!("unknown command kind {other}")),
        };
        let leave_taken = match (&res, people_msg_id.as_deref()) {
            (Ok(()), Some(msg_id)) => self.people_shared_send_open(chat_id, &host, msg_id).await,
            _ => false,
        };
        let mut h = host.lock().await;
        match &res {
            Ok(()) => {
                if !leave_taken {
                    h.doc.set_command_status(&cmd.id, CommandStatus::Done, None);
                }
            }
            Err(e) => {
                tracing::warn!(chat_id, cmd = %cmd.kind, error = %e, "command failed");
                h.doc.set_command_status(
                    &cmd.id,
                    CommandStatus::Failed,
                    Some(json!({ "error": e.to_string() })),
                );
                // iOS never renders command status. A send that failed before any assistant
                // row would leave the user message alone forever — surface an error with Retry.
                if matches!(cmd.kind.as_str(), "send" | "answer" | "steer" | "guide") {
                    let user_msg = cmd
                        .payload
                        .get("messageId")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if !user_msg.is_empty() {
                        let has_reply = h.doc.messages().iter().any(|m| {
                            m.role == "assistant"
                                && m.rest.get("inReplyTo").and_then(Value::as_str) == Some(user_msg)
                                && m.status != "cancelled"
                        });
                        // A failed steer still has the live turn's assistant row; only surface a
                        // new error bubble when nothing is answering this user message.
                        if !has_reply {
                            let id = ulid();
                            h.doc
                                .start_assistant_message(&id, user_msg, &self.cfg.device_id);
                            h.doc.add_status_part(&id, "failed", "Failed");
                            h.doc.add_error_part(&id, "send_failed", &e.to_string());
                            h.doc.finish_message(&id, MessageStatus::Error);
                        }
                    }
                }
            }
        }
        res
    }

    /// Abort the turn running in `chat_id` (people chat removal).
    pub(crate) async fn stop_turn(&self, chat_id: &str) -> Result<()> {
        self.handle_stop(chat_id).await
    }

    /// Re-run the turn for a failed assistant message (`retry { messageId }`). The payload is the
    /// assistant row the UI shows Retry on; we find its user message (`inReplyTo`) and `send` again.
    async fn handle_retry(
        self: &Arc<Self>,
        chat_id: &str,
        host: &SharedHost,
        cmd: &CommandView,
    ) -> Result<()> {
        let failed_id = cmd
            .payload
            .get("messageId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if failed_id.is_empty() {
            return Err(anyhow!("retry: missing messageId"));
        }
        let user_msg_id = {
            let mut h = host.lock().await;
            let msgs = h.doc.messages();
            let failed = msgs.iter().find(|m| m.id == failed_id);
            let user_id = failed
                .and_then(|m| m.rest.get("inReplyTo").and_then(Value::as_str))
                .map(str::to_owned)
                .or_else(|| {
                    let ix = msgs.iter().position(|m| m.id == failed_id)?;
                    msgs[..ix]
                        .iter()
                        .rev()
                        .find(|m| m.role == "user")
                        .map(|m| m.id.clone())
                })
                .ok_or_else(|| anyhow!("retry: no user message for {failed_id}"))?;
            if failed.is_some_and(|m| m.role == "assistant") {
                h.doc.finish_message(&failed_id, MessageStatus::Cancelled);
            }
            user_id
        };
        let send = CommandView {
            id: cmd.id.clone(),
            kind: "send".into(),
            payload: json!({ "messageId": user_msg_id }),
            issued_by: cmd.issued_by.clone(),
            issued_at: cmd.issued_at,
            status: cmd.status.clone(),
        };
        self.handle_send(chat_id, host, &send).await
    }

    /// `stop` runs out of band (its own task, no turn lock): abort the pi turn now, mark the turn
    /// stopped so no hand-off follows, and tell the chat.
    async fn handle_stop(&self, chat_id: &str) -> Result<()> {
        let target = {
            let mut tl = self.turn.lock().await;
            match tl.as_mut() {
                Some(t) if t.chat_id == chat_id => {
                    t.stopped = true;
                    t.escalate = false;
                    Some(t.assistant_msg_id.clone())
                }
                _ => None,
            }
        };
        let Some(msg_id) = target else {
            tracing::info!(chat_id, "stop: no turn running for this chat");
            return Ok(());
        };
        if let Ok(pi) = self.pi().await {
            let _ = pi
                .call_timeout(
                    RpcCommand::Abort {
                        id: pi.next_id("ab"),
                    },
                    Duration::from_secs(5),
                )
                .await;
        }
        if let Some(host) = self.sync.host(chat_id).await {
            let mut h = host.lock().await;
            h.doc
                .add_status_part(&msg_id, "stopped", "Stopped by the user");
        }
        tracing::info!(chat_id, "stop: pi turn aborted");
        Ok(())
    }

    /// `steer` / `guide` runs out of band (no turn lock): inject the user's words into the
    /// currently running pi turn via `RpcCommand::Steer`. When nothing is running, fall back to
    /// a normal `send` so a late Guide still lands.
    async fn handle_steer(
        self: &Arc<Self>,
        chat_id: &str,
        host: &SharedHost,
        cmd: &CommandView,
    ) -> Result<()> {
        let user_msg_id = cmd
            .payload
            .get("messageId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let user_text = {
            let h = host.lock().await;
            h.doc
                .message(&user_msg_id)
                .map(|m| m.text())
                .unwrap_or_default()
        };
        if user_text.trim().is_empty() {
            return Err(anyhow!("steer: message {user_msg_id} not found or empty"));
        }
        let running_here = {
            let tl = self.turn.lock().await;
            matches!(tl.as_ref(), Some(t) if t.chat_id == chat_id)
        };
        if !running_here {
            tracing::info!(chat_id, "steer: no turn running; falling back to send");
            return self.handle_send(chat_id, host, cmd).await;
        }
        let _ = self.store.t3_append(
            chat_id,
            None,
            Some(chat_id),
            "user",
            Some("user"),
            &user_text,
            Some(&json!({ "message_id": user_msg_id, "steer": true })),
        );
        let pi = self.pi().await?;
        let r = pi
            .call(RpcCommand::Steer {
                id: pi.next_id("s"),
                message: user_text,
            })
            .await?;
        if !r.success {
            return Err(anyhow!("steer rejected: {}", r.error.unwrap_or_default()));
        }
        tracing::info!(chat_id, "steer: injected into running turn");
        Ok(())
    }

    /// For an `answer` command with `{ questionId, value }`: `(question, rendered text)`. The
    /// widget comes from the ask row (falling back to the doc part); the value is rendered once,
    /// here, so the model, T3 and the bubble all read the same words. Also flips the part to
    /// `answered` and the ask row to answered.
    async fn resolve_structured_answer(
        &self,
        host: &SharedHost,
        cmd: &CommandView,
    ) -> Option<(String, String)> {
        if cmd.kind != "answer" {
            return None;
        }
        let question_id = cmd.payload.get("questionId").and_then(Value::as_str)?;
        let value = cmd.payload.get("value").cloned().unwrap_or(Value::Null);
        let (question, widget_json) = match self.store.ask(question_id) {
            Ok(Some((q, w, _))) => (q, w),
            Ok(None) | Err(_) => {
                let h = host.lock().await;
                let (_, q, w) = h
                    .doc
                    .open_question()
                    .filter(|(id, _, _)| id == question_id)?;
                (q, w)
            }
        };
        let rendered = match Widget::parse(widget_json.as_ref()) {
            Ok(Some(w)) => w.answer_text(&value),
            Ok(None) | Err(_) => {
                let text = cmd
                    .payload
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        value
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or(value.to_string())
                    });
                format!("Answer: {text}")
            }
        };
        let _ = self.store.answer_ask(question_id);
        {
            let mut h = host.lock().await;
            h.doc.answer_question_part(question_id, &value);
        }
        Some((question, rendered))
    }

    async fn handle_send(
        self: &Arc<Self>,
        chat_id: &str,
        host: &SharedHost,
        cmd: &CommandView,
    ) -> Result<()> {
        let _guard = self.turn_lock.lock().await;
        let user_msg_id = cmd
            .payload
            .get("messageId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        // A structured answer (`answer { questionId, value }`): render the value to text with the
        // widget the question was asked with, and mark the part answered before the turn runs.
        let structured = self.resolve_structured_answer(host, cmd).await;
        let (mut user_text, channel_note, message_view) = {
            let h = host.lock().await;
            let m = h.doc.message(&user_msg_id);
            let text = m.as_ref().map(|m| m.text()).unwrap_or_default();
            (text, m.as_ref().and_then(crate::channels::prompt_note), m)
        };
        if user_text.trim().is_empty() {
            if let Some((_, rendered)) = &structured {
                user_text = rendered.clone();
            }
        }
        // Attachments (images, voice notes, files): fetched into the workspace; images ride
        // along as vision input, a voice note's transcript becomes the text when none was typed.
        let attachments = match &message_view {
            Some(m) => self.turn_attachments(chat_id, m).await,
            None => crate::attachments::TurnAttachments::default(),
        };
        if user_text.trim().is_empty() {
            if let Some(t) = &attachments.transcript {
                user_text = t.clone();
            }
        }
        if user_text.trim().is_empty() && !attachments.notes.is_empty() {
            user_text = "(attachment)".to_owned();
        }
        if user_text.trim().is_empty() {
            return Err(anyhow!("send: message {user_msg_id} not found or empty"));
        }
        let _ = self.store.touch_chat(chat_id);
        let _ = self.store.t3_append(
            chat_id,
            None,
            Some(chat_id),
            "user",
            Some("user"),
            &user_text,
            Some(&json!({ "message_id": user_msg_id })),
        );
        // Title from the first message (design §3.4) when the chat still has the default title.
        if let Ok(Some(c)) = self.store.chat(chat_id) {
            if c.title.is_empty() || c.title == "New chat" {
                let title: String = user_text
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(48)
                    .collect();
                let _ = self
                    .retitle(chat_id, title.trim_end_matches(['.', ',', ':', ';']).trim())
                    .await;
            }
        }
        // Answering an open question (S8): mark it answered; pi receives the text as the next prompt.
        let mut prompt_text = user_text.clone();
        if let Some((question, rendered)) = &structured {
            prompt_text = format!("Answer to your question \"{question}\":\n{rendered}");
        } else if let Ok(Some((ask_id, _, question))) = self.store.open_ask(chat_id) {
            let _ = self.store.answer_ask(&ask_id);
            prompt_text = format!("Answer to your question \"{question}\": {user_text}");
        }
        // A message that arrived through a channel (F7): the model learns where it came from and
        // that the reply is read as plain chat text.
        if let Some(note) = channel_note {
            prompt_text = format!("{note}\n\n{prompt_text}");
        }
        // Group chats (F6): the decision model picks which agents reply (none / one / some /
        // all); fan-out goes only to the chosen others, and the host skips its turn when not
        // selected.
        let mut skip_host_group_turn = false;
        if let Some(gt) = self.group_turn_context(chat_id, host, &user_msg_id).await {
            if let Some(text) = &gt.fan_out_user_text {
                let responders = self.decide_group_responders(&gt, &user_msg_id, text).await;
                let host_id = self.cfg.project_id.as_str();
                let others: Vec<String> = responders
                    .iter()
                    .filter(|id| id.as_str() != host_id)
                    .cloned()
                    .collect();
                let all_others: Vec<String> = gt
                    .participant_ids
                    .iter()
                    .filter(|id| id.as_str() != host_id)
                    .cloned()
                    .collect();
                let targets = if others.is_empty() {
                    Some(Vec::new())
                } else if others.len() == all_others.len() {
                    None
                } else {
                    Some(others)
                };
                self.spawn_group_fan_out(&gt, text, targets);
                if !responders.iter().any(|id| id == host_id) {
                    skip_host_group_turn = true;
                }
            }
            if !skip_host_group_turn {
                prompt_text = format!("{}{prompt_text}", gt.prelude);
            }
        }
        if skip_host_group_turn {
            self.people.set_current(None);
            return Ok(());
        }
        // People chats (D72): who is asking, in what role; the reply is attributed to them.
        // Otherwise: guests get a label; the owner gets an address-by-display-name note.
        if let Some(prelude) = self.people_turn_context(chat_id, host, &user_msg_id).await {
            prompt_text = format!("{prelude}{prompt_text}");
        } else if let Some(note) = self
            .shared_project_guest_note(chat_id, host, &user_msg_id)
            .await
        {
            prompt_text = format!("{note}{prompt_text}");
        } else if let Some(note) = self.owner_address_note(chat_id, host, &user_msg_id).await {
            prompt_text = format!("{note}{prompt_text}");
        }
        if !attachments.notes.is_empty() {
            let mut extra = attachments.notes.join("\n");
            if let Some(t) = &attachments.transcript {
                if user_text.trim() != t.trim() {
                    extra = format!("{extra}\n[Voice note transcript]: {t}");
                }
            }
            prompt_text = format!("{extra}\n\n{prompt_text}");
        }
        *self.turn_images.lock().expect("turn images") = attachments.images;

        // Memory tree (D77): the decision model walks this agent's history and the chosen raw
        // records ride in front of the message. Falls back to today's behaviour (nothing added)
        // when the tree is not built, the walk times out, or the daily cap is reached.
        if let Some(memory_block) = self
            .tree_pack(chat_id, Some(&user_msg_id), "reply", &user_text)
            .await
        {
            prompt_text = format!("{memory_block}\n\n{prompt_text}");
        }

        let outcome = self
            .drive_turn(chat_id, host, &user_msg_id, &prompt_text, TurnKind::User)
            .await;
        self.people.set_current(None);
        let outcome = outcome?;
        let TurnOutcome {
            assistant_msg_id,
            route_name: _,
            turn,
            result,
        } = outcome;

        // The model acknowledges a hand-off in one sentence (demo review item 2); only when it
        // wrote nothing at all does agentd fill the gap so the chat never shows a bare route.
        if matches!(
            turn.route,
            Some(ValidRoute::New { .. }) | Some(ValidRoute::Wake { .. })
        ) && turn.streamed_text.trim().is_empty()
            && turn.ack_text.trim().is_empty()
        {
            let mut h = host.lock().await;
            if let Some(p) = h.doc.add_text_part(&assistant_msg_id, "text") {
                h.doc.append_text(
                    &assistant_msg_id,
                    &p,
                    "Handing this to a subagent; I will post its result here.",
                );
            }
            tracing::info!(chat_id, "ack_fallback: model wrote no sentence after route");
        }
        // An act turn that hit a hard limit and escalated says so instead of leaving an empty
        // bubble; the subagent that continues it reports back in a later message.
        if matches!(turn.route, Some(ValidRoute::Act { .. }))
            && turn.escalate
            && !turn.stopped
            && turn.streamed_text.trim().is_empty()
        {
            let mut h = host.lock().await;
            if let Some(p) = h.doc.add_text_part(&assistant_msg_id, "text") {
                h.doc.append_text(
                    &assistant_msg_id,
                    &p,
                    "I hit a turn limit; I'm handing the rest to a subagent and will post the result here.",
                );
            }
            tracing::info!(chat_id, "escalation note: act turn ended with no text");
        }

        self.spawn_memory_pass(chat_id, &assistant_msg_id, &user_text, &turn.streamed_text);

        // Hand-offs after the main turn: new / wake / escalation. Never on the chat's critical path.
        if turn.stopped {
            return result;
        }
        let followup = match (&turn.route, turn.escalate) {
            (
                Some(ValidRoute::New {
                    name,
                    purpose,
                    brief,
                    harness,
                    isolate: _,
                    data_heavy,
                }),
                _,
            ) => Some((
                None,
                name.clone(),
                purpose.clone(),
                brief.clone(),
                *harness,
                *data_heavy,
                false,
            )),
            (Some(ValidRoute::Wake { subagent_id, brief }), _) => Some((
                Some(subagent_id.clone()),
                String::new(),
                String::new(),
                brief.clone(),
                None,
                false,
                false,
            )),
            (Some(ValidRoute::Act { plan }), true) => {
                let brief = format!(
                    "Continue this task that the project agent started and could not finish within its limits.\n\nUser request: {user_text}\n\nPlan: {plan}\n\nAct so far:\n{}\n\nFinish the task.",
                    turn.act_transcript.join("\n").chars().take(2400).collect::<String>()
                );
                Some((
                    None,
                    "continue-act".into(),
                    "Finish a task the project agent started".into(),
                    brief,
                    None,
                    false,
                    true,
                ))
            }
            _ => None,
        };
        if let Some((existing, name, purpose, brief, harness, data_heavy, escalated)) = followup {
            self.spawn_subagent_task(
                chat_id,
                host,
                &user_msg_id,
                existing,
                name,
                purpose,
                brief,
                harness,
                data_heavy,
                escalated,
            );
        }
        result
    }

    /// Run the subagent in the background and report a start failure into the chat.
    #[allow(clippy::too_many_arguments)]
    fn spawn_subagent_task(
        self: &Arc<Self>,
        chat_id: &str,
        host: &SharedHost,
        user_msg_id: &str,
        existing: Option<String>,
        name: String,
        purpose: String,
        brief: String,
        harness: Option<Harness>,
        data_heavy: bool,
        escalated: bool,
    ) {
        let me = self.clone();
        let chat = chat_id.to_owned();
        let host2 = host.clone();
        let user_msg = user_msg_id.to_owned();
        tokio::spawn(async move {
            if let Err(e) = me
                .run_subagent(
                    &chat,
                    &host2,
                    &user_msg,
                    existing.clone(),
                    &name,
                    &purpose,
                    &brief,
                    harness,
                    data_heavy,
                    escalated,
                )
                .await
            {
                tracing::warn!(error = %e, "subagent run failed");
                let label = if name.is_empty() {
                    existing.clone().unwrap_or_else(|| "subagent".into())
                } else {
                    name.clone()
                };
                {
                    let mut h = host2.lock().await;
                    let id = ulid();
                    h.doc
                        .start_assistant_message(&id, &user_msg, &me.cfg.device_id);
                    h.doc.add_status_part(&id, "failed", "Failed");
                    h.doc.add_error_part(&id, "subagent_failed", &e.to_string());
                    h.doc.finish_message(&id, MessageStatus::Error);
                }
                // The main agent must learn about the failure too, or its next turn still believes
                // the hand-off is running; it also tells the user in prose.
                if let Err(e2) = me
                    .run_result_turn(
                        &chat,
                        &host2,
                        &user_msg,
                        existing.as_deref().unwrap_or(""),
                        &label,
                        "failed",
                        0,
                        "(the subagent did not start)",
                        Some(&e.to_string()),
                    )
                    .await
                {
                    tracing::warn!(error = %e2, "result turn after a failed start");
                }
            }
        });
    }

    /// Memory write pass + compaction, off the reply path (design §6.9).
    fn spawn_memory_pass(self: &Arc<Self>, chat_id: &str, msg_id: &str, ask: &str, reply: &str) {
        let me = self.clone();
        let chat = chat_id.to_owned();
        let text = format!("User: {ask}\nAgent: {reply}");
        let msg = msg_id.to_owned();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            if let Err(e) = me
                .memory
                .write_pass(&msg, &chat, "assistant", &text, &[])
                .await
            {
                tracing::debug!(error = %e, "memory write pass");
            }
            if let Err(e) = me.memory.maybe_compact(&chat).await {
                tracing::debug!(error = %e, "compaction");
            }
            let _ = me.write_system_prompt();
            if me.tree_mode() != crate::tree::TreeMode::Off {
                if let Err(e) = me.tree.build_pass().await {
                    tracing::debug!(error = %e, "memory tree build pass");
                }
            }
        });
    }

    /// The tree mode for this Project (`decision_model.memory_tree`, else the box env).
    pub fn tree_mode(&self) -> crate::tree::TreeMode {
        self.settings
            .models
            .try_read()
            .map(|m| m.decision_cfg.memory_tree)
            .unwrap_or(self.cfg.decision_cfg.memory_tree)
    }

    /// Whether `chat_id` sees only its own subtree (people and group chats, D72/D69).
    pub fn tree_restricted(&self, chat_id: &str) -> bool {
        self.people.is_people_chat(chat_id)
            || self
                .groups
                .kind_of(chat_id)
                .map(|k| k != "chat")
                .unwrap_or(false)
    }

    /// Walk the memory tree for one turn or subagent launch; `None` = inject nothing.
    pub async fn tree_pack(
        &self,
        chat_id: &str,
        message_id: Option<&str>,
        purpose: &str,
        query_text: &str,
    ) -> Option<String> {
        let mode = self.tree_mode();
        if mode == crate::tree::TreeMode::Off {
            return None;
        }
        let (scorer, asker) = {
            let m = self.settings.models.read().await;
            (
                m.decision_cfg.scorer.clone(),
                self.people
                    .asker(chat_id)
                    .map(|a| format!("{} ({})", a.spoken(), a.role))
                    .unwrap_or_else(|| "the project owner".to_owned()),
            )
        };
        let recent = self
            .store
            .t3_thread_before(chat_id, None, 12)
            .map(|rows| json!({ "n": rows.len(), "ids": rows.iter().map(|r| r.id.clone()).collect::<Vec<_>>() }))
            .unwrap_or(Value::Null);
        self.tree
            .pack_for_turn(
                mode,
                &scorer,
                chat_id,
                message_id,
                purpose,
                self.tree_restricted(chat_id),
                query_text,
                &asker,
                &recent,
            )
            .await
    }

    /// Periodic builder: new leaves become episodes and cards even when no turn runs.
    pub fn spawn_tree_builder(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let me = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(20)).await;
            loop {
                if me.tree_mode() != crate::tree::TreeMode::Off {
                    match me.tree.build_pass().await {
                        Ok(stats) => {
                            if stats.get("cards").and_then(Value::as_i64).unwrap_or(0) > 0
                                || stats.get("leaves").and_then(Value::as_i64).unwrap_or(0) > 0
                            {
                                tracing::info!(stats = %stats, "memory tree build pass");
                            }
                        }
                        Err(e) => tracing::debug!(error = %e, "memory tree build pass"),
                    }
                }
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        })
    }

    /// One prompt to the main pi session, folded into a new assistant message. The caller holds
    /// `turn_lock`. Finalizes the message (decision, usage, status), T3, and turn metrics.
    ///
    /// The assistant / "Working…" row is created *before* `ensure_chat_session`: a session or
    /// pi failure must still leave a visible error (with Retry), not a silent Failed command.
    async fn drive_turn(
        self: &Arc<Self>,
        chat_id: &str,
        host: &SharedHost,
        user_msg_id: &str,
        prompt_text: &str,
        kind: TurnKind,
    ) -> Result<TurnOutcome> {
        let assistant_msg_id = ulid();
        let text_part_id = {
            let mut h = host.lock().await;
            let pid =
                h.doc
                    .start_assistant_message(&assistant_msg_id, user_msg_id, &self.cfg.device_id);
            match &kind {
                TurnKind::User => {
                    h.doc
                        .add_status_part(&assistant_msg_id, "working", "Working…");
                    if let Some(attr) = self.people_reply_fields(chat_id) {
                        h.doc.set_message_fields(&assistant_msg_id, &attr.fields);
                    }
                }
                TurnKind::SubagentResult { subagent_id, name } => {
                    h.doc
                        .add_status_part(&assistant_msg_id, "working", "Working…");
                    h.doc.set_message_fields(
                        &assistant_msg_id,
                        &[
                            ("subagentId", json!(subagent_id)),
                            ("subagentName", json!(name)),
                        ],
                    );
                }
                TurnKind::ApprovalOutcome { approval_id, title } => {
                    h.doc.add_status_part(
                        &assistant_msg_id,
                        "working",
                        &format!("Following up on “{title}”…"),
                    );
                    h.doc.set_message_fields(
                        &assistant_msg_id,
                        &[("approvalId", json!(approval_id))],
                    );
                }
            }
            pid
        };

        if let Err(e) = self.ensure_chat_session(chat_id).await {
            {
                let mut h = host.lock().await;
                h.doc.add_status_part(&assistant_msg_id, "failed", "Failed");
                h.doc
                    .add_error_part(&assistant_msg_id, "session", &e.to_string());
                h.doc
                    .finish_message(&assistant_msg_id, MessageStatus::Error);
            }
            return Err(e);
        }

        *self.turn.lock().await = Some(TurnState {
            chat_id: chat_id.to_owned(),
            user_msg_id: user_msg_id.to_owned(),
            assistant_msg_id: assistant_msg_id.clone(),
            text_part_id: text_part_id.clone(),
            started: Instant::now(),
            route: None,
            route_attempts: 0,
            route_failed: false,
            escalate: false,
            asked: None,
            metrics: TurnMetrics::default(),
            streamed_text: String::new(),
            act_transcript: Vec::new(),
            tool_inputs: HashMap::new(),
            reply_extractor: ReplyExtractor::default(),
            ack_extractor: ReplyExtractor::for_key("ack"),
            ack_text: String::new(),
            ack_split: false,
            row_closed: false,
            progress_texts: Vec::new(),
            activity: None,
            prose: ProseFilter::default(),
            act_buffer: String::new(),
            route_call_id: None,
            text_part_dirty: false,
            stopped: false,
            prompt_text: match &kind {
                TurnKind::User => prompt_text.to_owned(),
                TurnKind::SubagentResult { .. } | TurnKind::ApprovalOutcome { .. } => String::new(),
            },
            skills_used: Vec::new(),
        });
        self.turn_running.store(true, Ordering::SeqCst);
        let keepalive = self.spawn_keepalive();
        let result = self.run_main_turn(chat_id, host, prompt_text).await;
        self.turn_running.store(false, Ordering::SeqCst);
        keepalive.abort();

        // Whatever the filter still holds back at the end of the turn is judged and written.
        {
            let mut tl = self.turn.lock().await;
            if let Some(t) = tl.as_mut() {
                let held = t.prose.flush();
                if !held.is_empty() {
                    self.stage_text(host, t, &held).await;
                }
                let final_text = std::mem::take(&mut t.act_buffer);
                if !final_text.trim().is_empty() {
                    self.write_text(host, t, final_text.trim_start()).await;
                }
            }
        }
        let turn = self.turn.lock().await.take();
        let Some(turn) = turn else {
            return Err(anyhow!("turn state vanished mid-turn"));
        };
        // After an ack or progress bubble the live row is a later one than the row this turn
        // opened with; everything below finalizes the row the turn ended on.
        let assistant_msg_id = turn.assistant_msg_id.clone();
        let total_ms = turn.started.elapsed().as_millis() as i64;
        let route_name = turn
            .route
            .as_ref()
            .map(route_name)
            .unwrap_or(if turn.route_failed { "reply" } else { "none" });
        let turn_kind = match &kind {
            TurnKind::User => route_name,
            TurnKind::SubagentResult { .. } => "subagent_result",
            TurnKind::ApprovalOutcome { .. } => "approval_outcome",
        };
        let (final_status, err_text) = match &result {
            Ok(()) => (MessageStatus::Done, None),
            Err(e) => (MessageStatus::Error, Some(e.to_string())),
        };
        let (talking_model, _) = self.live_models().await;
        let room_name;
        {
            let mut h = host.lock().await;
            if let Some(e) = &err_text {
                h.doc.add_error_part(&assistant_msg_id, "turn_failed", e);
            }
            if turn.activity.is_some() {
                h.doc.set_activity(&assistant_msg_id, None);
            }
            h.doc.set_message_fields(
                &assistant_msg_id,
                &[
                    ("decision", json!(route_name)),
                    ("turn", json!({ "kind": turn_kind })),
                    ("model", json!(talking_model)),
                    ("usage", turn.metrics.usage.clone().unwrap_or(Value::Null)),
                ],
            );
            if turn.ack_split && !turn.row_closed {
                h.doc
                    .set_message_fields(&assistant_msg_id, &[("turnRole", json!("result"))]);
            }
            h.doc.finish_message(&assistant_msg_id, final_status);
            room_name = h.room.clone();
        }
        if turn.activity.is_some() {
            self.post_activity(chat_id, None);
        }
        // One notification per turn: the result, or the ack when the turn ended on it.
        let announce = if turn.streamed_text.trim().is_empty() {
            turn.ack_text.as_str()
        } else {
            turn.streamed_text.as_str()
        };
        if matches!(final_status, MessageStatus::Done) && !announce.trim().is_empty() {
            self.announce_reply(&room_name, &assistant_msg_id, announce);
        }
        if !turn.ack_text.trim().is_empty() {
            let _ = self.store.t3_append(
                chat_id,
                None,
                Some(chat_id),
                "assistant",
                Some("assistant"),
                turn.ack_text.trim(),
                Some(&json!({ "ack": true, "route": route_name, "turn": turn_kind })),
            );
        }
        if !turn.streamed_text.trim().is_empty() {
            let _ = self.store.t3_append(
                chat_id,
                None,
                Some(chat_id),
                "assistant",
                Some("assistant"),
                &turn.streamed_text,
                Some(&json!({ "message_id": assistant_msg_id, "route": route_name, "turn": turn_kind })),
            );
        }
        let mut usage_rec = turn.metrics.usage.clone().unwrap_or(json!({}));
        if turn.metrics.route_missing {
            usage_rec["route_missing"] = json!(true);
        }
        if turn.metrics.route_corrected {
            usage_rec["route_corrected"] = json!(true);
        }
        let _ = self.store.record_turn(
            chat_id,
            user_msg_id,
            Some(turn_kind),
            turn.metrics.ttft_ms,
            turn.metrics.route_ms,
            total_ms,
            turn.metrics.tool_calls,
            turn.escalate,
            Some(&usage_rec),
        );
        if let Some(ttft) = turn.metrics.ttft_ms {
            let _ = self
                .post_timing("main_turn_ttft_ms", ttft, Some(turn_kind))
                .await;
        }
        let _ = self
            .post_timing("main_turn_total_ms", total_ms, Some(turn_kind))
            .await;
        tracing::info!(chat_id, route = route_name, turn = turn_kind, ttft_ms = ?turn.metrics.ttft_ms, route_ms = ?turn.metrics.route_ms, total_ms, tool_calls = turn.metrics.tool_calls, "turn done");
        Ok(TurnOutcome {
            assistant_msg_id,
            route_name,
            turn,
            result,
        })
    }

    /// A finished subagent never speaks to the user directly: its raw output stays hidden and the
    /// main agent writes the update in prose, as a follow-up turn in the same chat (demo review
    /// item 1). Waits for `turn_lock`, so it lands after whatever the user is doing right now.
    #[allow(clippy::too_many_arguments)]
    async fn run_result_turn(
        self: &Arc<Self>,
        chat_id: &str,
        host: &SharedHost,
        user_msg_id: &str,
        subagent_id: &str,
        name: &str,
        status: &str,
        total_ms: i64,
        raw: &str,
        error: Option<&str>,
    ) -> Result<()> {
        let _guard = self.turn_lock.lock().await;
        let ask = {
            let h = host.lock().await;
            h.doc
                .message(user_msg_id)
                .map(|m| m.text())
                .unwrap_or_default()
        };
        let recent = self
            .store
            .t3_thread(chat_id, None, None, 12)
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.kind == "user" || r.kind == "assistant")
            .map(|r| {
                format!(
                    "- {}: {}",
                    r.role.unwrap_or_default(),
                    r.text
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .chars()
                        .take(240)
                        .collect::<String>()
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let raw_trim: String = raw.chars().take(7000).collect();
        let mut prompt = format!(
            "[Arbos] Subagent \"{name}\" finished · status {status} · {:.0} s.\n\nThe user originally asked: {}\n",
            total_ms as f64 / 1000.0,
            if ask.trim().is_empty() { "(see the brief)" } else { ask.trim() },
        );
        if !recent.is_empty() {
            prompt.push_str(&format!("\nRecent turns in this chat:\n{recent}\n"));
        }
        if let Some(e) = error {
            prompt.push_str(&format!("\nThe run ended with an error: {e}\n"));
        }
        prompt.push_str(&format!(
            "\nSubagent output (for you only; the user has not seen it):\n<<<\n{raw_trim}\n>>>\n\nRoute `reply` and write the update for the user like a colleague in chat: what got done and the outcome, what changed that they care about, and what is open or needs their answer. Speak as if you did the work; do not mention the subagent, its name, the harness, the model, tool calls, commands, scratch paths, or token counts. No JSON, no quoted output, no list of steps. Two to five sentences."
        ));
        let kind = TurnKind::SubagentResult {
            subagent_id: subagent_id.to_owned(),
            name: name.to_owned(),
        };
        let outcome = self
            .drive_turn(chat_id, host, user_msg_id, &prompt, kind)
            .await?;
        // A result turn relays; it does not start further work. Any other route is logged only.
        if !matches!(outcome.turn.route, Some(ValidRoute::Reply { .. }) | None) {
            tracing::warn!(
                route = outcome.route_name,
                "result turn routed away from reply; ignored"
            );
        }
        self.spawn_memory_pass(
            chat_id,
            &outcome.assistant_msg_id,
            &format!("[subagent {name} finished: {status}]"),
            &outcome.turn.streamed_text,
        );
        outcome.result
    }

    /// A `[Arbos]` notice to the main agent as its own turn in `chat_id` (approval outcomes).
    /// Waits for `turn_lock`, so it lands after whatever the user is doing right now.
    pub async fn run_notice_turn(
        self: &Arc<Self>,
        chat_id: &str,
        host: &SharedHost,
        in_reply_to: &str,
        prompt: &str,
        kind: TurnKind,
    ) -> Result<()> {
        let _guard = self.turn_lock.lock().await;
        let outcome = self
            .drive_turn(chat_id, host, in_reply_to, prompt, kind)
            .await?;
        self.spawn_memory_pass(
            chat_id,
            &outcome.assistant_msg_id,
            "[approval outcome]",
            &outcome.turn.streamed_text,
        );
        outcome.result
    }

    fn spawn_keepalive(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let me = self.clone();
        tokio::spawn(async move {
            let _ = me.keepalive(true).await;
            let mut t = tokio::time::interval(Duration::from_secs(5 * 60));
            t.tick().await;
            loop {
                t.tick().await;
                let _ = me.keepalive(true).await;
            }
        })
    }

    /// Rename a chat: registry (`PATCH /projects/{id}/chats/{chat}`), local index, and the project
    /// doc `chats` list on the next poll.
    pub async fn retitle(&self, chat_id: &str, title: &str) -> Result<()> {
        if title.is_empty() {
            return Err(anyhow!("retitle: empty title"));
        }
        if let Ok(Some(c)) = self.store.chat(chat_id) {
            let _ = self
                .store
                .upsert_chat(chat_id, title, c.is_pinned, c.created_at);
        }
        let r = self
            .http
            .patch(format!(
                "{}/projects/{}/chats/{}",
                self.cfg.relay_url, self.cfg.project_id, chat_id
            ))
            .bearer_auth(&self.cfg.box_token)
            .json(&json!({ "title": title }))
            .send()
            .await?;
        if !r.status().is_success() {
            return Err(anyhow!("retitle: relay HTTP {}", r.status()));
        }
        Ok(())
    }

    pub async fn keepalive(&self, turn_running: bool) -> Result<()> {
        self.http
            .post(format!("{}/box/keepalive", self.cfg.relay_url))
            .bearer_auth(&self.cfg.box_token)
            .json(&json!({ "turn_running": turn_running }))
            .send()
            .await?;
        Ok(())
    }

    /// Tell the relay a whole reply landed, so people who are not looking get a notification
    /// (`POST /chats/{id}/notify` for a people chat, `POST /projects/{p}/chats/{c}/notify`
    /// otherwise). Fire-and-forget: the message is in the doc either way.
    pub fn announce_reply(self: &Arc<Self>, room: &str, message_id: &str, text: &str) {
        let Some((project, chat_id)) = room.split_once('/') else {
            return;
        };
        let url = if project == crate::people::PEOPLE_PROJECT {
            format!("{}/chats/{chat_id}/notify", self.cfg.relay_url)
        } else {
            format!(
                "{}/projects/{}/chats/{chat_id}/notify",
                self.cfg.relay_url, self.cfg.project_id
            )
        };
        let body = json!({
            "message_id": message_id,
            "text": text.chars().take(600).collect::<String>(),
            "project_id": self.cfg.project_id,
        });
        let me = self.clone();
        tokio::spawn(async move {
            if let Err(e) = me
                .http
                .post(url)
                .bearer_auth(&me.cfg.box_token)
                .json(&body)
                .send()
                .await
            {
                tracing::debug!(error = %e, "notify relay of reply failed");
            }
        });
    }

    pub async fn post_timing(&self, kind: &str, ms: i64, detail: Option<&str>) -> Result<()> {
        self.http
            .post(format!(
                "{}/projects/{}/timings",
                self.cfg.relay_url, self.cfg.project_id
            ))
            .bearer_auth(&self.cfg.box_token)
            .json(&json!({ "kind": kind, "ms": ms, "detail": detail }))
            .send()
            .await?;
        Ok(())
    }

    /// Upload a blob to the relay mirror, fire-and-forget. Returns the key.
    pub fn upload_blob(self: &Arc<Self>, bytes: Vec<u8>, mime: &str) -> String {
        use sha2::Digest;
        let key = hex::encode(sha2::Sha256::digest(&bytes));
        let me = self.clone();
        let mime = mime.to_owned();
        let k = key.clone();
        tokio::spawn(async move {
            let _ = me
                .http
                .put(format!(
                    "{}/blob/{}/{}",
                    me.cfg.relay_url, me.cfg.project_id, k
                ))
                .bearer_auth(&me.cfg.box_token)
                .header("content-type", mime)
                .body(bytes)
                .send()
                .await;
        });
        key
    }

    // ---- the main turn ------------------------------------------------------------------------

    async fn run_main_turn(
        self: &Arc<Self>,
        chat_id: &str,
        host: &SharedHost,
        prompt_text: &str,
    ) -> Result<()> {
        let pi = self.pi().await?;
        let mut events = pi.subscribe();
        let images = std::mem::take(&mut *self.turn_images.lock().expect("turn images"));
        let r = pi
            .call(RpcCommand::Prompt {
                id: pi.next_id("p"),
                message: prompt_text.to_owned(),
                streaming_behavior: None,
                images,
            })
            .await?;
        if !r.success {
            return Err(anyhow!("prompt rejected: {}", r.error.unwrap_or_default()));
        }
        let started = Instant::now();
        let mut deadline_announced = false;
        let deadline = Duration::from_secs(turn_deadline_secs());
        // Time the turn spends blocked on an approval card is the user's, not the agent's: it
        // does not count toward the turn deadline.
        let mut approval_paused = Duration::ZERO;
        let mut approval_pause_started: Option<Instant> = None;
        loop {
            let ev = tokio::select! {
                e = events.recv() => e,
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    let waiting = self.approval_pending_for(chat_id).await;
                    match (waiting, approval_pause_started) {
                        (true, None) => approval_pause_started = Some(Instant::now()),
                        (false, Some(t0)) => {
                            approval_paused += t0.elapsed();
                            approval_pause_started = None;
                        }
                        (true, Some(_)) | (false, None) => {}
                    }
                    if waiting {
                        continue;
                    }
                    let elapsed = started.elapsed().saturating_sub(approval_paused);
                    // Turn deadline: announce in chat, hand remaining work to a subagent, never silent.
                    if elapsed > deadline && !deadline_announced {
                        deadline_announced = true;
                        if let Some(t) = self.turn.lock().await.as_mut() {
                            t.escalate = true;
                        }
                        self.post_limit_notice(
                            host,
                            "I hit the turn time limit; I'm handing the rest to a subagent.",
                        )
                        .await;
                        let _ = pi
                            .call(RpcCommand::Abort {
                                id: pi.next_id("ab"),
                            })
                            .await;
                    }
                    continue;
                }
            };
            let ev = match ev {
                Ok(e) => e,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(n, "lagged behind pi events");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(anyhow!("pi event stream closed"))
                }
            };
            if self.fold_main_event(chat_id, host, &ev, started).await? {
                break;
            }
            if matches!(ev, Event::Other) && !pi.is_alive() {
                return Err(anyhow!("pi main child exited mid-turn"));
            }
        }
        Ok(())
    }

    /// Fold one pi event into the chat doc and the turn state. Returns true when the turn settled.
    async fn fold_main_event(
        self: &Arc<Self>,
        chat_id: &str,
        host: &SharedHost,
        ev: &Event,
        started: Instant,
    ) -> Result<bool> {
        let mut tl = self.turn.lock().await;
        let Some(t) = tl.as_mut() else {
            return Ok(true);
        };
        let elapsed = started.elapsed().as_millis() as i64;
        match ev {
            Event::MessageUpdate {
                assistant_message_event,
                usage,
            } => {
                if let Some(u) = usage {
                    if u.total_tokens > 0 {
                        t.metrics.usage = Some(
                            json!({ "input": u.input, "output": u.output, "cacheRead": u.cache_read, "cacheWrite": u.cache_write, "cost": u.cost }),
                        );
                    }
                }
                match assistant_message_event {
                    AssistantEvent::TextDelta { delta, .. } => {
                        if t.metrics.ttft_ms.is_none() {
                            t.metrics.ttft_ms = Some(elapsed);
                        }
                        self.append_text(host, t, delta).await;
                    }
                    AssistantEvent::ThinkingDelta { .. } => {
                        if t.metrics.ttft_ms.is_none() {
                            t.metrics.ttft_ms = Some(elapsed);
                        }
                    }
                    AssistantEvent::ToolcallStart { id, tool_name, .. } => {
                        if t.metrics.ttft_ms.is_none() {
                            t.metrics.ttft_ms = Some(elapsed);
                        }
                        if tool_name == "route" {
                            t.route_call_id = Some(id.clone());
                            t.reply_extractor = ReplyExtractor::default();
                        }
                    }
                    AssistantEvent::ToolcallDelta {
                        delta,
                        content_index: _,
                    } => {
                        if t.route_call_id.is_some() && t.route.is_none() {
                            // The ack streams first (schema order) and becomes its own bubble;
                            // a short `reply` streams into the same row as today.
                            let ack = t.ack_extractor.feed(delta);
                            if !ack.is_empty() {
                                if t.metrics.ttft_ms.is_none() {
                                    t.metrics.ttft_ms = Some(elapsed);
                                }
                                self.write_ack(host, t, &ack).await;
                            }
                            self.maybe_split_ack(host, t, false).await;
                            let text = t.reply_extractor.feed(delta);
                            if !text.is_empty() {
                                self.append_text(host, t, &text).await;
                            }
                        }
                    }
                    AssistantEvent::ToolcallEnd { .. }
                    | AssistantEvent::Start
                    | AssistantEvent::TextStart { .. }
                    | AssistantEvent::TextEnd { .. }
                    | AssistantEvent::ThinkingStart { .. }
                    | AssistantEvent::ThinkingEnd { .. }
                    | AssistantEvent::Done { .. }
                    | AssistantEvent::Unknown => {}
                    AssistantEvent::Error { error, .. } => {
                        let err_text = error
                            .as_ref()
                            .map(|e| e.to_string())
                            .unwrap_or_else(|| "provider error".into());
                        {
                            let mut h = host.lock().await;
                            h.doc
                                .add_error_part(&t.assistant_msg_id, "model_error", &err_text);
                        }
                        if let Some((notice, escalate)) = classify_hard_limit(&err_text) {
                            if escalate {
                                t.escalate = true;
                            }
                            // Visible chat line: never stop silently on a hard limit.
                            self.write_text(host, t, notice).await;
                        }
                    }
                }
            }
            Event::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            } => {
                // Talking to the user is not a tool step: no tool part, no count, no activity.
                if tool_name == "route" || tool_name == "send_message" {
                    return Ok(false);
                }
                // A tool is about to run: the ack sentence (if any) is complete by now.
                self.maybe_split_ack(host, t, true).await;
                self.set_activity(host, t, crate::activity::label(tool_name, args))
                    .await;
                t.tool_inputs.insert(tool_call_id.clone(), args.clone());
                t.metrics.tool_calls += 1;
                let one_line = args.to_string().chars().take(120).collect::<String>();
                t.act_transcript.push(format!("→ {tool_name} {one_line}"));
                // No `step` event in the doc (one working indicator only); the hidden tool part
                // and T3 keep the detail for the drawer.
                let _ = self.store.t3_append(
                    chat_id,
                    None,
                    Some(chat_id),
                    "tool_call",
                    Some("assistant"),
                    &step_label(tool_name, args),
                    Some(&json!({ "call_id": tool_call_id, "input": args })),
                );
                let held = t.prose.flush();
                if !held.is_empty() {
                    self.stage_text(host, t, &held).await;
                }
                let interim = std::mem::take(&mut t.act_buffer);
                if !interim.trim().is_empty() {
                    let _ = self.store.t3_append(
                        chat_id,
                        None,
                        Some(chat_id),
                        "interim",
                        Some("assistant"),
                        interim.trim(),
                        Some(&json!({ "before_call": tool_call_id })),
                    );
                }
                t.text_part_dirty = true;
            }
            Event::ToolExecutionEnd {
                tool_call_id,
                tool_name,
                result,
                is_error,
                ..
            } => {
                if tool_name == "route" {
                    if t.metrics.route_ms.is_none() {
                        t.metrics.route_ms = Some(elapsed);
                    }
                    // The route is validated (hook) by now: an ack that streamed becomes its
                    // own bubble, and an act turn gets its result row.
                    self.maybe_split_ack(host, t, true).await;
                    return Ok(false);
                }
                if tool_name == "send_message" {
                    return Ok(false);
                }
                let output = result
                    .get("content")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|c| c.get("text").and_then(Value::as_str))
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                // D19 at write time: the doc copy gets the same redaction as T3.
                let output = crate::store::redact(&output);
                t.act_transcript.push(format!(
                    "← {tool_name}: {}",
                    output.chars().take(300).collect::<String>()
                ));
                let _ = self.store.t3_append(
                    chat_id,
                    None,
                    Some(chat_id),
                    "tool_output",
                    Some("tool"),
                    &format!("{tool_name}: {output}"),
                    Some(&json!({ "call_id": tool_call_id })),
                );
                let summary: String = output.chars().take(TOOL_OUTPUT_SUMMARY_MAX).collect();
                let output_ref = if output.len() > TOOL_OUTPUT_SUMMARY_MAX {
                    Some(self.upload_blob(output.clone().into_bytes(), "text/plain"))
                } else {
                    None
                };
                let input = t.tool_inputs.remove(tool_call_id).unwrap_or(json!({}));
                {
                    let mut h = host.lock().await;
                    h.doc.add_tool_part(
                        &t.assistant_msg_id,
                        tool_call_id,
                        tool_name,
                        &input,
                        &summary,
                        output_ref.as_deref(),
                        Some(output.len() as i64),
                        *is_error,
                    );
                }
                t.text_part_dirty = true;
                if *is_error {
                    if let Some((notice, escalate)) = classify_hard_limit(&output) {
                        if escalate {
                            t.escalate = true;
                        }
                        self.write_text(host, t, notice).await;
                    }
                }
            }
            Event::AgentSettled { .. } => return Ok(true),
            Event::CompactionEnd { .. } => {
                let _ = self.store.bump_session_compactions(chat_id);
                let _ = self.store.t3_append(
                    chat_id,
                    None,
                    Some(chat_id),
                    "compaction",
                    Some("system"),
                    "pi session compacted",
                    None,
                );
            }
            Event::Other
            | Event::AgentStart
            | Event::AgentEnd { .. }
            | Event::TurnStart
            | Event::TurnEnd { .. }
            | Event::MessageStart { .. }
            | Event::MessageEnd { .. }
            | Event::ToolExecutionUpdate { .. }
            | Event::QueueUpdate { .. }
            | Event::EntryAppended { .. }
            | Event::CompactionStart { .. }
            | Event::ExtensionUiRequest { .. } => {}
        }
        Ok(false)
    }

    /// Main-agent prose enters the doc through the boilerplate filter.
    async fn append_text(&self, host: &SharedHost, t: &mut TurnState, delta: &str) {
        let out = t.prose.feed(delta);
        if !out.is_empty() {
            // The model is talking now, not running a tool.
            self.set_activity(host, t, None).await;
            self.stage_text(host, t, &out).await;
        }
    }

    /// Buffer `route.ack` until it is complete, then strip thought-aloud and write one clean
    /// sentence (agent-flow contract §1). Streaming raw deltas would flash planning text.
    async fn write_ack(&self, _host: &SharedHost, t: &mut TurnState, delta: &str) {
        t.ack_text.push_str(delta);
    }

    /// Once the ack is complete and the action is known: `act` → the ack row closes and a
    /// result row opens; `new`/`wake` → the ack row closes and the turn has nothing more to
    /// show; `reply` → one row, as before. `force` treats a still-open ack string as complete
    /// (the arguments have finished streaming by the time a tool runs).
    async fn maybe_split_ack(&self, host: &SharedHost, t: &mut TurnState, force: bool) {
        if t.ack_split || t.ack_text.trim().is_empty() {
            return;
        }
        if !t.ack_extractor.is_done() && !force {
            return;
        }
        let action: Option<String> = match &t.route {
            Some(r) => Some(route_name(r).to_owned()),
            None => t.ack_extractor.action_seen().map(str::to_owned),
        };
        let action = match (action, force) {
            (Some(a), _) => a,
            (None, true) => "act".to_owned(),
            (None, false) => return,
        };
        // Drop planning / routing thoughts and trailing tool-narration; keep only the
        // user-facing sentence(s), with glued ".Word" spacing fixed.
        let cleaned = ProseFilter::clean_prose(t.ack_text.trim());
        let cleaned = cleaned.trim().to_owned();
        if cleaned.is_empty() {
            t.ack_text.clear();
            t.ack_split = true;
            if action != "reply" && !t.row_closed {
                self.close_row(host, t, "ack").await;
            }
            if action == "act" {
                self.open_row(host, t, "result").await;
            }
            tracing::info!(chat_id = %t.chat_id, action, "ack bubble dropped (thought-aloud only)");
            return;
        }
        t.ack_text = cleaned;
        if t.row_closed {
            self.open_row(host, t, "ack").await;
        }
        {
            let mut h = host.lock().await;
            h.doc
                .append_text(&t.assistant_msg_id, &t.text_part_id, &t.ack_text);
        }
        t.ack_split = true;
        match action.as_str() {
            "reply" => {}
            "act" => {
                self.close_row(host, t, "ack").await;
                self.open_row(host, t, "result").await;
            }
            _ => self.close_row(host, t, "ack").await,
        }
        tracing::info!(chat_id = %t.chat_id, action, ack_chars = t.ack_text.chars().count(), "ack bubble");
    }

    /// Open a fresh streaming assistant row for this turn (after an ack or a progress bubble).
    async fn open_row(&self, host: &SharedHost, t: &mut TurnState, role: &str) {
        let id = ulid();
        let mut h = host.lock().await;
        let pid = h
            .doc
            .start_assistant_message(&id, &t.user_msg_id, &self.cfg.device_id);
        h.doc.add_status_part(&id, "working", "Working…");
        h.doc.set_message_fields(&id, &[("turnRole", json!(role))]);
        if let Some(attr) = self.people_reply_fields(&t.chat_id) {
            h.doc.set_message_fields(&id, &attr.fields);
        }
        if let Some(a) = &t.activity {
            h.doc.set_activity(&id, Some(a));
        }
        t.assistant_msg_id = id;
        t.text_part_id = pid;
        t.text_part_dirty = false;
        t.row_closed = false;
    }

    /// Close the live row as a finished prose bubble (ack or progress).
    async fn close_row(&self, host: &SharedHost, t: &mut TurnState, role: &str) {
        let mut h = host.lock().await;
        if t.activity.is_some() {
            h.doc.set_activity(&t.assistant_msg_id, None);
        }
        h.doc
            .set_message_fields(&t.assistant_msg_id, &[("turnRole", json!(role))]);
        h.doc
            .finish_message(&t.assistant_msg_id, MessageStatus::Done);
        t.row_closed = true;
    }

    /// The working line's text (agent-flow contract §2): on the live row and mirrored to the relay.
    async fn set_activity(&self, host: &SharedHost, t: &mut TurnState, activity: Option<String>) {
        if t.activity == activity {
            return;
        }
        t.activity = activity.clone();
        if !t.row_closed {
            let mut h = host.lock().await;
            h.doc.set_activity(&t.assistant_msg_id, activity.as_deref());
        }
        self.post_activity(&t.chat_id, activity);
    }

    /// `POST /projects/{id}/activity` so lists and sidebars can show it without the chat doc.
    /// Fire-and-forget: the doc field is the source of truth.
    pub fn post_activity(&self, chat_id: &str, activity: Option<String>) {
        let url = format!(
            "{}/projects/{}/activity",
            self.cfg.relay_url, self.cfg.project_id
        );
        let body = json!({ "chat_id": chat_id, "text": activity });
        let http = self.http.clone();
        let token = self.cfg.box_token.clone();
        tokio::spawn(async move {
            if let Err(e) = http
                .post(url)
                .bearer_auth(token)
                .json(&body)
                .timeout(Duration::from_secs(5))
                .send()
                .await
            {
                tracing::debug!(error = %e, "post activity to relay failed");
            }
        });
    }

    /// `send_message(text, activity?)` from the main agent: a progress bubble now, as its own
    /// row, while the turn keeps running (agent-flow contract §1).
    pub async fn hook_send_message(
        &self,
        text: &str,
        activity: Option<&str>,
    ) -> Result<(), String> {
        let text = ProseFilter::clean_prose(text.trim());
        let text = text.trim();
        if text.is_empty() && activity.is_none() {
            return Err("nothing to send".into());
        }
        let mut tl = self.turn.lock().await;
        let Some(t) = tl.as_mut() else {
            return Err("no turn is running".into());
        };
        let Some(host) = self.sync.host(&t.chat_id).await else {
            return Err("no room for this chat".into());
        };
        if !text.is_empty() {
            self.maybe_split_ack(&host, t, true).await;
            // Narration the act turn held back is not the message: it stays in T3 only.
            let mut interim = std::mem::take(&mut t.act_buffer);
            interim.push_str(&t.prose.flush());
            if !interim.trim().is_empty() {
                let _ = self.store.t3_append(
                    &t.chat_id,
                    None,
                    Some(&t.chat_id),
                    "interim",
                    Some("assistant"),
                    interim.trim(),
                    Some(&json!({ "before": "send_message" })),
                );
            }
            if t.row_closed {
                self.open_row(&host, t, "progress").await;
            }
            {
                let mut h = host.lock().await;
                if t.text_part_dirty {
                    if let Some(p) = h.doc.add_text_part(&t.assistant_msg_id, "text") {
                        t.text_part_id = p;
                    }
                    t.text_part_dirty = false;
                }
                h.doc
                    .append_text(&t.assistant_msg_id, &t.text_part_id, text);
            }
            t.progress_texts.push(text.to_owned());
            self.close_row(&host, t, "progress").await;
            self.open_row(&host, t, "result").await;
            let _ = self.store.t3_append(
                &t.chat_id,
                None,
                Some(&t.chat_id),
                "assistant",
                Some("assistant"),
                text,
                Some(&json!({ "message_id": t.assistant_msg_id, "progress": true })),
            );
            tracing::info!(chat_id = %t.chat_id, chars = text.chars().count(), "progress bubble");
        }
        if let Some(a) = activity {
            self.set_activity(&host, t, Some(a.to_owned())).await;
        }
        Ok(())
    }

    /// Reply turns stream straight into the doc; act turns hold the text until the turn settles
    /// (only what follows the last tool call is the answer).
    async fn stage_text(&self, host: &SharedHost, t: &mut TurnState, text: &str) {
        if matches!(t.route, Some(ValidRoute::Act { .. })) {
            t.act_buffer.push_str(text);
        } else {
            self.write_text(host, t, text).await;
        }
    }

    async fn write_text(&self, host: &SharedHost, t: &mut TurnState, delta: &str) {
        if t.row_closed {
            // The turn said it was over (ack only) but the model kept talking: give it a row.
            self.open_row(host, t, "result").await;
        }
        let mut h = host.lock().await;
        if t.text_part_dirty {
            // A tool part landed since the last text: open a new text part so order is kept.
            if let Some(p) = h.doc.add_text_part(&t.assistant_msg_id, "text") {
                t.text_part_id = p;
            }
            t.text_part_dirty = false;
        }
        h.doc
            .append_text(&t.assistant_msg_id, &t.text_part_id, delta);
        t.streamed_text.push_str(delta);
    }

    // ---- hooks called by arbos-pi-ext ---------------------------------------------------------

    /// `/hook/route`: validate; one retry; a second failure is treated as `reply` (design §6.2).
    ///
    /// For `act` / `new` / `wake`, the ack bubble must land here — not only from streamed
    /// tool-arg deltas. Many providers deliver the full `route` arguments only when the
    /// tool runs, so `ToolcallDelta` never feeds `ack_extractor`; without this, the chat
    /// shows a bare "Working…" line until the result (agent-flow contract §1).
    pub async fn hook_route(&self, args: &Value) -> Result<(), String> {
        let mut tl = self.turn.lock().await;
        let Some(t) = tl.as_mut() else {
            return Err("no turn is running".into());
        };
        t.route_attempts += 1;
        match validate(args) {
            Ok(r) => {
                let action = route_name(&r);
                tracing::info!(route = action, attempt = t.route_attempts, "route accepted");
                let _ = self.store.t3_append(
                    &t.chat_id,
                    None,
                    Some(&t.chat_id),
                    "route",
                    Some("system"),
                    &format!("route {action}: {args}"),
                    None,
                );
                t.route = Some(r);
                if action != "reply" {
                    if let Some(host) = self.sync.host(&t.chat_id).await {
                        if t.ack_text.trim().is_empty() {
                            let ack = args
                                .get("ack")
                                .and_then(Value::as_str)
                                .map(str::trim)
                                .filter(|s| !s.is_empty())
                                .unwrap_or("On it.");
                            self.write_ack(&host, t, ack).await;
                        }
                        self.maybe_split_ack(&host, t, true).await;
                    }
                }
                Ok(())
            }
            Err(e) => {
                tracing::warn!(error = %e, attempt = t.route_attempts, "route invalid");
                if t.route_attempts >= 2 {
                    t.route_failed = true;
                    t.route = Some(ValidRoute::Reply { reply: None });
                }
                Err(e)
            }
        }
    }

    /// `ask_user(question, widget?)` from any harness seam. Writes the `question` part (with the
    /// typed widget when one was given) plus the legacy `waiting_user` status part, and records
    /// the ask so the `answer` command can render the value for the model. Returns the question id.
    ///
    /// When the turn is in a side thread but the prompt came from another agent (group /
    /// Projects ⇄), the card is posted on the pinned main chat so the owner can find it.
    pub async fn hook_ask_user(
        &self,
        question: &str,
        widget: Option<&Value>,
        session_id: &str,
    ) -> Result<String> {
        let widget = match Widget::parse(widget) {
            Ok(w) => w,
            Err(e) => {
                // An invalid widget degrades to a plain question rather than losing the turn.
                tracing::warn!(error = %e, "ask_user widget rejected; asking as plain text");
                None
            }
        };
        let widget_json = widget.as_ref().and_then(|w| serde_json::to_value(w).ok());
        let mut tl = self.turn.lock().await;
        let turn_chat = tl
            .as_ref()
            .map(|t| t.chat_id.clone())
            .unwrap_or_else(|| session_id.to_owned());
        let pinned = self
            .store
            .chats()
            .ok()
            .and_then(|cs| cs.into_iter().find(|c| c.is_pinned).map(|c| c.id));
        // Other-agent / system-tab turns: surface the ask on Main, not a buried side thread.
        let from_other_agent = if let Some(t) = tl.as_ref() {
            let in_projects = self
                .groups
                .kind_of(&t.chat_id)
                .map(|k| k != "chat")
                .unwrap_or(false);
            let cross = if let Some(host) = self.sync.host(&t.chat_id).await {
                let h = host.lock().await;
                h.doc
                    .message(&t.user_msg_id)
                    .map(|m| {
                        m.rest.get("groupChatId").is_some()
                            || m.rest.get("hostProjectId").is_some()
                            || m.rest.get("groupTitle").is_some()
                    })
                    .unwrap_or(false)
            } else {
                false
            };
            in_projects || cross
        } else {
            false
        };
        let chat_id = if from_other_agent {
            pinned.unwrap_or_else(|| turn_chat.clone())
        } else {
            turn_chat.clone()
        };
        let id = ulid();
        self.store.insert_ask(
            &id,
            Some(&chat_id),
            Some(session_id),
            question,
            widget_json.as_ref(),
        )?;
        if let Some(t) = tl.as_mut() {
            t.asked = Some(question.to_owned());
            if chat_id == t.chat_id {
                if let Some(host) = self.sync.host(&t.chat_id).await {
                    let mut h = host.lock().await;
                    h.doc.add_question_part(
                        &t.assistant_msg_id,
                        &id,
                        question,
                        widget_json.as_ref(),
                    );
                    h.doc
                        .add_status_part(&t.assistant_msg_id, "waiting_user", question);
                    h.doc
                        .set_message_fields(&t.assistant_msg_id, &[("waitingUser", json!(true))]);
                }
            } else if let Some(host) = self.sync.host(&chat_id).await {
                // Standalone ask on the main chat; the side-thread turn just stops.
                let mut h = host.lock().await;
                let last_user = h
                    .doc
                    .messages()
                    .iter()
                    .rev()
                    .find(|m| m.role == "user")
                    .map(|m| m.id.clone())
                    .unwrap_or_default();
                let msg_id = ulid();
                h.doc
                    .start_assistant_message(&msg_id, &last_user, &self.cfg.device_id);
                h.doc
                    .set_message_fields(&msg_id, &[("turn", json!({ "kind": "ask_user" }))]);
                h.doc
                    .add_question_part(&msg_id, &id, question, widget_json.as_ref());
                h.doc.add_status_part(&msg_id, "waiting_user", question);
                h.doc
                    .set_message_fields(&msg_id, &[("waitingUser", json!(true))]);
                h.doc.finish_message(&msg_id, MessageStatus::Done);
            }
        }
        let _ = self.store.t3_append(
            &chat_id,
            None,
            Some(session_id),
            "ask_user",
            Some("assistant"),
            question,
            Some(&json!({
                "question_id": id,
                "widget": widget_json,
                "posted_chat": chat_id,
            })),
        );
        Ok(id)
    }

    /// Post a one- or two-sentence hard-limit notice into the live chat row (never silent).
    async fn post_limit_notice(&self, host: &SharedHost, text: &str) {
        let mut tl = self.turn.lock().await;
        let Some(t) = tl.as_mut() else {
            return;
        };
        self.maybe_split_ack(host, t, true).await;
        let interim = std::mem::take(&mut t.act_buffer);
        if !interim.trim().is_empty() {
            let _ = self.store.t3_append(
                &t.chat_id,
                None,
                Some(&t.chat_id),
                "interim",
                Some("assistant"),
                interim.trim(),
                Some(&json!({ "before": "limit_notice" })),
            );
        }
        self.write_text(host, t, text).await;
    }

    /// Working-set section (design §6.6 item 2b): running subagents + compaction summary +
    /// handles + notes, ≤ 40 lines.
    pub fn working_set_text(&self, chat_id: &str) -> String {
        let mut lines = Vec::new();
        let running: Vec<_> = self
            .store
            .subagents()
            .unwrap_or_default()
            .into_iter()
            .filter(|s| s.status == "running" || s.status == "queued")
            .collect();
        if !running.is_empty() {
            lines.push(format!(
                "Subagents running now ({}): their results arrive as later [Arbos] messages; do not start the same work twice.",
                running.len()
            ));
            for s in running.iter().take(SUBAGENT_MAX_CONCURRENT + 2) {
                lines.push(format!(
                    "- {} ({}, {}): {}",
                    s.name,
                    s.id,
                    s.status,
                    if s.last_status.is_empty() {
                        "starting"
                    } else {
                        s.last_status.as_str()
                    }
                ));
            }
        }
        if let Ok(Some(c)) = self.store.chat(chat_id) {
            if let Some(s) = c.compaction_summary {
                lines.push(format!(
                    "Summary of earlier turns in this chat: {}",
                    s.chars().take(6000).collect::<String>()
                ));
            }
        }
        for h in self.store.handles_for_chat(chat_id, 20).unwrap_or_default() {
            lines.push(format!(
                "- {} ({}, {} chars, {} lines): {}",
                h.id, h.kind, h.chars, h.lines, h.note
            ));
        }
        for (k, v) in self.store.scratch_all(chat_id).unwrap_or_default() {
            lines.push(format!(
                "- remember:{k} = {}",
                v.chars().take(120).collect::<String>()
            ));
        }
        lines.into_iter().take(40).collect::<Vec<_>>().join("\n")
    }

    /// "Relevant skills" section for the running turn (F5): selected against the prompt text
    /// within `SECTION_BUDGET_TOKENS`; empty when nothing matches or no turn is running. The
    /// first call per turn records the chosen slugs on the assistant message (Details drawer).
    pub async fn skills_section_text(&self) -> String {
        let (chat_id, msg_id, prompt, first) = {
            let mut tl = self.turn.lock().await;
            let Some(t) = tl.as_mut() else {
                return String::new();
            };
            if t.prompt_text.trim().is_empty() {
                return String::new();
            }
            let first = t.skills_used.is_empty();
            (
                t.chat_id.clone(),
                t.assistant_msg_id.clone(),
                t.prompt_text.clone(),
                first,
            )
        };
        let index = self.skills.snapshot();
        let (selected, text) = skills::section_for(&prompt, &index, skills::SECTION_BUDGET_TOKENS);
        if selected.is_empty() {
            return String::new();
        }
        let slugs: Vec<String> = selected.iter().map(|s| s.slug.clone()).collect();
        if first {
            tracing::info!(
                chat_id,
                skills = ?slugs,
                tokens = arbos_core::skill::estimate_tokens(&text),
                "skills injected into the working set"
            );
            if let Some(t) = self.turn.lock().await.as_mut() {
                t.skills_used = slugs.clone();
            }
            if let Some(host) = self.sync.host(&chat_id).await {
                let mut h = host.lock().await;
                h.doc.add_hidden_part(
                    &msg_id,
                    "skills",
                    &text,
                    &json!({ "slugs": slugs, "scores": selected.iter().map(|s| s.score).collect::<Vec<_>>() }),
                );
            }
            let _ = self.store.t3_append(
                &chat_id,
                None,
                Some(&chat_id),
                "skills",
                Some("system"),
                &format!("skills injected: {}", slugs.join(", ")),
                None,
            );
        }
        text
    }

    // ---- subagents ----------------------------------------------------------------------------

    /// Context pack v0 (design §6.7 sections 1–4, 9, 10).
    fn build_pack(&self, name: &str, purpose: &str, brief: &str, data_heavy: bool) -> String {
        let mut s = String::new();
        s.push_str(&format!("# Task: {name}\n\n{purpose}\n\n{brief}\n\n"));
        s.push_str(&format!(
            "## Project folder\n\n`{}` (your working directory). Keep every file of this project there; other folders under `/work` belong to other projects.\n\n",
            self.cfg.work_dir.display()
        ));
        if !self.cfg.instructions.trim().is_empty() {
            s.push_str(&format!(
                "## Project instructions\n\n{}\n\n",
                self.cfg.instructions.trim()
            ));
        }
        let core = self.memory.core_memory_text();
        if !core.trim().is_empty() {
            s.push_str("## Project memory (T0 + T1)\n\n");
            s.push_str(&core);
            s.push('\n');
        }
        if data_heavy {
            s.push_str("## Data handling\n\nInputs are large. Keep data in variables or files and print slices; never paste whole files into the conversation.\n\n");
        }
        // Section 11 (F5): skills matching the brief, same budget as the main agent's section.
        let (selected, skills_text) = skills::section_for(
            &format!("{name} {purpose} {brief}"),
            &self.skills.snapshot(),
            skills::SECTION_BUDGET_TOKENS,
        );
        if !skills_text.is_empty() {
            tracing::info!(subagent = name, skills = ?selected.iter().map(|x| x.slug.as_str()).collect::<Vec<_>>(), "skills injected into the pack");
            s.push_str(&skills_text);
            s.push_str("\n\n");
        }
        s.push_str("## Tools\n\nMCP server `arbos`: context_search, context_open, context_around (this Project's memory tree: raw events plus index cards of episodes and threads), history_search, history_read, read_thread, memory_search, memory_read (you may search and read the entire history of this Project), peek, sub_llm, remember/recall, projects_list, ask_user (optional widget: choice, multi_select, text, date, form), skills_list/skills_read/skills_save (reusable how-tos), draft_message (an outbound email or chat message shown to the user as an approval card; nothing is sent without their click), connectors_list/connector_call (the user's Gmail, Calendar, Slack, GitHub, Harness Comms through the relay; write/send actions need the user's approval), local_devices/local_run/local_read/local_list (the user's own computer through the arbos-local helper; every command and, unless the user allowed reads, every file read needs the user's approval), deliver_file (hand a file you made to the user as a card), generate_image (an image from a prompt, delivered as a card), secrets_list/secrets_request/egress/browser_fill (the owner's secrets: you only ever get a placeholder such as `arbos-secret://STRIPE_KEY/value`, `$STRIPE_KEY` in a shell, or an `op://` reference; send it through `egress` or use it as `$NAME`; never try to read, print, encode or copy a value; a secret with no access → secrets_request and wait for the owner's card; if they deny or pause, stop and say so; logins are typed by browser_fill), routines_list/routines_create/routines_update/routines_delete (cron jobs and one-shot reminders on the relay scheduler), web_search/web_fetch (search the web; read a public page, spec or file as text), submit_feedback (file the user's feedback or bug report for them: captures their screen from their app, attaches the recent conversation, tool calls and timings scrubbed of secrets, and returns a reference id; use it whenever the user asks to submit, send or report feedback or a bug — never point them to a menu, settings screen or another channel).\n\n");
        s.push_str(RESEARCH_SECTION);
        s.push_str(&crate::media::prompt_section(&self.cfg));
        s.push_str("## Return contract\n\nEnd your final reply with one fenced ```json block:\n{ \"status\": \"done\" | \"blocked\" | \"partial\" | \"needs_input\", \"summary\": \"<= 5 lines\", \"artifacts\": [ { \"kind\": \"file\" | \"commit\" | \"url\" | \"command\", \"ref\": \"…\", \"note\": \"…\" } ], \"acceptance\": [ { \"criterion\": \"…\", \"met\": true, \"evidence\": \"…\" } ], \"open_items\": [], \"proposed_memory\": [ { \"kind\": \"decision\", \"scope\": \"project\", \"text\": \"…\" } ], \"questions\": [] }\n");
        s
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn run_subagent(
        self: &Arc<Self>,
        chat_id: &str,
        host: &SharedHost,
        in_reply_to: &str,
        existing: Option<String>,
        name: &str,
        purpose: &str,
        brief: &str,
        route_harness: Option<Harness>,
        data_heavy: bool,
        escalated: bool,
    ) -> Result<()> {
        let default_harness = std::env::var("ARBOS_DEFAULT_HARNESS")
            .ok()
            .and_then(|s| match s.as_str() {
                "pi" => Some(Harness::Pi),
                "claude_code" => Some(Harness::ClaudeCode),
                "prime" => Some(Harness::Prime),
                _ => None,
            })
            .unwrap_or(Harness::Prime);
        let (_, subagent_model) = self.live_models().await;
        let (subagent_id, harness, is_new) = match existing {
            Some(id) => {
                let row = match self.store.subagent(&id)? {
                    Some(r) => r,
                    None => self
                        .store
                        .subagent_by_name(&id)?
                        .ok_or_else(|| anyhow!("unknown subagent {id}"))?,
                };
                let h = match row.harness.as_str() {
                    "pi" => Harness::Pi,
                    "claude_code" => Harness::ClaudeCode,
                    _ => Harness::Prime,
                };
                (row.id, h, false)
            }
            None => {
                let h = pick_harness(
                    route_harness,
                    data_heavy,
                    brief,
                    &subagent_model,
                    default_harness,
                );
                let id = ulid();
                self.store
                    .insert_subagent(&id, name, purpose, h.as_str(), chat_id)?;
                (id, h, true)
            }
        };
        let adapter = self.adapter(harness);
        let sub_name = self
            .store
            .subagent(&subagent_id)?
            .map(|s| s.name)
            .unwrap_or_else(|| name.to_owned());

        // The progress message carries status events and hidden parts only; the main agent writes
        // the prose ack before this and the prose result after (demo review items 1–2).
        let msg_id = ulid();
        {
            let mut h = host.lock().await;
            h.doc
                .start_assistant_message(&msg_id, in_reply_to, &self.cfg.device_id);
            h.doc.set_message_fields(
                &msg_id,
                &[
                    ("subagentId", json!(subagent_id)),
                    ("subagentName", json!(sub_name)),
                    (
                        "turn",
                        json!({ "kind": if is_new { "spawn" } else { "wake" } }),
                    ),
                    ("escalated_from_act", json!(escalated)),
                    ("model", json!(subagent_model)),
                    // Messages exchanged with the subagent so far: the brief now, the
                    // result later (clients say "Messaged X" / "N messages with X").
                    ("handoffMessages", json!(1)),
                ],
            );
        }
        let queued_at = Instant::now();
        if self.subagent_slots.available_permits() == 0 {
            let waiting = self
                .store
                .subagents()
                .unwrap_or_default()
                .iter()
                .filter(|s| s.status == "queued")
                .count();
            let _ = self.store.set_subagent_status(&subagent_id, "queued");
            let _ = self
                .store
                .set_subagent_last_status(&subagent_id, &format!("queued ({waiting} ahead)"));
            let mut h = host.lock().await;
            h.doc
                .add_status_part(&msg_id, "working", "Waiting for a free slot…");
        }
        self.write_threads_index().await;
        let _slot = self
            .subagent_slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| anyhow!("subagent slots closed"))?;
        let _box_slot = self
            .box_slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| anyhow!("box subagent slots closed"))?;
        let queued_ms = queued_at.elapsed().as_millis() as i64;
        if queued_ms > 500 {
            let _ = self
                .post_timing("subagent_queued_ms", queued_ms, Some(harness.as_str()))
                .await;
        }
        {
            let mut h = host.lock().await;
            h.doc.add_status_part(&msg_id, "working", "Working…");
        }
        self.subagent_msgs
            .lock()
            .await
            .insert(subagent_id.clone(), (chat_id.to_owned(), msg_id.clone()));
        let _ = self.store.set_subagent_last_status(&subagent_id, "working");
        let t0 = Instant::now();
        let mut pack = self.build_pack(&sub_name, purpose, brief, data_heavy);
        if let Some(memory_block) = self
            .tree_pack(
                chat_id,
                Some(in_reply_to),
                "subagent",
                &format!("{purpose}\n{brief}"),
            )
            .await
        {
            pack.push_str("\n## Retrieved memory\n\n");
            pack.push_str(&memory_block);
            pack.push('\n');
        }
        std::fs::create_dir_all(self.cfg.state_dir.join("packs"))?;
        std::fs::write(
            self.cfg
                .state_dir
                .join("packs")
                .join(format!("{subagent_id}.md")),
            &pack,
        )?;
        self.store.set_subagent_status(&subagent_id, "running")?;
        self.write_threads_index().await;
        let _ = self.store.t3_append(
            &subagent_id,
            Some(&subagent_id),
            Some(&subagent_id),
            "user",
            Some("user"),
            brief,
            Some(&json!({ "chat_id": chat_id, "pack": "pack-v0" })),
        );

        let mut resumed_fresh = false;
        if is_new {
            adapter
                .start(StartRequest {
                    session_id: subagent_id.clone(),
                    cwd: self.cfg.work_dir.clone(),
                    system_prompt: pack.clone(),
                    model: subagent_model.clone(),
                    thinking: "medium".into(),
                })
                .await?;
            let _ = self
                .post_timing(
                    "subagent_start_ms",
                    t0.elapsed().as_millis() as i64,
                    Some(harness.as_str()),
                )
                .await;
        } else if !adapter.is_open(&subagent_id) {
            match adapter.resume(&subagent_id).await {
                Ok(()) => {
                    let _ = self
                        .post_timing(
                            "subagent_resume_ms",
                            t0.elapsed().as_millis() as i64,
                            Some(harness.as_str()),
                        )
                        .await;
                }
                Err(e) => {
                    // A session that cannot come back (moved box, corrupt store, harness upgrade)
                    // must not sink the hand-off: start the same subagent fresh with its summary.
                    tracing::warn!(subagent = %subagent_id, error = %e, "resume failed; starting the subagent fresh");
                    resumed_fresh = true;
                    adapter
                        .start(StartRequest {
                            session_id: subagent_id.clone(),
                            cwd: self.cfg.work_dir.clone(),
                            system_prompt: pack.clone(),
                            model: subagent_model.clone(),
                            thinking: "medium".into(),
                        })
                        .await
                        .with_context(|| format!("resume failed ({e}); fresh start failed too"))?;
                }
            }
        }
        let stream = adapter
            .stream(&subagent_id)
            .ok_or_else(|| anyhow!("no stream for {subagent_id}"))?;
        let first_prompt = if is_new {
            brief.to_owned()
        } else if resumed_fresh {
            let summary = self
                .store
                .subagent(&subagent_id)
                .ok()
                .flatten()
                .map(|r| r.summary)
                .unwrap_or_default();
            format!(
                "Your previous session could not be resumed, so this is a fresh session for the same thread. What you had done before: {}\n\nContinue:\n\n{brief}",
                if summary.trim().is_empty() { "(no summary recorded)" } else { summary.trim() }
            )
        } else {
            format!("Context update: continue your thread.\n\n{brief}")
        };
        let mut stream = match adapter.prompt(&subagent_id, &first_prompt).await {
            Ok(()) => stream,
            Err(e) if !is_new && !resumed_fresh => {
                // The resumed child died on first contact (broken pipe): same fallback as above.
                tracing::warn!(subagent = %subagent_id, error = %e, "resumed session failed on prompt; starting fresh");
                let _ = adapter.stop(&subagent_id).await;
                adapter
                    .start(StartRequest {
                        session_id: subagent_id.clone(),
                        cwd: self.cfg.work_dir.clone(),
                        system_prompt: pack.clone(),
                        model: subagent_model.clone(),
                        thinking: "medium".into(),
                    })
                    .await
                    .with_context(|| format!("prompt failed ({e}); fresh start failed too"))?;
                let fresh = adapter
                    .stream(&subagent_id)
                    .ok_or_else(|| anyhow!("no stream for {subagent_id}"))?;
                let summary = self
                    .store
                    .subagent(&subagent_id)
                    .ok()
                    .flatten()
                    .map(|r| r.summary)
                    .unwrap_or_default();
                adapter
                    .prompt(
                        &subagent_id,
                        &format!(
                            "Your previous session could not be resumed, so this is a fresh session for the same thread. What you had done before: {}\n\nContinue:\n\n{brief}",
                            if summary.trim().is_empty() { "(no summary recorded)" } else { summary.trim() }
                        ),
                    )
                    .await?;
                fresh
            }
            Err(e) => return Err(e),
        };

        self.subagent_turns.fetch_add(1, Ordering::SeqCst);
        let keepalive = self.spawn_keepalive();
        let mut text = String::new();
        let mut sub_tool_inputs: HashMap<String, Value> = HashMap::new();
        let mut usage_total = json!({});
        let mut first_delta: Option<i64> = None;
        let mut done_err: Option<String> = None;
        let deadline = Instant::now() + Duration::from_secs(60 * 60);
        loop {
            let ev = tokio::select! {
                e = stream.recv() => e,
                _ = tokio::time::sleep_until(deadline.into()) => { done_err = Some("subagent exceeded 60 minutes".into()); break; }
            };
            let ev = match ev {
                Ok(e) => e,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            };
            match ev {
                AgentEvent::TextDelta(d) => {
                    // Raw subagent prose never renders (item 1): kept for the hidden result part.
                    if first_delta.is_none() {
                        first_delta = Some(t0.elapsed().as_millis() as i64);
                    }
                    text.push_str(&d);
                }
                AgentEvent::ReasoningDelta(_) => {}
                AgentEvent::ToolCall { id, name, input } => {
                    sub_tool_inputs.insert(id, input.clone());
                    let label = step_label(&name, &input);
                    let _ = self
                        .store
                        .set_subagent_last_status(&subagent_id, &format!("step: {label}"));
                    let _ = self.store.t3_append(
                        &subagent_id,
                        Some(&subagent_id),
                        Some(&subagent_id),
                        "tool_call",
                        Some("assistant"),
                        &format!("{name} {input}"),
                        None,
                    );
                }
                AgentEvent::ToolResult {
                    id,
                    name,
                    output,
                    is_error,
                } => {
                    let output = crate::store::redact(&output);
                    let _ = self.store.t3_append(
                        &subagent_id,
                        Some(&subagent_id),
                        Some(&subagent_id),
                        "tool_output",
                        Some("tool"),
                        &format!("{name}: {output}"),
                        None,
                    );
                    let summary: String = output.chars().take(TOOL_OUTPUT_SUMMARY_MAX).collect();
                    let output_ref = if output.len() > TOOL_OUTPUT_SUMMARY_MAX {
                        Some(self.upload_blob(output.clone().into_bytes(), "text/plain"))
                    } else {
                        None
                    };
                    let input = sub_tool_inputs.remove(&id).unwrap_or(json!({}));
                    let mut h = host.lock().await;
                    h.doc.add_tool_part(
                        &msg_id,
                        &id,
                        &name,
                        &input,
                        &summary,
                        output_ref.as_deref(),
                        Some(output.len() as i64),
                        is_error,
                    );
                }
                AgentEvent::Usage {
                    input,
                    output,
                    cache_read,
                    cache_write,
                    cost_usd,
                } => {
                    usage_total = json!({ "input": input, "output": output, "cacheRead": cache_read, "cacheWrite": cache_write, "cost": cost_usd });
                }
                AgentEvent::InputRequested { question } => {
                    let qid = ulid();
                    let _ = self.store.insert_ask(
                        &qid,
                        Some(chat_id),
                        Some(&subagent_id),
                        &question,
                        None,
                    );
                    let mut h = host.lock().await;
                    h.doc.add_question_part(&msg_id, &qid, &question, None);
                    h.doc.add_status_part(&msg_id, "waiting_user", &question);
                }
                AgentEvent::CompactionStarted => {}
                AgentEvent::CompactionEnded { summary } => {
                    let n = self
                        .store
                        .bump_session_compactions(&subagent_id)
                        .unwrap_or(0);
                    let _ = self.store.t3_append(
                        &subagent_id,
                        Some(&subagent_id),
                        Some(&subagent_id),
                        "compaction",
                        Some("system"),
                        &format!(
                            "session compacted ({n}); summary {} chars",
                            summary.as_deref().map(str::len).unwrap_or(0)
                        ),
                        None,
                    );
                }
                AgentEvent::Done { aborted, error } => {
                    if aborted || error.is_some() {
                        done_err = error.or(Some("aborted".into()));
                    }
                    break;
                }
            }
        }
        keepalive.abort();
        self.subagent_turns.fetch_sub(1, Ordering::SeqCst);
        let total_ms = t0.elapsed().as_millis() as i64;
        let contract = extract_return_contract(&text);
        let status = contract
            .as_ref()
            .and_then(|c| c.get("status"))
            .and_then(Value::as_str)
            .unwrap_or(if done_err.is_some() {
                "blocked"
            } else {
                "done"
            })
            .to_owned();
        let failed = done_err.is_some() || status == "blocked";
        {
            let mut h = host.lock().await;
            if let Some(e) = &done_err {
                h.doc.add_error_part(&msg_id, "subagent_error", e);
            }
            // Raw output + contract: Details drawer only (`hidden: true`).
            h.doc.add_hidden_part(
                &msg_id,
                "subagent_result",
                &text,
                &json!({
                    "subagentId": subagent_id,
                    "status": status,
                    "returnContract": contract.clone().unwrap_or(Value::Null),
                    "durationMs": total_ms,
                }),
            );
            h.doc.add_status_part(
                &msg_id,
                if failed { "failed" } else { "done" },
                if failed { "Failed" } else { "Done" },
            );
            h.doc.set_message_fields(
                &msg_id,
                &[
                    ("usage", usage_total.clone()),
                    ("returnContract", contract.clone().unwrap_or(Value::Null)),
                ],
            );
            h.doc.finish_message(
                &msg_id,
                if done_err.is_some() {
                    MessageStatus::Error
                } else {
                    MessageStatus::Done
                },
            );
        }
        self.subagent_msgs.lock().await.remove(&subagent_id);
        let _ = self
            .store
            .set_subagent_last_status(&subagent_id, if failed { "failed" } else { "done" });
        let _ = self.store.t3_append(
            &subagent_id,
            Some(&subagent_id),
            Some(&subagent_id),
            "assistant",
            Some("assistant"),
            &text,
            Some(&json!({ "chat_id": chat_id, "message_id": msg_id })),
        );
        let new_status = match status.as_str() {
            "needs_input" => "waiting_user",
            _ => "idle",
        };
        self.store.set_subagent_status(&subagent_id, new_status)?;
        if let Some(c) = &contract {
            if let Some(s) = c.get("summary").and_then(Value::as_str) {
                let _ = self.store.set_subagent_summary(&subagent_id, s, &json!([]));
            }
            if let Some(qs) = c.get("questions").and_then(Value::as_array) {
                for q in qs.iter().filter_map(Value::as_str) {
                    let _ =
                        self.store
                            .insert_ask(&ulid(), Some(chat_id), Some(&subagent_id), q, None);
                }
            }
        }
        let _ = self
            .post_timing("subagent_turn_total_ms", total_ms, Some(harness.as_str()))
            .await;
        if let Some(fd) = first_delta {
            let _ = self
                .post_timing("subagent_ttft_ms", fd, Some(harness.as_str()))
                .await;
        }
        self.write_threads_index().await;
        // T3 index of the subagent's own session file (history_search source) + write pass, off path.
        {
            let me = self.clone();
            let sid = subagent_id.clone();
            let proposals = contract
                .as_ref()
                .and_then(|c| c.get("proposed_memory"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let summary_text = contract
                .as_ref()
                .and_then(|c| c.get("summary"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let text2 = text.clone();
            let chat = chat_id.to_owned();
            tokio::spawn(async move {
                me.index_session_history(&sid, harness).await;
                let body = if summary_text.is_empty() {
                    text2
                } else {
                    summary_text
                };
                let _ = me
                    .memory
                    .write_pass(
                        &format!("subagent:{sid}"),
                        &sid,
                        "subagent",
                        &body,
                        &proposals,
                    )
                    .await;
                let _ = me.memory.maybe_compact(&chat).await;
                let _ = me.write_system_prompt();
            });
        }
        drop(_box_slot);
        drop(_slot);
        // The user hears about it from the main agent, in prose, in this chat.
        {
            let mut h = host.lock().await;
            h.doc
                .set_message_fields(&msg_id, &[("handoffMessages", json!(2))]);
        }
        let result_text = match &contract {
            Some(c) => format!("{text}\n\nParsed return contract:\n{c}"),
            None => text.clone(),
        };
        self.run_result_turn(
            chat_id,
            host,
            in_reply_to,
            &subagent_id,
            &sub_name,
            &status,
            total_ms,
            &result_text,
            done_err.as_deref(),
        )
        .await
    }

    /// Tail the subagent's session JSONL into T3 (pre-compaction entries included).
    pub async fn index_session_history(&self, session_id: &str, harness: Harness) {
        let adapter = self.adapter(harness);
        let cursor = self.store.session_cursor(session_id).ok().flatten();
        match adapter.read_history(session_id, cursor).await {
            Ok((entries, next)) => {
                for e in &entries {
                    if let Some((role, text)) = crate::harness::entry_text(e) {
                        let _ = self.store.t3_append(
                            session_id,
                            Some(session_id),
                            Some(session_id),
                            "session_entry",
                            Some(&role),
                            &text,
                            Some(&json!({ "entry_id": e.get("id") })),
                        );
                    }
                }
                if let Some(c) = next {
                    let _ = self.store.set_session_cursor(session_id, &c);
                }
            }
            Err(e) => tracing::debug!(error = %e, session_id, "history index"),
        }
    }

    /// `threads` (T1 index) + `status` in the project doc.
    pub async fn write_threads_index(&self) {
        let Some(h) = self.sync.host(arbos_core::wire::PROJECT_DOC_CHAT_ID).await else {
            return;
        };
        let h = h.lock().await;
        let threads = h.doc.doc.get_map("threads");
        for s in self.store.subagents().unwrap_or_default() {
            let v = json!({ "name": s.name, "purpose": s.purpose, "summary": s.summary, "tags": s.tags, "status": s.status, "lastTurnAt": s.last_turn_at, "harness": s.harness });
            let lv: loro::LoroValue = serde_json::from_value(v).unwrap_or(loro::LoroValue::Null);
            threads.insert(&s.id, lv).ok();
        }
        h.doc.doc.commit();
    }

    /// MCP `spawn_subagent` / `wake_subagent` from inside an act turn (D66 escalation).
    pub async fn spawn_from_tool(
        self: &Arc<Self>,
        existing: Option<String>,
        name: &str,
        purpose: &str,
        brief: &str,
        isolate: bool,
    ) -> Result<String> {
        let _ = isolate;
        let (chat_id, user_msg) = {
            let t = self.turn.lock().await;
            match t.as_ref() {
                Some(t) => (t.chat_id.clone(), t.user_msg_id.clone()),
                None => (
                    self.store
                        .chats()?
                        .first()
                        .map(|c| c.id.clone())
                        .unwrap_or_default(),
                    String::new(),
                ),
            }
        };
        let host = self
            .sync
            .host(&chat_id)
            .await
            .ok_or_else(|| anyhow!("chat room not open"))?;
        if let Some(t) = self.turn.lock().await.as_mut() {
            t.escalate = false;
            let mut h = host.lock().await;
            h.doc
                .add_status_part(&t.assistant_msg_id, "working", "Working…");
        }
        let me = self.clone();
        let (n, p, b) = (name.to_owned(), purpose.to_owned(), brief.to_owned());
        let id_holder = Arc::new(Mutex::new(String::new()));
        let idh = id_holder.clone();
        tokio::spawn(async move {
            let _ = idh;
            if let Err(e) = me
                .run_subagent(
                    &chat_id, &host, &user_msg, existing, &n, &p, &b, None, false, true,
                )
                .await
            {
                tracing::warn!(error = %e, "spawn_from_tool");
            }
        });
        Ok("subagent started; its result will be posted in this chat".to_owned())
    }

    /// Stop every child in order (subagents, then pi) so session files and kernel snapshots land.
    pub async fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        self.pi_ready.store(false, Ordering::SeqCst);
        for a in self.adapters.values() {
            a.shutdown_all().await;
        }
        if let Some(pi) = self.pi.lock().await.take() {
            pi.shutdown().await;
        }
    }

    /// True while the main agent or any subagent is mid-turn.
    pub fn any_turn_running(&self) -> bool {
        self.turn_running.load(Ordering::SeqCst) || self.subagent_turns.load(Ordering::SeqCst) > 0
    }

    pub fn is_healthy(&self) -> bool {
        self.pi_ready.load(Ordering::SeqCst)
    }
}

/// Short label for a `step` status event: the command head for `bash`, the path for file tools,
/// otherwise the tool name and its first string argument. One line, ≤ 72 chars.
pub fn step_label(tool_name: &str, args: &Value) -> String {
    let first_str = |keys: &[&str]| -> Option<String> {
        keys.iter()
            .find_map(|k| args.get(k).and_then(Value::as_str))
            .or_else(|| {
                args.as_object()
                    .and_then(|o| o.values().find_map(Value::as_str))
            })
            .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
    };
    let body = match tool_name {
        "bash" | "shell" | "execute" => first_str(&["command", "cmd"]),
        "read" | "write" | "edit" | "read_file" | "write_file" | "edit_file" | "glob" | "grep" => {
            first_str(&["path", "file_path", "pattern"])
        }
        _ => first_str(&["query", "url", "name", "code"]),
    }
    .unwrap_or_default();
    let label = if body.is_empty() {
        tool_name.to_owned()
    } else {
        format!("{tool_name} {body}")
    };
    let mut out: String = label.chars().take(72).collect();
    if out.chars().count() < label.chars().count() {
        out.push('…');
    }
    out
}

pub fn route_name(r: &ValidRoute) -> &'static str {
    match r {
        ValidRoute::Reply { .. } => "reply",
        ValidRoute::Act { .. } => "act",
        ValidRoute::Wake { .. } => "wake",
        ValidRoute::New { .. } => "new",
    }
}

/// Main-turn wall-clock deadline in seconds. `ARBOS_TURN_DEADLINE_SECS` (default 900).
fn turn_deadline_secs() -> u64 {
    std::env::var("ARBOS_TURN_DEADLINE_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(TURN_DEADLINE_SECS)
}

/// Map a provider / tool error to a short chat announcement and whether to escalate to a subagent.
/// Returns `None` when the error is not a known hard limit (ordinary tool failures stay quiet).
fn classify_hard_limit(err: &str) -> Option<(&'static str, bool)> {
    let lower = err.to_ascii_lowercase();
    if lower.contains("spend_cap")
        || lower.contains("credits_exhausted")
        || lower.contains("credits are used up")
        || lower.contains("daily spend cap")
        || lower.contains("insufficient credits")
        || (lower.contains("spend") && lower.contains("cap"))
    {
        return Some((
            "I hit the spend limit for today, so I have to stop here.",
            false,
        ));
    }
    if lower.contains("context_length")
        || lower.contains("context length")
        || lower.contains("context window")
        || lower.contains("maximum context")
        || lower.contains("prompt is too long")
        || lower.contains("too many tokens")
        || lower.contains("token limit")
        || (lower.contains("max_tokens")
            && (lower.contains("exceed") || lower.contains("limit") || lower.contains("over")))
    {
        return Some((
            "I hit the model's context limit; I'm summarizing and continuing in a fresh context.",
            true,
        ));
    }
    if lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("deadline exceeded")
        || lower.contains("time-out")
    {
        return Some((
            "A tool or model call timed out; I'll try another approach.",
            false,
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Routing rule: the inline `reply` streams out of partial JSON exactly as written.
    #[test]
    fn reply_extractor_streams_partial_json() {
        let mut ex = ReplyExtractor::default();
        let mut out = String::new();
        for d in [
            "{\"action\": \"reply",
            "\", \"reply\": \"Hi ",
            "there\\n\\\"you\\\"",
            " — ok\"}",
        ] {
            out.push_str(&ex.feed(d));
        }
        assert_eq!(out, "Hi there\n\"you\" — ok");
        let mut ex2 = ReplyExtractor::default();
        assert_eq!(
            ex2.feed("{\"action\":\"act\",\"plan\":\"x\",\"reply\":null}"),
            ""
        );
    }

    /// Clone-style routing thoughts in `ack` leave only the user-facing sentence.
    #[test]
    fn strip_thought_prefix_keeps_clone_ack() {
        let raw = "The user wants me to clone a GitHub repo. That's real work — cloning can take a bit and might exceed a quick act if the repo is large. The instructions say prefer act over new only when it fits 8 calls and ~60s, no build. A git clone of one repo is typically one command. I'll act and clone it into the project folder.\n\nI should not use /work itself. Clone into the project folder, e.g. subtensor/.\n\nAck first via route.Cloning RaoFoundation/subtensor into this project now.";
        let cleaned = ProseFilter::strip_thought_prefix(raw);
        assert_eq!(
            cleaned.trim(),
            "Cloning RaoFoundation/subtensor into this project now."
        );
    }

    /// Status prose with a glued tool-narration tail drops the narration and keeps spacing.
    #[test]
    fn clean_prose_strips_glued_checking_tail() {
        let raw = "I have good data on the research loop from the retrieved memory. The baseline is done at val_loss 6.2568 with a 124M model, and the accum sweep (bs4/bs2/bs1) is running—no improvement yet. I'll report status from what I already have.Checking the latest experiment results.";
        let cleaned = ProseFilter::clean_prose(raw);
        assert_eq!(
            cleaned.trim(),
            "I have good data on the research loop from the retrieved memory. The baseline is done at val_loss 6.2568 with a 124M model, and the accum sweep (bs4/bs2/bs1) is running—no improvement yet. I'll report status from what I already have."
        );
        assert!(!cleaned.contains("Checking"));
        assert!(!cleaned.contains("have.Checking"));
    }

    /// A lone glued answer after thought-aloud still becomes just the answer.
    #[test]
    fn clean_prose_keeps_glued_austin_answer() {
        let raw = "The user is asking a simple factual question. I should reply directly.Austin.";
        assert_eq!(ProseFilter::clean_prose(raw).trim(), "Austin.");
    }

    /// Two real sentences glued without a space get a space, not a drop.
    #[test]
    fn clean_prose_fixes_glued_real_sentences() {
        let raw = "Baseline is done.Accum sweep is still running.";
        assert_eq!(
            ProseFilter::clean_prose(raw).trim(),
            "Baseline is done. Accum sweep is still running."
        );
    }

    /// Streaming: after real prose has started, a glued Checking-tail is held and dropped.
    #[test]
    fn feed_holds_then_drops_glued_checking_tail() {
        let mut f = ProseFilter::default();
        let first = f.feed("I'll report status from what I already have.\n");
        assert!(
            first.contains("already have."),
            "prose should stream: {first:?}"
        );
        let mid = f.feed("Checking the latest experiment results");
        assert!(
            !mid.contains("Checking"),
            "incomplete narration must not land: {mid:?}"
        );
        let end = f.feed(".");
        assert!(
            !end.contains("Checking"),
            "finished narration must stay out: {end:?}"
        );
        let flushed = f.flush();
        assert!(
            !flushed.contains("Checking"),
            "flush must not revive narration: {flushed:?}"
        );
    }
}
