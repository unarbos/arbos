//! Bug AQ: a sweep read "the adapter's lock was busy" as "this worker is dead", and killed a
//! live worker's whole tree on it.
//!
//! The second half of the reaper's evidence is the question "does any adapter still hold a child
//! for this session". It was asked through `HarnessAdapter::is_open`, which was
//! `children.try_lock().map(…).unwrap_or(false)` — so **a lock it could not take answered
//! `false`, and `false` is what licences the kill.** One mutex guards the whole children map per
//! adapter, and `stop()` holds it across `c.shutdown().await`, which waits on the child under a
//! 5-second `timeout`. So for as long as any one session took to stop, every other live session
//! on that adapter read as closed, and any of them with a subagent row had its entire marked
//! tree collected.
//!
//! ## Where this came from
//!
//! Not from a field report. It was found by reading, while answering what remained destructive
//! after #2863 narrowed the crash-loop sweep, and it was first demonstrated with a throwaway
//! standalone probe rather than a test — twenty lines reproducing the `if let` plus `try_lock`
//! shape outside this repo, which printed:
//!
//! ```text
//! before any stop: is_open(live-worker) = true
//! while another session is being stopped: is_open(live-worker) = false
//! after the stop completes: is_open(live-worker) = true
//! ```
//!
//! That probe is why the window is known not to be theoretical, and this file is that probe made
//! permanent and moved onto the real path. A reviewer who sees only the case below should know
//! the language-level behaviour was measured first and separately.
//!
//! ## Why it is driven through two real jobs
//!
//! The lock cannot be held from outside: `RpcSessions` and its `children` field are private to
//! `harness.rs`, and an adapter is reached only as a `dyn HarnessAdapter`. Faking it would also
//! prove nothing about the real hold, which is the whole question — the window exists because of
//! where `stop()` puts its `.await`, not because a mutex can be contended.
//!
//! So: two jobs, two real worker sessions on one `PiAdapter`, a `stop()` left in flight on one,
//! and the sweep run while that `stop()` is inside its wait. The stub's worker ignores the close
//! of its stdin and sleeps, so the hold lasts the full `timeout` rather than a race the test
//! would lose most runs.
//!
//! Real processes, read back by pid out of `/proc`. Linux-only, skipped elsewhere.
//!
//! ## And the window is reachable from a workload, which was doubted
//!
//! The 0.1.200 cancel canary could not open it from a workload in three attempts and concluded
//! it was structurally unreachable, on the ground that the cancel kills through the cgroup and
//! `cgroup.kill` delivers SIGKILL, so `RpcChild::shutdown` could never block. **That has the
//! order backwards**, and the correction is worth stating here because this file is where the
//! next reader will come looking before deleting the guard.
//!
//! `Agent::end_cancelled_worker` issues an in-band RPC `Abort` (no signal), then `adapter.stop`
//! (the blocking wait, no signal), and only then `tree.kill()` — the one place `cgroup.kill` is
//! written. The SIGKILL lands after the window, so it cannot be what keeps it shut. The canary's
//! lever put `trap '' TERM` on a job's bash **subprocess**, which `shutdown` never waits on; it
//! waits on the worker's pi child, which exits promptly on EOF. That run tested something else.
//!
//! The third case below is that reachability, measured: the cancel a user issues, and the
//! five-minute `reap_unreachable_workers` tick that already runs, with no second cancel and
//! nothing planted.

#![cfg(target_os = "linux")]

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use arbos_core::doc::UserMessageOpts;
use arbos_core::ids::ulid;
use arbos_core::route::Harness;

use crate::agent::Agent;
use crate::harness::StartRequest;
use crate::isolation::{Evidence, UnreadCause};
use crate::status::tests::test_agent_with_pi;

