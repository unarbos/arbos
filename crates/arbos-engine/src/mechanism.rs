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

use anyhow::Result;
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

/// A user message starts a new task: forget the previous line.
pub fn reset(place: &Place, agent: &AgentId) {
    let _ = std::fs::remove_file(path(place, agent));
}

/// The line recorded for the current task, if any.
pub fn current(place: &Place, agent: &AgentId) -> Option<String> {
    std::fs::read_to_string(path(place, agent))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Same, by agent folder (for `changes`, which has the cwd and place only).
pub fn current_in(agent_dir: &Path) -> Option<String> {
    std::fs::read_to_string(agent_dir.join("mechanism.md"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Before a write tool runs: record the `mechanism` line when the call
/// carries one worth the name (the newest line wins). `Ok(Some(line))` =
/// recorded, echo it; `Ok(None)` = nothing to record. Never an error:
/// the refusal is gone (see the module doc).
pub fn gate(place: &Place, agent: &AgentId, tool: &str, args: &Value) -> Result<Option<String>> {
    if !GATED.contains(&tool) {
        return Ok(None);
    }
    let Some(line) = args
        .get(ARG)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| s.len() >= MIN_LEN)
    else {
        return Ok(None);
    };
    let file = path(place, agent);
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&file, format!("{line}\n"))?;
    Ok(Some(line.to_string()))
}

/// The JSON-schema property the gated tools add.
pub fn schema_property() -> Value {
    serde_json::json!({
        "type": "string",
        "description": "Optional, one line: what is wrong (the code path that produces the wrong value, and why) and what change fixes it. Recorded beside the task and shown by changes; not checked."
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

    /// No refusal for any shape: an edit without a line, with a label,
    /// with `placeholder` — none is evidence and none is a gate. A real
    /// line is recorded and the newest wins.
    #[test]
    fn the_line_is_recorded_when_given_and_nothing_is_refused() {
        // SAFETY: test-local; the variable is read for compatibility only.
        unsafe { std::env::set_var(REQUIRED_ENV, "1") };
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
