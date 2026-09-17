//! A coordinator that goes silent for minutes while its worker runs is
//! what Jacob watched (2m 26s, "run it" four times). The parent's live
//! line now follows the worker it waits on — "waiting on <name> — <the
//! worker's step>", the worker's own clock — in both shapes (spawn-first
//! with the parent idle; `spawn wait=true` with the parent blocked), and
//! goes the moment no worker is live: a stale "waiting on" is worse than
//! none.

mod common;

use common::{Attach, start_kernel_replay};
use std::time::{Duration, Instant};

fn status_file(place: &std::path::Path, agent: &str) -> Option<String> {
    std::fs::read_to_string(place.join(".arbos/agents").join(agent).join("status.toml")).ok()
}

#[test]
fn the_parent_says_waiting_on_its_worker_and_the_line_goes_when_the_worker_is_done() {
    let replies = concat!(
        // Spawn-first: root delegates and its turn ends.
        "{\"agent\":\"root\",\"content\":\"delegating\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"Slow builder\",\"task\":\"Do: bash `sleep 5; echo built` (description Building the thing). Report built.\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"Slow builder is on it.\"}\n",
        "{\"agent\":\"slow-builder\",\"content\":\"building\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 5; echo built\",\"description\":\"Building the thing\"}}]}\n",
        "{\"agent\":\"slow-builder\",\"content\":\"built\"}\n",
        "{\"agent\":\"root\",\"content\":\"Built.\"}\n",
    );
    let mut k = start_kernel_replay("waiting-on-line", replies);
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(serde_json::json!({"type":"user","agent":"root","text":"build it","attachments":[]}));
    // Root's line, while it is idle and the worker works: the worker's
    // step, under the worker's name as the user gave it.
    let waiting = a
        .wait(Duration::from_secs(20), |f| {
            f["type"] == "status"
                && f["agent"] == "root"
                && f["source"] == "waiting"
                && f["step"]
                    .as_str()
                    .is_some_and(|s| s.contains("Running sleep 5"))
        })
        .expect("root says what it is waiting on");
    let step = waiting["step"].as_str().unwrap();
    assert!(step.starts_with("waiting on Slow builder — "), "{step}");
    assert!(
        waiting["since"].as_str().is_some_and(|s| !s.is_empty()),
        "the worker's clock: {waiting}"
    );
    // Not a transcript line: the chat stays quiet.
    let root_transcript =
        std::fs::read_to_string(k.place.join(".arbos/agents/root/transcript.jsonl")).unwrap();
    assert!(!root_transcript.contains("waiting on"), "{root_transcript}");
    // On disk too, so a client attaching now reads it in the snapshot.
    assert!(status_file(&k.place, "root").is_some_and(
        |t| t.contains("waiting on Slow builder") && t.contains("source = \"waiting\"")
    ));

    // The worker finishes: the line goes (an empty status frame), root's
    // done turn runs and ends, and nothing lingers.
    let cleared = a
        .wait(Duration::from_secs(30), |f| {
            f["type"] == "status" && f["agent"] == "root" && f["step"] == ""
        })
        .expect("the waiting line is cleared");
    let _ = cleared;
    assert!(
        a.wait(Duration::from_secs(20), |f| f["type"] == "event"
            && f["agent"] == "root"
            && f["event"]["kind"] == "assistant"
            && f["event"]["text"] == "Built.")
            .is_some()
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while status_file(&k.place, "root").is_some() {
        assert!(
            Instant::now() < deadline,
            "no stale line: {:?}",
            status_file(&k.place, "root")
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = k.child.kill();
}

#[test]
fn a_parent_blocked_in_spawn_wait_shows_its_workers_step_too() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"waiting\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"Test runner\",\"task\":\"Do: bash `sleep 5; echo ok` (description Running the tests). Report ok.\",\"wait\":true}}]}\n",
        "{\"agent\":\"test-runner\",\"content\":\"testing\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 5; echo ok\",\"description\":\"Running the tests\"}}]}\n",
        "{\"agent\":\"test-runner\",\"content\":\"ok\"}\n",
        "{\"agent\":\"root\",\"content\":\"Tests pass.\"}\n",
    );
    let mut k = start_kernel_replay("waiting-on-line-blocked", replies);
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(
        serde_json::json!({"type":"user","agent":"root","text":"run the tests","attachments":[]}),
    );
    let waiting = a
        .wait(Duration::from_secs(20), |f| {
            f["type"] == "status"
                && f["agent"] == "root"
                && f["source"] == "waiting"
                && f["step"]
                    .as_str()
                    .is_some_and(|s| s.contains("Running sleep 5"))
        })
        .expect("root, blocked in spawn wait, shows the worker's step");
    assert!(
        waiting["step"]
            .as_str()
            .unwrap()
            .starts_with("waiting on Test runner — "),
        "{waiting}"
    );
    assert!(
        a.wait(Duration::from_secs(30), |f| f["type"] == "event"
            && f["agent"] == "root"
            && f["event"]["kind"] == "assistant"
            && f["event"]["text"] == "Tests pass.")
            .is_some()
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while status_file(&k.place, "root").is_some() {
        assert!(
            Instant::now() < deadline,
            "no stale line: {:?}",
            status_file(&k.place, "root")
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = k.child.kill();
}
