//! `bash`, `await`, `jobs`. Every command is a job (see `jobs.rs`); the
//! tool call is only a window onto it. `wait_ms` bounds the call, never the
//! process. Only `timeout_ms` kills.

use anyhow::{Result, bail};
use serde_json::Value;
use std::time::Duration;

use super::ToolOut;
use crate::access::Access;
use crate::evict::{EVICT_BYTES, EVICT_LINES};
use crate::jobs::{Job, JobsRoot, Status};
use crate::tool::{
    BoxFuture, Plan, PlanCx, RunCx, Tool, opt_bool, opt_str, opt_u64, req, simple_schema,
    typed_schema,
};

/// `background: true` still waits this long so an instant failure
/// (`command not found`) is reported without a second call.
const BACKGROUND_GRACE: Duration = Duration::from_millis(500);
const AWAIT_DEFAULT_MS: u64 = 30_000;
const AWAIT_MAX_MS: u64 = 3_600_000;
const AWAIT_POLL: Duration = Duration::from_millis(200);
/// Output shown to the model. Kept under the eviction limits so the
/// transcript never cuts it a second time and the cite always points at the
/// journal. At 60 lines a `cat` or a test run lost its head and models
/// that read files through the shell (Gemini) spent whole turns probing
/// whether stdout worked at all.
const TAIL_LINES: usize = EVICT_LINES - 4;
const TAIL_BYTES: usize = EVICT_BYTES - 1024;
/// Lines from the start kept when output is cut: the command's own first
/// words (a file's header, a test session banner) are where orientation is.
const HEAD_LINES: usize = 15;

pub struct Bash;
pub struct Await;
pub struct Jobs;

