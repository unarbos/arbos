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

/// The report itself, and the picture beside it as base64 — the door it is
/// read back through serves text. The poller keys on `report.json`, so it is
/// written and delivered last.
pub const REPORT_NAME: &str = "report.json";
pub const SCREENSHOT_NAME: &str = "screenshot.b64";

/// The kernel serves one file at most 1 MiB (`files::READ_CAP`), and the
/// poller reads the picture as base64 through that door, so a picture past
/// this arrives cut — and a cut base64 string is a broken image, which is
/// worse than a small one.
pub const SHOT_MAX_BASE64: usize = 900 * 1024;

/// How much of the desktop's own state a report carries.
///
/// Jacob's ruling: the bundle holds what is needed to debug it, his app's
/// internals included. A place's session records hold whole chats, so this
/// bounds them — and what will not fit is counted and named rather than
/// dropped in silence, which is the fault this feature has spent a day
/// removing.
pub const DESKTOP_STATE_BUDGET: usize = 128 * 1024;

/// A picture is scaled to this width before it is encoded.
///
/// A bug report does not need Retina pixels; it needs to show what the window
/// looked like. `screencapture -l` of a 1440×900 Retina window is 2880×1800,
/// which as PNG is comfortably past the cap — so before this, every report
/// from Jacob's Mac would have carried no picture at all (F-101, found by
/// driving the sheet on the rig).
pub const SHOT_WIDTH: u32 = 1440;

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
    /// The place's settings that decide behaviour (permission, caps, the
    /// window pin, spend, whether a key is in reach) — the kernel's.
    pub place: Value,
    /// Every agent of the place at the moment of the report, with its
    /// live state and inbox — the roster the reader needs for "the worker
    /// line says Starting forever".
    pub agents: Vec<Value>,
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
    /// The capture is the whole display, not the Arbos window alone. True only
    /// where the window could not be photographed on its own — a Linux desktop
    /// with no way to name the window.
    ///
    /// It matters because his other windows are then in the picture, and the
    /// design's own rule is that they are not the report. So it is never sent
    /// quietly: the sheet says so, and one click drops it.
    pub whole_screen: bool,
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
    /// This report's contents were made up — by the on-demand writer below, or
    /// by a rig exercising the loop — and it must never be diagnosed from or
    /// quoted as something Jacob saw.
    ///
    /// It exists because `written_by` is not enough. Both smoke reports in the
    /// Project store went through this same real writer, so they carry every
    /// mark of being genuine; one of them pairs a *real* transcript with an
    /// invented complaint, which is the worst case — a reader diagnoses a
    /// complaint the events were never about. A fixture has to say so itself.
    pub fixture: bool,
    /// Why there is no trajectory, when the kernel would not give one.
    ///
    /// Without it the report would say `included.trajectory: true` with no
    /// events in it — a lie the other way round from the one `chose` exists to
    /// stop. He kept the part; the kernel did not supply it, and a reader has
    /// to be told which.
    pub trajectory_unavailable: Option<String>,
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
        let log_present = !b.log.is_empty();
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
            // What made this report. Anything without it did not come from the
            // app, and the poller says so — a hand-made fixture sitting beside
            // real reports would otherwise be read as one of Jacob's, and
            // invented events are worse than no events.
            "written_by": "arbos-desktop",
            // Loud, and first: anything reading this file raw sees it before
            // the words.
            "fixture": self.fixture,
            "id": id,
            "sent_ms": sent_ms,
            "note": self.note.trim(),
            "app": self.app,
            "kernel": b.kernel,
            "place": b.place,
            "agent": b.agent,
            "turn": b.turn,
            "events": events,
            "tail": tail,
            "children": children,
            "agents": if self.parts.trajectory { b.agents } else { vec![] },
            "log": if self.parts.log { b.log } else { vec![] },
            // The window's own state: the rows it draws and the facts behind
            // them, the session records on disk, the tabs and the focus. The
            // kernel's roster and agent list are in `place` and `agents` above,
            // so the two sides can be set against each other — which is the
            // only way a report can show a row the kernel has no agent for.
            "session": if self.parts.session {
                self.session.clone().unwrap_or(Value::Null)
            } else {
                Value::Null
            },
            "redacted": b.redacted,
            "tool_io_stripped": stripped,
            "truncated": b.truncated,
            // What is actually in this report, true or false. A part that is
            // merely absent would leave a loop guessing.
            "included": {
                "screenshot": self.parts.screenshot && self.shot.is_some(),
                // Kept *and* present. A kernel too old for the `feedback` frame
                // supplies nothing, and a report claiming a trajectory it does
                // not carry sends a reader hunting events that never existed.
                "trajectory": self.parts.trajectory && !events.is_empty(),
                "log": self.parts.log && log_present,
                "tail": self.parts.tail && self.parts.trajectory && !tail.is_empty(),
                "session": self.parts.session && self.session.is_some(),
                "tool_io": self.parts.tool_io,
            },
            // And what he *chose*, which is not the same thing. A part he kept
            // that is missing anyway is a fault — the capture failed, the
            // kernel had nothing — and a part he removed is a choice. Read
            // together these two tell them apart; `included` alone cannot, and
            // reading it alone had the poller printing a failed screenshot as
            // "he removed it", inverting the one distinction the sheet exists
            // to make.
            "chose": {
                "screenshot": self.parts.screenshot,
                "trajectory": self.parts.trajectory,
                "log": self.parts.log,
                "tail": self.parts.tail && self.parts.trajectory,
                "session": self.parts.session,
                "tool_io": self.parts.tool_io,
            },
            "screenshot_error": self.shot_error,
            "trajectory_unavailable": self.trajectory_unavailable,
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

