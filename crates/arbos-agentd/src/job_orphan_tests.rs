//! Bug AK: telling a live worker's background work from a dead worker's leftovers.
//!
//! Jacob on Test5, round 17 (agentd 0.1.197 / `ef90488f8`, scenario `r17status`). A job's worker
//! died after a pi OOM kill and left its loop running as orphan PID 23536 — with **no job row**,
//! so no `stop_job` could ever name it. `stop_jobs` iterates `store.jobs_for_chat`, so a process
//! with no row is invisible to cancel by construction.
//!
//! The row-lessness is not the OOM's doing. Nothing in this codebase deletes a job row, and
//! `start_job` inserts one strictly before the worker spawns, which is strictly before the
//! worker's bash call spawns anything — so no window exists where a process has a dead row. The
//! row-less path is `start_job_for_send`'s non-forced branch, which says of itself that it has
//! "no job row, no job id and nothing `list_jobs`, `steer_job` or `stop_job` can name". The OOM
//! only removed the worker that would have ended its own tool call.
//!
//! So the handle that is left is the mark. `ARBOS_WORKER` is inherited through fork, exec,
//! `setsid`, `nohup` and reparenting to PID 1, and it names the session that started the
//! process. A process whose session is gone is work nobody can reach again.
//!
//! **What these cases pin, and why it had to be measured first.** The rule is only safe if a
//! live worker's background children are reliably distinguishable from a dead worker's. Two
//! ways to get that catastrophically wrong, both checked here:
//!
//! - Reaping a live worker's tree, which would kill one job's work when another job is cancelled.
//! - Reaping the **main pi child**. It is spawned through `RpcChild` exactly like a worker, so it
//!   and everything it runs carry `ARBOS_WORKER=pi-main`. A classifier that looked that mark up
//!   as a worker session, found nothing, and called it garbage would SIGKILL the box's own agent
//!   mid-turn — a far worse bug than the one being fixed.
//!
//! Real processes, with real marks, read back out of `/proc`.

use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use arbos_core::ids::ulid;

use crate::isolation::{
    marked_procs_all, orphan_marked_procs, reap_orphan_workers, session_of, WORKER_MARK_ENV,
};

/// The mark the main pi child and everything it runs carry (`agent.rs` spawns it under this
/// name). Never a worker session, and never collectable.
const PI_MAIN: &str = "pi-main";

/// Every case here reads `/proc`, which is the whole machine, and two of them need the real
/// `pi-main` string rather than a unique stand-in. Run in parallel they capture each other's
/// processes as their own and tear them down — which reads exactly like the reaper killing the
/// box's own agent, and is the most misleading failure this file could produce.
///
/// Poisoning is ignored on purpose: one case failing must not cascade into the rest reporting
/// a lock error instead of their own result.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn exclusive() -> MutexGuard<'static, ()> {
    ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner())
}

/// A mark no other run of this file can collide with. `/proc` is the whole machine, so a case
/// that asserts on an exact set must own its namespace — including against a tree an earlier
/// aborted run leaked.
fn fresh_mark() -> String {
    format!("pi:{}", ulid())
}

/// A worker root as `RpcChild::spawn` leaves one: its own session, and the mark its descendants
/// inherit. The shell backgrounds a `setsid` child of its own, which is the shape that outlived
/// cancel in rounds 13, 14 and 16 and outlived its worker in 17.
///
/// Both processes `exec` a plain sleep and fork nothing further, so the marked set is exactly
/// two and stays that way. A tree that respawns children — the obvious `while true; do sleep;
/// done` — makes every exact-set assertion race the next fork, which is a property of the
/// fixture and not of the thing being measured.
/// Returns once the tree is up, holding the pids it created and nothing else. Cases that use a
/// shared mark — `pi-main` is one, deliberately, because that is the real string — would
/// otherwise read each other's processes as their own.
fn worker_with_background_child(mark: &str) -> Reaped {
    let before = pids_for(mark);
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg("setsid sleep 300 & exec sleep 300")
        .env(WORKER_MARK_ENV, mark)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            nix::libc::setsid();
            Ok(())
        });
    }
    let child = cmd.spawn().expect("the worker shell starts");
    let mut mine = Vec::new();
    wait_until(&format!("the `{mark}` tree is up"), || {
        mine = pids_for(mark)
            .into_iter()
            .filter(|p| !before.contains(p))
            .collect();
        mine.len() >= 2
    });
    Reaped { pids: mine, child }
}

