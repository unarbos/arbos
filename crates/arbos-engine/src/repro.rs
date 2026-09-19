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
/// Longest one re-run at `changes` time may take.
const RERUN_TIMEOUT: Duration = Duration::from_secs(180);
/// Longest the whole re-run pass may take. Seventeen recorded `runserver`
/// commands, each run to its 180 s, held one `changes` call for 51
/// minutes (SWE-bench cycle 36, django-13809) while the kernel's own
/// stall notice counted the wait. Past this, the rest are listed as not
/// re-run, with the reason; the report never lies about what it ran.
const RERUN_BUDGET: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repro {
    pub command: String,
    pub cwd: PathBuf,
    /// Exit code when first run (before the fix). `None` = killed or unknown.
    pub exit: Option<i32>,
    pub ts: i64,
}

/// How many distinct failing reproductions the first edit needs: `1`
/// (or `true`) = one; `2` = the reporter's example and a second input the
/// agent derives from the request (cycle 7 experiment against "right
/// file, wrong mechanism"); unset or `0` = no gate.
fn required() -> usize {
    match std::env::var(REQUIRED_ENV).ok().as_deref().map(str::trim) {
        Some("true") => 1,
        Some(n) => n.parse().unwrap_or(0),
        None => 0,
    }
}

/// Distinct failing reproductions on record (same command text counts once).
fn failing_distinct(place: &Place, agent: &AgentId) -> usize {
    let mut seen: Vec<String> = Vec::new();
    for r in list(place, agent) {
        if r.exit != Some(0) {
            let key = r.command.split_whitespace().collect::<Vec<_>>().join(" ");
            if !seen.contains(&key) {
                seen.push(key);
            }
        }
    }
    seen.len()
}

fn path(place: &Place, agent: &AgentId) -> PathBuf {
    Layout::new(place, agent.as_str()).dir.join("repro.jsonl")
}

fn last_failing_path(place: &Place, agent: &AgentId) -> PathBuf {
    Layout::new(place, agent.as_str())
        .dir
        .join("repro-last-failing.json")
}

/// Why a failed command is no evidence of the bug: the exit says the
/// command never ran the code, not that the code is wrong.
pub fn not_evidence(exit: Option<i32>) -> Option<&'static str> {
    match exit {
        Some(0) => Some("the command exited 0"),
        Some(126) => Some("exit 126: the command was not executable"),
        Some(127) => Some("exit 127: the command was not found"),
        None => Some("the command was killed or timed out, which says nothing about the code"),
        _ => None,
    }
}

/// Whether `command` runs code — an interpreter, a test runner, a build
/// tool, a script or a binary by path — as opposed to fetching, listing,
/// probing or installing. Leading `cd … &&`, `VAR=x` assignments,
/// `timeout N` and `env` are skipped. SWE-bench cycle 11: a refused `pip
/// download` exited non-zero and became "reproduction 1", so an agent
/// that had reproduced nothing believed it had, and the done rule passed
/// on a command that only failed. A command that failed is evidence of
/// nothing unless it exercised the bug.
pub fn runs_code(command: &str) -> bool {
    let mut words = command
        .split("&&")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .skip_while(|s| s.starts_with("cd ") || *s == "cd")
        .flat_map(|s| s.split_whitespace())
        .skip_while(|w| {
            w.contains('=') && !w.starts_with('=') && !w.starts_with('-')
                || *w == "env"
                || *w == "exec"
        });
    let Some(mut first) = words.next() else {
        return false;
    };
    if first == "timeout" {
        // `timeout [opts] N cmd`
        for w in words.by_ref() {
            if !w.starts_with('-') && w.chars().next().is_some_and(|c| !c.is_ascii_digit()) {
                first = w;
                break;
            }
        }
    }
    let base = first.rsplit('/').next().unwrap_or(first);
    if first.starts_with("./") || first.starts_with('/') && !INSTALLERS.contains(&base) {
        return true;
    }
    if base.starts_with("python") || base.starts_with("pypy") {
        // `python -m pip …` installs, `python setup.py install` too.
        let rest: Vec<&str> = words.clone().take(3).collect();
        return !(rest.first() == Some(&"-m")
            && rest
                .get(1)
                .is_some_and(|m| *m == "pip" || *m == "ensurepip"))
            && !(rest.first() == Some(&"setup.py")
                && rest
                    .get(1)
                    .is_some_and(|s| *s == "install" || *s == "develop"));
    }
    if RUNNERS.contains(&base) {
        return true;
    }
    if PACKAGE_TOOLS.contains(&base) {
        // `npm test` runs code; `npm install` does not.
        let sub = words.next().unwrap_or("");
        return matches!(sub, "test" | "run" | "start" | "exec" | "x");
    }
    false
}

