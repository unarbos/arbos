//! A main turn whose child will not end its run is ended for it.
//!
//! At the deadline the box posts its notice and sends one abort. A child that acknowledges the
//! abort and keeps the run open used to hold the turn, and every later message in the workspace,
//! for as long as it lived.

use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::Ordering;
use std::time::Duration;

use arbos_core::doc::{CommandView, UserMessageOpts};
use arbos_core::ids::{now_ms, ulid};
use serde_json::{json, Value};

use crate::agent::TURN_HUNG_NOTICE;
use crate::status::tests::test_agent_with_pi;
use crate::sync::SharedHost;

const CHAT: &str = "01TESTCHATTURNHUNG00000001";

/// The first child starts a run on its first prompt and never ends it, whatever it is told.
/// Every later child answers.
const STUB_PI: &str = r#"#!/usr/bin/env python3
import json, os, sys, time

here = os.path.dirname(os.path.abspath(__file__))
counter = os.path.join(here, "starts")
try:
    n = int(open(counter).read().strip())
except Exception:
    n = 0
n += 1
with open(counter + ".tmp", "w") as f:
    f.write(str(n))
os.replace(counter + ".tmp", counter)

def out(o):
    sys.stdout.write(json.dumps(o) + "\n")
    sys.stdout.flush()

hung = False
for line in sys.stdin:
    try:
        c = json.loads(line)
    except ValueError:
        continue
    t = c.get("type")
    r = {"type": "response", "id": c.get("id"), "command": t, "success": True}
    if t == "get_state":
        r["data"] = {"isStreaming": hung}
    out(r)
    if t == "get_state":
        # What the real extension writes once it is loaded; the box reads a child as ready by it.
        path = os.environ.get("ARBOS_AUDIT_PATH")
        if path:
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "a") as f:
                f.write(json.dumps({"kind": "extension_loaded", "ts": int(time.time() * 1000)}) + "\n")
    if t != "prompt":
        continue
    out({"type": "agent_start"})
    if n == 1:
        hung = True
        continue
    out({"type": "message_update",
         "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": "a fresh child answered"}})
    out({"type": "agent_settled", "aborted": False})
"#;

async fn send_cmd(host: &SharedHost, text: &str) -> (String, CommandView) {
    let msg_id = ulid();
    host.lock()
        .await
        .doc
        .append_user_message(&msg_id, text, "dev-mac", &UserMessageOpts::default())
        .expect("user message");
    (
        msg_id.clone(),
        CommandView {
            id: ulid(),
            kind: "send".into(),
            payload: json!({ "messageId": msg_id }),
            issued_by: "dev-mac".into(),
            issued_at: now_ms(),
            status: "pending".into(),
        },
    )
}

async fn replies(host: &SharedHost, user_msg: &str) -> String {
    let h = host.lock().await;
    h.doc
        .messages()
        .into_iter()
        .filter(|m| {
            m.role == "assistant"
                && m.rest.get("inReplyTo").and_then(Value::as_str) == Some(user_msg)
                && m.rest.get("hidden").and_then(Value::as_bool) != Some(true)
        })
        .map(|m| m.text())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_whose_child_ignores_the_abort_ends_in_words_and_the_next_message_is_answered() {
    let dir = std::env::temp_dir().join(format!("arbos-turn-hung-{}", ulid()));
    std::fs::create_dir_all(&dir).unwrap();
    let pi_bin = dir.join("stub-pi.py");
    std::fs::write(&pi_bin, STUB_PI).unwrap();
    std::fs::set_permissions(&pi_bin, std::fs::Permissions::from_mode(0o755)).unwrap();

    let (agent, root) = test_agent_with_pi(&pi_bin.display().to_string()).await;
    agent.turn_deadline_ms.store(1_000, Ordering::SeqCst);
    agent.turn_abort_grace_ms.store(1_000, Ordering::SeqCst);
    std::fs::write(
        agent.cfg.state_dir.join(crate::config::PI_BACKOFF_MS_FILE),
        "40",
    )
    .unwrap();
    agent
        .store
        .upsert_chat(CHAT, "hung turn", false, now_ms())
        .unwrap();
    let host = agent.sync.open_room(CHAT).await.unwrap();
    agent.start_pi(CHAT).await.expect("the first child boots");

    let (first, cmd) = send_cmd(&host, "do the thing that never returns").await;
    let ended = tokio::time::timeout(Duration::from_secs(30), agent.handle_command(CHAT, cmd))
        .await
        .is_ok();
    let said = replies(&host, &first).await;

    // The workspace is free again: the next message gets a fresh child and an answer.
    let (second, cmd) = send_cmd(&host, "are you there?").await;
    let answered = ended
        && tokio::time::timeout(Duration::from_secs(30), agent.handle_command(CHAT, cmd))
            .await
            .is_ok();
    let mut answer = String::new();
    for _ in 0..600 {
        answer = replies(&host, &second).await;
        if !answered || answer.contains("a fresh child answered") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&root);
    assert!(ended, "the turn was still held 30 s after a 1 s deadline");
    assert!(
        said.contains(TURN_HUNG_NOTICE),
        "the chat is told the turn was ended: {said:?}"
    );
    assert!(
        answer.contains("a fresh child answered"),
        "the next message is answered by a fresh child: {answer:?}"
    );
}
