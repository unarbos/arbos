//! Symmetry cycle 7, the worker chat's three asks: a `thinking` record
//! per model step that reasoned, with `secs`; a worker's thinking deltas
//! on the attach stream while it runs; and the brief alone on the child's
//! first wake, beside the framed text.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::path::Path;
use std::time::{Duration, Instant};

fn transcript(place: &Path, agent: &str) -> Vec<serde_json::Value> {
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

fn wait_for(timeout: Duration, mut ok: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    ok()
}

#[test]
fn a_worker_chat_gets_the_brief_the_thought_and_its_duration() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"one worker\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"w1\",\"task\":\"say the codeword\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"started\"}\n",
        // The worker thinks, then answers.
        "{\"content\":\"the codeword is xylophone\",\"thinking\":\"The codeword. It must be xylophone; the brief says so.\"}\n",
        "{\"agent\":\"root\",\"content\":\"noted\"}\n",
    );
    let mut k = start_kernel_replay_prepared("worker-chat", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"coordinator\"\narchive_children = false\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "get the codeword"}));
    // Ask 2: the worker's thought reaches an attached client as it streams.
    let delta = a.wait(Duration::from_secs(40), |f| {
        f["type"] == "thinking_delta" && f["agent"] == "w1"
    });
    assert!(delta.is_some(), "a thinking_delta frame for the worker");
    assert!(
        delta.unwrap()["text"]
            .as_str()
            .unwrap_or("")
            .contains("xylophone")
    );
    assert!(
        wait_for(Duration::from_secs(40), || transcript(&k.place, "w1")
            .iter()
            .any(|e| e["kind"] == "turn_complete")),
        "w1's turn ends"
    );
    let w1 = transcript(&k.place, "w1");
    // The brief alone, beside the framed text (the small ask).
    let wake = w1
        .iter()
        .find(|e| e["kind"] == "wake")
        .expect("the worker's first wake");
    let text = wake["text"].as_str().unwrap_or("");
    let brief = wake["brief"].as_str().unwrap_or("");
    assert!(
        text.starts_with("You were spawned by agent root for this mission:"),
        "{text}"
    );
    assert!(
        brief.starts_with("Read first:") && brief.contains("Task: say the codeword"),
        "the brief as the parent wrote it: {brief}"
    );
    assert!(!brief.contains("You were spawned"), "{brief}");
    assert!(
        text.contains(brief),
        "the framed text holds the brief whole"
    );
    // Ask 1: one settled thinking record, with its duration, before the
    // step's assistant line.
    let thoughts: Vec<usize> = w1
        .iter()
        .enumerate()
        .filter(|(_, e)| e["kind"] == "thinking")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(thoughts.len(), 1, "{w1:#?}");
    let thought = &w1[thoughts[0]];
    assert!(
        thought["text"].as_str().unwrap_or("").contains("xylophone"),
        "{thought:#?}"
    );
    assert!(thought["secs"].is_u64(), "secs on the record: {thought:#?}");
    let reply = w1
        .iter()
        .position(|e| e["kind"] == "assistant" && e["text"] == "the codeword is xylophone")
        .expect("the reply");
    assert!(
        thoughts[0] < reply,
        "the thought is recorded before the reply"
    );
    // Root, which did not think, has no thinking record.
    assert!(
        !transcript(&k.place, "root")
            .iter()
            .any(|e| e["kind"] == "thinking")
    );
    let _ = k.child.kill();
}
