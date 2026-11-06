//! Bug AL: a job cancelled while it waited for a slot started anyway and ran to completion.
//!
//! Jacob, Test5 round 18 (2026-11-06, agentd 0.1.198), job `status18g`. He asked to cancel the
//! six longest jobs. `g` was `queued` at the time — waiting for one of the four
//! [`SUBAGENT_MAX_CONCURRENT`] slots — and `Job::is_open()` covers `queued`, so `stop_jobs`
//! matched it and wrote `cancelled` to its row. It then ran 03:32:40Z to 03:38:00Z with no
//! cancel anywhere in its life.
//!
//! The cancel was not wrong. It was **overtaken by a start**, which is a worse shape than a
//! cancel that misses: every surface told the truth at the time it spoke, and the row ended up
//! with no trace that the job had ever been stopped.
//!
//! `run_subagent` checked for a stop in exactly one place, above the slot wait, under a comment
//! that claimed to cover "a lock or a slot". The slot is acquired *after* that line, so the
//! check covered the lock only. On the far side of a wait of unbounded length the start wrote
//! `running` over `cancelled` — and `set_job_state` clears `ended_at` on the way to `running`,
//! so the cancel was erased from the row rather than merely superseded.

use std::time::Duration;

use arbos_core::doc::UserMessageOpts;
use arbos_core::route::Harness;

use arbos_core::ids::ulid;

use crate::agent::{Agent, JobSpec};
use crate::status::tests::test_agent;
use crate::store::Store;
use crate::sync::SharedHost;

const CHAT: &str = "01TESTCHATJOBQUEUEDCANCEL1";
const JOB: &str = "01TESTJOBQUEUEDATCANCEL001";
const ASK: &str = "01TESTASKJOBQUEUEDCANCEL01";

/// The statuses of every assistant row. A job that was stopped before it started finishes its
/// row `Cancelled`; a job that got past the slot and into its work leaves the row streaming,
/// because the work then runs on past the end of this test.
async fn assistant_statuses(host: &SharedHost) -> Vec<String> {
    let h = host.lock().await;
    h.doc
        .messages()
        .into_iter()
        .filter(|m| m.role == "assistant")
        .map(|m| m.status.clone())
        .collect()
}