impl Tool for Bash {
    fn name(&self) -> &'static str {
        "bash"
    }
    fn schema(&self) -> Value {
        typed_schema(
            "bash",
            "Run a shell command. Output returns within wait_ms; otherwise it continues as a job (await/jobs). Only timeout_ms kills. background:true for servers.",
            &[
                ("command", "", true, "string"),
                (
                    "description",
                    "What this command does, 5–10 words, for the line the user sees (\"List repo contents and recent commits\").",
                    false,
                    "string",
                ),
                ("cwd", "", false, "string"),
                (
                    "wait_ms",
                    "Attached wait, ms (default 600000).",
                    false,
                    "integer",
                ),
                (
                    "background",
                    "Return at once; keep running as a job.",
                    false,
                    "boolean",
                ),
                (
                    "keep",
                    "Let the job outlive this kernel (default: it dies with it and is reaped at the next start).",
                    false,
                    "boolean",
                ),
                (
                    "timeout_ms",
                    "Hard kill after this many ms.",
                    false,
                    "integer",
                ),
                (
                    "repro",
                    "This command is a reproduction of the reported failure, derived from the request. Recorded with its exit code (it must be non-zero now); changes re-runs it after your edits.",
                    false,
                    "boolean",
                ),
            ],
        )
    }
    fn plan(&self, cx: &PlanCx, args: &Value) -> Result<Plan> {
        let cmd = req(args, "command")?;
        let dir = opt_str(args, "cwd")
            .map(|c| cx.resolve_unconfined(c))
            .unwrap_or_else(|| cx.cwd.to_path_buf());
        let access = if is_readonly_command(cmd) {
            Access::read_path(&dir)
        } else {
            Access::exclusive()
        };
        let plan = Plan::access(access);
        // Interactive only when it will wait on a card (ask mode); in
        // auto the same command is refused in `run`, no card.
        Ok(
            if needs_approval(cmd) && cx.agent.mode == arbos_core::Mode::Ask {
                plan.interactive()
            } else {
                plan
            },
        )
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        Box::pin(async move {
            let cmd = req(&args, "command")?;
            // A coordinator's shell is for the one quick command the user
            // asked to see; a build or a test run is a worker's (decision
            // 2026-09-14; the model ran pytest itself on cycle 6).
            if cx.agent.role.as_deref() == Some(arbos_core::project::COORDINATOR)
                && let Some(what) = build_or_test(cmd)
            {
                bail!(
                    "bash: {what} is a worker's job, not the coordinator's — spawn a worker with the exact command (wait=true for a one-off) and relay its result. Your bash is for one quick command the user asked to see."
                );
            }
            // Before the approval prompt: a refused command is not a
            // question for the user.
            {
                let dir = opt_str(&args, "cwd")
                    .map(|c| cx.cwd.join(c))
                    .unwrap_or_else(|| cx.cwd.clone());
                let place = cx.place.path().to_path_buf();
                let cmd_owned = cmd.to_string();
                tokio::task::spawn_blocking(move || {
                    super::git_guard::check(&place, &dir, &cmd_owned)
                })
                .await
                .map_err(|e| anyhow::anyhow!("git guard task: {e}"))??;
            }
            // A wipe of the filesystem root, sudo, mkfs, a fork bomb: in
            // auto mode nothing waits on a card (decision 2026-09-15), so
            // these are refused outright with the reason — the model can
            // ask the user in words if it truly needs one. In ask mode
            // the call was already allowed before it ran.
            if needs_approval(cmd) && cx.agent.mode != arbos_core::Mode::Ask {
                bail!(
                    "bash: refused — this command wipes a system path, escalates with sudo, or formats a disk, and the default mode runs without approval cards. Do it another way, or ask the user in words and have them run it; ask mode (the mode chip) asks per command instead."
                );
            }
            let dir = opt_str(&args, "cwd")
                .map(|c| cx.cwd.join(c))
                .unwrap_or_else(|| cx.cwd.clone());
            if !dir.is_dir() {
                bail!(
                    "cwd {} does not exist; the working directory is {}. Use a path relative to it, or omit cwd.",
                    dir.display(),
                    cx.cwd.display()
                );
            }
            let background = opt_bool(&args, "background").unwrap_or(false);
            let timeout_ms = opt_u64(&args, "timeout_ms");
            let wait = if background {
                BACKGROUND_GRACE
            } else {
                Duration::from_millis(opt_u64(&args, "wait_ms").unwrap_or(cx.bash_wait_ms))
            };

            let root = JobsRoot::for_agent(&cx.place, &cx.agent.id);
            let sandbox = crate::sandbox::for_agent(&cx.place, &cx.agent);
            let granted = crate::secrets::store()
                .env_for(&arbos_core::lineage(&cx.place, cx.agent.id.as_str()));
            let (job, mut child) = root.spawn(cmd, &dir, timeout_ms, sandbox.as_ref(), granted)?;
            if opt_bool(&args, "keep").unwrap_or(false) {
                let _ = std::fs::write(job.dir.join("keep"), "");
            }
            let journal = job.journal().display().to_string();

            // Reap in the background so the call can return before the
            // command does. The exit code is the wrapper's job, not ours.
            let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
            tokio::spawn(async move {
                let _ = child.wait().await;
                let _ = done_tx.send(());
            });
            if let Some(ms) = timeout_ms {
                let root = root.clone();
                let id = job.id.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                    if let Ok(j) = root.load(&id) {
                        root.kill(&j);
                    }
                });
            }

            let mut done_rx = done_rx;
            let finished = tokio::select! {
                _ = &mut done_rx => true,
                _ = tokio::time::sleep(wait) => false,
                _ = cx.cancel.cancelled() => {
                    // Stop while attached means stop: the user wants it gone.
                    if let Ok(j) = root.load(&job.id) {
                        root.kill(&j);
                    }
                    bail!("bash interrupted; job {} killed", job.id);
                }
            };

            let job = root.load(&job.id)?;
            if !finished && job.running() {
                root.mark_detached(&job);
                let (text, skipped) = root.read_new(&job);
                let body = format_tail(&text, "(no output yet)", &journal, skipped);
                let verb = if background {
                    "Running"
                } else {
                    "Still running"
                };
                return Ok(ToolOut::with_paths(
                    format!(
                        "{body}\n\n{verb} as job {id} (pid {pid}). Follow with await {id} (optional regex pattern), list with jobs, stop with bash `kill -- -{pid}`. Log: {journal}",
                        id = job.id,
                        pid = job.meta.pid,
                    ),
                    vec![journal],
                ));
            }

            // Finished (or finished in the same instant the wait expired).
            let (text, skipped) = root.read_new(&job);
            let mut body = format_tail(&text, "(no output)", &journal, skipped);
            match job.status {
                Status::Exited(0) => {
                    if let Some(file) = viewed_file(cmd) {
                        // Reading through the shell gives no LINE:HASH, so
                        // the next edit has nothing to anchor on and the
                        // model reads the file again with `read`. Say so once.
                        body.push_str(&format!(
                            "\n[read {file} shows LINE:HASH anchors for edit; grep for a symbol instead of paging]"
                        ));
                    }
                }
                Status::Exited(code) => body.push_str(&format!("\nexit {code}\n")),
                Status::Killed => {
                    let why = match &job.killed_why {
                        Some(why) => why.clone(),
                        None if timeout_ms.is_some() => "timed out".to_string(),
                        None => "was killed by a signal from outside the kernel".to_string(),
                    };
                    body.push_str(&format!(
                        "\nCommand {why} before completing (job {})\n",
                        job.id
                    ));
                }
                Status::Running => unreachable!(),
            }
            let exit = match job.status {
                Status::Exited(code) => Some(code),
                _ => None,
            };
            if crate::repro::marked(&args) {
                body.push('\n');
                body.push_str(&crate::repro::record(
                    &cx.place,
                    &cx.agent.id,
                    cmd,
                    &dir,
                    exit,
                ));
            } else {
                crate::repro::note_failing(&cx.place, &cx.agent.id, cmd, &dir, exit);
            }
            Ok(ToolOut::with_paths(body, vec![journal]))
        })
    }
}

