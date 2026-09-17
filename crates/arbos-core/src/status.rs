//! What an agent is doing right now, in a few words: `agents/<id>/status.toml`.
//!
//! ```toml
//! step = "Reading project context"
//! since = "2026-09-14T09:41:03Z"
//! source = "agent"          # or "derived": the kernel's guess from the tool in flight
//! ```
//!
//! The `status` tool writes it (Cursor's coordinator has the same: a verb
//! phrase, six words or less, updated when the subtask changes); the
//! kernel writes a derived one from the tool that is running when the
//! agent has not said anything this turn, and clears it when the turn
//! ends. Windows draw it as the live line beside the agent's name.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::Place;

pub const FILE: &str = "status.toml";
/// Longest step kept: a line, not a paragraph.
pub const MAX_CHARS: usize = 80;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Status {
    pub step: String,
    #[serde(default)]
    pub since: String,
    /// `agent` (set by the `status` tool) or `derived` (the kernel's guess).
    #[serde(default)]
    pub source: String,
}

pub fn path(place: &Place, agent: &str) -> std::path::PathBuf {
    place.agent_dir(agent).join(FILE)
}

/// The current status, or None when there is none (idle, or never set).
pub fn read(place: &Place, agent: &str) -> Option<Status> {
    let text = std::fs::read_to_string(path(place, agent)).ok()?;
    let s: Status = toml::from_str(&text).ok()?;
    (!s.step.trim().is_empty()).then_some(s)
}

/// Write the status, whole or not at all. `step` is trimmed and capped.
pub fn write(place: &Place, agent: &str, step: &str, source: &str) -> Result<Status> {
    write_since(
        place,
        agent,
        step,
        source,
        &crate::inbox::rfc3339(crate::now_ms()),
    )
}

/// `write` with the clock carried over: a parent's "waiting on <worker>"
/// line keeps the worker's own `since`, so its timer reads the worker's
/// time on the step, not the time since the parent last copied it.
pub fn write_since(
    place: &Place,
    agent: &str,
    step: &str,
    source: &str,
    since: &str,
) -> Result<Status> {
    let status = Status {
        step: clip(step),
        since: since.to_string(),
        source: source.to_string(),
    };
    let p = path(place, agent);
    let text = toml::to_string(&status).context("serialise status")?;
    let tmp = p.with_extension("toml.tmp");
    std::fs::write(&tmp, text).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &p).with_context(|| format!("replace {}", p.display()))?;
    Ok(status)
}

/// The turn is over: nothing is being done.
pub fn clear(place: &Place, agent: &str) -> bool {
    std::fs::remove_file(path(place, agent)).is_ok()
}

/// One line, at most `MAX_CHARS` characters, no trailing period.
pub fn clip(step: &str) -> String {
    let one: String = step.split_whitespace().collect::<Vec<_>>().join(" ");
    let one = one.trim_end_matches(['.', '…']).to_string();
    if one.chars().count() <= MAX_CHARS {
        return one;
    }
    let mut cut: String = one.chars().take(MAX_CHARS - 1).collect();
    cut.push('…');
    cut
}

/// A `status` call the model wrote as prose instead: a reply that is one
/// line, `status: Reading project context`, `status "Setting plan"` (the
/// prompt's own form, which Gemini copies out as text), or
/// `status("Reading the issue")`, any case. Some models announce the
/// step this way before their tool calls; the kernel takes the words as
/// the live line so the window shows the step and not a paragraph — and
/// the line is not written as a reply. Anything longer, or with more
/// lines, is a real reply.
pub fn spoken(text: &str) -> Option<String> {
    let line = text.trim().trim_matches(['*', '`']).trim();
    if line.is_empty() || line.contains('\n') || !line.get(..6)?.eq_ignore_ascii_case("status") {
        return None;
    }
    let rest = line[6..].trim_start();
    // `status:` / `status "…"` / `status '…'` / `status(…)`. A word that
    // merely begins with "status" (statuses, "status of x", "status
    // Reading the folder") is prose and stays a reply.
    let step = if let Some(r) = rest.strip_prefix(':') {
        r
    } else if let Some(r) = rest.strip_prefix('(') {
        r.strip_suffix(')').unwrap_or(r)
    } else if rest.starts_with('"') || rest.starts_with('\'') {
        rest
    } else {
        return None;
    };
    let step = step.trim().trim_matches(['"', '\'', '*', '`']).trim();
    (!step.is_empty() && step.chars().count() <= MAX_CHARS * 2).then(|| clip(step))
}

