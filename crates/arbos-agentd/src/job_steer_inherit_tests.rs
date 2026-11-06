//! Bug AG: an accepted correction is dropped when the work moves to a follow-up job.
//!
//! Jacob on Test5, round 13 (agentd 0.1.196 `508151f6a`, chat `01M6PQF7NGTJ7RDQ4H0AN8Y10V`).
//! Five changes arrived one second apart. One of them — "top 50" — reached the running job as a
//! `steer_job` and was acknowledged. A second job then posted a confident "final" that silently
//! reverted three of the accepted changes (src-only, len ≥ 5, no capitals) and presented the
//! result as correct.
//!
//! **What the brief for this fix got wrong, and what it got right.** It asked why only one of
//! four changes became a `steer_job`, and for the aggregation to be fixed. There is no
//! aggregation: three of the four arrived *before* `job_start`, and `steer_job` resolves only a
//! running job (or a done one inside the replace window), so any attempt then is refused with
//! "no job is running in this chat" and writes nothing at all — no T3 row, no `steers` entry.
//! That half is a capture problem and is held pending a decision. What is fixed here is the
//! half where the system *did* record the user's correction and then dropped it anyway.
//!
//! **The two drops, which are independent — mending either alone still loses the change.**
//!
//! 1. *By kind.* `Agent::correction_reached_worker` records an accepted correction in T3 as
//!    kind `job_steer`, the only machine-readable proof the user was told it was taken.
//!    `recent_turns_for_brief` — the sole context any code-started job gets — kept `user` and
//!    `assistant` rows and dropped that one.
//! 2. *By budget, applied before the kind filter.* It asked `t3_thread` for twelve rows, whose
//!    SQL limit runs `ORDER BY id DESC` over **every** kind, and filtered afterwards in Rust.
//!    A job's own bookkeeping therefore evicted the user's turns from the window meant to hold
//!    them. Round 13's burst cleared twelve rows by itself.
//!
//! Neither drop was the whole of it. Raw user messages under "Recent turns in this chat" are
//! background prose, not requirements. [`crate::agent::job_steers_note`] is the one carrier that
//! states corrections as overriding the brief, and all of its call sites — like all readers of
//! `JobRow::steers` — key on a single job id, so no job could inherit another's.
//! [`Agent::steers_to_inherit`] is the missing carrier, and these tests check the binding
//! wording rather than merely the text.
//!
//! **The two ablations, as asked.**
//!
//! - *Behaviour.* Revert either half of the fix and the matching test goes red:
//!   `an_accepted_correction_is_a_turn_a_brief_carries` for the kind,
//!   `a_jobs_own_bookkeeping_does_not_evict_the_users_turns` for the budget, and
//!   `a_follow_up_job_inherits_a_correction_already_accepted` for the carrier. The window the
//!   carrier is bounded by is held from both sides, by
//!   `a_correction_still_binds_across_a_gap_wider_than_the_replace_window` and
//!   `a_correction_from_finished_work_does_not_bind_an_unrelated_later_job`, so neither
//!   widening it away nor narrowing it back to the replace window passes.
//! - *Timing, with the job boundary as the discriminator.* The same correction, the same job
//!   and the same store give opposite answers either side of `start_job`:
//!   `a_follow_up_job_inherits_a_correction_already_accepted` requires it in the brief when it
//!   was accepted first, and `a_correction_accepted_after_the_follow_up_job_started_is_not_in_it`
//!   requires it absent when it was accepted a moment later. A fix that copied every correction
//!   in the chat into every brief would pass the first and fail the second.

use std::sync::Arc;

use serde_json::json;

use crate::agent::Agent;
use crate::status::tests::test_agent;

const CHAT: &str = "01TESTCHATSTEERINHERIT0001";
const ASK: &str = "m-busy";
const FIRST: &str = "01TESTJOBINHERITFIRST00001";
const WORKER: &str = "01TESTWORKERINHERIT0000001";

/// The job the user corrected, as `run_subagent` leaves one while it is still working.
async fn a_corrected_job(agent: &Arc<Agent>, id: &str, correction: &str) {
    agent
        .store
        .insert_job(
            id,
            CHAT,
            ASK,
            "Building the word list",
            "other",
            "Write out/words.csv from the repository.",
            &[],
        )
        .unwrap();
    agent
        .store
        .insert_subagent(WORKER, "job-word-list", "build the word list", "pi", CHAT)
        .unwrap();
    agent.store.set_job_subagent(id, WORKER).unwrap();
    agent.store.set_job_state(id, "running", None).unwrap();
    accept_a_correction(agent, id, correction);
}

