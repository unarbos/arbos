//! Jacob's phone, 2026-09-17: he paused for under a second mid-sentence,
//! and the project's chat showed three lines for one spoken sentence —
//! the half sentence as a user line, "ⓘ stop during model call", then the
//! whole sentence as a second user line. The speech gateway was right to
//! stop the superseded turn before asking the merged question (that is
//! what fixed an answer arriving twice); the record was wrong to describe
//! the kernel's internals instead of what the person did.
//!
//! Driven: a turn is stopped with `reason: "superseded"` during its model
//! call and the fuller message follows. The transcript ends with one
//! user line for the utterance and no stop notice; the half-said line is
//! in the rewind archive, not the chat; nothing queued is held. A turn
//! that had already spoken keeps its lines, with `superseded` on the
//! interrupted record for a client to draw softly.

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

const HALF: &str = "Hello Arbos. What are we working on right now?";
const WHOLE: &str = "Hello Arbos. What are we working on right now? Give me one sentence.";

#[test]
fn a_turn_superseded_during_its_model_call_leaves_one_user_line_and_no_stop_notice() {
    let replies = concat!(
        // The model is slow; the stop lands during the half sentence's
        // call, which never takes the line — the merged question does.
        "{\"agent\":\"root\",\"content\":\"One sentence: the leash work.\",\"delay_ms\":6000}\n",
    );
    let mut k = start_kernel_replay("superseded", replies);
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(serde_json::json!({"type":"user","agent":"root","text":HALF,"attachments":[]}));
    assert!(a.wait_turn("root", "running", Duration::from_secs(10)));
    // The kernel wrote the half sentence before the model was asked.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !transcript(&k.place)
        .iter()
        .any(|e| e["kind"] == "user" && e["text"] == HALF)
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the half sentence never landed"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // The gateway's shape: stop with the reason, then the merged sentence.
    a.send(serde_json::json!({"type":"stop","agent":"root","reason":"superseded"}));
    a.send(serde_json::json!({"type":"user","agent":"root","text":WHOLE,"attachments":[]}));
    let answer = a
        .wait(Duration::from_secs(20), |f| {
            f["type"] == "event"
                && f["agent"] == "root"
                && f["event"]["kind"] == "assistant"
                && f["event"]["text"]
                    .as_str()
                    .is_some_and(|t| t.contains("One sentence"))
        })
        .expect("the merged question is answered");
    let _ = answer;
    // The idle frame can precede the tail's events; wait for the record's close.
    a.wait(Duration::from_secs(10), |f| {
        f["type"] == "event" && f["agent"] == "root" && f["event"]["kind"] == "turn_complete"
    })
    .expect("the merged turn completes");

    let events = transcript(&k.place);
    let users: Vec<&str> = events
        .iter()
        .filter(|e| e["kind"] == "user")
        .filter_map(|e| e["text"].as_str())
        .collect();
    assert_eq!(
        users,
        vec![WHOLE],
        "one user line for one utterance: {events:#?}"
    );
    assert!(
        !events.iter().any(|e| e["kind"] == "interrupted"),
        "no stop notice for a turn no one chose to stop: {events:#?}"
    );
    assert!(
        !events.iter().any(
            |e| e["kind"] == "notice" && e["text"].as_str().is_some_and(|t| t.contains("stop"))
        ),
        "{events:#?}"
    );
    // What was heard is not lost: the half sentence is in the archive.
    let archived = std::fs::read_dir(k.place.join(".arbos/agents/root"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .flat_map(|d| {
            std::fs::read_dir(d)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
        })
        .chain(
            std::fs::read_dir(k.place.join(".arbos/agents/root"))
                .unwrap()
                .flatten()
                .map(|e| e.path()),
        )
        .filter(|p| p.is_file())
        .filter(|p| !p.ends_with("transcript.jsonl"))
        .any(|p| std::fs::read_to_string(&p).is_ok_and(|t| t.contains(HALF)));
    assert!(
        archived,
        "the half sentence is kept out of the chat, not destroyed"
    );
    // The record of the answer's cause is intact: wake, user, assistant, complete.
    let kinds: Vec<&str> = events.iter().filter_map(|e| e["kind"].as_str()).collect();
    let wake_at = kinds.iter().rposition(|k| *k == "wake").expect("a wake");
    assert_eq!(kinds[wake_at + 1], "user", "{kinds:?}");
    let _ = k.child.kill();
}

#[test]
fn a_superseded_turn_that_had_already_spoken_keeps_its_lines_and_says_superseded() {
    let replies = concat!(
        // Spoke and ran a tool, then a slow second step; superseded during
        // that. The slow line goes to the merged question.
        "{\"agent\":\"root\",\"content\":\"Looking.\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"echo looked\",\"description\":\"Look\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"One sentence: found it.\",\"delay_ms\":6000}\n",
    );
    let mut k = start_kernel_replay("superseded-spoke", replies);
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(serde_json::json!({"type":"user","agent":"root","text":HALF,"attachments":[]}));
    // The tool ran: the turn has done something a reader would miss.
    a.wait(Duration::from_secs(15), |f| {
        f["type"] == "event"
            && f["event"]["kind"] == "tool"
            && f["event"]["name"] == "bash"
            && f["event"]["ended"].is_number()
    })
    .expect("the tool ran");
    a.send(serde_json::json!({"type":"stop","agent":"root","reason":"superseded"}));
    a.send(serde_json::json!({"type":"user","agent":"root","text":WHOLE,"attachments":[]}));
    a.wait(Duration::from_secs(20), |f| {
        f["type"] == "event"
            && f["event"]["kind"] == "assistant"
            && f["event"]["text"]
                .as_str()
                .is_some_and(|t| t.contains("One sentence"))
    })
    .expect("the merged question is answered");
    a.wait(Duration::from_secs(10), |f| {
        f["type"] == "event" && f["agent"] == "root" && f["event"]["kind"] == "turn_complete"
    })
    .expect("the merged turn completes");
    let events = transcript(&k.place);
    let users: Vec<&str> = events
        .iter()
        .filter(|e| e["kind"] == "user")
        .filter_map(|e| e["text"].as_str())
        .collect();
    assert_eq!(
        users,
        vec![HALF, WHOLE],
        "the turn that ran a tool keeps its record"
    );
    let interrupted = events
        .iter()
        .find(|e| e["kind"] == "interrupted")
        .expect("the stop is on the record, since work ran");
    assert!(
        interrupted["detail"]
            .as_str()
            .is_some_and(|d| d.starts_with("superseded")),
        "the reason is the client's word, not the kernel's: {interrupted}"
    );
    assert!(
        events
            .iter()
            .any(|e| e["kind"] == "tool" && e["name"] == "bash")
    );
    let _ = k.child.kill();
}
