//! qa-005 and qa-006: every frame that names an agent must name one the
//! kernel lists. An `answer` for an unknown id used to mint a folder with a
//! transcript and no agent.md; a `user` prompt to a folder whose agent.md
//! does not parse was stored as a node that could never fire.

mod common;

use common::{Attach, start_kernel};
use std::time::Duration;

#[test]
fn frames_for_ids_the_kernel_does_not_list_write_nothing() {
    let mut k = start_kernel("unknown");
    let agents = k.place.join(".arbos").join("agents");
    // A folder the kernel skips: agent.md is not UTF-8, so Agent::load fails.
    std::fs::create_dir_all(agents.join("garbage")).unwrap();
    std::fs::write(
        agents.join("garbage").join("agent.md"),
        b"\xff\xfe not an agent",
    )
    .unwrap();

    let mut a = Attach::connect(&k.url);
    let snap = a
        .wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
        .unwrap();
    let listed: Vec<String> = snap["tree"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect();
    assert!(!listed.iter().any(|id| id == "garbage"), "{listed:?}");

    a.send(serde_json::json!({"type": "answer", "agent": "nobody", "text": "42"}));
    a.send(
        serde_json::json!({"type": "approve", "agent": "nobody-2", "call_id": "c1", "allow": true}),
    );
    a.send(serde_json::json!({"type": "user", "agent": "garbage", "text": "hello garbage"}));
    a.send(serde_json::json!({"type": "user", "agent": "does-not-exist", "text": "hi"}));
    assert!(
        a.wait(Duration::from_secs(3), |f| f["type"] == "turn")
            .is_none(),
        "no turn may start for an unlisted id"
    );

    assert!(
        !agents.join("nobody").exists(),
        "answer for an unknown id must not mint a folder"
    );
    assert!(
        !agents.join("nobody-2").exists(),
        "approve for an unknown id must not mint a folder"
    );
    assert!(!agents.join("does-not-exist").exists());
    assert!(
        !agents.join("garbage").join("plan.jsonl").exists(),
        "a prompt to an unlisted folder must not be stored as a node"
    );

    // The listed agent still works.
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "hello"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let _ = k.child.kill();
}
