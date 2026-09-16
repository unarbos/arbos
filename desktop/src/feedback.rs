//! What an in-app report is made of, and where it waits.
//!
//! Jacob presses one control when something looks wrong. The kernel hands
//! over the exchange he is looking at — its trajectory, the log for its
//! span, which build answered — already redacted of credentials
//! ([`arbos_core::redact`]). This module adds what only the app knows: a
//! picture of the window, the app's own view of the chat, and his words.
//! Then it writes the whole thing to disk **before** anything is sent, so a
//! report made with the network down is not lost.
//!
//! Nothing here sends. Delivery reads the outbox; the sheet decides what
//! goes in it. One rule holds the design together: a part he removes is
//! recorded as removed, never simply left out, so a loop reading the report
//! can tell his choice from a fault.

use anyhow::{Context, Result};
use arbos_core::Place;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Whether a report carries tool arguments and outputs unless he says
/// otherwise.
///
/// This is Jacob's open question, and it is one line on purpose. The
/// trajectory carries the arguments of the calls his agents made, which
/// means the contents of files they wrote. Credential redaction catches
/// credentials by shape; nothing catches "this file is mine". Everything
/// goes to our own hub and never the public repository, but it does leave
/// his machine.
///
/// `true`: the control starts on, and a report is actionable out of the box.
/// `false`: the control starts off, and he turns it on for the reports where
/// the code is the point. Either way the control is there and either way the
/// report says which he chose.
pub const TOOL_IO_BY_DEFAULT: bool = true;

/// How many transcript lines before the exchange to ask the kernel for. Its
/// own cap is 500; this is what a behaviour bug has needed in practice
/// without making the review sheet unreadable.
pub const TAIL_LINES: u32 = 200;

/// A screenshot wider than this is scaled down before it is encoded. The
/// kernel serves one file at most 1 MiB (`files::READ_CAP`), and the
/// poller reads the picture as base64 through that door, so a Retina window
/// at full size would arrive cut — and a cut base64 string is a broken
/// image, which is worse than a small one.
pub const SHOT_MAX_BASE64: usize = 900 * 1024;

/// The kernel's answer, as the app holds it.
#[derive(Clone, Debug, Default)]
pub struct Bundle {
    pub agent: String,
    pub turn: Value,
    pub events: Vec<Value>,
    pub tail: Vec<Value>,
    pub children: Vec<Value>,
    pub log: Vec<Value>,
    pub kernel: Value,
    /// His words as the kernel redacted them. The app shows its own copy and
    /// sends this one, so what he reads is what leaves.
    pub note: String,
    pub redacted: Value,
    pub truncated: bool,
    pub bytes: u64,
}

impl Bundle {
    /// How many credentials went, across all four kinds the kernel counts.
    pub fn credentials_removed(&self) -> u64 {
        ["secrets", "tokens", "values", "blocks"]
            .iter()
            .filter_map(|k| self.redacted.get(*k).and_then(Value::as_u64))
            .sum()
    }

    pub fn tool_calls(&self) -> usize {
        self.events
            .iter()
            .filter(|e| e.get("kind").and_then(Value::as_str) == Some("tool"))
            .count()
    }

    pub fn failed_calls(&self) -> usize {
        self.events
            .iter()
            .filter(|e| {
                e.get("kind").and_then(Value::as_str) == Some("tool")
                    && e.get("error").is_some_and(|v| !v.is_null())
            })
            .count()
    }
}

/// A captured picture of the window.
#[derive(Clone, Debug)]
pub struct Shot {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// `image/png` or `image/jpeg`.
    pub mime: String,
}

impl Shot {
    fn suffix(&self) -> &'static str {
        if self.mime == "image/jpeg" {
            "jpg"
        } else {
            "png"
        }
    }
}

/// Which parts of a report he is sending. Every field is a row in the sheet
/// with its own way to remove it, and every field is written into the report
/// whether it is true or false.
#[derive(Clone, Copy, Debug)]
pub struct Parts {
    pub screenshot: bool,
    pub trajectory: bool,
    pub log: bool,
    pub tail: bool,
    pub session: bool,
    /// The one that is his call rather than ours: tool arguments and the
    /// glance at their output.
    pub tool_io: bool,
}

