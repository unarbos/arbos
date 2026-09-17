//! The mechanism line: what is wrong and what change fixes it, stated by
//! the agent beside its first edit of a task, when it states one.
//!
//! History, so nobody re-derives the gate from the notes of cycles 3–7.
//! SWE-bench loop, cycle 3: asked for in prose, the agent wrote the line
//! before its first edit in 3 of 50 rollouts; six "right file, wrong fix"
//! losses did not move. Cycle 4: the kernel *refused* the first `edit`,
//! `write`, or `apply_patch` without a line at least `MIN_LEN` long —
//! every rollout then stated one, and no outcome moved. Cycle 13: a
//! rollout satisfied the gate with the literal text `placeholder`, and
//! the twelve honest failures of that cycle held no "wrong mechanism" at
//! all — the class the gate was built for was an artefact of contaminated
//! rollouts that had fetched the upstream fix. A gate any long-enough
//! string passes is worse than none, because its passing is recorded as
//! evidence. So the gate is gone: the line is optional, recorded when
//! given, echoed in the tool result, shown by `changes`, and re-read in
//! the done-criterion pass. Nothing refuses an edit for its absence, and
//! nothing about it is evidence of anything but that the agent said it.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use arbos_core::{AgentId, Layout, Place};
use serde_json::Value;

/// The argument name on the write tools.
pub const ARG: &str = "mechanism";
/// Tools that carry the line.
pub const GATED: &[&str] = &["edit", "write", "apply_patch"];
/// Fewer characters than this is a label, not a statement; it is not
/// recorded (and, since cycle 13, not refused either).
const MIN_LEN: usize = 24;
/// Read for compatibility only: the refusal it turned on is gone
/// (cycle 13). A harness that still sets it changes nothing.
pub const REQUIRED_ENV: &str = "ARBOS_MECHANISM_REQUIRED";

fn path(place: &Place, agent: &AgentId) -> PathBuf {
    Layout::new(place, agent.as_str()).dir.join("mechanism.md")
}

/// A user message starts a new task: forget the previous line. Unlinking
/// needs write permission on the folder; when that fails (a folder gone
/// read-only mid-session, qal-j22's shape), the file itself is emptied,
/// which needs only the file — so the last task's line is not shown for
/// this one by `changes`. Both failing is said on stderr, once per call.
pub fn reset(place: &Place, agent: &AgentId) {
    let p = path(place, agent);
    match std::fs::remove_file(&p) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(unlink) => {
            if let Err(write) = std::fs::write(&p, "") {
                eprintln!(
                    "mechanism {}: the last task's line could not be forgotten (unlink: {unlink}; empty: {write}); `changes` may show it for this task",
                    agent.as_str()
                );
            }
        }
    }
}

/// The task a line belongs to: the transcript line of the user wake that
/// began it (a task runs across the turns that follow — done wakes,
/// steers — until the user's next words). The record carries it as its
/// first line, `task:<n>`, and a reader compares before believing the
/// text: a line from an earlier task that could not be forgotten (a
/// read-only folder, #503) is not this task's.
fn task_start(agent_dir: &Path) -> Option<u64> {
    let events = arbos_core::load_transcript(&agent_dir.join("transcript.jsonl")).ok()?;
    events
        .iter()
        .rposition(
            |e| matches!(&e.kind, arbos_core::EventKind::Wake { wake, .. } if wake == "user"),
        )
        .map(|i| i as u64 + 1)
}

/// The text of a record, when its task is the current one. A record
/// without a stamp (from before the stamp) is believed, once: the reset on
/// the next user message removes or empties it either way.
fn read_for_task(file: &Path, agent_dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(file).ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    match raw.strip_prefix("task:") {
        Some(rest) => {
            let (stamp, text) = rest.split_once('\n')?;
            let stamped: u64 = stamp.trim().parse().ok()?;
            if task_start(agent_dir) != Some(stamped) {
                return None;
            }
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_string())
        }
        None => Some(raw.to_string()),
    }
}

/// The line recorded for the current task, if any.
pub fn current(place: &Place, agent: &AgentId) -> Option<String> {
    let dir = Layout::new(place, agent.as_str()).dir;
    read_for_task(&path(place, agent), &dir)
}

/// Same, by agent folder (for `changes`, which has the cwd and place only).
pub fn current_in(agent_dir: &Path) -> Option<String> {
    read_for_task(&agent_dir.join("mechanism.md"), agent_dir)
}