impl Tool for Await {
    fn name(&self) -> &'static str {
        "await"
    }
    fn schema(&self) -> Value {
        simple_schema(
            "await",
            "Wait on a bash job: new output when it exits, matches pattern, or wait_ms elapses.",
            &[
                ("id", "e.g. j3", true),
                ("pattern", "Regex: return on match.", false),
                ("wait_ms", "ms (30000, max 3600000).", false),
            ],
        )
    }
    fn plan(&self, _cx: &PlanCx, _args: &Value) -> Result<Plan> {
        Ok(Plan::access(Access::none()))
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        Box::pin(async move {
            let root = JobsRoot::for_agent(&cx.place, &cx.agent.id);
            let id = req(&args, "id")?.trim().to_string();
            let mut job = root.load(&id)?;
            let pattern = match opt_str(&args, "pattern") {
                Some(p) => Some(
                    regex::Regex::new(p).map_err(|e| anyhow::anyhow!("invalid pattern: {e}"))?,
                ),
                None => None,
            };
            let wait = Duration::from_millis(
                opt_u64(&args, "wait_ms")
                    .unwrap_or(AWAIT_DEFAULT_MS)
                    .min(AWAIT_MAX_MS),
            );
            let deadline = tokio::time::Instant::now() + wait;
            let journal = job.journal().display().to_string();

            let mut acc = String::new();
            let mut skipped_total = 0;
            let status = loop {
                let (text, skipped) = root.read_new(&job);
                acc.push_str(&text);
                skipped_total += skipped;
                job = root.load(&id)?;
                if !job.running() {
                    let (text, skipped) = root.read_new(&job); // final flush raced the exit file
                    acc.push_str(&text);
                    skipped_total += skipped;
                    break format!("Job {id} {}.", job.status_line());
                }
                if let Some(re) = &pattern {
                    if re.is_match(&acc) {
                        break format!("Pattern matched. Job {id} is {}.", job.status_line());
                    }
                }
                if tokio::time::Instant::now() >= deadline {
                    break format!(
                        "Still waiting: job {id} is {}. Await again, or stop it with bash `kill -- -{}`.",
                        job.status_line(),
                        job.meta.pid
                    );
                }
                tokio::select! {
                    _ = tokio::time::sleep(AWAIT_POLL) => {}
                    // The job is not this turn's child. Stop the waiting, not the work.
                    _ = cx.cancel.cancelled() => bail!("await interrupted; job {id} keeps running"),
                }
            };
            let body = format_tail(&acc, "(no new output)", &journal, skipped_total);
            Ok(ToolOut::with_paths(
                format!("{body}\n\n{status}"),
                vec![journal],
            ))
        })
    }
}