/// Interpreters, test runners, build tools: a non-zero exit from one of
/// these is the code failing (or failing to build), which is evidence.
const RUNNERS: &[&str] = &[
    "pytest",
    "py.test",
    "nose2",
    "nosetests",
    "tox",
    "nox",
    "unittest",
    "node",
    "deno",
    "bun",
    "ts-node",
    "tsx",
    "jest",
    "mocha",
    "vitest",
    "ruby",
    "rspec",
    "rake",
    "bundle",
    "php",
    "phpunit",
    "perl",
    "prove",
    "go",
    "cargo",
    "rustc",
    "java",
    "javac",
    "mvn",
    "gradle",
    "gradlew",
    "dotnet",
    "make",
    "cmake",
    "ctest",
    "ninja",
    "swift",
    "julia",
    "R",
    "Rscript",
    "lua",
    "elixir",
    "mix",
    "ghc",
    "runghc",
    "stack",
    "cabal",
    "zig",
    "bash",
    "sh",
    "zsh",
    "dash",
    "expect",
    "npx",
];

/// Run code only with a run-like subcommand.
const PACKAGE_TOOLS: &[&str] = &["npm", "yarn", "pnpm", "poetry", "pipenv", "uv", "pdm"];

/// Whether the command's program is a fetcher or installer (`pip`, `curl`,
/// `git`, `npm install`…): a failure of it is never the bug's, marked or
/// not.
pub fn only_fetches(command: &str) -> bool {
    let words: Vec<&str> = command.split_whitespace().collect();
    let Some(first) = words.first() else {
        return false;
    };
    let base = first.rsplit('/').next().unwrap_or(first);
    if INSTALLERS.contains(&base) {
        return true;
    }
    if base.starts_with("python") && words.get(1) == Some(&"-m") && words.get(2) == Some(&"pip") {
        return true;
    }
    PACKAGE_TOOLS.contains(&base)
        && matches!(
            words.get(1).copied().unwrap_or(""),
            "install" | "i" | "add" | "sync" | "update" | "ci" | "lock" | "download"
        )
}

/// Never evidence, whatever the exit: they fetch, install or list.
const INSTALLERS: &[&str] = &[
    "pip", "pip3", "conda", "mamba", "apt", "apt-get", "brew", "gem", "curl", "wget", "git",
];

/// Every bash command that exits non-zero before the first edit *and
/// runs code* is a candidate reproduction. Cycle 5: the agent ran the
/// failing snippet without `repro:true` in 46 of 146 refusals, then
/// wandered; the gate now takes the last failing command as the
/// reproduction instead of refusing. Cycle 11: only a command that ran
/// code is taken — a refused `pip download`, a `grep` with no match, a
/// `ls` of a missing path are not the bug failing.
pub fn note_failing(place: &Place, agent: &AgentId, command: &str, cwd: &Path, exit: Option<i32>) {
    if not_evidence(exit).is_some()
        || !runs_code(command)
        || is_server(command)
        || moves_tree(command).is_some()
    {
        return;
    }
    let entry = Repro {
        command: command.to_string(),
        cwd: cwd.to_path_buf(),
        exit,
        ts: arbos_core::now_ms(),
    };
    let file = last_failing_path(place, agent);
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(file, serde_json::to_string(&entry).unwrap_or_default());
}