/// Exactly what `Agent::correction_reached_worker` writes once a correction has reached the
/// worker's session: the job row, and the T3 row that is the proof it was accepted. Both,
/// because the two halves of this bug drop one each.
fn accept_a_correction(agent: &Arc<Agent>, job_id: &str, text: &str) {
    agent.store.append_job_steer(job_id, text).unwrap();
    let _ = agent.store.t3_append(
        CHAT,
        None,
        Some(CHAT),
        "job_steer",
        Some("user"),
        text,
        Some(&json!({ "job_id": job_id })),
    );
}

/// The brief the next job of this chat is given, read off its own row — not off the prompt one
/// worker happened to get, because the row is what a respawn and the inline fallback read too.
async fn brief_of_the_next_job(agent: &Arc<Agent>) -> String {
    let started = agent
        .hook_start_job(
            "Writing the final list",
            "Produce the final word list the user asked for and deliver it as one file.",
            "other",
            vec![],
        )
        .await
        .unwrap();
    let id = started["job_id"].as_str().unwrap();
    agent.store.job(id).unwrap().unwrap().brief
}

/// Round 13 itself: the correction is accepted, the work then moves to a second job, and that
/// job has to be told. Before this change nothing carried it — every reader of `JobRow::steers`
/// keyed on one job id — so the follow-up began again from the original ask.
#[tokio::test]
async fn a_follow_up_job_inherits_a_correction_already_accepted() {
    let (agent, _root) = test_agent().await;
    let _host = agent.sync.open_room(CHAT).await.unwrap();
    let _busy = agent.test_hold_turn(CHAT).await;
    a_corrected_job(&agent, FIRST, "only the top 50").await;

    let brief = brief_of_the_next_job(&agent).await;

    assert!(
        brief.contains("only the top 50"),
        "the follow-up job was not told about a change the user had already been told was \
         taken, which is the silent revert this bug is: {brief}"
    );
    assert!(
        brief.contains("override the brief"),
        "the correction reached the follow-up job as prose rather than as a requirement, which \
         is the difference round 13 turned on — the second job read the original ask as still \
         authoritative and called its answer final: {brief}"
    );
}

/// The timing ablation, with the job boundary as the discriminator. This is the same store, the
/// same job and the same correction as the test above; only the order changed. A correction
/// accepted *after* the follow-up job started belongs to the work that was already running and
/// reaches it as a steer, so writing it into this job's brief as a settled requirement would be
/// a different bug. A fix that copied every correction in the chat into every brief would pass
/// the test above while failing this one.
#[tokio::test]
async fn a_correction_accepted_after_the_follow_up_job_started_is_not_in_it() {
    let (agent, _root) = test_agent().await;
    let _host = agent.sync.open_room(CHAT).await.unwrap();
    let _busy = agent.test_hold_turn(CHAT).await;
    a_corrected_job(&agent, FIRST, "only the top 50").await;

    let started = agent
        .hook_start_job(
            "Writing the final list",
            "Produce the final word list the user asked for and deliver it as one file.",
            "other",
            vec![],
        )
        .await
        .unwrap();
    let id = started["job_id"].as_str().unwrap().to_owned();
    // A second later, and on the far side of the boundary.
    accept_a_correction(&agent, FIRST, "no capitals");

    let brief = agent.store.job(&id).unwrap().unwrap().brief;
    assert!(
        brief.contains("only the top 50"),
        "the correction accepted before this job started still has to be in it: {brief}"
    );
    assert!(
        !brief.contains("no capitals"),
        "a correction accepted after this job started was written into its brief, so the brief \
         is a copy of the chat rather than of what was settled when the job began: {brief}"
    );
    // Not vacuous: the later correction was recorded, just not here.
    let steers = agent.store.job(FIRST).unwrap().unwrap().steers;
    assert_eq!(steers, vec!["only the top 50", "no capitals"]);
}

