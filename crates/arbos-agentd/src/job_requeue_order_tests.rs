//! A job put back in the queue for a fresh worker must not still be pointing at a worker that
//! reads `running`.
//!
//! # The pair, and why it is read as a pair
//!
//! A job row names its worker in `subagent_id`, and that worker's own row carries the status.
//! Neither alone says whether work is under way: `queued` with no live worker is a job waiting
//! its turn, and `queued` with a worker that reads `running` is a job whose worker is coming up.
//! Reading the two together is how the box can tell those apart, and it is what the 0.1.201
//! `starting` item is built on.
//!
//! # What went wrong
//!
//! A worker that crashes gets one fresh worker for the same job row (`JOB_WORKER_RETRIES`). The
//! settling path requeued the job and then, thirty-odd lines further down, took the dead worker
//! out of `running`. In between, the pair said *a job whose worker is coming up* about a job
//! whose worker had just died — the exact inversion, arrived at from the other side.
//!
//! The window is not theoretical, and it is not narrow. Between the two writes the path releases
//! the job's locks, takes the host lock to clear the working row's activity, and awaits
//! `refresh_job_activity`, which rebuilds the chat's job chip and posts it. Measured against the
//! unfixed order, a reader saw the two at once in **443 of 60,417 samples** of one retry.
//!
//! There is a second exposure that owes nothing to timing. The later write is one of the few
//! subagent writes using `?` rather than `let _ =`, so a store error there returns before the
//! worker ever leaves `running`, and the pair stays wrong until the next boot sweep heals it.
//!
//! # What this case measures
//!
//! Nothing is planted. `start_job` mints the job id, `run_subagent` mints each worker's session
//! id, and a real child is spawned for each; the first one exits having produced no words and no
//! tool call, which is what a crashed worker looks like and what earns the job its fresh worker.
//! A sampler reads the pair — the job's state and the status of the worker the job itself names
//! — across the whole retry and records every sample that says the two things at once.
//!
//! **A sample is only ever evidence against.** The sampler cannot invent a bad pair, so a
//! failure here is always real. It can in principle miss one, which is why the case also
//! requires that the retry it was watching actually happened.

use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arbos_core::ids::ulid;

use crate::agent::Agent;
use crate::status::tests::test_agent_with_pi;
use crate::sync::SharedHost;

const CHAT: &str = "01TESTCHATREQUEUEORDER0001";
const ASK: &str = "01TESTASKREQUEUEORDER00001";

/// A pi that answers the RPC protocol, and whose **first worker** closes its stdout having said
/// nothing at all. That is a crashed worker as `worker_failure` reads one, and it is what buys
/// the job the one fresh worker this case is about.
///
/// `ARBOS_SESSION_ROLE` tells a worker's child from the box's main pi child, which this same
/// binary also serves. The main child must keep answering: the fresh worker waits for a live one
/// before it starts (bug V), and without it that wait runs to its 45 s bound.
const STUB_PI: &str = r#"#!/usr/bin/env python3
import json, os, sys

worker = os.environ.get("ARBOS_SESSION_ROLE") == "subagent"
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
    first = not os.path.exists(once)
    if first:
        open(once, "w").close()
    reply(kind, cid)
    if first:
        sys.exit(0)
    sys.stdout.write(json.dumps({"type": "agent_settled", "aborted": False}) + "\n")
    sys.stdout.flush()
"#;

struct Fixture {
    root: std::path::PathBuf,
    pi_bin: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("arbos-requeue-order-{}", ulid()));
        std::fs::create_dir_all(&root).unwrap();
        let pi_bin = root.join("stub-pi.py");
        std::fs::write(
            &pi_bin,
            STUB_PI.replace("@ONCE@", &root.join("worker-one-ran").display().to_string()),
        )
        .unwrap();
        std::fs::set_permissions(&pi_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self { root, pi_bin }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A box with a live main pi child and an open room, started the way one is on a box.
async fn a_box(fx: &Fixture) -> (Arc<Agent>, SharedHost) {
    let (agent, _root) = test_agent_with_pi(&fx.pi_bin.display().to_string()).await;
    agent
        .start_pi(CHAT)
        .await
        .expect("the main pi child starts");
    let host = agent.sync.open_room(CHAT).await.unwrap();
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(ASK, "count the rows for me", "dev-mac", &Default::default())
            .unwrap();
    }
    (agent, host)
}

/// What the sampler saw: how many pairs it read at all, and every one that said `queued` job
/// and `running` worker at the same time.
///
/// The count is not decoration. A sampler that never ran reports no bad pair, which is the one
/// way this case could pass while saying nothing.
#[derive(Default)]
struct Samples {
    read: usize,
    bad: usize,
    first_bad: Option<String>,
}

