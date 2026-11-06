//! What `/box/state` tells a client is outstanding, and why it cannot be `jobs_running`.
//!
//! `jobs_running` counts asks answered beside a running turn — runs with no job row, no job id
//! and nothing `list_jobs`, `steer_job` or `stop_job` can name (`start_job_for_send`). Real jobs
//! take the `start_forced_job` branch and never touch it. So the Mac machine profile, which read
//! that counter for its "Background jobs" row, printed "None" for a workspace running three
//! jobs, and the same counter is decremented when a worker parks on an approval card — leaving
//! "None" beside cards that have been open for days.
//!
//! [`Agent::work_outstanding`] is the replacement, and these two cases are the ones the counter
//! misses: a job row with no ask running, and an approval card with nothing else at all.

use arbos_core::ids::ulid;
use serde_json::json;

use crate::status::tests::test_agent;

const CHAT: &str = "01TESTCHATWORKOUTSTAND001";

fn asks_running(agent: &std::sync::Arc<crate::agent::Agent>) -> usize {
    agent.jobs_running.load(std::sync::atomic::Ordering::SeqCst)
}

/// Three jobs the user can name, and nothing answering beside a turn: the counter says zero and
/// the count says three.
#[tokio::test]
async fn open_job_rows_count_while_no_ask_is_running() {
    let (agent, _root) = test_agent().await;

    for (title, state) in [
        ("Running the long loop", "running"),
        ("Reading the design doc", "running"),
        ("Waiting for a slot", "queued"),
    ] {
        let id = ulid();
        agent
            .store
            .insert_job(&id, CHAT, &ulid(), title, "other", "do the thing", &[])
            .expect("the row is written");
        agent.store.set_job_state(&id, state, None).unwrap();
    }

    assert_eq!(asks_running(&agent), 0, "no ask runs beside a turn");
    assert_eq!(
        agent.work_outstanding(),
        Some((3, 0)),
        "three open job rows are three pieces of outstanding work"
    );
}

/// One approval card and nothing else: no job row, no ask, no running turn. This is the park's
/// steady state, and the counter is zero through all of it.
#[tokio::test]
async fn an_open_approval_card_counts_on_its_own() {
    let (agent, _root) = test_agent().await;

    agent
        .store
        .insert_approval(
            &ulid(),
            CHAT,
            &ulid(),
            "Delete the old backups",
            "high",
            &json!({ "tool": "shell", "input": { "command": "rm -rf /work/backups" } }),
            0,
        )
        .expect("the card is written");

    assert_eq!(asks_running(&agent), 0, "no ask runs beside a turn");
    assert_eq!(
        agent.work_outstanding(),
        Some((0, 1)),
        "a card nobody has answered is work outstanding on the owner"
    );
}

/// A decided card stops counting, so the number cannot ratchet up over a box's life. Answering
/// is the only thing that closes it: there is no expiry behind this count.
#[tokio::test]
async fn a_decided_card_stops_counting() {
    let (agent, _root) = test_agent().await;
    let id = ulid();
    agent
        .store
        .insert_approval(
            &id,
            CHAT,
            &ulid(),
            "Send the email",
            "medium",
            &json!({}),
            0,
        )
        .expect("the card is written");
    assert_eq!(agent.work_outstanding(), Some((0, 1)));

    assert!(agent
        .store
        .decide_approval(&id, "approved", Some("owner"), None)
        .expect("the decision is recorded"));
    assert_eq!(
        agent.work_outstanding(),
        Some((0, 0)),
        "an answered card is not outstanding"
    );
}

/// An empty workspace really is empty: the count is zero rather than absent, so a client can
/// tell "nothing outstanding" from "this box does not report it".
#[tokio::test]
async fn an_empty_workspace_counts_zero_rather_than_nothing() {
    let (agent, _root) = test_agent().await;
    assert_eq!(agent.work_outstanding(), Some((0, 0)));
}
