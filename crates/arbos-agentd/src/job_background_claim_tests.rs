//! Bug AD, Test5 round 12 (2026-11-05, image 0.1.196, seed 508151f6a, chat
//! `01M6ME9A7EGV5PJ7DZW43AHTN5`): the final post of a real background job told the user "the four
//! steps ran in the foreground, not as a background job". It was a job — started through
//! `start_job`, job `01M6PHP14MPAGBJE83VKXBHPNS`, four `job_notes` posted in the chat — and the
//! agent had answered the same question correctly a minute earlier, at 13:07:23, with "still
//! running in the background".
//!
//! How the sentence got written, end to end:
//!
//! 1. the `brief` parameter of `start_job` is described to the main model as "never tell the
//!    worker to detach anything … its steps run in the foreground" (`harness/arbos-pi-ext/index.ts`,
//!    and `prompts/router.md`), so the model writes that into the brief it sends;
//! 2. `Agent::build_pack` puts the brief verbatim at the top of the worker's prompt and appends
//!    the Return contract, which asks the worker for `acceptance: [{criterion, met, evidence}]`.
//!    Nothing in agentd builds or scores that list — the worker model writes it, from the brief's
//!    own words — and it read the brief's "foreground" instruction as a criterion about
//!    backgrounding and scored it unmet;
//! 3. `run_subagent` appends the parsed contract to the worker's text and hands the pair to
//!    `Agent::write_job_result` as `raw`, which passes it to the talking model as "Notes from the
//!    work". That model is told nothing about the job, so the unmet criterion was the only thing
//!    in its context that spoke to backgrounding, and it restated it as fact.
//!
//! The job's backgrounded-ness is not something to infer: the ask has a job row, with an id and
//! its notes. So this case drives the real writer with round 12's notes and a model stub that
//! answers with round 12's sentence, and asks what the user is left holding.
//!
//! Each assertion and what breaks it:
//!
//! - no denial in the row → fails if the deterministic strip in `job_backgrounded.rs` goes, or if
//!   `write_job_result` stops calling it;
//! - the answer survives → fails if the strip takes the sentences that carry the result, which is
//!   the way a filter like this goes wrong;
//! - the writer was told the fact → fails if the job lookup in `write_job_result` goes, which is
//!   the half of the fix that stops the inference rather than catching its words.

use serde_json::Value;

use crate::status::tests::{test_agent, AD_MODEL_ANSWER, AD_UNMET_BACKGROUND_CRITERION};
use crate::sync::SharedHost;

const CHAT: &str = "01TESTCHATJOBBACKGROUNDAD1";
const ASK: &str = "01TESTASKJOBBACKGROUNDAD01";
const JOB: &str = "01TESTJOBBACKGROUNDADR12I1";
const WORKER: &str = "01TESTWORKERBACKGROUNDAD01";
const WORKER_NAME: &str = "job-four-step-pipeline-ad01";

/// Round 12's ask. It forces a job in words (`forced_job::forces_a_job`), which is the path whose
/// result `write_job_result` writes.
const USER_ASK: &str = "Run the four-step pipeline over the archive and report the row counts, as a background job (not inline).";

/// The brief as the main model wrote it, carrying the instruction about how the *steps* run that
/// the `start_job` tool description asks for.
const BRIEF: &str = "Run the four-step pipeline over /work/archive and report the row counts for each step. Run each step in the foreground, do not detach anything (no nohup, setsid or trailing &). Post a one-line note after each step. Return the four counts and the path of the summary file.";

/// What the worker left behind: its final text, then the contract `run_subagent` parses and
/// appends to it. The third criterion is the one round 12 turned into a sentence about the user's
/// own work.
fn worker_output() -> String {
    format!(
        "Step 4 of 4 done. 1,200 rows parsed, 1,196 normalised, summary.csv written.\n\nParsed return contract:\n{{\"status\": \"done\", \"summary\": \"The four-step pipeline ran over the archive: 1,200 rows parsed, 1,196 normalised, summary.csv written.\", \"acceptance\": [{{\"criterion\": \"all four steps run over the archive\", \"met\": true, \"evidence\": \"steps 1-4 each reported a count\"}}, {{\"criterion\": \"each step runs in the foreground, nothing detached\", \"met\": true, \"evidence\": \"no nohup, setsid or trailing & in any step\"}}, {{{AD_UNMET_BACKGROUND_CRITERION}, \"evidence\": \"the four steps ran in the foreground, not as a background job\"}}]}}"
    )
}

