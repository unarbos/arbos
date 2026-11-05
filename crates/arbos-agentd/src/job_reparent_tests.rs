//! Bugs Z and AE, Test5 round 13 (agentd 0.1.196 `508151f6a`, side chat
//! 01M6ME9A7EGV5PJ7DZW43AHTN5, job 01M6PPWGG29TQMZENSB8K861Q1 "Running tick13 ticker").
//!
//! The job's `bash` tool call backgrounded a tree —
//! `(bash tmp/tick13.sh > tmp/tick13.run.out 2>&1; echo exit >> …) &` — and the shell that
//! started it returned. Root shell 1698 was reparented to PID 1 with 1700, 1701 and 1702
//! beneath it. `stop_job` at 14:37:59 CR marked the job `cancelled`, and all three ran to
//! 60/60 anyway: 12 lines at 20:38:33Z, 15 at 20:38:48Z, 45 at 20:41:20Z.
//!
//! Then the second half. `stop_job`'s own output said "every process it had started is dead"
//! without having checked, and that claim disarmed the one way out: the bash guard refused the
//! agent's `kill 1700 1701 1702` on the grounds that only a pid `stop_job` reports it could NOT
//! kill may be signalled — and `stop_job` had reported none. A second `stop_job` found nothing
//! to cancel. Three runaway processes, a correct guard, and no way to act.
//!
//! ## Why these tests background the tree
//!
//! The same job shape ran twice in round 12 in the foreground and was cancelled cleanly both
//! times, so a foreground test passes against the broken code and proves nothing. Backgrounding
//! changes two things at once, and both matter:
//!
//! 1. the tree is **orphaned** — the shell that forked it returns, so the descendants are
//!    reparented to PID 1 and no parent link leads back to the job;
//! 2. its output goes to a **file**, so nothing closes under it when the worker dies. A
//!    foreground tool call writes down a pipe to the harness child, and killing that child
//!    ends the writer on its next write.
//!
//! Neither of those is reached by a kill aimed at the worker's pid, nor by a walk down from it.
//!
//! These tests spawn real processes and read `/proc`, because nothing else can tell a kill that
//! worked from one that was only issued. They are Linux-only and are skipped elsewhere.

#![cfg(target_os = "linux")]

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use arbos_core::doc::UserMessageOpts;
use serde_json::json;

use crate::agent::Agent;
use crate::isolation::WORKER_MARK_ENV;
use crate::status::tests::test_agent;

const CHAT: &str = "01TESTCHATJOBREPARENT00001";
const JOB: &str = "01TESTJOBREPARENTTICK13001";
const WORKER: &str = "01TESTWORKERJOBREPARENT001";
const ASK: &str = "01TESTASKJOBREPARENT000001";

/// The cancel's budget: within five seconds nothing of the job is alive and its log stops
/// growing.
const BUDGET: Duration = Duration::from_secs(5);

/// Fast enough that "the file stopped growing" is measurable inside the budget, and slow enough
/// that the loop is reliably mid-`sleep` rather than mid-write when the cancel lands.
const TICK: &str = "0.2";

/// A worker as the box leaves one, carrying round 13's shape.
///
/// The root shell leads a **session** of its own (`setsid`), which is what `RpcChild::spawn`
/// does, and carries [`WORKER_MARK_ENV`], which every process it starts inherits.
///
/// Its tool call then backgrounds a tick loop through an intermediate shell that exits at once,
/// with job control on so the loop lands in a process **group** of its own — which is what a
/// tool runner does to its commands so it can time them out. The loop therefore ends up:
///
/// - reparented to PID 1, like shell 1698;
/// - in a process group that is not the worker's, so the recorded pgid does not name it;
/// - in the worker's session, which is the one link left;
/// - writing to a file, so nothing closes under it.
struct Worker {
    /// The root shell's pid, which `setsid` makes its session id and its process group id too.
    leader: u32,
    log: PathBuf,
    /// The backgrounded loop's own pid, which it writes out for the assertions below.
    loop_pid: u32,
    /// Held only so [`Drop`] can reap it. A killed child stays a zombie until its parent waits,
    /// and this test process is that parent — the box's real workers are reparented to PID 1,
    /// which reaps them itself.
    shell: std::process::Child,
}