impl Default for Parts {
    fn default() -> Self {
        Self {
            screenshot: true,
            trajectory: true,
            log: true,
            tail: true,
            session: true,
            tool_io: TOOL_IO_BY_DEFAULT,
        }
    }
}

/// A report being written. The sheet holds one of these and shows it.
#[derive(Clone, Debug, Default)]
pub struct Draft {
    /// His words, as typed in the sheet.
    pub note: String,
    pub bundle: Option<Bundle>,
    pub shot: Option<Shot>,
    /// Why there is no picture, when there is none: a refused permission, a
    /// missing capture program. Shown rather than swallowed, so a report with
    /// no screenshot does not read as one he cut.
    pub shot_error: Option<String>,
    /// The app's own view of the chat. The transcript is usually right and
    /// the drawing is wrong, and the divergence is the bug, so both travel.
    pub session: Option<Value>,
    pub parts: Parts,
    /// What the app is, compiled in.
    pub app: Value,
}

impl Draft {
    pub fn new(parts: Parts) -> Self {
        Self {
            parts,
            app: json!({
                "version": env!("CARGO_PKG_VERSION"),
                "build": crate::build::BUILD,
                "commit": crate::build::COMMIT,
                "kernel_version": crate::build::KERNEL_VERSION,
                "label": crate::build::version_label(),
            }),
            ..Default::default()
        }
    }

    /// True once there is something worth sending. His words alone are
    /// enough — half his reports are about a thing on screen, not an answer.
    pub fn ready(&self) -> bool {
        !self.note.trim().is_empty()
    }

    /// How many tool calls would lose their arguments and output if the
    /// control were off. What the sheet says beside it, so the cost of the
    /// choice is visible before he makes it.
    pub fn tool_io_count(&self) -> usize {
        self.bundle.as_ref().map_or(0, Bundle::tool_calls)
    }

    /// The report as it will be written: his cuts applied, the toggle
    /// applied, and `included` naming every decision.
    pub fn report(&self, id: &str, sent_ms: i64) -> Value {
        let b = self.bundle.clone().unwrap_or_default();
        let mut events = if self.parts.trajectory {
            b.events
        } else {
            vec![]
        };
        let mut tail = if self.parts.tail && self.parts.trajectory {
            b.tail
        } else {
            vec![]
        };
        let mut children = if self.parts.trajectory {
            b.children
        } else {
            vec![]
        };
        let mut stripped = 0usize;
        if !self.parts.tool_io {
            stripped += strip_tool_io(&mut events);
            stripped += strip_tool_io(&mut tail);
            for child in &mut children {
                if let Some(list) = child.get_mut("events").and_then(Value::as_array_mut) {
                    stripped += strip_tool_io(list);
                }
            }
        }

        let mut report = json!({
            "id": id,
            "sent_ms": sent_ms,
            "note": self.note.trim(),
            "app": self.app,
            "kernel": b.kernel,
            "agent": b.agent,
            "turn": b.turn,
            "events": events,
            "tail": tail,
            "children": children,
            "log": if self.parts.log { b.log } else { vec![] },
            "session": if self.parts.session {
                self.session.clone().unwrap_or(Value::Null)
            } else {
                Value::Null
            },
            "redacted": b.redacted,
            "tool_io_stripped": stripped,
            "truncated": b.truncated,
            // Every decision, true or false. A part that is merely absent
            // would leave a loop guessing whether he cut it or it was never
            // there, and it would chase the wrong one.
            "included": {
                "screenshot": self.parts.screenshot && self.shot.is_some(),
                "trajectory": self.parts.trajectory,
                "log": self.parts.log,
                "tail": self.parts.tail && self.parts.trajectory,
                "session": self.parts.session && self.session.is_some(),
                "tool_io": self.parts.tool_io,
            },
            "screenshot_error": self.shot_error,
        });

        // The last thing that happens to a report before it is written: every
        // string in it, whatever its source, through the credential redactor.
        //
        // The kernel already redacts what it hands over, but two parts of a
        // report are the app's own and never went through it — the words Jacob
        // types, and the app's view of the chat. He consented to his code
        // travelling, not his keys, so the guarantee has to hold for the whole
        // object rather than for the parts that happen to have come from the
        // kernel. One choke point is checkable; several are hopeful.
        let mut out = arbos_core::redact::Redacted::default();
        redact_strings(&mut report, &mut out);
        report["redacted_on_the_way_out"] = json!({
            "tokens": out.tokens,
            "values": out.values,
            "blocks": out.blocks,
        });
        report
    }