impl Tool for Jobs {
    fn name(&self) -> &'static str {
        "jobs"
    }
    fn schema(&self) -> Value {
        simple_schema("jobs", "List this agent's jobs.", &[])
    }
    fn plan(&self, _cx: &PlanCx, _args: &Value) -> Result<Plan> {
        Ok(Plan::access(Access::none()))
    }
    fn run(&self, cx: RunCx, _args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        Box::pin(async move {
            let root = JobsRoot::for_agent(&cx.place, &cx.agent.id);
            Ok(ToolOut::text(jobs_list(&root.list())))
        })
    }
}

pub fn jobs_list(jobs: &[Job]) -> String {
    if jobs.is_empty() {
        return "(no jobs)\n".into();
    }
    let mut out = String::new();
    for j in jobs {
        let mut cmd = j.meta.command.replace('\n', " ");
        if cmd.chars().count() > 120 {
            cmd = cmd.chars().take(120).collect::<String>() + "…";
        }
        out.push_str(&format!(
            "{}: {} — `{}` — log: {}\n",
            j.id,
            j.status_line(),
            cmd,
            j.journal().display()
        ));
    }
    out
}

/// Tail-truncate for the model. The journal on disk is the full record, so
/// the notice points there instead of spilling a copy anywhere.
fn format_tail(output: &str, empty: &str, journal: &str, skipped: u64) -> String {
    let lines: Vec<&str> = output.lines().collect();
    let total = lines.len();
    let fits_lines = total <= TAIL_LINES;
    let fits_bytes = output.len() <= TAIL_BYTES;
    if fits_lines && fits_bytes {
        let text = if output.trim().is_empty() {
            empty.to_string()
        } else {
            output.trim_end().to_string()
        };
        if skipped == 0 {
            return text;
        }
        return format!(
            "{text}\n\n[{} of earlier output omitted. Full log: {journal}]",
            human_bytes(skipped)
        );
    }
    // Head + tail. The marker sits where the cut is, so a `cat` that lost
    // its middle reads as exactly that, not as a broken command.
    let head_n = HEAD_LINES.min(total);
    let mut tail_n = TAIL_LINES.saturating_sub(head_n).min(total - head_n);
    let mut head: Vec<&str> = lines[..head_n].to_vec();
    let mut tail: Vec<&str> = lines[total - tail_n..].to_vec();
    let bytes = |h: &[&str], t: &[&str]| h.iter().chain(t).map(|l| l.len() + 1).sum::<usize>();
    while bytes(&head, &tail) > TAIL_BYTES && tail.len() > 1 {
        tail.remove(0);
        tail_n -= 1;
    }
    while bytes(&head, &tail) > TAIL_BYTES && head.len() > 1 {
        head.pop();
    }
    let mut text = String::new();
    for l in &head {
        text.push_str(&clip_line(l));
        text.push('\n');
    }
    let omitted = total - head.len() - tail.len();
    text.push_str(&format!(
        "[… {omitted} lines omitted (lines {}–{} of {total}). Full output: {journal}. To see a file, use the read tool; to search it, grep.]\n",
        head.len() + 1,
        total - tail.len()
    ));
    for l in &tail {
        text.push_str(&clip_line(l));
        text.push('\n');
    }
    let _ = tail_n;
    text.trim_end().to_string()
}