impl Worker {
    fn start(dir: &Path) -> Self {
        let log = dir.join("tick13.log");
        let pid_file = dir.join("tick13.pid");
        let loop_body = format!(
            "echo \\$\\$ > {}; for k in \\$(seq 1 400); do sleep {TICK}; echo tick >> {}; done",
            pid_file.display(),
            log.display()
        );
        // `set -m` gives the backgrounded job a process group of its own. The middle shell
        // exits the moment it has forked the loop, which is what orphans the loop; the root
        // shell stays alive, as a worker does until the cancel reaches it.
        let script = format!(
            "set -m\nbash -c '( bash -c \"{loop_body}\" > {}.out 2>&1 & )' &\nsleep 600",
            log.display()
        );
        let mut cmd = Command::new("bash");
        cmd.arg("-c")
            .arg(&script)
            .env(WORKER_MARK_ENV, WORKER)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        own_session(&mut cmd);
        let child = cmd.spawn().expect("the worker shell starts");
        let leader = child.id();
        // The loop is running before anything is cancelled: a test that cancelled an idle tree
        // would pass against the unfixed code.
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(10) {
            if lines(&log) > 0 && pid_in(&pid_file).is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let loop_pid = pid_in(&pid_file)
            .expect("the backgrounded loop never recorded its pid, so this test proves nothing");
        assert!(
            lines(&log) > 0,
            "the worker's loop never wrote a line, so this test would prove nothing"
        );
        Self {
            leader,
            log,
            loop_pid,
            shell: child,
        }
    }

    /// The round-13 shape, asserted rather than assumed. Each of these is part of what made the
    /// cancel miss, and a test that ran without them would pass against the broken code.
    fn assert_is_round_13_shape(&self) {
        assert_eq!(
            ppid_of(self.loop_pid),
            Some(1),
            "the loop is not orphaned, so no reparenting is being tested"
        );
        assert_ne!(
            pgid_of(self.loop_pid),
            Some(self.leader),
            "the loop is in the worker's own process group, so the recorded pgid alone would \
             find it and this test would pass against the unfixed code"
        );
        assert_eq!(
            sid_of(self.loop_pid),
            Some(self.leader),
            "the loop left the worker's session, which is not the shape round 13 had"
        );
    }
}

impl Drop for Worker {
    /// A failed assertion must not leave a tick loop running on the box for the rest of the
    /// suite — which is, after all, the bug. The loop is in neither the test runner's group nor
    /// its process tree, so it is killed by pid.
    fn drop(&mut self) {
        for pid in [self.loop_pid, self.leader] {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid as i32),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(self.leader as i32),
            nix::sys::signal::Signal::SIGKILL,
        );
        let _ = self.shell.wait();
    }
}

/// Start this child in a session of its own, as `RpcChild::spawn` does.
fn own_session(cmd: &mut Command) {
    unsafe {
        cmd.pre_exec(|| {
            nix::libc::setsid();
            Ok(())
        });
    }
}

