//! Desktop symmetry loop, cycle 58, prompt p6: the person wrote "Spawn one
//! worker with this brief: '…'"; the model called `spawn` with `{}`; the
//! refusal named the arguments, and the model's reply asked the person for
//! `task` or `brief` — the tool's words, not theirs. Now the person's own
//! line this turn stands in as the brief, said in the result; with no
//! line to take, the refusal tells the model the move and not to ask.

mod common;

use common::{Attach, start_kernel_replay};
use std::time::Duration;

const REPLIES: &str = concat!(
    "{\"agent\":\"root\",\"content\":\"spawning\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{}}]}\n",
    "{\"agent\":\"root\",\"content\":\"a worker is on it\"}\n",
    "{\"agent\":\"run-the-tick-loop\",\"content\":\"tick 3 of 3\"}\n",
);

#[test]
fn spawn_with_no_arguments_takes_the_users_line_as_the_brief_and_says_so() {
    let mut k = start_kernel_replay("spawn-empty-args", REPLIES);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    let line = "Spawn one worker with this brief: 'Run this exact bash command and report the last line it printed: for i in 1 2 3; do echo tick $i of 3; done'. Await it and tell me the result.";
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": line}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(60)));
    let root: Vec<serde_json::Value> =
        std::fs::read_to_string(k.place.join(".arbos/agents/root/transcript.jsonl"))
            .unwrap()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
    let spawn = root
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "spawn")
        .unwrap_or_else(|| panic!("{root:#?}"));
    assert!(
        spawn["error"].is_null(),
        "spawn {{}} went through: {spawn:#?}"
    );
    let body = spawn["body"].as_str().unwrap_or("");
    assert!(body.starts_with("spawned "), "{body}");
    assert!(
        body.contains("the user's line this turn was taken as the brief"),
        "{body}"
    );
    let child = spawn["child"].as_str().expect("a child id");
    // The worker's brief is the person's words.
    let inbox = k.place.join(format!(".arbos/agents/{child}"));
    assert!(
        common::wait_for(Duration::from_secs(10), || {
            std::fs::read_to_string(inbox.join("transcript.jsonl"))
                .unwrap_or_default()
                .contains("for i in 1 2 3")
        }),
        "{}",
        std::fs::read_to_string(inbox.join("transcript.jsonl")).unwrap_or_default()
    );
    // And no reply to the person names a tool argument.
    for e in root.iter().filter(|e| e["kind"] == "assistant") {
        let text = e["text"].as_str().unwrap_or("");
        assert!(
            !text.contains("`task`") && !text.contains("`brief`"),
            "a tool's argument names reached the person: {text}"
        );
    }
    let _ = k.child.kill();
}
