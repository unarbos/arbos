//! The cancel path, measured against the two windows bug AL was first read as living in.
//!
//! **Both are closed on today's `main`, and these cases are here to say so.** They are not
//! regression cover for a fix in this file; they are the evidence that the cancel is not what
//! bug AL is about, which is what keeps it from being re-opened on its description alone.
//!
//! # What bug AL actually was
//!
//! Test5 round 22, `r22status`. Three files were still growing a minute after the stop — `h`
//! (a plain background child) 165 to 264 bytes, `i` (`nohup`) 360 to 540, `j` (`setsid`) 468 to
//! 702 — while `b`, `d`, `e` and `f` were flat to the byte across both snapshots.
//!
//! The four flat files are the ones that were real jobs, stopped by `stop_job`. The three that
//! grew had never been jobs at all. `start_job` refused them on the per-chat cap — `6 jobs
//! already run in this chat (cap 3)` — and the agent then did the capped work inline instead, as
//! prime-agent `bash`: PIDs 1855, 1875, 1893 and 1909. Work the main session launches is outside
//! everything a cancel has, by design and not by oversight: `job_worker_ids` names sessions,
//! `worker_tree` resolves a process group, a cgroup leaf and an `ARBOS_WORKER` mark *from a
//! session id*, and `reap_unreachable_workers` deliberately keeps the box's own child because
//! `pi-main` is no session at all. The agent said as much at the time, honestly, and asked
//! before killing anything.
//!
//! So the defect is that capped work was done as unmanaged long-running processes, and the fix
//! belongs at the cap. Widening the cancel to chase main-session children would be fixing the
//! symptom in the one place that is designed never to touch them.
//!
//! # The two windows these cases rule out
//!
//! Both are asserted on the writers' own pids in `/proc` and on their log's length, never on the
//! returned [`crate::isolation::Sweep`]: a sweep that resolves no usable handle kills nothing,
//! finds nothing, and reports exactly what a clean cancel reports, so a green built on the
//! `Sweep` would prove nothing.
//!
//! 1. **The start window.** `PiAdapter::start` spawns the pi child, asks it for its state, and
//!    only then calls `RpcSessions::attach`; `run_subagent` records the child's process group
//!    after `start` has returned. So a cancel landing inside `start` finds no entry in the
//!    adapter's children map and no recorded process group. It is still covered, because
//!    `RpcChild::spawn` attaches the cgroup leaf immediately after `cmd.spawn()` and the child
//!    carries its mark from exec, so the sweep reaches it by either handle.
//! 2. **The queued window.** A job stopped while it waited for a slot. Bug AN established that
//!    it must not start when the slot frees and proved it through the row and the assistant
//!    message; this proves it through processes, which is what round 22 measured.
//!
//! # Why the first of them is deterministic
//!
//! Bug AN's case had to settle 500 ms against a wait with no observable edge, and says so at
//! length. This one does not. The window opens when the worker's pi child is exec'd and closes
//! when it answers `get_state`, and both ends belong to the stub: it touches a marker file as it
//! starts and then holds its `get_state` reply for [`HOLD`] seconds. The test waits for the
//! marker, so the cancel lands inside `PiAdapter::start` by construction rather than by timing,
//! with [`HOLD`] seconds of room instead of a settle.

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use arbos_core::doc::UserMessageOpts;
use arbos_core::ids::ulid;

use crate::status::tests::test_agent_with_pi;
use crate::sync::SharedHost;

const CHAT: &str = "01TESTCHATCANCELSTARTWIN01";
const ASK: &str = "01TESTASKCANCELSTARTWIN001";

/// How long the worker's pi child holds its `get_state` reply, which is how long
/// `PiAdapter::start` stays open and the cancel has to land in.
///
/// Wide enough that the cancel, the sweep and the assertions below all fit inside it with room
/// to spare, so the case does not depend on how fast the box running it is.
const HOLD: u64 = 5;

/// The three shapes round 22 saw still writing, so a fix that reaches only one of them cannot
/// pass: a plain background child, a `nohup` child that ignores `SIGHUP`, and a `setsid` child
/// that leads a session of its own and so escapes a recorded process group.
///
/// Each records its own pid before it starts writing, so the test can watch every one of them
/// without reading a mark, a cgroup or anything else the cancel resolves.
const WRITERS: &str = r#"#!/bin/bash
echo $$ >> "@PIDS@"
bash -c 'echo $$ >> "@PIDS@"; while true; do echo plain >> "@LOG@"; sleep 0.1; done' &
nohup bash -c 'echo $$ >> "@PIDS@"; while true; do echo nohup >> "@LOG@"; sleep 0.1; done' >/dev/null 2>&1 &
setsid bash -c 'echo $$ >> "@PIDS@"; while true; do echo setsid >> "@LOG@"; sleep 0.1; done' >/dev/null 2>&1 &
wait
"#;

