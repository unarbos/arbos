//! Bug AC, Test5 round 12 (image 0.1.196 / `508151f6a`, chat 01M6ME9A7EGV5PJ7DZW43AHTN5): a
//! job's own return contract was posted into the user's chat.
//!
//! At 13:20:47 CR, job 01M6PJABNVZ84YXHHFY9HV3FAK logged `job result call gave nothing; plain
//! line` — the direct model call that turns a worker's notes into the one answer bubble came
//! back empty. On that fallback `write_job_result` posted the worker's raw final text instead,
//! and a worker's final text ends with the fenced ```json block the "## Return contract"
//! section of its system prompt asks for: `status`, `summary`, `artifacts`, `acceptance`,
//! `open_items`, `proposed_memory`, `questions`. agentd's own `Parsed return contract:` note
//! was appended to it as well, so the user read the machinery twice over.
//!
//! The cases here are written at what reaches the chat, not at the helper: each one drives the
//! real `write_job_result` with the model stub answering nothing, and reads the row a client
//! would draw.
//!
//! What must *not* happen is the other half. A worker that was asked for JSON puts JSON in its
//! answer, and that block is the answer — so the rule is the contract's own shape (a `status`
//! of exactly `done`, `blocked`, `partial` or `needs_input`, plus at least one contract field)
//! at the end of the text, never "a fenced block".

use arbos_core::doc::{MessageView, UserMessageOpts};
use arbos_core::ids::ulid;
use serde_json::Value;

use crate::agent::{relay_fallback_line, TurnKind};
use crate::llm::{extract_return_contract, strip_return_contract};
use crate::status::tests::{test_agent, Journal, EMPTY_JOB_RESULT_ASK};
use crate::sync::SharedHost;

/// The human half of round 12's worker answer.
const PROSE: &str = "All eight checks passed and the report is in out/round12.md. \
The two sites that answered 403 are named in it.";

/// The return contract that followed it, in the shape the worker prompt asks for.
const CONTRACT: &str = r#"```json
{ "status": "done",
  "summary": "Eight checks passed; two sites answered 403.",
  "artifacts": [ { "kind": "file", "ref": "out/round12.md", "note": "the report" } ],
  "acceptance": [ { "criterion": "every site checked", "met": true, "evidence": "out/round12.md" } ],
  "open_items": [],
  "proposed_memory": [],
  "questions": [] }
```"#;

/// The worker's final text as the harness hands it over, and then as `run_subagent` passes it to
/// `write_job_result`: with agentd's own parsed-contract note appended.
fn worker_final_text() -> (String, String) {
    let text = format!("{PROSE}\n\n{CONTRACT}");
    let contract = extract_return_contract(&text).expect("the fixture is a real return contract");
    let raw = format!("{text}\n\nParsed return contract:\n{contract}");
    (text, raw)
}

/// The rows of `msg_id` a client would draw (`hidden` rows are drawn by neither app).
fn visible_replies(host: &SharedHost, msg_id: &str) -> Vec<MessageView> {
    let h = host.try_lock().expect("host free");
    h.doc
        .messages()
        .into_iter()
        .filter(|m| {
            m.role == "assistant"
                && m.rest.get("inReplyTo").and_then(Value::as_str) == Some(msg_id)
                && m.rest.get("hidden").and_then(Value::as_bool) != Some(true)
                && !m.text().trim().is_empty()
        })
        .collect()
}

/// Run a job's result through the fallback path and return what the chat got, with the box's own
/// warnings. The ask carries [`EMPTY_JOB_RESULT_ASK`], which is what makes the model stub answer
/// the job-result call with nothing.
async fn posted_on_the_empty_summary_path(chat: &str, raw: &str) -> (String, String) {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(chat).await.unwrap();
    let ask = ulid();
    host.lock()
        .await
        .doc
        .append_user_message(
            &ask,
            EMPTY_JOB_RESULT_ASK,
            "dev-mac",
            &UserMessageOpts::default(),
        )
        .unwrap();
    let journal = Journal::default();
    {
        let _capture = journal.capture();
        agent
            .write_job_result(chat, &host, &ask, "job-round-12-ab12", "done", raw, None)
            .await
            .expect("the result is written");
    }
    let rows = visible_replies(&host, &ask);
    assert_eq!(
        rows.len(),
        1,
        "one answer bubble for the ask: {:?}",
        rows.iter().map(|m| m.text()).collect::<Vec<_>>()
    );
    let posted = rows[0].text();
    let _ = std::fs::remove_dir_all(&root);
    (posted, journal.text())
}

/// Bug AC itself: the summary call gives nothing, and what lands in the chat is the worker's
/// words without its return contract.
#[tokio::test]
async fn a_job_result_fallback_posts_the_prose_and_none_of_the_return_contract() {
    let (_, raw) = worker_final_text();
    let (posted, logged) =
        posted_on_the_empty_summary_path("01TESTCHATHANDOFFLEAK00001", &raw).await;

    assert!(
        logged.contains("job result call gave nothing"),
        "this case has to go through the fallback path, not the model one: {logged}"
    );
    assert!(
        posted.contains("All eight checks passed"),
        "the worker's own words are the answer: {posted}"
    );
    for machinery in [
        "```json",
        "\"acceptance\"",
        "\"artifacts\"",
        "\"proposed_memory\"",
        "\"open_items\"",
        "Parsed return contract",
    ] {
        assert!(
            !posted.contains(machinery),
            "the chat got the job's machinery ({machinery}): {posted}"
        );
    }
}

