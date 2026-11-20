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
    // How the helper's own run was recorded: `done`, not a run that lost its child.
    let runs: Vec<String> = agent
        .store
        .subagents()
        .unwrap_or_default()
        .into_iter()
        .filter(|s| s.chat_id.as_deref() == Some(CHAT))
        .map(|s| s.last_status)
        .collect();

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
    assert_eq!(runs, ["done"], "the helper's run is recorded as finished");
}

/// A pi worker sends `agent_end` and then `agent_settled`, which is what its run ends on. With
/// the over-long line read as a lost child the settle was never read: the job's finished worker
/// counted as crashed and a fresh one was given the same brief.
const STUB_PI: &str = r#"#!/usr/bin/env python3
import json, os, sys

worker = os.environ.get("ARBOS_SESSION_ROLE") == "subagent"

def out(text):
    sys.stdout.write(text + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    try:
        c = json.loads(line)
    except ValueError:
        continue
    t = c.get("type")
    data = {"isStreaming": False} if t == "get_state" else None
    out(json.dumps({"type": "response", "id": c.get("id"), "command": t, "success": True, "data": data}))
    if t != "prompt" or not worker:
        continue
    with open("@PROMPTS@", "a") as f:
        f.write("x")
    out(json.dumps({"type": "agent_start"}))
    out(json.dumps({"type": "message_update",
                    "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": "counted: 21"}}))
    out('{"type":"agent_end","messages":["' + "x" * @SIZE@ + '"]}')
    out(json.dumps({"type": "agent_settled", "aborted": False}))
"#;

#[tokio::test(flavor = "multi_thread")]
async fn a_job_whose_worker_ends_on_an_over_cap_agent_end_is_done_and_not_run_again() {
    let dir = std::env::temp_dir().join(format!("arbos-over-cap-job-{}", ulid()));
    std::fs::create_dir_all(&dir).unwrap();
    let prompts = dir.join("prompts");
    let pi = dir.join("stub-pi.py");
    std::fs::write(
        &pi,
        STUB_PI
            .replace("@PROMPTS@", &prompts.display().to_string())
            .replace("@SIZE@", &(CHILD_LINE_MAX + 1024).to_string()),
    )
    .unwrap();
    std::fs::set_permissions(&pi, std::fs::Permissions::from_mode(0o755)).unwrap();

    let (agent, root) = crate::status::tests::test_agent_with_pi(&pi.display().to_string()).await;
    agent
        .start_pi(CHAT)
        .await
        .expect("the main pi child starts");
    let host = agent.sync.open_room(CHAT).await.unwrap();
    let ask = ulid();
    host.lock()
        .await
        .doc
        .append_user_message(
            &ask,
            "count the lines",
            "dev-mac",
            &UserMessageOpts::default(),
        )
        .unwrap();
    let started = agent
        .start_job(
            CHAT,
            &host,
            &ask,
            "Counting lines",
            "Count the licence lines and report the number.",
            "other",
            vec![],
            true,
            &mut None,
        )
        .await
        .expect("the job starts");
    let job = started["job_id"].as_str().expect("a job id").to_owned();

    let mut state = String::new();
    for _ in 0..1200 {
        state = agent
            .store
            .job(&job)
            .ok()
            .flatten()
            .map(|r| r.state)
            .unwrap_or_default();
        if state == "done" || state == "failed" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // Long enough for a fresh worker to have been prompted, had the first counted as crashed.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let asked = std::fs::read_to_string(&prompts).unwrap_or_default().len();
    let state_after = agent.store.job(&job).unwrap().expect("the job row").state;

    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(state, "done", "the finished job is done");
    assert_eq!(state_after, "done", "and stays done");
    assert_eq!(
        asked, 1,
        "the finished job's brief was given to a worker {asked} times"
    );
}
