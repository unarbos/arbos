//! Bugs Z and AE, Test5 rounds 13 and 14, and the shape round 15 is aimed at.
//!
//! **Round 13** (agentd 0.1.196 `508151f6a`, job 01M6PPWGG29TQMZENSB8K861Q1). The job's `bash`
//! tool call backgrounded a tree — `(bash tmp/tick13.sh > tmp/tick13.run.out 2>&1; …) &` — and
//! the shell that started it returned. Root shell 1698 was reparented to PID 1 with 1700, 1701
//! and 1702 beneath it. `stop_job` at 14:37:59 CR marked the job `cancelled`, and all three ran
//! to 60/60 anyway: 12 lines at 20:38:33Z, 15 at 20:38:48Z, 45 at 20:41:20Z.
//!
//! **Round 14** (0.1.197 `ef90488f8`, job 01M6PVZF7FD1SPRSC1TAWC630G). A four-level tree, 2571,
//! 2572, 2574 and 2576, in which the deepest called `setsid` and led session 2576. `stop_job` at
//! 16:06:56 marked the job cancelled and all four kept writing — 5 to 7 lines in ten seconds,
//! later 21 to 25 — until a human approved a `kill`, after which the logs froze at 26/80.
//!
//! The two escapes are different, and a fix for one is not a fix for the other. Round 13's tree
//! left the **process group** by being backgrounded under job control and lost its parent link
//! by being orphaned. Round 14's deepest process left the **session** by its own `setsid`. Round
//! 15 stacks `setsid` on `nohup` five levels deep, which is why
//! `cancelling_a_job_kills_a_tree_that_stacks_setsid_on_nohup` is here before that round is
//! scored rather than after.
//!
//! **Bug AE**, both rounds: `stop_job`'s output said "every process it had started is dead"
//! without having checked, and that claim disarmed the one way out. The bash guard refused the
//! agent's `kill 1700 1701 1702` because only a pid `stop_job` reports it could NOT kill may be
//! signalled, and `stop_job` had reported none. The agent was honest and asked; the tool had
//! lied to it. So the guard is right and its input was wrong, and these tests pin the input.
//!
//! ## Why these tests background the tree
//!
//! Z passed twice in round 12 with a foreground tree and regressed the moment one was
//! backgrounded, so a foreground test passes against the broken code and proves nothing.
//! Backgrounding changes two things at once, and both matter:
//!
//! 1. the tree is **orphaned** — the shell that forked it returns, so the descendants are
//!    reparented to PID 1 and no parent link leads back to the job;
//! 2. its output goes to a **file**, so nothing closes under it when the worker dies. A
//!    foreground tool call writes down a pipe to the harness child, and killing that child ends
//!    the writer on its next write.
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

use crate::agent::{Agent, StoppedJob};
use crate::isolation::{ProcessTree, Sweep, WORKER_MARK_ENV};
use crate::status::tests::test_agent;

const CHAT: &str = "01TESTCHATJOBREPARENT00001";
const JOB: &str = "01TESTJOBREPARENTTICK13001";
const ASK: &str = "01TESTASKJOBREPARENT000001";

/// A worker **session id** per test, because the mark is read out of the box's process table
/// and that is one namespace for the whole suite. Two tests sharing one would each find the
/// other's processes and report them as survivors of its own job — a flake, and one that would
/// look exactly like the bug.
///
/// The mark a worker actually carries is [`planted_mark`], not this. Until bug AI these
/// fixtures planted the bare session id, which is what the cancel looked for and **not** what
/// `RpcChild::spawn` writes — so the cases below passed against an environment production
/// never creates, and the two `setsid` shapes, the only ones that depend on the mark, went
/// green over a handle that had never matched a real process.
const WORKER_R13: &str = "01TESTWORKERREPARENTKILL01";
const WORKER_R14: &str = "01TESTWORKERSETSIDCHILD001";
const WORKER_R15: &str = "01TESTWORKERSETSIDNOHUP001";
const WORKER_READBACK: &str = "01TESTWORKERREADBACKPID001";

/// The cancel's budget: within five seconds nothing of the job is alive and its logs stop
/// growing.
const BUDGET: Duration = Duration::from_secs(5);