/// Before a write tool runs: record the `mechanism` line when the call
/// carries one worth the name (the newest line wins). `Ok(Some(line))` =
/// recorded, echo it; `Ok(None)` = nothing to record. Never an error:
/// the refusal is gone (see the module doc).
pub fn gate(place: &Place, agent: &AgentId, tool: &str, args: &Value) -> Result<Option<String>> {
    if !GATED.contains(&tool) {
        return Ok(None);
    }
    let given = args
        .get(ARG)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let file = path(place, agent);
    let agent_dir = Layout::new(place, agent.as_str()).dir;
    let record = |line: &str| -> Result<Option<String>> {
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // Stamped with the task it belongs to, so a reader in a later task
        // does not believe it (see `task_start`).
        let stamp = task_start(&agent_dir)
            .map(|n| format!("task:{n}\n"))
            .unwrap_or_default();
        std::fs::write(&file, format!("{stamp}{line}\n"))?;
        Ok(Some(line.to_string()))
    };
    match given {
        Some(line) if line.len() >= MIN_LEN => record(line),
        // The gate, back for the A/B only (`ARBOS_MECHANISM_REQUIRED=1`):
        // the first edit of a task without a line is refused as before
        // #399. Off by default; the loop measures whether being made to
        // state a mechanism was worth solves, which "satisfiable by
        // `placeholder`" did not establish (16/24 → 9/24 across the
        // bases that include #399).
        Some(short) if required() && !file.exists() => bail!(
            "{tool} refused: mechanism is too short ({short:?}). One full line: what is wrong (the code path that produces the wrong value, and why) and what change fixes it. Then check it against every symptom the request names before you edit."
        ),
        None if required() && !file.exists() => bail!(
            "{tool} refused: the first edit of a task needs mechanism. Add mechanism: one line, what is wrong (the code path that produces the wrong value, and why) and what change fixes it. Check that line against every symptom the request names (each example, error message, edge); a mechanism that explains one symptom but not another is the wrong one, even in the right file. Then repeat this call with mechanism set."
        ),
        _ => Ok(None),
    }
}

