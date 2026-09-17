//! Acceptance journey J4/J5, the kernel's half: a coordinator parked in
//! `spawn wait=true` for the minutes a worker takes must still take a
//! mid-flight follow-up now, not after the worker finishes. The wait
//! ends when the user speaks; the words are read at that tool boundary;
//! the worker keeps working and its report arrives as a message.

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
fn a_waiting_coordinator_takes_the_users_words_before_the_worker_finishes() {
    let replies = concat!(
        // root: spawn and wait. The worker sleeps twelve seconds.
        "{\"agent\":\"root\",\"content\":\"delegating\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"builder\",\"task\":\"Run the slow build. Do: bash `sleep 12; echo built`. Then report the word built.\",\"wait\":true}}]}\n",
        "{\"agent\":\"builder\",\"content\":\"building\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 12; echo built\",\"description\":\"Slow build\"}}]}\n",
        // root, after the wait yields to the user's words: answers them.
        "{\"agent\":\"root\",\"content\":\"It is roughly early afternoon. builder is still building; I will report when it finishes.\"}\n",
        "{\"agent\":\"builder\",\"content\":\"built\"}\n",
        // root's done turn.
        "{\"agent\":\"root\",\"content\":\"builder finished: built.\"}\n",
    );
    let mut k = start_kernel_replay("wait-yields-steer", replies);
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(serde_json::json!({"type":"user","agent":"root","text":"build it","attachments":[]}));
    // The worker is up and in its bash.
    assert!(
        a.wait_turn("builder", "running", Duration::from_secs(15)),
        "the worker starts"
    );
    std::thread::sleep(Duration::from_millis(1500));
    let asked_at = Instant::now();
    a.send(serde_json::json!({"type":"user","agent":"root","text":"What time is it, roughly? One line.","steer":true,"attachments":[]}));

    // Root answers now — long before the worker's twelve seconds are up.
    let answer = a
        .wait(Duration::from_secs(8), |f| {
            f["type"] == "event"
                && f["agent"] == "root"
                && f["event"]["kind"] == "assistant"
                && f["event"]["text"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("It is roughly"))
        })
        .expect("root answers the steer while the worker still works");
    let took = asked_at.elapsed();
    assert!(
        took < Duration::from_secs(7),
        "answered in {took:?}, not after the worker"
    );
    let _ = answer;
    // The answer is on the wire before the turn's end is on disk; wait for
    // the `turn_complete` line to be announced before reading the
    // transcript, or a loaded runner shows the turn still open.
    assert!(
        a.wait(Duration::from_secs(8), |f| {
            f["type"] == "event" && f["agent"] == "root" && f["event"]["kind"] == "turn_complete"
        })
        .is_some(),
        "root's turn ends after the answer, before the worker's"
    );

    let root = transcript(&k.place, "root");
    let spawn = root
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "spawn")
        .expect("the spawn record");
    let body = spawn["body"].as_str().unwrap_or_default();
    assert!(
        body.contains("builder is still working; the user said something meanwhile"),
        "{body}"
    );
    assert!(body.contains("say to=builder mode=steer"), "{body}");
    // J4's check: the follow-up is on the transcript before the turn ends.
    let steer_at = root
        .iter()
        .position(|e| {
            e["kind"] == "user"
                && e["text"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("What time"))
        })
        .expect("the steer is a user line on root's transcript");
    let first_end = root
        .iter()
        .position(|e| e["kind"] == "turn_complete")
        .expect("root's turn ended");
    assert!(
        steer_at < first_end,
        "the words were taken in the same turn"
    );
    assert!(
        transcript(&k.place, "builder")
            .iter()
            .all(|e| e["kind"] != "turn_complete"),
        "the worker was not stopped"
    );

    // The worker finishes and its report reaches root as a done turn.
    assert!(
        a.wait(Duration::from_secs(30), |f| {
            f["type"] == "event"
                && f["agent"] == "root"
                && f["event"]["kind"] == "assistant"
                && f["event"]["text"] == "builder finished: built."
        })
        .is_some(),
        "the worker's report arrives as a message"
    );
    let _ = k.child.kill();
}
