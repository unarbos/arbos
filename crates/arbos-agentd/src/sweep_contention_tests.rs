//! A sweep must keep live work and describe the evidence it could not read.
//!
//! The original adapter held its entire child map while a sibling stopped. A failed
//! try_lock was once read as “closed”, authorizing a kill of unrelated live writers
//! (bug AQ, investigated after #2863). The registry now releases its short lock before
//! child shutdown: an independent Ready sibling is readable, while the stopping
//! session itself remains unknown. These cases distinguish both from a genuinely
//! contended registry, without making the adapter recreate the old coupling.
//!
//! Real jobs, /proc marks and independently observed writers pin physical survival;
//! the returned Sweep separately pins honest Nothing versus Unconfirmed evidence.
//! A job cancel still orders in-band Abort, adapter stop, then process-tree kill.
//! The historical 0.1.200 canary trapped a bash descendant's TERM, which could not
//! exercise the worker's EOF wait. The job-path case retains that actual overlap.

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
/// In a job's brief, this marks the worker that remains alive after stdin closes.
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
/// loop ends it sleeps past RpcChild::shutdown's5-second timeout. This makes the
/// owned Stopping interval deterministic; independent Ready entries remain readable.
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

# Keep this captured worker in its shutdown wait; every other child exits on EOF.
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
/// Through the adapter because the sibling only needs to remain in Stopping;
/// it needs no job row, no worker row and no brief, and a second job cannot
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

/// A genuine registry read failure must never authorize collecting live work.
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

    let adapter = agent.adapter(Harness::Pi);
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let held = adapter.clone();
    let holding = tokio::spawn(async move {
        held.test_hold_children_until(ready_tx, release_rx).await;
    });
    ready_rx.await.expect("the actual registry is held");
    assert_eq!(adapter.is_open(&live), None);

    let sweep = agent.reap_unreachable_workers().await;

    // The physical half first, so nothing below rests on the sweep agreeing with itself.
    assert_eq!(
        fx.alive(),
        doomed_if_broken,
        "the sweep killed a live worker's detached work while the registry was held: \
         an unreadable receipt cannot prove the session is over (pids {:?}, sweep {sweep:?})",
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

    let _ = release_tx.send(());
    holding.await.unwrap();
}

/// Slow cleanup of A cannot make an independent Ready sibling look unknown/dead.
#[tokio::test]
async fn a_sweep_reads_a_live_sibling_while_another_session_is_stopping() {
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

    let adapter = agent.adapter(Harness::Pi);
    let stopping_adapter = adapter.clone();
    let stopping = tokio::spawn(async move { stopping_adapter.stop(SIBLING).await });
    wait_for("the sibling enters owned Stopping", || {
        adapter.is_open(SIBLING).is_none()
    })
    .await;
    assert!(!stopping.is_finished());
    assert_eq!(adapter.is_open(&live), Some(true));
    assert!(
        has_marked_session(SIBLING),
        "the sibling must still be waiting for its captured child"
    );

    let sweep = agent.reap_unreachable_workers().await;

    // The physical half first, so nothing below rests on the sweep agreeing with itself.
    assert_eq!(
        fx.alive(),
        doomed_if_broken,
        "the sweep killed a live worker's detached work while the registry was held: \
         an unreadable receipt cannot prove the session is over (pids {:?}, sweep {sweep:?})",
        fx.writer_pids()
    );
    let at_sweep = fx.log_len();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        fx.log_len() > at_sweep,
        "the live job's writers stopped writing after the sweep, so something reached them"
    );

    assert_eq!(sweep.evidence, Evidence::Nothing, "{sweep:?}");
    assert_eq!(
        sweep.unread, None,
        "the Ready sibling was actually read: {sweep:?}"
    );
    assert!(sweep.clean(), "{sweep:?}");

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

/// A user cancel overlaps the real keepalive tick. Only the cancelled worker's
/// Stopping receipt is unknown; the subject's Ready receipt is actually read.
#[tokio::test]
async fn a_cancel_and_timer_tick_keep_live_work_and_name_only_the_stopping_session() {
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

    // The job the user cancels. Keep the existing marked worker fixture; the cleanup
    // window below is owned by a process-handle barrier, not its response to stdin EOF.
    let doomed_job = a_job_in(
        &agent,
        &host,
        CHAT_VIA_JOB,
        ASK_VIA_JOB,
        "Waiting for owned cleanup",
        &format!("Start some work and leave it running. {HANG_BRIEF}"),
        vec![format!("{}/doomed", fx.root.display())],
    )
    .await;
    let doomed = a_started_session(&agent, &doomed_job).await;
    assert_ne!(
        subject, doomed,
        "the two sessions must differ or this measures nothing"
    );

    // Hold only the doomed child's process handle before cancellation. stop_jobs can
    // still send its RPC and reap its tree; owned cleanup then pauses without making
    // the independent Ready subject's registry receipt unreadable.
    let adapter = agent.adapter(Harness::Pi);
    // A /proc mark appears at spawn, before GetState completes and publishes Ready.
    // Capture only the actual admitted worker, rather than racing its boot receipt.
    wait_for("the doomed worker publishes its Ready receipt", || {
        adapter.is_open(&doomed) == Some(true)
    })
    .await;
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let held = adapter.clone();
    let held_session = doomed.clone();
    let holding = tokio::spawn(async move {
        held.test_hold_process_until(&held_session, ready_tx, release_rx)
            .await
            .expect("the doomed process is captured");
    });
    ready_rx.await.expect("the doomed process handle is held");

    // The cancel, through the path a user's `stop_job` takes. Nothing here calls `adapter.stop`.
    let cancelling = {
        let agent = agent.clone();
        let job = doomed_job.clone();
        tokio::spawn(async move { agent.stop_jobs(CHAT_VIA_JOB, Some(&job)).await })
    };

    // The persisted cancellation and exact Stopping receipt establish the overlap;
    // a killed child may exit immediately, so elapsed time cannot establish it.
    wait_for("the cancel reaches the doomed job's row", || {
        job_state(&agent, &doomed_job) == "cancelled"
    })
    .await;
    wait_for("owned cleanup reaches the held doomed process", || {
        adapter.is_open(&doomed).is_none() && adapter.is_open(&subject) == Some(true)
    })
    .await;
    assert!(
        !cancelling.is_finished(),
        "the cancel had already returned, so the tick below ran outside the window and this \
         measured nothing"
    );

    assert_eq!(adapter.is_open(&subject), Some(true));
    assert_eq!(adapter.is_open(&doomed), None);
    // The five-minute keepalive tick, which is this call and nothing else.
    let sweep = agent.reap_unreachable_workers().await;

    // The physical half first, so nothing below rests on the sweep agreeing with itself.
    assert_eq!(
        fx.alive(),
        doomed_if_broken,
        "the tick collected a live job's detached work while another job was cancelling; \
         the subject still had a readable Ready receipt (pids {:?}, \
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
    assert_eq!(unread.sessions, vec![doomed.clone()], "{unread:?}");
    assert!(!unread.sessions.contains(&subject));

    release_tx
        .send(())
        .expect("release the owned process handle");
    holding.await.expect("the process barrier completed");
    let _ = cancelling.await;
}