/// The words a reader of the chat can see, in transcript order.
fn visible(host: &SharedHost) -> Vec<String> {
    let h = host.try_lock().expect("host free");
    h.doc
        .messages()
        .into_iter()
        .filter(|m| m.role == "assistant" && m.has_visible_content())
        .map(|m| m.text())
        .collect()
}

/// The one answer row of the job (`decision: "job"`), which is the post round 12's sentence was
/// in.
fn job_answer(host: &SharedHost) -> String {
    let h = host.try_lock().expect("host free");
    h.doc
        .messages()
        .into_iter()
        .filter(|m| m.rest.get("decision").and_then(Value::as_str) == Some("job"))
        .map(|m| m.text())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn a_real_jobs_final_post_never_says_it_was_not_a_background_job() {
    let (agent, _root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(ASK, USER_ASK, "dev-mac", &Default::default())
            .unwrap();
    }
    assert!(
        crate::forced_job::forces_a_job(USER_ASK),
        "the ask must force a job, or this is not the path that wrote round 12's post"
    );

    agent
        .store
        .insert_subagent(WORKER, WORKER_NAME, "run the pipeline", "pi", CHAT)
        .unwrap();
    agent
        .store
        .insert_job(JOB, CHAT, ASK, "Running the pipeline", "ops", BRIEF, &[])
        .unwrap();
    agent.store.set_job_subagent(JOB, WORKER).unwrap();

    // The four notes round 12's job posted. They are the second half of what the box knows for
    // certain about this work: a job row with an id, and notes in the chat it ran beside.
    for n in 1..=4 {
        agent
            .post_to_job_chat(WORKER, &format!("Step {n} of 4 done."))
            .await
            .expect("a job posts its progress notes in its chat");
    }
    agent.store.set_job_state(JOB, "done", None).unwrap();

    agent
        .write_job_result(
            CHAT,
            &host,
            ASK,
            WORKER_NAME,
            "done",
            &worker_output(),
            None,
        )
        .await
        .expect("the result is written");

    let answer = job_answer(&host);
    assert!(!answer.trim().is_empty(), "the job posted no answer at all");

    // What round 12's user read. Whatever the talking model writes, this row may not carry it:
    // the work ran as a background job, and the box holds the row that proves it.
    for claim in [
        "not as a background job",
        "not a background job",
        "did not run as a background job",
        "rather than as a background job",
        "instead of as a background job",
        "not in the background",
    ] {
        assert!(
            !answer.to_ascii_lowercase().contains(claim),
            "the job's final post tells the user \"{claim}\": {answer}"
        );
    }

    assert!(
        !crate::job_backgrounded::denies_backgrounding(&answer),
        "the job's final post denies its own backgrounding: {answer}"
    );

    // And the answer is still an answer. A strip that took the result along with the denial would
    // pass every assertion above and leave the user worse off than round 12 did.
    assert!(
        answer.contains("1,200") && answer.contains("summary.csv"),
        "the result did not survive: {answer}"
    );
    assert!(
        AD_MODEL_ANSWER.contains("not as a background job"),
        "the stub stopped writing round 12's sentence, so this case proves nothing"
    );

    // Four notes, then the answer: the notes are the scenario's own precondition, and nothing may
    // have eaten them.
    let seen = visible(&host);
    assert_eq!(seen.len(), 5, "four notes and one answer: {seen:?}");

    // The writer no longer has to work any of this out. This is the half of the fix that stops
    // the category error at its source rather than catching its words.
    let told = agent
        .job_backgrounding_note(CHAT, ASK)
        .expect("the writer is told what the box records about this ask");
    assert!(told.contains(JOB), "the note must name the job: {told}");
    assert!(
        told.contains("4 progress notes"),
        "the note must name the notes the job posted: {told}"
    );
    assert_eq!(
        agent.job_backgrounding_note(CHAT, "01TESTASKWITHNOJOBATALL001"),
        None,
        "an ask with no job row is told nothing about backgrounding"
    );
}
