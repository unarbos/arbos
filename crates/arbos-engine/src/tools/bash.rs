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
/// The least an attached (foreground) call waits for its command before
/// handing it to a job, whatever `wait_ms` the model sent: a "run this
/// and show me the output" came back after the first line with
/// wait_ms=3000 and the user saw one line of eight (remote track, F-37).
/// A command that runs to its end within this is shown whole.
const ATTACHED_WAIT_FLOOR: Duration = Duration::from_secs(120);
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
                    "Attached wait, ms (default 600000; never under 120000 — a command the user asked to see runs to its end while they watch).",
                    false,
                    "integer",
                ),
                (
                    "background",
                    "Return at once; keep running as a job. For a server or watcher only (something that never ends by itself); a loop, script, build, or test run is not, and runs attached whatever this says.",
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
            // A file this turn wrote is never moved or deleted to satisfy
            // the brief's Output line: told its deliverable was "not
            // written yet" at the brief's path, a worker moved the user's
            // CHANGELOG.md out of their repository (qal-j04). The reminder
            // is bookkeeping; the file stays where the task put it.
            if let Some(why) = moves_a_delivered_file(&cx, cmd) {
                bail!("{why}");
            }
            // `kill <pid>` on a pid `jobs` showed: the kernel ends that
            // job — its whole process group, with a `killed` line that
            // says so — instead of the shell signalling the leash alone.
            // QA's 164 GB writer was exactly this: the model did the
            // obvious thing with the pid it was shown, the leash forwarded
            // the signal to the wrapper shell only, and the loop under it
            // lived on with no supervisor. (The leash now ends its group
            // on a signal too; this is the clean path with the clean
            // record.)
            if let Some(text) = kill_jobs_by_pid(&JobsRoot::for_agent(&cx.place, &cx.agent.id), cmd)
            {
                return Ok(ToolOut::text(text));
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
            // `background:true` is for a server. A loop, a script, a build
            // marked background came back after its first line ("step 1")
            // and the user saw one line of six (F-37, F-43): for anything
            // that is not a server the call stays attached to the floor.
            let asked_background = opt_bool(&args, "background").unwrap_or(false);
            let background = asked_background && looks_like_server(cmd);
            let background_ignored = asked_background && !background;
            let timeout_ms = opt_u64(&args, "timeout_ms");
            let wait = if background {
                BACKGROUND_GRACE
            } else if background_ignored {
                // Attached, but not for the whole default: a long build
                // the model wanted out of the way becomes a job at the floor.
                ATTACHED_WAIT_FLOOR
            } else {
                Duration::from_millis(opt_u64(&args, "wait_ms").unwrap_or(cx.bash_wait_ms))
                    .max(ATTACHED_WAIT_FLOOR)
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
            // The runtime's wait is signal-driven on macOS: a kernel whose
            // SIGCHLD never arrives (Jacob's Mac, 2026-09-17: five jobs
            // exited 0, five zombies, no tool result; `bubble_sort.py`
            // returned at the 600 s floor as "still running" with `exit`
            // long written) never hears from it. The wrapper's `exit`
            // file is the truth about the command, so the wait below also
            // watches for it, and reaps by pid.
            let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
            let waiter = tokio::spawn(async move {
                if std::env::var_os("ARBOS_TEST_NO_CHILD_WAIT").is_some() {
                    // Fault injection for the test of the exit-file path:
                    // a reaper that never wakes.
                    std::future::pending::<()>().await;
                }
                let _ = child.wait().await;
                let _ = done_tx.send(());
            });
            let job_pid = job.meta.pid;
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
            // The wait ends on the command, on `wait`, on Stop — or when
            // the user speaks: a steer is read at the tool boundary, and
            // an attached command can hold that boundary for ten minutes.
            // Jacob typed "run it" four times into a worker's
            // `python3 bubble_sort.py` and heard nothing for 2m 26s
            // (2026-09-16); the command keeps running as a job and the
            // turn answers now.
            let deadline = tokio::time::Instant::now() + wait;
            let mut tick = tokio::time::interval(Duration::from_millis(500));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut steered = false;
            let finished = loop {
                tokio::select! {
                    _ = &mut done_rx => break true,
                    _ = tokio::time::sleep_until(deadline) => break false,
                    _ = cx.cancel.cancelled() => {
                        // Stop while attached means stop: the user wants it gone.
                        if let Ok(j) = root.load(&job.id) {
                            root.kill(&j);
                        }
                        bail!("bash interrupted; job {} killed", job.id);
                    }
                    _ = tick.tick() => {
                        // The command's own end is the `exit` file. The
                        // leash stays behind it while children of the
                        // command still run in the group (a server the
                        // command backgrounded), so waiting on the leash
                        // alone would hold the call for those.
                        if root.load(&job.id).is_ok_and(|j| !j.running()) {
                            break true;
                        }
                        if arbos_core::inbox::has_user_steer(&cx.place, cx.agent.id.as_str()) {
                            steered = true;
                            break false;
                        }
                        // The command ended and the runtime did not say:
                        // the wrapper's `exit` file did. Reap the wrapper
                        // ourselves so no zombie is left under the kernel.
                        if root.load(&job.id).is_ok_and(|j| !j.running()) {
                            reap_by_pid(job_pid);
                            waiter.abort();
                            break true;
                        }
                    }
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
                let why = if steered {
                    " The user said something while it ran — it follows this result. Answer them, then follow the command with await."
                } else {
                    ""
                };
                return Ok(ToolOut::with_paths(
                    format!(
                        "{body}\n\n{verb} as job {id} (pid {pid}).{why} Follow with await {id} (optional regex pattern), list with jobs, stop with bash `kill -- -{pid}`. Log: {journal}",
                        id = job.id,
                        pid = job.meta.pid,
                    ),
                    vec![journal],
                ));
            }

            // Finished (or finished in the same instant the wait expired).
            let (text, skipped) = root.read_new(&job);
            let mut body = format_tail(&text, "(no output)", &journal, skipped);
            if background_ignored {
                body.push_str(
                    "\n[background:true is for a server; this command is not one, so it ran attached to its end and the output above is all of it]",
                );
            }
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

/// Whether `cmd` is the kind of command `background:true` exists for: a
/// server or watcher that would never end on its own. A loop, a script,
/// a test or build run is not — the model marks those background too,
/// and the user then sees one line of the output (F-37).
pub fn looks_like_server(cmd: &str) -> bool {
    let lower = cmd.to_ascii_lowercase();
    let trimmed = lower.trim_end_matches([' ', ';']);
    if trimmed.ends_with('&') || lower.contains("while true") || lower.contains("while :") {
        return true;
    }
    // A bare `sleep N`: a wait with nothing to show, not a loop of output.
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    if words.len() == 2 && words[0] == "sleep" {
        return true;
    }
    let markers = [
        "serve",
        "server",
        "--port",
        "-p ",
        "--host",
        "listen",
        "watch",
        "tail -f",
        "tail -F",
        "npm start",
        "npm run dev",
        "pnpm dev",
        "yarn dev",
        "yarn start",
        "vite",
        "uvicorn",
        "gunicorn",
        "flask run",
        "manage.py runserver",
        "rails s",
        "cargo run",
        "node ",
        "docker run",
        "docker compose up",
        "docker-compose up",
        "nohup",
        "daemon",
        "ngrok",
        "cloudflared",
        "ssh -n",
        "sleep infinity",
    ];
    markers.iter().any(|m| lower.contains(m))
}

/// The build and test runners a coordinator hands to a worker: what the
/// command is, in words for the refusal, when its first program (after
/// `cd x &&`, env assignments, `time`, `nice`) is one of them.
/// After an "output owed" reminder this turn, a `mv`, `rm`, `git mv` or
/// `git rm` whose operand is a file the turn wrote (or shares its name
/// with an owed path) is refused with the reason. Only then: the
/// guard is for the one shape that lost a user's file, not for every
/// move a worker makes.
/// `kill`, an optional signal, and one or more positive pids, nothing
/// else: the pids. `kill -- -123` (a group) and anything compound are
/// left to the shell.
fn kill_of_pids(cmd: &str) -> Option<Vec<u32>> {
    let mut words = cmd.split_whitespace();
    if words.next()? != "kill" {
        return None;
    }
    let mut pids = Vec::new();
    for (i, w) in words.enumerate() {
        if i == 0 && w.starts_with('-') && !w.starts_with("--") {
            // `-9`, `-TERM`, `-s`… a signal; `-s SIG` would leave SIG as
            // a non-number below and bail out.
            continue;
        }
        pids.push(w.parse::<u32>().ok().filter(|&p| p > 1)?);
    }
    (!pids.is_empty()).then_some(pids)
}

/// `kill <pid…>` where every pid is a running job of this agent: the
/// kernel's kill on each (the whole group, a `killed` line in the folder)
/// and the result's text. None when the command is anything else, or any
/// pid is not a job: the shell runs it as written.
pub(crate) fn kill_jobs_by_pid(root: &JobsRoot, cmd: &str) -> Option<String> {
    let pids = kill_of_pids(cmd)?;
    let jobs: Vec<Job> = root
        .list()
        .into_iter()
        .filter(|j| j.running() && pids.contains(&j.meta.pid))
        .collect();
    if jobs.len() != pids.len() {
        return None;
    }
    let lines: Vec<String> = jobs
        .iter()
        .map(|j| {
            root.kill(j);
            format!(
                "job {} (pid {}) ended — the whole process group, not only its shell: `{}`",
                j.id,
                j.meta.pid,
                arbos_core::text::clip(j.meta.command.trim(), 80)
            )
        })
        .collect();
    Some(lines.join("\n"))
}

#[cfg(test)]
mod kill_by_pid_tests {
    use super::{kill_jobs_by_pid, kill_of_pids};
    use std::time::Duration;

    #[test]
    fn the_parser_takes_kill_and_pids_only() {
        assert_eq!(kill_of_pids("kill 123"), Some(vec![123]));
        assert_eq!(kill_of_pids("kill -9 123 456"), Some(vec![123, 456]));
        assert_eq!(kill_of_pids("kill -TERM 123"), Some(vec![123]));
        assert_eq!(kill_of_pids("kill -- -123"), None, "a group: the shell's");
        assert_eq!(kill_of_pids("kill 1"), None, "never pid 1");
        assert_eq!(kill_of_pids("kill $(cat pid)"), None);
        assert_eq!(kill_of_pids("kill 123; echo done"), None);
        assert_eq!(kill_of_pids("pkill yes"), None);
    }

    #[tokio::test]
    async fn a_kill_on_a_jobs_pid_ends_its_whole_group_with_a_line() {
        let dir = std::env::temp_dir().join(format!(
            "arbos-kill-by-pid-{}-{}",
            std::process::id(),
            arbos_core::now_ms()
        ));
        std::fs::create_dir_all(dir.join(".arbos/agents/root/jobs")).unwrap();
        let root = crate::JobsRoot::new(dir.join(".arbos/agents/root/jobs"));
        let (job, mut child) = root
            .spawn(
                "(while :; do sleep 0.1; done) & sleep 300",
                &dir,
                None,
                None,
                vec![],
            )
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            kill_jobs_by_pid(&root, "kill 999999").is_none(),
            "not a job"
        );
        assert!(
            kill_jobs_by_pid(&root, &format!("kill {} 999999", job.meta.pid)).is_none(),
            "one of them is not a job: the shell's"
        );
        let text = kill_jobs_by_pid(&root, &format!("kill {}", job.meta.pid)).expect("routed");
        assert!(text.contains("the whole process group"), "{text}");
        // The leash is our child: reaped here, or it counts as alive.
        let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let members = std::process::Command::new("pgrep")
                .args(["-g", &job.meta.pid.to_string()])
                .output()
                .map(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .lines()
                        .filter_map(|l| l.trim().parse::<u32>().ok())
                        .filter(|&p| unsafe { libc::kill(p as libc::pid_t, 0) } == 0)
                        .count()
                })
                .unwrap_or(0);
            if members == 0 {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "the group lived on");
            std::thread::sleep(Duration::from_millis(100));
        }
        let killed = std::fs::read_to_string(job.dir.join("killed")).unwrap();
        assert!(killed.contains("killed by the kernel"), "{killed}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

fn moves_a_delivered_file(cx: &RunCx, cmd: &str) -> Option<String> {
    let toks: Vec<&str> = cmd.split_whitespace().collect();
    let moving = toks.iter().any(|t| matches!(*t, "mv" | "rm" | "unlink"));
    if !moving {
        return None;
    }
    let transcript = arbos_core::Layout::new(&cx.place, cx.agent.id.as_str()).transcript();
    let events = arbos_core::load_transcript(&transcript).ok()?;
    let start = events.iter().rposition(arbos_core::Event::is_wake)?;
    let turn = &events[start..];
    let nudged = turn.iter().any(|e| {
        matches!(&e.kind, arbos_core::EventKind::Nudge { reason, .. } if reason == "output owed")
    });
    if !nudged {
        return None;
    }
    let mut protected: Vec<String> = crate::turn::written_this_turn(turn)
        .iter()
        .map(|p| {
            p.rsplit(['/', '\\'])
                .next()
                .unwrap_or(p)
                .to_ascii_lowercase()
        })
        .collect();
    if let arbos_core::EventKind::Wake { text: Some(t), .. } = &events[start].kind {
        protected.extend(
            crate::turn::brief_output_paths(t)
                .iter()
                .map(|p| p.rsplit('/').next().unwrap_or(p).to_ascii_lowercase()),
        );
    }
    let hit = toks.iter().find(|t| {
        let base = t
            .trim_matches(['"', '\''])
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        !base.is_empty() && protected.contains(&base)
    })?;
    Some(format!(
        "bash: refused — this moves or deletes {hit}, a file this turn delivered, after a reminder about the brief's Output path. The reminder is bookkeeping, never a reason to relocate a user's file: leave {hit} where the task put it and name that path in your report (the brief's Output line is satisfied by the file existing)."
    ))
}

/// `waitpid(pid, WNOHANG)`: clear a child that has exited when the
/// runtime's own reaper did not — a zombie is what the process table
/// shows otherwise, and it is what Jacob's Mac showed.
pub fn reap_by_pid(pid: u32) {
    #[cfg(unix)]
    unsafe {
        let mut status: libc::c_int = 0;
        let _ = libc::waitpid(pid as libc::pid_t, &mut status, libc::WNOHANG);
    }
    #[cfg(not(unix))]
    let _ = pid;
}

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

#[cfg(test)]
mod background_tests {
    use super::looks_like_server;

    /// F-37 / F-43: a six-step loop marked `background:true` came back
    /// after "step 1". Only a server or watcher is a background command.
    #[test]
    fn only_a_server_or_watcher_is_background() {
        for cmd in [
            "for i in 1 2 3 4 5 6; do echo \"step $i\"; sleep 2; done",
            "python3 script.py",
            "cargo test",
            "make build",
            "ls -la && git log --oneline | head",
            "sleep 12; echo done",
        ] {
            assert!(!looks_like_server(cmd), "{cmd}");
        }
        for cmd in [
            "python3 -m http.server 8000",
            "npm run dev",
            "uvicorn app:app --port 8080",
            "cargo run --bin arbos-kernel serve .",
            "tail -f /var/log/syslog",
            "while true; do date; sleep 5; done",
            "node index.js &",
            "docker compose up",
            "sleep 600",
        ] {
            assert!(looks_like_server(cmd), "{cmd}");
        }
    }
}
