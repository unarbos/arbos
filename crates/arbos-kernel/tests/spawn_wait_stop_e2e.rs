//! Stop on a parent blocked in `spawn wait=true` ends the parent's turn
//! now, not when the worker's report comes back (desktop gate phase T:
//! a 49 s spawn held the Stop until it returned). The spawn's record says
//! it ran and was interrupted — with the time it held — never "skipped",
//! since the worker exists; and it does not claim the worker keeps
//! working, because a user's Stop ends the tree.

mod common;

use common::{Attach, start_kernel_replay};
use std::time::{Duration, Instant};

fn transcript(place: &std::path::Path, agent: &str) -> Vec<serde_json::Value> {
    std::fs::read_to_string(
        place
            .join(".arbos/agents")
            .join(agent)
            .join("transcript.jsonl"),
    )
    .unwrap_or_default()
    .lines()
    .filter_map(|l| serde_json::from_str(l).ok())
    .collect()
}

#[test]
fn stop_cuts_a_waiting_spawn_within_seconds_and_its_record_says_interrupted_not_skipped() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"delegating\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"sleeper\",\"task\":\"Run: bash `sleep 40; echo slept`. Then report.\",\"wait\":true}}]}\n",
        "{\"agent\":\"sleeper\",\"content\":\"sleeping\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 40; echo slept\",\"description\":\"Long sleep\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"stopped, noted\"}\n",
        "{\"agent\":\"sleeper\",\"content\":\"slept\"}\n",
    );
    let mut k = start_kernel_replay("spawn-wait-stop", replies);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "run the sleeper"}));
    // The worker is up and inside its bash; root is parked in the spawn.
    assert!(
        a.wait(Duration::from_secs(20), |f| {
            f["type"] == "event"
                && f["agent"] == "sleeper"
                && f["event"]["kind"] == "tool"
                && f["event"]["name"] == "bash"
        })
        .is_some(),
        "the worker's bash started"
    );
    std::thread::sleep(Duration::from_millis(500));

    let asked_at = Instant::now();
    a.send(serde_json::json!({"type": "stop", "agent": "root"}));
    assert!(
        a.wait(Duration::from_secs(8), |f| {
            f["type"] == "event" && f["agent"] == "root" && f["event"]["kind"] == "turn_complete"
        })
        .is_some(),
        "root's turn ends on Stop, not when the worker returns: {:#?}",
        transcript(&k.place, "root")
    );
    let took = asked_at.elapsed();
    assert!(
        took < Duration::from_secs(5),
        "ended {took:?} after Stop; the worker's sleep is 40 s"
    );
    let root = transcript(&k.place, "root");
    let spawn = root
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "spawn")
        .expect("the spawn record");
    let ended = spawn["ended"].as_i64().unwrap_or(0);
    let started = spawn["started"].as_i64().unwrap_or(0);
    assert!(
        ended - started < 5_000,
        "the spawn call returned on Stop, not after 40 s: {spawn}"
    );
    assert!(
        spawn["error"]
            .as_str()
            .is_some_and(|e| e.contains("interrupted while waiting for sleeper")),
        "the record says why it returned: {spawn}"
    );
    assert!(
        root.iter().any(|e| e["kind"] == "interrupted"),
        "the stop is on the record: {root:#?}"
    );
    // Stop means the tree (serve's rule, Cursor's too): the worker's turn
    // ends as well, and the record does not claim it keeps working.
    assert!(
        spawn["error"]
            .as_str()
            .is_some_and(|e| e.contains("ends sleeper's turn too")),
        "{spawn}"
    );
    assert!(
        common::wait_for(Duration::from_secs(8), || {
            transcript(&k.place, "sleeper")
                .iter()
                .any(|e| e["kind"] == "turn_complete")
        }),
        "the worker's turn ended with the stop: {:#?}",
        transcript(&k.place, "sleeper")
    );
    let _ = k.child.kill();
}