fn take_last_failing(place: &Place, agent: &AgentId) -> Option<Repro> {
    let file = last_failing_path(place, agent);
    let entry = serde_json::from_str(&std::fs::read_to_string(&file).ok()?).ok()?;
    let _ = std::fs::remove_file(file);
    Some(entry)
}

/// A user message starts a new task: forget the previous reproductions.
pub fn reset(place: &Place, agent: &AgentId) {
    let _ = std::fs::remove_file(path(place, agent));
    let _ = std::fs::remove_file(last_failing_path(place, agent));
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
    if is_server(command) {
        return "Not recorded as a reproduction: this command runs a server or a watcher, which never exits on its own — its exit is the timeout's, not the bug's. The reproduction is the request that hits the server (curl, the test client, a script that asserts on the response); run that with repro:true.".to_string();
    }
    if let Some(what) = moves_tree(command) {
        return format!(
            "Not recorded as a reproduction: `{what}` changes the working tree, and changes re-runs every reproduction after your edits — this one would {what} your fix each time it ran. A reproduction only runs the code and fails; run the failing test or script itself with repro:true."
        );
    }
    match exit {
        Some(0) => "Not recorded as a reproduction: the command exited 0. A reproduction must fail before the fix (a non-zero exit: a failing assertion, an exception, a wrong value checked with a comparison). Make it fail, then run it again with repro:true.".to_string(),
        _ if not_evidence(exit).is_some() => format!(
            "Not recorded as a reproduction: {}. A reproduction runs the code and fails because of the bug; fix the command so it runs, then run it again with repro:true.",
            not_evidence(exit).unwrap_or("")
        ),
        // Marked by hand, the agent's word stands — except for a fetch or
        // an install, whose failure is never the bug's.
        _ if only_fetches(command) => format!(
            "Not recorded as a reproduction: `{}` fetches or installs rather than running the code, so its failure says nothing about the bug. Run the failing behaviour itself (an interpreter, a test runner, a script) with repro:true.",
            command.split_whitespace().next().unwrap_or("")
        ),
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
            // Appended, not read-then-rewritten: a failed read must not
            // shrink the record to this one line (arbos_core::record).
            if let Err(e) = arbos_core::files::append_line(&file, &entry) {
                eprintln!("repro {}: {e:#}", file.display());
            }
            let n = list(place, agent).len();
            format!(
                "Reproduction {n} recorded (exit {}). changes re-runs it after your edits; the task is not done while it still fails.",
                exit.map(|c| c.to_string()).unwrap_or_else(|| "?".into())
            )
        }
    }
}

/// Before a write tool runs: with `ARBOS_REPRO_REQUIRED=N`, the first edit
/// of a task needs N distinct failing reproductions on record. When fewer
/// were marked but the agent's last bash command failed, that command is
/// taken as one and, if the count is then met, the edit proceeds with a
/// note (`Ok(Some)`).
pub fn gate(place: &Place, agent: &AgentId, tool: &str) -> Result<Option<String>> {
    let need = required();
    if need == 0 || !crate::mechanism::GATED.contains(&tool) {
        return Ok(None);
    }
    let mut have = failing_distinct(place, agent);
    if have >= need {
        return Ok(None);
    }
    let mut taken = None;
    if let Some(last) = take_last_failing(place, agent) {
        let note = record(place, agent, &last.command, &last.cwd, last.exit);
        let shown: String = last.command.chars().take(120).collect();
        taken = Some(format!(
            "Your last failing bash command was taken as a reproduction ({}): {shown}. Mark the intended one with repro:true next time.",
            note.split(". ").next().unwrap_or("recorded")
        ));
        have = failing_distinct(place, agent);
    }
    if have >= need {
        return Ok(taken);
    }
    if have == 0 {
        bail!(
            "{tool} refused: no failing reproduction is on record for this task. Before the first edit, run the failure with bash repro:true — a command you derive from the request (the reporter's example, and a second input the request implies: another edge, another caller, another type) that exits non-zero now. Then repeat this call."
        );
    }
    bail!(
        "{tool} refused: {have} failing reproduction(s) on record, {need} needed before the first edit. The reporter's example is one; derive another input from the request text — an edge, caller, type, or option it names or implies — that must also fail for the same reason, and run it with bash repro:true (it must exit non-zero now). A fix that passes the example but not the second input is the wrong mechanism. Then repeat this call."
    )
}

