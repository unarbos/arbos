//! Visible items → provider messages. What the model sees.
//!
//! One model step is persisted as an `Assistant` text event followed by one
//! `Tool` event per call (each carries its own args). On the wire that is one
//! assistant message holding every `tool_calls` entry, then one `tool` message
//! per call. Grouping them here keeps the shape providers expect and keeps the
//! cached prefix stable across steps.
//!
//! A `Compaction` item renders as one user message at the position of the
//! first line it replaced. A folded tool result renders as one cite line.
//! Both are decided by events on the transcript, so the same log always
//! projects to the same messages.
//!
//! Images: a `User` attachment or a `ToolRec.images` entry is a file path.
//! The projection loads it and sends a real `image_url` part. Chat
//! Completions rejects image parts on the `tool` role, so images from a tool
//! step ride in one `user` message right after that step's tool messages.
//! Only the newest [`KEEP_IMAGES`] images are sent as pixels; older ones
//! become a text stub so a long session does not pay for every screenshot
//! on every call.

use arbos_core::{Agent, Event, EventKind, Place, ToolRec};
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::compact::Item;
use crate::evict::{estimate_tokens, evict_tool_body};
use crate::image::{self, IMAGE_TOKENS, KEEP_IMAGES, is_image_path};
use crate::prompt::{CONTRACT, instance_prompt};
use crate::provider::{ChatMessage, ImagePart, ToolCall};

/// First line of the user message that stands in for compacted turns.
pub const COMPACTION_HEADER: &str = "[context checkpoint — earlier turns summarised]";
/// First line of the user message that carries a tool step's images.
const IMAGES_HEADER: &str = "[images from tool results]";
/// Prefix on a user line that came in as speech (`channel = "voice"`):
/// the words are a transcript, not typing.
pub const VOICE_MARK: &str = "[spoken — transcribed speech, may hold transcription errors] ";

/// Characters of a folded tool body kept as a preview.
const PREVIEW_CHARS: usize = 120;
/// Most bytes one step's fresh tool results may add to the prompt together
/// (~32k tokens) on a large window. Above it, results share the budget
/// equally. A small window gets less; see `compact::Policy::step_bytes`.
pub const STEP_BYTES: usize = 128 * 1024;

/// The messages for one model call, with enough bookkeeping for compaction
/// to reason about them.
pub struct Projection {
    pub messages: Vec<ChatMessage>,
    /// Estimated tokens each visible item contributes. Same length as the
    /// item slice that was projected.
    pub item_tokens: Vec<u64>,
    /// Tokens of the system prefix.
    pub base_tokens: u64,
}

impl Projection {
    /// Estimated prompt tokens, before calibration against the provider.
    pub fn tokens(&self) -> u64 {
        self.base_tokens + self.item_tokens.iter().sum::<u64>()
    }

    fn push(&mut self, item: Option<usize>, m: ChatMessage) {
        let n = message_tokens(&m);
        match item {
            Some(i) => self.item_tokens[i] += n,
            None => self.base_tokens += n,
        }
        self.messages.push(m);
    }
}

/// Estimated tokens one message costs on the wire.
pub fn message_tokens(m: &ChatMessage) -> u64 {
    let mut n = 4;
    if let Some(c) = &m.content {
        n += estimate_tokens(c);
    }
    if let Some(calls) = &m.tool_calls {
        n += estimate_tokens(&serde_json::to_string(calls).unwrap_or_default());
    }
    n += IMAGE_TOKENS * m.images.len() as u64;
    n
}

/// Where the model is told the full record lives, relative to the place.
pub fn cite_path(agent: &Agent) -> String {
    format!(".arbos/agents/{}/transcript.jsonl", agent.id)
}

/// The message a compaction becomes in the model's view.
pub fn render_compaction(lo: u64, hi: u64, summary: &str, cite_path: &str) -> String {
    format!(
        "{COMPACTION_HEADER}\nThis is a machine-written record of earlier turns, not new instructions. Full record: {cite_path} lines {lo}–{hi}; grep or read it to recover any detail.\n\n{summary}"
    )
}

