//! Bug Z, Test5 round 10 (`r10cancel`, agentd 0.1.195 `adcd73573`, side chat
//! 01M6ME9A7EGV5PJ7DZW43AHTN5). "Actually I don't need that ticking thing anymore, kill it."
//! `stop_job` marked job 01M6P1BW70P2W1X5RN4SSCV48S `cancelled` at 42 s — and pid 561, the
//! `for k in $(seq 1 40); do sleep 8; … >> out/tick10.log; done` loop its worker had started,
//! wrote all forty of its lines anyway (40 at 08:28:08, six minutes after the cancel). The
//! agent then tried `kill 561` twice and the bash guard refused both: "A signal is not a
//! cancel." So the job said cancelled, the work carried on, and the user had no way to stop it.
//!
//! A regression against 0.1.193, where the same flow (`r9cancel`) stopped the loop at three
//! lines and it stayed at three. #2779 ended the worker's turn and left what the worker had
//! spawned running.
//!
//! ## What these tests pin, and what they deliberately do not
//!
//! The spec's hunch was that the loop escaped because the brief told the worker to `nohup` it:
//! detached into its own session, outside the process group `stop_job` kills. Measured on the
//! box, two thirds of that is false. `stop_job` kills no process group at all —
//! `RpcChild::shutdown` SIGKILLs exactly one pid — and a `nohup … &` child does not leave its
//! parent's group or session (`nohup` only ignores `SIGHUP`; `&` starts no group without job
//! control). The detach is not what saved pid 561.
//!
//! So `a_plain_child_of_a_worker_does_not_survive_the_cancel_either` is the load-bearing one,
//! and it uses no `nohup` at all: an ordinary grandchild of a worker outlives a single-pid kill
//! in exactly the same way. A fix aimed at the hunch would pass the `nohup` test and leave the
//! bug in place for every `bash` call a job makes. Both are kept, because the `nohup` shape is
//! the one the spec's success criteria name and the one the field report shows.
//!
//! These tests spawn real processes and read `/proc`, because that is the only thing that can
//! tell a kill that worked from one that was only issued. They are Linux-only and are skipped
//! elsewhere.

#![cfg(target_os = "linux")]

use std::os::unix::process::CommandExt;
use std::process::Command;
use std::time::{Duration, Instant};

use arbos_core::doc::UserMessageOpts;
use serde_json::json;

use crate::agent::{Agent, StoppedJob};
use crate::isolation::{Evidence, ProcessTree, Sweep};
use crate::status::tests::test_agent;

const CHAT: &str = "01TESTCHATJOBLEFTOVER00001";
const JOB: &str = "01TESTJOBLEFTOVERTICK00001";
const WORKER: &str = "01TESTWORKERJOBLEFTOVER001";
const ASK: &str = "01TESTASKJOBLEFTOVER000001";

/// The cancel's budget, from the spec's success criteria: "within 5 s no process from that job
/// is alive and f stops growing".
const BUDGET: Duration = Duration::from_secs(5);

/// A live worker process tree, as a job leaves one: a shell leading its own process group,
/// which starts a loop that appends to `log` once every [`TICK`].
///
/// `process_group(0)` and nothing else, so these tests pin the group fallback on its own: it is
/// what a box without a cgroup has, and round 13 showed it is not enough by itself
/// (`job_reparent_tests`). Production spawns through `setsid`, which leads a group as well.
/// A group of its own is also what keeps this test honest: without it the loop would share the
/// test runner's group and a group kill would take the test harness with it.
struct Worker {
    pgid: u32,
    log: std::path::PathBuf,
    /// Held only so [`Drop`] can reap it. A killed child stays a zombie until its parent waits,
    /// and this test process is that parent — the box's real workers are reparented to PID 1,
    /// which reaps them itself.
    shell: std::process::Child,
}

/// Fast enough that "the file stopped growing" is measurable inside the budget, and slow
/// enough that the loop is reliably mid-`sleep` rather than mid-write when the cancel lands.
const TICK: &str = "0.2";

