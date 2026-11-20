//! A helper whose run ends on an `agent_end` over the line cap has still finished.
//!
//! `agent_end` carries the run's whole message list, so a run with many screenshots ends on a
//! line of tens of MiB. agentd reads only `willRetry` from it. Treating the over-long line as a
//! lost child failed the finished run, and its brief was then given out again.

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use arbos_core::doc::{CommandView, UserMessageOpts};
use arbos_core::ids::{now_ms, ulid};
use serde_json::{json, Value};

use crate::rpc_child::CHILD_LINE_MAX;
use crate::status::tests::test_agent_with_prime;

const CHAT: &str = "01TESTCHATOVERCAPAGENTEND1";

/// A Prime worker that answers its prompt and ends the run on an `agent_end` just over the cap,
/// then stays up as a worker does. Each prompt it is given is counted.
const STUB_PRIME: &str = r#"#!/usr/bin/env python3
import json, os, sys

def out(text):
    sys.stdout.write(text + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    try:
        c = json.loads(line)
    except ValueError:
        continue
    t = c.get("type")
    out(json.dumps({"type": "response", "id": c.get("id"), "command": t, "success": True, "data": {}}))
    if t != "prompt":
        continue
    with open("@PROMPTS@", "a") as f:
        f.write("x")
    out(json.dumps({"type": "agent_start"}))
    out(json.dumps({"type": "message_update",
                    "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": "Paris."}}))
    out('{"type":"agent_end","messages":["' + "x" * @SIZE@ + '"]}')
"#;

#[tokio::test(flavor = "multi_thread")]
async fn a_helper_that_ends_on_an_over_cap_agent_end_answers_once() {
    let dir = std::env::temp_dir().join(format!("arbos-over-cap-end-{}", ulid()));
    std::fs::create_dir_all(&dir).unwrap();
    let prompts = dir.join("prompts");
    let prime = dir.join("stub-prime.py");
    std::fs::write(
        &prime,
        STUB_PRIME
            .replace("@PROMPTS@", &prompts.display().to_string())
            .replace("@SIZE@", &(CHILD_LINE_MAX + 1024).to_string()),
    )
    .unwrap();
    std::fs::set_permissions(&prime, std::fs::Permissions::from_mode(0o755)).unwrap();

    let (agent, root) = test_agent_with_prime(&prime.display().to_string()).await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    let _turn = agent.test_hold_turn(CHAT).await;

    // A message beside a busy turn is answered by a Prime helper.
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
    assert!(agent
        .start_job_for_send(CHAT, &host, &cmd, &mut None)
        .await
        .expect("the send path answers"));

    let replies = || async {
        host.lock()
            .await
            .doc
            .messages()
            .into_iter()
            .filter(|m| {
                m.role == "assistant"
                    && m.rest.get("inReplyTo").and_then(Value::as_str) == Some(ask.as_str())
                    && m.rest.get("hidden").and_then(Value::as_bool) != Some(true)
            })
            .collect::<Vec<_>>()
    };
    let mut settled = Vec::new();
    for _ in 0..1200 {
        settled = replies().await;
        if settled.iter().any(|m| m.status != "streaming") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // Long enough for a second worker to have been prompted, had the run been failed.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let asked = std::fs::read_to_string(&prompts).unwrap_or_default().len();
    let rows: Vec<String> = replies()
        .await
        .iter()
        .map(|m| format!("{} {:?} {:?}", m.status, m.text(), m.parts))
        .collect();
    let failed = settled.iter().any(|m| {
        m.status == "error"
            || m.parts
                .iter()
                .any(|p| p.get("kind").and_then(Value::as_str) == Some("error"))
    });
    let answered = settled.iter().any(|m| !m.text().trim().is_empty());

    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        answered && !failed,
        "the finished run is answered, not failed: {rows:#?}"
    );
    assert_eq!(
        asked, 1,
        "the finished run's brief was given out {asked} times"
    );
}
