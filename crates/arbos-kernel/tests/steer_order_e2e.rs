//! Steer order is a contract: the person's words are read in the order
//! they were said. QA's steer-storm saw `… 16 23 24 17 … 22` once in eight
//! runs — 17–22 typed in the gap after a turn ended, filed as plain
//! follow-ups, and read after 23–24, which arrived during the next turn
//! and were read at its first step. A user's steer now keeps its kind and
//! wakes, so whatever starts the next turn reads it first.

mod common;

use common::{Attach, start_kernel_replay};
use std::time::Duration;

fn transcript(place: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn user_lines(place: &std::path::Path) -> Vec<String> {
    transcript(place)
        .iter()
        .filter(|e| e["kind"] == "user")
        .map(|e| e["text"].as_str().unwrap_or("").to_string())
        .collect()
}

#[test]
fn steers_sent_while_idle_are_read_before_steers_sent_during_the_next_turn() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"hi\"}\n",
        // The next turn: one step with a tool call so a steer sent
        // meanwhile lands at a boundary, then the reply.
        "{\"agent\":\"root\",\"content\":\"working\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 2\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"done\"}\n",
        // Only reached if a steer opened a turn of its own — the fault.
        "{\"agent\":\"root\",\"content\":\"a third turn\"}\n",
    );
    let mut k = start_kernel_replay("steer-order", replies);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "hello"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));

    // Two steers into the gap: the agent is idle, no turn to steer.
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "S1", "steer": true}));
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "S2", "steer": true}));
    // The first starts a turn; the second is read at that turn's first
    // step. A third steer while it runs is read at the next boundary.
    assert!(a.wait_turn("root", "running", Duration::from_secs(10)));
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "S3", "steer": true}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    // Give a turn the fault would open time to run, then look.
    std::thread::sleep(Duration::from_secs(3));

    let users = user_lines(&k.place);
    assert_eq!(
        users,
        vec!["hello", "S1", "S2", "S3"],
        "the person's words in the order they were said:\n{:#?}",
        transcript(&k.place)
    );
    let turns = transcript(&k.place)
        .iter()
        .filter(|e| e["kind"] == "turn_complete")
        .count();
    assert_eq!(
        turns,
        2,
        "S2 rode the turn S1 opened, not one of its own:\n{:#?}",
        transcript(&k.place)
    );
    let _ = k.child.kill();
}
