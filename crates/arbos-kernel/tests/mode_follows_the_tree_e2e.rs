//! Desktop cycle 39: root in ask mode, "create hello.txt", and the file
//! appeared with no card. Root is a coordinator; the write was its
//! worker's, and the worker was born in auto. The user's leash on an
//! agent is the leash on its tree: a child takes the parent's mode at
//! spawn, and a mode set on the parent reaches its live children.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::time::Duration;

fn agent_md(place: &std::path::Path, id: &str) -> String {
    std::fs::read_to_string(place.join(".arbos/agents").join(id).join("agent.md"))
        .unwrap_or_default()
}

#[test]
fn a_worker_takes_its_parents_mode_and_follows_a_change_to_it() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"one worker\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"maker\",\"task\":\"create hello.txt containing hi\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"started\"}\n",
        "{\"agent\":\"maker\",\"content\":\"making it\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"echo \\\"hi\\\" > hello.txt\",\"description\":\"Create hello.txt\"}}]}\n",
        "{\"agent\":\"maker\",\"content\":\"waiting a moment\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 4\",\"description\":\"Wait\"}}]}\n",
        "{\"agent\":\"maker\",\"content\":\"made\"}\n",
    );
    let k = start_kernel_replay_prepared("mode-tree", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"coordinator\"\narchive_children = false\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(serde_json::json!({"type":"set_mode","agent":"root","mode":"ask"}));
    assert!(
        common::wait_for(Duration::from_secs(5), || agent_md(&k.place, "root")
            .contains("mode: ask")),
        "root is in ask mode"
    );

    a.send(serde_json::json!({"type":"user","agent":"root","text":"Create a file called hello.txt containing hi","attachments":[]}));
    // The worker's write asks — the card carries the worker's id — and
    // nothing is written until it is allowed.
    let ask = a
        .wait(Duration::from_secs(30), |f| {
            f["type"] == "ask" && f["agent"] == "maker"
        })
        .expect("the worker's bash asks");
    assert!(
        ask["question"].as_str().unwrap().contains("hello.txt"),
        "{ask}"
    );
    assert!(
        agent_md(&k.place, "maker").contains("mode: ask"),
        "{}",
        agent_md(&k.place, "maker")
    );
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        !k.place.join("hello.txt").exists(),
        "not written before the answer"
    );
    let id = ask["id"].as_str().unwrap().to_string();
    a.send(serde_json::json!({"type":"approve","agent":"maker","call_id":id,"allow":true}));
    assert!(
        common::wait_for(Duration::from_secs(10), || k
            .place
            .join("hello.txt")
            .exists()),
        "written once allowed"
    );

    // The worker is on its sleep; the user loosens root to auto. The
    // worker follows, on its file and its transcript.
    let second = a
        .wait(Duration::from_secs(20), |f| {
            f["type"] == "ask" && f["agent"] == "maker"
        })
        .expect("the sleep asks too, in ask mode");
    a.send(serde_json::json!({"type":"set_mode","agent":"root","mode":"auto"}));
    assert!(
        common::wait_for(Duration::from_secs(5), || agent_md(&k.place, "maker")
            .contains("mode: auto")),
        "the worker's mode followed: {}",
        agent_md(&k.place, "maker")
    );
    let transcript =
        std::fs::read_to_string(k.place.join(".arbos/agents/maker/transcript.jsonl")).unwrap();
    assert!(
        transcript.contains("set on root, and on its workers with it"),
        "{transcript}"
    );
    let id2 = second["id"].as_str().unwrap().to_string();
    a.send(serde_json::json!({"type":"approve","agent":"maker","call_id":id2,"allow":true}));
}