    /// Whether his own words held something credential-shaped, for the sheet
    /// to say so while he can still see it. He may have pasted a log line in.
    pub fn note_redaction(&self) -> arbos_core::redact::Redacted {
        arbos_core::redact::redact(&self.note).1
    }
}

/// Every string value in `v`, however deep, replaced by its redacted form.
///
/// Values only: a key is a field name this app chose, never a provider's text,
/// and rewriting keys would change the report's shape for nothing.
fn redact_strings(v: &mut Value, n: &mut arbos_core::redact::Redacted) {
    match v {
        Value::String(s) => {
            let (out, counted) = arbos_core::redact::redact(s);
            if &out != s {
                *s = out;
            }
            n.tokens += counted.tokens;
            n.values += counted.values;
            n.blocks += counted.blocks;
        }
        Value::Array(items) => items.iter_mut().for_each(|i| redact_strings(i, n)),
        Value::Object(map) => map
            .iter_mut()
            .for_each(|(_, value)| redact_strings(value, n)),
        _ => {}
    }
}

/// Take the arguments and the output glance off every tool line, leaving the
/// call itself: its name, when it ran, how big the output was, and whether
/// it failed. So a report with the control off still shows *what the agent
/// did* — which is most of the diagnosis — without carrying his code.
///
/// Returns how many calls were changed.
pub fn strip_tool_io(events: &mut [Value]) -> usize {
    let mut n = 0;
    for e in events.iter_mut() {
        if e.get("kind").and_then(Value::as_str) != Some("tool") {
            continue;
        }
        let Some(obj) = e.as_object_mut() else {
            continue;
        };
        let had = obj.remove("args").is_some()
            | obj.remove("args_clipped").is_some()
            | obj.remove("output").is_some()
            | obj.remove("body").is_some()
            | obj.remove("diff").is_some();
        // The paths name his folders, which is the same objection as the
        // contents, so they go with them. The error text stays: it is the
        // symptom, and it is what he is complaining about.
        obj.remove("paths");
        if had {
            obj.insert("io_removed".into(), Value::Bool(true));
            n += 1;
        }
    }
    n
}

// --------------------------------------------------------------------------
// What he reads before he sends
//
// His consent is to a category — "my code may go" — and that is not consent
// to sending something he cannot see. So every part turns into lines a person
// can scan at the speed they read, not the JSON that actually travels. A wall
// of JSON in a dialog is the same as showing nothing.
// --------------------------------------------------------------------------

/// One readable line per transcript entry: who spoke, what ran, what failed.
pub fn event_lines(events: &[Value]) -> Vec<String> {
    events.iter().map(event_line).collect()
}

fn event_line(e: &Value) -> String {
    let s = |k: &str| e.get(k).and_then(Value::as_str).unwrap_or_default();
    let kind = s("kind");
    match kind {
        "wake" => {
            let wake = s("wake");
            let text = clip(s("text"), 120);
            if wake == "user" || wake == "kickoff" {
                format!("you: {text}")
            } else if text.is_empty() {
                format!("woke ({wake})")
            } else {
                format!("woke ({wake}): {text}")
            }
        }
        "user" => format!("you: {}", clip(s("text"), 120)),
        "assistant" => format!("Arbos: {}", clip(s("text"), 120)),
        "thought" => format!("thought: {}", clip(s("text"), 90)),
        "say" => format!("{} said: {}", s("from"), clip(s("text"), 100)),
        "tool" => tool_line(e),
        "turn_complete" => "— turn ended —".into(),
        "" => "(unrecognised line)".into(),
        other => format!("{other}: {}", clip(s("text"), 100)),
    }
}

