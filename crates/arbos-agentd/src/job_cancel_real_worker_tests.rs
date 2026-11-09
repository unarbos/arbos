//! The cancel, driven end to end through the worker path a job really takes.
//!
//! Every other cancel case in this crate plants the thing it then looks up: it writes a subagent
//! row with an id of its own choosing, spawns a process carrying
//! [`crate::harness::worker_name`] built from that same id, and asks
//! [`crate::agent::Agent::worker_tree`] for that id back. Such a case proves that one spelling
//! is used consistently inside one function. **It cannot fail when the cancel resolves the wrong
//! id**, because the test never mints a second id for it to confuse — which is the whole of bug
//! AO below.
//!
//! Here nothing is planted. [`crate::agent::Agent::start_job`] mints the job id, `run_subagent`
//! mints each worker's session id, `PiAdapter::start` spawns a real child under `pi:<that id>`,
//! the child's descendants inherit `ARBOS_WORKER` through fork and exec, and the cancel is asked
//! for the **job** — never for a worker. The test learns the writers' pids from the writers
//! themselves, so no assertion here goes near the handles the cancel resolves.

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use arbos_core::doc::UserMessageOpts;
use arbos_core::ids::ulid;

use crate::status::tests::test_agent_with_pi;

const CHAT: &str = "01TESTCHATCANCELREALPATH01";
const ASK: &str = "01TESTASKCANCELREALPATH001";

/// The two shapes the Forest cancel canary used, and the two that the recorded process group and
/// session cannot reach: a writer that calls `setsid` and so leads a session of its own, and a
/// writer nested one level below it.
///
/// Each records its own pid, so the test can watch them without reading the mark or the cgroup,
/// and [`the_writers_escaped`] checks the kernel really put them in that shape before any case
/// reads anything else.
const WRITERS: &str = r#"#!/bin/bash
echo $$ >> "@PIDS@"
bash -c 'echo $$ >> "@PIDS@"; while true; do echo grand >> "@LOG@"; sleep 0.2; done' &
while true; do echo setsid >> "@LOG@"; sleep 0.2; done
"#;

/// A pi that answers the RPC protocol and, on a worker's first prompt, backgrounds the writers
/// above — the way a worker's `bash` tool call leaves a detached process behind.
///
/// `@DIE@` decides what that first worker does next. `stay` keeps it open and emits nothing, so
/// the job is `running` with a live worker when the cancel lands. `exit` makes it close its
/// stdout having produced no words and no tool call, which is what a worker that crashed after
/// detaching its work looks like — and which `run_subagent` answers with one fresh worker for
/// the same job row.
///
/// Only a worker spawns writers: `ARBOS_SESSION_ROLE` tells a subagent child from the box's main
/// pi child, which this same binary also serves.
const STUB_PI: &str = r#"#!/usr/bin/env python3
import json, os, subprocess, sys

