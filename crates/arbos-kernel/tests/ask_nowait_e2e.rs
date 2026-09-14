//! T3-07: `ask wait:false` does not park the turn. The question is shown,
//! the agent keeps working, and the answer lands at the next tool
//! boundary of the same turn — or opens the next turn if this one ended.

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

#[test]
fn a_non_blocking_ask_keeps_the_turn_going_and_reads_the_answer_at_the_next_boundary() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"asking while I work\",\"calls\":[{\"name\":\"ask\",\"arguments\":{\"question\":\"Teal or red?\",\"options\":[\"teal\",\"red\"],\"wait\":false}}]}\n",
        "{\"agent\":\"root\",\"content\":\"meanwhile\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 3; echo worked\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"done, with the answer in hand\"}\n",
    );
    let mut k = start_kernel_replay("ask-nowait", replies);
    // Root needs bash: an old-style place.
    std::fs::write(
        k.place.join(".arbos/project.toml"),
        "schema = 2\nname = \"a\"\n",
    )
    .unwrap();
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "pick a colour and keep going"}));
    let ask = a
        .wait(Duration::from_secs(20), |f| {
            f["type"] == "ask" && f["agent"] == "root"
        })
        .expect("the question reaches the window");
    assert_eq!(ask["question"], "Teal or red?");
    let id = ask["id"].as_str().unwrap_or("").to_string();
    // The turn is still running (the bash step sleeps): answer now.
    std::thread::sleep(Duration::from_millis(800));
    a.send(serde_json::json!({"type": "answer", "agent": "root", "text": "teal", "id": id}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(40)));
    std::thread::sleep(Duration::from_millis(500));

    let t = transcript(&k.place);
    let kinds: Vec<&str> = t.iter().filter_map(|e| e["kind"].as_str()).collect();
    // One turn, not two: the ask did not park.
    assert_eq!(
        kinds.iter().filter(|k| **k == "turn_complete").count(),
        1,
        "{kinds:?}"
    );
    let ask_tool = t
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "ask")
        .expect("ask tool line");
    assert!(
        ask_tool["body"]
            .as_str()
            .unwrap_or("")
            .contains("keep working"),
        "{ask_tool:#?}"
    );
    assert!(ask_tool.get("park").is_none(), "not parked: {ask_tool:#?}");
    // The answer is on the transcript before the turn ended, and the bash
    // step still ran after it was asked.
    let answer_ix = t
        .iter()
        .position(|e| e["kind"] == "answer" && e["text"] == "teal")
        .expect("answer line");
    let end_ix = t.iter().position(|e| e["kind"] == "turn_complete").unwrap();
    let bash_ix = t
        .iter()
        .position(|e| e["kind"] == "tool" && e["name"] == "bash")
        .unwrap();
    assert!(answer_ix < end_ix, "{kinds:?}");
    assert!(bash_ix < end_ix, "{kinds:?}");
    assert!(
        t.iter()
            .any(|e| e["kind"] == "assistant" && e["text"] == "done, with the answer in hand")
    );
    // No second turn opened for the answer, and no answer file is left.
    let inbox: Vec<_> = std::fs::read_dir(k.place.join(".arbos/agents/root/inbox"))
        .into_iter()
        .flatten()
        .flatten()
        .collect();
    assert!(
        inbox.is_empty(),
        "the answer was consumed mid-turn: {inbox:?}"
    );
    let waiting = k.place.join(".arbos/agents/root/waiting");
    let asks: Vec<_> = std::fs::read_dir(&waiting)
        .into_iter()
        .flatten()
        .flatten()
        .collect();
    assert!(asks.is_empty(), "the question is resolved: {asks:?}");
    let _ = k.child.kill();
}