/// The file a command merely displays: `cat f`, `head -n 50 f`, `tail f`,
/// `sed -n '1,80p' f`, `cat f | head`. None when the command does anything
/// else with it.
fn viewed_file(cmd: &str) -> Option<String> {
    let cmd = cmd.trim();
    if cmd.contains("&&") || cmd.contains(';') || cmd.contains('>') || cmd.contains("<<") {
        return None;
    }
    let first = cmd.split('|').next()?.trim();
    let mut words = first.split_whitespace();
    let tool = words.next()?;
    if !matches!(
        tool,
        "cat" | "head" | "tail" | "sed" | "less" | "more" | "nl"
    ) {
        return None;
    }
    let files: Vec<&str> = words
        .filter(|w| {
            !w.starts_with('-')
                && !w.chars().all(|c| c.is_ascii_digit())
                && !(tool == "sed" && (w.contains('p') && w.contains(',') || w.starts_with('\'')))
        })
        .collect();
    match files.as_slice() {
        [one] if one.contains('.') || one.contains('/') => Some((*one).to_string()),
        _ => None,
    }
}

/// One giant line (minified JSON, a hex dump) still has to fit.
fn clip_line(l: &str) -> String {
    const MAX: usize = 2000;
    if l.len() <= MAX {
        return l.to_string();
    }
    let mut cut = MAX;
    while !l.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}… [{} more bytes on this line]", &l[..cut], l.len() - cut)
}

fn human_bytes(n: u64) -> String {
    if n >= 1 << 20 {
        format!("{:.1} MB", n as f64 / (1u64 << 20) as f64)
    } else if n >= 1 << 10 {
        format!("{} KB", n >> 10)
    } else {
        format!("{n} B")
    }
}

/// Kill the job and everything it spawned. The job leads its own process
/// group, so signalling the group reaches the whole tree.
///
/// This used to shell out to `kill -9 -<pid>`. Without `--`, procps `kill`
/// read the negative pid as an option and sent `kill(-1, SIGKILL)`: every
/// process the user may signal, twice on a production box (QA bug qa-020).
/// No shell here: the syscalls take the numbers as numbers. Pids 0 and 1
/// (and anything that does not fit) are refused outright, because
/// `kill(0)` and `killpg(0)` also mean "my whole group" or "everything".
pub fn kill_job(pid: u32) {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        eprintln!("kill_job: pid {pid} out of range; refusing");
        return;
    };
    if pid <= 1 {
        eprintln!("kill_job: pid {pid} is not a job; refusing");
        return;
    }
    // SAFETY: plain syscalls on a validated positive pid; no memory involved.
    let group_ok = unsafe { libc::killpg(pid, libc::SIGKILL) } == 0;
    if !group_ok {
        let _ = unsafe { libc::kill(pid, libc::SIGKILL) };
    }
}

#[cfg(test)]
mod kill_tests {
    use super::kill_job;
    use std::{os::unix::process::CommandExt, process::Command, time::Duration};

    fn alive(pid: u32) -> bool {
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }

    #[test]
    fn kill_job_ends_the_jobs_group_and_nothing_else() {
        // The job: a shell in its own process group with a child.
        let mut job = Command::new("sh")
            .args(["-c", "sleep 300 & wait"])
            .process_group(0)
            .spawn()
            .unwrap();
        // A bystander in a different group: what `kill -1` would have taken.
        let mut bystander = Command::new("sleep")
            .arg("300")
            .process_group(0)
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(200));

        kill_job(job.id());
        std::thread::sleep(Duration::from_millis(200));
        assert!(job.try_wait().unwrap().is_some(), "the job leader is dead");
        assert!(
            alive(bystander.id()),
            "a process outside the job's group must survive"
        );
        let _ = bystander.kill();
        let _ = bystander.wait();
    }

    #[test]
    fn kill_job_refuses_pids_that_mean_everything() {
        // 0 = own group, 1 = init; both must be no-ops. If either were
        // signalled, this test process would not be here to assert.
        kill_job(0);
        kill_job(1);
    }
}

/// Commands that ask the user first even in auto mode: wiping the root of
/// the filesystem (or a top-level directory of it), sudo, mkfs, a fork
/// bomb. `rm -rf /tmp/scratch` is an ordinary cleanup, not one of these;
/// the old substring test on `rm -rf /` stopped headless runs on exactly
/// that.
pub fn needs_approval(cmd: &str) -> bool {
    let c = cmd.to_ascii_lowercase();
    c.contains("sudo ") || c.contains("mkfs") || c.contains(":(){") || rm_wipes_root(&c)
}

