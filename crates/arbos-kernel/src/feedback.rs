//! The material for an in-app feedback report about one turn: what the
//! desktop hands over when the user says "this went wrong". Built here so
//! every client sends the same thing, and so nothing leaves the machine
//! that should not: credentials are redacted, tool bodies are replaced by
//! their glance, images and attachments stay as paths, and the whole is
//! bounded in size. The turn the user is looking at, not everything.

use arbos_core::{Event, Place, load_transcript};
use serde_json::{Value, json};

/// The payload cap. Past it, the oldest lines of the turn go first (the
/// wake and the user's line are always kept), then the oldest log lines.
pub const MAX_BYTES: usize = 256 * 1024;
/// Log lines around the turn: this many before its start and after its
/// end, so what the kernel did leading in and out is there.
const LOG_MARGIN_MS: i64 = 5_000;
const LOG_MAX_LINES: usize = 400;

pub struct Bundle {
    pub turn: Value,
    pub events: Vec<Value>,
    pub log: Vec<Value>,
    pub kernel: Value,
    pub note: String,
    pub redacted: Value,
    pub truncated: bool,
    pub bytes: u64,
}

/// The turn of `agent` holding `seq`, or the latest: from its wake to the
/// next wake (or the end).
fn turn_span(events: &[Event], seq: Option<u64>) -> Option<(usize, usize)> {
    let wakes: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| e.is_wake())
        .map(|(i, _)| i)
        .collect();
    let start = match seq {
        Some(s) => {
            let at = events.iter().position(|e| e.seq >= s)?;
            *wakes.iter().rev().find(|&&w| w <= at).or(wakes.first())?
        }
        None => *wakes.last()?,
    };
    let end = wakes
        .iter()
        .find(|&&w| w > start)
        .copied()
        .unwrap_or(events.len());
    Some((start, end))
}

/// A transcript line as the report carries it: tool bodies and diffs out
/// (the glance stays), reasoning details out.
fn slim(mut v: Value) -> Value {
    if let Some(obj) = v.as_object_mut() {
        if obj.get("kind").and_then(Value::as_str) == Some("tool") {
            if obj.get("output").is_none()
                && let Ok(rec) =
                    serde_json::from_value::<arbos_core::ToolRec>(Value::Object(obj.clone()))
                && let Some(d) = rec.digest()
            {
                obj.insert("output".into(), Value::String(d));
            }
            obj.remove("body");
            obj.remove("diff");
        }
        obj.remove("reasoning_details");
    }
    v
}

/// Every string in `v`, redacted: the kernel's own key and granted secrets
/// by value, then credential shapes. Counts what went.
fn redact_value(v: &Value, counts: &mut Counts) -> Value {
    let text = v.to_string();
    let secrets = arbos_engine::secrets::store();
    let by_value = if secrets.has_any() {
        let out = secrets.redact(&text);
        if out != text {
            counts.secrets += 1;
        }
        out
    } else {
        text
    };
    let (out, n) = arbos_core::redact::redact(&by_value);
    counts.tokens += n.tokens;
    counts.values += n.values;
    counts.blocks += n.blocks;
    serde_json::from_str(&out).unwrap_or_else(|_| Value::String(out))
}

#[derive(Default)]
struct Counts {
    secrets: usize,
    tokens: usize,
    values: usize,
    blocks: usize,
}

fn log_lines(place: &Place, from_ms: i64, to_ms: i64, agent: &str) -> Vec<Value> {
    let path = crate::klog::log_path_for(&place.arbos());
    let Ok(text) = std::fs::read_to_string(&path) else {
        return vec![];
    };
    let lines: Vec<Value> = text
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| {
            let ts = v["ts"].as_i64().unwrap_or(0);
            ts >= from_ms - LOG_MARGIN_MS && ts <= to_ms + LOG_MARGIN_MS
        })
        // This agent's lines and the kernel's own; other agents' turns
        // are not the user's complaint.
        .filter(|v| v["agent"].is_null() || v["agent"] == agent)
        .collect();
    let skip = lines.len().saturating_sub(LOG_MAX_LINES);
    lines[skip..].to_vec()
}

fn payload_bytes(events: &[Value], log: &[Value]) -> usize {
    events.iter().map(|e| e.to_string().len()).sum::<usize>()
        + log.iter().map(|l| l.to_string().len()).sum::<usize>()
}

