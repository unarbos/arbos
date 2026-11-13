//! Bug W, Test5 round 7 (r7note, agentd 0.1.193): a three-step pipeline job was asked to post a
//! one-line progress note in its chat after each step. Its brief told it it could not post, so it
//! created three one-shot routines with `in_seconds: 1` instead. They ran in the project's main
//! chat `01M6KTFT1XBKB5GBA89NBKVWRC` rather than the job's, after the job's own final row, and
//! out of order — step 3 at 23:14:57, step 1 at 23:15:02, step 2 at 23:15:06, with the job's
//! final already in the side chat at 23:14:53.
//!
//! This is that scenario end to end with the fix in: the notes land in the job's own chat, in
//! step order, before the final, and the routine the job would have reached for is refused.
//!
//! Which assertions do what, because they are not equal, and each of these was measured by
//! breaking the named code and watching this test go red:
//!
//! - the caller's chat is the job's chat → fails if `mcp::caller_for` reads the running turn for
//!   a worker instead of the job's row;
//! - the routine is refused → fails if the sub-60s rule goes;
//! - three notes, in step order, before the final, and nothing in the main chat → these are the
//!   scenario's outcome, and they hold up the posting path (`post_to_job_chat`) rather than
//!   anything this change touches. They stay green if the caller's-chat default in
//!   `routines::create_body` is removed; that default is held by the unit test in `routines.rs`;
//! - the notes are 1.5s apart → fails against no production change at all. See the comment on
//!   that assertion before deciding it is proving something.

use serde_json::json;

use crate::mcp::{caller_for, SessionRole};
use crate::sync::SharedHost;

/// One step of the pipeline. The r7note run used 30 seconds per step; this is 2, which is a
/// deliberate scaling rather than a drift from the scenario — **please do not "restore" 30**.
///
/// Nothing is lost by it. The three assertions that discriminate against the fix — own chat,
/// order before the final, no routine — do not depend on how long a step takes, and each was
/// re-checked against its targeted break at this timing. The spacing assertion below
/// discriminates against nothing either way. So 30 seconds bought no proving power and cost 91
/// seconds of every future run of this crate's suite, against about 31 seconds for the other
/// 355 tests put together.
///
/// It has to be real sleeping. `arbos_core::ids::now_ms` is `chrono::Utc::now()` with no test
/// seam, so `#[tokio::test(start_paused = true)]` would make these sleeps instant while leaving
/// the recorded timestamps below unmoved — the spacing assertion would then fail for a reason
/// that has nothing to do with the code. Scale both constants together, or neither.
const STEP: std::time::Duration = std::time::Duration::from_secs(2);

/// How far apart consecutive notes must sit. Paced by the work, not batched at the end. Held a
/// little under `STEP` so a slow scheduler cannot fail it.
const MIN_GAP_MS: i64 = 1_500;

const CHAT: &str = "01TESTCHATJOBNOTEOWNCHAT01";
/// Where W's three routines actually landed. Nothing may reach it here.
const MAIN_CHAT: &str = "01TESTCHATPROJECTMAINW0001";
const ASK: &str = "01TESTASKJOBNOTEOWNCHAT001";
const JOB: &str = "01TESTJOBNOTEOWNCHAT000001";
const WORKER: &str = "01TESTWORKERJOBNOTEOWN0001";

/// The words a reader of the chat can see, in the order they sit in the transcript.
fn visible(host: &SharedHost) -> Vec<String> {
    let h = host.try_lock().expect("host free");
    h.doc
        .messages()
        .into_iter()
        .filter(|m| m.role == "assistant" && m.has_visible_content())
        .map(|m| m.text())
        .collect()
}

