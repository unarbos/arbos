//! qa-008: a `user` frame with blank text and no attachments is not a
//! message. It used to become a plan node and start a model turn.

mod common;

use common::{Attach, start_kernel_replay};
use std::time::Duration;

#[test]
fn a_blank_prompt_starts_no_turn_and_writes_no_node() {
    let mut k = start_kernel_replay("empty", "");
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );

    for text in ["", "   ", "\n\t"] {
        a.send(serde_json::json!({"type": "user", "agent": "root", "text": text}));
    }
    assert!(
        a.wait(Duration::from_secs(3), |f| f["type"] == "turn"
            && f["agent"] == "root")
            .is_none(),
        "a blank prompt must not start a turn"
    );
    let plan = k
        .place
        .join(".arbos")
        .join("agents")
        .join("root")
        .join("plan.jsonl");
    assert!(
        !plan.exists() || std::fs::read_to_string(&plan).unwrap().trim().is_empty(),
        "a blank prompt must not be stored as a node"
    );

    // A real prompt still works afterwards.
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "hello"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let _ = k.child.kill();
}