/// `ARBOS_MECHANISM_REQUIRED=1`: the pre-#399 gate, for measuring it.
pub fn required() -> bool {
    std::env::var(REQUIRED_ENV).is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

/// The JSON-schema property the gated tools add.
pub fn schema_property() -> Value {
    serde_json::json!({
        "type": "string",
        "description": if required() {
            "Required on the first edit of a task, optional after: one line — what is wrong (the code path that produces the wrong value, and why) and what change fixes it. Checked against every symptom the request names."
        } else {
            "On the first edit of a task, one line: what is wrong (the code path that produces the wrong value, and why) and what change fixes it. Recorded beside the task and shown by changes; not checked — state it anyway, before the edit."
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place() -> (Place, AgentId) {
        let dir = std::env::temp_dir().join(format!("arbos-mech-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (Place::new(dir), AgentId::new("root"))
    }

    /// qal-j22's shape on this record: the agent folder went read-only,
    /// so the last task's line could not be unlinked. Emptying the file
    /// needs only the file, and `current` reads nothing.
    #[cfg(unix)]
    #[test]
    fn a_line_that_cannot_be_unlinked_is_emptied_so_the_next_task_does_not_show_it() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let dir = std::env::temp_dir().join(format!("arbos-mech-ro-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let place = Place::new(dir.clone());
        let agent = AgentId::new("root");
        let file = path(&place, &agent);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            &file,
            "the loop skips the last group because of an off-by-one\n",
        )
        .unwrap();
        assert!(current(&place, &agent).is_some());
        let folder = file.parent().unwrap().to_path_buf();
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
        assert!(std::fs::remove_file(&file).is_err(), "the fault is staged");
        reset(&place, &agent);
        assert_eq!(
            current(&place, &agent),
            None,
            "the old line is not believed"
        );
        assert!(
            file.exists(),
            "emptied, not removed: the folder forbade that"
        );
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The durable form of #503: a line that could not be forgotten (a
    /// read-only folder, a failed unlink and a failed empty) is still not
    /// believed once the user's next message has started another task —
    /// the record names its task, and the reader checks.
    #[test]
    fn a_line_from_an_earlier_task_is_not_believed_even_when_it_could_not_be_forgotten() {
        use arbos_core::{Event, EventKind, append_event};
        let dir = std::env::temp_dir().join(format!("arbos-mech-task-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let place = Place::new(dir.clone());
        let agent = AgentId::new("root");
        let agent_dir = Layout::new(&place, "root").dir;
        std::fs::create_dir_all(&agent_dir).unwrap();
        let transcript = agent_dir.join("transcript.jsonl");
        let user_wake = || {
            Event::new(EventKind::Wake {
                wake: "user".into(),
                text: Some("fix it".into()),
                brief: None,
            })
        };
        append_event(&transcript, &user_wake()).unwrap();
        let args = serde_json::json!({"path": "a.py", "mechanism": "the loop skips the last group because the bound is off by one"});
        assert!(gate(&place, &agent, "edit", &args).unwrap().is_some());
        let file = path(&place, &agent);
        assert!(
            std::fs::read_to_string(&file)
                .unwrap()
                .starts_with("task:1\n")
        );
        assert!(
            current(&place, &agent).is_some(),
            "this task's line is read"
        );
        assert!(current_in(&agent_dir).is_some());
        // A done wake in between is the same task.
        append_event(
            &transcript,
            &Event::new(EventKind::Wake {
                wake: "done".into(),
                text: None,
                brief: None,
            }),
        )
        .unwrap();
        assert!(
            current(&place, &agent).is_some(),
            "a done wake does not end the task"
        );
        // The user's next words start another task; the file stays as if
        // reset had failed. Not believed.
        append_event(&transcript, &user_wake()).unwrap();
        assert_eq!(current(&place, &agent), None, "an earlier task's line");
        assert_eq!(current_in(&agent_dir), None);
        // A record from before the stamp existed is read as before.
        std::fs::write(&file, "an unstamped line from an older kernel\n").unwrap();
        assert!(current(&place, &agent).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// No refusal for any shape by default: an edit without a line, with
    /// a label, with `placeholder` — none is evidence and none is a gate.
    /// A real line is recorded and the newest wins. With
    /// `ARBOS_MECHANISM_REQUIRED=1` the pre-#399 gate is back, for the
    /// A/B: the first edit without a line is refused, a label is too
    /// short, a real line passes and later edits are free.
    #[test]
    fn the_line_is_recorded_when_given_and_nothing_is_refused() {
        // SAFETY: test-local; set and unset within this test.
        unsafe { std::env::remove_var(REQUIRED_ENV) };
        {
            let (place, agent) = place();
            unsafe { std::env::set_var(REQUIRED_ENV, "1") };
            let none = serde_json::json!({"path": "a.py"});
            let err = gate(&place, &agent, "edit", &none).unwrap_err().to_string();
            assert!(err.contains("needs mechanism"), "{err}");
            let label = serde_json::json!({"path": "a.py", "mechanism": "placeholder"});
            let err = gate(&place, &agent, "edit", &label)
                .unwrap_err()
                .to_string();
            assert!(err.contains("too short"), "{err}");
            let ok = serde_json::json!({"path": "a.py", "mechanism": "set_cmap stores cmap.name, not the registered name; use the given name string"});
            assert!(gate(&place, &agent, "edit", &ok).unwrap().is_some());
            // Later edits of the task are not gated.
            assert!(gate(&place, &agent, "edit", &none).unwrap().is_none());
            unsafe { std::env::remove_var(REQUIRED_ENV) };
            let _ = std::fs::remove_dir_all(&place.path);
        }
        let (place, agent) = place();
        let none = serde_json::json!({"path": "a.py"});
        assert!(gate(&place, &agent, "edit", &none).unwrap().is_none());
        let label = serde_json::json!({"path": "a.py", "mechanism": "placeholder"});
        assert!(gate(&place, &agent, "edit", &label).unwrap().is_none());
        assert!(current(&place, &agent).is_none(), "a label is not recorded");
        let ok = serde_json::json!({"path": "a.py", "mechanism": "set_cmap stores cmap.name, not the registered name; use the given name string"});
        let echoed = gate(&place, &agent, "edit", &ok).unwrap().unwrap();
        assert!(echoed.starts_with("set_cmap stores"));
        assert_eq!(current(&place, &agent).unwrap(), echoed);
        let later = serde_json::json!({"path": "b.py", "mechanism": "the registry keys on the object, not its name; key on name"});
        let echoed = gate(&place, &agent, "write", &later).unwrap().unwrap();
        assert_eq!(
            current(&place, &agent).unwrap(),
            echoed,
            "the newest line wins"
        );
        assert!(gate(&place, &agent, "read", &ok).unwrap().is_none());
        reset(&place, &agent);
        assert!(current(&place, &agent).is_none());
        let _ = std::fs::remove_dir_all(&place.path);
    }
}