/// What the window knows, set against what the kernel knows.
///
/// This does the comparison rather than leaving it to a reader, because the
/// comparison *is* the diagnosis. F-137 was a `Delegate 1 · Working` row for an
/// agent the kernel had never heard of, and finding that out meant fetching the
/// window's state from Jacob's machine by hand. A row the kernel has no agent
/// for is now named as such, here, in the sheet he is looking at and in the
/// report an agent reads.
pub fn session_lines(session: &Value, kernel_agents: &[Value]) -> Vec<String> {
    if session.is_null() {
        return vec!["(the app kept no view of this place)".into()];
    }
    let known: Vec<&str> = kernel_agents
        .iter()
        .filter_map(|a| a.get("id").and_then(Value::as_str))
        .collect();

    let mut out = Vec::new();
    let rows = session.get("rows").and_then(Value::as_array);
    if let Some(rows) = rows {
        out.push(format!(
            "{} drawn in this place; the kernel has {}",
            plural(rows.len(), "row"),
            plural(known.len(), "agent")
        ));
        for row in rows.iter().take(40) {
            let s = |k: &str| row.get(k).and_then(Value::as_str).unwrap_or("");
            let label = s("label");
            let agent = s("agent");
            let mut line = format!("{label} — agent {}", if agent.is_empty() { "(none)" } else { agent });
            if row.get("streaming").and_then(Value::as_bool) == Some(true) {
                line.push_str(", streaming");
            }
            if let Some(n) = row.get("live_work").and_then(Value::as_u64).filter(|n| *n > 0) {
                line.push_str(&format!(", {} running", plural(n as usize, "job")));
            }
            if let Some(status) = row.get("status").and_then(Value::as_str) {
                line.push_str(&format!(", says \"{}\"", clip(status, 40)));
            }
            // The disagreement, called by its name.
            if !agent.is_empty() && !known.is_empty() && !known.contains(&agent) {
                line.push_str("  ← THE KERNEL HAS NO AGENT BY THIS NAME");
            }
            out.push(line);
        }
    }
    if let Some(records) = session.get("records").and_then(Value::as_array) {
        out.push(format!("{} on disk", plural(records.len(), "session record")));
        if let Some(note) = session.get("records_note").and_then(Value::as_str) {
            out.push(note.to_string());
        }
    }
    if let Some(items) = session
        .get("drawn")
        .and_then(|d| d.get("items"))
        .and_then(Value::as_array)
    {
        out.push(format!("{} drawn in the chat in front", plural(items.len(), "item")));
    }
    if out.is_empty() {
        out.push("(the app kept no view of this place)".into());
    }
    out
}

