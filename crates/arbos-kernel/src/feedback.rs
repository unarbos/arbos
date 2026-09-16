//! The material for an in-app feedback report: what the desktop hands over
//! when the user says "this went wrong". Built here so every client sends
//! the same thing, and so nothing leaves the machine that should not:
//! credentials are redacted, tool bodies and fat arguments are cut to a
//! glance, images and attachments stay as paths, and the whole is bounded.
//!
//! The unit is an *exchange*: everything that happened because the user
//! asked one thing — from a `user` or `kickoff` wake to the next such wake.
//! The `done`, `job`, `serve`, `plan` and `say` wakes inside it stay
//! inside: spawn-first is the coordinator's normal shape, and a boundary on
//! every wake put the user's words in one half and the answer in the other
//! (desktop feedback owner's review, 2026-09-16).

use arbos_core::{Event, EventKind, Place, ToolRec, load_transcript};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

/// The payload cap: one gzipped POST, sent by hand, once.
pub const MAX_BYTES: usize = 1024 * 1024;
/// Log lines around the exchange: before its start, and after its end —
/// a crash, a respawn or a dropped link shows up after the turn ends,
/// which is often the moment the user notices and clicks.
const LOG_BEFORE_MS: i64 = 5_000;
const LOG_AFTER_MS: i64 = 60_000;
const LOG_MAX_LINES: usize = 400;
/// The log never goes to zero while transcript lines remain; and its own
/// newest lines ride along whatever the span, as "what the kernel is
/// doing now".
const LOG_FLOOR: usize = 40;
/// A call that went wrong, or the span's last call: this much body,
/// weighted to the tail, where a failing run announces itself.
const DIAGNOSTIC_BYTES: usize = 8 * 1024;
/// A bash command is what an agent reads first and is almost never long.
const COMMAND_BYTES: usize = 4 * 1024;
/// The most transcript lines `tail` will carry.
pub const TAIL_MAX: u32 = 500;

/// Argument keys that carry a whole file or patch: glanced, with the real
/// length recorded under `args_clipped`.
const FAT_ARGS: &[&str] = &[
    "contents",
    "content",
    "new_string",
    "old_string",
    "patch",
    "text",
    "markdown",
    "brief",
    "body",
    "task",
    "data",
];

/// What the client asked for.
pub struct Request<'a> {
    pub agent: &'a str,
    /// A transcript line of the exchange the user is looking at.
    pub seq: Option<u64>,
    /// The tool line the user clicked: its exchange is the anchor and its
    /// body is carried whole (up to the cap).
    pub call_id: Option<&'a str>,
    /// The last N lines of the agent's transcript, whatever exchange they
    /// fall in, beside the anchor: the wake lines in them show the turn
    /// structure, so one primitive serves "it keeps doing this" and
    /// reproduction alike. Capped at `TAIL_MAX`.
    pub tail: u32,
    pub note: &'a str,
}

pub struct Bundle {
    pub turn: Value,
    pub events: Vec<Value>,
    pub tail: Vec<Value>,
    pub children: Vec<Value>,
    pub log: Vec<Value>,
    pub kernel: Value,
    pub note: String,
    pub redacted: Value,
    pub truncated: bool,
    pub bytes: u64,
}

#[derive(Default)]
struct Counts {
    secrets: usize,
    tokens: usize,
    values: usize,
    blocks: usize,
}

/// A wake that opens an exchange: the user's words, or the kickoff.
fn is_boundary(e: &Event) -> bool {
    matches!(&e.kind, EventKind::Wake { wake, .. } if wake == "user" || wake == "kickoff")
}

/// Exchange starts, as indexes into `events`.
fn boundaries(events: &[Event]) -> Vec<usize> {
    events
        .iter()
        .enumerate()
        .filter(|(_, e)| is_boundary(e))
        .map(|(i, _)| i)
        .collect()
}

