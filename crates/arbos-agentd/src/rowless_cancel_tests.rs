//! Bug AL: a stop could not reach an ask that owned no job row, and the ask started afterwards.
//!
//! An ask only gets a job row when the predicate in `forced_job.rs` reads the user's wording as
//! forcing one. Everything else runs through `run_subagent` with `job: None` — the same worker,
//! the same slot queue, the same unbounded wait — and owns nothing in the `jobs` table.
//!
//! `stop_jobs` iterates `jobs_for_chat`. A row-less ask is therefore not something it fails to
//! kill; it is something it never enumerates. Every surface downstream then tells the truth
//! about an incomplete set: the stop line names the jobs it cancelled, `stop_job("all")` used to
//! answer `no running job matched`, and both were accurate about the rows and silent about the
//! ask. Jacob's round 18 is the shape in full: five asks, four cancelled, `status18g` not named
//! by the cancel at all, and nineteen seconds later `g` started and ran its 330-second loop to
//! completion.
//!
//! That silence is why this went unnoticed for so long. A cancel that reports a failure gets
//! investigated; a cancel that reports a smaller success than the user asked for reads as
//! working software.
//!
//! The fix holds each row-less ask by session id from its creation until the moment it starts,
//! so a chat-wide stop has something to name, and re-checks on the far side of the slot wait so
//! a stop landing inside that wait is not overtaken by the start.

use std::time::Duration;

use arbos_core::doc::UserMessageOpts;
use arbos_core::route::Harness;
use serde_json::Value;

use crate::agent::Agent;
use crate::status::tests::test_agent;
use crate::sync::SharedHost;

const CHAT: &str = "01TESTCHATROWLESSCANCEL001";
const OTHER_CHAT: &str = "01TESTCHATROWLESSCANCEL002";
const ASK: &str = "01TESTASKROWLESSCANCEL0001";

/// Every status part on every assistant row, as `code: message`.
///
/// The assertions below are about something that must *not* be in this list, so the list itself
/// is the evidence: a failure prints what the ask actually did instead of reporting that an
/// expectation was unmet. "It never started" is unobservable; "here is everything it did, and
/// `Working…` is not among it" is not.
async fn status_parts(host: &SharedHost) -> Vec<String> {
    let h = host.lock().await;
    h.doc
        .messages()
        .into_iter()
        .filter(|m| m.role == "assistant")
        .flat_map(|m| m.parts.clone())
        .filter_map(|p: Value| {
            let code = p.get("code")?.as_str()?.to_owned();
            let message = p.get("message")?.as_str()?.to_owned();
            Some(format!("{code}: {message}"))
        })
        .collect()
}

async fn assistant_statuses(host: &SharedHost) -> Vec<String> {
    let h = host.lock().await;
    h.doc
        .messages()
        .into_iter()
        .filter(|m| m.role == "assistant")
        .map(|m| m.status.clone())
        .collect()
}

async fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    for _ in 0..200 {
        if ready() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("{what} did not happen within 5s");
}

/// Park a row-less ask on the slot semaphore and hand back its session id.
///
/// Every slot is taken before the ask is spawned, so there is no window in which it could start;
/// the returned guard is the only thing that can let it through.
async fn ask_parked_on_a_slot(
    agent: &std::sync::Arc<Agent>,
    host: &SharedHost,
    chat_id: &str,
) -> (
    tokio::sync::OwnedSemaphorePermit,
    tokio::task::JoinHandle<anyhow::Result<()>>,
    String,
) {
    let held = agent
        .subagent_slots
        .clone()
        .acquire_many_owned(agent.subagent_slots.available_permits() as u32)
        .await
        .expect("the slots are open");
    assert_eq!(agent.subagent_slots.available_permits(), 0);

    let runner = {
        let agent = agent.clone();
        let host = host.clone();
        let chat_id = chat_id.to_owned();
        tokio::spawn(async move {
            agent
                .run_subagent(
                    &chat_id,
                    &host,
                    ASK,
                    None,
                    "ask-al",
                    "count the files under /etc",
                    "Count the files under /etc and report the number.",
                    Some(Harness::Pi),
                    false,
                    false,
                    // The whole point: no job row exists for any stop to find.
                    None,
                )
                .await
        })
    };

    // The ask records itself `queued` immediately before it parks on the slot it cannot have.
    {
        let agent = agent.clone();
        wait_for("the ask records itself queued for a slot", move || {
            agent
                .store
                .subagents()
                .unwrap_or_default()
                .iter()
                .any(|s| s.status == "queued")
        })
        .await;
    }
    let subagent_id = agent
        .store
        .subagents()
        .unwrap()
        .into_iter()
        .find(|s| s.status == "queued")
        .expect("the queued ask")
        .id;
    (held, runner, subagent_id)
}

