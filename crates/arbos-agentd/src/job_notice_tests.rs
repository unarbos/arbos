//! The notice agentd writes itself when a job finishes and the model cannot relay the result:
//! it names the job, it carries the job's own answer, and it is never ordered between a user's
//! message and that message's first assistant row.
//!
//! Jacob on Test5, round 3 (2026-11-04, agentd 0.1.192, chat 01M6M0G9257SCE4RRQDW2GXFS9), bug
//! M. He sent a research task at 16:44:27. At 16:44:31 — before the acknowledgement of that
//! task, which landed at 16:44:36 — the chat showed "The background work I started has
//! finished. Ask me for the details whenever you like." It was the round-2 Flask job finishing.
//! Three things were wrong with those five seconds: the notice named no job (two were in
//! flight), it carried none of what the job had produced, and it sat between his message and
//! the reply to it.

use arbos_core::doc::{ChatDoc, CommandStatus, MessageStatus, UserMessageOpts};
use serde_json::json;

use crate::agent::{relay_fallback_line, TurnKind, TurnOutcome, RELAY_SLOT_LOST};
use crate::status::tests::test_agent;
use crate::sync::SharedHost;
use crate::truncate::CHAT_NOTICE_CHARS;

/// The error a seam turn ended with. `TurnOutcome` is not `Debug`, so `expect_err` is out.
fn turn_err(r: anyhow::Result<TurnOutcome>, expected: &str) -> String {
    match r {
        Ok(_) => panic!("the turn ran: {expected}"),
        Err(e) => e.to_string(),
    }
}

/// The `TurnKind` a finished job hands to its relay turn.
fn finished_job(title: &str, status: &str, result: &str) -> TurnKind {
    TurnKind::SubagentResult {
        subagent_id: "01TESTSUBAGENTJOBNOTICE001".into(),
        name: "job:flask".into(),
        job_id: Some("01TESTJOBNOTICEJOB00000001".into()),
        status: status.into(),
        title: title.into(),
        result: result.into(),
    }
}

/// A user message with its `send` command, both in the doc, as the apps write them.
fn user_message(doc: &ChatDoc, msg_id: &str, text: &str) -> String {
    doc.append_user_message(msg_id, text, "dev-mac", &UserMessageOpts::default())
        .expect("user message");
    doc.push_command("send", json!({ "messageId": msg_id }), "dev-mac")
}

/// Every message id in the doc, in the order the apps render them.
fn row_ids(host: &SharedHost) -> Vec<String> {
    let h = host.try_lock().expect("host free");
    h.doc.messages().into_iter().map(|m| m.id).collect()
}

fn assistant_rows(host: &SharedHost) -> Vec<String> {
    let h = host.try_lock().expect("host free");
    h.doc
        .messages()
        .into_iter()
        .filter(|m| m.role == "assistant")
        .map(|m| m.id)
        .collect()
}

/// Defect 1: the notice named nothing. Jacob had the Flask job and the pandas job running, and
/// "the background work I started" told him which had finished only by luck.
#[test]
fn a_job_notice_names_the_job_it_is_about() {
    let line = relay_fallback_line(&finished_job(
        "Flask app, curl check and pytest",
        "done",
        "8 tests passed and the app answers on :5000.",
    ));
    assert!(
        line.contains("Flask app, curl check and pytest"),
        "the notice must name the job it is about: {line}"
    );
    assert!(
        !line.contains("The background work I started"),
        "and must not fall back to naming nothing: {line}"
    );

    // A hand-off with no job title still names itself by its label rather than by nothing.
    let unnamed = relay_fallback_line(&finished_job("", "done", "done"));
    assert!(
        unnamed.contains("The background work I started"),
        "{unnamed}"
    );
}

/// Defect 2: the notice carried no result — "ask me for the details whenever you like" put the
/// work back on the user. A job result can also be far longer than a chat bubble, so a long one
/// is cut; what it must never be is cut in silence.
#[test]
fn a_job_notice_carries_the_result_and_marks_what_it_cut() {
    let short = relay_fallback_line(&finished_job(
        "Flask app, curl check and pytest",
        "done",
        "8 tests passed. The zip is at out/taxi_app.zip.",
    ));
    assert!(
        short.contains("8 tests passed") && short.contains("out/taxi_app.zip"),
        "a result this size goes into the notice whole: {short}"
    );
    assert!(
        !short.contains("Ask me for the details whenever you like"),
        "the result is in the notice, so there is nothing to ask for: {short}"
    );

    // A long result: the notice stays a chat row, the cut is announced with both lengths, and
    // the user is told how to get the rest.
    let long = format!(
        "The migration moved 45 tables.\n{}\nthe-very-last-line-of-the-report",
        "  - one table, with the row counts before and after\n".repeat(400)
    );
    let total = long.chars().count();
    assert!(total > 20_000, "the fixture must be far too long: {total}");
    let cut = relay_fallback_line(&finished_job("Migrate the rows", "done", &long));
    assert!(
        cut.contains("Migrate the rows") && cut.contains("The migration moved 45 tables."),
        "the head of the result is still there: {cut}"
    );
    assert!(
        !cut.contains("the-very-last-line-of-the-report"),
        "a result this long does not fit in a row"
    );
    assert!(
        cut.contains(&format!("of {total} characters")) && cut.contains("Ask me for the rest"),
        "a cut result says how much was cut and how to get it: {}",
        &cut[cut.len().saturating_sub(200)..]
    );
    assert!(
        cut.chars().count() < CHAT_NOTICE_CHARS + 400,
        "the notice is a chat row, not the report: {} chars",
        cut.chars().count()
    );
}

