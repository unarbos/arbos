//! The legs of a turn before its first token, as the journal has to show them.
//!
//! Test5, 2026-11-05, 14:17:23Z: a ping on an existing chat took 4,540 ms on the box, and 384 ms
//! of that was between the box reading the command and the session switch starting — 287 ms
//! before the reply row opened and 97 ms after it. Nothing was logged in that window, so a
//! turn-lock wait and a slow row could not be told apart, and the 384 ms stayed unexplained.
//!
//! These are the marks that make that window readable: how long the lock was waited for, how
//! long setup took, how long the row took to open, and how long the session took to be ready.

use arbos_core::doc::{CommandView, UserMessageOpts};
use arbos_core::ids::{now_ms, ulid};
use serde_json::json;

use crate::status::tests::{test_agent, Journal};
use crate::sync::SharedHost;

fn send_cmd(host: &SharedHost, text: &str) -> (String, CommandView) {
    let msg_id = ulid();
    let h = host.try_lock().expect("host free");
    h.doc
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

/// One send, start to the point the model would be called. The turn cannot finish on a test box
/// — there is no pi to answer it — but every leg under test is passed before that, which is the
/// point: a turn that never reaches its model call must still say where its time went.
#[tokio::test]
async fn a_send_reports_its_lock_wait_its_row_and_its_session() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATTURNMARKS000001";
    let host = agent.sync.open_room(chat).await.unwrap();
    let (msg_id, cmd) = send_cmd(&host, "Reply with only the word PONG.");

    let journal = Journal::default();
    {
        let _capture = journal.capture_info();
        let _ = agent.handle_command(chat, cmd).await;
    }
    let text = journal.text();

    for mark in ["turn lock taken", "turn setup done", "turn row open"] {
        assert!(text.contains(mark), "no {mark:?} in the journal:\n{text}");
    }
    for field in ["lock_ms=", "setup_ms=", "row_ms="] {
        assert!(text.contains(field), "no {field:?} in the journal:\n{text}");
    }
    assert!(
        text.contains(&msg_id),
        "the marks name the message they are for:\n{text}"
    );
    // The window under test ends at the session: on a box with no pi it fails, and the failure
    // is itself the mark — an unexplained gap is what this exists to rule out.
    assert!(
        text.contains("turn session ready") || text.contains("session_ms="),
        "nothing said about the session leg:\n{text}"
    );
    let _ = std::fs::remove_dir_all(root);
}
