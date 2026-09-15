//! qa-035 (the subscribe one): `subscribe kind=timer` with a `cmd` and
//! `deliver_to=user` was refused ("deliver_to = user is for shell") and
//! the model did not retry, so the standing QA loop of kickoff item 9
//! never existed. A command on a schedule is a shell subscription: the
//! kernel reads it as one and says so.

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

#[test]
fn a_timer_with_a_cmd_becomes_a_shell_subscription_instead_of_a_refusal() {
    // The exact call from the kickoff rollout.
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"setting up the loop\",\"calls\":[{\"name\":\"subscribe\",\"arguments\":{\"op\":\"add\",\"kind\":\"timer\",\"cmd\":\"python3 toy-repo/hello.py\",\"every\":\"1h\",\"deliver_to\":\"user\",\"notify\":\"QA loop result: {output}\",\"prompt\":\"record the QA loop result in notes.md\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"the loop stands\"}\n",
    );
    let mut k = start_kernel_replay("timer-cmd", replies);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(
        serde_json::json!({"type": "user", "agent": "root", "text": "set up a standing QA loop"}),
    );
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let root = transcript(&k.place, "root");
    let call = root
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "subscribe")
        .expect("subscribe record");
    assert!(
        call.get("error").is_none(),
        "not refused any more: {call:#?}"
    );
    let body = call["body"].as_str().unwrap_or("");
    assert!(
        body.starts_with("Subscribed #") && body.contains("(shell · every 1h"),
        "{body}"
    );
    assert!(
        body.contains("Read as: kind = timer with a cmd runs as kind = shell"),
        "the reading is said: {body}"
    );
    // The file is a shell subscription with the command and the user
    // delivery (the kernel's own gc chore is the other file there).
    let dir = k.place.join(".arbos/agents/root/subscriptions");
    let files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .filter(|p| {
            !std::fs::read_to_string(p)
                .unwrap_or_default()
                .contains("internal = true")
        })
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    let text = std::fs::read_to_string(&files[0]).unwrap();
    assert!(text.contains("kind = \"shell\""), "{text}");
    assert!(
        text.contains("cmd = \"python3 toy-repo/hello.py\""),
        "{text}"
    );
    assert!(text.contains("deliver_to = \"user\""), "{text}");
    assert!(
        text.contains("notify = \"QA loop result: {output}\""),
        "{text}"
    );
    assert!(text.contains("every = \"1h\""), "{text}");
    let _ = k.child.kill();
}