worker = os.environ.get("ARBOS_SESSION_ROLE") == "subagent"
once = "@ONCE@"
die = "@DIE@" == "exit"

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
    first = not os.path.exists(once)
    if first:
        open(once, "w").close()
        subprocess.Popen(
            ["setsid", "bash", "@WRITERS@"],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    reply(kind, cid)
    if first and die:
        sys.exit(0)
"#;

struct Fixture {
    root: std::path::PathBuf,
    pids: std::path::PathBuf,
    log: std::path::PathBuf,
    pi_bin: std::path::PathBuf,
}

impl Fixture {
    /// `die`: does the first worker close after detaching the writers (and so get a fresh worker
    /// for the same job), or stay open?
    fn new(die: bool) -> Self {
        assert!(
            std::path::Path::new("/usr/bin/setsid").exists(),
            "setsid (util-linux) is missing, so the escape these cases measure cannot be built"
        );
        let root = std::env::temp_dir().join(format!("arbos-cancel-real-{}", ulid()));
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
                .replace("@ONCE@", &root.join("worker-one-ran").display().to_string())
                .replace("@DIE@", if die { "exit" } else { "stay" }),
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

/// The kernel's session id for a pid, from field 6 of `/proc/<pid>/stat`.
///
/// Named for the kernel and not for the mark. [`crate::isolation::session_of`] answers a
/// different question — which marked session a process belongs to — and the fixture has to
/// reason about the session it planted rather than about the classifier the cancel uses. One
/// word for both is how a reader comes to believe a test checks the thing it plants.
fn kernel_session_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after = stat.rsplit_once(')').map(|(_, rest)| rest)?;
    after.split_whitespace().nth(3)?.parse().ok()
}

/// Wait for the worker's detached writers and pin the two shapes the cases are named for: the
/// one that called `setsid` and leads a session of its own, and the one nested inside it.
///
/// This replaced `fx.alive().len() >= 2`, which a pair of writers sitting in the pi child's own
/// session satisfies exactly as well as an escape does. Deleting `setsid` from the launch left
/// every case in this file green — the shapes were planted and never checked, so the coverage
/// was real but unguarded and would have gone on passing after a change stopped building it.
///
/// The wait is on the pids, which is a set the fixture was **told**: each writer appends its own
/// pid exactly once, so two lines means both have reported and neither identity is in doubt.
/// The shape is then read from `/proc` in one pass and asserted, so a fixture that failed to
/// build it says so in a second with the kernel's own answer rather than running a wait out.
///
/// "Exactly one of the two leads its own session, and the other is inside that session" is a
/// complete description of the planted shape for a pair, which is why the count below is a
/// shape and not a gate.
async fn the_writers_escaped(fx: &Fixture) -> (u32, u32) {
    wait_for(
        "both of the worker's detached writers record their pids",
        || fx.writer_pids().len() == 2,
    )
    .await;
    let pids = fx.writer_pids();
    let sessions: Vec<(u32, Option<u32>)> =
        pids.iter().map(|p| (*p, kernel_session_of(*p))).collect();
    let leaders: Vec<u32> = sessions
        .iter()
        .filter(|(p, sid)| *sid == Some(*p))
        .map(|(p, _)| *p)
        .collect();
    assert_eq!(
        leaders.len(),
        1,
        "exactly one writer must lead a session of its own — that `setsid` is what the recorded \
         process group and session cannot reach, and it is what these cases are about. The \
         kernel has (pid, session) {sessions:?}"
    );
    let escapee = leaders[0];
    let nested = pids
        .iter()
        .copied()
        .find(|p| *p != escapee)
        .expect("two pids were recorded and one of them is not the leader");
    assert_eq!(
        kernel_session_of(nested),
        Some(escapee),
        "writer {nested} is not inside the escaped session {escapee}, so the second shape — a \
         writer nested below the one that left — was never built. The kernel has (pid, session) \
         {sessions:?}"
    );
    (escapee, nested)
}

async fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    for _ in 0..1200 {
        if ready() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{what} did not happen within 60s");
}

/// A box with a live main pi child and an open room, started the way one is on a box.
async fn a_box(fx: &Fixture) -> (std::sync::Arc<crate::agent::Agent>, crate::sync::SharedHost) {
    let (agent, _root) = test_agent_with_pi(&fx.pi_bin.display().to_string()).await;
    // A box always has one, and the fresh worker a failed job gets waits for a live main pi
    // child before it starts (bug V). Without it that wait runs to its 45 s bound.
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
                "start a background writer, then stop it",
                "dev-mac",
                &UserMessageOpts::default(),
            )
            .unwrap();
    }
    (agent, host)
}

async fn a_job(
    agent: &std::sync::Arc<crate::agent::Agent>,
    host: &crate::sync::SharedHost,
) -> String {
    let started = agent
        .start_job(
            CHAT,
            host,
            ASK,
            "Writing ticks",
            "Start a detached writer and leave it running.",
            "ops",
            vec![],
            false,
            &mut None,
        )
        .await
        .expect("the job starts");
    started["job_id"].as_str().expect("a job id").to_owned()
}