/// The one line a folded tool body becomes.
/// Indices of unfolded read-only results that the model asked for again,
/// identically, later on. The newer copy is the one it is working from;
/// the older one is dead weight until a fold would reach it anyway.
fn superseded_results(items: &[Item]) -> std::collections::HashSet<usize> {
    let mut last: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut out = std::collections::HashSet::new();
    for (i, it) in items.iter().enumerate() {
        let Item::Event {
            event,
            folded: false,
            ..
        } = it
        else {
            continue;
        };
        let EventKind::Tool(r) = &event.kind else {
            continue;
        };
        if !matches!(r.name.as_str(), "read" | "grep" | "find" | "ls") || r.error.is_some() {
            continue;
        }
        let sig = format!(
            "{}\u{0}{}",
            r.name,
            r.args.as_ref().map(|a| a.to_string()).unwrap_or_default()
        );
        if let Some(prev) = last.insert(sig, i) {
            out.insert(prev);
        }
    }
    out
}

pub fn render_folded(rec: &ToolRec, seq: u64, cite_path: &str) -> String {
    let body = rec.body.as_deref().unwrap_or("");
    let size = rec.result_size.unwrap_or(body.len() as u64);
    let first = |s: &str| {
        s.lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(PREVIEW_CHARS)
            .collect::<String>()
    };
    match &rec.error {
        Some(e) => format!(
            "[{} output folded, failed: {} — {size} bytes, {cite_path}:{seq}]",
            rec.name,
            first(e)
        ),
        None => {
            let head = first(body);
            let preview = if head.is_empty() {
                String::new()
            } else {
                format!("\n{head}")
            };
            format!(
                "[{} output folded — {size} bytes, {cite_path}:{seq}]{preview}",
                rec.name
            )
        }
    }
}

/// The text a non-model event contributes as a user-role line, or None
/// for events the model never sees (wake, ask, approval).
pub fn user_line(e: &Event) -> Option<String> {
    match &e.kind {
        EventKind::User { text, .. } => Some(text.clone()),
        // A user wake is followed by its User event; the others carry the
        // kernel's summons in the wake line itself.
        EventKind::Wake {
            wake,
            text: Some(text),
        } if wake != "user" && !text.trim().is_empty() => Some(format!("[kernel {wake}] {text}")),
        EventKind::Say { from, text } => Some(format!("[{from}] {text}")),
        EventKind::Answer { text } => Some(format!("[user answer] {text}")),
        EventKind::Notice { text, .. } => Some(format!("[kernel] {text}")),
        EventKind::Nudge { text } => Some(format!("[kernel] {text}")),
        _ => None,
    }
}

/// Image paths one event contributes to the model's view.
fn image_paths(e: &Event) -> Vec<&str> {
    match &e.kind {
        EventKind::User { attachments, .. } => attachments
            .iter()
            .map(String::as_str)
            .filter(|p| is_image_path(Path::new(p)))
            .collect(),
        EventKind::Tool(rec) => rec.images.iter().map(String::as_str).collect(),
        _ => Vec::new(),
    }
}

/// Decides, per image in order of appearance, whether it goes out as pixels
/// or as a stub. The last `KEEP_IMAGES` win.
struct ImageBudget {
    seen: usize,
    total: usize,
    cwd: PathBuf,
    /// Images the selected model could not see, in words (`ImageDescribed`
    /// lines): path → (model, description). Shown as text, not pixels.
    described: HashMap<String, (String, String)>,
}

impl ImageBudget {
    fn new(items: &[Item], cwd: PathBuf) -> Self {
        let mut described = HashMap::new();
        for it in items {
            if let Item::Event { event, .. } = it
                && let EventKind::ImageDescribed { path, model, text } = &event.kind
            {
                described.insert(path.clone(), (model.clone(), text.clone()));
            }
        }
        let total = items
            .iter()
            .filter_map(|it| match it {
                Item::Event {
                    event,
                    folded: false,
                    ..
                } => Some(
                    image_paths(event)
                        .iter()
                        .filter(|p| !described.contains_key(**p))
                        .count(),
                ),
                _ => None,
            })
            .sum();
        Self {
            seen: 0,
            total,
            cwd,
            described,
        }
    }

    /// Returns the part to attach, or `None` with a stub line to print.
    fn take(&mut self, path: &str) -> (Option<ImagePart>, String) {
        if let Some((model, text)) = self.described.get(path) {
            return (
                None,
                format!("[image {path} — described by {model}]\n{text}"),
            );
        }
        self.seen += 1;
        let keep = self.total <= KEEP_IMAGES || self.seen > self.total - KEEP_IMAGES;
        if !keep {
            return (
                None,
                format!("[image {path}: evicted — read it again to see it]"),
            );
        }
        let file = crate::tools::resolve(&self.cwd, path);
        match image::load(&file) {
            Ok(part) => {
                let mut part: ImagePart = part.into();
                part.path = path.to_string();
                (Some(part), format!("[image {path}]"))
            }
            Err(e) => (None, format!("[image {path}: not shown — {e}]")),
        }
    }
}