/// A worker whose answer *is* JSON (the user asked for the response body) keeps it. The strip is
/// the contract's shape, not every fenced block.
#[tokio::test]
async fn a_fenced_json_answer_the_user_asked_for_is_not_stripped() {
    let raw = "Here is what the endpoint returned:\n\n```json\n{ \"status\": \"ok\", \"data\": { \"id\": 7 } }\n```";
    let (posted, logged) =
        posted_on_the_empty_summary_path("01TESTCHATHANDOFFLEAK00002", raw).await;

    assert!(logged.contains("job result call gave nothing"), "{logged}");
    assert!(
        posted.contains("```json") && posted.contains("\"data\""),
        "a JSON answer the user asked for must survive the fallback: {posted}"
    );
}

/// The other path that posts a worker's final text verbatim: the notice agentd writes when the
/// relay turn could not tell the user itself (`relay_fallback_line` → `job_notice_line`). Its
/// result is the contract's `summary` when the contract parsed — and the worker's whole final
/// text when it did not, which is the same leak.
#[test]
fn the_notice_agentd_writes_itself_carries_no_return_contract() {
    let (text, _) = worker_final_text();
    let line = relay_fallback_line(&TurnKind::SubagentResult {
        subagent_id: "01TESTSUBAGENTHANDOFFLEAK1".into(),
        name: "job:round-12".into(),
        job_id: Some("01M6PJABNVZ84YXHHFY9HV3FAK".into()),
        status: "done".into(),
        title: "Checking the eight sites".into(),
        result: text,
    });
    assert!(line.contains("All eight checks passed"), "{line}");
    assert!(
        !line.contains("```json") && !line.contains("\"acceptance\""),
        "the notice carried the return contract: {line}"
    );
}

/// The rule, stated as its own cases: the contract block at the end of a reply goes, and
/// anything else stays.
#[test]
fn only_a_trailing_return_contract_is_stripped() {
    let (text, _) = worker_final_text();
    assert_eq!(strip_return_contract(&text), PROSE);

    // No fence, nothing to do.
    assert_eq!(strip_return_contract(PROSE), PROSE);

    // A fenced block whose `status` is not one of the contract's four, and which carries none of
    // the contract's fields: an answer, not machinery.
    let answer = "The endpoint says:\n\n```json\n{ \"status\": \"ok\", \"data\": 1 }\n```";
    assert_eq!(strip_return_contract(answer), answer);

    // Contract-shaped, but with the worker's words after it: not the trailing contract, so it
    // stays. The narrowest rule that fixes round 12 — a block in the middle of a reply cannot be
    // told from one a worker quoted on purpose.
    let quoted = format!("{CONTRACT}\n\nThat is the shape I was asked for.");
    assert_eq!(strip_return_contract(&quoted), quoted);

    // A contract whose JSON does not parse is not removed either; it is not a contract to
    // `extract_return_contract` either, so both halves of agentd see the same block.
    let broken = format!("{PROSE}\n\n```json\n{{ \"status\": \"done\", \n```");
    assert_eq!(strip_return_contract(&broken), broken);
}