/// Fast enough that "the file stopped growing" is measurable inside the budget, and slow enough
/// that a loop is reliably mid-`sleep` rather than mid-write when the cancel lands.
const TICK: &str = "0.2";

/// Many ticks at [`TICK`], so "it stopped writing" is measured rather than assumed from a pause.
const SETTLE: Duration = Duration::from_millis(1_500);

// ---- the worker shapes -------------------------------------------------------------------

/// Round 13's worker: a tick loop backgrounded, orphaned onto PID 1, and in a process group
/// that is not the worker's.
///
/// The root shell leads a **session** of its own (`setsid`) and carries [`WORKER_MARK_ENV`],
/// which is what `RpcChild::spawn` does and what every process it starts inherits. Its tool
/// call backgrounds the loop through an intermediate shell that exits at once, with job control
/// on so the loop lands in a process group of its own — which is what a tool runner does to its
/// commands so it can time them out.
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
    fn start(dir: &Path, mark: &str) -> Self {
        let log = dir.join("tick13.log");
        let pid_file = dir.join("tick13.pid");
        let loop_body = format!(
            "echo \\$\\$ > {}; for k in \\$(seq 1 400); do sleep {TICK}; echo tick >> {}; done",
            pid_file.display(),
            log.display()
        );
        // `set -m` gives the backgrounded job a process group of its own. The middle shell exits
        // the moment it has forked the loop, which is what orphans the loop; the root shell
        // stays alive, as a worker does until the cancel reaches it.
        let script = format!(
            "set -m\nbash -c '( bash -c \"{loop_body}\" > {}.out 2>&1 & )' &\nsleep 600",
            log.display()
        );
        let shell = spawn_worker_root(&script, mark);
        let leader = shell.id();
        wait_until(Duration::from_secs(10), || {
            lines(&log) > 0 && pid_in(&pid_file).is_some()
        });
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
            shell,
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
    /// suite — which is, after all, the bug.
    fn drop(&mut self) {
        reap(&[self.loop_pid, self.leader]);
        let _ = self.shell.wait();
    }
}

/// Rounds 14 and 15: a worker whose tool call builds a tree several levels deep, each level
/// ticking into a log of its own and launching the next.
///
/// `launchers` is how each level starts the one below it — `"setsid"` for a level that leads a
/// session of its own, `"nohup"` for one that only ignores `SIGHUP`, `""` for a plain
/// background child. The tree is one level longer than the launcher list.
struct DeepWorker {
    leader: u32,
    dir: PathBuf,
    /// One pid per level, innermost last.
    pids: Vec<u32>,
    shell: std::process::Child,
}

impl DeepWorker {
    fn start(dir: &Path, mark: &str, launchers: &[&str]) -> Self {
        assert!(
            Path::new("/usr/bin/setsid").exists(),
            "setsid (util-linux) is missing, so the escape these tests measure cannot be built"
        );
        let levels = launchers.len() + 1;
        for d in 1..=levels {
            let mut body = format!("echo $$ > {}\n", level_pid(dir, d).display());
            if d < levels {
                body.push_str(&format!(
                    "{} bash {} >/dev/null 2>&1 &\n",
                    launchers[d - 1],
                    level_script(dir, d + 1).display()
                ));
            }
            body.push_str(&format!(
                "for k in $(seq 1 400); do sleep {TICK}; echo tick >> {}; done\n",
                level_log(dir, d).display()
            ));
            std::fs::write(level_script(dir, d), body).expect("the level script is written");
        }
        // `set -m` puts the tool call's own shell in a process group of its own, as a tool
        // runner does; the root shell then stays alive, as a worker does until the cancel.
        let script = format!(
            "set -m\nbash {} &\nsleep 600\n",
            level_script(dir, 1).display()
        );
        let shell = spawn_worker_root(&script, mark);
        let leader = shell.id();
        wait_until(Duration::from_secs(20), || {
            (1..=levels)
                .all(|d| lines(&level_log(dir, d)) > 0 && pid_in(&level_pid(dir, d)).is_some())
        });
        let pids: Vec<u32> = (1..=levels)
            .map(|d| {
                pid_in(&level_pid(dir, d)).unwrap_or_else(|| {
                    panic!("level {d} never started, so this test proves nothing")
                })
            })
            .collect();
        for d in 1..=levels {
            assert!(
                lines(&level_log(dir, d)) > 0,
                "level {d} never wrote a line, so a cancel of it would prove nothing"
            );
        }
        Self {
            leader,
            dir: dir.to_owned(),
            pids,
            shell,
        }
    }