const CHAT: &str = "01TESTCHATSWEEPCONTEND0001";
const ASK_ONE: &str = "01TESTASKSWEEPCONTENDONE01";
/// The sibling session's id, and the string its stub child looks for in its own mark to know
/// that it is the one that must not die when its stdin closes.
const SIBLING: &str = "01TESTSIBLINGMIDSTOP00001";
/// The chat the job-path case runs in, kept apart so the two cases cannot see each other's jobs.
const CHAT_VIA_JOB: &str = "01TESTCHATSWEEPCONTEND0002";
const ASK_VIA_JOB: &str = "01TESTASKSWEEPCONTENDTWO01";
/// In a job's brief, this marks its worker as the one whose `stop()` must hold the children map.
const HANG_BRIEF: &str = "hold-the-children-map-on-close";

/// The live worker's detached work: a `setsid` writer and a nested child of it, each recording
/// its own pid so the test watches them directly rather than through any handle the sweep uses.
///
/// **The `setsid` is incidental here and is deliberately not asserted.** Sibling files plant the
/// same shape as their subject and now check they built it; this one must not, and the reason is
/// the polarity. These cases assert the writers **survive** — bug AQ was a sweep collecting a
/// live worker's tree — so an escape makes the case easier to pass, not harder. Deleting the
/// `setsid` puts both writers back inside the worker's session, where a sweep that read a locked
/// children map as proof of death can actually reach them, and all three cases stay green: the
/// stricter configuration passes too, so the sparing is correct rather than merely unreachable.
/// The handle that would reach them either way is the `ARBOS_WORKER` mark they inherit.
///
/// So asserting the session here would add a premise the cases do not rest on, and a premise
/// kept for its own sake is the kind a later reader deletes as inexplicable. The shape is here
/// because it is what a job's `bash` call really leaves behind, not because it is the subject.
const WRITERS: &str = r#"#!/bin/bash
echo $$ >> "@PIDS@"
bash -c 'echo $$ >> "@PIDS@"; while true; do echo grand >> "@LOG@"; sleep 0.2; done' &
while true; do echo setsid >> "@LOG@"; sleep 0.2; done
"#;

/// A pi that answers the RPC protocol, and whose sessions do two things that matter.
///
/// The first **worker** to be prompted backgrounds the writers above, which is what a job's
/// `bash` tool call leaves behind. `ARBOS_SESSION_ROLE` is what tells a worker from the box's
/// own main pi child, which this same binary also serves.
///
/// And the one session that is told to refuses to die when its stdin closes: after the read
/// loop ends it sleeps well past `RpcChild::shutdown`'s 5-second `timeout`, so a `stop()` on it
/// holds that adapter's children map for the whole of it. **That hold is the condition under
/// test.** Without it the window is a few microseconds wide and the test would lose the race
/// nearly every run; with it the window is the test's to choose.
///
/// It is told in one of two ways, so that only the chosen session hangs and every other child on
/// the box — the main pi child above all — shuts down as it normally would. Either its session id
/// carries the marker, read back out of the `ARBOS_WORKER` mark, for a session the test starts
/// with an id of its own; or its **brief** carries the marker, for a worker whose session id a
/// job generates and the test cannot know in advance. A new worker's first prompt is the brief
/// verbatim (`agent.rs:12100`), so the marker arrives in the prompt line.
const STUB_PI: &str = r#"#!/usr/bin/env python3
import json, os, subprocess, sys, time

worker = os.environ.get("ARBOS_SESSION_ROLE") == "subagent"
hang = "@HANG@" in os.environ.get("ARBOS_WORKER", "")
once = "@ONCE@"

