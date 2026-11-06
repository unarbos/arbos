//! Bug AB, the half that is not a delivery race: a correction that arrives **before the job
//! exists**, or while it is still queued.
//!
//! Jacob on Test5, rounds 13 and 14 (side chat `01M6ME9A7EGV5PJ7DZW43AHTN5`). Round 14's
//! `r14race`, on image 0.1.197 `ef90488f8`:
//!
//! | 16:20:11 | the correction `R1` arrives — **three seconds before the job exists** |
//! | 16:20:14 | `job_start` |
//! | 16:20:24 | the uncorrected n² CSV is posted |
//!
//! Round 13's `r13race` is the same shape with the correction landing in `job_start`'s own
//! second. The run was honest about it — it never claimed the correction had been applied — which
//! is the only thing that kept it from being worse.
//!
//! `steer_job` could not help, by construction: it resolves a *running* job, or a done one inside
//! the replace window, so a correction landing between the model composing a brief and
//! `start_job` executing it has nothing to attach to, and the refusal wrote nothing at all — no
//! row, no `steers` entry, not even a log line. The far-side re-checks (#2819, #2828) are a
//! different window and cannot reach this one: they catch a correction that arrives while the
//! answer waits, and here there is no answer and no job yet.
//!
//! Three things were wrong, and all three are needed for the correction to survive. These tests
//! hold each of them separately, because each was independently sufficient to lose it:
//!
//! 1. **The first worker never read `steers`.** Every path that gives a job a *fresh* worker
//!    folds `job_steers_note` in — the restart, the worker after a pi death, the inline fallback
//!    — and the initial spawn passed the brief with no note. So a correction written onto the row
//!    had nowhere to go: the row kept it and the worker never saw it.
//! 2. **A queued job refused corrections.** `insert_job` writes `queued`, `steer_job_in` required
//!    `running`, and `resolve_steer_target` only fell back to `done` inside the replace window, so
//!    `queued` matched no branch. Not a seconds-wide window: a job queued behind the per-chat cap
//!    or a folder lock waits as long as the job ahead of it runs.
//! 3. **Nothing captured a correction older than the job.** `start_job` now inherits the ask's
//!    unanswered corrections onto the row it has just inserted.

use std::sync::Arc;

use arbos_core::doc::UserMessageOpts;
use arbos_core::ids::{now_ms, ulid};
use serde_json::json;

use crate::agent::Agent;
use crate::status::tests::test_agent;
use crate::sync::SharedHost;

const CHAT: &str = "01TESTCHATSTEERINHERIT0001";
const ASK: &str = "01TESTASKSTEERINHERIT00001";

/// The brief the model writes, which must reach the worker byte-identical.
const BRIEF: &str =
    "Write out/race14.csv: for n in 1..100, one row per n with n squared. Sort by n.";

/// The ask, and the `send` command that carried it, `at` milliseconds on the doc's clock.
///
/// The command matters and is not decoration: [`crate::agent::steers_to_inherit`] bounds what it
/// inherits by the ask's own `issuedAt`, so a correction belonging to whatever came before this
/// ask is not handed to this job.
async fn an_ask(host: &SharedHost, at: i64) {
    let h = host.lock().await;
    h.doc
        .append_user_message(
            ASK,
            "make me a CSV of n squared",
            "dev-mac",
            &UserMessageOpts::default(),
        )
        .unwrap();
    h.doc
        .push_command_at("send", json!({ "messageId": ASK }), "dev-mac", at);
}

/// A later user message of `CHAT`, routed as `kind` (`steer` / `guide` for a correction of the
/// running turn, `send` for a message that gets a turn of its own) at `at`. Returns its id.
async fn a_user_message(host: &SharedHost, kind: &str, text: &str, at: i64) -> String {
    let id = ulid();
    let h = host.lock().await;
    h.doc
        .append_user_message(&id, text, "dev-mac", &UserMessageOpts::default())
        .unwrap();
    h.doc
        .push_command_at(kind, json!({ "messageId": id }), "dev-mac", at);
    id
}

/// The job the model starts for [`ASK`], as `hook_start_job` makes it. Returns its id.
async fn the_model_starts_a_job(agent: &Arc<Agent>) -> String {
    let v = agent
        .hook_start_job("Building the squares table", BRIEF, "other", vec![])
        .await
        .expect("start_job");
    v["job_id"].as_str().expect("job_id").to_owned()
}

fn steers_of(agent: &Arc<Agent>, job: &str) -> Vec<String> {
    agent.store.job(job).unwrap().expect("the job row").steers
}