/// A command that runs a server or a watcher never exits on its own, so
/// its failure is the timeout's and re-running it is minutes for nothing.
fn is_server(command: &str) -> bool {
    crate::tools::looks_like_server(command)
}

/// Git subcommands that move the working tree, the index or HEAD.
const TREE_MOVING_GIT: &[&str] = &[
    "stash",
    "checkout",
    "switch",
    "restore",
    "reset",
    "clean",
    "commit",
    "add",
    "rm",
    "mv",
    "apply",
    "am",
    "revert",
    "cherry-pick",
    "rebase",
    "merge",
    "pull",
    "worktree",
];

/// The `git <sub>` in `command` that would move the working tree, if
/// any — in any segment (`git stash && pytest …`), through `git -C dir`
/// and `git -c k=v`. A reproduction is re-run by `changes` after every
/// edit; one that stashes, checks out or resets undoes the fix each time
/// it runs. SWE-bench cycle 39, pytest-10356: `git stash && python -m
/// pytest …` was recorded, each `changes` call stashed the fix, and the
/// patch at exit was 0 bytes with the gold's design in the stash.
pub fn moves_tree(command: &str) -> Option<String> {
    for seg in crate::tools::git_guard::segments(command) {
        let words = crate::tools::git_guard::shell_words(&seg);
        let mut it = words.iter().map(String::as_str);
        // `env X=1 git …`, `sudo git …` are the same command.
        let mut head = it.next()?;
        while matches!(head, "env" | "sudo" | "command") || head.contains('=') {
            head = it.next()?;
        }
        if head != "git" && !head.ends_with("/git") {
            continue;
        }
        let mut sub = None;
        while let Some(w) = it.next() {
            if w == "-C" || w == "-c" || w == "--git-dir" || w == "--work-tree" {
                it.next();
                continue;
            }
            if w.starts_with('-') {
                continue;
            }
            sub = Some(w);
            break;
        }
        if let Some(sub) = sub
            && TREE_MOVING_GIT.contains(&sub)
        {
            return Some(format!("git {sub}"));
        }
    }
    None
}

/// What the tree looks like, for telling a re-run that moved it: HEAD,
/// the stash count, and the status lines. None outside a repository.
fn tree_mark(cwd: &Path) -> Option<String> {
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(cwd)
            .stdin(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    };
    let head = git(&["rev-parse", "HEAD"])?;
    let stash = git(&["stash", "list"]).unwrap_or_default();
    let status = git(&["status", "--porcelain"]).unwrap_or_default();
    Some(format!("{head}\n{}\n{status}", stash.lines().count()))
}

/// Re-run every recorded reproduction (for `changes`): one line per
/// reproduction, `pass` when it now exits 0. The pass as a whole keeps
/// to `RERUN_BUDGET`; reproductions it did not reach are listed as not
/// re-run and counted as unsettled, never as passing.
pub fn rerun_report(agent_dir: &Path) -> Option<String> {
    rerun_report_within(agent_dir, RERUN_BUDGET)
}