def reply(cmd, cid, data=None):
    sys.stdout.write(json.dumps({"type": "response", "id": cid, "command": cmd, "success": True, "data": data}) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    c = json.loads(line)
    kind, cid = c.get("type"), c.get("id")
    if kind == "get_state":
        reply(kind, cid, {"isStreaming": False})
        continue
    if kind != "prompt" or not worker:
        reply(kind, cid)
        continue
    if "@HANG_BRIEF@" in line:
        hang = True
    if not os.path.exists(once):
        open(once, "w").close()
        subprocess.Popen(
            ["setsid", "bash", "@WRITERS@"],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    reply(kind, cid)

# stdin is closed. The sibling outlives it, so `stop()` sits in its wait holding the adapter's
# children map; every other child exits as it normally would.
if hang:
    time.sleep(30)
"#;

struct Fixture {
    root: std::path::PathBuf,
    pids: std::path::PathBuf,
    log: std::path::PathBuf,
    pi_bin: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("arbos-sweep-contend-{}", ulid()));
        std::fs::create_dir_all(&root).unwrap();
        let pids = root.join("writer.pids");
        let log = root.join("writer.log");
        let writers = root.join("writers.sh");
        let pi_bin = root.join("stub-pi.py");
        std::fs::write(
            &writers,
            WRITERS
                .replace("@PIDS@", &pids.display().to_string())
                .replace("@LOG@", &log.display().to_string()),
        )
        .unwrap();
        std::fs::write(
            &pi_bin,
            STUB_PI
                .replace("@WRITERS@", &writers.display().to_string())
                .replace(
                    "@ONCE@",
                    &root.join("writers-spawned").display().to_string(),
                )
                .replace("@HANG@", SIBLING)
                .replace("@HANG_BRIEF@", HANG_BRIEF),
        )
        .unwrap();
        for f in [&writers, &pi_bin] {
            std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self {
            root,
            pids,
            log,
            pi_bin,
        }
    }

    fn writer_pids(&self) -> Vec<u32> {
        std::fs::read_to_string(&self.pids)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .collect()
    }

    /// The writers still running, read by pid and not through any handle the sweep uses. A
    /// SIGKILLed child of this process stays in `/proc` as a zombie, so the state is read too.
    fn alive(&self) -> Vec<u32> {
        self.writer_pids()
            .into_iter()
            .filter(|p| is_running(*p))
            .collect()
    }

    fn log_len(&self) -> u64 {
        std::fs::metadata(&self.log).map(|m| m.len()).unwrap_or(0)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for pid in self.writer_pids() {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid as i32),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn is_running(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some((_, after_comm)) = stat.rsplit_once(')') else {
        return false;
    };
    !matches!(after_comm.split_whitespace().next(), Some("Z") | Some("X"))
}

async fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    for _ in 0..600 {
        if ready() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{what} did not happen within 60s");
}

/// A box with a live main pi child and an open room, started the way one is on a box.
async fn a_box_for(
    fx: &Fixture,
    chat: &str,
    ask: &str,
) -> (std::sync::Arc<Agent>, crate::sync::SharedHost) {
    let (agent, _root) = test_agent_with_pi(&fx.pi_bin.display().to_string()).await;
    agent
        .start_pi(chat)
        .await
        .expect("the main pi child starts");
    let host = agent.sync.open_room(chat).await.unwrap();
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(
                ask,
                "start some work",
                "dev-mac",
                &UserMessageOpts::default(),
            )
            .unwrap();
    }
    (agent, host)
}

async fn a_box(fx: &Fixture) -> (std::sync::Arc<Agent>, crate::sync::SharedHost) {
    a_box_for(fx, CHAT, ASK_ONE).await
}

/// A job with a real worker session of its own, through `start_job` so that nothing about the
/// session id is planted by the test.
/// `touches` is what keeps two jobs of one chat running at once rather than queueing. An `ops`
/// job that names nothing takes the shared `routines` key (`job_lock_keys`), so a second one
/// waits on the first — and with this stub the first job's run never ends, so it would wait for
/// the whole test. A key of its own is the same lever the 0.1.200 canary needed for its own
/// concurrent pair.
async fn a_job_in(
    agent: &std::sync::Arc<Agent>,
    host: &crate::sync::SharedHost,
    chat: &str,
    ask: &str,
    title: &str,
    brief: &str,
    touches: Vec<String>,
) -> String {
    let started = agent
        .start_job(
            chat, host, ask, title, brief, "ops", touches, false, &mut None,
        )
        .await
        .expect("the job starts");
    started["job_id"].as_str().expect("a job id").to_owned()
}

async fn a_job(agent: &std::sync::Arc<Agent>, host: &crate::sync::SharedHost) -> String {
    a_job_in(
        agent,
        host,
        CHAT,
        ASK_ONE,
        "Writing ticks",
        "Start some work and leave it running.",
        vec![],
    )
    .await
}

/// Every worker session a job has had, via the helper #2859 added — a job accumulates a session
/// per retry and per box restart, and its row keeps only the last.
fn sessions_of(agent: &std::sync::Arc<Agent>, job: &str) -> Vec<String> {
    let row = agent
        .store
        .job(job)
        .expect("the job row reads")
        .expect("the job row exists");
    agent.job_worker_ids(&row)
}

/// Wait out `start_job`, which answers the ask and leaves the worker to a task of its own, so
/// the session id lands after it returns. Then wait for the worker's own process to appear in
/// `/proc` under its mark — the session is only worth stopping once there is a child to stop.
async fn a_started_session(agent: &std::sync::Arc<Agent>, job: &str) -> String {
    let mut found = None;
    wait_for(&format!("job {job} records a worker session"), || {
        found = sessions_of(agent, job).into_iter().next();
        found.is_some()
    })
    .await;
    let session = found.expect("a worker session");
    let marked = session.clone();
    wait_for(&format!("worker {marked} appears in /proc"), || {
        crate::isolation::marked_procs_all()
            .unwrap_or_default()
            .iter()
            .any(|(_, mark)| crate::isolation::session_of(mark) == Some(marked.as_str()))
    })
    .await;
    session
}

/// A second real session on the same adapter, started directly rather than through a job.
///
/// Through the adapter because the sibling's only part is to be a session whose `stop()` holds
/// the children map — it needs no job row, no worker row and no brief, and a second job cannot
/// supply one anyway: a job's worker waits for the first one's run to finish, so the second
/// session would still have no child when the sweep ran.
///
/// It has no subagent row, so the sweep skips it on the first half of its evidence and the
/// measurement below is about the live worker and nothing else.
async fn a_sibling_session(agent: &std::sync::Arc<Agent>, fx: &Fixture) {
    agent
        .adapter(Harness::Pi)
        .start(StartRequest {
            session_id: SIBLING.to_owned(),
            cwd: fx.root.clone(),
            system_prompt: String::new(),
            model: "stub".to_owned(),
            thinking: String::new(),
        })
        .await
        .expect("the sibling session starts");
    wait_for(&format!("sibling {SIBLING} appears in /proc"), || {
        has_marked_session(SIBLING)
    })
    .await;
}

fn has_marked_session(session: &str) -> bool {
    crate::isolation::marked_procs_all()
        .unwrap_or_default()
        .iter()
        .any(|(_, mark)| crate::isolation::session_of(mark) == Some(session))
}

/// The negative control, and it pins a cause of [`Evidence::Nothing`] that nothing else pins.
///
/// `Nothing`'s doc comment names two causes — "no worker was ever started, **or no candidate
/// survived the filter**" — and only the first was covered. `a_cancel_whose_scan_ran_and_found_
/// nothing_says_there_was_nothing_to_check` reaches the same branch through a job that never had
/// a worker, so the second cause had no test of its own.
///
/// This is that cause literally: the mark walk reads the whole box and does enumerate marked
/// processes of a real worker session, and the filter — is this session provably over? — selects
/// none of them, because the session is open. `Nothing` is the right answer and not the lie the
/// case above is about, and the distinction is the handle: a handle-based enumeration can be
/// aimed at the wrong cgroup, so empty is unconfirmed, whereas the mark walk reads every process
/// on the box and has no wrong place to look, so empty is a fact about the box.
///
/// **It is also the control for the case above.** That one asserts a skip forces `Unconfirmed`;
/// this one asserts the branch still reaches `Nothing` when nothing was skipped. Without it the
/// conditional early return could fire on every call — never claiming anything, never wrong, and
/// useless — and both of this file's other assertions would still pass.
#[tokio::test]
async fn a_sweep_whose_filter_spares_everything_it_enumerated_has_nothing_to_account_for() {
    let fx = Fixture::new();
    let (agent, host) = a_box(&fx).await;
    let live_job = a_job(&agent, &host).await;
    let live = a_started_session(&agent, &live_job).await;
    wait_for("the live worker's detached writers start", || {
        fx.alive().len() >= 2
    })
    .await;
    let running = fx.alive();

    // No stop in flight, so every adapter answers about every session.
    let sweep = agent.reap_unreachable_workers().await;

    assert_eq!(
        fx.alive(),
        running,
        "nothing of a live worker may be collected (pids {:?})",
        fx.writer_pids()
    );
    assert!(
        has_marked_session(&live),
        "the walk must have had this worker's processes to enumerate, or the filter was handed \
         an empty box and this measures the wrong cause"
    );
    assert_eq!(
        sweep.evidence,
        Evidence::Nothing,
        "the box was read and no candidate survived the filter, which is exactly \
         `Evidence::Nothing`: {sweep:?}"
    );
    assert_eq!(
        sweep.unread, None,
        "nothing was skipped, so nothing may be reported as unread: {sweep:?}"
    );
    assert_eq!(
        sweep.checked(),
        None,
        "and there is no `processes_checked` to report over a question that does not arise"
    );
    assert!(sweep.clean(), "{sweep:?}");
}

/// The measurement. Two live sessions on one adapter; one of them is being stopped, so the
/// adapter's children map is locked; the sweep runs inside that window.
///
/// On the unfixed code the sweep reads the locked map as "no adapter holds this session", takes
/// that as proof the **other** session is over, and SIGKILLs everything carrying its mark — the
/// detached work of a job that is running correctly and was never asked to stop.
///
/// Two assertions, and the first is the one that matters: the live job's work is still running.
/// The second is the half that is easy to lose — the sweep must *say* it could not look. A sweep
/// that spares the work and then reports a clean result has only moved the dishonesty: the next
/// reader is told the box is accounted for over a session nobody read.
#[tokio::test]
async fn a_sweep_that_cannot_read_an_adapter_keeps_the_session_and_says_it_could_not_look() {
    let fx = Fixture::new();
    let (agent, host) = a_box(&fx).await;

    // The job that must survive: a live worker with detached work of its own.
    let live_job = a_job(&agent, &host).await;
    let live = a_started_session(&agent, &live_job).await;
    wait_for("the live worker's detached writers start", || {
        fx.alive().len() >= 2
    })
    .await;
    let before = fx.log_len();
    wait_for("the writers are writing", || fx.log_len() > before).await;
    let doomed_if_broken = fx.alive();
    assert!(
        doomed_if_broken.len() >= 2,
        "the live worker must have detached work for the sweep to take"
    );

    a_sibling_session(&agent, &fx).await;
    assert_ne!(
        live, SIBLING,
        "the two sessions must differ or this measures nothing"
    );

    // `stop` takes the adapter's children map and holds it across `shutdown`, whose wait on a
    // child that ignores the close of its stdin runs to the full 5 s timeout.
    let adapter = agent.adapter(Harness::Pi);
    let stopping = tokio::spawn(async move { adapter.stop(SIBLING).await });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        has_marked_session(SIBLING),
        "the sibling's child must still be in its wait, or the lock is already free"
    );

    let sweep = agent.reap_unreachable_workers().await;

    // The physical half first, so nothing below rests on the sweep agreeing with itself.
    assert_eq!(
        fx.alive(),
        doomed_if_broken,
        "the sweep killed a live worker's detached work while a sibling session was mid-stop: \
         it read a locked children map as proof the session was over (pids {:?}, sweep {sweep:?})",
        fx.writer_pids()
    );
    let at_sweep = fx.log_len();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        fx.log_len() > at_sweep,
        "the live job's writers stopped writing after the sweep, so something reached them"
    );

    // And the honesty half: a session that could not be read must not be reported as accounted
    // for. `Nothing` is the dangerous answer here, not `Enumerated` — it is the one that reads
    // as "there are no processes to account for".
    assert_eq!(
        sweep.evidence,
        Evidence::Unconfirmed,
        "a sweep that skipped a session it could not read must say so: {sweep:?}"
    );
    assert!(
        !sweep.clean(),
        "and must not be read as a clean sweep: {sweep:?}"
    );

    // The cause and the ids, machine-readably, because "unconfirmed" on its own does not tell an
    // operator or a canary which of the three causes it was looking at.
    let unread = sweep.unread.as_ref().expect("the skip is reported");
    assert_eq!(unread.cause, UnreadCause::AdapterLockContended);
    assert_eq!(
        unread.cause.as_str(),
        "adapter_lock_contended",
        "the token is what the audit record carries; changing it breaks every reader"
    );
    assert_eq!(
        unread.sessions,
        vec![live.clone()],
        "the skipped session must be named, not merely counted"
    );

    let _ = stopping.await;
}