fn plural(n: usize, one: &str) -> String {
    if n == 1 {
        format!("{n} {one}")
    } else {
        format!("{n} {one}s")
    }
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
        &dir.join(REPORT_NAME),
        (serde_json::to_string_pretty(&report)? + "\n").as_bytes(),
    )?;

    // The picture travels as base64 in its own file, because the door it is
    // read through serves text. Its own file rather than a field inside the
    // report, so neither one pushes the other past the read cap.
    if draft.parts.screenshot {
        if let Some(shot) = &draft.shot {
            let text = encode_shot(shot);
            write_atomic(&dir.join(SCREENSHOT_NAME), text.as_bytes())?;
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
    // And note the place, so this report is retried whether or not the project
    // is still open. He closes a project because the thing he reported is over.
    remember_outbox(place);
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

// --------------------------------------------------------------------------
// Delivery
//
// A report is written to his own disk first and delivered from there, so
// pressing Send never depends on the network. This is the part that carries it
// the rest of the way: into a store on a machine an agent can read, using the
// federated write that already exists.
//
// It goes through `arbos-kernel store put`, the binary the app already ships
// beside itself. That is not laziness: it means one authenticated path to the
// hub rather than two, the hub address and token come from the same
// `hub.toml` the kernel already reads, and this app holds no credential of its
// own. A second hub client in the desktop would be a second thing to get
// wrong.
// --------------------------------------------------------------------------

/// Waits between attempts on a report that will not go. Doubling, to an hour:
/// a report is not urgent to the minute, and a machine that is offline for a
/// morning should not spend the morning retrying.
const BACKOFF_SECS: [u64; 7] = [30, 60, 120, 300, 900, 1800, 3600];

/// Where a report stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// On his disk, waiting for a link or for an address to send it to.
    Waiting {
        attempts: u32,
        last_error: Option<String>,
        /// When the reason above was current. A reason with no time behind it
        /// reads as the present tense whatever its age.
        tried_ms: Option<i64>,
    },
    /// In the store, where the loop can read it.
    Sent { at_ms: i64 },
}

/// Deliver one written report. `base` is the store address its folder is made
/// under; `hub_home` the configuration directory holding the credentials for
/// it. `retrying` says a previous attempt has already run.
///
/// `report.json` goes **first**, with an empty `base_hash`, which the store
/// reads as create-if-absent: it claims the folder, so two reports cannot land
/// in the same one. Then the picture, as base64 text beside it.
pub fn deliver(dir: &Path, base: &str, hub_home: &Path, retrying: bool) -> Result<()> {
    let id = dir
        .file_name()
        .and_then(|n| n.to_str())
        .context("the outbox folder has no name")?;
    let base = base.trim_end_matches('/');
    let kernel = crate::kernel::arbos_bin()?;
    let hub = hub_home.join("arbos").join("hub.toml");
    if !hub.is_file() {
        // Deliberately not a fallback to his own `~/.config/arbos/hub.toml`.
        // That file can hold a token that is owner on every project he has,
        // and a bug report must not be able to write with it. Waiting is the
        // right failure.
        anyhow::bail!(
            "no feedback credentials at {} — a report will not be sent under another token",
            hub.display()
        );
    }

    // Claim the folder. A conflict here on a retry is our own earlier claim,
    // since the id carries this machine's clock and a random tail; carry on to
    // the picture rather than starting a new folder and orphaning the first.
    match put(
        &kernel,
        hub_home,
        &format!("{base}/{id}/{REPORT_NAME}"),
        &dir.join(REPORT_NAME),
        Some(""),
    ) {
        Ok(()) => {}
        Err(e) if retrying && format!("{e:#}").contains("conflict") => {}
        Err(e) => return Err(e),
    }

    // Then the picture, as base64 text, which is the form the poller reads it
    // back in. No base hash: on a retry this should replace what is there.
    let shot = dir.join(SCREENSHOT_NAME);
    if shot.is_file() {
        put(
            &kernel,
            hub_home,
            &format!("{base}/{id}/{SCREENSHOT_NAME}"),
            &shot,
            None,
        )?;
    }
    Ok(())
}

fn put(
    kernel: &Path,
    hub_home: &Path,
    address: &str,
    file: &Path,
    base_hash: Option<&str>,
) -> Result<()> {
    let mut cmd = std::process::Command::new(kernel);
    cmd.args(["store", "put", address])
        .arg(file)
        // `host_dir()` reads this, so the kernel run for a delivery looks for
        // its hub in the feedback directory and nowhere else.
        .env("XDG_CONFIG_HOME", hub_home);
    if let Some(base) = base_hash {
        cmd.args(["--base", base]);
    }
    let out = cmd
        .output()
        .with_context(|| format!("run {} store put", kernel.display()))?;
    if !out.status.success() {
        // The kernel's own words rather than a status code: "no hub
        // configured", "a reader client may not send put", "conflict — the
        // file exists now" and a dropped link all read differently, and this
        // is what the sheet shows him.
        let said = String::from_utf8_lossy(&out.stderr).trim().to_string();
        anyhow::bail!(
            "{}",
            if said.is_empty() {
                format!("store put {address} failed")
            } else {
                said
            }
        );
    }
    Ok(())
}

/// Try every waiting report whose next attempt is due. Returns what happened
/// to each, newest state last, for whoever is drawing the outbox.
///
/// Nothing is deleted on success: the folder stays, holding its receipt, so
/// the loop can write `fixed.json` back into it and the app can tell him which
/// build carries the fix.
pub fn deliver_pending(
    place: &Place,
    base: &str,
    hub_home: &Path,
    now_ms: i64,
) -> Vec<(String, Delivery)> {
    let mut out = Vec::new();
    if base.trim().is_empty() {
        // No address yet. The reports keep, and the sheet says so; this is the
        // same state as being offline and is not an error to report every
        // thirty seconds.
        return out;
    }
    for dir in pending(place) {
        let state = state_of(&dir);
        if let Delivery::Waiting { attempts, .. } = &state
            && !due(&dir, *attempts, now_ms)
        {
            continue;
        }
        let id = dir.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        let retrying = matches!(state, Delivery::Waiting { attempts, .. } if attempts > 0);
        match deliver(&dir, base, hub_home, retrying) {
            Ok(()) => {
                let _ = write_atomic(
                    &dir.join("delivered"),
                    (json!({"at_ms": now_ms, "to": base}).to_string() + "\n").as_bytes(),
                );
                out.push((id, Delivery::Sent { at_ms: now_ms }));
            }
            Err(e) => {
                let attempts = match &state {
                    Delivery::Waiting { attempts, .. } => attempts + 1,
                    Delivery::Sent { .. } => 1,
                };
                // Only the current reason, and the moment it was current. The
                // old text used to sit on a report that nobody had retried
                // since, so a cause that had been fixed still read as "still
                // broken" — `tried_ms` is what lets a reader tell a live
                // failure from a stale one.
                let last_error = format!("{e:#}");
                let _ = write_atomic(
                    &dir.join("attempts.json"),
                    (json!({
                        "attempts": attempts,
                        "at_ms": now_ms,
                        "tried_ms": now_ms,
                        "error": last_error,
                    })
                    .to_string()
                        + "\n")
                        .as_bytes(),
                );
                out.push((
                    id,
                    Delivery::Waiting {
                        attempts,
                        last_error: Some(last_error),
                        tried_ms: Some(now_ms),
                    },
                ));
            }
        }
    }
    out
}

fn state_of(dir: &Path) -> Delivery {
    if let Ok(text) = std::fs::read_to_string(dir.join("delivered"))
        && let Ok(v) = serde_json::from_str::<Value>(&text)
    {
        return Delivery::Sent {
            at_ms: v.get("at_ms").and_then(Value::as_i64).unwrap_or(0),
        };
    }
    let (attempts, last_error, tried_ms) = std::fs::read_to_string(dir.join("attempts.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .map(|v| {
            (
                v.get("attempts").and_then(Value::as_u64).unwrap_or(0) as u32,
                v.get("error").and_then(Value::as_str).map(str::to_string),
                v.get("tried_ms")
                    .or_else(|| v.get("at_ms"))
                    .and_then(Value::as_i64),
            )
        })
        .unwrap_or((0, None, None));
    Delivery::Waiting {
        attempts,
        last_error,
        tried_ms,
    }
}

/// Whether a report that has failed `attempts` times may be tried again yet.
fn due(dir: &Path, attempts: u32, now_ms: i64) -> bool {
    if attempts == 0 {
        return true;
    }
    let last = std::fs::read_to_string(dir.join("attempts.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("at_ms").and_then(Value::as_i64))
        .unwrap_or(0);
    let wait = BACKOFF_SECS[(attempts as usize - 1).min(BACKOFF_SECS.len() - 1)] as i64 * 1000;
    now_ms - last >= wait
}

// --------------------------------------------------------------------------
// Which outboxes exist
//
// A report must not depend on Jacob keeping a tab open. He closes a project
// *because* the thing he was reporting is over — and two of his reports sat at
// attempt three with a stale reason for exactly that: the sweep walked the list
// of open places, so a closed project's outbox was never looked at again.
//
// So the outboxes are remembered on the machine, once, when a report is
// written. The drain reads that list and never asks what is on screen.
// --------------------------------------------------------------------------

fn registry_path() -> Option<PathBuf> {
    crate::model::settings::data_dir()
        .ok()
        .map(|dir| dir.join("feedback-outboxes.json"))
}

/// Note that this place holds reports. Idempotent.
pub fn remember_outbox(place: &Place) {
    let Some(path) = registry_path() else { return };
    let mut places = read_registry(&path);
    let here = place.path().to_string_lossy().into_owned();
    if places.iter().any(|p| p == &here) {
        return;
    }
    places.push(here);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, json!({"places": places}).to_string() + "\n");
}

fn read_registry(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| {
            v.get("places").and_then(Value::as_array).map(|a| {
                a.iter()
                    .filter_map(|p| p.as_str().map(str::to_string))
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// Every place known to hold an outbox, open or not.
///
/// A place whose outbox folder has gone — the project deleted, the store
/// cleared — is dropped from the list as it is read, so this cannot grow for
/// ever from folders that no longer exist.
pub fn known_outboxes() -> Vec<Place> {
    let Some(path) = registry_path() else {
        return vec![];
    };
    let all = read_registry(&path);
    let (alive, gone): (Vec<String>, Vec<String>) = all
        .iter()
        .cloned()
        .partition(|p| outbox(&Place::new(p)).is_dir());
    if !gone.is_empty() {
        let _ = std::fs::write(&path, json!({"places": alive}).to_string() + "\n");
    }
    alive.into_iter().map(Place::new).collect()
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
    let whole_screen = grab(width, height, &path)?;
    let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    let _ = std::fs::remove_file(&path);
    if bytes.is_empty() {
        anyhow::bail!("the capture wrote nothing");
    }
    fit_for_sending(&bytes, whole_screen)
}

/// Put a picture of the window at `path`. `Ok(true)` means the whole display
/// was taken because the window alone could not be.
#[cfg(target_os = "macos")]
fn grab(width: f32, height: f32, path: &Path) -> Result<bool> {
    let id = crate::driver::ns_window_number(width, height)
        .context("could not find this window to photograph")?;
    crate::driver::capture_window(id, path)?;
    Ok(false)
}

/// X11 and Wayland have no AppKit window number, so the window is named
/// instead. `import -window <title>` takes ours alone; failing that the id
/// from `xdotool`; failing both, the whole display, which the caller is told
/// about rather than left to assume.
#[cfg(not(target_os = "macos"))]
fn grab(_width: f32, _height: f32, path: &Path) -> Result<bool> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        let _ = std::fs::create_dir_all(dir);
    }
    let wrote = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0) > 0;
    let by_name = std::process::Command::new("import")
        .args(["-window", crate::driver::WINDOW_TITLE])
        .arg(path)
        .status();
    if by_name.map(|s| s.success()).unwrap_or(false) && wrote(path) {
        return Ok(false);
    }
    if let Ok(out) = std::process::Command::new("xdotool")
        .args(["search", "--pid", &std::process::id().to_string()])
        .output()
        && let Some(id) = String::from_utf8_lossy(&out.stdout).lines().next()
        && std::process::Command::new("import")
            .args(["-window", id.trim()])
            .arg(path)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        && wrote(path)
    {
        return Ok(false);
    }
    // The rig's own path, and the last resort on a desktop: everything on the
    // screen. Honest rather than silent — his other windows are in it.
    crate::driver::capture_window(0, path)?;
    Ok(true)
}

/// Scale and encode a capture so it fits through the door it has to go
/// through, rather than being refused at it.
///
/// A window on a Retina Mac is 2880×1800 and its PNG is well past the cap, so
/// refusing meant no report from Jacob ever carried a picture. Scaled to
/// [`SHOT_WIDTH`] and JPEG-encoded it is a few hundred kilobytes and still
/// shows what he was looking at. Quality steps down, and then the width, until
/// it fits; a report is worth more than a sharp picture.
pub fn fit_for_sending(bytes: &[u8], whole_screen: bool) -> Result<Shot> {
    let image = image::load_from_memory(bytes).context("read the capture back")?;
    for width in [SHOT_WIDTH, SHOT_WIDTH / 2, SHOT_WIDTH / 3] {
        let scaled = if image.width() > width {
            image.resize(
                width,
                u32::MAX,
                image::imageops::FilterType::CatmullRom,
            )
        } else {
            image.clone()
        };
        for quality in [82u8, 70, 55, 40] {
            let mut out = Vec::new();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
                .encode_image(&scaled)
                .context("encode the capture")?;
            let shot = Shot {
                bytes: out,
                width: scaled.width(),
                height: scaled.height(),
                mime: "image/jpeg".into(),
                whole_screen,
            };
            if !shot_too_big(&shot) {
                return Ok(shot);
            }
        }
    }
    anyhow::bail!(
        "the capture will not fit in {} bytes even at {}px",
        SHOT_MAX_BASE64,
        SHOT_WIDTH / 3
    )
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

    /// The bug the rig found: a Retina window's capture is 2880×1800, its PNG
    /// is well past the cap, and the sheet used to refuse it — so no report
    /// from Jacob's Mac would ever have carried a picture (F-101). It has to
    /// be made to fit, not turned away at the door.
    #[test]
    fn a_retina_window_is_scaled_to_fit_rather_than_refused() {
        // Noise, not flat colour: a flat image compresses to nothing and would
        // pass this test without proving anything.
        let (w, h) = (2880u32, 1800u32);
        let mut raw = image::RgbImage::new(w, h);
        let mut seed = 0x2545F491u32;
        for pixel in raw.pixels_mut() {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let b = seed.to_le_bytes();
            *pixel = image::Rgb([b[0], b[1], b[2]]);
        }
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(raw)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        assert!(
            png.len().saturating_mul(4).div_ceil(3) > SHOT_MAX_BASE64,
            "the fixture is past the cap to begin with, or it proves nothing: {}",
            png.len()
        );

        let shot = fit_for_sending(&png, false).expect("a window capture always fits");
        assert!(!shot_too_big(&shot), "{} bytes", shot.bytes.len());
        assert_eq!(shot.mime, "image/jpeg");
        assert_eq!(shot.width, SHOT_WIDTH, "scaled to a readable width");
        assert_eq!(shot.height, SHOT_WIDTH * h / w, "aspect kept");
        assert!(
            shot.bytes.starts_with(&[0xff, 0xd8, 0xff]),
            "a real JPEG comes out"
        );
        // And it is still an image afterwards, not a truncated buffer.
        let back = image::load_from_memory(&shot.bytes).expect("decodes again");
        assert_eq!(back.width(), SHOT_WIDTH);

        // A whole-screen capture keeps saying so through the scaling, since
        // that is what the sheet warns him about.
        assert!(fit_for_sending(&png, true).unwrap().whole_screen);
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
            session_lines(&Value::Null, &[])[0],
            "(the app kept no view of this place)"
        );
    }

    /// The report has to answer F-137 by itself: a row the window draws for an
    /// agent the kernel has never heard of. A report carrying one side cannot
    /// show a disagreement, so both sides travel and the comparison is done
    /// here rather than left to whoever reads it.
    #[test]
    fn a_row_the_kernel_has_no_agent_for_is_named_as_such() {
        let state = json!({
            "rows": [
                {"label": "Main", "agent": "root", "streaming": false, "live_work": 0},
                {"label": "Delegate 1", "agent": "ghost-7", "streaming": true, "live_work": 1,
                 "status": "Working"},
            ],
            "records": [{"agent": "ghost-7", "delegate_number": 1}],
            "records_note": "every session record is here",
        });
        let kernel = vec![json!({"id": "root", "running": false})];
        let lines = session_lines(&state, &kernel);

        assert_eq!(lines[0], "2 rows drawn in this place; the kernel has 1 agent");
        assert!(!lines[1].contains("NO AGENT"), "root is known: {}", lines[1]);
        assert!(
            lines[2].contains("THE KERNEL HAS NO AGENT BY THIS NAME"),
            "the phantom row is named: {}",
            lines[2]
        );
        assert!(lines[2].contains("Delegate 1") && lines[2].contains("ghost-7"));
        assert!(lines[2].contains("streaming") && lines[2].contains("1 job running"));
        assert!(lines[2].contains(r#"says "Working""#), "{}", lines[2]);
        assert!(lines.iter().any(|l| l == "1 session record on disk"));

        // With no kernel list at all nothing is accused: an empty list means the
        // kernel did not answer, not that every row is a phantom.
        let quiet = session_lines(&state, &[]);
        assert!(
            !quiet.iter().any(|l| l.contains("NO AGENT")),
            "no kernel side, no accusation: {quiet:?}"
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

    /// A throwaway folder that clears up after itself, so these tests owe no
    /// dependency for one directory.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            // A counter, not just the clock: tests run in parallel, and two
            // that asked in the same millisecond got the same folder — so one
            // test's cleanup deleted another's report and the delay assertion
            // read a missing `attempts.json` as "never tried". It passed alone
            // and failed in the suite, which is the shape of every flake.
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let at = std::env::temp_dir().join(format!(
                "arbos-feedback-test-{}-{name}-{}-{n}",
                std::process::id(),
                arbos_core::now_ms()
            ));
            std::fs::create_dir_all(&at).unwrap();
            Self(at)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn outbox_with_one_report() -> (Scratch, Place) {
        let dir = Scratch::new("outbox");
        let place = Place::new(dir.path());
        let mut draft = Draft::new(Parts::default());
        draft.note = "it froze".into();
        write(&place, &draft, "20260916T154210Z-aa11", 1_789_573_330_000).unwrap();
        (dir, place)
    }

    /// With nowhere to send a report it must keep, quietly. This is the state
    /// the app ships in until the store address is settled, and it is the same
    /// state as being offline — not an error to shout about every 30 seconds.
    #[test]
    fn with_no_address_a_report_waits_instead_of_failing() {
        let (_dir, place) = outbox_with_one_report();
        assert_eq!(pending(&place).len(), 1);
        assert!(deliver_pending(&place, "", Path::new("/nonexistent"), 0).is_empty());
        assert!(deliver_pending(&place, "   ", Path::new("/nonexistent"), 0).is_empty());
        // And it is still there to send later.
        assert_eq!(pending(&place).len(), 1);
    }

    /// A report that will not go is retried on a widening delay, and its
    /// reason is kept where the sheet can read it.
    #[test]
    fn a_report_that_will_not_go_backs_off_and_keeps_its_reason() {
        let (_dir, place) = outbox_with_one_report();
        let dir = pending(&place).remove(0);

        assert_eq!(
            state_of(&dir),
            Delivery::Waiting { attempts: 0, last_error: None, tried_ms: None }
        );
        assert!(due(&dir, 0, 0), "a fresh report is tried at once");

        // An address the kernel cannot resolve: the attempt fails, and the
        // kernel's own words are what is kept.
        let now = 1_789_573_400_000;
        let out = deliver_pending(&place, "arbos://nowhere/nothing/internal/feedback", Path::new("/nonexistent"), now);
        assert_eq!(out.len(), 1);
        let Delivery::Waiting { attempts, last_error, .. } = &out[0].1 else {
            panic!("a report with no hub cannot have been sent: {:?}", out[0].1);
        };
        assert_eq!(*attempts, 1);
        assert!(last_error.is_some(), "the reason is kept, not swallowed");

        // Thirty seconds is the first wait, so it is not due a second later
        // and is due a minute later.
        assert!(!due(&dir, 1, now + 1_000));
        assert!(due(&dir, 1, now + 31_000));
        // And the waits widen rather than hammering a machine that is away.
        assert!(!due(&dir, 4, now + 299_000));
        assert!(due(&dir, 4, now + 301_000));
        // Past the end of the table it holds at the longest wait.
        assert!(!due(&dir, 99, now + 3_599_000));
        assert!(due(&dir, 99, now + 3_601_000));

        // It is still pending, because nothing marked it delivered.
        assert_eq!(pending(&place).len(), 1);
    }

    /// Every project's outbox is drained, not only the one on screen. A report
    /// he filed in one project while another was in front is still his report,
    /// and the drain that runs without a new Send has to find it.
    #[test]
    fn a_report_waiting_in_another_project_is_still_attempted() {
        let a = Scratch::new("place-a");
        let b = Scratch::new("place-b");
        let (pa, pb) = (Place::new(a.path()), Place::new(b.path()));
        for (n, place) in [(1, &pa), (2, &pb)] {
            let mut draft = Draft::new(Parts::default());
            draft.note = format!("report {n}");
            write(place, &draft, &format!("20260916T15421{n}Z-aa11"), 1_789_573_330_000).unwrap();
        }
        // As the drain does it: every open place, one pass.
        let now = 1_789_573_400_000;
        let tried: Vec<_> = [&pa, &pb]
            .iter()
            .flat_map(|place| {
                deliver_pending(
                    place,
                    "arbos://nowhere/nothing/internal/feedback",
                    Path::new("/nonexistent"),
                    now,
                )
            })
            .collect();
        assert_eq!(tried.len(), 2, "both were tried: {tried:?}");
        for (_, state) in &tried {
            assert!(
                matches!(state, Delivery::Waiting { attempts: 1, last_error: Some(_), .. }),
                "each kept its reason: {state:?}"
            );
        }
        // And both are still there to try again — nothing was dropped for
        // being in the wrong project.
        assert_eq!(pending(&pa).len(), 1);
        assert_eq!(pending(&pb).len(), 1);
    }

    /// A report from a project he has since closed is still retried.
    ///
    /// Two of Jacob's sat at attempt three with a reason that had already been
    /// fixed, because the sweep walked the list of *open* places. He closes a
    /// project because the thing he was reporting is over, so the outbox has to
    /// be findable without it.
    #[test]
    fn a_closed_project_s_outbox_is_still_found() {
        let scratch = Scratch::new("closed");
        let data = scratch.path().join("data");
        // `data_dir` reads this, so the registry lands under the scratch folder
        // rather than this machine's real one.
        let restore = std::env::var("XDG_DATA_HOME").ok();
        unsafe { std::env::set_var("XDG_DATA_HOME", &data) };

        let place = Place::new(scratch.path().join("project"));
        let mut draft = Draft::new(Parts::default());
        draft.note = "it drew the sidebar twice".into();
        write(&place, &draft, "20260916T154210Z-cc33", 1_789_573_330_000).unwrap();

        // Nothing here knows or asks which projects are open.
        let known = known_outboxes();
        assert_eq!(known.len(), 1, "the place was remembered: {known:?}");
        assert_eq!(known[0].path(), place.path());
        assert_eq!(pending(&known[0]).len(), 1, "and its report is there to send");

        // A place whose outbox has gone drops out rather than lingering.
        std::fs::remove_dir_all(outbox(&place)).unwrap();
        assert!(known_outboxes().is_empty(), "a vanished outbox is forgotten");

        match restore {
            Some(v) => unsafe { std::env::set_var("XDG_DATA_HOME", v) },
            None => unsafe { std::env::remove_var("XDG_DATA_HOME") },
        }
    }

    /// A delivered report keeps its folder: the loop writes `fixed.json` back
    /// into it, and the app reads that to tell him which build carries the fix.
    #[test]
    fn a_delivered_report_leaves_the_outbox_but_not_the_disk() {
        let (_dir, place) = outbox_with_one_report();
        let dir = pending(&place).remove(0);
        std::fs::write(dir.join("delivered"), r#"{"at_ms":7,"to":"arbos://x/y/z"}"#).unwrap();
        assert_eq!(state_of(&dir), Delivery::Sent { at_ms: 7 });
        assert!(pending(&place).is_empty(), "not tried again");
        assert!(dir.join(REPORT_NAME).is_file(), "still on disk for the answer");
    }

    /// A half-written report is never picked up: `ready` is the last thing
    /// written, and the outbox only offers folders that have it.
    #[test]
    fn a_half_written_report_is_not_offered_for_delivery() {
        let dir = Scratch::new("half");
        let place = Place::new(dir.path());
        let half = outbox(&place).join("20260916T160000Z-bb22");
        std::fs::create_dir_all(&half).unwrap();
        std::fs::write(half.join(REPORT_NAME), "{}").unwrap();
        assert!(pending(&place).is_empty(), "no `ready`, so not offered");
        std::fs::write(half.join("ready"), "").unwrap();
        assert_eq!(pending(&place).len(), 1);
    }

    /// Not a test of behaviour: writes one report into a folder named by the
    /// environment so the poller can be run against what the app really
    /// produces. Ignored unless asked for by name.
    #[test]
    #[ignore]
    fn write_one_report_for_the_poller() {
        let at = std::env::var("ARBOS_FEEDBACK_PROBE").expect("ARBOS_FEEDBACK_PROBE");
        let place = Place::new(&at);
        let mut draft = Draft::new(Parts::default());
        // Marked at the source. Two reports written by this very function are
        // sitting in the Project store looking genuine, and one of them cost a
        // reader a diagnosis of a complaint its events were never about.
        draft.fixture = true;
        draft.note = format!(
            "FIXTURE, not a real report — made by write_one_report_for_the_poller. \
             Its words and events are invented and do not correspond. \
             (Pretend complaint: the sheet froze when I opened it, and my key {} was on screen.)",
            format!("sk-{}v1-3f8a9b2c4d5e6f7a8b9c0d1e2f3a4b5c", "or-")
        );
        draft.session = Some(json!({"items": [{"User": {}}, {"Assistant": {}}]}));
        draft.shot_error = Some("Screen Recording is not allowed for Arbos".into());
        draft.bundle = Some(Bundle {
            agent: "root".into(),
            turn: json!({"from": 112, "to": 140, "complete": true}),
            events: vec![
                json!({"kind": "wake", "seq": 112, "wake": "user", "text": "add the retry"}),
                json!({"kind": "tool", "seq": 118, "name": "bash",
                       "args": {"command": "cargo test"}, "output": "test result: FAILED",
                       "error": "exit 101", "result_size": 41233}),
                json!({"kind": "assistant", "seq": 139, "text": "Done."}),
            ],
            log: vec![json!({"ts": 1_789_573_301_000i64, "level": "warn", "event": "turn.slow"})],
            kernel: json!({"version": "0.2.0", "git_sha": "abc123def456", "os": "macos",
                           "arch": "aarch64", "provider": "openrouter", "model": "gemini",
                           "project": "subnet120"}),
            redacted: json!({"secrets": 0, "tokens": 1, "values": 0, "blocks": 0}),
            ..Default::default()
        });
        let id = new_id(1_789_573_330_000);
        let dir = write(&place, &draft, &id, 1_789_573_330_000).unwrap();
        println!("WROTE {}", dir.display());
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