fn tool_line(e: &Value) -> String {
    let name = e.get("name").and_then(Value::as_str).unwrap_or("tool");
    let mut line = String::from(name);
    // The most telling argument, named the way the tool names it.
    for key in ["command", "path", "pattern", "agent", "url"] {
        if let Some(v) = e
            .get("args")
            .and_then(|a| a.get(key))
            .and_then(Value::as_str)
        {
            line.push_str(" · ");
            line.push_str(&clip(v, 80));
            break;
        }
    }
    if e.get("io_removed").and_then(Value::as_bool) == Some(true) {
        line.push_str(" · (arguments and output removed)");
    }
    if let Some(size) = e.get("result_size").and_then(Value::as_u64) {
        line.push_str(&format!(" · {}", bytes_human(size)));
    }
    if let Some(err) = e.get("error").and_then(Value::as_str) {
        line.push_str(" · failed: ");
        line.push_str(&clip(err, 80));
    }
    if let Some(cut) = e.get("args_clipped").and_then(Value::as_object) {
        let which: Vec<String> = cut
            .iter()
            .map(|(k, v)| format!("{k} {}", bytes_human(v.as_u64().unwrap_or(0))))
            .collect();
        line.push_str(&format!(" · shortened: {}", which.join(", ")));
    }
    line
}

/// One line per kernel log entry, with its clock.
pub fn log_lines(log: &[Value]) -> Vec<String> {
    log.iter()
        .map(|l| {
            let ts = l.get("ts").and_then(Value::as_i64).unwrap_or(0);
            let level = l.get("level").and_then(Value::as_str).unwrap_or("");
            let event = l.get("event").and_then(Value::as_str).unwrap_or("");
            let detail = l
                .get("detail")
                .map(|d| {
                    d.as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| d.to_string())
                })
                .unwrap_or_default();
            let clock = clock_of(ts);
            if detail.is_empty() {
                format!("{clock} {level} {event}")
            } else {
                format!("{clock} {level} {event} — {}", clip(&detail, 110))
            }
        })
        .collect()
}

/// What the app itself believes the chat holds. Deliberately shallow: the
/// point is whether its count and its kinds match the transcript, not its
/// internals.
pub fn session_lines(session: &Value) -> Vec<String> {
    let Some(items) = session.get("items").and_then(Value::as_array) else {
        return vec!["(the app kept no view of this chat)".into()];
    };
    let mut out = vec![format!("{} items drawn in this chat", items.len())];
    out.extend(items.iter().take(60).enumerate().map(|(n, item)| {
        let kind = item
            .as_object()
            .and_then(|o| o.keys().next().cloned())
            .unwrap_or_else(|| "?".into());
        format!("{}. {kind}", n + 1)
    }));
    out
}

fn clock_of(ms: i64) -> String {
    let rest = (ms / 1000).rem_euclid(86_400);
    format!(
        "{:02}:{:02}:{:02}",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}

fn clip(text: &str, max: usize) -> String {
    let flat = text.replace('\n', " ").trim().to_string();
    if flat.chars().count() <= max {
        return flat;
    }
    format!(
        "{}…",
        flat.chars().take(max.saturating_sub(1)).collect::<String>()
    )
}

pub fn bytes_human(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{} KB", n / 1024)
    } else {
        format!("{:.1} MB", n as f64 / (1024. * 1024.))
    }
}

/// A report's own name: the moment it was made, and enough randomness that
/// two in the same second cannot collide. The loop turns this into the
/// `<date>-<n>` a person reads, the way the phone loop turns an App Store
/// Connect identifier into `2026-09-16-1`.
pub fn new_id(now_ms: i64) -> String {
    let when = format_utc(now_ms);
    let salt = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{when}-{:04x}", salt & 0xffff)
}

fn format_utc(ms: i64) -> String {
    // Only the pieces of a timestamp a folder name needs, so this owes no
    // date crate: the app already carries enough dependencies.
    let secs = ms / 1000;
    let days = secs.div_euclid(86_400);
    let rest = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}

/// Howard Hinnant's `civil_from_days`, the standard way to turn a day count
/// since 1970 into a calendar date without a table.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Where reports wait for delivery: on his own disk, under the project's
/// `.arbos/`, beside the sessions the app already keeps there.
pub fn outbox(place: &Place) -> PathBuf {
    place.arbos().join("desktop").join("feedback-outbox")
}