    /// The escape round 14 found: a descendant that left the worker's session by its own
    /// `setsid`, so no session, group or parent link of the worker names it any more.
    fn assert_a_descendant_leads_its_own_session(&self) {
        let escaped: Vec<u32> = self
            .pids
            .iter()
            .copied()
            .filter(|p| sid_of(*p) == Some(*p) && *p != self.leader)
            .collect();
        assert!(
            !escaped.is_empty(),
            "no level of this tree leads a session of its own, so the round-14 escape is not \
             being tested: {:?}",
            self.sessions()
        );
        for pid in escaped {
            assert_ne!(
                sid_of(pid),
                Some(self.leader),
                "pid {pid} is still in the worker's session"
            );
        }
    }

    fn sessions(&self) -> Vec<(u32, Option<u32>)> {
        self.pids.iter().map(|p| (*p, sid_of(*p))).collect()
    }

    fn total_lines(&self) -> usize {
        (1..=self.pids.len())
            .map(|d| lines(&level_log(&self.dir, d)))
            .sum()
    }

    fn still_alive(&self) -> Vec<u32> {
        self.pids.iter().copied().filter(|p| alive(*p)).collect()
    }
}

impl Drop for DeepWorker {
    fn drop(&mut self) {
        let mut pids = self.pids.clone();
        pids.push(self.leader);
        reap(&pids);
        let _ = self.shell.wait();
    }
}

fn level_script(dir: &Path, depth: usize) -> PathBuf {
    dir.join(format!("level{depth}.sh"))
}

fn level_pid(dir: &Path, depth: usize) -> PathBuf {
    dir.join(format!("level{depth}.pid"))
}

fn level_log(dir: &Path, depth: usize) -> PathBuf {
    dir.join(format!("level{depth}.log"))
}

/// A worker root as `RpcChild::spawn` leaves one: its own session, and the mark its descendants
/// inherit.
/// The mark `RpcChild::spawn` would have given a worker of this session, so these fixtures
/// plant the environment the box really creates rather than the one the cancel used to expect.
fn planted_mark(sid: &str) -> String {
    crate::harness::worker_name("pi", sid)
}

