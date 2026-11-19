//! A worker that runs past its cap is stopped, not just given up on.
//!
//! The cap used to end the wait only: the job row read `failed` while the worker's process, and
//! whatever its tool call had started, kept running outside every concurrency gate.

use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::Ordering;
use std::time::Duration;

use arbos_core::doc::UserMessageOpts;
use arbos_core::ids::ulid;

use crate::status::tests::test_agent_with_pi;

const CHAT: &str = "01TESTCHATJOBDEADLINE00001";
const ASK: &str = "01TESTASKJOBDEADLINE000001";

/// A pi that answers the RPC protocol. As a worker it records its pid on the first prompt and
/// then says nothing more: a worker stuck inside a tool call that never returns.
const STUB_PI: &str = r#"#!/usr/bin/env python3
import json, os, sys

worker = os.environ.get("ARBOS_SESSION_ROLE") == "subagent"
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    c = json.loads(line)
    kind, cid = c.get("type"), c.get("id")
    data = {"isStreaming": False} if kind == "get_state" else None
    if kind == "prompt" and worker:
        with open("@PID@.prompts", "a") as f:
            f.write("x")
        with open("@PID@.tmp", "w") as f:
            f.write(str(os.getpid()))
        os.replace("@PID@.tmp", "@PID@")
    if kind == "prompt" and not worker:
        with open(os.path.join(os.path.dirname("@PID@"), "main.prompts"), "a") as f:
            f.write("x")
    sys.stdout.write(json.dumps({"type": "response", "id": cid, "command": kind, "success": True, "data": data}) + "\n")
    sys.stdout.flush()
"#;

fn alive(pid: i32) -> bool {
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
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

#[tokio::test(flavor = "multi_thread")]
async fn a_worker_that_outlives_its_cap_is_stopped() {
    let root = std::env::temp_dir().join(format!("arbos-job-deadline-{}", ulid()));
    std::fs::create_dir_all(&root).unwrap();
    let pid_file = root.join("worker.pid");
    let pi_bin = root.join("stub-pi.py");
    std::fs::write(
        &pi_bin,
        STUB_PI.replace("@PID@", &pid_file.display().to_string()),
    )
    .unwrap();
    std::fs::set_permissions(&pi_bin, std::fs::Permissions::from_mode(0o755)).unwrap();

    let (agent, agent_root) = test_agent_with_pi(&pi_bin.display().to_string()).await;
    agent.worker_deadline_ms.store(1_500, Ordering::SeqCst);
    agent
        .start_pi(CHAT)
        .await
        .expect("the main pi child starts");
    let host = agent.sync.open_room(CHAT).await.unwrap();
    host.lock()
        .await
        .doc
        .append_user_message(
            ASK,
            "run something long",
            "dev-mac",
            &UserMessageOpts::default(),
        )
        .unwrap();
    let started = agent
        .start_job(
            CHAT,
            &host,
            ASK,
            "Running forever",
            "Start a program that never returns.",
            "ops",
            vec![],
            false,
            &mut None,
        )
        .await
        .expect("the job starts");
    let job = started["job_id"].as_str().expect("a job id").to_owned();

    wait_for("the worker takes its prompt", || pid_file.exists()).await;
    let pid: i32 = std::fs::read_to_string(&pid_file).unwrap().parse().unwrap();
    assert!(alive(pid), "the worker is running before its cap");

    let store = agent.store.clone();
    wait_for("the job fails at its cap", || {
        store
            .job(&job)
            .ok()
            .flatten()
            .is_some_and(|r| r.state == "failed")
    })
    .await;
    let mut gone = false;
    for _ in 0..100 {
        gone = !alive(pid);
        if gone {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&agent_root);
    assert!(
        gone,
        "worker {pid} kept running after its job failed at the cap"
    );
}

/// The cap is the job's answer, not a failed start. Once the chat has read that the job ran out
/// of time, no fresh worker and no inline turn runs the same brief again behind the user's back.
#[tokio::test(flavor = "multi_thread")]
async fn a_job_that_ended_at_its_cap_is_not_run_again() {
    let root = std::env::temp_dir().join(format!("arbos-job-deadline-{}", ulid()));
    std::fs::create_dir_all(&root).unwrap();
    let pid_file = root.join("worker.pid");
    let prompts = root.join("worker.pid.prompts");
    let pi_bin = root.join("stub-pi.py");
    std::fs::write(
        &pi_bin,
        STUB_PI.replace("@PID@", &pid_file.display().to_string()),
    )
    .unwrap();
    std::fs::set_permissions(&pi_bin, std::fs::Permissions::from_mode(0o755)).unwrap();

    let (agent, agent_root) = test_agent_with_pi(&pi_bin.display().to_string()).await;
    agent.worker_deadline_ms.store(1_500, Ordering::SeqCst);
    agent
        .start_pi(CHAT)
        .await
        .expect("the main pi child starts");
    let host = agent.sync.open_room(CHAT).await.unwrap();
    host.lock()
        .await
        .doc
        .append_user_message(
            ASK,
            "run something long",
            "dev-mac",
            &UserMessageOpts::default(),
        )
        .unwrap();
    let started = agent
        .start_job(
            CHAT,
            &host,
            ASK,
            "Running forever",
            "Start a program that never returns.",
            "ops",
            vec![],
            false,
            &mut None,
        )
        .await
        .expect("the job starts");
    let job = started["job_id"].as_str().expect("a job id").to_owned();

    let store = agent.store.clone();
    wait_for("the job fails at its cap", || {
        store
            .job(&job)
            .ok()
            .flatten()
            .is_some_and(|r| r.state == "failed")
    })
    .await;
    // Long enough for a second worker to have been started and prompted, had one been.
    tokio::time::sleep(Duration::from_secs(4)).await;
    let asked = std::fs::read_to_string(&prompts).unwrap_or_default().len();
    let row = agent.store.job(&job).unwrap().expect("the job row");
    let main_prompts = std::fs::read_to_string(root.join("main.prompts"))
        .unwrap_or_default()
        .len();
    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&agent_root);
    assert_eq!(
        asked, 1,
        "the capped job's brief was given to a worker {asked} times"
    );
    assert_eq!(
        main_prompts, 0,
        "the main agent was asked to redo the capped job"
    );
    assert_eq!(
        row.state, "failed",
        "the row stays as the chat was told: {row:?}"
    );
}