/// The system prompt's parts and their token estimates (chars/4), plus the
/// tool schemas: what every model call carries before the conversation.
/// For `arbos-kernel prompt` and the per-turn `prompt_size` log line.
pub fn measure(
    place: &Place,
    agent: &Agent,
    skills: &[String],
    view: &crate::tool::View,
) -> Vec<(String, u64)> {
    use crate::evict::estimate_tokens;
    sections(place, agent, skills, view)
        .into_iter()
        .map(|(k, text)| (k, estimate_tokens(&text)))
        .collect()
}

/// The same parts as text: the contract, the project context, the instance
/// prompt (whole, then per block), the plan segment, the tool schemas
/// (whole, then per tool). Nested rows start with two spaces.
pub fn sections(
    place: &Place,
    agent: &Agent,
    skills: &[String],
    view: &crate::tool::View,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    out.push(("contract".to_string(), CONTRACT.to_string()));
    if let Some(context) = arbos_core::store::prompt_segment(place) {
        out.push(("project-context".to_string(), context));
    }
    let inst = instance_prompt(place, agent, skills);
    for block in inst.split("\n\n").filter(|b| !b.trim().is_empty()) {
        let head: String = block
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(48)
            .collect();
        out.push((format!("  instance › {head}"), block.to_string()));
    }
    out.push(("instance".to_string(), inst));
    if let Some(seg) = crate::prompt::plan_segment(place, agent) {
        out.push(("plan".to_string(), seg));
    }
    for schema in view.schemas() {
        let name = schema
            .pointer("/function/name")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        out.push((format!("  tool › {name}"), schema.to_string()));
    }
    let schemas = serde_json::to_string(view.schemas()).unwrap_or_default();
    out.push((format!("tools ({})", view.schemas().len()), schemas));
    out
}

/// `step_bytes`: most bytes one step's fresh tool results may take together.
pub fn project(
    place: &Place,
    agent: &Agent,
    items: &[Item],
    skills: &[String],
    step_bytes: usize,
) -> Projection {
    let mut out = Projection {
        messages: Vec::new(),
        item_tokens: vec![0; items.len()],
        base_tokens: 0,
    };
    out.push(None, system(CONTRACT.to_string()));
    // The project context comes first, before anything about this agent:
    // the standing brief every agent reads (docs/project-context.md,
    // root-owned).
    if let Some(context) = arbos_core::store::prompt_segment(place) {
        out.push(None, system(context));
    }
    out.push(None, system(instance_prompt(place, agent, skills)));
    if let Some(seg) = crate::prompt::plan_segment(place, agent) {
        out.push(None, system(seg));
    }

    let cwd = agent.work_dir(&place.path);
    let mut budget = ImageBudget::new(items, cwd.clone());
    let cite = cite_path(agent);
    let superseded = superseded_results(items);
    // Assistant text waits for the tool calls of its step, if any.
    let mut pending: Option<(usize, String, Option<serde_json::Value>)> = None;
    let mut i = 0;
    while i < items.len() {
        match &items[i] {
            Item::Compaction { lo, hi, summary } => {
                flush(&mut out, &mut pending);
                out.push(Some(i), user(render_compaction(*lo, *hi, summary, &cite)));
                i += 1;
            }
            Item::Event { event, .. } => match &event.kind {
                EventKind::Assistant {
                    text,
                    reasoning_details,
                } => {
                    flush(&mut out, &mut pending);
                    pending = Some((i, text.clone(), reasoning_details.clone()));
                    i += 1;
                }
                EventKind::Tool(_) => {
                    i = tool_step(
                        &mut out,
                        items,
                        i,
                        pending.take(),
                        &mut budget,
                        &cite,
                        &superseded,
                        step_bytes,
                    );
                }
                EventKind::User {
                    text,
                    attachments,
                    channel,
                    ..
                } => {
                    flush(&mut out, &mut pending);
                    let mut t = text.clone();
                    // Spoken, not typed (dictation or a call): the model is
                    // told, or "can you hear me" reads as a question about
                    // its ears and gets "I have no audio capabilities".
                    if channel == "voice" {
                        t = format!("{VOICE_MARK}{t}");
                    }
                    // `/name args` names a skill: the transcript keeps what
                    // was typed; the model reads the skill's body under it.
                    if text.trim_start().starts_with('/') {
                        if let Some((skill, args)) = arbos_core::slash_skill(place, text) {
                            t.push_str(&format!(
                                "\n\n[skill {} — {}]\n{}",
                                skill.name,
                                skill.path.display(),
                                skill.render(&args)
                            ));
                        }
                    }
                    let mut shown = Vec::new();
                    if !attachments.is_empty() {
                        t.push_str("\nattachments:");
                        for a in attachments {
                            t.push_str("\n  ");
                            if is_image_path(Path::new(a)) {
                                let (part, line) = budget.take(a);
                                t.push_str(&line);
                                shown.extend(part);
                            } else if crate::pdf::is_pdf_path(Path::new(a)) {
                                // A PDF arrives as its text, capped; the
                                // model reads the file for the rest.
                                t.push_str(&crate::pdf::attachment_text(&resolve_attachment(
                                    &cwd, a,
                                )));
                            } else {
                                t.push_str(a);
                            }
                        }
                    }
                    let mut m = user(t);
                    m.images = shown;
                    out.push(Some(i), m);
                    i += 1;
                }
                _ => {
                    flush(&mut out, &mut pending);
                    if let Some(line) = user_line(event) {
                        out.push(Some(i), user(line));
                    }
                    i += 1;
                }
            },
        }
    }
    flush(&mut out, &mut pending);
    out
}