/// The exchange holding `seq` (or the tool call `call_id`), or the last
/// one the user opened: `[start, end)` over `events`.
fn anchor_span(
    events: &[Event],
    seq: Option<u64>,
    call_id: Option<&str>,
) -> Option<(usize, usize)> {
    let starts = boundaries(events);
    let at = match (call_id, seq) {
        (Some(id), _) => events
            .iter()
            .position(|e| matches!(&e.kind, EventKind::Tool(rec) if rec.call_id == id)),
        (None, Some(s)) => events.iter().position(|e| e.seq >= s),
        (None, None) => None,
    };
    let start = match at {
        Some(at) => *starts.iter().rev().find(|&&w| w <= at).or(starts.first())?,
        None if seq.is_some() || call_id.is_some() => return None,
        None => *starts.last()?,
    };
    Some((start, span_end(events, &starts, start)))
}

fn span_end(events: &[Event], starts: &[usize], start: usize) -> usize {
    starts
        .iter()
        .find(|&&w| w > start)
        .copied()
        .unwrap_or(events.len())
}

/// Head and tail of `text` within `budget` bytes, most of it the tail.
fn tail_weighted(text: &str, budget: usize) -> (String, bool) {
    if text.len() <= budget {
        return (text.to_string(), false);
    }
    let head_n = budget / 4;
    let tail_n = budget - head_n - 32;
    let head = clip_at_boundary(&text[..text.len().min(head_n)], false);
    let tail_start = text.len() - tail_n;
    let tail_start = (tail_start..text.len())
        .find(|&i| text.is_char_boundary(i))
        .unwrap_or(text.len());
    let tail = clip_at_boundary(&text[tail_start..], true);
    (
        format!(
            "{}\n… [{} bytes cut] …\n{}",
            head.trim_end(),
            text.len() - head.len() - tail.len(),
            tail.trim_start()
        ),
        true,
    )
}

/// A slice ending (or starting) on a whole line where one is near.
fn clip_at_boundary(s: &str, from_start: bool) -> &str {
    if from_start {
        match s.find('\n') {
            Some(i) if i < s.len() / 4 => &s[i + 1..],
            _ => s,
        }
    } else {
        match s.rfind('\n') {
            Some(i) if i > s.len() * 3 / 4 => &s[..i],
            _ => s,
        }
    }
}

/// How much of a tool's body the report carries.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Budget {
    /// The glance (`ToolRec::digest`): proves it ran.
    Glance,
    /// The error uncut plus a tail-weighted `DIAGNOSTIC_BYTES` of body: a
    /// call that failed, or the span's last call, which may have ended
    /// the turn wrong without setting `error`.
    Diagnostic,
    /// The call the user pointed at: whole, up to the cap.
    Whole,
}

/// One transcript line as the report carries it.
fn slim(ev: &Event, budget: Budget) -> Value {
    let mut v = serde_json::to_value(ev).unwrap_or(Value::Null);
    let Some(obj) = v.as_object_mut() else {
        return v;
    };
    obj.remove("reasoning_details");
    if let EventKind::Tool(rec) = &ev.kind {
        obj.remove("diff");
        let body = rec.body.as_deref().unwrap_or("");
        let out = match budget {
            Budget::Glance => (
                rec.digest().unwrap_or_default(),
                rec.body
                    .as_ref()
                    .is_some_and(|b| b.len() > body_glance_len(rec)),
            ),
            Budget::Diagnostic => tail_weighted(body, DIAGNOSTIC_BYTES),
            Budget::Whole => {
                if body.len() <= MAX_BYTES / 2 {
                    (body.to_string(), false)
                } else {
                    tail_weighted(body, MAX_BYTES / 2)
                }
            }
        };
        obj.remove("body");
        obj.insert("output".into(), Value::String(out.0));
        if out.1 {
            obj.insert("output_clipped".into(), Value::Bool(true));
        }
        // Arguments: paths, patterns and commands whole (commands capped);
        // file contents and patches glanced, with what was cut recorded.
        if let Some(Value::Object(args)) = obj.get_mut("args") {
            let mut clipped = Map::new();
            for (k, val) in args.iter_mut() {
                let Value::String(s) = val else { continue };
                let key = k.as_str();
                if key == "command" {
                    if s.len() > COMMAND_BYTES {
                        clipped.insert(key.into(), json!(s.len()));
                        let (t, _) = tail_weighted(s, COMMAND_BYTES);
                        *s = t;
                    }
                } else if FAT_ARGS.contains(&key) && s.chars().count() > arbos_core::DIGEST_CHARS {
                    clipped.insert(key.into(), json!(s.len()));
                    *s = arbos_core::tool_digest(s);
                }
            }
            if !clipped.is_empty() {
                obj.insert("args_clipped".into(), Value::Object(clipped));
            }
        }
    }
    v
}