/// Defect 3, and the one that matters: the notice was inserted at a point in the row order
/// where the new message's own reply did not exist yet, so it read as the answer to that
/// message. The gate is in `drive_turn`, inside the one doc lock that creates the relay row, so
/// there is no instant at which a message can be in the doc and its reply row not yet be.
#[tokio::test]
async fn a_job_notice_never_opens_its_row_before_a_new_message_is_answered() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATJOBNOTICE0000001";
    let host = agent.sync.open_room(chat).await.unwrap();
    // No pi: the turn fails at its first model call, which is past the row under test.
    agent
        .shutting_down
        .store(true, std::sync::atomic::Ordering::SeqCst);

    let (ask, new_send) = {
        let h = host.lock().await;
        let ask = "01TESTJOBNOTICEASK00000001";
        user_message(&h.doc, ask, "build the flask app and run the tests");
        h.doc
            .start_assistant_message("01TESTJOBNOTICEACK00000001", ask, "box:test");
        h.doc
            .finish_message("01TESTJOBNOTICEACK00000001", MessageStatus::Done);
        // 16:44:27: the next task arrives. Its `send` command is in the doc; nothing has
        // claimed it yet, so the turn that will acknowledge it has not started.
        let new_send = user_message(&h.doc, "01TESTJOBNOTICENEW00000001", "now the WDI pipeline");
        (ask.to_owned(), new_send)
    };
    let rows_before = row_ids(&host);

    // 16:44:31: the Flask job finishes and wants to say so.
    let kind = finished_job(
        "Flask app, curl check and pytest",
        "done",
        "8 tests passed.",
    );
    let err = turn_err(
        agent.test_drive_turn(chat, &host, &ask, kind.clone()).await,
        "the notice must not take this slot",
    );
    assert_eq!(
        err, RELAY_SLOT_LOST,
        "a job notice yields to a message that has not been answered yet"
    );
    assert_eq!(
        row_ids(&host),
        rows_before,
        "and it leaves no row behind: nothing may sit between the message and its reply"
    );

    // Same answer while a send is being handled, which is the case the doc cannot show: the
    // command is claimed, its turn is queued for the turn lock, and no row exists for it yet.
    {
        let h = host.lock().await;
        h.doc
            .set_command_status(&new_send, CommandStatus::Taken, None);
    }
    let in_flight = agent.test_send_in_flight();
    let err = turn_err(
        agent.test_drive_turn(chat, &host, &ask, kind.clone()).await,
        "a claimed send holds the slot too",
    );
    assert_eq!(err, RELAY_SLOT_LOST);
    assert_eq!(row_ids(&host), rows_before, "still no row");
    drop(in_flight);

    // 16:44:36: the new message gets its reply row. Now the notice may land — behind it.
    {
        let h = host.lock().await;
        h.doc.start_assistant_message(
            "01TESTJOBNOTICEACK00000002",
            "01TESTJOBNOTICENEW00000001",
            "box:test",
        );
        h.doc
            .set_command_status(&new_send, CommandStatus::Done, None);
    }
    let err = turn_err(
        agent.test_drive_turn(chat, &host, &ask, kind).await,
        "there is no pi in a test box",
    );
    assert_ne!(
        err, RELAY_SLOT_LOST,
        "with every message answered the notice runs"
    );
    let rows = row_ids(&host);
    let notice = assistant_rows(&host)
        .into_iter()
        .find(|id| !rows_before.contains(id) && id != "01TESTJOBNOTICEACK00000002")
        .expect("the notice opened a row");
    let at = |id: &str| rows.iter().position(|r| r == id).expect("row in the doc");
    assert!(
        at("01TESTJOBNOTICENEW00000001") < at("01TESTJOBNOTICEACK00000002")
            && at("01TESTJOBNOTICEACK00000002") < at(&notice),
        "the notice comes after the reply to the newest message, never between the two: {rows:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// The notice carries whatever the worker wrote, and `partial` is a finished return contract,
/// not a failure (Jacob, Test5, 2026-11-04).
#[test]
fn a_partial_job_reads_as_finished_and_a_failed_one_does_not() {
    let partial = relay_fallback_line(&finished_job(
        "Compare the shortlisted sites",
        "partial",
        "Four of six sites answered; the other two returned 403.",
    ));
    assert!(partial.contains("is finished"), "{partial}");
    assert!(!partial.contains("did not finish"), "{partial}");
    assert!(partial.contains("403"), "the gaps it names are the result");

    let failed = relay_fallback_line(&finished_job("Migrate the rows", "failed", ""));
    assert!(failed.contains("did not finish"), "{failed}");
    assert!(failed.contains("Migrate the rows"), "{failed}");

    // An approval follow-up is a relay turn too, and its line is unchanged.
    let noted = relay_fallback_line(&TurnKind::ApprovalOutcome {
        approval_id: "01A".into(),
        title: "delete out/taxi_app.zip".into(),
    });
    assert_eq!(noted, "Noted your decision on “delete out/taxi_app.zip”.");
    assert_eq!(relay_fallback_line(&TurnKind::User), "");
}