/// Kills the tree on the way out, including when a case panics part-way. Without this a failed
/// assertion leaves real `sleep` processes on the machine carrying the mark, and the next run
/// reads them as part of its own measurement.
///
/// It reaps **its own** pids rather than everything carrying the mark. Reaping by mark looks
/// equivalent and is not: two cases running in parallel under `pi-main` would each tear down
/// the other's tree, which reads exactly like the reaper killing the box's own agent.
struct Reaped {
    pids: Vec<u32>,
    child: std::process::Child,
}

impl Reaped {
    /// The processes this tree is made of: the worker root and the loop it backgrounded.
    fn pids(&self) -> &[u32] {
        &self.pids
    }
}

impl Drop for Reaped {
    fn drop(&mut self) {
        reap(&self.pids);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn pids_for(mark: &str) -> Vec<u32> {
    marked_procs_all()
        .into_iter()
        .filter(|(_, m)| m == mark)
        .map(|(pid, _)| pid)
        .collect()
}

fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        if ready() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("{what} did not happen within 10s");
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

/// The measurement. Three marked trees stand side by side — one worker whose session the box
/// still holds, one whose session is gone (AK's orphan), and the main agent — and the classifier
/// must pick out exactly the middle one.
#[test]
fn only_a_dead_workers_leftovers_are_collectable() {
    let _serial = exclusive();
    let live_mark = fresh_mark();
    let dead_mark = fresh_mark();
    let (live_mark, dead_mark) = (live_mark.as_str(), dead_mark.as_str());

    let live = worker_with_background_child(live_mark);
    let dead = worker_with_background_child(dead_mark);
    let main_agent = worker_with_background_child(PI_MAIN);

    let live_pids = live.pids().to_vec();
    let dead_pids = dead.pids().to_vec();
    let main_pids = main_agent.pids().to_vec();

    // The background child is a session of its own, so this is the case no handle but the mark
    // reaches — exactly PID 23536's shape in r17status.
    assert!(
        dead_pids.len() >= 2,
        "the fixture must have a backgrounded descendant, or it tests nothing: {dead_pids:?}"
    );

    // The only thing the box asserts is which session is over. It says nothing about `pi-main`,
    // which is the point: that tree survives because no evidence was offered for killing it,
    // not because it was named in an exception.
    let dead_session = session_of(dead_mark).expect("a worker mark names a session");
    let orphans = orphan_marked_procs(|session| session == dead_session);
    let collected: Vec<u32> = orphans.iter().map(|(pid, _)| *pid).collect();

    for pid in &dead_pids {
        assert!(
            collected.contains(pid),
            "the dead worker's tree is collectable: {pid} missing from {collected:?}"
        );
    }
    for pid in &live_pids {
        assert!(
            !collected.contains(pid),
            "a live worker's own background work is its business: {pid} must not be collected"
        );
    }
    for pid in &main_pids {
        assert!(
            !collected.contains(pid),
            "the main agent and everything it runs carry a mark too; reaping {pid} would kill \
             the box's own agent mid-turn"
        );
    }
    assert!(
        !collected.contains(&std::process::id()),
        "and never the caller"
    );
}

/// Every mark written anywhere in this tree, and whether it names a session.
///
/// `ARBOS_WORKER` has exactly one writer — `RpcChild::spawn` — so this list is the complete set
/// of values it can hold, and it is checked here rather than asserted in a comment because the
/// cost of a new non-session name going unnoticed is the box killing its own agent.
#[test]
fn only_the_worker_marks_name_a_session() {
    let _serial = exclusive();
    // `harness.rs`: the two adapters that spawn through `RpcChild`.
    assert_eq!(
        session_of("pi:01ARZ3NDEKTSV4RRFFQ69G5FAV"),
        Some("01ARZ3NDEKTSV4RRFFQ69G5FAV")
    );
    assert_eq!(
        session_of("prime:01ARZ3NDEKTSV4RRFFQ69G5FAV"),
        Some("01ARZ3NDEKTSV4RRFFQ69G5FAV")
    );

    // `agent.rs`: the box's own agent, and the fakes its tests spawn in the same binary. None
    // of these is a session, and every one of them would be fatal to collect.
    assert_eq!(session_of(PI_MAIN), None);
    assert_eq!(session_of("fake-pi"), None);
    assert_eq!(session_of("fake-pi-turn-end"), None);

    // Degenerate shapes are not sessions either.
    assert_eq!(session_of(""), None);
    assert_eq!(session_of("pi:"), None);
}

/// The allow-list, tested the only way that means anything: with an oracle that has gone wrong
/// in the worst direction it can. Even told that **every** session on the box is over, a mark
/// that does not name a session must not be collectable — because the box's own agent carries
/// one of those marks, and so does every bash call the agent makes.
#[test]
fn a_mark_that_names_no_session_survives_an_oracle_that_says_everything_is_dead() {
    let _serial = exclusive();
    let not_sessions = [PI_MAIN, "fake-pi", "fake-pi-turn-end"];
    let trees: Vec<Reaped> = not_sessions
        .iter()
        .map(|m| worker_with_background_child(m))
        .collect();

    let collected: Vec<u32> = orphan_marked_procs(|_| true)
        .into_iter()
        .map(|(pid, _)| pid)
        .collect();

    for (mark, tree) in not_sessions.iter().zip(&trees) {
        for pid in tree.pids() {
            assert!(
                !collected.contains(pid),
                "`{mark}` names no session, so no verdict about sessions can condemn it; \
                 collecting {pid} would kill the box's own agent"
            );
        }
    }
    drop(trees);
}

/// The liveness oracle is the whole safety argument, so the classifier must follow it rather
/// than any property of the process itself. The same tree, unchanged and still running, is
/// spared while its session is held and collectable the moment it is not — which is what makes
/// a worker's death, not its tidiness, the thing that releases its leftovers.
#[test]
fn a_tree_becomes_collectable_when_its_session_ends_and_not_before() {
    let _serial = exclusive();
    let mark = fresh_mark();
    let mark = mark.as_str();
    let worker = worker_with_background_child(mark);
    let pids = worker.pids().to_vec();
    let session = session_of(mark).expect("a worker mark names a session");

    let held = orphan_marked_procs(|_| false);
    for pid in &pids {
        assert!(
            !held.iter().any(|(p, _)| p == pid),
            "while the box holds the session, nothing of the tree is collectable: {pid}"
        );
    }

    // The worker ends. Nothing about the processes changed — only what the box still holds.
    let released = orphan_marked_procs(|s| s == session);
    for pid in &pids {
        assert!(
            released.iter().any(|(p, _)| p == pid),
            "once the session is gone the same tree is collectable: {pid}"
        );
    }
    // Scoped to this case's own mark: `/proc` is the whole machine, and the other case in this
    // file has trees of its own running beside this one.
    let mine: Vec<u32> = released
        .iter()
        .filter(|(_, m)| m == mark)
        .map(|(p, _)| *p)
        .collect();
    assert_eq!(
        mine, pids,
        "exactly this tree is released, no more and no less"
    );
}

/// The whole of AK, end to end: a worker dies, its backgrounded loop survives it with no job row
/// to name it, and the reap collects exactly that loop while the job running beside it is
/// untouched. The sweep reads the kernel back, so the count is what died rather than what was
/// signalled.
#[test]
fn the_reap_collects_the_dead_workers_loop_and_leaves_the_job_beside_it_running() {
    let _serial = exclusive();
    let live_mark = fresh_mark();
    let dead_mark = fresh_mark();
    let (live_mark, dead_mark) = (live_mark.as_str(), dead_mark.as_str());

    let live = worker_with_background_child(live_mark);
    let dead = worker_with_background_child(dead_mark);
    let main_agent = worker_with_background_child(PI_MAIN);

    let doomed = dead.pids().to_vec();
    let spared = live.pids().to_vec();
    let agent = main_agent.pids().to_vec();

    let dead_session = session_of(dead_mark).expect("a worker mark names a session");
    let sweep = reap_orphan_workers(|session| session == dead_session);

    assert!(
        sweep.checked,
        "a reap that did not read the kernel knows nothing (bug AE)"
    );
    assert!(
        sweep.clean(),
        "nothing should have survived this reap: {:?}",
        sweep.survivors
    );
    assert_eq!(
        sweep.killed,
        doomed.len(),
        "every process of the dead worker's tree is accounted for"
    );

    wait_until("the dead worker's tree is gone", || {
        doomed.iter().all(|p| !is_running(*p))
    });

    // The point of the derived keep-set: the job running beside it never noticed.
    for pid in &spared {
        assert!(
            is_running(*pid),
            "cancelling one job's leftovers must not touch another job's work: {pid} died"
        );
    }
    for pid in &agent {
        assert!(is_running(*pid), "the box's own agent must survive: {pid}");
    }
}

/// Running in the sense `isolation.rs` uses: a zombie is not. A reaped tree whose root is this
/// process's own child stays in `/proc` as a `Z` until it is waited on, so the directory
/// existing says nothing about whether the process is still doing anything.
fn is_running(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some(after_comm) = stat.rsplit_once(')') else {
        return false;
    };
    !matches!(
        after_comm.1.split_whitespace().next(),
        Some("Z") | Some("X")
    )
}