fn lines(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

fn pid_in(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// `/proc/<pid>/stat` after the last `)`: state, ppid, pgrp, session.
fn stat_field(pid: u32, n: usize) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after = stat.rsplit_once(')').map(|(_, rest)| rest)?;
    after.split_whitespace().nth(n)?.parse().ok()
}

fn ppid_of(pid: u32) -> Option<u32> {
    stat_field(pid, 1)
}

fn pgid_of(pid: u32) -> Option<u32> {
    stat_field(pid, 2)
}

fn sid_of(pid: u32) -> Option<u32> {
    stat_field(pid, 3)
}

fn alive(pid: u32) -> bool {
    stat_field(pid, 1).is_some()
}

/// The job as `run_subagent` leaves one, with the worker's session recorded as the box records
/// it.
async fn a_running_job(agent: &std::sync::Arc<Agent>, worker: &Worker) {
    let host = agent.sync.open_room(CHAT).await.unwrap();
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(
                ASK,
                "run the tick13 ticker, 60 iterations",
                "dev-mac",
                &UserMessageOpts::default(),
            )
            .unwrap();
    }
    agent
        .store
        .insert_job(
            JOB,
            CHAT,
            ASK,
            "Running tick13 ticker",
            "code",
            "Run tmp/tick13.sh, 60 iterations.",
            &[],
        )
        .unwrap();
    agent
        .store
        .insert_subagent(WORKER, "job-tick13", "run the ticker", "pi", CHAT)
        .unwrap();
    agent.store.set_job_subagent(JOB, WORKER).unwrap();
    agent.store.set_job_state(JOB, "running", None).unwrap();
    agent.note_worker_pgid(WORKER, worker.leader).await;
}

