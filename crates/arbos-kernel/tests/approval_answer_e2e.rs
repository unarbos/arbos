//! Jacob's Mac, 2026-09-15: he clicked "allow" on an approval card and the
//! kernel said "answer refused: no question is pending for root". An
//! approval goes out as an `ask` frame with allow/deny options, and the
//! desktop answered it with an `answer` frame; the kernel looked only at
//! parked questions. An answer to a card the window shows is never
//! refused: when it names the pending approval, or nothing else is
//! pending, it is the verdict.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::path::Path;
use std::time::Duration;

fn tools(place: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|e| e["kind"] == "tool" || e["kind"] == "approval")
        .collect()
}

fn start(name: &str) -> common::Kernel {
    // Ask mode, so the write asks; the answer comes back as a question's.
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"writing\",\"calls\":[{\"name\":\"write\",\"arguments\":{\"path\":\"note.txt\",\"content\":\"hi\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"done\"}\n",
    );
    start_kernel_replay_prepared(name, replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\nname = \"a\"\n\n[root]\npermission = \"ask\"\n",
        )
        .unwrap();
    })
}

#[test]
fn an_answer_of_allow_to_the_approval_card_is_the_verdict() {
    let mut k = start("approval-answer");
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "write a note"}));
    let ask = a
        .wait(Duration::from_secs(30), |f| {
            f["type"] == "ask" && f["agent"] == "root"
        })
        .expect("the approval card");
    assert_eq!(ask["options"], serde_json::json!(["allow", "deny"]));
    let id = ask["id"].as_str().unwrap_or("").to_string();
    // The desktop's shape: an answer, with the card's id and its text.
    a.send(serde_json::json!({"type": "answer", "agent": "root", "id": id, "text": "allow"}));
    let mut refused = None;
    let idle = a.wait(Duration::from_secs(30), |f| {
        if f["type"] == "error" && f["agent"] == "root" {
            refused = Some(f["detail"].as_str().unwrap_or("").to_string());
        }
        f["type"] == "turn" && f["agent"] == "root" && f["state"] == "idle"
    });
    assert!(idle.is_some(), "the turn ends");
    assert!(refused.is_none(), "the answer was not refused: {refused:?}");
    let t = tools(&k.place);
    let approval = t
        .iter()
        .find(|e| e["kind"] == "approval")
        .expect("the decision is on the record");
    assert_eq!(approval["allowed"], true);
    assert_eq!(approval["tool"], "write");
    let write = t.iter().find(|e| e["name"] == "write").expect("the write");
    assert!(write.get("error").is_none(), "{write:#?}");
    assert_eq!(
        std::fs::read_to_string(k.place.join("note.txt")).unwrap(),
        "hi"
    );
    let _ = k.child.kill();
}

#[test]
fn an_answer_of_deny_without_an_id_is_the_verdict_when_only_the_approval_is_pending() {
    let mut k = start("approval-answer-blind");
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "write a note"}));
    assert!(
        a.wait(Duration::from_secs(30), |f| f["type"] == "ask"
            && f["agent"] == "root")
            .is_some()
    );
    a.send(serde_json::json!({"type": "answer", "agent": "root", "text": "deny"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let t = tools(&k.place);
    let approval = t
        .iter()
        .find(|e| e["kind"] == "approval")
        .expect("the decision");
    assert_eq!(approval["allowed"], false);
    let write = t.iter().find(|e| e["name"] == "write").expect("the write");
    assert!(
        write["error"]
            .as_str()
            .unwrap_or("")
            .contains("did not allow"),
        "{write:#?}"
    );
    assert!(!k.place.join("note.txt").exists());
    let _ = k.child.kill();
}