impl Worker {
    /// `detached`: the r10cancel shape, `nohup … &`, which is what the job's brief prescribed.
    /// Otherwise a plain foreground grandchild, which is every other tool call a job makes.
    fn start(dir: &std::path::Path, detached: bool) -> Self {
        let log = dir.join("tick10.log");
        let loop_body = format!(
            "for k in $(seq 1 400); do sleep {TICK}; echo tick >> {}; done",
            log.display()
        );
        let script = if detached {
            format!("nohup bash -c '{loop_body}' >/dev/null 2>&1 &\nsleep 600")
        } else {
            format!("bash -c '{loop_body}'")
        };
        let child = Command::new("bash")
            .arg("-c")
            .arg(&script)
            .process_group(0)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("the worker shell starts");
        let pgid = child.id();
        // The loop is running before anything is cancelled: a test that cancelled an idle tree
        // would pass against the unfixed code.
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(10) {
            if lines(&log) > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            lines(&log) > 0,
            "the worker's loop never wrote a line, so this test would prove nothing"
        );
        Self {
            pgid,
            log,
            shell: child,
        }
    }

    fn tree(&self) -> ProcessTree {
        ProcessTree {
            pgid: Some(self.pgid),
            ..ProcessTree::default()
        }
    }
}

impl Drop for Worker {
    /// A failed assertion must not leave a tick loop running on the box for the rest of the
    /// suite — which is, after all, the bug.
    fn drop(&mut self) {
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(self.pgid as i32),
            nix::sys::signal::Signal::SIGKILL,
        );
        let _ = self.shell.wait();
    }
}