fn rerun_report_within(agent_dir: &Path, budget: Duration) -> Option<String> {
    let repros = list_in(agent_dir);
    if repros.is_empty() {
        return None;
    }
    let started = std::time::Instant::now();
    let mut lines = Vec::new();
    let mut failing = 0;
    let mut skipped = 0;
    for (i, r) in repros.iter().enumerate() {
        let shown: String = r.command.chars().take(120).collect();
        let left = budget.saturating_sub(started.elapsed());
        if is_server(&r.command) {
            skipped += 1;
            lines.push(format!(
                "  {}. not re-run — a server or watcher never exits on its own; the reproduction is the request that hits it — {shown}",
                i + 1
            ));
            continue;
        }
        if let Some(what) = moves_tree(&r.command) {
            // Recorded by an older kernel: not run — it would undo the fix.
            skipped += 1;
            lines.push(format!(
                "  {}. not re-run — `{what}` changes the working tree and would undo your fix; run the failing test itself with repro:true — {shown}",
                i + 1
            ));
            continue;
        }
        if left < Duration::from_secs(1) {
            skipped += 1;
            lines.push(format!(
                "  {}. not re-run — the re-run pass's {} s budget is spent — {shown}",
                i + 1,
                budget.as_secs()
            ));
            continue;
        }
        let before = tree_mark(&r.cwd);
        let exit = run_once(&r.command, &r.cwd, left.min(RERUN_TIMEOUT));
        let moved = before.is_some() && tree_mark(&r.cwd) != before;
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
        lines.push(format!("  {}. {verdict} — {shown}", i + 1));
        if moved {
            // Said, whatever the exit: a reproduction that leaves the tree
            // different from how it found it is not a reproduction, and
            // the person checks their work before trusting the verdict.
            lines.push(
                "     this reproduction CHANGED THE WORKING TREE (HEAD, the stash or the status differ from before it ran). Check `git status` and `git stash list` before going on; a reproduction must only run the code."
                    .to_string(),
            );
        }
    }
    let head = if failing == 0 && skipped == 0 {
        format!(
            "Reproductions ({} recorded before the fix): all pass now.",
            repros.len()
        )
    } else if failing == 0 {
        format!(
            "Reproductions ({} recorded before the fix): {} pass now; {skipped} not re-run (see below) — run those yourself before calling the task done.",
            repros.len(),
            repros.len() - skipped
        )
    } else {
        format!(
            "Reproductions ({} recorded before the fix): {failing} still fail{}. The task is not done.",
            repros.len(),
            if skipped > 0 {
                format!(", {skipped} not re-run")
            } else {
                String::new()
            }
        )
    };
    Some(format!("\n{head}\n{}\n", lines.join("\n")))
}

