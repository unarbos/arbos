//! Mobile cycle 5: the repeat guard from #236 compared bytes, and the
//! model reworded ("if needed" → "if necessary") — the reader saw the
//! same paragraph twice. Now a reply whose words overlap an earlier one
//! almost entirely is the same reply: said once. A final reply that
//! repeats something said this turn, or the last reply before a wake the
//! user did not send, is not written; a notice takes its place and the
//! turn ends.

mod common;

use common::{Attach, start_kernel_replay};
use std::path::Path;
use std::time::Duration;

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

const A: &str = "The connection to the sub-agent on arboslife has been lost, and the remote kernel may still be active but currently unresponsive here. To continue, you may need to restart this kernel to re-attach or respawn the sub-agent if needed. Let me know how you'd like to proceed.";
const B: &str = "The connection to the sub-agent on arboslife has been lost, and the remote kernel may still be active but is currently unresponsive here. To continue, you may need to restart this kernel to re-attach or respawn the sub-agent if necessary. Let me know how you'd like to proceed.";

#[test]
fn a_reworded_final_reply_that_repeats_this_turn_is_not_said_again() {
    let replies = format!(
        "{{\"agent\":\"root\",\"content\":{a},\"calls\":[{{\"name\":\"status\",\"arguments\":{{\"text\":\"Checking the link\"}}}}]}}\n{{\"agent\":\"root\",\"content\":{b}}}\n",
        a = serde_json::to_string(A).unwrap(),
        b = serde_json::to_string(B).unwrap(),
    );
    let mut k = start_kernel_replay("repeat-reworded", &replies);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "what happened to the worker?"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert!(common::wait_for(Duration::from_secs(5), || {
        transcript(&k.place, "root")
            .iter()
            .any(|e| e["kind"] == "turn_complete")
    }));
    let root = transcript(&k.place, "root");
    let said: Vec<&str> = root
        .iter()
        .filter(|e| e["kind"] == "assistant")
        .filter_map(|e| e["text"].as_str())
        .filter(|t| !t.trim().is_empty())
        .collect();
    assert_eq!(said, vec![A], "the paragraph once: {root:#?}");
    assert!(
        root.iter().any(|e| e["kind"] == "notice"
            && e["text"]
                .as_str()
                .unwrap_or("")
                .contains("repeated something said earlier this turn")),
        "{root:#?}"
    );
    assert!(
        !root
            .iter()
            .any(|e| e["kind"] == "nudge" && e["reason"] == "empty reply"),
        "{root:#?}"
    );
    let _ = k.child.kill();
}

// The worker pauses before reporting, so its report lands after root's
// turn and opens the done wake this test is about; answering at once, it
// folded into root's running turn and no done turn came.
const REPLIES_DONE: &str = concat!(
    "{\"agent\":\"root\",\"content\":\"one worker\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"first\",\"task\":\"say sentence one\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"I have started the worker on the sentence; it will report back here when it is finished.\"}\n",
    "{\"agent\":\"first\",\"content\":\"pausing\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 3\",\"description\":\"Wait a moment\"}}]}\n",
    "{\"agent\":\"first\",\"content\":\"Sentence one.\"}\n",
    "{\"agent\":\"root\",\"content\":\"I have started the worker on the sentence — it will report back here when it has finished.\"}\n",
);

/// On a done wake the root repeated its previous answer, reworded: not
/// said again — the user read it once; the turn ends with a notice.
#[test]
fn a_reply_on_a_done_wake_that_repeats_the_previous_answer_is_not_said_again() {
    let mut k = start_kernel_replay("repeat-across-turns", REPLIES_DONE);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(
        serde_json::json!({"type": "user", "agent": "root", "text": "one sentence from a worker"}),
    );
    assert!(common::wait_for(Duration::from_secs(60), || {
        transcript(&k.place, "root")
            .iter()
            .filter(|e| e["kind"] == "turn_complete")
            .count()
            >= 2
    }));
    let root = transcript(&k.place, "root");
    let said: Vec<&str> = root
        .iter()
        .filter(|e| e["kind"] == "assistant")
        .filter_map(|e| e["text"].as_str())
        .filter(|t| t.contains("started the worker"))
        .collect();
    assert_eq!(said.len(), 1, "the answer once: {root:#?}");
    assert!(
        said[0].contains("when it is finished"),
        "the first wording stays"
    );
    assert!(
        root.iter().any(|e| e["kind"] == "notice"
            && e["text"]
                .as_str()
                .unwrap_or("")
                .contains("repeated the previous message")),
        "{root:#?}"
    );
    let _ = k.child.kill();
}
