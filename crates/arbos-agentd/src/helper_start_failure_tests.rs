//! A helper that cannot be started closes its own progress row.
//!
//! A message that arrives beside a busy turn is answered by a helper, and the helper's hidden
//! progress row is opened before the helper starts. When the start fails the user reads a
//! failure line, and the row used to stay `streaming` with "Working…" until the orphan sweep
//! found it fifteen minutes later. The soak check found it; this is the same case in CI.

use std::time::Duration;

use arbos_core::doc::{CommandView, UserMessageOpts};
use arbos_core::ids::{now_ms, ulid};
use serde_json::{json, Value};

use crate::status::tests::test_agent;

const CHAT: &str = "01TESTCHATHELPERSTARTFAIL1";

#[tokio::test(flavor = "multi_thread")]
async fn a_helper_that_cannot_start_leaves_no_row_streaming() {
    // The test box's Prime binary does not exist, so the helper's spawn fails: what a box that
    // is out of pids does to it.
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    let _turn = agent.test_hold_turn(CHAT).await;

    let ask = ulid();
    host.lock()
        .await
        .doc
        .append_user_message(
            &ask,
            "what is the capital of France?",
            "dev-mac",
            &UserMessageOpts::default(),
        )
        .unwrap();
    let cmd = CommandView {
        id: ulid(),
        kind: "send".into(),
        payload: json!({ "messageId": ask }),
        issued_by: "dev-mac".into(),
        issued_at: now_ms(),
        status: "pending".into(),
    };
    let accepted = agent
        .start_job_for_send(CHAT, &host, &cmd, &mut None)
        .await
        .expect("the send path answers");
    assert!(accepted, "the ask is taken beside the running turn");

    let replies = || async {
        host.lock()
            .await
            .doc
            .messages()
            .into_iter()
            .filter(|m| {
                m.role == "assistant"
                    && m.rest.get("inReplyTo").and_then(Value::as_str) == Some(ask.as_str())
            })
            .collect::<Vec<_>>()
    };
    let mut told = false;
    for _ in 0..400 {
        told = replies().await.iter().any(|m| {
            m.parts
                .iter()
                .any(|p| p.get("code").and_then(Value::as_str) == Some("job_failed"))
        });
        if told {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let streaming: Vec<String> = replies()
        .await
        .into_iter()
        .filter(|m| m.status == "streaming")
        .map(|m| m.id)
        .collect();
    let _ = std::fs::remove_dir_all(&root);
    assert!(told, "the user is told the helper could not answer");
    assert!(
        streaming.is_empty(),
        "the failed helper's rows are still streaming: {streaming:?}"
    );
}
