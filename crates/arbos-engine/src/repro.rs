//! Failing reproductions: evidence before the first edit, re-run after.
//!
//! SWE-bench loop, cycles 3 and 4: asking the agent to *state* its
//! mechanism (in prose, then kernel-enforced) changed no outcome; the
//! stated mechanism was the wrong one it already believed. Cycle 5 asks
//! for evidence instead. A `bash` call with `repro:true` is a reproduction
//! the agent derived from the request: it is recorded with its exit code,
//! and only a *failing* one counts. With `ARBOS_REPRO_REQUIRED=1` the first
//! `edit`/`write`/`apply_patch` of a task is refused until one failing
//! reproduction is on record. `changes` re-runs every recorded
//! reproduction and says which still fail, so the done-criterion pass has
//! the evidence in front of it.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Result, bail};
use arbos_core::{AgentId, Layout, Place};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The `bash` argument that marks a reproduction.
pub const ARG: &str = "repro";
/// Set to `1` to refuse the first edit without a failing reproduction.
pub const REQUIRED_ENV: &str = "ARBOS_REPRO_REQUIRED";
/// Longest a re-run at `changes` time may take.
const RERUN_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repro {
    pub command: String,
    pub cwd: PathBuf,
    /// Exit code when first run (before the fix). `None` = killed or unknown.
    pub exit: Option<i32>,
    pub ts: i64,
}

fn required() -> bool {
    std::env::var(REQUIRED_ENV).is_ok_and(|v| v == "1" || v == "true")
}

fn path(place: &Place, agent: &AgentId) -> PathBuf {
    Layout::new(place, agent.as_str()).dir.join("repro.jsonl")
}

/// A user message starts a new task: forget the previous reproductions.
pub fn reset(place: &Place, agent: &AgentId) {
    let _ = std::fs::remove_file(path(place, agent));
}

pub fn list(place: &Place, agent: &AgentId) -> Vec<Repro> {
    list_in(&Layout::new(place, agent.as_str()).dir)
}

pub fn list_in(agent_dir: &Path) -> Vec<Repro> {
    std::fs::read_to_string(agent_dir.join("repro.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Whether `args` marks this bash call as a reproduction.
pub fn marked(args: &Value) -> bool {
    args.get(ARG).and_then(Value::as_bool).unwrap_or(false)
}

/// Record a reproduction that just ran. Returns the line to append to the
/// tool result. Exit 0 is not a reproduction of a failure and is not kept.
pub fn record(
    place: &Place,
    agent: &AgentId,
    command: &str,
    cwd: &Path,
    exit: Option<i32>,
) -> String {
    match exit {
        Some(0) => "Not recorded as a reproduction: the command exited 0. A reproduction must fail before the fix (a non-zero exit: a failing assertion, an exception, a wrong value checked with a comparison). Make it fail, then run it again with repro:true.".to_string(),
        _ => {
            let file = path(place, agent);
            if let Some(dir) = file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let entry = Repro {
                command: command.to_string(),
                cwd: cwd.to_path_buf(),
                exit,
                ts: arbos_core::now_ms(),
            };
            let line = serde_json::to_string(&entry).unwrap_or_default();
            let mut text = std::fs::read_to_string(&file).unwrap_or_default();
            text.push_str(&line);
            text.push('\n');
            let _ = std::fs::write(&file, text);
            let n = list(place, agent).len();
            format!(
                "Reproduction {n} recorded (exit {}). changes re-runs it after your edits; the task is not done while it still fails.",
                exit.map(|c| c.to_string()).unwrap_or_else(|| "?".into())
            )
        }
    }
}

/// Before a write tool runs: with `ARBOS_REPRO_REQUIRED=1`, the first edit
/// of a task needs one failing reproduction on record.
pub fn gate(place: &Place, agent: &AgentId, tool: &str) -> Result<()> {
    if !required() || !crate::mechanism::GATED.contains(&tool) {
        return Ok(());
    }
    if list(place, agent).iter().any(|r| r.exit != Some(0)) {
        return Ok(());
    }
    bail!(
        "{tool} refused: no failing reproduction is on record for this task. Before the first edit, run the failure with bash repro:true — a command you derive from the request (the reporter's example, and a second input the request implies: another edge, another caller, another type) that exits non-zero now. Then repeat this call."
    )
}

/// Re-run every recorded reproduction (for `changes`): one line per
/// reproduction, `pass` when it now exits 0.
pub fn rerun_report(agent_dir: &Path) -> Option<String> {
    let repros = list_in(agent_dir);
    if repros.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    let mut failing = 0;
    for (i, r) in repros.iter().enumerate() {
        let exit = run_once(&r.command, &r.cwd);
        let verdict = match exit {
            Some(0) => "pass".to_string(),
            Some(c) => {
                failing += 1;
                format!("STILL FAILS (exit {c})")
            }
            None => {
                failing += 1;
                "STILL FAILS (killed or timed out)".to_string()
            }
        };
        let shown: String = r.command.chars().take(120).collect();
        lines.push(format!("  {}. {verdict} — {shown}", i + 1));
    }
    let head = if failing == 0 {
        format!(
            "Reproductions ({} recorded before the fix): all pass now.",
            repros.len()
        )
    } else {
        format!(
            "Reproductions ({} recorded before the fix): {failing} still fail. The task is not done.",
            repros.len()
        )
    };
    Some(format!("\n{head}\n{}\n", lines.join("\n")))
}

fn run_once(command: &str, cwd: &Path) -> Option<i32> {
    let shell = crate::jobs::job_shell();
    let secs = RERUN_TIMEOUT.as_secs().to_string();
    let mut cmd = if which_timeout() {
        let mut c = Command::new("timeout");
        c.arg(&secs).arg(shell).arg("-lc").arg(command);
        c
    } else {
        let mut c = Command::new(shell);
        c.arg("-lc").arg(command);
        c
    };
    cmd.current_dir(if cwd.is_dir() { cwd } else { Path::new(".") })
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd.env_clear();
    cmd.envs(arbos_core::envsafe::filtered(&[]));
    cmd.status().ok().and_then(|s| s.code())
}

fn which_timeout() -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join("timeout").is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failing_command_is_recorded_and_rerun() {
        // SAFETY: test-local; no other thread reads this variable.
        unsafe { std::env::set_var(REQUIRED_ENV, "1") };
        let dir = std::env::temp_dir().join(format!("arbos-repro-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let place = Place::new(dir.clone());
        let agent = AgentId::new("root");
        assert!(gate(&place, &agent, "edit").is_err());
        assert!(gate(&place, &agent, "read").is_ok());
        let note = record(&place, &agent, "true", &dir, Some(0));
        assert!(note.starts_with("Not recorded"));
        assert!(gate(&place, &agent, "edit").is_err());
        let flag = dir.join("fixed");
        let cmd = format!("test -f {}", flag.display());
        let note = record(&place, &agent, &cmd, &dir, Some(1));
        assert!(
            note.starts_with("Reproduction 1 recorded (exit 1)"),
            "{note}"
        );
        assert!(gate(&place, &agent, "edit").is_ok());
        let agent_dir = Layout::new(&place, "root").dir;
        let report = rerun_report(&agent_dir).unwrap();
        assert!(report.contains("1 still fail"), "{report}");
        std::fs::write(&flag, "").unwrap();
        let report = rerun_report(&agent_dir).unwrap();
        assert!(report.contains("all pass now"), "{report}");
        reset(&place, &agent);
        assert!(rerun_report(&agent_dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
