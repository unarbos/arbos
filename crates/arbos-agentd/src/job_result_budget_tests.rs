//! Bug AC, Test5 rounds 12–19: a background job's final post carries raw ```json fences instead
//! of the answer.
//!
//! The fences are not written by the talking model. They are the worker's own words, posted by
//! the fallback arm of `Agent::write_job_result`, and they contain a fence because every worker
//! pack ends with "End your final reply with one fenced ```json block" (the Return contract,
//! `agent.rs`). So the question this case asks is not "who strips the fence" but "why did the
//! fallback fire at all on finished work".
//!
//! The answer is the budget. `write_job_result` asked for 400 tokens. On an endpoint where
//! thinking cannot be turned off — Opus 5.5 answers HTTP 400 "Reasoning is mandatory for this
//! endpoint and cannot be disabled", which is the talking model Test5 ran — the hidden tokens
//! are spent first and the 400 buys no visible sentence at all. The call returns empty content
//! at `finish_reason: "length"`, the fallback posts the notes, and the notes carry the fence.
//! The quick lane already had the cure for this exact endpoint (`Llm::complete_visible`, #1411);
//! the job-result call did not use it.
//!
//! What makes this case bite rather than pass by luck:
//!
//! - the stub models the endpoint, not a generic failure: it refuses `reasoning {enabled:false}`
//!   AND returns empty content for any budget under 1000 tokens. A stub that always answered
//!   would pass before and after the fix and prove nothing, so the precondition below calls the
//!   old shape against it by hand and asserts it really does come back empty;
//! - the notes really do carry a fenced block, asserted before the writer runs;
//! - the posted row is checked for the model's sentence as well as for the absence of a fence. A
//!   fix that posted an empty bubble, or the "I couldn't finish that one" line, would clear the
//!   fence assertion and leave the user worse off;
//! - the warn and info lines are read back out of this test's own tracing capture, so "the
//!   fallback did not fire" is evidence rather than an inference from the text that was posted.

use serde_json::Value;

use crate::status::tests::{test_agent, Journal, AC_MODEL_ANSWER, AC_THINKING_EATS_THE_BUDGET_ASK};
use crate::sync::SharedHost;

const CHAT: &str = "01TESTCHATJOBRESULTBUDGET1";
const ASK: &str = "01TESTASKJOBRESULTBUDGET01";
const JOB: &str = "01TESTJOBRESULTBUDGETAC191";
const WORKER: &str = "01TESTWORKERRESULTBUDGET01";
const WORKER_NAME: &str = "job-count-licence-lines-ac19";

/// What the worker left behind, in the shape `run_subagent` hands to the writer: the worker's own
/// final reply — which ends in the fenced block its pack asked for — and then the parsed contract
/// appended under a plain heading.
fn worker_output() -> String {
    "Counted the licence lines across the archive. 1,204 lines in 312 files; the CSV is at /work/licences.csv.\n\n```json\n{\"status\": \"done\", \"summary\": \"1,204 licence lines across 312 files; one CSV written.\", \"artifacts\": [\"/work/licences.csv\"], \"acceptance\": [{\"criterion\": \"exactly one CSV\", \"met\": true, \"evidence\": \"only /work/licences.csv was written\"}], \"open_items\": [], \"questions\": []}\n```\n\nParsed return contract:\n{\"status\":\"done\",\"summary\":\"1,204 licence lines across 312 files; one CSV written.\",\"artifacts\":[\"/work/licences.csv\"]}".to_owned()
}

/// The one answer row of the job (`decision: "job"`) — the bubble the user reads.
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
async fn a_jobs_answer_is_written_by_the_model_even_when_the_endpoint_insists_on_thinking() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(
                ASK,
                AC_THINKING_EATS_THE_BUDGET_ASK,
                "dev-mac",
                &Default::default(),
            )
            .unwrap();
    }

    // Precondition one: the notes carry a fence. Without it nothing here is about bug AC.
    let notes = worker_output();
    assert!(
        notes.contains("```json"),
        "the worker's notes must carry the fenced block its Return contract asks for: {notes}"
    );

    // Precondition two: against this stub, the call `write_job_result` used to make really does
    // come back with nothing. This is the line that stops the case passing on a friendly stub.
    let (talking, _) = agent.live_models().await;
    let (empty, _) = agent
        .llm
        .with_purpose("job-result")
        .complete(
            &talking,
            "You answer a message the user sent while you were busy.",
            &format!("The user's message: {AC_THINKING_EATS_THE_BUDGET_ASK}"),
            400,
        )
        .await
        .expect("the endpoint answers a plain call; it just has nothing to say in 400 tokens");
    assert!(
        empty.trim().is_empty(),
        "the stub must model the mandatory-thinking endpoint: a 400-token plain call has to come \
         back empty, or this case cannot tell the old shape from the new one. Got: {empty}"
    );

    agent
        .store
        .insert_subagent(WORKER, WORKER_NAME, "count the licence lines", "pi", CHAT)
        .unwrap();
    agent
        .store
        .insert_job(
            JOB,
            CHAT,
            ASK,
            "Counting licence lines",
            "ops",
            "Count the licence lines across the archive and write exactly one CSV.",
            &[],
        )
        .unwrap();
    agent.store.set_job_subagent(JOB, WORKER).unwrap();
    agent.store.set_job_state(JOB, "done", None).unwrap();

    let journal = Journal::default();
    {
        let _capture = journal.capture_info();
        agent
            .write_job_result(CHAT, &host, ASK, WORKER_NAME, "done", &notes, None)
            .await
            .expect("the result is written");
    }
    let logged = journal.text();

    let answer = job_answer(&host);
    assert!(!answer.trim().is_empty(), "the job posted no answer at all");

    // What round 19's user read.
    assert!(
        !answer.contains("```"),
        "the job's final post carries a code fence: {answer}"
    );
    assert!(
        !answer.contains("Parsed return contract"),
        "the job's final post carries the machinery heading: {answer}"
    );

    // And it is still an answer. A fix that posted the plain apology line, or nothing, would
    // clear both assertions above.
    assert!(
        answer.contains("1,204") && answer.contains("licences.csv"),
        "the answer did not survive: {answer}"
    );
    assert_eq!(
        answer.trim(),
        AC_MODEL_ANSWER,
        "the posted row has to be the talking model's sentence, not the worker's notes"
    );

    // The evidence, read back out of what the box logged rather than guessed from the text.
    assert!(
        logged.contains("endpoint requires reasoning"),
        "the writer must notice the refusal and retry with room for the thinking: {logged}"
    );
    assert!(
        logged.contains("job result written"),
        "the writer must record a written result: {logged}"
    );
    assert!(
        !logged.contains("job result call gave nothing"),
        "the fallback fired, which is the bug: {logged}"
    );

    let _ = std::fs::remove_dir_all(&root);
}
