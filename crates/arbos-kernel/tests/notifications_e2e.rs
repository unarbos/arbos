//! Notifications: a worker finishing or asking while every client was away
//! was silent. A top-level agent's reply, a question or approval, a failed
//! turn, and a notice to the user are recorded in
//! `.arbos/notifications.jsonl` and sent as `notify` frames; a client
//! that attaches later gets the unseen ones replayed; `seen` clears them
//! for every client.

mod common;

use common::{Attach, start_kernel_replay};
use std::time::Duration;

const REPLIES: &str = concat!(
    "{\"agent\":\"root\",\"content\":\"Done: the three files are written and the tests are green.\"}\n",
    "{\"agent\":\"root\",\"content\":\"one question\",\"calls\":[{\"name\":\"ask\",\"arguments\":{\"question\":\"Which branch should the fix go on?\",\"options\":[\"main\",\"release\"]}}]}\n",
    "{\"agent\":\"root\",\"content\":\"Going with release.\"}\n",
);

#[test]
fn replies_and_questions_notify_live_replay_when_missed_and_clear_on_seen() {
    let mut k = start_kernel_replay("notifications", REPLIES);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    // A reply while this client is attached: a live notify.
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "write the files"}));
    let n = a
        .wait(Duration::from_secs(30), |f| {
            f["type"] == "notify" && f["kind"] == "reply"
        })
        .expect("a reply notification");
    assert_eq!(n["agent"], "root");
    assert_eq!(n["id"], 1);
    assert!(n["title"].as_str().unwrap().ends_with("replied"), "{n}");
    assert!(
        n["body"]
            .as_str()
            .unwrap()
            .starts_with("Done: the three files"),
        "{n}"
    );
    assert!(n.get("replayed").is_none(), "live, not replayed: {n}");
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));

    // A question: an `ask` notification with the question as the body.
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "fix it"}));
    let n = a
        .wait(Duration::from_secs(30), |f| {
            f["type"] == "notify" && f["kind"] == "ask"
        })
        .expect("an ask notification");
    assert_eq!(n["id"], 2);
    assert_eq!(n["body"], "Which branch should the fix go on?");
    assert!(n["title"].as_str().unwrap().ends_with("asks"), "{n}");
    drop(a);

    // A client that was away: both replayed after hello, oldest first.
    let mut b = Attach::connect(&k.url);
    let mut replayed = Vec::new();
    let _ = b.wait(Duration::from_secs(10), |f| {
        if f["type"] == "notify" {
            replayed.push(f.clone());
        }
        f["type"] == "notify" && f["id"] == 2
    });
    assert_eq!(replayed.len(), 2, "{replayed:#?}");
    assert!(
        replayed.iter().all(|f| f["replayed"] == true),
        "{replayed:#?}"
    );
    assert_eq!(
        (replayed[0]["kind"].as_str(), replayed[1]["kind"].as_str()),
        (Some("reply"), Some("ask"))
    );
    let on_disk = std::fs::read_to_string(k.place.join(".arbos/notifications.jsonl")).unwrap();
    assert_eq!(on_disk.lines().count(), 2, "{on_disk}");

    // Seen through 2: broadcast, and a third client gets nothing replayed.
    b.send(serde_json::json!({"type": "seen", "through": 2}));
    let s = b
        .wait(Duration::from_secs(5), |f| f["type"] == "seen")
        .expect("seen echoed");
    assert_eq!(s["through"], 2);
    // Answer the question so the turn ends; the answer's reply notifies (id 3).
    b.send(serde_json::json!({"type": "answer", "agent": "root", "text": "release"}));
    let n = b
        .wait(Duration::from_secs(30), |f| {
            f["type"] == "notify" && f["id"] == 3
        })
        .expect("the reply after the answer");
    assert_eq!(n["kind"], "reply");
    assert!(n["body"].as_str().unwrap().contains("release"), "{n}");
    drop(b);
    let mut c = Attach::connect(&k.url);
    let mut replayed = Vec::new();
    let _ = c.wait(Duration::from_secs(3), |f| {
        if f["type"] == "notify" {
            replayed.push(f.clone());
        }
        false
    });
    assert_eq!(replayed.len(), 1, "only the unseen one: {replayed:#?}");
    assert_eq!(replayed[0]["id"], 3);
    // "Clear everything": an id past the newest means the newest.
    c.send(serde_json::json!({"type": "seen", "through": 999}));
    let s = c
        .wait(Duration::from_secs(5), |f| f["type"] == "seen")
        .expect("seen echoed");
    assert_eq!(s["through"], 3, "clamped to the newest id");
    let _ = k.child.kill();
}
