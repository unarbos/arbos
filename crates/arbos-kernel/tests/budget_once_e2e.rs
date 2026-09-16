//! JB-4 (2026-09-16, the phone's `pod` kernel): a config.toml pin of
//! `window_tokens = 32000` left from a small-window model made every
//! turn over budget with nothing old enough to compact — said thirteen
//! times in one turn — while a failed `write` was reported as "seeded".
//! Three kernel answers: the stale pin is loud once at start, the
//! over-budget notice is said once per turn, and a reply that reads as
//! done after a failed write is nudged.

mod common;

use common::{Attach, restart_replay, start_kernel_replay_with};
use std::time::Duration;

fn transcript(place: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[test]
fn a_small_pinned_window_is_loud_once_and_over_budget_is_said_once_per_turn() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"looking\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"echo one\",\"description\":\"One\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"again\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"echo two\",\"description\":\"Two\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"and once more\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"echo three\",\"description\":\"Three\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"Three echoes ran.\"}\n",
    );
    // A pin far under the standing prompt (contract + tools ≈ 10k+).
    let mut k = start_kernel_replay_with("budget-once", replies, "window_tokens = 6000\n");
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");

    // At start: one failed notice naming the pin and the size it needs.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let pin = loop {
        if let Some(n) = transcript(&k.place).into_iter().find(|e| {
            e["kind"] == "notice"
                && e["text"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("config.toml pins window_tokens = 6000"))
        }) {
            break n;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the pin is named at start"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(pin["failed"], true);
    let text = pin["text"].as_str().unwrap();
    assert!(
        text.contains("standing prompt is ~") && text.contains("needs a window of about"),
        "{text}"
    );
    assert!(text.contains("Remove the pin"), "{text}");

    // One turn, four model steps, every one over budget: said once.
    a.send(serde_json::json!({"type":"user","agent":"root","text":"run three echoes","attachments":[]}));
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !transcript(&k.place)
        .iter()
        .any(|e| e["kind"] == "turn_complete")
    {
        assert!(std::time::Instant::now() < deadline, "the turn ends");
        std::thread::sleep(Duration::from_millis(100));
    }
    let events = transcript(&k.place);
    let over: Vec<&str> = events
        .iter()
        .filter(|e| {
            e["kind"] == "notice"
                && e["text"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("over budget"))
        })
        .map(|e| e["text"].as_str().unwrap())
        .collect();
    assert_eq!(over.len(), 1, "said once per turn: {over:?}");
    assert!(
        over[0].contains("against a") && over[0].contains("window_tokens"),
        "{}",
        over[0]
    );
    assert_eq!(
        events.iter().filter(|e| e["kind"] == "tool").count(),
        3,
        "the turn still ran"
    );

    // A restart on the same pin does not say it again; a second turn does
    // say over budget again (once).
    let mut k2 = restart_replay(&mut k, "{\"agent\":\"root\",\"content\":\"Still here.\"}\n");
    let mut b = Attach::connect(&k2.url);
    let _ = b.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    std::thread::sleep(Duration::from_millis(500));
    b.send(
        serde_json::json!({"type":"user","agent":"root","text":"still there?","attachments":[]}),
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while transcript(&k2.place)
        .iter()
        .filter(|e| e["kind"] == "turn_complete")
        .count()
        < 2
    {
        assert!(std::time::Instant::now() < deadline, "the second turn ends");
        std::thread::sleep(Duration::from_millis(100));
    }
    let events = transcript(&k2.place);
    assert_eq!(
        events
            .iter()
            .filter(|e| e["kind"] == "notice"
                && e["text"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("config.toml pins")))
            .count(),
        1,
        "the pin is named once, not once per boot"
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e["kind"] == "notice"
                && e["text"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("over budget")))
            .count(),
        2,
        "once per turn, two turns"
    );
    let _ = k2.child.kill();
}

#[test]
fn a_reply_that_reads_as_done_after_a_failed_write_is_nudged() {
    let replies = concat!(
        // A write outside the place: refused. Then "seeded".
        "{\"agent\":\"root\",\"content\":\"seeding\",\"calls\":[{\"name\":\"write\",\"arguments\":{\"path\":\"/definitely/not/here/tests/test_math.py\",\"content\":\"x\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"Seeded the project with tests/test_math.py.\"}\n",
        "{\"agent\":\"root\",\"content\":\"The write failed: the path is outside the project. Nothing was seeded.\"}\n",
    );
    let mut k = start_kernel_replay_with("failed-write-done", replies, "");
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(serde_json::json!({"type":"user","agent":"root","text":"seed the project","attachments":[]}));
    let nudge = a
        .wait(Duration::from_secs(15), |f| {
            f["type"] == "event" && f["event"]["kind"] == "nudge"
        })
        .expect("the false 'seeded' is nudged");
    assert_eq!(nudge["event"]["reason"], "failed write reported as done");
    let text = nudge["event"]["text"].as_str().unwrap();
    assert!(text.starts_with("Your last write call failed ("), "{text}");
    assert!(
        text.contains("never report a failed write as done"),
        "{text}"
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while !transcript(&k.place)
        .iter()
        .any(|e| e["kind"] == "turn_complete")
    {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(100));
    }
    let events = transcript(&k.place);
    let write = events
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "write")
        .unwrap();
    assert!(
        write["error"].is_string(),
        "the failed write carries its error: {write}"
    );
    assert!(
        events.iter().any(|e| e["kind"] == "assistant"
            && e["text"]
                .as_str()
                .is_some_and(|t| t.starts_with("The write failed"))),
        "the owned-up reply follows the nudge"
    );
    assert_eq!(
        events.iter().filter(|e| e["kind"] == "nudge").count(),
        1,
        "once"
    );
    let _ = k.child.kill();
}
