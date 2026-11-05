//! The cold-chat legs: a room told to open, and the work waiting behind it.
//!
//! Test5, 2026-11-05, 14:16:53Z: a ping in a new chat took 3,809 ms end to end, and 1,269 ms of
//! it was gone before the box had a prompt to send — 576 ms from the relay's push to "room
//! opened" and 693 ms more to "room caught up". Both lines existed; neither carried a duration,
//! so the legs could only be reconstructed by subtracting one journal timestamp from another,
//! and only when both lines happened to be present.
//!
//! The second half is what the push says. The relay sends how far the room has got; the box
//! holds its own cursor, so the two together say rows are already waiting. That lets the box
//! start the slow part of a turn — pi's session switch — while the room is still connecting,
//! instead of discovering the work afterwards.

use serde_json::json;

use crate::status::tests::{test_agent, Journal};

const CHAT: &str = "01TESTCHATROOMMARKS000001";

#[tokio::test]
async fn a_room_knows_how_long_ago_it_opened() {
    let (agent, root) = test_agent().await;
    assert_eq!(
        agent.sync.open_ms(CHAT).await,
        None,
        "a room that was never opened has no clock"
    );
    let _host = agent.sync.open_room(CHAT).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    let ms = agent.sync.open_ms(CHAT).await.expect("the room is open");
    assert!(
        (40..5_000).contains(&ms),
        "the clock runs from the open, got {ms} ms"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// The relay's `chat_open` push, as it arrives when a row landed in a room the box holds no
/// socket in. What it costs to act on is the first leg of a cold chat's reply.
#[tokio::test]
async fn a_chat_open_push_reports_what_it_cost_and_what_was_waiting() {
    let (agent, root) = test_agent().await;
    let journal = Journal::default();
    {
        let _capture = journal.capture_info();
        agent
            .handle_push(
                "chat_open",
                json!({
                    "chat_id": CHAT,
                    "reason": "rows_pending",
                    "head_seq": 1,
                }),
            )
            .await;
    }
    let text = journal.text();
    assert!(
        text.contains("chat_open push: room opened"),
        "the push is still logged:\n{text}"
    );
    assert!(text.contains("open_ms="), "with its cost:\n{text}");
    assert!(
        text.contains("pending=1"),
        "and with what the relay said was waiting:\n{text}"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// A push that names waiting rows means a turn is already owed. pi's session switch does not
/// depend on the room, so it starts here rather than after the room has caught up.
///
/// There is no pi on a test box, so the prefetch fails — the mark is that it was attempted and
/// that its outcome was recorded, which is what makes the overlap readable on a real box.
#[tokio::test]
async fn pending_rows_start_the_session_switch_while_the_room_connects() {
    let (agent, root) = test_agent().await;
    let journal = Journal::default();
    {
        let _capture = journal.capture_info();
        agent
            .handle_push(
                "chat_open",
                json!({
                    "chat_id": CHAT,
                    "reason": "rows_pending",
                    "head_seq": 3,
                }),
            )
            .await;
    }
    let text = journal.text();
    assert!(
        text.contains("session prefetch"),
        "a push with work waiting warms the session:\n{text}"
    );

    // A push with nothing waiting is just a room to open: no session is touched for it, because
    // switching pi's session costs a turn's worth of state and nothing has asked for one.
    let (agent2, root2) = test_agent().await;
    let quiet = Journal::default();
    {
        let _capture = quiet.capture_info();
        agent2
            .handle_push("chat_open", json!({ "chat_id": CHAT, "reason": "created" }))
            .await;
    }
    let text = quiet.text();
    assert!(
        !text.contains("session prefetch"),
        "an empty new chat warms nothing:\n{text}"
    );
    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(root2);
}