/// The window is wider than the one it would have been natural to borrow, and this is why.
///
/// Round 13 pins the first job's four CSVs at 14:46:53–14:47:12 and the re-run's final at
/// 14:47:50, and never states when the re-run started: it began somewhere in a 38-second gap,
/// with its own execution inside that gap too. At `STEER_REPLACE_WINDOW_MS` the carrier lands
/// on the wrong side of the boundary for a plausible share of that interval, so the fix would
/// not demonstrably have fixed the round it was written for.
///
/// Ninety seconds here is deliberately outside the replace window and inside the inherit one:
/// this test goes red against the thirty-second constant and green against the two-minute one,
/// which is what makes the widening a decision the suite holds rather than a number in a diff.
#[tokio::test]
async fn a_correction_still_binds_across_a_gap_wider_than_the_replace_window() {
    let (agent, _root) = test_agent().await;
    let _host = agent.sync.open_room(CHAT).await.unwrap();
    let _busy = agent.test_hold_turn(CHAT).await;
    a_corrected_job(&agent, FIRST, "only the top 50").await;
    agent.store.set_job_state(FIRST, "done", None).unwrap();
    let gap = crate::config::STEER_REPLACE_WINDOW_MS + 60_000;
    assert!(
        gap < crate::config::STEER_INHERIT_WINDOW_MS,
        "this test only means something while the gap sits between the two windows"
    );
    agent.store.test_backdate_job_end(FIRST, gap).unwrap();

    let brief = brief_of_the_next_job(&agent).await;

    assert!(
        brief.contains("only the top 50"),
        "a correction stopped binding across a gap the size of round 13's, so the fix would not \
         have fixed the round it was written for: {brief}"
    );
}

/// The far side of the same bound. These lines arrive as requirements, so a correction from
/// work the chat has finished with must not govern an unrelated job asked for later — a worse
/// failure than the one being fixed, and a harder one to see.
#[tokio::test]
async fn a_correction_from_finished_work_does_not_bind_an_unrelated_later_job() {
    let (agent, _root) = test_agent().await;
    let _host = agent.sync.open_room(CHAT).await.unwrap();
    let _busy = agent.test_hold_turn(CHAT).await;
    a_corrected_job(&agent, FIRST, "only the top 50").await;
    agent.store.set_job_state(FIRST, "done", None).unwrap();
    agent
        .store
        .test_backdate_job_end(FIRST, crate::config::STEER_INHERIT_WINDOW_MS + 60_000)
        .unwrap();

    let brief = brief_of_the_next_job(&agent).await;

    assert!(
        !brief.contains("only the top 50"),
        "a correction from work the chat had finished with was made binding on a later job: \
         {brief}"
    );
}

/// Drop one, by kind. An accepted correction is a turn of the chat, and the brief builder kept
/// `user` and `assistant` rows only — so the single machine-readable record that the user was
/// promised a change was the one row it threw away.
#[tokio::test]
async fn an_accepted_correction_is_a_turn_a_brief_carries() {
    let (agent, _root) = test_agent().await;
    let _ = agent.store.t3_append(
        CHAT,
        None,
        Some(CHAT),
        "user",
        Some("user"),
        "make me a word list",
        None,
    );
    accept_a_correction(&agent, FIRST, "only the top 50");

    let turns = agent.recent_turns_for_brief(CHAT);

    assert!(
        turns.contains("only the top 50"),
        "the accepted correction is missing from the turns a brief carries: {turns}"
    );
    assert!(
        turns.contains("accepted"),
        "the correction is in the turns but reads like any other remark; a worker cannot tell a \
         change the user was promised from one they wondered aloud about: {turns}"
    );
}

/// Drop two, by budget. The twelve-row limit ran over every kind before the kind filter, so a
/// job's own bookkeeping evicted the user from the window that exists to carry the user. The
/// bookkeeping kinds here are ones a brief never wanted, which is the point: they cost the user
/// their place without taking one themselves.
#[tokio::test]
async fn a_jobs_own_bookkeeping_does_not_evict_the_users_turns() {
    let (agent, _root) = test_agent().await;
    let _ = agent.store.t3_append(
        CHAT,
        None,
        Some(CHAT),
        "user",
        Some("user"),
        "make me a word list, src only",
        None,
    );
    for n in 0..20 {
        let _ = agent.store.t3_append(
            CHAT,
            None,
            Some(CHAT),
            "job_note",
            Some("system"),
            &format!("progress {n}"),
            None,
        );
    }

    let turns = agent.recent_turns_for_brief(CHAT);

    assert!(
        turns.contains("src only"),
        "the user's own ask was pushed out of the brief's window by a job's progress notes, so \
         the follow-up job never saw the constraint it was meant to keep: {turns}"
    );
    assert!(
        !turns.contains("progress 19"),
        "the bookkeeping rows are in the brief; they were supposed to be filtered, not carried: \
         {turns}"
    );
}