/// An `rm` with a recursive flag whose target is `/`, `/*`, `~`, or a
/// top-level directory such as `/usr` or `/etc`.
fn rm_wipes_root(lower: &str) -> bool {
    for segment in lower.split(['|', ';', '&', '\n']) {
        let mut words = segment.split_whitespace();
        if words.next() != Some("rm") {
            continue;
        }
        let mut recursive = false;
        for w in words {
            if let Some(flags) = w.strip_prefix('-').filter(|f| !f.starts_with('-')) {
                recursive |= flags.contains('r');
                continue;
            }
            if w == "--recursive" || w == "-r" {
                recursive = true;
                continue;
            }
            if w.starts_with("--") {
                continue;
            }
            let target = w.trim_matches(['"', '\'']);
            let t = target.trim_end_matches('/');
            let top_level = t.starts_with('/')
                && !t[1..].is_empty()
                && !t[1..].contains('/')
                && !matches!(t, "/tmp" | "/var");
            let wipe = target == "/" || target == "/*" || t == "~" || t == "$home" || top_level;
            if recursive && wipe {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod boundary_tests {
    use super::build_or_test;

    /// Symmetry cycle 6: the coordinator ran `python3 -m pytest -q` itself.
    #[test]
    fn builds_and_test_runs_are_named_quick_commands_are_not() {
        for cmd in [
            "python3 -m pytest -q",
            "pytest tests/",
            "cd toy-repo && python3 -m pytest",
            "cargo test -p arbos-kernel",
            "RUST_LOG=debug cargo build --release",
            "npm test",
            "npm run build",
            "make -j4",
            "go test ./...",
            "time npx vitest",
        ] {
            assert!(build_or_test(cmd).is_some(), "{cmd}");
        }
        for cmd in [
            "ls -la",
            "python3 hello.py",
            "for i in 1 2 3; do echo step $i; sleep 2; done",
            "git status",
            "cat README.md | head",
            "cargo --version",
            "npm --version",
            "go version",
            "python3 -c 'print(1)'",
        ] {
            assert!(build_or_test(cmd).is_none(), "{cmd}");
        }
    }
}

#[cfg(test)]
mod approval_tests {
    use super::needs_approval;

    #[test]
    fn root_wipes_ask_and_scratch_cleanups_do_not() {
        for ask in [
            "rm -rf /",
            "rm -rf /*",
            "rm -rf ~",
            "cd /testbed && rm -rf /usr",
            "rm -r --no-preserve-root /etc",
            "sudo apt-get install x",
            "mkfs.ext4 /dev/sda1",
        ] {
            assert!(needs_approval(ask), "{ask:?} should ask");
        }
        for free in [
            "rm -rf /tmp/udltest && cd /testbed && git diff",
            "rm -rf build/ dist/",
            "rm -rf /var/tmp/x",
            "rm -rf /testbed/.pytest_cache",
            "rm -f /tmp/a.txt",
            "grep -r foo /",
            "python -c 'print(1)'",
        ] {
            assert!(!needs_approval(free), "{free:?} should run");
        }
    }
}

/// Commands whose first word is here never write the place.
const READONLY_CMDS: &[&str] = &[
    "ls",
    "cat",
    "head",
    "tail",
    "wc",
    "grep",
    "rg",
    "find",
    "fd",
    "pwd",
    "echo",
    "printf",
    "which",
    "file",
    "stat",
    "du",
    "tree",
    "env",
    "printenv",
    "true",
    "test",
    "type",
    "date",
    "uname",
    "whoami",
    "basename",
    "dirname",
    "realpath",
    "readlink",
    "sort",
    "uniq",
    "cut",
    "tr",
    "awk",
    "diff",
    "cmp",
    "md5",
    "md5sum",
    "shasum",
    "sha256sum",
    "jq",
    "yq",
    "less",
    "more",
    "nl",
    "od",
    "xxd",
    "strings",
    "column",
    "seq",
    "expr",
    "bc",
    "ps",
    "lsof",
];

const READONLY_GIT: &[&str] = &[
    "status",
    "log",
    "diff",
    "show",
    "branch",
    "blame",
    "rev-parse",
    "ls-files",
    "describe",
    "remote",
    "tag",
    "stash list",
    "config --get",
    "cat-file",
    "shortlog",
    "grep",
];

/// Tokens that mean "this might write". Any of them makes the command opaque.
const WRITE_MARKERS: &[&str] = &[
    ">", "$(", "`", "tee ", "xargs", "sudo ", "sed -i", "-exec", "-delete",
];

/// The build and test runners a coordinator hands to a worker: what the
/// command is, in words for the refusal, when its first program (after
/// `cd x &&`, env assignments, `time`, `nice`) is one of them.
pub fn build_or_test(cmd: &str) -> Option<&'static str> {
    for segment in cmd
        .split("&&")
        .flat_map(|s| s.split("||"))
        .flat_map(|s| s.split(';'))
    {
        let words: Vec<&str> = segment
            .split_whitespace()
            .skip_while(|w| w.contains('=') && !w.starts_with('-'))
            .skip_while(|w| matches!(*w, "time" | "nice" | "sudo" | "env"))
            .collect();
        let Some(first) = words.first() else {
            continue;
        };
        let prog = first.rsplit('/').next().unwrap_or(first);
        let second = words.get(1).copied().unwrap_or("");
        let what = match prog {
            "pytest" | "py.test" | "tox" | "nox" => Some("a test run"),
            "cargo"
                if matches!(
                    second,
                    "test" | "build" | "check" | "clippy" | "bench" | "nextest"
                ) =>
            {
                Some("a cargo build or test run")
            }
            "npm" | "pnpm" | "yarn" | "bun"
                if matches!(second, "test" | "run" | "build" | "ci" | "install") =>
            {
                Some("an npm build, install, or test run")
            }
            "npx" | "jest" | "vitest" | "mocha" | "playwright" | "cypress" => Some("a test run"),
            "make" | "cmake" | "ninja" | "gradle" | "gradlew" | "mvn" | "bazel" | "meson" => {
                Some("a build")
            }
            "go" if matches!(second, "test" | "build" | "vet") => Some("a go build or test run"),
            "python" | "python3" if matches!(second, "-m") => match words.get(2).copied() {
                Some("pytest" | "unittest" | "tox" | "nox" | "build") => Some("a test run"),
                _ => None,
            },
            "dotnet" if matches!(second, "test" | "build") => Some("a dotnet build or test run"),
            "swift" if matches!(second, "test" | "build") => Some("a swift build or test run"),
            "docker" if matches!(second, "build" | "compose") => Some("a docker build"),
            _ => None,
        };
        if what.is_some() {
            return what;
        }
    }
    None
}

/// Conservative. A wrong answer here is only ever "too cautious".
pub fn is_readonly_command(cmd: &str) -> bool {
    if cmd.trim().is_empty() || WRITE_MARKERS.iter().any(|m| cmd.contains(m)) {
        return false;
    }
    cmd.split(['\n', ';', '|'])
        .flat_map(|s| s.split("&&"))
        .flat_map(|s| s.split("||"))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .all(segment_is_readonly)
}

fn segment_is_readonly(seg: &str) -> bool {
    // Strip leading VAR=val assignments.
    let mut words = seg
        .split_whitespace()
        .skip_while(|w| w.contains('=') && !w.starts_with('-'));
    let Some(first) = words.next() else {
        return false;
    };
    let first = first.rsplit('/').next().unwrap_or(first);
    if first == "git" {
        let rest: Vec<&str> = words.collect();
        let sub = rest.join(" ");
        return READONLY_GIT
            .iter()
            .any(|g| sub == *g || sub.starts_with(&format!("{g} ")));
    }
    READONLY_CMDS.contains(&first)
}
