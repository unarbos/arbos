//! `list_jobs` and the cap: a finished job must never push a live one off the list.
//!
//! The list the model answers "cancel the six longest" from was `jobs_for_chat` — which is
//! `created_at DESC` — truncated to twelve. So the twelve *newest* rows won, and **the
//! longest-running jobs are the oldest**. Past twelve jobs in a chat the list dropped exactly
//! the rows the question was about, and a job the user could still steer or stop became
//! unnameable because some finished job happened to be newer than it.
//!
//! Found while refuting a different theory about bug AL: the suspicion was that the ranking
//! excluded queued jobs, which it does not — `JobRow::elapsed_s` falls back to `created_at`, so
//! a queued job accrues a growing duration and ranks honestly against a running one. The
//! truncation was the real hole in that list, and it is a separate defect from AL.
//!
//! Round 18 held seven job rows, so this was not live in that round. It is filed and fixed on
//! its own evidence, not on round 18's.

use arbos_core::ids::ulid;

use crate::agent::JOB_LIST_CAP;
use crate::status::tests::test_agent;

const CHAT: &str = "01TESTCHATJOBLISTCAP00001";

/// A job row in the chat, created in call order so the last one made is the newest.
///
/// The pause is what makes "newest" mean anything. `insert_job` stamps `created_at` with
/// `now_ms`, and `jobs_for_chat` orders by it — so rows inserted inside one millisecond share a
/// timestamp and come back in an order SQLite does not promise. A fixture built that way cannot
/// test an ordering rule: the first draft of this file asserted which row was dropped and
/// failed against the correct fix, because every row claimed to be the same age.
///
/// `state` is written after the insert because `insert_job` starts every row `queued`.
async fn job(agent: &std::sync::Arc<crate::agent::Agent>, title: &str, state: &str) -> String {
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    let id = ulid();
    agent
        .store
        .insert_job(&id, CHAT, &ulid(), title, "other", "do the thing", &[])
        .expect("the row is written");
    if state != "queued" {
        agent.store.set_job_state(&id, state, None).unwrap();
    }
    id
}

async fn listed_ids(agent: &std::sync::Arc<crate::agent::Agent>) -> Vec<String> {
    let v = agent.hook_list_jobs().await.expect("a turn is running");
    v["jobs"]
        .as_array()
        .expect("jobs is a list")
        .iter()
        .map(|j| j["job_id"].as_str().unwrap().to_owned())
        .collect()
}

/// The boundary that matters: a live job older than a cap's worth of finished jobs is still
/// listed.
///
/// This is the shape the defect needed — more than `JOB_LIST_CAP` finished rows, all newer than
/// the live ones — and it is the shape a long-running chat reaches on its own.
#[tokio::test]
async fn a_live_job_older_than_a_whole_cap_of_finished_jobs_is_still_listed() {
    let (agent, _root) = test_agent().await;
    let _turn = agent.test_hold_turn(CHAT).await;

    // The two oldest rows in the chat, and the only ones the user can still act on.
    let long_runner = job(&agent, "Running the long loop", "running").await;
    let queued_behind = job(&agent, "Waiting for a slot", "queued").await;

    // Then a cap's worth and more of finished work on top of them, every row newer.
    for i in 0..(JOB_LIST_CAP + 5) {
        job(&agent, &format!("Finished {i}"), "done").await;
    }

    let ids = listed_ids(&agent).await;
    assert!(
        ids.contains(&long_runner),
        "the longest-running job is the oldest row and must not be dropped for newer finished \
         work: {} rows listed",
        ids.len()
    );
    assert!(
        ids.contains(&queued_behind),
        "nor the job queued behind it, which the user can still cancel"
    );
}

/// The cap still does its job: it bounds the finished tail.
///
/// Stated separately so the fix above cannot be read as "the cap is gone". A chat with a long
/// history hands the model a bounded list, and the rows it keeps are the newest finished ones.
#[tokio::test]
async fn the_cap_still_bounds_the_finished_tail() {
    let (agent, _root) = test_agent().await;
    let _turn = agent.test_hold_turn(CHAT).await;

    let oldest_finished = job(&agent, "Finished first of all", "done").await;
    for i in 0..(JOB_LIST_CAP + 5) {
        job(&agent, &format!("Finished {i}"), "done").await;
    }

    let ids = listed_ids(&agent).await;
    assert_eq!(
        ids.len(),
        JOB_LIST_CAP,
        "with nothing live, the list is the cap's worth of the newest finished jobs"
    );
    assert!(
        !ids.contains(&oldest_finished),
        "and the oldest finished row is the one dropped, which is the right thing to drop"
    );
}

/// The deliberate answer for live work past the cap: **every live job is listed and the finished
/// tail gets nothing.**
///
/// The list is allowed to exceed the cap rather than drop a live job. A live job is something
/// the user can steer or stop; a finished one is history. Truncating the live set is the defect
/// this file is about, and a cap is not worth reintroducing it for — a user who has forced
/// twenty jobs has twenty things they may legitimately ask to cancel, and a list that hides
/// eight of them is worse than a long list.
#[tokio::test]
async fn live_jobs_past_the_cap_are_all_listed_and_the_finished_tail_is_dropped_instead() {
    let (agent, _root) = test_agent().await;
    let _turn = agent.test_hold_turn(CHAT).await;

    let mut live = Vec::new();
    for i in 0..(JOB_LIST_CAP + 3) {
        live.push(job(&agent, &format!("Live {i}"), "running").await);
    }
    let finished = job(&agent, "Finished and newest of all", "done").await;

    let ids = listed_ids(&agent).await;
    for id in &live {
        assert!(
            ids.contains(id),
            "a live job is never dropped, even past the cap: {} rows listed",
            ids.len()
        );
    }
    assert!(
        !ids.contains(&finished),
        "and the newest finished job yields its place to them, rather than the other way round"
    );
    assert_eq!(ids.len(), JOB_LIST_CAP + 3);
}

/// Under the cap nothing changes: the whole chat is listed, live work and history together.
#[tokio::test]
async fn a_small_chat_lists_everything_it_has() {
    let (agent, _root) = test_agent().await;
    let _turn = agent.test_hold_turn(CHAT).await;

    let running = job(&agent, "Running", "running").await;
    let queued = job(&agent, "Queued", "queued").await;
    let done = job(&agent, "Done", "done").await;
    let failed = job(&agent, "Failed", "failed").await;

    let ids = listed_ids(&agent).await;
    assert_eq!(ids.len(), 4);
    for id in [&running, &queued, &done, &failed] {
        assert!(ids.contains(id), "every row of a small chat is listed");
    }
}