/// Write the report to the outbox and return its folder.
///
/// This runs before anything is sent, and its success is what the sheet
/// calls "sent". A report is on disk or it is not; there is no state where
/// he pressed the button and nothing exists.
pub fn write(place: &Place, draft: &Draft, id: &str, now_ms: i64) -> Result<PathBuf> {
    let dir = outbox(place).join(id);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("make the outbox folder {}", dir.display()))?;

    let report = draft.report(id, now_ms);
    write_atomic(
        &dir.join("report.json"),
        (serde_json::to_string_pretty(&report)? + "\n").as_bytes(),
    )?;

    // The picture travels as base64 in its own file, because the door it is
    // read through serves text. Its own file rather than a field inside the
    // report, so neither one pushes the other past the read cap.
    if draft.parts.screenshot {
        if let Some(shot) = &draft.shot {
            let text = encode_shot(shot);
            write_atomic(&dir.join("screenshot.b64"), text.as_bytes())?;
            // A copy in its real form too: this folder is also what he can
            // hand to someone directly, and a person cannot open base64.
            write_atomic(
                &dir.join(format!("screenshot.{}", shot.suffix())),
                &shot.bytes,
            )?;
        }
    }

    // Last, and only once everything else is on disk: delivery looks for
    // this file, so a half-written report is never picked up.
    write_atomic(&dir.join("ready"), b"")?;
    Ok(dir)
}

fn encode_shot(shot: &Shot) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(&shot.bytes)
}

/// True when the encoded picture would not fit through the read cap.
pub fn shot_too_big(shot: &Shot) -> bool {
    shot.bytes.len().saturating_mul(4).div_ceil(3) > SHOT_MAX_BASE64
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("place {}", path.display()))?;
    Ok(())
}

/// Reports written and not yet delivered, oldest first.
pub fn pending(place: &Place) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(outbox(place))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("ready").exists() && !p.join("delivered").exists())
        .collect();
    out.sort();
    out
}

/// What the loop wrote back about a report of his: which build carries the
/// fix. Read on attach so the answer reaches him where the complaint left.
#[derive(Clone, Debug)]
pub struct Fixed {
    pub report: String,
    pub build: String,
    pub pr: Option<u64>,
    pub what: String,
}

