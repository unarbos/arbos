//! Cycle 15, F-66: the coordinator called `say to=user` with its kickoff
//! greeting and then wrote the same greeting as its reply, so the chat
//! carried the words twice. `user` is no longer a `say` target: the user
//! reads the reply. The refusal says so and what to do; nothing lands in
//! user.md; a worker is told its words reach the user through its parent.

mod common;

use common::{Attach, start_kernel_replay};
use std::path::Path;
use std::time::Duration;

fn transcript(place: &Path, agent: &str) -> Vec<serde_json::Value> {
    std::fs::read_to_string(
        place
            .join(".arbos/agents")
            .join(agent)
            .join("transcript.jsonl"),
    )
    .unwrap_or_default()
    .lines()
    .filter_map(|l| serde_json::from_str(l).ok())
    .collect()
}

const REPLIES: &str = concat!(
    "{\"agent\":\"root\",\"content\":\"greeting\",\"calls\":[{\"name\":\"say\",\"arguments\":{\"to\":\"user\",\"text\":\"Hey Jacob — the project is ready.\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"Hey Jacob — the project is ready.\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"helper\",\"task\":\"say hello to the user\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"started\"}\n",
    "{\"agent\":\"helper\",\"content\":\"trying\",\"calls\":[{\"name\":\"say\",\"arguments\":{\"to\":\"user\",\"text\":\"Hello from the worker.\"}}]}\n",
    "{\"agent\":\"helper\",\"content\":\"Hello — reported to my parent.\"}\n",
    "{\"agent\":\"root\",\"content\":\"noted\"}\n",
);

#[test]
fn say_to_user_is_refused_with_what_to_do_and_writes_nothing() {
    let mut k = start_kernel_replay("say-user", REPLIES);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "hi"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let root = transcript(&k.place, "root");
    let call = root
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "say")
        .unwrap_or_else(|| panic!("{root:#?}"));
    let err = call["error"].as_str().unwrap_or("");
    assert!(err.contains("`user` is not a target"), "{err}");
    assert!(err.contains("the user reads your reply"), "{err}");
    assert!(err.contains("Nothing was sent"), "{err}");
    assert!(
        !err.contains("parent"),
        "root's refusal does not mention a parent: {err}"
    );
    let user_md = std::fs::read_to_string(k.place.join(".arbos/user.md")).unwrap_or_default();
    assert!(
        !user_md.contains("project is ready"),
        "nothing landed as a notice: {user_md}"
    );
    // The greeting is on the transcript once, as the reply.
    assert_eq!(
        root.iter()
            .filter(|e| e["text"]
                .as_str()
                .is_some_and(|t| t.contains("project is ready")))
            .count(),
        1,
        "{root:#?}"
    );
    // The worker's refusal points at its parent.
    assert!(common::wait_for(Duration::from_secs(30), || {
        transcript(&k.place, "helper")
            .iter()
            .chain(
                std::fs::read_to_string(
                    k.place
                        .join(".arbos/archive/agents/helper/transcript.jsonl"),
                )
                .unwrap_or_default()
                .lines()
                .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                .collect::<Vec<_>>()
                .iter(),
            )
            .any(|e| e["kind"] == "tool" && e["name"] == "say")
    }));
    let helper: Vec<serde_json::Value> = {
        let live = transcript(&k.place, "helper");
        if live.is_empty() {
            std::fs::read_to_string(
                k.place
                    .join(".arbos/archive/agents/helper/transcript.jsonl"),
            )
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
        } else {
            live
        }
    };
    let call = helper
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "say")
        .unwrap();
    let err = call["error"].as_str().unwrap_or("");
    assert!(err.contains("hears from your parent"), "{err}");
    let _ = k.child.kill();
}
