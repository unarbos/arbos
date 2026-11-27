//! A job waiting for a free slot is not an orphan, however long it waits.
//!
//! The orphan sweep reads a worker row that says `queued` or `running`, has no run in flight and
//! is older than fifteen minutes as work whose task died, and closes its row in the chat as
//! "Stopped". A job queued behind hour-long jobs matches all three while its task is alive.

use std::time::Duration;

use arbos_core::doc::UserMessageOpts;
use serde_json::Value;

use crate::status::tests::test_agent;

const CHAT: &str = "01TESTCHATJOBWAITSWEEP001";
const ASK: &str = "01TESTASKJOBWAITSWEEP0001";

#[tokio::test(flavor = "multi_thread")]
async fn a_job_that_has_waited_past_the_orphan_window_keeps_its_row() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    host.lock()
        .await
        .doc
        .append_user_message(
            ASK,
            "run this in the background",
            "dev-mac",
            &UserMessageOpts::default(),
        )
        .unwrap();
    // Every worker slot is taken, as it is behind four long jobs.
    let held = agent
        .subagent_slots
        .clone()
        .acquire_many_owned(agent.subagent_slots.available_permits() as u32)
        .await
        .unwrap();
    let started = agent
        .start_job(
            CHAT,
            &host,
            ASK,
            "Counting lines",
            "Count the licence lines and report the number.",
            "ops",
            vec![],
            false,
            &mut None,
        )
        .await
        .expect("the job is taken");
    let job = started["job_id"].as_str().unwrap().to_owned();

    // The worker enrols and waits for a slot.
    let mut worker = String::new();
    for _ in 0..200 {
        worker = agent
            .store
            .job(&job)
            .unwrap()
            .and_then(|r| r.subagent_id)
            .unwrap_or_default();
        let queued = agent
            .store
            .subagent(&worker)
            .ok()
            .flatten()
            .is_some_and(|s| s.status == "queued");
        if queued {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(!worker.is_empty(), "the job has a worker waiting");

    // Twenty minutes pass.
    agent
        .store
        .with(|c| {
            c.execute(
                "UPDATE subagents SET created_at = created_at - 1200000, last_seen_at = NULL, \
                 last_turn_at = NULL WHERE id = ?1",
                [&worker],
            )
        })
        .unwrap();
    agent.reclaim_orphan_subagents(false).await;

    let row = agent.store.subagent(&worker).unwrap().unwrap();
    let stopped = host.lock().await.doc.messages().into_iter().any(|m| {
        m.rest.get("subagentId").and_then(Value::as_str) == Some(worker.as_str())
            && m.status != "streaming"
    });
    drop(held);
    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(
        row.status, "queued",
        "the waiting worker was read as an orphan: {row:?}"
    );
    assert!(!stopped, "the waiting job's row was closed in the chat");
}