/// Everything the cancel must have done, asserted on the writers themselves.
///
/// # What a false pass would look like here
///
/// A sweep that resolves no usable handle kills nothing and finds nothing — and as long as the
/// recorded process group keeps the tree non-empty it still returns `checked: true` with an empty
/// survivor list, which [`crate::isolation::Sweep::clean`] reads as success and `stop_job` states
/// as "every process it had started was checked and is dead". So a green built on the `Sweep`
/// alone proves nothing. These assertions rest on the pids the writers wrote down being gone from
/// `/proc` and on their log having stopped growing; the sweep is only read afterwards, to require
/// that it found work to do rather than reporting an empty success.
async fn the_writers_are_gone(
    fx: &Fixture,
    escape: (u32, u32),
    stopped: &[crate::agent::StoppedJob],
) {
    let (escapee, nested) = escape;
    assert_eq!(stopped.len(), 1, "the open job is cancelled: {stopped:?}");
    assert_eq!(
        stopped[0].state, "cancelled",
        "the row records the stop: {stopped:?}"
    );
    assert_eq!(
        fx.alive(),
        Vec::<u32>::new(),
        "a writer of the cancelled job outlived it: the cancel reached neither the setsid writer \
         {escapee}, which led a session of its own, nor its nested child {nested} (sweep {:?})",
        stopped[0].sweep
    );
    let at_cancel = fx.log_len();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        fx.log_len(),
        at_cancel,
        "the writers' log kept growing after the cancel"
    );
    assert!(
        stopped[0].sweep.evidence.proven(),
        "the cancel must have read the kernel: {:?}",
        stopped[0].sweep
    );
    assert!(
        stopped[0].sweep.found >= 2,
        "a sweep that found nothing to kill proves nothing about work that was still running: {:?}",
        stopped[0].sweep
    );
    assert_eq!(
        stopped[0].sweep.survivors,
        Vec::<u32>::new(),
        "{:?}",
        stopped[0].sweep
    );
}

/// The settled case: one worker, still live, and its detached writers die with the cancel.
///
/// This is the shape #2849 fixed and it holds. It is here so the case below is read as what it
/// is — a second worker, not a third naming scheme.
#[tokio::test]
async fn a_cancel_kills_the_writers_its_only_worker_left_behind() {
    let fx = Fixture::new(false);
    let (agent, host) = a_box(&fx).await;
    let job = a_job(&agent, &host).await;

    let escape = the_writers_escaped(&fx).await;
    let before = fx.log_len();
    wait_for("the writers are writing", || fx.log_len() > before).await;

    let stopped = agent.stop_jobs(CHAT, Some(&job)).await;
    the_writers_are_gone(&fx, escape, &stopped).await;
}

/// The same cancel as above, through the hook the agent actually calls, asserting on what the
/// model is handed rather than on the `Sweep`.
///
/// **This is the true negative for bug AP's guard, and it is the case the strict rule broke.**
/// "Nothing is checked unless the scan enumerated something" is right about Forest and wrong
/// about this: `adapter.stop` closes the worker before the sweep runs, so a cancel that did its
/// job perfectly can enumerate zero afterwards, and reporting "I could not confirm" over a
/// cancel that found and killed real work is how a guard stops being read. The probe in
/// [`crate::agent::Agent::end_cancelled_worker`] is what keeps this case honest: the handles
/// matched this worker while it was alive, so the empty tree afterwards is a read.
///
/// Green before the guard as well as after — deliberately. It is here to pin the behaviour the
/// guard must not change, and a case that only went green with the change could not do that.
#[tokio::test]
async fn a_real_cancel_that_killed_what_it_found_says_so_through_the_hook() {
    let fx = Fixture::new(false);
    let (agent, host) = a_box(&fx).await;
    let job = a_job(&agent, &host).await;

    let (escapee, nested) = the_writers_escaped(&fx).await;
    let before = fx.log_len();
    wait_for("the writers are writing", || fx.log_len() > before).await;

    let _turn = agent.test_hold_turn(CHAT).await;
    let answer = agent
        .hook_stop_job(&job)
        .await
        .expect("the job is stoppable");

    // The physical half first, so nothing below rests on the answer agreeing with itself.
    assert_eq!(
        fx.alive(),
        Vec::<u32>::new(),
        "a writer outlived the cancel: the setsid writer {escapee} or its nested child \
         {nested}: {answer}"
    );
    let at_cancel = fx.log_len();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(fx.log_len(), at_cancel, "the writers' log kept growing");

    assert_eq!(
        answer["stopped"][0]["processes_checked"],
        serde_json::json!(true),
        "this cancel enumerated real processes and killed them, so it may say so: {answer}"
    );
    let note = answer["note"].as_str().unwrap_or_default();
    assert!(
        note.contains("checked and is dead"),
        "and the model is told the strongest true thing, not a hedge: {answer}"
    );
}

