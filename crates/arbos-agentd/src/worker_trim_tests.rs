//! Finished workers are trimmed to the box's cap as work arrives, not only by the timed pass.
//!
//! A finished job's worker keeps its child so the thread can be woken cheaply.
//! `WORKER_RETAINED_MAX` bounds how many, but the pass that enforces it runs every five minutes,
//! so a run of short jobs held one idle process each until then.

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use arbos_core::doc::UserMessageOpts;
use arbos_core::ids::ulid;

use crate::agent::Agent;
use crate::status::tests::test_agent_with_pi;

const CHAT: &str = "01TESTCHATWORKERTRIM000001";

/// A pi whose workers record their pid and answer at once.
const STUB_PI: &str = r#"#!/usr/bin/env python3
import json, os, sys

def out(o):
    sys.stdout.write(json.dumps(o) + "\n")
    sys.stdout.flush()

if os.environ.get("ARBOS_SESSION_ROLE") == "subagent":
    open(os.path.join("@PIDS@", str(os.getpid())), "w").close()
for line in sys.stdin:
    try:
        c = json.loads(line)
    except ValueError:
        continue
    t = c.get("type")
    r = {"type": "response", "id": c.get("id"), "command": t, "success": True}
    if t == "get_state":
        r["data"] = {"isStreaming": False}
    out(r)
    if t != "prompt":
        continue
    out({"type": "agent_start"})
    out({"type": "message_update",
         "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": "counted: 21"}})
    out({"type": "agent_settled", "aborted": False})
"#;

fn workers_alive(pids: &std::path::Path) -> usize {
    std::fs::read_dir(pids)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse::<i32>().ok())
        .filter(|pid| nix::sys::signal::kill(nix::unistd::Pid::from_raw(*pid), None).is_ok())
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_of_short_jobs_does_not_hold_a_process_for_each() {
    let dir = std::env::temp_dir().join(format!("arbos-worker-trim-{}", ulid()));
    let pids = dir.join("pids");
    std::fs::create_dir_all(&pids).unwrap();
    let pi_bin = dir.join("stub-pi.py");
    std::fs::write(
        &pi_bin,
        STUB_PI.replace("@PIDS@", &pids.display().to_string()),
    )
    .unwrap();
    std::fs::set_permissions(&pi_bin, std::fs::Permissions::from_mode(0o755)).unwrap();

    let (agent, root) = test_agent_with_pi(&pi_bin.display().to_string()).await;
    agent.start_pi(CHAT).await.expect("the main child boots");
    let host = agent.sync.open_room(CHAT).await.unwrap();

    let jobs = Agent::WORKER_RETAINED_MAX + 4;
    let mut most = 0;
    for i in 0..jobs {
        let ask = ulid();
        host.lock()
            .await
            .doc
            .append_user_message(
                &ask,
                "count the lines",
                "dev-mac",
                &UserMessageOpts::default(),
            )
            .unwrap();
        let started = agent
            .start_job(
                CHAT,
                &host,
                &ask,
                &format!("Counting lines {i}"),
                "Count the licence lines and report the number.",
                "other",
                vec![],
                true,
                &mut None,
            )
            .await
            .expect("the job starts");
        let job = started["job_id"].as_str().unwrap().to_owned();
        for _ in 0..600 {
            let state = agent.store.job(&job).unwrap().map(|r| r.state);
            if matches!(state.as_deref(), Some("done" | "failed")) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        most = most.max(workers_alive(&pids));
    }
    let started = std::fs::read_dir(&pids).unwrap().count();
    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        started >= jobs,
        "every job had a worker of its own: {started}"
    );
    // The cap, and the one that is running.
    assert!(
        most <= Agent::WORKER_RETAINED_MAX + 1,
        "{most} worker processes held at once after {jobs} short jobs"
    );
}