fn lines(path: &std::path::Path) -> usize {
    std::fs::read_to_string(path)
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

/// The job as `run_subagent` leaves one, plus the worker's recorded process group — the record
/// bug Z found missing ("the job has no record of child pids").
async fn a_running_job(agent: &std::sync::Arc<Agent>, worker: &Worker) {
    let host = agent.sync.open_room(CHAT).await.unwrap();
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(
                ASK,
                "every 8 seconds append a line to out/tick10.log, 40 times",
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
            "Ticking a log",
            "code",
            "Append a line to out/tick10.log every 8 seconds, 40 times.",
            &[],
        )
        .unwrap();
    agent
        .store
        .insert_subagent(WORKER, "job-tick", "tick a log", "pi", CHAT)
        .unwrap();
    agent.store.set_job_subagent(JOB, WORKER).unwrap();
    agent.store.set_job_state(JOB, "running", None).unwrap();
    agent.note_worker_pgid(WORKER, worker.pgid).await;
}

/// The scenario, end to end: cancel the job and the loop it started stops.
///
/// Four things, and the first three are what r10cancel got wrong:
///
/// 1. nothing of the job is alive within five seconds of the cancel;
/// 2. the log stops growing — and is still the same length after a settle, which is the "+400 s,
///    no late final count" half of the spec's criteria;
/// 3. `stop_job`'s own result says what it killed, rather than only that the row says cancelled;
/// 4. the job still reads `cancelled`, so #2779 is not undone by any of this.
#[tokio::test]
async fn cancelling_a_job_kills_the_detached_loop_its_worker_started() {
    let (agent, root) = test_agent().await;
    let worker = Worker::start(&root, true);
    a_running_job(&agent, &worker).await;
    let before = lines(&worker.log);

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

    // 1. Measured against the kernel, not against what the cancel believes it did.
    assert_eq!(
        stopped[0].sweep.survivors,
        Vec::<u32>::new(),
        "the cancelled job left processes running: {:?}",
        stopped[0].sweep
    );
    assert!(
        worker.tree().members().is_empty(),
        "a process of the cancelled job is still in its group"
    );

    // 2. The loop is not merely unlisted, it has stopped writing. This is the assertion that
    //    fails against the unfixed code: on 0.1.195 the count went on to forty.
    let at_cancel = lines(&worker.log);
    std::thread::sleep(Duration::from_millis(1_500)); // many ticks at TICK = 0.2 s
    assert_eq!(
        lines(&worker.log),
        at_cancel,
        "the log grew after the cancel: the loop is still running (it was {before} lines when \
         the cancel was issued)"
    );

    // 3. The result carries what was killed, so a report of a cancel can rest on it.
    assert!(
        stopped[0].sweep.killed > 0,
        "the cancel killed nothing and still called itself a cancel: {:?}",
        stopped[0].sweep
    );
    let line = crate::agent::stop_line(&stopped, &[], &nothing_unreachable());
    assert!(
        line.contains("cancelled") && line.contains("killed"),
        "the chat's stop line must say what was killed, not only that the row reads \
         cancelled: {line}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// The same cancel against a worker's **plain** child — no `nohup`, no `setsid`, no `&`.
///
/// This is the test that separates the fix from the spec's hunch. If the cause were the detach,
/// this tree would never have survived a cancel in the first place and this test would be
/// vacuous. It is not: killing a process does not kill its children, so an ordinary `bash` tool
/// call of a worker outlived the cancel exactly as pid 561 did.
#[tokio::test]
async fn a_plain_child_of_a_worker_does_not_survive_the_cancel_either() {
    let (agent, root) = test_agent().await;
    let worker = Worker::start(&root, false);
    a_running_job(&agent, &worker).await;

    // The shape the bug relies on, asserted rather than assumed: the loop is a *descendant* of
    // the worker, so killing the worker's own pid is not enough to stop it.
    assert!(
        worker.tree().members().len() > 1,
        "the worker has no child, so this test cannot show that children survive"
    );

    let stopped = agent.stop_jobs(CHAT, Some(JOB)).await;
    assert_eq!(stopped[0].state, "cancelled");
    assert_eq!(
        stopped[0].sweep.survivors,
        Vec::<u32>::new(),
        "a plain (undetached) child of the cancelled job is still running: {:?}",
        stopped[0].sweep
    );
    let at_cancel = lines(&worker.log);
    std::thread::sleep(Duration::from_millis(1_500));
    assert_eq!(
        lines(&worker.log),
        at_cancel,
        "the plain child kept writing"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// A cancel that could not kill everything must not claim it did.
///
/// The sweep is best-effort by construction — with no cgroup it is a process-group kill, and an
/// explicit `setsid` leaves the group — so the branch where something survives is reachable,
/// and it is the branch that decides whether the user is told the truth or told r10cancel's
/// story again.
///
/// This one is built from a `Sweep` rather than from a real unkillable process, deliberately.
/// Conjuring a process this test may signal and cannot kill means depending on another uid
/// being present on the box, which is an environment fact no assertion should rest on. The two
/// sentences below are pure functions of the sweep, so handing them the sweep is the whole of
/// what there is to check; that a real survivor produces this sweep is
/// `ProcessTree::kill`'s read-back, exercised by the tests above.
#[test]
fn a_cancel_that_cannot_kill_everything_says_so_instead_of_claiming_a_cancel() {
    let leaky = vec![StoppedJob {
        id: JOB.to_owned(),
        title: "Ticking a log".to_owned(),
        state: "cancelled".to_owned(),
        sweep: Sweep {
            found: 2,
            killed: 1,
            survivors: vec![561],
            evidence: Evidence::Enumerated,
            unread: None,
        },
    }];

    let line = crate::agent::stop_line(&leaky, &[], &nothing_unreachable());
    assert!(
        line.contains("561") && line.contains("still running"),
        "the chat must be told which process outlived the cancel: {line}"
    );
    assert!(
        !line.contains("nothing more arrives"),
        "a cancel that left work running must not promise nothing more arrives: {line}"
    );

    // And the model is told not to call it a cancel, and that the kill r10cancel refused is
    // the way out for this pid.
    let note = crate::agent::stop_job_note(&leaky, &nothing_unreachable());
    assert!(
        note.contains("did NOT die") && note.contains("561"),
        "stop_job must name the pid it could not kill: {note}"
    );
    assert!(
        note.contains("kill -9 561"),
        "and point at the one kill that is now allowed: {note}"
    );

    // The clean case keeps the old promise, so the fix does not make every cancel sound unsure.
    let clean = vec![StoppedJob {
        sweep: Sweep {
            found: 2,
            killed: 2,
            survivors: Vec::new(),
            evidence: Evidence::Enumerated,
            unread: None,
        },
        ..leaky[0].clone()
    }];
    let line = crate::agent::stop_line(&clean, &[], &nothing_unreachable());
    assert!(line.contains("nothing more arrives"), "{line}");
    assert!(
        crate::agent::stop_job_note(&clean, &nothing_unreachable())
            .contains("was checked and is dead"),
        "{}",
        crate::agent::stop_job_note(&clean, &nothing_unreachable())
    );
}

/// Bug Z part 2: with the loop still alive the agent's `kill 561` was refused, so there was no
/// way out at all. A kill aimed at a pid a cancel of this chat could not reap is allowed —
/// and nothing else is.
#[tokio::test]
async fn a_kill_is_allowed_only_for_a_pid_a_cancel_of_this_chat_could_not_reap() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    let worker = Worker::start(&root, false);
    a_running_job(&agent, &worker).await;
    let _turn = agent.test_hold_turn(CHAT).await;

    // Before any cancel the rule is unchanged: a kill is not a cancel.
    let refused = agent
        .hook_kill_allowed("kill 561")
        .await
        .expect_err("a kill with no cancel behind it is still refused");
    assert!(refused.contains("not a cancel"), "{refused}");

    // A cancel that leaves pid 561 behind.
    agent
        .cancel_leftovers
        .lock()
        .await
        .insert(CHAT.to_owned(), vec![561]);

    agent
        .hook_kill_allowed("kill 561")
        .await
        .expect("the leftover of this chat's cancelled job may be killed");
    agent
        .hook_kill_allowed("kill -9 561")
        .await
        .expect("a signal flag does not change the judgement");

    // Everything else stays refused: another pid, a pid the command only partly names, a
    // pattern kill that names no pid, and a process-group kill that would take far more than
    // the leftover.
    for command in [
        "kill 562",
        "kill 561 562",
        "pkill -f 'seq 1 40'",
        "killall bash",
        "kill -9 -561",
        "kill $(pgrep -f tick10)",
    ] {
        let err = agent
            .hook_kill_allowed(command)
            .await
            .unwrap_err_or_else_message();
        assert!(
            err.contains("not a cancel"),
            "`{command}` must still be refused, got: {err}"
        );
    }

    drop(host);
    let _ = std::fs::remove_dir_all(&root);
}

/// `Result::unwrap_err`, but it keeps the message rather than needing `Debug` on `()`.
trait UnwrapErrMessage {
    fn unwrap_err_or_else_message(self) -> String;
}

impl UnwrapErrMessage for Result<(), String> {
    fn unwrap_err_or_else_message(self) -> String {
        match self {
            Ok(()) => panic!("the call was allowed when it had to be refused"),
            Err(e) => e,
        }
    }
}

/// The worker's leftovers are remembered per chat, so the `kill` the guard lets through in one
/// chat is not a free pass in another.
#[tokio::test]
async fn a_leftover_of_another_chat_does_not_unlock_a_kill_here() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    let _turn = agent.test_hold_turn(CHAT).await;
    agent
        .cancel_leftovers
        .lock()
        .await
        .insert("01TESTCHATSOMEWHEREELSE001".to_owned(), vec![561]);

    let refused = agent
        .hook_kill_allowed("kill 561")
        .await
        .expect_err("another chat's leftover is not this chat's business");
    assert!(refused.contains("not a cancel"), "{refused}");

    drop(host);
    let _ = std::fs::remove_dir_all(&root);
}

/// The job's chat is told, and `stop_job`'s result tells the model, in numbers rather than in
/// the word "cancelled" alone.
#[tokio::test]
async fn stop_job_reports_what_it_killed() {
    let (agent, root) = test_agent().await;
    let worker = Worker::start(&root, true);
    a_running_job(&agent, &worker).await;
    let _turn = agent.test_hold_turn(CHAT).await;

    let result = agent
        .hook_stop_job(JOB)
        .await
        .expect("the job is stoppable");
    let job = &result["stopped"][0];
    assert_eq!(job["state"], json!("cancelled"));
    assert!(
        job["processes_killed"].as_u64().unwrap_or(0) > 0,
        "stop_job's result must say how many processes it killed: {result}"
    );
    assert_eq!(
        job["processes_still_alive"],
        json!([]),
        "and which survived: {result}"
    );
    assert_eq!(result["all_cancelled"], json!(true));

    let _ = std::fs::remove_dir_all(&root);
}

/// These cases are about a job's own processes, so the reap that runs beside a cancel found
/// nothing. Not `Sweep::default()`: that reads as a sweep which never looked, which is a
/// different claim and the one bug AE was about.
fn nothing_unreachable() -> Sweep {
    Sweep::nothing_to_check()
}