fn body_glance_len(rec: &ToolRec) -> usize {
    rec.digest().map(|d| d.len()).unwrap_or(0)
}

/// Every string leaf of `v`, redacted in place: the kernel's own key and
/// granted secrets by value, then credential shapes.
fn redact_leaves(v: &mut Value, counts: &mut Counts) {
    match v {
        Value::String(s) => {
            let secrets = arbos_engine::secrets::store();
            if secrets.has_any() {
                let out = secrets.redact(s);
                if &out != s {
                    counts.secrets += 1;
                    *s = out;
                }
            }
            let (out, n) = arbos_core::redact::redact(s);
            counts.tokens += n.tokens;
            counts.values += n.values;
            counts.blocks += n.blocks;
            if n.total() > 0 {
                *s = out;
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|i| redact_leaves(i, counts)),
        Value::Object(map) => map.values_mut().for_each(|i| redact_leaves(i, counts)),
        _ => {}
    }
}

/// A child's transcript, live or archived: a finished worker's folder
/// moves under `archive/agents/` when its report is read, and the
/// complaint may come after.
fn child_transcript(place: &Place, child: &str) -> std::path::PathBuf {
    let live = arbos_core::Layout::new(place, child).transcript();
    if live.exists() {
        return live;
    }
    place
        .archive_dir()
        .join("agents")
        .join(child)
        .join("transcript.jsonl")
}

/// `kernel.log` lines for the span, this agent's, its children's, and the
/// kernel's own; plus the log's newest `LOG_FLOOR` lines whatever the
/// span. Oldest first, at most `LOG_MAX_LINES`.
fn log_lines(place: &Place, from_ms: i64, to_ms: i64, agents: &BTreeSet<String>) -> Vec<Value> {
    let path = crate::klog::log_path_for(&place.arbos());
    let Ok(text) = std::fs::read_to_string(&path) else {
        return vec![];
    };
    let all: Vec<Value> = text
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect();
    let mine =
        |v: &Value| v["agent"].is_null() || v["agent"].as_str().is_some_and(|a| agents.contains(a));
    let mut picked: Vec<Value> = all
        .iter()
        .filter(|v| {
            let ts = v["ts"].as_i64().unwrap_or(0);
            ts >= from_ms - LOG_BEFORE_MS && ts <= to_ms + LOG_AFTER_MS && mine(v)
        })
        .cloned()
        .collect();
    let skip = picked.len().saturating_sub(LOG_MAX_LINES);
    picked.drain(..skip);
    let newest = &all[all.len().saturating_sub(LOG_FLOOR)..];
    for line in newest {
        if !picked.contains(line) {
            picked.push(line.clone());
        }
    }
    picked.sort_by_key(|v| v["ts"].as_i64().unwrap_or(0));
    picked
}

fn size_of(parts: &[&[Value]]) -> usize {
    parts
        .iter()
        .flat_map(|p| p.iter())
        .map(|v| v.to_string().len() + 1)
        .sum()
}

/// Build the report material.
pub fn bundle(place: &Place, req: &Request<'_>, host: &arbos_engine::Host) -> Bundle {
    let agent = req.agent;
    let events =
        load_transcript(&arbos_core::Layout::new(place, agent).transcript()).unwrap_or_default();
    let mut counts = Counts::default();
    let now = arbos_core::now_ms();

    let anchor = anchor_span(&events, req.seq, req.call_id);
    let span: &[Event] = match anchor {
        Some((s, e)) => &events[s..e],
        None => &[],
    };
    let started_ms = span.first().map(|e| e.ts).unwrap_or(0);
    let complete = span.iter().any(Event::is_turn_complete);
    let ended_ms = if complete {
        span.last().map(|e| e.ts).unwrap_or(started_ms)
    } else {
        now
    };

    // Budgets: the named call whole; failed calls and the span's last call
    // diagnostic; the rest a glance.
    let last_tool = span
        .iter()
        .rposition(|e| matches!(e.kind, EventKind::Tool(_)));
    let mut lines: Vec<Value> = span
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let budget = match &e.kind {
                EventKind::Tool(rec) if req.call_id == Some(rec.call_id.as_str()) => Budget::Whole,
                EventKind::Tool(rec) if rec.error.is_some() || Some(i) == last_tool => {
                    Budget::Diagnostic
                }
                _ => Budget::Glance,
            };
            slim(e, budget)
        })
        .collect();

    // The transcript's tail, beside the anchor: lines the anchor already
    // carries are left out; budgets as for the anchor (a failed call or
    // the tail's last call diagnostic, the rest a glance).
    let mut tail: Vec<Value> = if req.tail > 0 {
        let n = req.tail.min(TAIL_MAX) as usize;
        let skip = events.len().saturating_sub(n);
        let picked: Vec<&Event> = events
            .iter()
            .enumerate()
            .skip(skip)
            .filter(|(i, _)| !anchor.is_some_and(|(s, en)| (s..en).contains(i)))
            .map(|(_, e)| e)
            .collect();
        let last_tool = picked
            .iter()
            .rposition(|x| matches!(x.kind, EventKind::Tool(_)));
        picked
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let budget = match &e.kind {
                    EventKind::Tool(r) if r.error.is_some() || Some(i) == last_tool => {
                        Budget::Diagnostic
                    }
                    _ => Budget::Glance,
                };
                slim(e, budget)
            })
            .collect()
    } else {
        vec![]
    };

    // Children spawned or reporting in the span: their own lines since the
    // exchange began, slimmed and tagged. The complaint often lives there
    // ("creating a sub agent just says starting forever").
    let mut agents: BTreeSet<String> = BTreeSet::from([agent.to_string()]);
    let mut children: Vec<Value> = Vec::new();
    for e in span {
        let EventKind::Tool(rec) = &e.kind else {
            continue;
        };
        let Some(child) = rec.child.as_deref() else {
            continue;
        };
        if !agents.insert(child.to_string()) {
            continue;
        }
        let theirs = load_transcript(&child_transcript(place, child)).unwrap_or_default();
        let since: Vec<&Event> = theirs.iter().filter(|x| x.ts >= started_ms).collect();
        let last_tool = since
            .iter()
            .rposition(|x| matches!(x.kind, EventKind::Tool(_)));
        let their_lines: Vec<Value> = since
            .iter()
            .enumerate()
            .map(|(i, x)| {
                let budget = match &x.kind {
                    EventKind::Tool(r) if r.error.is_some() || Some(i) == last_tool => {
                        Budget::Diagnostic
                    }
                    _ => Budget::Glance,
                };
                slim(x, budget)
            })
            .collect();
        children.push(json!({
            "agent": child,
            "lines": their_lines.len(),
            "of": theirs.len(),
            "events": their_lines,
        }));
    }

    let mut log = log_lines(place, started_ms, ended_ms, &agents);

    for v in lines
        .iter_mut()
        .chain(tail.iter_mut())
        .chain(children.iter_mut())
        .chain(log.iter_mut())
    {
        redact_leaves(v, &mut counts);
    }
    let (note, n) = arbos_core::redact::redact(req.note);
    counts.tokens += n.tokens;
    counts.values += n.values;
    counts.blocks += n.blocks;

    // The cap, in the order that loses the least: the tail's oldest lines,
    // then child spans (oldest first), then log lines down to the floor,
    // then the anchor's middle — its wake, the user's line and its last
    // line always stay.
    let over = |lines: &[Value], tail: &[Value], children: &[Value], log: &[Value]| {
        size_of(&[lines, tail, children, log]) > MAX_BYTES
    };
    let mut truncated = false;
    while over(&lines, &tail, &children, &log) && !tail.is_empty() {
        tail.remove(0);
        truncated = true;
    }
    while over(&lines, &tail, &children, &log) && !children.is_empty() {
        children.remove(0);
        truncated = true;
    }
    while over(&lines, &tail, &children, &log) && log.len() > LOG_FLOOR {
        log.remove(0);
        truncated = true;
    }
    while over(&lines, &tail, &children, &log) && lines.len() > 3 {
        lines.remove(2);
        truncated = true;
    }

    let kernel = json!({
        "version": crate::klog::version(),
        "git_sha": crate::klog::git_sha(),
        "built_at": crate::klog::built_at(),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "provider": host.config.provider().as_str(),
        "model": host.config.model(),
        "project": arbos_core::project::load(place).name.unwrap_or_default(),
    });
    let turn = json!({
        "from": span.first().map(|e| e.seq).unwrap_or(0),
        "to": span.last().map(|e| e.seq).unwrap_or(0),
        "started_ms": started_ms,
        "ended_ms": ended_ms,
        "complete": complete,
        "lines": lines.len(),
        "of": span.len(),
        "call_id": req.call_id,
        "tail": req.tail.min(TAIL_MAX),
    });
    let redacted = json!({
        "secrets": counts.secrets,
        "tokens": counts.tokens,
        "values": counts.values,
        "blocks": counts.blocks,
    });
    // The whole frame's size, as the client shows it.
    let bytes = json!({
        "agent": agent, "turn": turn, "events": lines, "tail": tail, "children": children,
        "log": log, "kernel": kernel, "note": note, "redacted": redacted, "truncated": truncated,
    })
    .to_string()
    .len() as u64;
    Bundle {
        turn,
        events: lines,
        tail,
        children,
        log,
        kernel,
        note,
        redacted,
        truncated,
        bytes,
    }
}