/// A chat-wide stop must reach an ask that owns no job row, and the ask must never start.
///
/// # Why this is red without the fix, and what it reports
///
/// Without the hold, `stop_jobs` enumerates nothing, `cancel_pending_asks` has nothing to
/// cancel, and when the slots free the ask proceeds into its work and writes the `Working…`
/// status part. The failure then reads as the part list including `working: Working…`, which is
/// the defect itself rather than a missing assertion: the user stopped the chat and the ask
/// started anyway.
///
/// # The part of this that rests on timing
///
/// The stop has to land *after* the ask has been held and *before* it starts. The hold happens
/// on the near side of the slot wait and the test waits for the `queued` row that precedes it,
/// then settles. If the stop landed earlier than the hold the ask would not be held, the case
/// would go red, and a red here is never a false alarm — so the timing risk in this case is a
/// flake, not a false green. That is the safe direction for it to fail in, and it is the
/// opposite of the risk in `job_queued_cancel_tests`.
#[tokio::test]
async fn a_stop_reaches_an_ask_that_owns_no_job_row() {
    let (agent, _root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(
                ASK,
                "count the files under /etc for me",
                "dev-mac",
                &UserMessageOpts::default(),
            )
            .unwrap();
    }

    let (held, runner, subagent_id) = ask_parked_on_a_slot(&agent, &host, CHAT).await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // The bare stop, exactly as the chat issues it: no job named.
    let stopped = agent.stop_jobs(CHAT, None).await;
    assert!(
        stopped.is_empty(),
        "there is no job row here, so the job path truthfully reaches nothing: {stopped:?}"
    );
    let pending = agent.cancel_pending_asks(CHAT).await;
    assert_eq!(
        pending,
        vec![subagent_id.clone()],
        "and the ask path must name the ask the job path could not see"
    );

    // Now let it through. The slot freeing is the moment the old code started the cancelled ask.
    drop(held);
    let _ = tokio::time::timeout(Duration::from_secs(10), runner).await;

    let parts = status_parts(&host).await;
    assert!(
        !parts.iter().any(|p| p.ends_with("Working…")),
        "the ask must not begin its work after being stopped; everything it did was: {parts:?}"
    );
    assert!(
        parts.iter().any(|p| p.starts_with("stopped:")),
        "and it must say that it stopped rather than end silently: {parts:?}"
    );

    let statuses = assistant_statuses(&host).await;
    assert!(
        statuses.iter().any(|s| s == "cancelled"),
        "the ask's row ends cancelled: {statuses:?}"
    );

    // Back to `idle`, not left at `cancelled`: the stop ends this ask, it does not retire the
    // session, and the next thing the user says to it must be able to start.
    let row = agent.store.subagent(&subagent_id).unwrap().unwrap();
    assert_eq!(
        row.status, "idle",
        "a stopped ask leaves a reusable session behind"
    );
}

/// An ask that is not stopped still starts. Pinned because the cheapest way to pass the case
/// above is to stop every row-less ask, which would be a far worse bug than the one being fixed.
#[tokio::test]
async fn an_ask_nobody_stopped_starts_when_its_slot_frees() {
    let (agent, _root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(
                ASK,
                "count the files under /etc for me",
                "dev-mac",
                &UserMessageOpts::default(),
            )
            .unwrap();
    }

    let (held, runner, _subagent_id) = ask_parked_on_a_slot(&agent, &host, CHAT).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    drop(held);
    let _ = tokio::time::timeout(Duration::from_secs(10), runner).await;

    let parts = status_parts(&host).await;
    assert!(
        parts.iter().any(|p| p.ends_with("Working…")),
        "nothing cancelled this ask, so it must get past the slot and into its work: {parts:?}"
    );
}

/// A stop in one chat does not reach an ask waiting in another.
///
/// The hold is keyed by chat for this reason. A global set would make the case above pass and
/// would let one user's cancel silently end another's work.
#[tokio::test]
async fn a_stop_in_one_chat_leaves_another_chats_ask_alone() {
    let (agent, _root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(
                ASK,
                "count the files under /etc for me",
                "dev-mac",
                &UserMessageOpts::default(),
            )
            .unwrap();
    }

    let (held, runner, subagent_id) = ask_parked_on_a_slot(&agent, &host, CHAT).await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let reached = agent.cancel_pending_asks(OTHER_CHAT).await;
    assert!(
        reached.is_empty(),
        "the other chat holds no ask, so its stop reaches nothing: {reached:?}"
    );
    assert_eq!(
        agent.store.subagent(&subagent_id).unwrap().unwrap().status,
        "queued",
        "and this chat's ask is untouched by it"
    );

    drop(held);
    let _ = tokio::time::timeout(Duration::from_secs(10), runner).await;
    let parts = status_parts(&host).await;
    assert!(
        parts.iter().any(|p| p.ends_with("Working…")),
        "so it still starts when its slot frees: {parts:?}"
    );
}

/// An ask that has already started is not in the held set, so a later stop cannot pick it up
/// from there and must reach it through the running-worker path instead.
///
/// This is the boundary between this fix and #2847: before the start, the ask is held here;
/// after it, there is a live worker and the reaper owns the problem. Nothing is held in the
/// overlap, and nothing is held after.
#[tokio::test]
async fn a_started_ask_is_no_longer_held() {
    let (agent, _root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(
                ASK,
                "count the files under /etc for me",
                "dev-mac",
                &UserMessageOpts::default(),
            )
            .unwrap();
    }

    let (held, runner, _subagent_id) = ask_parked_on_a_slot(&agent, &host, CHAT).await;
    drop(held);
    let _ = tokio::time::timeout(Duration::from_secs(10), runner).await;

    let reached = agent.cancel_pending_asks(CHAT).await;
    assert!(
        reached.is_empty(),
        "the ask released its hold when it started, so a later stop finds nothing to pre-empt \
         and must go after the worker instead: {reached:?}"
    );
}

const _: fn() = || {
    let _ = Agent::cancel_pending_asks;
};
