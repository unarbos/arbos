//! Acceptance journey J8a (`docs/acceptance-journeys.md`), the kernel's
//! half: the kernel dies while a turn runs; the next kernel picks the
//! turn up from the transcript (the `serve` wake) and ends it; a line
//! typed after the restart is answered once, with no failed notice left
//! behind and no duplicate of anything.

mod common;

use common::{Attach, restart_replay, start_kernel_replay};
use std::time::Duration;

fn transcript(place: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn kernel_pid(place: &std::path::Path) -> u64 {
    let text = std::fs::read_to_string(place.join(".arbos/runtime/kernel.json")).unwrap();
    serde_json::from_str::<serde_json::Value>(&text).unwrap()["pid"]
        .as_u64()
        .unwrap()
}

#[test]
fn a_kernel_killed_mid_turn_is_picked_up_and_the_next_line_is_answered_once() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"working\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 8; echo slept\",\"description\":\"Sleep\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"first done\"}\n",
    );
    let mut k = start_kernel_replay("restart-mid-turn", replies);
    let pid_before = kernel_pid(&k.place);
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(serde_json::json!({"type":"user","agent":"root","text":"do the slow thing","attachments":[]}));
    assert!(a.wait_turn("root", "running", Duration::from_secs(10)));
    // Into the bash: the kernel is killed (SIGKILL, as a crash would) with
    // the turn open and a tool in flight.
    std::thread::sleep(Duration::from_millis(2500));
    let events = transcript(&k.place);
    assert!(
        events.iter().all(|e| e["kind"] != "turn_complete"),
        "{events:?}"
    );

    let after = concat!(
        "{\"agent\":\"root\",\"content\":\"continued after restart\"}\n",
        "{\"agent\":\"root\",\"content\":\"answer to the second line\"}\n",
    );
    let mut k2 = restart_replay(&mut k, after);
    assert_ne!(
        kernel_pid(&k2.place),
        pid_before,
        "kernel.json names the new kernel"
    );
    let mut b = Attach::connect(&k2.url);
    let _ = b.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    b.send(serde_json::json!({"type":"user","agent":"root","text":"second line after restart","attachments":[]}));
    let answer = b
        .wait(Duration::from_secs(20), |f| {
            f["type"] == "event"
                && f["event"]["kind"] == "assistant"
                && f["event"]["text"] == "answer to the second line"
        })
        .expect("the second line is answered");
    assert_eq!(answer["agent"], "root");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while transcript(&k2.place)
        .iter()
        .filter(|e| e["kind"] == "turn_complete")
        .count()
        < 2
    {
        assert!(std::time::Instant::now() < deadline, "the second turn ends");
        std::thread::sleep(Duration::from_millis(50));
    }

    let events = transcript(&k2.place);
    let kinds: Vec<&str> = events.iter().map(|e| e["kind"].as_str().unwrap()).collect();
    // The cut turn: its user line, then the serve wake that picked it up,
    // a reply, and its end — before the second line's turn.
    let serve_at = events
        .iter()
        .position(|e| e["kind"] == "wake" && e["wake"] == "serve")
        .expect("the serve wake continued the cut turn: {kinds:?}");
    let first_end = events
        .iter()
        .position(|e| e["kind"] == "turn_complete")
        .expect("the cut turn ended");
    assert!(serve_at < first_end, "{kinds:?}");
    let second_user = events
        .iter()
        .position(|e| e["kind"] == "user" && e["text"] == "second line after restart")
        .expect("the second line is on the transcript");
    assert!(
        first_end < second_user,
        "the second line waited for the cut turn to end: {kinds:?}"
    );
    assert_eq!(
        events.iter().filter(|e| e["kind"] == "user").count(),
        2,
        "each line exactly once: {kinds:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e["kind"] == "assistant" && e["text"] == "answer to the second line")
            .count(),
        1,
        "answered once: {kinds:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e["kind"] == "turn_complete")
            .count(),
        2,
        "{kinds:?}"
    );
    assert!(
        events
            .iter()
            .all(|e| !(e["kind"] == "notice" && e["failed"] == true)),
        "no failed notice left behind: {events:?}"
    );
    assert!(
        events.iter().all(|e| e["kind"] != "interrupted"),
        "{kinds:?}"
    );
    let _ = k2.child.kill();
}