/// The job row's state as the store has it, which is what `stop_jobs` writes before it touches
/// any process.
fn job_state(agent: &std::sync::Arc<Agent>, job: &str) -> String {
    agent
        .store
        .job(job)
        .ok()
        .flatten()
        .map(|row| row.state)
        .unwrap_or_default()
}

/// The same window, opened by the cancel a user issues and read by the timer tick that exists.
///
/// # Why this case exists on top of the one above
///
/// The case above opens the window by calling `adapter.stop` directly on a session with no job.
/// That proves the guard works; it does not prove any workload can reach it. The 0.1.200 cancel
/// canary tried to reach it from a workload, failed three times, and concluded the window was
/// structurally unreachable — **on a reading of the cancel path that is inverted**, stated here so
/// that nobody repeats it:
///
/// > the cancel kills through the cgroup and `cgroup.kill` delivers SIGKILL, so
/// > `RpcChild::shutdown` cannot block
///
/// [`Agent::end_cancelled_worker`] does three things, in this order:
///
/// 1. `adapter.cancel(sid)` — an in-band RPC `Abort` down the child's stdin. Not a signal.
/// 2. `adapter.stop(sid)` — takes the adapter's children map and holds it across
///    `c.shutdown().await`, because the `if let` scrutinee's temporary lives to the end of the
///    block. [`crate::rpc_child::RpcChild::shutdown`] closes stdin and then waits **up to five
///    seconds for the child to exit on EOF**, sending no signal of its own.
/// 3. `tree.kill()` — the only place `cgroup.kill` is written.
///
/// So the cgroup SIGKILL lands strictly **after** the window, and cannot be what stops `shutdown`
/// blocking. The canary's lever put `trap '' TERM` on the job's bash **subprocess**, which
/// `shutdown` never waits on — it waits on the worker's pi child — so that run did not test this
/// guard at all, and its 1.041 s reading is just pi exiting promptly on EOF.
///
/// # The concurrent caller is the one that already runs
///
/// No second cancel is needed and none is issued here. `Agent::spawn_keepalive` runs
/// `reap_unreachable_workers` on a five-minute `tokio::time::interval` for the daemon's whole
/// life, and the tick's body is this one call. So a single cancel of a worker that is slow to
/// exit on EOF, overlapping a tick, is the whole shape — and `adapter.stop` is also called with
/// no cgroup kill anywhere near it when a resumed session fails on its first prompt and when an
/// approval card expires.
///
/// # What makes it fail if the guard is removed
///
/// With `is_open` back to answering a contended lock with `Some(false)`, `session_is_over` reads
/// every adapter as having answered, returns `Some(true)` for the **subject** — a job running
/// correctly that nobody asked to stop — and `reap_orphan_workers` SIGKILLs everything carrying
/// its mark. Both halves below then fail: the subject's detached writers are gone, and the sweep
/// reports no unread part. That is bug AQ, and it is why this guard is not dead code to delete.
#[tokio::test]
async fn a_cancel_through_the_job_path_opens_the_window_and_a_timer_tick_reads_it() {
    let fx = Fixture::new();
    let (agent, host) = a_box_for(&fx, CHAT_VIA_JOB, ASK_VIA_JOB).await;

    // The subject: a job running correctly, with detached work of its own, never cancelled.
    // Started first so that it — and not the doomed job's worker — is the one the stub's
    // run-once branch gives the writers to.
    let subject_job = a_job_in(
        &agent,
        &host,
        CHAT_VIA_JOB,
        ASK_VIA_JOB,
        "Writing ticks",
        "Start some work and leave it running.",
        vec![format!("{}/subject", fx.root.display())],
    )
    .await;
    let subject = a_started_session(&agent, &subject_job).await;
    wait_for("the subject's detached writers start", || {
        fx.alive().len() >= 2
    })
    .await;
    let before = fx.log_len();
    wait_for("the writers are writing", || fx.log_len() > before).await;
    let doomed_if_broken = fx.alive();
    assert!(
        doomed_if_broken.len() >= 2,
        "the subject must have detached work for a broken sweep to take"
    );

    // The job the user cancels. Its brief marks its worker as the one that does not answer the
    // close of its stdin, so the `shutdown` inside the cancel runs to its five-second timeout.
    let doomed_job = a_job_in(
        &agent,
        &host,
        CHAT_VIA_JOB,
        ASK_VIA_JOB,
        "Holding the map",
        &format!("Start some work and leave it running. {HANG_BRIEF}"),
        vec![format!("{}/doomed", fx.root.display())],
    )
    .await;
    let doomed = a_started_session(&agent, &doomed_job).await;
    assert_ne!(
        subject, doomed,
        "the two sessions must differ or this measures nothing"
    );

    // The cancel, through the path a user's `stop_job` takes. Nothing here calls `adapter.stop`.
    let cancelling = {
        let agent = agent.clone();
        let job = doomed_job.clone();
        tokio::spawn(async move { agent.stop_jobs(CHAT_VIA_JOB, Some(&job)).await })
    };

    // Synchronised on the job row rather than on `is_open`, so the wait does not depend on the
    // guard under test: `stop_jobs` writes `cancelled` before it reaches any process. What is
    // left after that is the `Abort` round trip and one `/proc` read of the worker's tree, both
    // of which are milliseconds against `shutdown`'s five-second hold.
    wait_for("the cancel reaches the doomed job's row", || {
        job_state(&agent, &doomed_job) == "cancelled"
    })
    .await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        !cancelling.is_finished(),
        "the cancel had already returned, so the tick below ran outside the window and this \
         measured nothing"
    );

    // The five-minute keepalive tick, which is this call and nothing else.
    let sweep = agent.reap_unreachable_workers().await;

    // The physical half first, so nothing below rests on the sweep agreeing with itself.
    assert_eq!(
        fx.alive(),
        doomed_if_broken,
        "the tick collected a live job's detached work because a cancel held the adapter's \
         children map: it read a locked map as proof the session was over (pids {:?}, \
         sweep {sweep:?})",
        fx.writer_pids()
    );
    let at_sweep = fx.log_len();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        fx.log_len() > at_sweep,
        "the subject's writers stopped writing after the tick, so something reached them"
    );

    // And the honesty half: the skip has to be reported, with its cause and the session it was
    // about. `Nothing` is the dangerous answer here, not `Enumerated` — it is the one that reads
    // as "there are no processes to account for".
    assert_eq!(
        sweep.evidence,
        Evidence::Unconfirmed,
        "a tick that skipped a session it could not read must say so: {sweep:?}"
    );
    assert!(
        !sweep.clean(),
        "and must not be read as a clean sweep: {sweep:?}"
    );
    let unread = sweep.unread.as_ref().expect("the skip is reported");
    assert_eq!(unread.cause, UnreadCause::AdapterLockContended);
    assert!(
        unread.sessions.contains(&subject),
        "the live job's own session is the one that would have been collected, so it is the one \
         that must be named: {unread:?}"
    );

    let _ = cancelling.await;
}
