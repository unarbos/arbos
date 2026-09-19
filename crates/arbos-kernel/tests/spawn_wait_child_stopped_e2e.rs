//! Desktop symmetry loop, cycle 69, d39: a root waited in `spawn wait=true`
//! on a worker running `sleep 90`; the person pressed Stop on the worker.
//! The worker's record read bash interrupted, `interrupted stop`,
//! `turn_complete` — and the spawn returned "(the child's turn ended
//! without a report)", on which the model told the person "The worker
//! finished." Now the spawn's return is the turn's outcome as the done
//! message reads it: stopped by the user, with its last words if any.

mod common;

use common::{Attach, start_kernel_replay};
use std::time::Duration;

fn transcript(place: &std::path::Path, agent: &str) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(format!(".arbos/agents/{agent}/transcript.jsonl")))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[test]
fn a_worker_the_user_stopped_returns_to_spawn_as_stopped_not_as_ended_without_a_report() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"delegating\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"sleepy-worker\",\"task\":\"Run: bash `sleep 90; echo done`. Then report.\",\"wait\":true}}]}\n",
        "{\"agent\":\"sleepy-worker\",\"content\":\"sleeping\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 90; echo done\",\"description\":\"Long sleep\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"the worker was stopped before it reported\"}\n",
    );
    let mut k = start_kernel_replay("spawn-wait-child-stopped", replies);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "run the sleeper"}));
    assert!(
        a.wait(Duration::from_secs(20), |f| {
            f["type"] == "event"
                && f["agent"] == "sleepy-worker"
                && f["event"]["kind"] == "tool"
                && f["event"]["name"] == "bash"
        })
        .is_some(),
        "the worker's bash started"
    );
    std::thread::sleep(Duration::from_millis(500));
    // Stop the worker alone; root keeps waiting in its spawn.
    a.send(serde_json::json!({"type": "stop", "agent": "sleepy-worker"}));
    assert!(
        a.wait_turn("root", "idle", Duration::from_secs(30)),
        "{:#?}",
        transcript(&k.place, "root")
    );
    let root = transcript(&k.place, "root");
    let spawn = root
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "spawn")
        .unwrap_or_else(|| panic!("{root:#?}"));
    let body = spawn["body"].as_str().unwrap_or("");
    // Control on main 249ddb5f: "sleepy-worker reports: (the child's turn
    // ended without a report)".
    assert!(!body.contains("ended without a report"), "{body}");
    assert!(body.contains("stopped by the user"), "{body}");
    assert!(body.contains("did not finish"), "{body}");
    // The worker's own record (archived once its report was read) says
    // interrupted: the spawn's words agree with it now.
    let mut worker = transcript(&k.place, "sleepy-worker");
    if worker.is_empty() {
        worker = std::fs::read_to_string(
            k.place
                .join(".arbos/archive/agents/sleepy-worker/transcript.jsonl"),
        )
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    }
    assert!(
        worker.iter().any(|e| e["kind"] == "interrupted"),
        "the worker's own record says interrupted: {worker:#?}"
    );
    let _ = k.child.kill();
}
