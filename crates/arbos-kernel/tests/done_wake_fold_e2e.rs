//! Cold-track cycle 11 (cold-p5, cold-pp2): after a worker's done the root
//! woke, said "I have completed the following steps:", then an empty
//! reply, was nudged, and stayed Working; with three workers it relayed
//! each report as it arrived and gave the combined answer twice. Now a
//! worker's done opens a `done` wake whose kernel line names who reported
//! and who is still working; an empty reply on it ends the turn with no
//! nudge; the last report's line says to answer once, combined.

mod common;

use common::{Attach, start_kernel_replay};
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

// Both workers pause before reporting, so both reports land after root's
// dispatch turn has ended and each opens a done wake — the shape this
// test reads. A worker answering in a millisecond reported while root was
// still on its first turn; the report folded in and no done turn came
// (the red on #523).
const REPLIES: &str = concat!(
    "{\"agent\":\"root\",\"content\":\"two workers\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"first\",\"task\":\"say sentence one\"}},{\"name\":\"spawn\",\"arguments\":{\"name\":\"second\",\"task\":\"say sentence two\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"Both are under way.\"}\n",
    "{\"agent\":\"first\",\"content\":\"pausing\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 2\",\"description\":\"Wait a moment\"}}]}\n",
    "{\"agent\":\"first\",\"content\":\"Sentence one.\"}\n",
    "{\"agent\":\"second\",\"content\":\"pausing\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 5\",\"description\":\"Wait a moment\"}}]}\n",
    "{\"agent\":\"second\",\"content\":\"Sentence two.\"}\n",
    "{\"agent\":\"root\",\"content\":\"\"}\n",
    "{\"agent\":\"root\",\"content\":\"Sentence one. Sentence two.\"}\n",
);

#[test]
fn a_done_wake_names_who_is_still_working_and_an_empty_reply_ends_it_quietly() {
    let mut k = start_kernel_replay("done-fold", REPLIES);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "two sentences, one from each worker"}));

    // Three root turns: the dispatch, the first done (silent), the last.
    assert!(
        wait_for(Duration::from_secs(60), || {
            transcript(&k.place, "root")
                .iter()
                .filter(|e| e["kind"] == "turn_complete")
                .count()
                >= 3
        }),
        "root: {:#?}",
        transcript(&k.place, "root")
    );
    let root = transcript(&k.place, "root");
    let dones: Vec<&serde_json::Value> = root
        .iter()
        .filter(|e| e["kind"] == "wake" && e["wake"] == "done")
        .collect();
    assert_eq!(dones.len(), 2, "one done wake per report: {root:#?}");
    let first = dones[0]["text"].as_str().unwrap_or("");
    assert!(
        first.contains("Report from first") && first.contains("Still working: second"),
        "the first done names the reporter and who is still working: {first}"
    );
    assert!(first.contains("end this turn with no message"), "{first}");
    let last = dones[1]["text"].as_str().unwrap_or("");
    assert!(
        last.contains("Report from second") && last.contains("the last of your workers"),
        "the last done says so: {last}"
    );
    assert!(
        !root
            .iter()
            .any(|e| e["kind"] == "nudge" && e["reason"] == "empty reply"),
        "an empty reply on a done wake is not nudged: {root:#?}"
    );
    // The done line is the wake's text, not a say: the worker's report is
    // still the one say line per worker.
    let says: Vec<&str> = root
        .iter()
        .filter(|e| e["kind"] == "say")
        .filter_map(|e| e["from"].as_str())
        .collect();
    assert_eq!(says, vec!["first", "second"], "{root:#?}");
    // The silent turn wrote no assistant text; the last one answered once.
    let answers: Vec<&str> = root
        .iter()
        .filter(|e| e["kind"] == "assistant")
        .filter_map(|e| e["text"].as_str())
        .filter(|t| !t.trim().is_empty())
        .collect();
    assert_eq!(
        answers,
        vec![
            "two workers",
            "Both are under way.",
            "Sentence one. Sentence two."
        ],
        "{root:#?}"
    );
    let _ = k.child.kill();
}

const LOOPING: &str = concat!(
    "{\"agent\":\"root\",\"content\":\"Please provide the sentences from the sub-agents so I can combine them once both are available.\",\"calls\":[{\"name\":\"status\",\"arguments\":{\"text\":\"Waiting for workers\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"Please provide the sentences from the sub-agents so I can combine them once both are available.\",\"calls\":[{\"name\":\"status\",\"arguments\":{\"text\":\"Waiting for workers\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"Please provide the sentences from the sub-agents  so I can combine them once both are available.\",\"calls\":[{\"name\":\"status\",\"arguments\":{\"text\":\"Still waiting\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"Please provide the sentences from the sub-agents so I can combine them once both are available.\",\"calls\":[{\"name\":\"status\",\"arguments\":{\"text\":\"Waiting more\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"never reached\"}\n",
);

/// Mobile cycle 1, item 4: the demo coordinator streamed the same waiting
/// paragraph five to eight times in one turn between status calls. The
/// second time it is told once; the third ends the turn.
#[test]
fn the_same_paragraph_three_times_in_one_turn_ends_it() {
    let mut k = start_kernel_replay("repeat-loop", LOOPING);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(
        serde_json::json!({"type": "user", "agent": "root", "text": "combine the two sentences"}),
    );
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert!(wait_for(Duration::from_secs(5), || {
        transcript(&k.place, "root")
            .iter()
            .any(|e| e["kind"] == "turn_complete")
    }));
    let root = transcript(&k.place, "root");
    let said: Vec<&str> = root
        .iter()
        .filter(|e| e["kind"] == "assistant")
        .filter_map(|e| e["text"].as_str())
        .collect();
    assert_eq!(said.len(), 3, "the third repeat is the last: {root:#?}");
    assert!(!said.contains(&"never reached"));
    let nudges: Vec<&serde_json::Value> = root
        .iter()
        .filter(|e| e["kind"] == "nudge" && e["reason"] == "repeated reply")
        .collect();
    assert_eq!(nudges.len(), 1, "told once, at the second: {root:#?}");
    assert!(
        nudges[0]["text"]
            .as_str()
            .unwrap_or("")
            .contains("end the turn now with no tool calls")
    );
    assert!(
        root.iter().any(|e| e["kind"] == "notice"
            && e["text"]
                .as_str()
                .unwrap_or("")
                .contains("third time in one turn")),
        "{root:#?}"
    );
    // Order: reply, status, reply, status, nudge, reply, status, notice, end.
    let last = root.last().unwrap();
    assert_eq!(last["kind"], "turn_complete", "{last:#?}");
    let _ = k.child.kill();
}
