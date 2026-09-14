//! The mechanism line: what is wrong and what change fixes it, stated once
//! before the first edit of a task.
//!
//! SWE-bench loop, cycle 3: asked for in prose, the agent wrote the line
//! before its first edit in 3 of 50 rollouts and in the final summary in
//! the rest; six "right file, wrong fix" losses did not move. So the kernel
//! asks: the first `edit`, `write`, or `apply_patch` after a user message
//! must carry `mechanism`, or it is refused with the reason. The line is
//! kept beside the agent (`mechanism.md`), echoed in the tool result, shown
//! by `changes`, and re-read in the done-criterion pass.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use arbos_core::{AgentId, Layout, Place};
use serde_json::Value;

/// The argument name on the write tools.
pub const ARG: &str = "mechanism";
/// Tools whose first call in a task needs the line.
pub const GATED: &[&str] = &["edit", "write", "apply_patch"];
/// Fewer characters than this is a label, not a mechanism.
const MIN_LEN: usize = 24;
/// Set to `1` to refuse the first edit without a line. Measured on
/// SWE-bench (cycle 4): the refusal made every rollout state a mechanism
/// and moved no outcome, so by default the line is optional and recorded
/// when given; a harness turns the refusal on.
pub const REQUIRED_ENV: &str = "ARBOS_MECHANISM_REQUIRED";

fn required() -> bool {
    std::env::var(REQUIRED_ENV).is_ok_and(|v| v == "1" || v == "true")
}

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

/// Before a gated tool runs. `Ok(Some(line))` = this call recorded the
/// task's mechanism (echo it); `Ok(None)` = nothing to do; `Err` = the
/// first edit of the task came without one, and the call must not run.
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
    if file.exists() {
        // Later edits may restate or refine it; the newest line wins.
        if let Some(line) = given.filter(|l| l.len() >= MIN_LEN) {
            let _ = std::fs::write(&file, format!("{line}\n"));
            return Ok(Some(line.to_string()));
        }
        return Ok(None);
    }
    match given {
        None if !required() => Ok(None),
        Some(short) if !required() && short.len() < MIN_LEN => Ok(None),
        Some(line) if line.len() >= MIN_LEN => {
            if let Some(dir) = file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            std::fs::write(&file, format!("{line}\n"))?;
            Ok(Some(line.to_string()))
        }
        Some(short) => bail!(
            "{tool} refused: mechanism is too short ({short:?}). One full line: what is wrong (the code path that produces the wrong value, and why) and what change fixes it. Then check it against every symptom the request names before you edit."
        ),
        None => bail!(
            "{tool} refused: the first edit of a task needs mechanism. Add mechanism: one line, what is wrong (the code path that produces the wrong value, and why) and what change fixes it. Check that line against every symptom the request names (each example, error message, edge); a mechanism that explains one symptom but not another is the wrong one, even in the right file. Then repeat this call with mechanism set."
        ),
    }
}

/// The JSON-schema property the gated tools add.
pub fn schema_property() -> Value {
    serde_json::json!({
        "type": "string",
        "description": "Required on the first edit of a task, optional after: one line — what is wrong (the code path that produces the wrong value, and why) and what change fixes it. Checked against every symptom the request names."
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

    #[test]
    fn first_edit_needs_a_line_then_later_edits_do_not() {
        // SAFETY: test-local; no other thread reads this variable.
        unsafe { std::env::set_var(REQUIRED_ENV, "1") };
        let (place, agent) = place();
        let none = serde_json::json!({"path": "a.py"});
        let err = gate(&place, &agent, "edit", &none).unwrap_err().to_string();
        assert!(
            err.contains("first edit of a task needs mechanism"),
            "{err}"
        );
        let short = serde_json::json!({"path": "a.py", "mechanism": "fix bug"});
        assert!(gate(&place, &agent, "edit", &short).is_err());
        let ok = serde_json::json!({"path": "a.py", "mechanism": "set_cmap stores cmap.name, not the registered name; use the given name string"});
        let echoed = gate(&place, &agent, "edit", &ok).unwrap().unwrap();
        assert!(echoed.starts_with("set_cmap stores"));
        assert_eq!(current(&place, &agent).unwrap(), echoed);
        assert!(gate(&place, &agent, "write", &none).unwrap().is_none());
        assert!(gate(&place, &agent, "read", &none).unwrap().is_none());
        reset(&place, &agent);
        assert!(current(&place, &agent).is_none());
        assert!(gate(&place, &agent, "apply_patch", &none).is_err());
        let _ = std::fs::remove_dir_all(&place.path);
    }
}