pub fn read_fixed(dir: &Path) -> Option<Fixed> {
    let v: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("fixed.json")).ok()?).ok()?;
    Some(Fixed {
        report: v.get("report")?.as_str()?.to_string(),
        build: v.get("build")?.as_str().unwrap_or_default().to_string(),
        pr: v.get("pr").and_then(Value::as_u64),
        what: v
            .get("what")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

// --------------------------------------------------------------------------
// His last answer to the toggle
//
// Remembered per machine so he does not re-decide every time. Not in
// `settings.toml`: that file is edited by hand and describes the app, while
// this is one person's answer to one question.
// --------------------------------------------------------------------------

fn prefs_path(place: &Place) -> PathBuf {
    place.arbos().join("desktop").join("feedback.json")
}

pub fn load_parts(place: &Place) -> Parts {
    let mut parts = Parts::default();
    if let Ok(text) = std::fs::read_to_string(prefs_path(place))
        && let Ok(v) = serde_json::from_str::<Value>(&text)
        && let Some(kept) = v.get("tool_io").and_then(Value::as_bool)
    {
        parts.tool_io = kept;
    }
    parts
}

pub fn save_parts(place: &Place, parts: &Parts) {
    let path = prefs_path(place);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, json!({"tool_io": parts.tool_io}).to_string() + "\n");
}

// --------------------------------------------------------------------------
// The picture
// --------------------------------------------------------------------------

/// Capture the Arbos window, `width` x `height` points.
///
/// The whole screen is never taken: his other windows are not the report.
/// On macOS this needs the Screen Recording permission, and a refusal comes
/// back as an error rather than an empty picture — the sheet shows the
/// reason and the report goes without it, because a report that cannot be
/// sent is worse than a report with no screenshot.
pub fn capture_window(width: f32, height: f32) -> Result<Shot> {
    let path = std::env::temp_dir().join(format!(
        "arbos-feedback-{}-{}.png",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));
    let id = crate::driver::ns_window_number(width, height)
        .context("could not find this window to photograph")?;
    crate::driver::capture_window(id, &path)?;
    let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    let _ = std::fs::remove_file(&path);
    if bytes.is_empty() {
        anyhow::bail!("the capture wrote nothing");
    }
    Ok(Shot {
        bytes,
        width: width as u32,
        height: height as u32,
        mime: "image/png".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, with_error: bool) -> Value {
        json!({
            "kind": "tool", "seq": 5, "name": name, "call_id": "c1",
            "args": {"command": "cargo test", "contents": "fn main() {}"},
            "output": "running 41 tests", "paths": ["/Users/jacob/secret/src/lib.rs"],
            "result_size": 41233,
            "error": if with_error { json!("exit 101") } else { Value::Null },
        })
    }

    #[test]
    fn the_toggle_takes_his_code_and_leaves_what_the_agent_did() {
        let mut events = vec![
            json!({"kind": "wake", "seq": 1, "wake": "user", "text": "add the retry"}),
            tool("bash", true),
            tool("write", false),
        ];
        assert_eq!(strip_tool_io(&mut events), 2);
        for e in &events[1..] {
            assert!(e.get("args").is_none(), "his code went: {e}");
            assert!(e.get("output").is_none());
            assert!(e.get("paths").is_none(), "his folders went too: {e}");
            assert_eq!(e["io_removed"], json!(true));
            // What the agent did survives, because that is the diagnosis.
            assert!(e.get("name").is_some());
            assert_eq!(e["result_size"], json!(41233));
        }
        assert_eq!(events[1]["error"], json!("exit 101"), "the symptom stays");
        assert_eq!(events[0]["text"], json!("add the retry"), "his words stay");
    }

    #[test]
    fn a_removed_part_is_recorded_as_removed_not_left_out() {
        let mut draft = Draft::new(Parts::default());
        draft.note = "  the sidebar draws twice  ".into();
        draft.bundle = Some(Bundle {
            events: vec![tool("bash", false)],
            log: vec![json!({"ts": 1, "event": "turn.start"})],
            tail: vec![json!({"kind": "wake", "seq": 1})],
            ..Default::default()
        });
        draft.parts.log = false;
        draft.parts.screenshot = false;

        let r = draft.report("20260916T154210Z-7f3a", 1_789_573_330_000);
        assert_eq!(r["included"]["log"], json!(false));
        assert_eq!(r["included"]["screenshot"], json!(false));
        assert_eq!(r["included"]["trajectory"], json!(true));
        assert!(r["log"].as_array().unwrap().is_empty());
        assert_eq!(r["note"], json!("the sidebar draws twice"));
        assert_eq!(r["tool_io_stripped"], json!(0));

        // Cutting the trajectory takes the tail and the children with it:
        // they are the same material, and a tail without its exchange would
        // read as the whole story.
        draft.parts.trajectory = false;
        let r = draft.report("x", 0);
        assert!(r["events"].as_array().unwrap().is_empty());
        assert!(r["tail"].as_array().unwrap().is_empty());
        assert_eq!(r["included"]["tail"], json!(false));
    }

    #[test]
    fn the_toggle_off_is_counted_in_the_report() {
        let mut draft = Draft::new(Parts {
            tool_io: false,
            ..Parts::default()
        });
        draft.note = "it printed my key".into();
        draft.bundle = Some(Bundle {
            events: vec![tool("bash", true), tool("read", false)],
            children: vec![json!({"agent": "w1", "events": [tool("bash", false)]})],
            ..Default::default()
        });
        let r = draft.report("x", 0);
        assert_eq!(r["included"]["tool_io"], json!(false));
        assert_eq!(r["tool_io_stripped"], json!(3), "children counted too");
    }

    #[test]
    fn an_id_is_a_readable_moment_and_two_never_collide() {
        let a = new_id(1_789_573_330_000);
        assert!(a.starts_with("20260916T"), "{a}");
        assert_eq!(a.len(), "20260916T154210Z-7f3a".len(), "{a}");
        assert_eq!(&format_utc(0), "19700101T000000Z");
    }

    #[test]
    fn a_picture_too_big_for_the_door_is_known_before_it_is_sent() {
        let small = Shot {
            bytes: vec![0; 1000],
            width: 1,
            height: 1,
            mime: "image/png".into(),
        };
        assert!(!shot_too_big(&small));
        let big = Shot {
            bytes: vec![0; SHOT_MAX_BASE64],
            width: 1,
            height: 1,
            mime: "image/png".into(),
        };
        assert!(shot_too_big(&big));
    }

    #[test]
    fn every_part_reads_as_lines_a_person_can_scan() {
        let lines = event_lines(&[
            json!({"kind": "wake", "wake": "user", "text": "add the retry\nplease"}),
            json!({"kind": "wake", "wake": "done", "text": "w1 reported"}),
            json!({"kind": "tool", "name": "bash", "args": {"command": "cargo test"},
                   "result_size": 41233, "error": "exit 101"}),
            json!({"kind": "tool", "name": "write", "args": {"path": "src/retry.rs"},
                   "args_clipped": {"contents": 31204}}),
            json!({"kind": "assistant", "text": "Done."}),
            json!({"kind": "turn_complete"}),
        ]);
        assert_eq!(lines[0], "you: add the retry please", "newlines flattened");
        assert_eq!(lines[1], "woke (done): w1 reported");
        assert_eq!(lines[2], "bash · cargo test · 40 KB · failed: exit 101");
        assert_eq!(lines[3], "write · src/retry.rs · shortened: contents 30 KB");
        assert_eq!(lines[4], "Arbos: Done.");
        assert_eq!(lines[5], "— turn ended —");

        // With the toggle off, the line says so rather than looking empty.
        let mut stripped = vec![json!({"kind": "tool", "name": "read", "args": {"path": "x"}})];
        strip_tool_io(&mut stripped);
        assert_eq!(
            event_lines(&stripped)[0],
            "read · (arguments and output removed)"
        );

        assert_eq!(
            log_lines(&[
                json!({"ts": 55_000, "level": "warn", "event": "attach.drop",
                               "detail": "peer went away"})
            ])[0],
            "00:00:55 warn attach.drop — peer went away"
        );
        assert_eq!(
            session_lines(&json!({"items": [{"User": {}}, {"Assistant": {}}]}))[0],
            "2 items drawn in this chat"
        );
        assert_eq!(
            session_lines(&Value::Null)[0],
            "(the app kept no view of this chat)"
        );
    }

    #[test]
    fn a_long_line_is_cut_with_a_mark_rather_than_wrapping_the_sheet() {
        let long = "x".repeat(400);
        let out = event_line(&json!({"kind": "assistant", "text": long}));
        assert!(out.ends_with('…'), "{out}");
        assert!(out.chars().count() <= 128, "{}", out.chars().count());
    }

    /// The claim that matters most, checked rather than reasoned: **no path
    /// through the sheet can send a credential.**
    ///
    /// Jacob consented to his code travelling, not his keys. The kernel
    /// redacts what it hands over, but his typed words and the app's own view
    /// of the chat never pass through it, so this plants a credential of every
    /// shape in every field a report has — including the two the kernel never
    /// sees — and asserts none of them survives into what is written.
    #[test]
    fn no_field_of_a_report_can_carry_a_credential_out() {
        // One of each shape the redactor knows, in the form a real one takes —
        // assembled here rather than written out. Written whole, these are
        // shape-valid enough that GitHub's own secret scanner blocks the push,
        // which is a fair sign the shapes are right and a good reason to keep
        // the literals out of the file.
        let body = "3f8a9b2c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a6b7c8d9e0f1a";
        let planted = [
            format!("sk-{}v1-{body}", "or-"),
            format!("gh{}16C7e42F292c6912E7710c838347Ae178B4a", "p_"),
            format!("xo{}2401234567-2410987654321-AbCdEfGhIjKlMnOpQrStUvWx", "xb-"),
            format!("AK{}IOSFODNN7EXAMPLE", "IA"),
            format!("AI{}D-1234567890abcdefghijklmnopqrstu", "zaSy"),
            format!("sk{}51H8xkLMnOpQrStUvWxYz0123456789", "_live_"),
            format!("op:{}Arbos/OpenRouter/credential", "//"),
            format!(
                "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxIn0.{}",
                "dBjftJeZ4CVPmB92K27uhbUJU1p1r_wW1gFWFOEjXk"
            ),
        ];
        let secret_line = format!("api_key = {}", planted[0]);
        let pem =
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\n-----END RSA PRIVATE KEY-----";

        let mut draft = Draft::new(Parts::default());
        // 1. His own words — typed here, never through the kernel.
        draft.note = format!("it printed my key {} in the chat", planted[0]);
        // 2. The app's own view of the chat — likewise never through the kernel.
        draft.session = Some(json!({
            "items": [
                {"User": {"text": format!("use {}", planted[1])}},
                {"Assistant": {"text": secret_line.clone()}},
                {"Tool": {"body": pem}},
            ]
        }));
        // 3. Everything the kernel hands over, as though its own redaction had
        //    failed or an older kernel had none at all.
        draft.bundle = Some(Bundle {
            events: vec![
                json!({"kind": "assistant", "text": &planted[2]}),
                json!({"kind": "tool", "name": "bash",
                       "args": {"command": format!("export TOKEN={}", planted[3])},
                       "output": &planted[4], "error": &planted[5]}),
            ],
            tail: vec![json!({"kind": "user", "text": &planted[6]})],
            children: vec![json!({"agent": "w1",
                                  "events": [{"kind": "assistant", "text": &planted[7]}]})],
            log: vec![json!({"ts": 1, "event": "provider.call", "detail": secret_line.clone()})],
            kernel: json!({"version": "0.2.0", "model": "x", "extra": pem}),
            note: planted[0].clone(),
            ..Default::default()
        });

        let report = draft.report("20260916T154210Z-7f3a", 1_789_573_330_000);
        let written = serde_json::to_string(&report).unwrap();

        for secret in &planted {
            assert!(
                !written.contains(secret.as_str()),
                "a credential reached the report: {secret}\n{written}"
            );
        }
        assert!(
            !written.contains("MIIEowIBAAKCAQEA"),
            "a private key body survived"
        );
        // The redactor left a mark rather than silently deleting, and the
        // report counts what went — so a reviewer can see it worked.
        assert!(
            written.contains("redacted"),
            "the removal is marked: {written}"
        );
        // The count is on the total, not per bucket: which bucket a removal
        // falls in is the redactor's business, and it moves — a key inside
        // `api_key = …` is caught as a value before it is ever seen as a
        // token. What this asserts is that nothing planted went uncounted.
        let counted = &report["redacted_on_the_way_out"];
        let total: u64 = ["tokens", "values", "blocks"]
            .iter()
            .map(|k| counted[*k].as_u64().unwrap_or(0))
            .sum();
        assert!(
            total >= planted.len() as u64,
            "every planted credential counted: {counted}"
        );
        assert!(
            counted["blocks"].as_u64().unwrap() >= 2,
            "both PEM blocks: {counted}"
        );

        // And the prose around them survives, or the report would be useless.
        assert!(written.contains("it printed my key"));
        assert!(written.contains("in the chat"));
        assert!(written.contains("bash"));
    }

    /// The toggle is not a way round the redactor: with tool arguments removed
    /// the credential is gone too, and with them sent it is still gone.
    #[test]
    fn the_toggle_changes_what_travels_but_never_whether_a_key_does() {
        let key = format!("sk-{}v1-3f8a9b2c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a6b7c8d9e0f1a", "or-");
        for tool_io in [true, false] {
            let mut draft = Draft::new(Parts {
                tool_io,
                ..Parts::default()
            });
            draft.note = "look".into();
            draft.bundle = Some(Bundle {
                events: vec![json!({"kind": "tool", "name": "bash",
                                    "args": {"command": format!("curl -H 'Authorization: Bearer {key}'")}})],
                ..Default::default()
            });
            let written = serde_json::to_string(&draft.report("x", 0)).unwrap();
            assert!(!written.contains(key.as_str()), "tool_io={tool_io}: {written}");
        }
    }

    /// The sheet can tell him his own words held one, while he can still act.
    #[test]
    fn his_words_are_checked_where_he_can_still_see_them() {
        let mut draft = Draft::new(Parts::default());
        draft.note = format!("it printed gh{}16C7e42F292c6912E7710c838347Ae178B4a at me", "p_");
        assert_eq!(draft.note_redaction().tokens, 1);
        draft.note = "the sidebar draws twice".into();
        assert_eq!(draft.note_redaction().total(), 0);
    }

    #[test]
    fn credentials_removed_adds_the_four_kinds_up() {
        let b = Bundle {
            redacted: json!({"secrets": 1, "tokens": 2, "values": 0, "blocks": 1}),
            ..Default::default()
        };
        assert_eq!(b.credentials_removed(), 4);
    }
}