/// A pi that answers the RPC protocol, holds the start window open, and launches the writers
/// above the moment it is given a brief — the way a worker's `bash` tool call leaves detached
/// work behind.
///
/// Two behaviours, both keyed on `ARBOS_SESSION_ROLE`, because this same binary also serves the
/// box's own main pi child and that one must behave normally:
///
/// - it touches `@STARTED@` as it is exec'd, which is the test's signal that the worker's child
///   is alive and `PiAdapter::start` has not returned;
/// - it sleeps `@HOLD@` seconds before answering `get_state`, which is the command `start` waits
///   on before it attaches the child and before `run_subagent` records its process group.
///
/// Launching the writers on `prompt` is what makes the green meaningful: a worker that is never
/// prompted writes nothing, so an empty log is proof the brief was withheld rather than proof
/// the stub was quiet.
const STUB_PI: &str = r#"#!/usr/bin/env python3
import json, os, subprocess, sys, time

worker = os.environ.get("ARBOS_SESSION_ROLE") == "subagent"
if worker:
    open("@STARTED@", "a").close()

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
        if worker:
            time.sleep(@HOLD@)
        reply(kind, cid, {"isStreaming": False})
        continue
    if kind != "prompt" or not worker:
        reply(kind, cid)
        continue
    if not os.path.exists("@ONCE@"):
        open("@ONCE@", "w").close()
        subprocess.Popen(
            ["bash", "@WRITERS@"],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    reply(kind, cid)
"#;

struct Fixture {
    root: std::path::PathBuf,
    pids: std::path::PathBuf,
    log: std::path::PathBuf,
    started: std::path::PathBuf,
    pi_bin: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("arbos-al-start-{}", ulid()));
        std::fs::create_dir_all(&root).unwrap();
        let pids = root.join("writer.pids");
        let log = root.join("writer.log");
        let started = root.join("worker-pi-started");
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
                .replace("@STARTED@", &started.display().to_string())
                .replace(
                    "@ONCE@",
                    &root.join("worker-was-prompted").display().to_string(),
                )
                .replace("@HOLD@", &HOLD.to_string()),
        )
        .unwrap();
        for f in [&writers, &pi_bin] {
            std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self {
            root,
            pids,
            log,
            started,
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

    /// The writers still in `/proc`, read by pid and not through any handle the cancel uses.
    fn alive(&self) -> Vec<u32> {
        self.writer_pids()
            .into_iter()
            .filter(|p| std::path::Path::new(&format!("/proc/{p}/stat")).exists())
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

async fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    for _ in 0..600 {
        if ready() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{what} did not happen within 30s");
}

/// A box with a live main pi child and an open room, started the way one is on a box.
async fn a_box(fx: &Fixture) -> (std::sync::Arc<crate::agent::Agent>, SharedHost) {
    let (agent, _root) = test_agent_with_pi(&fx.pi_bin.display().to_string()).await;
    agent
        .start_pi(CHAT)
        .await
        .expect("the main pi child starts");
    let host = agent.sync.open_room(CHAT).await.unwrap();
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(
                ASK,
                "start the tick writers, then stop them",
                "dev-mac",
                &UserMessageOpts::default(),
            )
            .unwrap();
    }
    (agent, host)
}

async fn a_job(agent: &std::sync::Arc<crate::agent::Agent>, host: &SharedHost) -> String {
    let started = agent
        .start_job(
            CHAT,
            host,
            ASK,
            "Writing ticks",
            "Start the detached tick writers and leave them running.",
            "ops",
            vec![],
            false,
            &mut None,
        )
        .await
        .expect("the job starts");
    started["job_id"].as_str().expect("a job id").to_owned()
}

/// A job cancelled while its worker was starting is never given its brief.
///
/// The cancel lands with the worker's pi child alive and missing two of its four handles: no
/// entry in the adapter's children map, so `adapter.stop` cannot close it, and no recorded
/// process group, so `worker_tree` cannot reach it that way. The cgroup leaf and the mark are
/// enough on their own, which is the result — the sweep kills the child, `start` then fails on
/// its own round trip, and the brief is never delivered.
///
/// **Green on today's `main`, and that is the finding.** The case is written to fail loudly if
/// that ever stops being true: the log length and the pids are read directly, so a future change
/// that moves `attach_leaf` after the `get_state` round trip would open this window for real and
/// this case would catch it.
#[tokio::test]
async fn a_job_cancelled_while_its_worker_started_never_gets_its_brief() {
    let fx = Fixture::new();
    let (agent, host) = a_box(&fx).await;
    let job = a_job(&agent, &host).await;

    // The worker's pi child is exec'd and is now holding its `get_state` reply, so
    // `PiAdapter::start` has not returned and neither `attach` nor `note_worker_pgid` has run.
    wait_for("the worker's pi child starts", || fx.started.exists()).await;
    assert_eq!(
        fx.log_len(),
        0,
        "the precondition: the worker has not been prompted yet, so nothing is writing"
    );

    let stopped = agent.stop_jobs(CHAT, Some(&job)).await;
    assert_eq!(stopped.len(), 1, "the job is open, so it is cancelled");
    assert_eq!(
        stopped[0].state, "cancelled",
        "the stop is recorded on the row: {stopped:?}"
    );

    // Past the hold, so `start` has returned and anything the cancel failed to prevent has had
    // its chance to happen. Then a second look, which is the assertion the round 22 report made:
    // not that the cancel returned success, but that the file stopped growing.
    tokio::time::sleep(Duration::from_secs(HOLD + 3)).await;
    let at_cancel = fx.log_len();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        fx.log_len(),
        at_cancel,
        "a cancelled job's writers were still growing their file {}s after the stop (pids {:?}, \
         sweep {:?})",
        HOLD + 4,
        fx.writer_pids(),
        stopped[0].sweep
    );
    assert_eq!(
        fx.alive(),
        Vec::<u32>::new(),
        "a writer of the cancelled job is still in /proc (pids {:?}, sweep {:?})",
        fx.writer_pids(),
        stopped[0].sweep
    );
    // The strongest form, and the one that separates this from a cancel that killed the writers
    // after they started: a worker cancelled before it was prompted has nothing to kill.
    assert_eq!(
        fx.log_len(),
        0,
        "the brief was delivered to a worker whose job was already cancelled"
    );
    assert_eq!(
        agent.store.job(&job).unwrap().unwrap().state,
        "cancelled",
        "and the row must still read cancelled afterwards"
    );
}

/// A job cancelled while it queued for a slot writes nothing when the slot frees — asserted on
/// processes rather than on the row.
///
/// Bug AN established this and proved it through the row and the assistant message. Neither of
/// those is what round 22 measured: it measured a file still growing. A job stopped before it
/// held a slot has no worker and no process, so the whole of the claim rests on the start never
/// happening — which makes the physical assertion the only one that can fail for the right
/// reason.
///
/// Every slot is held by the test, so the job cannot reach its start until the test lets it and
/// the cancel lands squarely inside the wait. Unlike bug AN's case this one needs no settle: it
/// does not matter which side of `run_subagent`'s pre-slot check the cancel falls on, because
/// either side must end with nothing written.
#[tokio::test]
async fn a_job_cancelled_while_it_queued_for_a_slot_writes_nothing_when_the_slot_frees() {
    let fx = Fixture::new();
    let (agent, host) = a_box(&fx).await;

    // Taken before the job is spawned, so there is no window in which it could slip through.
    let held = agent
        .subagent_slots
        .clone()
        .acquire_many_owned(agent.subagent_slots.available_permits() as u32)
        .await
        .expect("the slots are open");
    assert_eq!(agent.subagent_slots.available_permits(), 0);

    let job = a_job(&agent, &host).await;
    {
        let agent = agent.clone();
        wait_for("the job records itself queued for a slot", move || {
            agent
                .store
                .subagents()
                .unwrap_or_default()
                .iter()
                .any(|s| s.status == "queued")
        })
        .await;
    }
    assert!(
        !fx.started.exists(),
        "the precondition: a job waiting for a slot has not started a worker at all"
    );

    let stopped = agent.stop_jobs(CHAT, Some(&job)).await;
    assert_eq!(
        stopped.len(),
        1,
        "the queued job is open, so it is cancelled"
    );

    // The moment the defect would turn on: the slot frees and the start is free to proceed.
    drop(held);
    tokio::time::sleep(Duration::from_secs(HOLD + 3)).await;

    let at_cancel = fx.log_len();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        fx.log_len(),
        at_cancel,
        "a cancelled job's writers were still growing their file after the slot freed (pids \
         {:?}, sweep {:?})",
        fx.writer_pids(),
        stopped[0].sweep
    );
    assert_eq!(
        fx.alive(),
        Vec::<u32>::new(),
        "a writer of the cancelled job is still in /proc (pids {:?})",
        fx.writer_pids()
    );
    assert_eq!(
        fx.log_len(),
        0,
        "a job cancelled before it ever held a slot ran its brief anyway"
    );
}