/// The kernel's guess at what a tool call is doing, for an agent that
/// has not said. Verb phrase, short, from the tool and its arguments.
pub fn derived(tool: &str, args: Option<&serde_json::Value>) -> String {
    let arg = |k: &str| -> Option<String> {
        args?
            .get(k)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let short = |s: &str, n: usize| -> String {
        let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
        if one.chars().count() <= n {
            one
        } else {
            let mut c: String = one.chars().take(n - 1).collect();
            c.push('…');
            c
        }
    };
    let base = |p: &str| -> String { p.rsplit('/').next().unwrap_or(p).to_string() };
    let line = match tool {
        "bash" | "terminal" => match arg("command") {
            Some(c) => format!("Running {}", short(&c, 40)),
            None => "Running a command".into(),
        },
        "read" => match arg("path") {
            Some(p) => format!("Reading {}", base(&p)),
            None => "Reading a file".into(),
        },
        "write" | "edit" | "apply_patch" => match arg("path") {
            Some(p) => format!("Editing {}", base(&p)),
            None => "Editing files".into(),
        },
        "grep" => match arg("pattern") {
            Some(p) => format!("Searching for {}", short(&p, 30)),
            None => "Searching the code".into(),
        },
        "find" => "Finding files".into(),
        "ls" => "Listing files".into(),
        "search" => match arg("query") {
            Some(q) => format!("Searching the web: {}", short(&q, 30)),
            None => "Searching the web".into(),
        },
        "fetch" => match arg("url") {
            Some(u) => format!(
                "Fetching {}",
                u.trim_start_matches("https://")
                    .trim_start_matches("http://")
                    .split('/')
                    .next()
                    .unwrap_or(&u)
            ),
            None => "Fetching a page".into(),
        },
        "spawn" => match arg("name") {
            Some(n) => format!("Starting worker {}", short(&n, 30)),
            None => "Starting a worker".into(),
        },
        "say" => match arg("to") {
            Some(t) => format!("Messaging {}", short(&t, 30)),
            None => "Sending a message".into(),
        },
        "ask" => "Asking the user".into(),
        "plan" => "Updating the plan".into(),
        "subscribe" => "Setting a subscription".into(),
        "browser" => match (arg("action"), arg("url")) {
            (Some(a), Some(u)) if a == "navigate" => format!(
                "Opening {}",
                u.trim_start_matches("https://")
                    .trim_start_matches("http://")
                    .split('/')
                    .next()
                    .unwrap_or(&u)
            ),
            (Some(a), _) => format!("Browser: {a}"),
            _ => "Using the browser".into(),
        },
        "screenshot" | "record" => "Taking a screenshot".into(),
        "await" | "jobs" => "Waiting on a job".into(),
        "secret" => "Checking the vault".into(),
        "remember" => "Saving a note to memory".into(),
        "changes" | "undo" => "Reviewing changes".into(),
        other => format!("Using {other}"),
    };
    clip(&line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guess_is_a_short_verb_phrase_from_the_tool_and_its_arguments() {
        let j = |s: &str| serde_json::from_str::<serde_json::Value>(s).unwrap();
        assert_eq!(
            derived(
                "bash",
                Some(&j(r#"{"command":"cargo test -p arbos-kernel"}"#))
            ),
            "Running cargo test -p arbos-kernel"
        );
        assert_eq!(
            derived(
                "read",
                Some(&j(r#"{"path":".arbos/docs/project-context.md"}"#))
            ),
            "Reading project-context.md"
        );
        assert_eq!(
            derived("edit", Some(&j(r#"{"path":"src/x.rs"}"#))),
            "Editing x.rs"
        );
        assert_eq!(
            derived(
                "fetch",
                Some(&j(r#"{"url":"https://docs.rs/tokio/latest"}"#))
            ),
            "Fetching docs.rs"
        );
        assert_eq!(
            derived("spawn", Some(&j(r#"{"name":"Fix the login bug"}"#))),
            "Starting worker Fix the login bug"
        );
        assert_eq!(derived("plan", None), "Updating the plan");
        assert_eq!(derived("whatever", None), "Using whatever");
        let long = "x".repeat(200);
        assert_eq!(clip(&long).chars().count(), MAX_CHARS);
        assert_eq!(clip("  Reading   files.  "), "Reading files");
    }

    #[test]
    fn write_read_clear_round_trip() {
        let dir = std::env::temp_dir().join(format!(
            "arbos-status-{}-{}",
            std::process::id(),
            crate::now_ms()
        ));
        std::fs::create_dir_all(dir.join(".arbos/agents/root")).unwrap();
        let place = Place::new(&dir);
        assert!(read(&place, "root").is_none());
        let s = write(
            &place,
            "root",
            "Reading project context and secrets inventory",
            "agent",
        )
        .unwrap();
        assert_eq!(s.source, "agent");
        let back = read(&place, "root").unwrap();
        assert_eq!(back.step, "Reading project context and secrets inventory");
        assert!(!back.since.is_empty());
        assert!(clear(&place, "root"));
        assert!(read(&place, "root").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_one_line_status_written_as_prose_is_the_step() {
        assert_eq!(
            spoken("status: Running sleep 45"),
            Some("Running sleep 45".into())
        );
        assert_eq!(
            spoken("  Status: **Reading the issue**"),
            Some("Reading the issue".into())
        );
        assert_eq!(spoken("status:"), None);
        assert_eq!(spoken("status: first\nthen a paragraph"), None);
        assert_eq!(spoken("The status: all tests pass."), None);
        assert_eq!(spoken("Done on branch x"), None);
        // The prompt's own form, copied out as text (Gemini, qal J1).
        assert_eq!(
            spoken("status \"Setting plan\""),
            Some("Setting plan".into())
        );
        assert_eq!(
            spoken("`status \"Reading the folder\"`"),
            Some("Reading the folder".into())
        );
        assert_eq!(
            spoken("status(\"Writing notes.md\")"),
            Some("Writing notes.md".into())
        );
        assert_eq!(
            spoken("Status 'Greeting the user'"),
            Some("Greeting the user".into())
        );
        assert_eq!(
            spoken("status Reading the folder"),
            None,
            "unmarked prose stays a reply"
        );
        assert_eq!(spoken("Statuses are green."), None);
        assert_eq!(spoken("status \"\""), None);
    }
}