fn run_once(command: &str, cwd: &Path, timeout: Duration) -> Option<i32> {
    let shell = crate::jobs::job_shell();
    let flag = if crate::jobs::login_shell(shell) {
        "-lc"
    } else {
        "-c"
    };
    let secs = timeout.as_secs().max(1).to_string();
    let mut cmd = if which_timeout() {
        let mut c = Command::new("timeout");
        c.arg(&secs).arg(shell).arg(flag).arg(command);
        c
    } else {
        let mut c = Command::new(shell);
        c.arg(flag).arg(command);
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
        assert!(gate(&place, &agent, "read").unwrap().is_none());
        let note = record(&place, &agent, "true", &dir, Some(0));
        assert!(note.starts_with("Not recorded"));
        assert!(gate(&place, &agent, "edit").is_err());
        // A failing command that did not run code is not a candidate: a
        // refused download, a listing of a missing path, a missing binary.
        note_failing(&place, &agent, "pip download requests==99", &dir, Some(1));
        note_failing(&place, &agent, "ls /nope", &dir, Some(2));
        note_failing(&place, &agent, "python3 -c 'import x'", &dir, Some(127));
        assert!(gate(&place, &agent, "edit").is_err());
        // Nor is one marked by hand.
        let note = record(&place, &agent, "pip download requests==99", &dir, Some(1));
        assert!(note.contains("fetches or installs"), "{note}");
        let note = record(&place, &agent, "npm install left-pad", &dir, Some(1));
        assert!(note.contains("fetches or installs"), "{note}");
        let note = record(&place, &agent, "python3 repro.py", &dir, Some(127));
        assert!(note.contains("exit 127"), "{note}");
        assert_eq!(list(&place, &agent).len(), 0);
        // An unmarked failing command that ran code is taken as the reproduction.
        note_failing(
            &place,
            &agent,
            "python3 -c 'raise SystemExit(1)'",
            &dir,
            Some(1),
        );
        let taken = gate(&place, &agent, "edit").unwrap().unwrap();
        assert!(taken.contains("taken as a reproduction"), "{taken}");
        assert_eq!(list(&place, &agent).len(), 1);
        reset(&place, &agent);
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

    /// SWE-bench cycle 36: seventeen `runserver` reproductions, each
    /// re-run to its 180 s, held one `changes` call for 51 minutes. A
    /// server is refused as a reproduction with the reason; a pass that
    /// runs out of budget lists the rest as not re-run and never calls
    /// the task done on their account.
    /// SWE-bench cycle 39, pytest-10356: `git stash && python -m pytest …`
    /// recorded as the reproduction; every `changes` re-run stashed the
    /// fix; the patch at exit was 0 bytes. A command that moves the tree
    /// is refused at record, not taken as the last failing command, and
    /// one an older kernel recorded is not re-run — the fix stays where
    /// it is. A re-run that moves the tree anyway is said.
    #[test]
    fn a_reproduction_that_moves_the_tree_is_refused_and_never_re_run() {
        for (cmd, want) in [
            (
                "git stash && python -m pytest testing/test_x.py",
                Some("git stash"),
            ),
            (
                "cd sub; git -C . checkout -- . && pytest",
                Some("git checkout"),
            ),
            (
                "git -c core.autocrlf=false reset --hard HEAD~1; make test",
                Some("git reset"),
            ),
            ("env FOO=1 git clean -fdx", Some("git clean")),
            ("git status && pytest tests/test_git.py", None),
            ("git diff --stat; git log -1", None),
            ("python -m pytest -k 'stash and checkout'", None),
            ("pytest tests/test_git_stash.py", None),
        ] {
            assert_eq!(moves_tree(cmd).as_deref(), want, "{cmd}");
        }
        let dir = std::env::temp_dir().join(format!("arbos-repro-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |a: &[&str]| {
            assert!(
                Command::new("git")
                    .args(a)
                    .current_dir(&dir)
                    .status()
                    .unwrap()
                    .success()
            )
        };
        git(&["init", "-q"]);
        std::fs::write(dir.join("a.py"), "x = 1\n").unwrap();
        git(&["add", "a.py"]);
        git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            "start",
        ]);
        // The fix, uncommitted.
        std::fs::write(dir.join("a.py"), "x = 2\n").unwrap();
        let place = Place::new(dir.clone());
        let agent = AgentId::new("root");
        let note = record(
            &place,
            &agent,
            "git stash && python3 -c 'exit(1)'",
            &dir,
            Some(1),
        );
        assert!(
            note.contains("`git stash` changes the working tree"),
            "{note}"
        );
        assert_eq!(list(&place, &agent).len(), 0);
        note_failing(
            &place,
            &agent,
            "git stash && python3 -c 'exit(1)'",
            &dir,
            Some(1),
        );
        assert!(take_last_failing(&place, &agent).is_none());
        // Recorded by an older kernel, beside a real one and one that
        // moves the tree without git.
        std::fs::create_dir_all(path(&place, &agent).parent().unwrap()).unwrap();
        let mut text = String::new();
        for (cmd, exit) in [
            ("git stash && python3 -c 'exit(1)'", 1),
            ("python3 -c 'exit(1)'", 1),
            ("touch stray.txt; exit 1", 1),
        ] {
            text.push_str(
                &serde_json::to_string(&Repro {
                    command: cmd.into(),
                    cwd: dir.clone(),
                    exit: Some(exit),
                    ts: 1,
                })
                .unwrap(),
            );
            text.push('\n');
        }
        std::fs::write(path(&place, &agent), text).unwrap();
        let report = rerun_report(&Layout::new(&place, "root").dir).unwrap();
        assert!(
            report.contains("1. not re-run — `git stash` changes the working tree"),
            "{report}"
        );
        assert!(report.contains("2. STILL FAILS (exit 1)"), "{report}");
        assert_eq!(
            report.matches("CHANGED THE WORKING TREE").count(),
            1,
            "only the third moved it: {report}"
        );
        assert!(
            report.lines().any(|l| l.starts_with("  3. STILL FAILS")),
            "{report}"
        );
        // The fix is where it was: nothing stashed, a.py still edited.
        assert_eq!(
            std::fs::read_to_string(dir.join("a.py")).unwrap(),
            "x = 2\n"
        );
        let stashes = Command::new("git")
            .args(["stash", "list"])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(
            stashes.stdout.is_empty(),
            "{:?}",
            String::from_utf8_lossy(&stashes.stdout)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_server_is_not_a_reproduction_and_the_rerun_pass_keeps_to_its_budget() {
        let dir = std::env::temp_dir().join(format!("arbos-repro-budget-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let place = Place::new(dir.clone());
        let agent = AgentId::new("root");
        let note = record(
            &place,
            &agent,
            "python manage.py runserver 8000",
            &dir,
            Some(124),
        );
        assert!(
            note.starts_with("Not recorded as a reproduction: this command runs a server"),
            "{note}"
        );
        note_failing(&place, &agent, "npm run dev", &dir, Some(1));
        assert!(
            take_last_failing(&place, &agent).is_none(),
            "a server is not taken as the last failing command"
        );
        // Three real reproductions of three seconds each; a budget of five
        // seconds runs the first whole, cuts the second short, and never
        // starts the third.
        for n in 1..=3 {
            record(&place, &agent, &format!("sleep 3; exit {n}"), &dir, Some(n));
        }
        // And one server recorded by an older kernel: skipped, never run.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(path(&place, &agent))
            .unwrap();
        use std::io::Write;
        writeln!(
            f,
            "{}",
            serde_json::to_string(&Repro {
                command: "python manage.py runserver".into(),
                cwd: dir.clone(),
                exit: Some(124),
                ts: 1
            })
            .unwrap()
        )
        .unwrap();
        let started = std::time::Instant::now();
        let report =
            rerun_report_within(&Layout::new(&place, "root").dir, Duration::from_secs(5)).unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(9),
            "the pass kept to its budget: {:?}",
            started.elapsed()
        );
        assert!(report.contains("STILL FAILS (exit 1)"), "{report}");
        assert!(report.contains("budget is spent"), "{report}");
        assert!(
            report.contains("a server or watcher never exits"),
            "{report}"
        );
        assert!(report.contains("not re-run"), "{report}");
        assert!(!report.contains("all pass now"), "{report}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_counts_as_running_code() {
        for cmd in [
            "python3 repro.py",
            "cd /repo && python -m pytest tests/test_x.py -x",
            "PYTHONPATH=. python3 -c 'import astropy'",
            "timeout 60 pytest -q",
            "./run_tests.sh",
            "/usr/bin/env node index.js",
            "cargo test --lib",
            "go test ./...",
            "npm test",
            "npx jest src/",
            "make check",
            "bash scripts/repro.sh",
            "java -jar app.jar",
        ] {
            assert!(runs_code(cmd), "{cmd}");
        }
        for cmd in [
            "pip download requests==99",
            "pip install -e .",
            "python -m pip install numpy",
            "python setup.py install",
            "ls /nope",
            "grep -r needle src/",
            "git checkout -b fix",
            "curl -sSf https://example.com",
            "npm install",
            "uv sync",
            "cd /nope",
            "test -f missing",
            "",
        ] {
            assert!(!runs_code(cmd), "{cmd}");
        }
        assert!(not_evidence(Some(127)).is_some());
        assert!(not_evidence(Some(126)).is_some());
        assert!(not_evidence(None).is_some());
        assert!(not_evidence(Some(0)).is_some());
        assert!(not_evidence(Some(1)).is_none());
        assert!(
            not_evidence(Some(124)).is_none(),
            "a timeout under `timeout` is a hang, which can be the bug"
        );
    }
}
