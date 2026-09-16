//! First install, no model key (qa): the user's first line must never be
//! held in silence. A `kickoff` on a keyless kernel is refused at once
//! with an `error` that says what is missing and a `turn idle` that
//! releases anything a client queued behind it; a `user` line is kept in
//! the inbox (the pending row), told once, and runs the moment
//! `configure` lands a key.

mod common;

use common::{Attach, start_kernel};
use std::time::Duration;

fn transcript(place: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[test]
fn a_keyless_kernel_refuses_the_kickoff_and_keeps_the_first_line_until_a_key_lands() {
    let mut k = start_kernel("keyless-first-line");
    let mut a = Attach::connect(&k.url);
    let provider = a
        .wait(Duration::from_secs(5), |f| f["type"] == "provider")
        .expect("provider frame");
    assert_eq!(provider["key"], false, "{provider}");

    // The kickoff: refused, and the turn boundary a client waits on.
    a.send(serde_json::json!({"type":"kickoff","agent":"root"}));
    let err = a
        .wait(Duration::from_secs(5), |f| {
            f["type"] == "error" && f["agent"] == "root"
        })
        .expect("kickoff refusal");
    let detail = err["detail"].as_str().unwrap();
    assert!(
        detail.starts_with("kickoff not started: No API key"),
        "{detail}"
    );
    assert!(
        detail.contains("kept and runs once a key is in place"),
        "{detail}"
    );
    assert!(
        a.wait_turn("root", "idle", Duration::from_secs(5)),
        "turn idle releases what a client holds behind the kickoff"
    );
    assert!(
        transcript(&k.place).iter().all(|e| e["kind"] != "wake"),
        "no kickoff turn was spent"
    );

    // The first line: kept, not burned on a failed turn; told once.
    a.send(serde_json::json!({"type":"user","agent":"root","text":"hello there","attachments":[]}));
    let err = a
        .wait(Duration::from_secs(5), |f| {
            f["type"] == "error"
                && f["detail"]
                    .as_str()
                    .is_some_and(|d| d.contains("hello there"))
        })
        .expect("held notice");
    assert!(
        err["detail"]
            .as_str()
            .unwrap()
            .contains("Your message is kept"),
        "{err}"
    );
    let plan = a
        .wait(Duration::from_secs(5), |f| {
            f["type"] == "plan"
                && f["nodes"].as_array().is_some_and(|n| {
                    n.iter()
                        .any(|n| n["inbox"] == true && n["goal"] == "hello there")
                })
        })
        .expect("the pending row shows the held line");
    assert_eq!(plan["nodes"][0]["origin"], "user");
    let events = transcript(&k.place);
    assert!(
        events.iter().any(|e| e["kind"] == "notice"
            && e["failed"] == true
            && e["text"].as_str().unwrap().contains("hello there")),
        "the transcript says the line waits: {events:?}"
    );
    assert!(
        events
            .iter()
            .all(|e| e["kind"] != "user" && e["kind"] != "turn_complete"),
        "{events:?}"
    );
    let inbox = k.place.join(".arbos/agents/root/inbox");
    assert_eq!(
        std::fs::read_dir(&inbox).unwrap().count(),
        1,
        "the line is an inbox file"
    );

    // A second line is kept too, told nothing new (one notice per agent).
    a.send(serde_json::json!({"type":"user","agent":"root","text":"and this","attachments":[]}));
    assert!(
        a.wait(Duration::from_secs(2), |f| f["type"] == "error"
            && f["detail"].as_str().is_some_and(|d| d.contains("and this")))
            .is_none(),
        "told once"
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::fs::read_dir(&inbox).unwrap().count() < 2 {
        assert!(std::time::Instant::now() < deadline, "second line filed");
        std::thread::sleep(Duration::from_millis(50));
    }

    // A key lands (against a base that refuses connections, so the turn
    // starts and fails fast): the held lines run, in order, at once.
    a.send(serde_json::json!({
        "type":"configure","provider":"openrouter","api_base":"http://127.0.0.1:9/v1",
        "model":"test/model","api_key":"sk-test-not-real","remember":false
    }));
    let provider = a
        .wait(Duration::from_secs(5), |f| {
            f["type"] == "provider" && f["key"] == true
        })
        .expect("provider frame with a key");
    assert_eq!(provider["source"], "memory");
    assert!(
        a.wait_turn("root", "running", Duration::from_secs(10)),
        "the held line starts a turn"
    );
    let user = a
        .wait(Duration::from_secs(10), |f| {
            f["type"] == "event" && f["event"]["kind"] == "user"
        })
        .expect("the user's words land on the transcript");
    assert_eq!(user["event"]["text"], "hello there");
    assert!(
        a.wait(Duration::from_secs(30), |f| f["type"] == "event"
            && f["event"]["kind"] == "user"
            && f["event"]["text"] == "and this")
            .is_some(),
        "the second line follows"
    );
    let _ = k.child.kill();
}