fn flush(out: &mut Projection, pending: &mut Option<(usize, String, Option<serde_json::Value>)>) {
    if let Some((at, t, rd)) = pending.take() {
        // A bare step boundary (empty text, no tool results after it — an
        // interrupted step) is nothing to send; providers reject empty
        // assistant messages.
        if !t.trim().is_empty() {
            out.push(Some(at), assistant(Some(t), None, rd));
        }
    }
}

/// One model step's tool results, starting at `first`: the assistant
/// message that holds every call, one tool message per result, then the
/// step's images as one user message. Returns the index after the step.
fn tool_step(
    out: &mut Projection,
    items: &[Item],
    first: usize,
    pending: Option<(usize, String, Option<serde_json::Value>)>,
    budget: &mut ImageBudget,
    cite: &str,
    superseded: &std::collections::HashSet<usize>,
    step_bytes: usize,
) -> usize {
    let mut end = first;
    while let Some(Item::Event { event, .. }) = items.get(end) {
        if !matches!(event.kind, EventKind::Tool(_)) {
            break;
        }
        end += 1;
    }
    let rec_at = |i: usize| -> (u64, &ToolRec, bool) {
        let Item::Event { seq, event, folded } = &items[i] else {
            unreachable!()
        };
        let EventKind::Tool(rec) = &event.kind else {
            unreachable!()
        };
        (*seq, rec, *folded)
    };

    let calls = (first..end)
        .map(|i| {
            let (_, r, _) = rec_at(i);
            ToolCall {
                id: r.call_id.clone(),
                name: r.name.clone(),
                arguments: r.args.clone().unwrap_or(json!({})),
            }
        })
        .collect();
    let (at, text, rd) = match pending {
        Some((at, t, rd)) => (at, Some(t).filter(|t| !t.trim().is_empty()), rd),
        None => (first, None, None),
    };
    out.push(Some(at), assistant(text, Some(calls), rd));

    let mut shown: Vec<ImagePart> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    // Eight parallel reads at the per-read cap would be ~100k tokens in one
    // step. When a step's fresh results together exceed `step_bytes`, each
    // gets an equal share instead, and the cite says how to see the rest.
    // On a small window the share is what keeps one big read inside the
    // model's context at all.
    let fresh = (first..end)
        .filter(|&i| !rec_at(i).2 && !superseded.contains(&i))
        .count()
        .max(1);
    // No result shrinks below the floor, so many parallel reads may run a
    // little over the budget rather than each show nothing useful. The
    // floor is EVICT_BYTES on a full budget and scales down with it.
    let floor = crate::evict::EVICT_BYTES * step_bytes / STEP_BYTES;
    let per_result = (step_bytes / fresh).max(floor);
    let total_fresh: usize = (first..end)
        .filter(|&i| !rec_at(i).2 && !superseded.contains(&i))
        .map(|i| {
            rec_at(i)
                .1
                .body
                .as_deref()
                .map_or(0, str::len)
                .min(crate::evict::READ_BYTES)
        })
        .sum();
    let squeeze = total_fresh > step_bytes;
    for i in first..end {
        let (seq, r, folded) = rec_at(i);
        let body = if folded {
            render_folded(r, seq, cite)
        } else if superseded.contains(&i) {
            format!(
                "[{} result superseded: the same call was made again below; its newer result is the one to read. {cite}:{seq}]",
                r.name
            )
        } else if squeeze {
            let full = r.body.as_deref().unwrap_or("");
            let cite = format!("{cite}:{seq}");
            match crate::evict::keep_for(&r.name) {
                crate::evict::Keep::Head => {
                    crate::evict::evict_head_to(full, &cite, per_result, crate::evict::READ_LINES)
                }
                crate::evict::Keep::Tail => {
                    crate::evict::evict_tail_to(full, &cite, per_result, crate::evict::EVICT_LINES)
                }
            }
        } else {
            evict_tool_body(
                &r.name,
                r.body.as_deref().unwrap_or(""),
                &format!("{cite}:{seq}"),
            )
        };
        let mut m = ChatMessage::plain("tool", Some(body));
        m.tool_call_id = Some(r.call_id.clone());
        m.name = Some(r.name.clone());
        out.push(Some(i), m);
        if folded {
            continue;
        }
        for path in &r.images {
            let (part, line) = budget.take(path);
            lines.push(format!("{}: {line}", r.name));
            shown.extend(part);
        }
    }
    if !lines.is_empty() {
        let mut m = user(format!("{IMAGES_HEADER}\n{}", lines.join("\n")));
        m.images = shown;
        out.push(Some(end - 1), m);
    }
    end
}

