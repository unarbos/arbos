//! Symmetry loop: a prompt's `wake` and `user` lines were written in one
//! batch with the same `ts`, so a client that orders by time (the phone,
//! a replay) could not tell which came first. Every line of a batch is
//! now at least a millisecond past the one before it.

mod common;

use common::{Attach, start_kernel_replay};
use std::time::Duration;

#[test]
fn a_prompts_wake_and_user_lines_are_ordered_in_time_as_on_disk() {
    let mut k = start_kernel_replay(
        "event-order",
        "{\"agent\":\"root\",\"content\":\"hello\"}\n{\"agent\":\"root\",\"content\":\"again\"}\n",
    );
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "first"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    // A second prompt queued behind nothing: its own batch.
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "second"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert!(common::wait_for(Duration::from_secs(5), || {
        std::fs::read_to_string(k.place.join(".arbos/agents/root/transcript.jsonl"))
            .unwrap_or_default()
            .matches("turn_complete")
            .count()
            >= 2
    }));
    let events: Vec<serde_json::Value> =
        std::fs::read_to_string(k.place.join(".arbos/agents/root/transcript.jsonl"))
            .unwrap()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
    let ts = |i: usize| events[i]["ts"].as_i64().unwrap();
    // Across separate appends two lines may share a millisecond; the
    // order never runs backwards.
    for w in events.windows(2) {
        assert!(
            w[0]["ts"].as_i64().unwrap() <= w[1]["ts"].as_i64().unwrap(),
            "time never runs backwards along the file: {w:#?}"
        );
    }
    let wakes: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| e["kind"] == "wake")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(wakes.len(), 2);
    for i in wakes {
        assert_eq!(events[i + 1]["kind"], "user", "{events:#?}");
        assert!(ts(i) < ts(i + 1), "wake before its user line in time too");
    }
    let _ = k.child.kill();
}
