//! The `plan` tool's node rules, as the model sends them (`NewNode::from_json`
//! then `plan_add`). qa-009: a one-shot `after` next to a goal that says
//! "every hour" was accepted; it fired once while the agent told the user it
//! recurred. And `after: "0s"` beside `every: "1h"` was refused as two
//! triggers, so the model needed eight tries to schedule anything.

use arbos_core::{Place, bootstrap};
use arbos_kernel::hooks::{KernelHooks, NewNode};
use std::sync::Arc;

fn hooks(name: &str) -> Arc<KernelHooks> {
    let dir = std::env::temp_dir().join(format!(
        "arbos-plan-tool-{name}-{}-{}",
        std::process::id(),
        arbos_core::now_ms()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let place = Place::new(&dir);
    bootstrap(&place).unwrap();
    let (wake_tx, _wake_rx) = tokio::sync::mpsc::unbounded_channel();
    let (kick_tx, _kick_rx) = tokio::sync::mpsc::unbounded_channel();
    KernelHooks::new(place, wake_tx, kick_tx)
}

fn add(h: &KernelHooks, spec: serde_json::Value) -> anyhow::Result<Vec<u64>> {
    let node = NewNode::from_json(&spec)?;
    h.plan_add("root", 0, &[node], "user")
}

#[test]
fn a_recurring_goal_with_only_a_one_shot_after_is_refused() {
    let h = hooks("wording");
    let err = add(
        &h,
        serde_json::json!({
            "goal": "Append UTC date and time to notes.md every hour",
            "when": {"after": "1m", "every": "", "wake": true},
            "do": {"shell": "date -u >> notes.md"}
        }),
    )
    .expect_err("one-shot after with 'every hour' in the goal must be refused");
    let msg = format!("{err:#}");
    assert!(msg.contains("when.every is not set"), "{msg}");
    assert!(
        msg.contains("fires it once"),
        "the message says what would happen: {msg}"
    );
    assert!(h.plan_nodes("root").is_empty(), "nothing stored");
}

#[test]
fn a_zero_after_beside_every_is_no_second_trigger() {
    let h = hooks("zero");
    for zero in ["0s", "0", "0m", "00"] {
        let ids = add(
            &h,
            serde_json::json!({
                "goal": "Append UTC date to notes.md",
                "when": {"after": zero, "every": "1h", "wake": true},
                "do": {"shell": "date -u >> notes.md"}
            }),
        )
        .unwrap_or_else(|e| panic!("after={zero:?} must read as unset: {e:#}"));
        let node = h
            .plan_nodes("root")
            .into_iter()
            .find(|n| n.id == ids[0])
            .unwrap();
        assert_eq!(node.when.every_ms, Some(3_600_000));
        assert!(node.when.after_ms.is_none());
    }
}

#[test]
fn a_real_delay_and_a_plain_recurrence_still_work() {
    let h = hooks("plain");
    add(
        &h,
        serde_json::json!({"goal": "Remind me about the deploy", "when": {"after": "30m"}}),
    )
    .expect("a deferred one-shot without recurring wording is fine");
    add(
        &h,
        serde_json::json!({"goal": "Check the build", "when": {"every": "1h"}, "do": {"shell": "true"}}),
    )
    .expect("a recurring node is fine");
    assert!(
        add(
            &h,
            serde_json::json!({"goal": "Check the build every hour"})
        )
        .is_err(),
        "recurring wording with no trigger at all is still refused"
    );
}