/// r14race, as the chat recorded it: the ask, then the correction four seconds later, then the
/// job. The correction is older than the job, so nothing that reads the job can find it — unless
/// `start_job` goes looking at the chat.
///
/// Asserts on the brief the **first worker** was handed, not on the job row. The row was never
/// the broken part: writing a correction onto it and never folding it into the initial spawn is
/// exactly how this survived two rounds, so a test that stops at the row checks the ingredients
/// and not the meal.
#[tokio::test]
async fn a_correction_that_arrived_before_the_job_existed_reaches_its_first_worker() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    let t0 = now_ms();
    an_ask(&host, t0).await;
    let _busy = agent.test_hold_turn_for(CHAT, ASK).await;
    let correction = a_user_message(&host, "steer", "no, cube them, not square", t0 + 4_000).await;
    assert!(!correction.is_empty());

    let job = the_model_starts_a_job(&agent).await;

    assert_eq!(
        steers_of(&agent, &job),
        vec!["no, cube them, not square".to_owned()],
        "the correction is inherited onto the row the job has just been given"
    );
    let brief = agent
        .test_first_worker_brief(&job)
        .expect("the first worker was started");
    assert!(
        brief.contains("no, cube them, not square"),
        "the first worker must start with the correction, not just the row: {brief}"
    );
    assert!(
        brief.starts_with(BRIEF),
        "the model's brief is carried byte-identical, with the correction appended as a \
         correction rather than edited into the spec: {brief}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// r16race, at the timings the round actually recorded — a five-second lead, the tightest of the
/// four rounds that reproduced this.
///
/// | 18:33:19 | `R1` arrives |
/// | 18:33:24 | `job_start`, **five seconds** later |
/// | 18:33:50 | `race16.csv` posts, built to the uncorrected n² spec |
/// | 18:34:27 | the corrected `race16_r1.csv` posts, 37 seconds after the wrong one |
///
/// The worker said it plainly at 18:33:57: *"I wrote race16.csv … and posted it once; **no
/// correction arrived**"*. It was right, and that is the bug — the correction was five seconds
/// older than the job and nothing carried it in.
///
/// **Why a second case rather than widening the one above.** The capture bounds what it inherits
/// by `issued_at >= asked_at` and has no upper bound, so no gap size can pass at thirty seconds
/// and fail at five — the only axis that can break is *ordering*, not duration. This case
/// therefore does not test a new code path. It pins a **measured** timing, so that a later change
/// introducing a window has to fail against a real trace rather than an invented margin.
///
/// Asserts all three of `R1`'s constraints reach the worker. A single-constraint assertion would
/// pass on a brief that carried the correction but truncated it, which is the failure that leaves
/// a user's last instruction silently half-applied.
#[tokio::test]
async fn the_five_second_lead_of_r16race_still_reaches_the_first_worker() {
    // The round's own clock, as `r16race.json` recorded it: the ask at 18:33:11, `R1` at
    // 18:33:19, `job_start` at 18:33:24.
    const ASKED_AT_MS: i64 = 0;
    const CORRECTION_AT_MS: i64 = 8_000;
    const JOB_START_AT_MS: i64 = 13_000;
    const LEAD_MS: i64 = JOB_START_AT_MS - CORRECTION_AT_MS;
    assert_eq!(LEAD_MS, 5_000, "r16race's measured lead is five seconds");

    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    let t0 = now_ms();
    an_ask(&host, t0 + ASKED_AT_MS).await;
    let _busy = agent.test_hold_turn_for(CHAT, ASK).await;
    let r1 = "R1: correction for the race16 job BEFORE you post it: use n^3 instead of n^2, \
              only n divisible by 6, and add a column n_mod_11.";
    a_user_message(&host, "steer", r1, t0 + CORRECTION_AT_MS).await;

    let job = the_model_starts_a_job(&agent).await;

    assert_eq!(
        steers_of(&agent, &job),
        vec![r1.to_owned()],
        "a correction five seconds older than the job is still inherited onto its row"
    );
    let brief = agent
        .test_first_worker_brief(&job)
        .expect("the first worker was started");
    for constraint in ["n^3", "divisible by 6", "n_mod_11"] {
        assert!(
            brief.contains(constraint),
            "the first worker must start with all of R1, including {constraint:?} — r16's worker \
             reported \"no correction arrived\": {brief}"
        );
    }
    assert!(
        brief.starts_with(BRIEF),
        "the model's brief is still carried byte-identical ahead of the correction: {brief}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The fold is a no-op when there is nothing to fold: a job nobody corrected hands its worker the
/// brief the model wrote and not a word more. The guard on fix 1 being a quiet change to every
/// job rather than only to corrected ones.
#[tokio::test]
async fn a_job_nobody_corrected_hands_its_worker_the_brief_the_model_wrote() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    an_ask(&host, now_ms()).await;
    let _busy = agent.test_hold_turn_for(CHAT, ASK).await;

    let job = the_model_starts_a_job(&agent).await;

    assert!(steers_of(&agent, &job).is_empty());
    assert_eq!(
        agent.test_first_worker_brief(&job).as_deref(),
        Some(BRIEF),
        "byte-identical, so a job with no corrections is unchanged by any of this"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A correction to a job that exists and has not started yet. This is the window bounded by
/// another job's runtime, not by seconds, and it used to be refused with "is queued: steer a
/// running job, or start a new one" while writing nothing.
#[tokio::test]
async fn a_correction_to_a_queued_job_is_kept_rather_than_refused() {
    let (agent, root) = test_agent().await;
    let _host = agent.sync.open_room(CHAT).await.unwrap();
    let job = ulid();
    agent
        .store
        .insert_job(
            &job,
            CHAT,
            ASK,
            "Building the squares table",
            "other",
            BRIEF,
            &[],
        )
        .unwrap();
    assert_eq!(
        agent.store.job(&job).unwrap().unwrap().state,
        "queued",
        "insert_job writes `queued`, which is the state this test is about"
    );

    let out = agent
        .steer_job_in(CHAT, &job, "no, cube them, not square")
        .await
        .expect("a queued job takes the correction instead of refusing it");

    assert_eq!(out["state"], "steered");
    assert_eq!(
        steers_of(&agent, &job),
        vec!["no, cube them, not square".to_owned()],
        "the row is the carrier: there is no worker session to put it in yet"
    );
    assert!(
        out["note"]
            .as_str()
            .unwrap_or_default()
            .contains("not started yet"),
        "the model is told this is applied from the beginning, not mid-run: {out}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `steer_job("latest")` with nothing running and nothing just finished used to answer "no job is
/// running in this chat" while a job of this chat sat queued.
#[tokio::test]
async fn steer_job_latest_finds_a_queued_job_when_nothing_is_running() {
    let (agent, root) = test_agent().await;
    let _host = agent.sync.open_room(CHAT).await.unwrap();
    let job = ulid();
    agent
        .store
        .insert_job(
            &job,
            CHAT,
            ASK,
            "Building the squares table",
            "other",
            BRIEF,
            &[],
        )
        .unwrap();

    let out = agent
        .steer_job_in(CHAT, "latest", "no, cube them, not square")
        .await
        .expect("`latest` resolves the queued job");

    assert_eq!(out["job_id"], json!(job));
    assert_eq!(
        steers_of(&agent, &job),
        vec!["no, cube them, not square".to_owned()]
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The duplication guard, and the reason it reads a field instead of judging text: a message the
/// chat routed as a `send` gets a turn of its own, so inheriting it would be this job doing work
/// another turn is already doing.
#[tokio::test]
async fn a_new_ask_after_the_job_ask_is_not_inherited() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    let t0 = now_ms();
    an_ask(&host, t0).await;
    let _busy = agent.test_hold_turn_for(CHAT, ASK).await;
    a_user_message(
        &host,
        "send",
        "also, what is the capital of Peru?",
        t0 + 4_000,
    )
    .await;

    let job = the_model_starts_a_job(&agent).await;

    assert!(
        steers_of(&agent, &job).is_empty(),
        "a `send` is a new ask with its own turn, never a correction of this one"
    );
    assert_eq!(agent.test_first_worker_brief(&job).as_deref(), Some(BRIEF));
    let _ = std::fs::remove_dir_all(&root);
}

/// A correction that already has its own assistant row is answered, and answered is the one
/// notion of done this shares with `fresh_unanswered_send`. `handle_steer` falls back to a plain
/// `send` when no turn is running, so a `steer`-routed row really can have been answered on its
/// own.
#[tokio::test]
async fn a_correction_that_already_has_an_answer_is_not_inherited() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    let t0 = now_ms();
    an_ask(&host, t0).await;
    let _busy = agent.test_hold_turn_for(CHAT, ASK).await;
    let answered = a_user_message(&host, "steer", "and use tabs, not commas", t0 + 4_000).await;
    {
        let h = host.lock().await;
        h.doc
            .start_assistant_message(&ulid(), &answered, "box:test");
    }

    let job = the_model_starts_a_job(&agent).await;

    assert!(
        steers_of(&agent, &job).is_empty(),
        "an assistant row claims this message through inReplyTo, so it is not outstanding"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The bound that stops a new job inheriting the whole chat's history of corrections: a
/// correction issued before this ask belongs to whatever came before it.
#[tokio::test]
async fn a_correction_from_before_the_ask_is_not_inherited() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    let t0 = now_ms();
    a_user_message(&host, "steer", "only the ones under $70", t0 - 60_000).await;
    an_ask(&host, t0).await;
    let _busy = agent.test_hold_turn_for(CHAT, ASK).await;

    let job = the_model_starts_a_job(&agent).await;

    assert!(
        steers_of(&agent, &job).is_empty(),
        "a correction older than the ask was aimed at earlier work, not at this job"
    );
    let _ = std::fs::remove_dir_all(&root);
}
