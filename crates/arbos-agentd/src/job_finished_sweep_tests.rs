//! Bug AO: a cancel naming a job that had already finished swept nothing at all.
//!
//! A job row goes `done` when its **worker** exits. That is not the same fact as its work
//! having stopped. A `setsid` descendant outlives the worker that spawned it, keeps the
//! `ARBOS_WORKER` mark it inherited, and goes on writing. `JobRow::is_open()` is false for such
//! a row, so `stop_jobs` skips it and returns nothing — and `hook_stop_job` then answered
//! `no running job matched` **on the line before the sweep**, so `/proc` was never read.
//!
//! Not a wrong id and not a failed kill. No sweep was attempted.
//!
//! The assumption was written down, which is the part worth keeping: `resolve_steer_target`'s
//! doc said in so many words that "`stop_job` keeps the stricter rule — there is nothing to stop
//! about a job that has ended". Everything downstream was consistent with that sentence, and
//! the sentence was wrong.
//!
//! The fix moves the reap above the verdict and lets `latest` resolve a finished job, so the
//! most natural phrasing a user types reaches the sweep too.
//!
//! ## What these cases do and do not establish
//!
//! They pin **where the sweep is called from**. The classification itself — telling a live
//! worker's children from a dead worker's, and never reaping `pi-main` — is #2847's subject and
//! is measured in `job_orphan_tests.rs`. Nothing here re-asserts it.
//!
//! The worker is spawned through [`RpcChild::spawn`], the same call production uses, so the
//! mark is set by production code from [`crate::harness::worker_name`] and is not planted by
//! the test. The detached writer is found by the pid it records in its own file, never by the
//! worker's session id: a case that located the writer through the mark would be asserting that
//! the test and the fix agree with each other rather than that either is right.

use std::time::Duration;

use tokio::sync::{Mutex, MutexGuard};

use arbos_core::ids::ulid;

use crate::isolation::{Evidence, Sweep};
use crate::rpc_child::RpcChild;
use crate::status::tests::test_agent;

/// These cases read `/proc` and reap by mark, which is machine-wide. Run in parallel one case's
/// sweep reaps another's writer and the failure reads as the fix not working.
///
/// Async rather than `std::sync`: the guard is held across awaits, and a blocking guard there
/// parks a runtime worker thread.
static ONE_AT_A_TIME: Mutex<()> = Mutex::const_new(());

async fn exclusive() -> MutexGuard<'static, ()> {
    ONE_AT_A_TIME.lock().await
}

/// Alive as `isolation.rs` counts it: a SIGKILLed child stays in `/proc` as a zombie until it is
/// waited on, so the directory existing is not liveness. Matches the existing rule rather than
/// inventing a second one — two liveness rules in one codebase is how the next bug gets in.
fn is_running(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some(after) = stat.rsplit(')').next() else {
        return false;
    };
    !matches!(after.split_whitespace().next(), Some("Z") | Some("X"))
}

/// A worker that detaches a writer and then exits cleanly, leaving the writer running.
///
/// Dropped rather than cleaned up at the end of a body: a panicking case skips its cleanup, and
/// a leaked marked writer is read by the next run as part of its own measurement.
struct DetachedWriter {
    pid: u32,
    log: std::path::PathBuf,
}

impl Drop for DetachedWriter {
    fn drop(&mut self) {
        unsafe {
            nix::libc::kill(self.pid as i32, nix::libc::SIGKILL);
        }
    }
}

impl DetachedWriter {
    fn bytes_written(&self) -> u64 {
        std::fs::metadata(&self.log).map(|m| m.len()).unwrap_or(0)
    }

    /// Positive evidence that the writer is doing work, not merely that a pid exists. Used
    /// before the cancel so a dead-on-arrival fixture cannot be mistaken for a successful sweep.
    fn is_still_appending(&self) -> bool {
        let before = self.bytes_written();
        std::thread::sleep(Duration::from_millis(600));
        self.bytes_written() > before
    }
}

