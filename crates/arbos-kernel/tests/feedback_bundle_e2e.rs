//! In-app feedback: the kernel hands over one turn's trajectory and the
//! kernel log for its span in a form a client can send as it is —
//! credentials redacted, tool bodies replaced by their glance, bounded,
//! and the turn the user is looking at rather than everything.

mod common;

use common::{Attach, start_kernel_replay};
use std::time::Duration;

#[test]
fn feedback_hands_over_one_turn_redacted_and_bounded() {
    let replies = concat!(
        // Turn 1: a command whose output holds a key.
        "{\"agent\":\"root\",\"content\":\"checking env\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"echo OPENROUTER_API_KEY=sk-or-v1-0123456789abcdef0123456789abcdef; echo token: ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ012345; seq 1 500\",\"description\":\"Env check\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"The env is set.\"}\n",
        // Turn 2: something else entirely.
        "{\"agent\":\"root\",\"content\":\"Second turn, unrelated.\"}\n",
    );
    let mut k = start_kernel_replay("feedback-bundle", replies);
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(
        serde_json::json!({"type":"user","agent":"root","text":"check the env","attachments":[]}),
    );
    let first_end = a
        .wait(Duration::from_secs(15), |f| {
            f["type"] == "event" && f["event"]["kind"] == "turn_complete"
        })
        .expect("turn 1 ends");
    let first_end_seq = first_end["event"]["seq"].as_u64().unwrap();
    a.send(serde_json::json!({"type":"user","agent":"root","text":"and now something else","attachments":[]}));
    let _ = a
        .wait(Duration::from_secs(15), |f| {
            f["type"] == "event"
                && f["event"]["kind"] == "turn_complete"
                && f["event"]["seq"].as_u64() > Some(first_end_seq)
        })
        .expect("turn 2 ends");

    // The report about turn 1: any seq inside it names it.
    a.send(serde_json::json!({"type":"feedback","agent":"root","seq": first_end_seq - 1, "note":"it printed my key sk-or-v1-0123456789abcdef0123456789abcdef"}));
    let b = a
        .wait(Duration::from_secs(10), |f| f["type"] == "feedback_bundle")
        .expect("the bundle");
    assert_eq!(b["agent"], "root");
    let events = b["events"].as_array().unwrap();
    assert_eq!(events[0]["kind"], "wake");
    assert_eq!(events[1]["text"], "check the env");
    assert_eq!(
        events.last().unwrap()["kind"],
        "turn_complete",
        "{events:?}"
    );
    assert!(b["turn"]["complete"].as_bool().unwrap());
    assert!(
        events.iter().all(
            |e| e["text"] != "and now something else" && e["text"] != "Second turn, unrelated."
        ),
        "only the named turn: {events:?}"
    );
    let text = b.to_string();
    assert!(!text.contains("sk-or-v1-0123"), "{text}");
    assert!(!text.contains("ghp_ABCDEFGHIJ"), "{text}");
    assert!(text.contains("[redacted:"), "{text}");
    assert!(
        b["redacted"]["tokens"].as_u64().unwrap() >= 2,
        "{}",
        b["redacted"]
    );
    let tool = events
        .iter()
        .find(|e| e["kind"] == "tool")
        .expect("the bash line");
    assert!(tool.get("body").is_none(), "bodies are out: {tool}");
    let output = tool["output"].as_str().unwrap();
    assert!(
        output.contains("…") && output.contains("500"),
        "the glance stays: {output}"
    );
    assert!(
        b["note"]
            .as_str()
            .unwrap()
            .starts_with("it printed my key [redacted:"),
        "{}",
        b["note"]
    );
    let kernel = &b["kernel"];
    assert_eq!(kernel["version"], env!("CARGO_PKG_VERSION"));
    assert!(
        kernel["git_sha"].is_string() && kernel["built_at"].is_string() && kernel["os"].is_string()
    );
    assert!(
        kernel["provider"].is_string() && kernel["model"].is_string(),
        "{kernel}"
    );
    let log = b["log"].as_array().unwrap();
    assert!(!log.is_empty(), "the kernel log for the turn's span");
    assert!(log.iter().all(|l| l["ts"].is_number()), "{log:?}");
    assert!(
        log.iter().any(|l| l["event"] == "turn_start"
            || l["event"] == "prompt_size"
            || l["agent"] == "root"),
        "{log:?}"
    );
    assert_eq!(b["truncated"], false);
    assert!(b["bytes"].as_u64().unwrap() < 256 * 1024);

    // No seq: the latest turn.
    a.send(serde_json::json!({"type":"feedback","agent":"root"}));
    let latest = a
        .wait(Duration::from_secs(10), |f| f["type"] == "feedback_bundle")
        .expect("the latest turn");
    assert_eq!(latest["events"][1]["text"], "and now something else");
    // An unknown agent is an error, not silence.
    a.send(serde_json::json!({"type":"feedback","agent":"nobody"}));
    let err = a
        .wait(Duration::from_secs(5), |f| f["type"] == "error")
        .expect("refused");
    assert!(err["detail"].as_str().unwrap().contains("nobody"));
    let _ = k.child.kill();
}