/// Both naming schemes, on a job with two workers: each one is spelled from **its own** harness
/// and **its own** session id.
///
/// This is the supporting case, not the evidence. It plants its rows, so on its own it could not
/// have caught bug AO — the behavioural case below is what does that. It is here because jobs run
/// on pi today and a Prime worker is therefore not reachable end to end from a test, while the
/// `prime:` / `w-prime-` pair is live on every box that runs one: without this, the fix would be
/// asserted for one of the two schemes it has to cover.
#[tokio::test]
async fn each_worker_of_a_job_is_spelled_from_its_own_harness_and_id() {
    let (agent, _root) = crate::status::tests::test_agent().await;
    let job = ulid();
    let (first, second) = (ulid(), ulid());
    agent
        .store
        .insert_job(&job, CHAT, ASK, "A job", "ops", "do the thing", &[])
        .unwrap();
    for (sid, harness) in [(&first, "prime"), (&second, "pi")] {
        agent
            .store
            .insert_subagent(sid, &format!("worker-{sid}"), "work", harness, CHAT)
            .unwrap();
        agent.store.set_job_subagent(&job, sid).unwrap();
    }

    let row = agent.store.job(&job).unwrap().unwrap();
    let ids = agent.job_worker_ids(&row);
    assert_eq!(
        ids,
        vec![second.clone(), first.clone()],
        "a cancel reaches every worker the job had, the live one first"
    );

    assert_eq!(
        agent.worker_tree(&first).await.mark,
        Some(crate::harness::worker_name("prime", &first)),
        "the replaced worker keeps its own harness and its own id"
    );
    assert_eq!(
        agent.worker_tree(&second).await.mark,
        Some(crate::harness::worker_name("pi", &second)),
    );
}

/// **Bug AO.** A job that has had more than one worker must be cancelled as a whole: the writers
/// the *first* worker left behind die too.
///
/// A job keeps one row and may be given a fresh worker — after a worker crashes
/// (`JOB_WORKER_RETRIES`), and after a box restart (`reclaim_interrupted_jobs`). Each fresh
/// worker is a new session with a new ULID, and `set_job_subagent` **overwrites** the job row's
/// only record of its worker. From that moment the first worker's processes are unreachable: they
/// carry `ARBOS_WORKER=pi:<first id>` and sit in `w-pi-<first id>`, and the cancel asks
/// `worker_tree` for `pi:<second id>` and `w-pi-<second id>`.
///
/// Every handle is spelled correctly. Both schemes are covered. The id is the wrong one — and a
/// crashed worker is exactly the case whose children outlive it, so the orphans are the whole of
/// what the cancel had left to kill.
///
/// This is also why the Forest canary's cgroup leaves carried ULIDs minted a millisecond after
/// the job ids the agent cancelled: those are first workers, and the id on the row by then was a
/// later one.
#[tokio::test]
async fn a_cancel_kills_the_writers_a_replaced_worker_left_behind() {
    let fx = Fixture::new(true);
    let (agent, host) = a_box(&fx).await;
    let job = a_job(&agent, &host).await;

    let escape = the_writers_escaped(&fx).await;
    // The first worker is gone and the job row names its replacement, which is the state the
    // cancel below has to resolve. Two subagent rows for one job is the whole precondition.
    {
        let agent = agent.clone();
        let job = job.clone();
        wait_for("the job is given a fresh worker", move || {
            // `subagents` answers newest first. The row must name that one and not the worker
            // that spawned the writers — otherwise the cancel below would be resolving the right
            // id by accident and would pass against the bug.
            let workers = agent.store.subagents().unwrap_or_default();
            let named = agent
                .store
                .job(&job)
                .ok()
                .flatten()
                .and_then(|r| r.subagent_id);
            workers.len() >= 2 && named.is_some() && named == workers.first().map(|s| s.id.clone())
        })
        .await;
    }
    let before = fx.log_len();
    wait_for("the orphaned writers are still writing", || {
        fx.log_len() > before
    })
    .await;

    let stopped = agent.stop_jobs(CHAT, Some(&job)).await;
    the_writers_are_gone(&fx, escape, &stopped).await;
}