fn spawn_worker_root(script: &str, mark: &str) -> std::process::Child {
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(script)
        .env(WORKER_MARK_ENV, mark)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    own_session(&mut cmd);
    cmd.spawn().expect("the worker shell starts")
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

// ---- reading the kernel ------------------------------------------------------------------

fn lines(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

fn pid_in(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn wait_until(budget: Duration, mut ready: impl FnMut() -> bool) {
    let started = Instant::now();
    while started.elapsed() < budget && !ready() {
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn reap(pids: &[u32]) {
    for pid in pids {
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(*pid as i32),
            nix::sys::signal::Signal::SIGKILL,
        );
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(*pid as i32),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
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

/// Alive, and not a zombie waiting to be reaped.
///
/// The distinction is not pedantry here. A killed process keeps its `/proc` entry until its
/// parent waits, and a test whose own children are the tree is that parent — so counting a
/// zombie as alive would report a cancel that worked perfectly as a failure. The box's workers
/// are reparented to PID 1, which reaps them itself, which is why production draws the same
/// line (`isolation::is_running`).
fn alive(pid: u32) -> bool {
    !matches!(state_of(pid), None | Some('Z') | Some('X'))
}

fn state_of(pid: u32) -> Option<char> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after = stat.rsplit_once(')').map(|(_, rest)| rest)?;
    after.split_whitespace().next()?.chars().next()
}

// ---- the job -----------------------------------------------------------------------------

/// The job as `run_subagent` leaves one, with the worker's session recorded as the box records
/// it.
async fn a_running_job(agent: &std::sync::Arc<Agent>, leader: u32, sid: &str) {
    a_job_row(agent).await;
    agent
        .store
        .insert_subagent(sid, "job-tick", "run the ticker", "pi", CHAT)
        .unwrap();
    agent.store.set_job_subagent(JOB, sid).unwrap();
    agent.store.set_job_state(JOB, "running", None).unwrap();
    agent.note_worker_pgid(sid, leader).await;
}

async fn a_job_row(agent: &std::sync::Arc<Agent>) {
    let host = agent.sync.open_room(CHAT).await.unwrap();
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(
                ASK,
                "run the ticker, 80 iterations",
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
            "Run tmp/tick13.sh, 80 iterations.",
            &[],
        )
        .unwrap();
}

// ---- bug Z -------------------------------------------------------------------------------

/// **Round 13.** The cancel reaches a tree the job backgrounded, orphaned and regrouped.
///
/// The assertion that fails against the unfixed code is the log one: on 0.1.196 the recorded
/// process group named the worker's own shell and nothing else, the loop was in a group of its
/// own with PID 1 for a parent, and it went on writing to 60/60 while the job read `cancelled`.
#[tokio::test]
async fn cancelling_a_job_kills_the_backgrounded_tree_it_orphaned() {
    let (agent, root) = test_agent().await;
    let worker = Worker::start(&root, &planted_mark(WORKER_R13));
    worker.assert_is_round_13_shape();
    a_running_job(&agent, worker.leader, WORKER_R13).await;

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
    std::thread::sleep(SETTLE);
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

/// **Round 14.** The cancel reaches a descendant that left the worker's session by its own
/// `setsid` — 2576 in the round, which led session 2576 and wrote on after the cancel.
///
/// The session handle that answers round 13 does not answer this one: a process that calls
/// `setsid` leaves the recorded session exactly as round 13's tree left the recorded group. The
/// mark is what still names it, because an environment variable is inherited through `setsid`
/// and cannot be shed by it.
#[tokio::test]
async fn cancelling_a_job_kills_a_descendant_that_started_its_own_session() {
    let (agent, root) = test_agent().await;
    // Round 14's tree: four levels, and the deepest starts a session of its own.
    let worker = DeepWorker::start(&root, &planted_mark(WORKER_R14), &["", "", "setsid"]);
    worker.assert_a_descendant_leads_its_own_session();
    a_running_job(&agent, worker.leader, WORKER_R14).await;

    let t0 = Instant::now();
    let stopped = agent.stop_jobs(CHAT, Some(JOB)).await;
    assert!(
        t0.elapsed() < BUDGET,
        "the cancel outran the user's patience"
    );

    let at_cancel = worker.total_lines();
    std::thread::sleep(SETTLE);
    assert_eq!(
        worker.total_lines(),
        at_cancel,
        "a level of the tree kept writing after the cancel; still alive: {:?} of {:?}",
        worker.still_alive(),
        worker.pids
    );
    assert_eq!(
        worker.still_alive(),
        Vec::<u32>::new(),
        "a process of the cancelled job outlived it (sessions: {:?})",
        worker.sessions()
    );
    assert_eq!(
        stopped[0].sweep.survivors,
        Vec::<u32>::new(),
        "{:?}",
        stopped[0].sweep
    );
    assert!(
        stopped[0].sweep.killed >= worker.pids.len(),
        "the cancel reported fewer kills than the tree had levels: {:?} for {:?}",
        stopped[0].sweep,
        worker.pids
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// **The shape round 15 is running now**: five levels, stacking `setsid` on `nohup`.
///
/// Here before that round is scored rather than after it. Neither launcher can shed an
/// environment variable, so the mark holds at every level — and this is the test that says so
/// rather than the argument that it should.
#[tokio::test]
async fn cancelling_a_job_kills_a_tree_that_stacks_setsid_on_nohup() {
    let (agent, root) = test_agent().await;
    let worker = DeepWorker::start(
        &root,
        &planted_mark(WORKER_R15),
        &["nohup", "setsid", "nohup", "setsid"],
    );
    worker.assert_a_descendant_leads_its_own_session();
    a_running_job(&agent, worker.leader, WORKER_R15).await;

    let stopped = agent.stop_jobs(CHAT, Some(JOB)).await;

    let at_cancel = worker.total_lines();
    std::thread::sleep(SETTLE);
    assert_eq!(
        worker.total_lines(),
        at_cancel,
        "a level kept writing after the cancel; still alive: {:?} of {:?}",
        worker.still_alive(),
        worker.pids
    );
    assert_eq!(
        worker.still_alive(),
        Vec::<u32>::new(),
        "a process of the cancelled job outlived it (sessions: {:?})",
        worker.sessions()
    );
    assert_eq!(
        stopped[0].sweep.survivors,
        Vec::<u32>::new(),
        "{:?}",
        stopped[0].sweep
    );

    let _ = std::fs::remove_dir_all(&root);
}

// ---- bug AE ------------------------------------------------------------------------------

/// **Bug AE, the claim.** A cancel that could not look at the job's processes must not say they
/// are dead.
///
/// The job's worker row is gone — an agentd that restarted mid-job, a store that lost it — so
/// `end_cancelled_worker` has nothing to sweep with. Against the unfixed code that produced an
/// empty survivor list, which read as a clean cancel, and the note said "every process it had
/// started is dead". That sentence is round 13's and round 14's, generated from a sweep that
/// had looked at nothing.
#[tokio::test]
async fn a_cancel_that_could_not_look_does_not_say_the_processes_are_dead() {
    let (agent, root) = test_agent().await;
    a_job_row(&agent).await;
    agent
        .store
        .set_job_subagent(JOB, "01TESTWORKERVANISHED00001")
        .unwrap();
    agent.store.set_job_state(JOB, "running", None).unwrap();
    let turn = agent.test_hold_turn(CHAT).await;

    let result = agent
        .hook_stop_job(JOB)
        .await
        .expect("the job is stoppable");
    let job = &result["stopped"][0];

    assert_eq!(job["state"], json!("cancelled"), "#2779's half still holds");
    assert_eq!(
        job["processes_checked"],
        json!(false),
        "a cancel that read nothing must say so: {result}"
    );
    assert_eq!(
        result["all_cancelled"],
        json!(false),
        "a job whose processes were never checked is not a confirmed cancel: {result}"
    );
    let note = result["note"].as_str().unwrap_or_default();
    assert!(
        note.contains("could NOT be checked"),
        "the model must be told the check did not happen: {note}"
    );
    assert!(
        !note.contains("is dead"),
        "and must not be handed round 13's sentence: {note}"
    );

    drop(turn);
    let _ = std::fs::remove_dir_all(&root);
}

/// **Bug AE, the read-back.** The survivor list is read from the kernel by pid, and it finds a
/// process that has left every set the kernel keeps for the worker.
///
/// This is the half that can be measured on a real process: the enumeration that feeds both the
/// kill and the read-back names a live `setsid` escapee, so a cancel can neither miss it nor
/// report it as gone. Re-enumerating a handle proves only that the handle is empty, which is
/// what this process makes true without dying.
#[tokio::test]
async fn the_read_back_finds_a_process_that_left_every_kernel_set() {
    let (_agent, root) = test_agent().await;
    let mut escapee = Escapee::start(&root, WORKER_READBACK);
    // The worker's own ids, which this process is in none of.
    let tree = ProcessTree {
        pgid: Some(std::process::id()),
        sid: Some(std::process::id()),
        cgroup: None,
        mark: Some(WORKER_READBACK.to_owned()),
    };

    assert_ne!(sid_of(escapee.pid), sid_of(std::process::id()));
    assert_ne!(pgid_of(escapee.pid), pgid_of(std::process::id()));
    assert!(
        tree.members().contains(&escapee.pid),
        "the cancel cannot see pid {}, so it can neither kill it nor name it",
        escapee.pid
    );

    // And it is reaped rather than merely seen, which is what makes round 14 a pass.
    let sweep = tree.kill();
    assert_eq!(sweep.survivors, Vec::<u32>::new(), "{sweep:?}");
    assert!(!alive(escapee.pid), "pid {} survived", escapee.pid);
    assert!(sweep.checked);

    escapee.reap();
    let _ = std::fs::remove_dir_all(&root);
}

/// **Bug AE, the report.** A cancel that left a process running names it, and the guard's
/// existing exception then lets the agent signal exactly that pid — the escape hatch round 13
/// and round 14 did not have.
///
/// Built from a `Sweep` rather than from a real survivor, deliberately and for a reason worth
/// stating: with the mark driving the kill, every process this suite can create is reaped, and
/// a process that can be seen and not killed needs another uid on the box. That is an
/// environment fact, and an assertion resting on one is worse than a gap that is disclosed. So
/// the real-process side is the test above, and this is the reporting it feeds, end to end
/// through the chat line, the model's note and the guard.
#[tokio::test]
async fn a_cancel_that_left_a_process_running_names_it_and_unlocks_the_guard() {
    let (agent, root) = test_agent().await;
    let _host = agent.sync.open_room(CHAT).await.unwrap();
    let turn = agent.test_hold_turn(CHAT).await;

    let leaky = vec![StoppedJob {
        id: JOB.to_owned(),
        title: "Running tick13 ticker".to_owned(),
        state: "cancelled".to_owned(),
        sweep: Sweep {
            found: 4,
            killed: 3,
            survivors: vec![2576],
            checked: true,
        },
    }];

    let line = crate::agent::stop_line(&leaky, &nothing_unreachable());
    assert!(
        line.contains("2576") && line.contains("still running"),
        "the chat must be told which process outlived the cancel: {line}"
    );
    assert!(
        !line.contains("nothing more arrives"),
        "a cancel that left work running must not promise nothing more arrives: {line}"
    );

    let note = crate::agent::stop_job_note(&leaky, &nothing_unreachable());
    assert!(
        note.contains("did NOT die") && note.contains("2576"),
        "stop_job must name the pid it could not kill: {note}"
    );
    assert!(
        note.contains("kill -9 2576"),
        "and point at the one kill that is now allowed: {note}"
    );
    assert!(
        !leaky[0].sweep.clean(),
        "a sweep with a survivor is not clean"
    );

    // The guard, with that pid recorded as this chat's leftover exactly as `stop_jobs` records
    // it. Allowed for the reported pid, and for nothing else.
    agent
        .cancel_leftovers
        .lock()
        .await
        .insert(CHAT.to_owned(), vec![2576]);
    agent
        .hook_kill_allowed("kill -9 2576")
        .await
        .expect("a kill aimed at a reported survivor of this chat is allowed");
    for command in [
        "kill 2577",
        "kill 2576 2577",
        "pkill -f tick14",
        "killall bash",
        "kill -9 -2576",
        "kill $(pgrep -f tick14)",
    ] {
        let refused = agent
            .hook_kill_allowed(command)
            .await
            .expect_err("this command must still be refused");
        assert!(
            refused.contains("not a cancel"),
            "`{command}` must still be refused, got: {refused}"
        );
    }

    drop(turn);
    let _ = std::fs::remove_dir_all(&root);
}

/// A process that has left every set the kernel keeps for its worker: its own session, its own
/// group, no cgroup — and the worker's mark in its environment, which `setsid` cannot shed.
struct Escapee {
    pid: u32,
    child: std::process::Child,
}

impl Escapee {
    fn start(dir: &Path, mark: &str) -> Self {
        let mut cmd = Command::new("bash");
        cmd.arg("-c")
            .arg(format!(
                "for k in $(seq 1 400); do sleep {TICK}; echo tick >> {}; done",
                dir.join("escapee.log").display()
            ))
            .env(WORKER_MARK_ENV, mark)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        own_session(&mut cmd);
        let child = cmd.spawn().expect("the escapee starts");
        let pid = child.id();
        wait_until(Duration::from_secs(5), || sid_of(pid) == Some(pid));
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

/// These cases are about a job's own processes, so the reap that runs beside a cancel found
/// nothing. Not `Sweep::default()`: that reads as a sweep which never looked, which is a
/// different claim and the one bug AE was about.
fn nothing_unreachable() -> Sweep {
    Sweep::nothing_to_check()
}
