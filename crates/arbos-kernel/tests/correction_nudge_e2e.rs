//! Projects-post gap 3: "with each turn of feedback, the Project learns
//! your architecture and preferences." Arbos had `remember` and the prompt
//! rule; nothing checked that a correction was kept. Now a turn opened by
//! a user line that reads as a correction or a standing preference, with
//! no `remember` and no edit to the context or memory file in it, gets one
//! `correction not kept` reminder at its end; a turn that kept it, or a
//! plain task, gets none.

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

fn nudges(place: &Path) -> Vec<serde_json::Value> {
    transcript(place, "root")
        .into_iter()
        .filter(|e| e["kind"] == "nudge" && e["reason"] == "correction not kept")
        .collect()
}

const REPLIES: &str = concat!(
    // Turn 1: a correction, nothing kept.
    "{\"agent\":\"root\",\"content\":\"Understood, drafts from now on.\"}\n",
    // Turn 2: a correction, kept with remember.
    "{\"agent\":\"root\",\"content\":\"Noted.\",\"calls\":[{\"name\":\"remember\",\"arguments\":{\"text\":\"Reply in short answers; the user prefers brevity.\",\"scope\":\"user\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"Kept.\"}\n",
    // Turn 3: a plain task.
    "{\"agent\":\"root\",\"content\":\"There are three tests.\"}\n",
);

#[test]
fn an_unkept_correction_is_reminded_once_and_a_kept_one_is_not() {
    let mut k = start_kernel_replay("correction-nudge", REPLIES);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );

    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "No, always open PRs as drafts, never ready for review."}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert!(common::wait_for(Duration::from_secs(5), || !nudges(
        &k.place
    )
    .is_empty()));
    let n = nudges(&k.place);
    assert_eq!(n.len(), 1, "{n:#?}");
    let text = n[0]["text"].as_str().unwrap_or("");
    assert!(text.starts_with("correction not kept"), "{text}");
    assert!(
        text.contains("always open PRs as drafts"),
        "the user's line is quoted: {text}"
    );
    assert!(text.contains("remember") && text.contains("project-context.md"));
    // The reminder is on the transcript after the turn, where the next
    // turn's model reads it.
    let root = transcript(&k.place, "root");
    let complete = root
        .iter()
        .position(|e| e["kind"] == "turn_complete")
        .unwrap();
    let nudge = root
        .iter()
        .position(|e| e["kind"] == "nudge" && e["reason"] == "correction not kept")
        .unwrap();
    assert!(nudge > complete, "{root:#?}");

    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "I prefer short answers."}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    std::thread::sleep(Duration::from_millis(700));
    assert_eq!(
        nudges(&k.place).len(),
        1,
        "kept with remember: no second reminder"
    );

    a.send(
        serde_json::json!({"type": "user", "agent": "root", "text": "How many tests are there?"}),
    );
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    std::thread::sleep(Duration::from_millis(700));
    assert_eq!(
        nudges(&k.place).len(),
        1,
        "a plain question is not a correction"
    );
    let _ = k.child.kill();
}