type Seen = Arc<Mutex<Samples>>;

/// Stops the sampler and waits for it, so no thread outlives the case that started it.
struct Sampler {
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    seen: Seen,
}

impl Drop for Sampler {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Reads the pair as a reader of it would, through the job's own `subagent_id`.
///
/// **On an OS thread of its own, and that is not incidental.** The window is in the database,
/// not in the scheduler: between the two writes the settling path releases the job's locks,
/// takes the host lock and awaits `refresh_job_activity`, and on an idle box none of those
/// actually suspends. A sampler running as a task would therefore never be given a turn inside
/// the window and would report a clean run whatever the order was — a green that means nothing.
/// A thread reads the same database while the path is between statements.
fn sample_the_pair(agent: &Arc<Agent>, job_id: &str) -> Sampler {
    let agent = agent.clone();
    let job_id = job_id.to_owned();
    let seen: Seen = Arc::default();
    let into = seen.clone();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let halt = stop.clone();
    let thread = std::thread::spawn(move || {
        while !halt.load(std::sync::atomic::Ordering::SeqCst) {
            let Ok(Some(job)) = agent.store.job(&job_id) else {
                continue;
            };
            let Some(worker) = job.subagent_id.as_deref() else {
                continue;
            };
            let Ok(Some(w)) = agent.store.subagent(worker) else {
                continue;
            };
            // A failing case panics while holding this, and a second panic in here would bury
            // the first under a `PoisonError`.
            let mut s = into.lock().unwrap_or_else(|e| e.into_inner());
            s.read += 1;
            if job.state == "queued" && w.status == "running" {
                s.bad += 1;
                s.first_bad.get_or_insert_with(|| {
                    format!(
                        "job {} {} / worker {worker} {}",
                        job.id, job.state, w.status
                    )
                });
            }
        }
    });
    Sampler {
        stop,
        thread: Some(thread),
        seen,
    }
}

async fn wait_for(agent: &Arc<Agent>, job_id: &str, what: &str, mut ready: impl FnMut() -> bool) {
    for _ in 0..1200 {
        if ready() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let job = agent.store.job(job_id).ok().flatten();
    let workers: Vec<(String, String)> = agent
        .store
        .subagents()
        .unwrap_or_default()
        .into_iter()
        .map(|s| (s.id, s.status))
        .collect();
    panic!("{what} did not happen within 60s.\njob: {job:#?}\nworkers: {workers:#?}");
}

/// The measurement: one job, one crashed worker, one retry, and the pair read throughout.
#[tokio::test]
async fn a_job_requeued_for_a_retry_never_names_a_worker_that_reads_running() {
    let fx = Fixture::new();
    let (agent, host) = a_box(&fx).await;
    let job_id = agent
        .start_job(
            CHAT,
            &host,
            ASK,
            "Counting rows",
            "Count the rows and report the number.",
            "ops",
            vec![],
            false,
            &mut None,
        )
        .await
        .expect("the job starts")["job_id"]
        .as_str()
        .expect("a job id")
        .to_owned();

    let sampler = sample_the_pair(&agent, &job_id);

    let worker_now = {
        let agent = agent.clone();
        let id = job_id.clone();
        move || {
            agent
                .store
                .job(&id)
                .ok()
                .flatten()
                .and_then(|j| j.subagent_id)
        }
    };
    let first = {
        let w = worker_now.clone();
        wait_for(&agent, &job_id, "the first worker is attached", move || {
            w().is_some()
        })
        .await;
        worker_now().expect("the first worker")
    };

    // **Wait past the window, not into it.** `bump_job_restarts` runs before the requeue, so a
    // case that stopped at `restarts > 0` would read its samples before the thing it was
    // watching for had happened, and would pass whatever the order was. A second worker row is
    // the first fact that is strictly after it.
    let replaced = agent.clone();
    wait_for(
        &agent,
        &job_id,
        "the crashed worker is replaced",
        move || {
            replaced
                .store
                .subagents()
                .unwrap_or_default()
                .iter()
                .any(|s| s.id != first)
        },
    )
    .await;

    let seen = sampler.seen.lock().expect("samples");
    assert!(
        seen.read > 0,
        "the sampler never read the pair, so this case measured nothing"
    );
    assert_eq!(
        seen.bad, 0,
        "a job waiting for a fresh worker must not read as one whose worker is coming up. \
         {} of {} samples said both at once, the first being {:?}",
        seen.bad, seen.read, seen.first_bad
    );
}