/// **Bug Z.** The cancel reaches a tree the job backgrounded, orphaned and regrouped.
///
/// The assertion that fails against the unfixed code is the log one: on 0.1.196 the recorded
/// process group named the worker's own shell and nothing else, the loop was in a group of its
/// own with PID 1 for a parent, and it went on writing to 60/60 while the job read `cancelled`.
#[tokio::test]
async fn cancelling_a_job_kills_the_backgrounded_tree_it_orphaned() {
    let (agent, root) = test_agent().await;
    let worker = Worker::start(&root);
    worker.assert_is_round_13_shape();
    a_running_job(&agent, &worker).await;

    let t0 = Instant::now();
    let stopped = agent.stop_jobs(CHAT, Some(JOB)).await;
    let took = t0.elapsed();

    assert_eq!(stopped.len(), 1);
    assert_eq!(
        stopped[0].state, "cancelled",
        "#2779's half must still hold"
    );
    assert!(
        took < BUDGET,
        "the cancel took {took:?}, past the five seconds a user waits"
    );

    // The log stops growing. This is round 13's own measure — 12 lines, then 15, then 45 — and
    // it is what the cancel is for.
    let at_cancel = lines(&worker.log);
    std::thread::sleep(Duration::from_millis(1_500)); // many ticks at TICK = 0.2 s
    assert_eq!(
        lines(&worker.log),
        at_cancel,
        "the log grew after the cancel: the orphaned loop (pid {}) is still running",
        worker.loop_pid
    );

    // Measured against the kernel rather than against what the cancel believes it did.
    assert!(
        !alive(worker.loop_pid),
        "pid {} outlived the cancel",
        worker.loop_pid
    );
    assert_eq!(
        stopped[0].sweep.survivors,
        Vec::<u32>::new(),
        "the cancelled job left processes running: {:?}",
        stopped[0].sweep
    );
    assert!(
        stopped[0].sweep.killed > 0,
        "the cancel killed nothing and still called itself a cancel: {:?}",
        stopped[0].sweep
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// **Bug AE.** A process of the job that no set of the kernel's still holds is named by pid,
/// rather than covered by a claim that everything is dead.
///
/// The escapee here has called `setsid`, so of the worker's cgroup (there is none on a box
/// without isolation), session and group it is in none. What it does carry is the worker's
/// mark, inherited through fork and exec, and that is what the cancel reads back. It is
/// spawned by the test rather than by the worker because what the cancel reads are the pid's
/// own three facts — its session, its group and its environment — and not how it came by them;
/// a `setsid` inside a tool call leaves exactly this.
///
/// Against the unfixed code `processes_still_alive` was `[]` and the note said "every process
/// it had started is dead": a sweep with nothing in its handle reporting a clean cancel. The
/// claim is the bug, and the pid list is the fix.
#[tokio::test]
async fn a_process_the_cancel_could_not_reap_is_named_by_pid() {
    let (agent, root) = test_agent().await;
    let worker = Worker::start(&root);
    a_running_job(&agent, &worker).await;
    let mut escapee = Escapee::start(&root);
    let turn = agent.test_hold_turn(CHAT).await;

    let result = agent
        .hook_stop_job(JOB)
        .await
        .expect("the job is stoppable");
    let job = &result["stopped"][0];
    assert_eq!(job["state"], json!("cancelled"));

    // The claim, and what it now rests on.
    assert_eq!(
        job["processes_checked"],
        json!(true),
        "a cancel must say whether it read the kernel at all: {result}"
    );
    let reported: Vec<u64> = job["processes_still_alive"]
        .as_array()
        .expect("stop_job must answer with a survivor list")
        .iter()
        .filter_map(serde_json::Value::as_u64)
        .collect();
    assert!(
        alive(escapee.pid),
        "the premise of this test is a process the cancel could not reap"
    );
    assert!(
        reported.contains(&u64::from(escapee.pid)),
        "pid {} outlived the cancel and stop_job did not name it: {result}",
        escapee.pid
    );
    assert_eq!(
        result["all_cancelled"],
        json!(false),
        "a job with a process still running is not wholly cancelled: {result}"
    );

    // What the model is told, and what the user reads. Both name the pid, because a pid is what
    // either of them can act on.
    let note = result["note"].as_str().unwrap_or_default();
    assert!(
        note.contains(&escapee.pid.to_string()) && note.contains("did NOT die"),
        "the note must name the pid that outlived the cancel: {note}"
    );
    assert!(
        !note.contains("every process it had started"),
        "and must not repeat round 13's claim: {note}"
    );

    // The escape hatch round 13 did not have: the guard's existing exception now applies,
    // because stop_job has reported a pid it could not kill.
    agent
        .hook_kill_allowed(&format!("kill -9 {}", escapee.pid))
        .await
        .expect("a kill aimed at a reported survivor of this chat is allowed");

    // And the guard is not weakened. Everything it refused before, it refuses still.
    for command in [
        format!("kill {}", escapee.pid + 1),
        format!("kill {} {}", escapee.pid, escapee.pid + 1),
        "pkill -f tick13".to_owned(),
        "killall bash".to_owned(),
        format!("kill -9 -{}", escapee.pid),
        "kill $(pgrep -f tick13)".to_owned(),
    ] {
        let refused = agent
            .hook_kill_allowed(&command)
            .await
            .expect_err("this command must still be refused");
        assert!(
            refused.contains("not a cancel"),
            "`{command}` must still be refused, got: {refused}"
        );
    }

    escapee.reap();
    drop(turn);
    let _ = std::fs::remove_dir_all(&root);
}

/// A process of the job that has left every set the kernel keeps for it: its own session, its
/// own group, no cgroup — and the worker's mark in its environment.
struct Escapee {
    pid: u32,
    child: std::process::Child,
}

impl Escapee {
    fn start(dir: &Path) -> Self {
        let mut cmd = Command::new("bash");
        cmd.arg("-c")
            .arg(format!(
                "for k in $(seq 1 400); do sleep {TICK}; echo tick >> {}; done",
                dir.join("escapee.log").display()
            ))
            .env(WORKER_MARK_ENV, WORKER)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        own_session(&mut cmd);
        let child = cmd.spawn().expect("the escapee starts");
        let pid = child.id();
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(5) && sid_of(pid) != Some(pid) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            sid_of(pid),
            Some(pid),
            "the escapee did not start a session of its own, so it has escaped nothing"
        );
        Self { pid, child }
    }

    fn reap(&mut self) {
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(self.pid as i32),
            nix::sys::signal::Signal::SIGKILL,
        );
        let _ = self.child.wait();
    }
}

impl Drop for Escapee {
    fn drop(&mut self) {
        self.reap();
    }
}