#[tokio::test]
async fn a_three_step_job_posts_one_note_per_step_in_its_own_chat_and_creates_no_routine() {
    let (agent, root) = crate::status::tests::test_agent().await;

    let host = agent.sync.open_room(CHAT).await.unwrap();
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(
                ASK,
                "Run the three-step pipeline and post a one-line progress note in this chat after each step.",
                "dev-mac",
                &Default::default(),
            )
            .unwrap();
    }
    // Opened only so this test can prove it stayed empty.
    let main_host = agent.sync.open_room(MAIN_CHAT).await.unwrap();

    agent
        .store
        .insert_subagent(
            WORKER,
            "job-three-step-pipeline-0001",
            "run the pipeline",
            "pi",
            CHAT,
        )
        .unwrap();
    agent
        .store
        .insert_job(
            JOB,
            CHAT,
            ASK,
            "Running the pipeline",
            "ops",
            "Run the three steps and post a one-line note after each.",
            &[],
        )
        .unwrap();
    agent.store.set_job_subagent(JOB, WORKER).unwrap();

    // A job runs beside the chat: the turn that started it ended long before.
    assert!(
        agent.turn.lock().await.is_none(),
        "there is no turn for a job to post into"
    );

    let notes = [
        "Step 1 of 3 done: 1,200 rows parsed.",
        "Step 2 of 3 done: 1,200 rows normalised, 4 rejected.",
        "Step 3 of 3 done: the summary table is written.",
    ];
    for note in notes {
        tokio::time::sleep(STEP).await; // the step's own work
        let answer = agent
            .post_to_job_chat(WORKER, note)
            .await
            .expect("a job posts its progress notes in the chat it runs beside");
        assert!(answer.contains("\"status\":\"posted\""), "{answer}");
    }

    // The job's result lands after them, as its own row.
    let result = "Pipeline finished: 1,196 of 1,200 rows in summary.csv.";
    agent
        .post_chat_line(&host, CHAT, ASK, &format!("job:{JOB}:result"), result)
        .await;

    // Three notes, each its own row, in step order, all before the final. W delivered them
    // after the final and in the order 3, 1, 2.
    assert_eq!(
        visible(&host),
        vec![
            notes[0].to_owned(),
            notes[1].to_owned(),
            notes[2].to_owned(),
            result.to_owned(),
        ],
        "three notes in step order, then the result"
    );

    // In the job's own chat. W put all three in the project's main chat, which the user was not
    // watching.
    assert!(
        visible(&main_host).is_empty(),
        "a note reached the project's main chat: {:?}",
        visible(&main_host)
    );

    // The notes are paced by the work rather than batched at the end: W's three fired within 9
    // seconds of each other, after the job had already finished.
    //
    // This assertion is scenario fidelity, not discrimination. No production change can force
    // pacing — the worker's script decides when to call, and here this test is that script — so
    // reverting any part of the fix leaves it green. It is here to keep the scenario honest: a
    // later reader can see that these really are per-step notes and not three calls in a row.
    let at: Vec<i64> = agent
        .store
        .t3_thread_in_order(CHAT, 100)
        .unwrap()
        .into_iter()
        .filter(|(r, _)| r.kind == "job_note")
        .map(|(r, _)| r.at)
        .collect();
    assert_eq!(at.len(), 3, "three job_note rows, one per step");
    for pair in at.windows(2) {
        assert!(
            pair[1] - pair[0] >= MIN_GAP_MS,
            "consecutive notes are {}ms apart, under the {MIN_GAP_MS}ms a per-step note implies",
            pair[1] - pair[0]
        );
    }

    // Meanwhile the user asks the main agent something else, so a turn is now live in another
    // chat while this job runs. That is the ordinary case, and it is the one that makes the role
    // split load-bearing: `Agent::turn` is the box's single main-agent turn, so a worker that
    // read it would have its routine filed against whatever chat the main agent is answering in.
    let _busy = agent.test_hold_turn(MAIN_CHAT).await;
    let caller = caller_for(&agent, SessionRole::Subagent, WORKER).await;
    assert_eq!(
        caller.chat_id.as_deref(),
        Some(CHAT),
        "a routine from this job belongs to the job's chat, not the main agent's running turn"
    );
    assert_eq!(
        caller.job_id.as_deref(),
        Some(JOB),
        "the caller must be known as a job, or the rule below cannot apply to it"
    );

    // And no routine behind any of it. This is the call the r7note job actually made, refused —
    // and refused before the relay is reached, which is what the absence of an HTTP error shows:
    // this test has no relay, so a routine that got past the rule would fail on the request
    // instead of on the rule.
    let out = crate::routines::call_tool(
        &agent,
        "routines_create",
        &json!({
            "name": "STEP1",
            "prompt": "Tell the user: step 1 of 3 finished.",
            "trigger": { "kind": "once", "in_seconds": 1 },
        }),
        &caller,
    )
    .await;
    assert_eq!(out["isError"], json!(true), "{out}");
    let said = out["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        said.contains(crate::routines::NOTE_PATH),
        "the refusal must send the job to the path it has just used: {said}"
    );

    let _ = std::fs::remove_dir_all(&root);
}
