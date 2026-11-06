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
//!
//! ## The second fault on the same path (round 13)
//!
//! Round 13 (2026-11-05, same image) reproduced it twice — notes13 at 14:45:20 and status13c at
//! 14:59:30 — and the notes13 bubble opened "It's listed. Step 1.", which is not a stripping
//! failure at all. Same trigger, same line, a different fault: the fallback ran the worker's
//! report through `ProseFilter::clean_prose`, which is written for the *talking model's* own
//! drafts. Its thought-prefix stripper treats "The user asked …" as the model thinking aloud and
//! drops it, and its trailing-narration stripper treats "Checking the two remaining notes is
//! still open." as tool narration and drops that too. A worker writing to the main agent uses
//! exactly that register, so both ends of its report were deleted — and removing the JSON alone
//! would have left the user with the broken sentence and nothing else.
//!
//! So the fallback no longer filters a worker's prose at all. It strips the contract, cuts for a
//! chat row and normalises tables, which is what `job_notice_line` — the other path that posts a
//! worker's words — has always done. `clean_prose` stays on the success branch, where the text
//! really is the talking model's.

use arbos_core::doc::{MessageView, UserMessageOpts};
use arbos_core::ids::ulid;
use serde_json::Value;

use crate::agent::{relay_fallback_line, TurnKind};
use crate::llm::{extract_return_contract, strip_return_contract};
use crate::status::tests::{test_agent, Journal, EMPTY_JOB_RESULT_ASK, GLUED_TABLE_JOB_RESULT_ASK};
use crate::sync::SharedHost;

/// The human half of round 12's worker answer.
const PROSE: &str = "All eight checks passed and the report is in out/round12.md. \
The two sites that answered 403 are named in it.";

/// Round 13's notes13 answer, in the register a worker writes to the main agent in: it names
/// what was asked, answers it, numbers the steps and ends with what is still open.
///
/// Every sentence here is the worker's own content. `clean_prose` deletes the first (it starts
/// "The user asked") and the last (it starts "Checking "), which is how the bubble came to open
/// at "It's listed. Step 1." — the two assertions below are those two sentences.
const R13_NOTES: &str = "The user asked whether the note is in the list. It's listed. \
Step 1. Open the notes file and read the third line. \
Step 2. The entry is dated 4 November. \
Checking the two remaining notes is still open.";

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
    posted_for_ask(chat, EMPTY_JOB_RESULT_ASK, raw).await
}

/// What the chat got for `ask`, with the box's own warnings. The ask text is what the model stub
/// answers from, so it chooses which branch of `write_job_result` the case drives.
async fn posted_for_ask(chat: &str, ask_text: &str, raw: &str) -> (String, String) {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(chat).await.unwrap();
    let ask = ulid();
    host.lock()
        .await
        .doc
        .append_user_message(&ask, ask_text, "dev-mac", &UserMessageOpts::default())
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

/// Round 13's second symptom, driven from the same trigger: the summary call returns nothing, and
/// what lands in the chat is the worker's report with **both ends still on it**.
///
/// Red before the fix with the exact string Jacob quoted: the bubble opened "It's listed. Step 1."
#[tokio::test]
async fn a_job_result_fallback_keeps_the_workers_first_and_last_sentence() {
    let raw = format!("{R13_NOTES}\n\n{CONTRACT}");
    let (posted, logged) =
        posted_on_the_empty_summary_path("01TESTCHATHANDOFFLEAK00003", &raw).await;

    assert!(logged.contains("job result call gave nothing"), "{logged}");
    assert!(
        !posted.starts_with("It's listed."),
        "round 13's garbled opener, byte for byte: {posted}"
    );
    assert!(
        posted.starts_with("The user asked whether the note is in the list."),
        "the worker's opening sentence was eaten as thought-aloud: {posted}"
    );
    assert!(
        posted.contains("Checking the two remaining notes is still open."),
        "the worker's open item was eaten as tool narration: {posted}"
    );
    // And still no contract: the two faults are fixed together or not at all.
    assert!(!posted.contains("```json"), "{posted}");
}

/// Round 14's r14notes, driven through the **success** branch: the summary call answers, and what
/// lands in the chat is the whole answer rather than its last line.
///
/// Red before the fix with r14notes' chat final, byte for byte: the row was
/// "It ran as a real background job, not inline." and the table was gone.
///
/// The cost of taking `clean_prose` off this branch is visible in the second assertion: a draft
/// that opens in the model's thought register now ships with that sentence. That is the rule this
/// path is held to — a prose filter is for a streamed turn, not for a job's result — and it is
/// what the fallback branch has done since round 13, so the two branches of one function no longer
/// disagree about whether a job's answer gets filtered.
#[tokio::test]
async fn a_job_results_table_survives_an_opener_the_prose_filter_would_eat() {
    let (posted, logged) = posted_for_ask(
        "01TESTCHATHANDOFFLEAK00004",
        GLUED_TABLE_JOB_RESULT_ASK,
        PROSE,
    )
    .await;

    // The fallback cannot run without logging that warning, so its absence is what says this case
    // took the model branch (the journal captures at WARN, and the success log is info).
    assert!(
        !logged.contains("job result call gave nothing"),
        "this case is the model path, not the fallback: {logged}"
    );
    assert_ne!(
        posted.trim(),
        "It ran as a real background job, not inline.",
        "round 14's one-line chat final, byte for byte"
    );
    for row in [
        "| step | value |",
        "| 1 | 32000 |",
        "| 4 | 1365346133360000 |",
    ] {
        assert!(
            posted.contains(row),
            "the answer's table was eaten with its opening sentence ({row}): {posted}"
        );
    }
    assert!(
        posted.contains("It ran as a real background job, not inline."),
        "the closing line is part of the answer too: {posted}"
    );
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