/// One tool body whole, redacted, capped: the companion to a report that
/// carried its glance, for the agent fixing the bug to pull later.
pub fn tool_body(place: &Place, agent: &str, call_id: &str) -> Option<(String, bool, u64)> {
    let events = load_transcript(&child_transcript(place, agent)).ok()?;
    let rec = events.iter().find_map(|e| match &e.kind {
        EventKind::Tool(rec) if rec.call_id == call_id => Some(rec),
        _ => None,
    })?;
    let body = match (&rec.error, &rec.body) {
        (Some(err), Some(b)) => format!("{err}\n{b}"),
        (Some(err), None) => err.clone(),
        (None, Some(b)) => b.clone(),
        (None, None) => String::new(),
    };
    let secrets = arbos_engine::secrets::store();
    let body = if secrets.has_any() {
        secrets.redact(&body)
    } else {
        body
    };
    let (body, _) = arbos_core::redact::redact(&body);
    let size = body.len() as u64;
    let (body, cut) = tail_weighted(&body, MAX_BYTES);
    Some((body, cut, size))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wake(seq: u64, ts: i64, kind: &str) -> Event {
        let mut e = Event::new(EventKind::Wake {
            wake: kind.into(),
            text: Some(format!("{kind} {seq}")),
            brief: None,
        });
        e.seq = seq;
        e.ts = ts;
        e
    }
    fn line(seq: u64, ts: i64, kind: EventKind) -> Event {
        let mut e = Event::new(kind);
        e.seq = seq;
        e.ts = ts;
        e
    }
    fn tool(seq: u64, ts: i64, call_id: &str, error: Option<&str>, body: &str) -> Event {
        line(
            seq,
            ts,
            EventKind::Tool(ToolRec {
                name: "bash".into(),
                call_id: call_id.into(),
                step: 1,
                paths: vec![],
                started: Some(ts),
                ended: Some(ts),
                result_size: Some(body.len() as u64),
                error: error.map(str::to_string),
                body: Some(body.into()),
                args: Some(json!({"command": "ls", "contents": "x".repeat(2000)})),
                child: None,
                images: vec![],
                diff: None,
                label: None,
                output: None,
            }),
        )
    }

    /// user → spawn → turn ends → done wake → answer: one exchange.
    fn story() -> Vec<Event> {
        vec![
            wake(1, 10, "user"),
            tool(2, 11, "c1", None, "spawned w1"),
            line(3, 12, EventKind::TurnComplete { usage: None }),
            wake(4, 20, "done"),
            tool(5, 21, "c2", Some("exit 1"), &"line\n".repeat(3000)),
            line(
                6,
                22,
                EventKind::Assistant {
                    text: "Both done.".into(),
                    step: 2,
                    reasoning_details: None,
                },
            ),
            line(7, 23, EventKind::TurnComplete { usage: None }),
            wake(8, 30, "serve"),
            line(9, 31, EventKind::TurnComplete { usage: None }),
            wake(10, 40, "user"),
            tool(11, 41, "c3", None, "ok"),
            line(12, 42, EventKind::TurnComplete { usage: None }),
        ]
    }

    #[test]
    fn an_exchange_runs_from_a_user_wake_to_the_next_and_housekeeping_stays_inside() {
        let ev = story();
        // The done wake and the serve wake are inside the first exchange.
        assert_eq!(anchor_span(&ev, Some(6), None), Some((0, 9)));
        assert_eq!(anchor_span(&ev, Some(2), None), Some((0, 9)));
        assert_eq!(anchor_span(&ev, Some(11), None), Some((9, 12)));
        // A call id names its exchange.
        assert_eq!(anchor_span(&ev, None, Some("c2")), Some((0, 9)));
        // No seq: the last user wake — not the serve wake.
        assert_eq!(anchor_span(&ev, None, None), Some((9, 12)));
        let only_housekeeping = vec![wake(1, 1, "user"), wake(2, 2, "serve"), wake(3, 3, "job")];
        assert_eq!(anchor_span(&only_housekeeping, None, None), Some((0, 3)));
        assert_eq!(anchor_span(&ev, Some(999), None), None);
    }

    #[test]
    fn budgets_by_outcome_and_fat_args_are_glanced() {
        let ev = story();
        let failed = slim(&ev[4], Budget::Diagnostic);
        let out = failed["output"].as_str().unwrap();
        assert!(
            out.len() <= DIAGNOSTIC_BYTES + 64 && out.contains("bytes cut"),
            "{}",
            out.len()
        );
        assert_eq!(failed["error"], "exit 1");
        assert_eq!(failed["output_clipped"], true);
        assert!(failed.get("body").is_none());
        assert_eq!(failed["args"]["command"], "ls");
        assert!(failed["args"]["contents"].as_str().unwrap().len() < 500);
        assert_eq!(failed["args_clipped"]["contents"], 2000);
        let glance = slim(&ev[1], Budget::Glance);
        assert_eq!(glance["output"], "spawned w1");
        assert!(glance.get("output_clipped").is_none());
        let whole = slim(&ev[4], Budget::Whole);
        assert_eq!(whole["output"].as_str().unwrap().len(), 15_000);
    }

    #[test]
    fn redaction_keeps_the_structure() {
        let mut v = json!({"a": ["sk-or-v1-0123456789abcdef0123456789abcdef", {"b": "api_key = \"zzz\""}], "n": 3});
        let mut c = Counts::default();
        redact_leaves(&mut v, &mut c);
        assert!(v["a"][0].as_str().unwrap().starts_with("[redacted:"));
        assert_eq!(v["a"][1]["b"], "api_key = \"[redacted:value]\"");
        assert_eq!(v["n"], 3);
        assert_eq!(c.tokens + c.values, 2);
    }
}
