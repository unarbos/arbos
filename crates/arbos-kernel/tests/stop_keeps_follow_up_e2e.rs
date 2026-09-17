//! F-105: a follow-up the user queued behind a running turn was deleted
//! when they pressed Stop — his words accepted and then lost, no file, no
//! line, no log. Stop now ends the turn and keeps the queue: the follow-up
//! is held (wake off, the row under the composer), survives a kernel
//! restart, is not folded into another turn, and runs when the user
//! presses Send now — or goes when they press Remove.

mod common;

use common::{Attach, restart_replay, start_kernel_replay};
use std::time::{Duration, Instant};

fn transcript(place: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn inbox_files(place: &std::path::Path) -> Vec<(String, String)> {
    let dir = place.join(".arbos/agents/root/inbox");
    let mut v: Vec<(String, String)> = std::fs::read_dir(&dir)
        .map(|d| {
            d.flatten()
                .map(|e| {
                    (
                        e.file_name().to_string_lossy().to_string(),
                        std::fs::read_to_string(e.path()).unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

#[test]
fn stop_keeps_the_users_queued_follow_up_held_until_send_now_or_remove() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"working\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 30; echo slow\",\"description\":\"Slow step\"}}]}\n",
        // After Send now: the follow-up's own turn.
        "{\"agent\":\"root\",\"content\":\"Here is the follow-up, answered.\"}\n",
        // After Send now on the second one, after a restart.
        "{\"agent\":\"root\",\"content\":\"And the second, answered.\"}\n",
    );
    let mut k = start_kernel_replay("stop-keeps-follow-up", replies);
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(serde_json::json!({"type":"user","agent":"root","text":"do the slow thing","attachments":[]}));
    assert!(a.wait_turn("root", "running", Duration::from_secs(10)));
    // The turn is *in its bash* before anything else happens: a Stop that
    // lands while the first model call is still in flight leaves the
    // scripted reply unconsumed, and the next turn would take it (the
    // 30 s sleep) instead of its own — the 1-in-3 flake. The fact to wait
    // on is the tool starting, which the kernel emits as a `tool` event
    // with no `ended`.
    assert!(
        a.wait(Duration::from_secs(10), |f| {
            f["type"] == "event"
                && f["agent"] == "root"
                && f["event"]["kind"] == "tool"
                && f["event"]["name"] == "bash"
                && f["event"]["ended"].is_null()
        })
        .is_some(),
        "the bash has started"
    );
    // Two follow-ups queued behind the turn (a plain user frame while a
    // turn runs is held for the next turn).
    a.send(serde_json::json!({"type":"user","agent":"root","text":"then do this next","attachments":[]}));
    a.send(
        serde_json::json!({"type":"user","agent":"root","text":"and this after","attachments":[]}),
    );
    let queued = a
        .wait(Duration::from_secs(10), |f| {
            f["type"] == "plan"
                && f["nodes"].as_array().is_some_and(|n| {
                    n.iter()
                        .filter(|n| n["inbox"] == true && n["when"] == "ready")
                        .count()
                        == 2
                })
        })
        .expect("both follow-ups queued as ready rows");
    let _ = queued;
    assert_eq!(inbox_files(&k.place).len(), 2);

    // Stop. The turn ends; the follow-ups stay, held.
    a.send(serde_json::json!({"type":"stop","agent":"root"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(10)));
    // The plan a fresh client is handed: both rows held, none ready.
    let mut p = Attach::connect(&k.url);
    let held = p
        .wait(Duration::from_secs(10), |f| {
            f["type"] == "plan"
                && f["agent"] == "root"
                && f["nodes"]
                    .as_array()
                    .is_some_and(|n| n.iter().any(|n| n["inbox"] == true))
        })
        .expect("the plan with the held rows");
    let inbox_rows: Vec<&serde_json::Value> = held["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["inbox"] == true)
        .collect();
    assert_eq!(inbox_rows.len(), 2, "{inbox_rows:?}");
    assert!(
        inbox_rows.iter().all(|n| n["when"] == "waits"),
        "held, not ready: {inbox_rows:?}"
    );
    drop(p);
    let rows: Vec<&serde_json::Value> = held["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["inbox"] == true)
        .collect();
    assert!(
        rows.iter().any(|r| r["goal"] == "then do this next")
            && rows.iter().any(|r| r["goal"] == "and this after"),
        "{rows:?}"
    );
    let files = inbox_files(&k.place);
    assert_eq!(files.len(), 2, "kept on disk: {files:?}");
    assert!(
        files.iter().all(|(_, text)| text.contains("wake = false")),
        "{files:?}"
    );
    let log = std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.log")).unwrap();
    assert_eq!(
        log.matches("\"event\":\"follow_up_held\"").count(),
        2,
        "{log}"
    );
    // Nothing ran on its own: no user line for either, one turn ended.
    std::thread::sleep(Duration::from_millis(1500));
    let events = transcript(&k.place);
    assert!(
        events
            .iter()
            .all(|e| e["text"] != "then do this next" && e["text"] != "and this after"),
        "{events:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e["kind"] == "turn_complete")
            .count(),
        1
    );

    // Send now on the first: its own turn, the words as the prompt, not
    // a note folded into something else.
    let first_id = rows
        .iter()
        .find(|r| r["goal"] == "then do this next")
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    a.send(
        serde_json::json!({"type":"plan_op","agent":"root","node": first_id,"op":"run","text":""}),
    );
    let reply = a
        .wait(Duration::from_secs(20), |f| {
            f["type"] == "event"
                && f["event"]["kind"] == "assistant"
                && f["event"]["text"] == "Here is the follow-up, answered."
        })
        .expect("Send now runs it");
    let _ = reply;
    let events = transcript(&k.place);
    let user_at = events
        .iter()
        .position(|e| e["kind"] == "user" && e["text"] == "then do this next")
        .expect("the words are the prompt");
    assert!(
        events[user_at - 1]["kind"] == "wake" && events[user_at - 1]["wake"] == "user",
        "its own turn: {events:?}"
    );
    assert!(
        events.iter().all(|e| e["kind"] != "say"),
        "not a note: {events:?}"
    );
    assert_eq!(inbox_files(&k.place).len(), 1, "the other still waits");

    // A relaunch: the held row survives the kernel, still held.
    let mut k2 = restart_replay(
        &mut k,
        "{\"agent\":\"root\",\"content\":\"And the second, answered.\"}\n",
    );
    let mut b = Attach::connect(&k2.url);
    let plan = b
        .wait(Duration::from_secs(10), |f| {
            f["type"] == "plan"
                && f["agent"] == "root"
                && f["nodes"]
                    .as_array()
                    .is_some_and(|n| n.iter().any(|n| n["inbox"] == true))
        })
        .expect("the held row is offered again after the restart");
    let row = plan["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["inbox"] == true)
        .unwrap();
    assert_eq!(row["goal"], "and this after");
    assert_eq!(row["when"], "waits");
    std::thread::sleep(Duration::from_millis(1500));
    assert!(
        transcript(&k2.place)
            .iter()
            .all(|e| e["text"] != "and this after"),
        "still not run on its own"
    );
    // Remove: gone, and the words never ran.
    let id = row["id"].as_u64().unwrap();
    b.send(serde_json::json!({"type":"plan_op","agent":"root","node": id,"op":"cancel","text":""}));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !inbox_files(&k2.place).is_empty() {
        assert!(Instant::now() < deadline, "removed on Remove");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        transcript(&k2.place)
            .iter()
            .all(|e| e["text"] != "and this after")
    );
    let _ = k2.child.kill();
}