/// Spawn a real worker through production's own `RpcChild::spawn`, have it `setsid` a writer and
/// exit 0, and return the writer once it is up.
///
/// `RpcChild::spawn` sets `ARBOS_WORKER` itself, from the name production's
/// `harness::worker_name` builds. The test supplies the session id and the program and nothing
/// else: no mark is planted, and if production ever stops marking its children these cases go
/// red rather than passing on a mark the test wrote.
async fn worker_that_detaches_and_exits(dir: &std::path::Path, session_id: &str) -> DetachedWriter {
    let log = dir.join(format!("writer-{}.log", ulid()));
    let pidfile = dir.join(format!("writer-{}.pid", ulid()));
    let script = format!(
        "setsid bash -c 'echo $$ > {pid}; while true; do echo tick >> {log}; sleep 0.1; done' \
         </dev/null >/dev/null 2>&1 & exit 0",
        pid = pidfile.display(),
        log = log.display(),
    );
    let name = crate::harness::worker_name("pi", session_id);
    let child = RpcChild::spawn(
        &name,
        "bash",
        &["-c".to_owned(), script],
        &[],
        dir,
        &dir.join("worker.log"),
        None,
    )
    .await
    .expect("the worker spawns");

    // The writer records its own pid. Read it from there rather than deriving it from the mark:
    // the mark is what the fix uses, so a test that used it too would only be checking that the
    // fix agrees with itself.
    let mut pid = None;
    for _ in 0..100 {
        if let Ok(text) = std::fs::read_to_string(&pidfile) {
            if let Ok(p) = text.trim().parse::<u32>() {
                pid = Some(p);
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let pid = pid.expect("the detached writer records its pid");

    // And the worker itself is gone, which is what puts the job row in `done` honestly.
    for _ in 0..100 {
        if !child.is_alive() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    DetachedWriter { pid, log }
}

/// Set a chat up with one job whose worker has finished and whose detached writer is still
/// running. Returns the agent, the job id and the writer.
async fn a_finished_job_with_work_still_running() -> (
    std::sync::Arc<crate::agent::Agent>,
    std::path::PathBuf,
    String,
    String,
    DetachedWriter,
) {
    let (agent, root) = test_agent().await;
    let chat = format!("01TESTCHATFINISHEDSWEEP{:03}", 1);
    let session_id = ulid();
    agent
        .store
        .insert_subagent(&session_id, "worker", "detach and exit", "pi", &chat)
        .unwrap();
    let job = ulid();
    agent
        .store
        .insert_job(
            &job,
            &chat,
            &ulid(),
            "A job that finished",
            "code",
            "detach something and exit",
            &[],
        )
        .unwrap();
    agent.store.set_job_subagent(&job, &session_id).unwrap();

    let writer = worker_that_detaches_and_exits(&root, &session_id).await;
    // The row records what actually happened: the worker exited cleanly.
    agent.store.set_job_state(&job, "done", None).unwrap();
    (agent, root, chat, job, writer)
}

/// A cancel naming a finished job sweeps the work its worker detached.
///
/// Red without the fix, and red in the way the defect reads: the writer is still appending after
/// the cancel, because no sweep was attempted. With the fix the writer is gone from `/proc`.
#[tokio::test]
async fn a_cancel_naming_a_finished_job_sweeps_the_work_it_left() {
    let _one = exclusive().await;
    let (agent, _root, chat, job, writer) = a_finished_job_with_work_still_running().await;

    // Positive evidence the fixture is real before anything is asserted about the sweep: the
    // writer is alive *and* growing its log. A writer that had already died would make the
    // assertion below pass without the fix doing anything.
    assert!(
        is_running(writer.pid),
        "the detached writer outlived its worker, which is the premise of this case"
    );
    assert!(
        writer.is_still_appending(),
        "and it is doing work, not merely present in /proc"
    );
    assert_eq!(
        agent.store.job(&job).unwrap().unwrap().state,
        "done",
        "the row says the job is over while its work is not"
    );

    // The job path on its own reaches nothing, correctly: the row is not open.
    let stopped = agent.stop_jobs(&chat, Some(&job)).await;
    assert!(
        stopped.is_empty(),
        "the row is not open, so the job path correctly cancels nothing: {stopped:?}"
    );

    // The cancel as the agent actually issues it. This goes through `hook_stop_job` rather than
    // calling the reaper, because the reaper is not what was broken — *where it was called
    // from* was. A case that invoked the sweep directly would pass against the bug.
    let _turn = agent.test_hold_turn_for(&chat, &ulid()).await;
    let answer = agent.hook_stop_job(&job).await;

    for _ in 0..40 {
        if !is_running(writer.pid) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        !is_running(writer.pid),
        "a cancel naming this job must reach the work its worker detached; `stop_job` answered \
         {answer:?}"
    );
    let answer = answer.expect("the cancel reached live work, so it is not a failure");
    // Not an exact count: the writer is a shell with a `sleep` child of its own, so the sweep
    // legitimately kills more than the one pid this case tracks. What matters is that the
    // number is the reap's own finding and is not zero.
    assert!(
        answer
            .get("unnamed_processes_killed")
            .and_then(|v| v.as_u64())
            .is_some_and(|n| n > 0),
        "and it must report what it actually killed: {answer}"
    );
    assert!(
        answer.get("processes_checked").is_none(),
        "no vacuous `checked` claim belongs in this answer: {answer}"
    );
}

/// `stop_job("latest")` must resolve a finished job, not refuse because nothing is running.
///
/// Without this the moved sweep is unreachable by the phrasing users actually type: `latest`
/// mapped to `state == "running"` only, so the resolution failed before the sweep could run.
#[tokio::test]
async fn latest_resolves_a_finished_job_so_the_sweep_can_run() {
    let (agent, _root) = test_agent().await;
    let chat = "01TESTCHATFINISHEDSWEEP002";
    let job = ulid();
    agent
        .store
        .insert_job(
            &job,
            chat,
            &ulid(),
            "A job that finished",
            "code",
            "do the thing",
            &[],
        )
        .unwrap();
    agent
        .store
        .set_job_state(&job, "done", Some("finished"))
        .unwrap();

    let resolved = agent.resolve_stop_target(chat, "latest");
    assert_eq!(
        resolved.map(|j| j.id),
        Ok(job.clone()),
        "`latest` names the job that just finished, because its work may not have"
    );

    // And a chat with no job at all still refuses, so the fallback has not become a catch-all.
    let empty = agent.resolve_stop_target("01TESTCHATFINISHEDSWEEP003", "latest");
    assert!(
        empty.is_err(),
        "nothing to resolve means nothing to resolve: {empty:?}"
    );
}

/// A running job is still preferred over a finished one. Pinned because the cheapest way to
/// pass the case above is to return the newest row of any state, which would make `latest`
/// name a finished job while the user watches a running one.
#[tokio::test]
async fn latest_still_prefers_a_running_job() {
    let (agent, _root) = test_agent().await;
    let chat = "01TESTCHATFINISHEDSWEEP004";
    let finished = ulid();
    agent
        .store
        .insert_job(&finished, chat, &ulid(), "Older", "code", "done", &[])
        .unwrap();
    agent
        .store
        .set_job_state(&finished, "done", Some("finished"))
        .unwrap();
    let running = ulid();
    agent
        .store
        .insert_job(&running, chat, &ulid(), "Newer", "code", "running", &[])
        .unwrap();
    agent
        .store
        .set_job_state(&running, "running", None)
        .unwrap();

    assert_eq!(
        agent.resolve_stop_target(chat, "latest").map(|j| j.id),
        Ok(running),
        "a running job is what `latest` means while one is running"
    );
}

/// Bug AP: a reap that killed everything it found must still produce a stop line.
///
/// The gate on `write_stop_line` read `survivors` alone, so `found: 4, killed: 4,
/// survivors: []` with no job row stopped wrote nothing. The line was suppressed *because* the
/// sweep had succeeded, which is the inverse of what a cancel line is for — and it leaves the
/// canary with no `STOP_LINE_VERBATIM` to scrape in exactly the case it exists to confirm.
///
/// Red without the fix with `line = ""`, which is the defect stated plainly: the sweep worked
/// and the chat was told nothing.
#[test]
fn a_reap_that_killed_everything_it_found_still_says_so() {
    let swept = Sweep {
        found: 4,
        killed: 4,
        survivors: vec![],
        // Enumerated: four processes read back by pid after the signal. The whole point of the
        // case is that this outcome used to be reported as silence.
        evidence: Evidence::Enumerated,
    };
    let line = crate::agent::stop_line(&[], &[], &swept);
    assert!(
        !line.is_empty(),
        "a sweep that killed four processes must not report as silence"
    );
    assert!(
        line.contains('4'),
        "and it must say how many it killed: {line:?}"
    );
    // The ownership refusal holds here too: box-wide and mark-keyed, so there is no job to
    // name and the line must not invent one.
    assert!(
        line.contains("no job row"),
        "and must not imply the work belonged to a job it cannot establish: {line:?}"
    );
}

/// A reap that enumerated nothing still says nothing. Pinned because the cheapest way to pass
/// the case above is to always write a line, which would put an empty cancel sentence in the
/// chat after every stop.
#[test]
fn a_reap_that_found_nothing_stays_silent() {
    let nothing = Sweep {
        found: 0,
        killed: 0,
        survivors: vec![],
        // Nothing to check rather than a confirmed clean: no worker of this kind existed.
        evidence: Evidence::Nothing,
    };
    assert_eq!(crate::agent::stop_line(&[], &[], &nothing), "");
}

/// The whole merged sentence, read as a user would read it.
///
/// Four clauses now meet in one line that three separate changes contributed to: the job's own
/// cancel, the job's own kills, the box-wide reap's kills (bug AP) and the honesty guard's
/// `nothing more arrives` promise. The guard's own lane hit a case where two individually
/// correct clauses merged into a sentence that attributed box-wide swept work to a named job,
/// so this pins the combination rather than the parts.
#[test]
fn the_merged_line_says_what_each_sweep_did_without_confusing_them() {
    let swept = Sweep {
        found: 3,
        killed: 3,
        survivors: vec![],
        evidence: Evidence::Enumerated,
    };
    let line = crate::agent::stop_line(&[a_cancelled_job(2)], &[], &swept);

    assert!(
        line.contains("The 2 processes it had left running were killed too"),
        "the job's own kills are still attributed to the job: {line:?}"
    );
    assert!(
        line.contains("3 processes left running by a worker that had already died"),
        "the reap's kills are stated: {line:?}"
    );
    assert!(
        line.contains("They belonged to no job row"),
        "and are disclaimed, so the three are not read as the job's: {line:?}"
    );
    assert!(
        line.contains("nothing more arrives from it"),
        "and the guard's promise still stands where everything was accounted for: {line:?}"
    );
}

/// The same line where the reap could not look: the guard drops its promise, and this change's
/// clause must not quietly put a confident one back.
#[test]
fn a_reap_that_could_not_look_takes_the_promise_with_it() {
    let blind = Sweep {
        found: 0,
        killed: 0,
        survivors: vec![],
        evidence: Evidence::Unconfirmed,
    };
    let line = crate::agent::stop_line(&[a_cancelled_job(2)], &[], &blind);
    assert!(
        !line.contains("nothing more arrives"),
        "an unconfirmed reap must leave the promise off: {line:?}"
    );
    assert!(
        !line.contains("were found and killed"),
        "and this change must claim no sweep it did not make: {line:?}"
    );
}

/// A cancelled job that killed `killed` processes of its own and left none behind.
fn a_cancelled_job(killed: usize) -> crate::agent::StoppedJob {
    crate::agent::StoppedJob {
        id: ulid(),
        title: "A job".to_owned(),
        state: "cancelled".to_owned(),
        sweep: Sweep {
            found: killed,
            killed,
            survivors: vec![],
            evidence: Evidence::Enumerated,
        },
    }
}

/// A stop that cancelled only row-less asks says so, completing #2857's reporting.
///
/// #2857 made the cancel reach those asks; the stop line still spoke only of jobs, so the
/// cancel worked and the chat heard nothing about it.
#[test]
fn a_stop_that_cancelled_only_asks_says_so() {
    let nothing = Sweep {
        found: 0,
        killed: 0,
        survivors: vec![],
        // Nothing to check rather than a confirmed clean: no worker of this kind existed.
        evidence: Evidence::Nothing,
    };
    let line = crate::agent::stop_line(&[], &["a".to_owned(), "b".to_owned()], &nothing);
    assert!(
        line.contains('2') && line.contains("had not started"),
        "two asks were cancelled before they began, and the line must say so: {line:?}"
    );
}

/// The writer fixture really is detached: it survives its worker, which is the property the
/// whole bug rests on. If a future change made the worker's exit take its children with it,
/// this case goes red and tells you the premise has moved rather than letting the case above
/// pass for a reason that no longer exists.
#[tokio::test]
async fn the_detached_writer_outlives_its_worker() {
    let _one = exclusive().await;
    let (_agent, _root, _chat, _job, writer) = a_finished_job_with_work_still_running().await;
    assert!(
        is_running(writer.pid) && writer.is_still_appending(),
        "a setsid child keeps running after the worker that spawned it has gone"
    );
}