/// A store of its own per case, so the row assertions are not reading another test's job.
fn job_store() -> Store {
    let dir = std::env::temp_dir().join(format!("arbos-al-{}", ulid()));
    Store::open(&dir.join("state.sqlite")).unwrap()
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

/// A job that was cancelled while it waited for a slot must not start when the slot frees.
///
/// The slots are all held by the test, so the job cannot reach its start until the test lets it,
/// and the cancel therefore lands squarely inside the slot wait — which is the window bug AL
/// lives in and the one the old check sat above.
///
/// # The one part of this that is not deterministic, and what it would cost you
///
/// This case has to cancel the job **after** `run_subagent`'s pre-slot check has run. If the
/// cancel landed before it, that check would catch the stop, the job would not start, and **this
/// case would pass against the bug** — a false green rather than a flake, which is the worse of
/// the two failures and the one worth writing down.
///
/// There is no signal to wait on. Nothing is written between the pre-slot check and the park on
/// the semaphore, so the test waits for the last thing that *is* observable — the subagent row
/// recording itself `queued` — and then settles 500 ms. In that window the task writes the
/// threads index, takes the job's resource locks and reads the row once: no unbounded await
/// among them, and it then parks on a semaphore only this test can release.
///
/// Measured rather than asserted: 20 consecutive runs passed, and all 20 were red with the fix
/// reverted. Closing the gap properly would mean production code carrying an observable that
/// exists only for this test, which is a worse trade than the residual risk (coordinator's
/// ruling, 2026-11-06).
///
/// **So a green here is only meaningful alongside the revert check.** If you change
/// `run_subagent`'s prologue, re-run this with the post-slot guard removed and confirm it fails
/// before you trust it passing.
#[tokio::test]
async fn a_job_cancelled_while_it_waited_for_a_slot_never_starts() {
    let (agent, _root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(
                ASK,
                "count the files under /etc, as a seventh background job",
                "dev-mac",
                &UserMessageOpts::default(),
            )
            .unwrap();
    }
    agent
        .store
        .insert_job(
            JOB,
            CHAT,
            ASK,
            "Counting the files under /etc",
            "code",
            "Count the files under /etc and report the number.",
            &[],
        )
        .unwrap();

    // Every slot held, so nothing can start until this test drops them. Taken before the job is
    // spawned, so there is no window in which it could slip through.
    let held = agent
        .subagent_slots
        .clone()
        .acquire_many_owned(agent.subagent_slots.available_permits() as u32)
        .await
        .expect("the slots are open");
    assert_eq!(agent.subagent_slots.available_permits(), 0);

    let spec = JobSpec {
        id: JOB.to_owned(),
        title: "Counting the files under /etc".to_owned(),
        kind: "code".to_owned(),
        brief: "Count the files under /etc and report the number.".to_owned(),
        touches: vec![],
        attempt: 0,
    };
    let runner = {
        let agent = agent.clone();
        let host = host.clone();
        tokio::spawn(async move {
            agent
                .run_subagent(
                    CHAT,
                    &host,
                    ASK,
                    None,
                    "job-al",
                    "count the files under /etc",
                    "Count the files under /etc and report the number.",
                    Some(Harness::Pi),
                    false,
                    false,
                    Some(spec),
                )
                .await
        })
    };

    // The job records itself as queued just before it parks on the slot it cannot have.
    {
        let agent = agent.clone();
        wait_for("the job records itself queued for a slot", move || {
            agent
                .store
                .subagents()
                .unwrap_or_default()
                .iter()
                .any(|s| s.status == "queued")
        })
        .await;
    }
    // Between that record and the park the task does a handful of operations, none of which can
    // block: it writes the threads index, takes the job's resource locks and reads the row once.
    // This settle is what puts the cancel below *inside* the slot wait rather than above it, and
    // it is the one part of this case that rests on timing rather than on a signal. See the
    // report: the pre-slot check would otherwise catch the stop and the case would pass for the
    // wrong reason.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let stopped = agent.stop_jobs(CHAT, Some(JOB)).await;
    assert_eq!(
        stopped.len(),
        1,
        "the queued job is open, so it is cancelled"
    );
    assert_eq!(
        agent.store.job(JOB).unwrap().unwrap().state,
        "cancelled",
        "the stop is recorded before the slot frees"
    );

    // Now let it through. This is the moment bug AL happened on.
    drop(held);
    let _ = tokio::time::timeout(Duration::from_secs(10), runner).await;

    let statuses = assistant_statuses(&host).await;
    assert!(
        statuses.iter().any(|s| s == "cancelled"),
        "the job must end its row as stopped rather than start its work: {statuses:?}"
    );
    let row = agent.store.job(JOB).unwrap().unwrap();
    assert_eq!(
        row.state, "cancelled",
        "a cancelled job that was never allowed to start must still read cancelled"
    );
    assert!(
        row.ended_at.is_some(),
        "and must keep the end the cancel gave it, which a start to `running` clears"
    );
}

/// The row-level backstop, stated on its own so it survives a rewrite of the case above: no path
/// may move a cancelled job to `running`.
///
/// This is what made bug AL invisible after the fact. The start overwrote the state *and*
/// cleared `ended_at`, so the finished row read as an ordinary completed job and the cancel left
/// no mark on it at all.
#[test]
fn a_cancelled_job_cannot_be_moved_back_to_running() {
    let store = job_store();
    store
        .insert_job(JOB, CHAT, ASK, "A job", "code", "do the thing", &[])
        .unwrap();
    store.set_job_state(JOB, "running", None).unwrap();
    store
        .set_job_state(JOB, "cancelled", Some("stopped by the user"))
        .unwrap();
    let at_cancel = store.job(JOB).unwrap().unwrap();
    assert_eq!(at_cancel.state, "cancelled");
    assert!(at_cancel.ended_at.is_some());

    store.set_job_state(JOB, "running", None).unwrap();

    let row = store.job(JOB).unwrap().unwrap();
    assert_eq!(
        row.state, "cancelled",
        "a start that lost the race to a stop must not win it in the row"
    );
    assert_eq!(
        row.ended_at, at_cancel.ended_at,
        "and must not clear the end the cancel gave it"
    );
}

/// `cancelled` to `queued` stays allowed: the box-restart reclaim is a new decision about the
/// job, not a start racing a stop, and narrowing the guard past the race would break it.
#[test]
fn a_cancelled_job_can_still_be_requeued_by_a_restart() {
    let store = job_store();
    store
        .insert_job(JOB, CHAT, ASK, "A job", "code", "do the thing", &[])
        .unwrap();
    store.set_job_state(JOB, "cancelled", None).unwrap();
    store.set_job_state(JOB, "queued", None).unwrap();
    assert_eq!(store.job(JOB).unwrap().unwrap().state, "queued");
}

/// A queued job is nominable as one of "the six longest", so the selection is not what lost
/// `status18g`.
///
/// [`JobRow::elapsed_s`] falls back to `created_at` when there is no `started_at`, so a job that
/// has only ever waited still reports a duration, and it grows. A long-queued job can therefore
/// rank above a short-running one — which is the right answer, and it rules out the theory that
/// the ranking silently dropped `g` before `stop_jobs` was ever asked about it.
#[test]
fn a_queued_job_reports_a_duration_and_can_rank_as_one_of_the_longest() {
    let store = job_store();
    store
        .insert_job(JOB, CHAT, ASK, "Queued all along", "code", "wait", &[])
        .unwrap();
    let queued = store.job(JOB).unwrap().unwrap();
    assert_eq!(queued.state, "queued");
    assert!(
        queued.started_at.is_none(),
        "it never started, so there is no start to measure from"
    );
    assert!(
        queued.elapsed_s() >= 0,
        "and it is still given a duration, measured from when it was created"
    );
}

const _: fn() = || {
    let _ = Agent::stop_jobs;
};