/// Build the report material for `agent`'s turn holding `seq` (or its
/// latest turn). `host` names the model and provider the place runs on.
pub fn bundle(
    place: &Place,
    agent: &str,
    seq: Option<u64>,
    note: &str,
    host: &arbos_engine::Host,
) -> Bundle {
    let transcript = arbos_core::Layout::new(place, agent).transcript();
    let events = load_transcript(&transcript).unwrap_or_default();
    let mut counts = Counts::default();
    let span = turn_span(&events, seq);
    let (turn_events, started_ms, ended_ms, from_seq, to_seq, complete) = match span {
        Some((s, e)) => {
            let slice = &events[s..e];
            let started = slice.first().map(|x| x.ts).unwrap_or(0);
            let ended = slice.last().map(|x| x.ts).unwrap_or(started);
            let complete = slice.iter().any(|x| x.is_turn_complete());
            (
                slice.to_vec(),
                started,
                if complete {
                    ended
                } else {
                    arbos_core::now_ms()
                },
                slice.first().map(|x| x.seq).unwrap_or(0),
                slice.last().map(|x| x.seq).unwrap_or(0),
                complete,
            )
        }
        None => (vec![], 0, arbos_core::now_ms(), 0, 0, false),
    };
    let mut lines: Vec<Value> = turn_events
        .iter()
        .map(|e| {
            redact_value(
                &slim(serde_json::to_value(e).unwrap_or(Value::Null)),
                &mut counts,
            )
        })
        .collect();
    let mut log: Vec<Value> = log_lines(place, started_ms, ended_ms, agent)
        .iter()
        .map(|l| redact_value(l, &mut counts))
        .collect();
    // The cap: oldest log lines first, then the turn's middle (its first
    // two lines — the wake and the user's words — and its last stay).
    let mut truncated = false;
    while payload_bytes(&lines, &log) > MAX_BYTES && !log.is_empty() {
        log.remove(0);
        truncated = true;
    }
    while payload_bytes(&lines, &log) > MAX_BYTES && lines.len() > 3 {
        lines.remove(2);
        truncated = true;
    }
    let (note_text, n) = arbos_core::redact::redact(note);
    counts.tokens += n.tokens;
    counts.values += n.values;
    counts.blocks += n.blocks;
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
        "from": from_seq,
        "to": to_seq,
        "started_ms": started_ms,
        "ended_ms": ended_ms,
        "complete": complete,
        "lines": lines.len(),
        "of": turn_events.len(),
    });
    let bytes = payload_bytes(&lines, &log) as u64;
    Bundle {
        turn,
        events: lines,
        log,
        kernel,
        note: note_text,
        redacted: json!({
            "secrets": counts.secrets,
            "tokens": counts.tokens,
            "values": counts.values,
            "blocks": counts.blocks,
        }),
        truncated,
        bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbos_core::EventKind;

    fn ev(seq: u64, ts: i64, kind: EventKind) -> Event {
        let mut e = Event::new(kind);
        e.seq = seq;
        e.ts = ts;
        e
    }

    #[test]
    fn the_turn_holding_a_seq_is_its_wake_through_the_next_wake() {
        let events = vec![
            ev(
                1,
                10,
                EventKind::Wake {
                    wake: "user".into(),
                    text: Some("a".into()),
                    brief: None,
                },
            ),
            ev(2, 11, EventKind::TurnComplete { usage: None }),
            ev(
                3,
                20,
                EventKind::Wake {
                    wake: "user".into(),
                    text: Some("b".into()),
                    brief: None,
                },
            ),
            ev(
                4,
                21,
                EventKind::Assistant {
                    text: "hi".into(),
                    step: 1,
                    reasoning_details: None,
                },
            ),
            ev(5, 22, EventKind::TurnComplete { usage: None }),
            ev(
                6,
                30,
                EventKind::Wake {
                    wake: "user".into(),
                    text: Some("c".into()),
                    brief: None,
                },
            ),
        ];
        assert_eq!(turn_span(&events, Some(4)), Some((2, 5)));
        assert_eq!(turn_span(&events, Some(1)), Some((0, 2)));
        assert_eq!(turn_span(&events, None), Some((5, 6)));
        assert_eq!(turn_span(&events, Some(99)), None);
        assert_eq!(turn_span(&[], None), None);
    }

    #[test]
    fn a_tool_line_is_slimmed_to_its_glance() {
        let v = json!({"kind":"tool","name":"bash","call_id":"c","body":"a very long body","diff":"---","args":{"command":"ls"}});
        let s = slim(v);
        assert!(s.get("body").is_none() && s.get("diff").is_none());
        assert_eq!(s["output"], "a very long body");
        assert_eq!(s["args"]["command"], "ls");
    }
}