fn system(content: String) -> ChatMessage {
    ChatMessage::plain("system", Some(content))
}

fn user(content: String) -> ChatMessage {
    ChatMessage::plain("user", Some(content))
}

fn assistant(
    content: Option<String>,
    tool_calls: Option<Vec<ToolCall>>,
    reasoning_details: Option<serde_json::Value>,
) -> ChatMessage {
    let mut m = ChatMessage::plain("assistant", content);
    m.tool_calls = tool_calls;
    m.reasoning_details = reasoning_details;
    m
}

/// An attachment path as written (absolute from the desktop, or relative
/// to the agent's cwd from the CLI).
fn resolve_attachment(cwd: &Path, a: &str) -> PathBuf {
    let p = Path::new(a);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

#[cfg(test)]
mod voice_mark_tests {
    use super::*;

    #[test]
    fn a_spoken_line_is_marked_for_the_model_and_a_typed_one_is_not() {
        let dir = std::env::temp_dir().join(format!(
            "arbos-voice-mark-{}-{}",
            std::process::id(),
            arbos_core::now_ms()
        ));
        std::fs::create_dir_all(dir.join(".arbos/agents/root")).unwrap();
        let place = Place::new(&dir);
        arbos_core::bootstrap(&place).unwrap();
        let agent = arbos_core::Agent::root("root");
        let user = |text: &str, channel: &str| {
            Event::new(EventKind::User {
                text: text.into(),
                attachments: vec![],
                channel: channel.into(),
                device: String::new(),
            })
        };
        let mut events = vec![
            user("Hello, can you hear", "voice"),
            user("now typed", "text"),
        ];
        for (i, e) in events.iter_mut().enumerate() {
            e.seq = i as u64 + 1;
        }
        let items = crate::compact::visible(&events);
        let p = project(&place, &agent, &items, &[], STEP_BYTES);
        let users: Vec<String> = p
            .messages
            .iter()
            .filter(|m| m.role == "user")
            .filter_map(|m| m.content.clone())
            .collect();
        assert_eq!(users.len(), 2, "{users:?}");
        assert!(users[0].starts_with(VOICE_MARK), "{}", users[0]);
        assert!(users[0].ends_with("Hello, can you hear"), "{}", users[0]);
        assert!(!users[1].contains("[spoken"), "{}", users[1]);
        // The contract tells the model what the mark means.
        assert!(CONTRACT.contains("Voice: a user line marked [spoken"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
