//! Cancelling a job: the job stops, its state says so, and nothing of it reaches the chat
//! afterwards.
//!
//! Jacob on Test5, round 6 (2026-11-04, agentd 0.1.193 `3595b9fe0`, side chat
//! 01M6ME9A7EGV5PJ7DZW43AHTN5, scenario `r6busyburst`), bug S. At 21:19:54 he asked to cancel
//! the Mersenne job. The main agent ran `kill 14237` — a signal to a pid, which no job row ever
//! hears about — and at 21:20:29 told him "the Mersenne job is cancelled and no longer running".
//!
//! Job 01M6MVENYJDXMKW8T8YCBWB2VC stayed `running`. Its worker's codemode loop
//! (`for i<70: bash 'sleep 20; tail -1 log'` under a 1 500 000 ms `timeout_ms`) polled an
//! unchanging log until 21:43:26, 1 401 seconds past the supposed cancel, and then posted a
//! second result and a file at 21:43:39–21:44:03. Final state `done`, elapsed 1 618 s. So the
//! user was told a false thing, and a dead job's output arrived 23 minutes later.
//!
//! The cancel path itself was not missing: `stop_job` has been the sanctioned tool all along.
//! What was missing is in these tests — a cancel that ends the worker's in-flight tool call, a
//! chat that cannot receive anything from a cancelled job, and a sentence about a cancel that has
//! to rest on the job's state.

use std::time::{Duration, Instant};

use arbos_core::doc::{CommandView, MessageStatus, UserMessageOpts};
use serde_json::{json, Value};

use crate::agent::{Agent, CANCEL_CLAIM_REFUSAL};
use crate::status::tests::test_agent;
use crate::sync::SharedHost;

const CHAT: &str = "01TESTCHATJOBCANCEL0000001";
const JOB: &str = "01TESTJOBCANCELMERSENNE001";
const WORKER: &str = "01TESTWORKERJOBCANCEL00001";
const ASK: &str = "01TESTASKJOBCANCEL00000001";

/// Every assistant row the apps would draw: `hidden` rows are skipped by both clients, so a
/// retired row is not a row the user sees.
fn visible_assistant_rows(host: &SharedHost) -> Vec<String> {
    let h = host.try_lock().expect("host free");
    h.doc
        .messages()
        .into_iter()
        .filter(|m| {
            m.role == "assistant" && m.rest.get("hidden").and_then(Value::as_bool) != Some(true)
        })
        .map(|m| m.id)
        .collect()
}

fn row_text(host: &SharedHost, msg_id: &str) -> String {
    let h = host.try_lock().expect("host free");
    h.doc.message(msg_id).map(|m| m.text()).unwrap_or_default()
}

/// A long job of `CHAT` as `run_subagent` leaves one while it works: a `running` job row, a pi
/// worker session, and the hidden progress row that carries the job id and streams its steps.
async fn a_running_job(agent: &std::sync::Arc<Agent>, host: &SharedHost) {
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(
                ASK,
                "hunt mersenne primes for me",
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
            "Hunting Mersenne primes",
            "code",
            "Search for Mersenne primes and report what you find.",
            &[],
        )
        .unwrap();
    agent
        .store
        .insert_subagent(WORKER, "job-mersenne", "hunt mersenne primes", "pi", CHAT)
        .unwrap();
    agent.store.set_job_subagent(JOB, WORKER).unwrap();
    agent.store.set_job_state(JOB, "running", None).unwrap();
    let row = "01TESTROWJOBCANCELWORKER01";
    {
        let h = host.lock().await;
        h.doc.start_assistant_message(row, ASK, "box:test");
        h.doc.set_message_fields(
            row,
            &[
                (
                    "job",
                    json!({ "id": JOB, "title": "Hunting Mersenne primes", "kind": "code", "touches": [] }),
                ),
                ("worker", json!(true)),
                ("hidden", json!(true)),
            ],
        );
        h.doc.add_status_part(row, "working", "Working…");
        h.doc.set_activity(row, Some("Running a command…"));
    }
    agent
        .subagent_msgs
        .lock()
        .await
        .insert(WORKER.to_owned(), (CHAT.to_owned(), row.to_owned()));
}

/// The cancel the chat issues for one job, the way the job chip does.
fn cancel_cmd(cancel_msg_id: &str) -> CommandView {
    CommandView {
        id: arbos_core::ids::ulid(),
        kind: "stop".into(),
        payload: json!({ "messageId": cancel_msg_id, "jobId": JOB }),
        issued_by: "dev-mac".into(),
        issued_at: arbos_core::ids::now_ms(),
        status: "pending".into(),
    }
}

/// The three things a cancel owes the user, on one long job cancelled from the chat.
///
/// 1. the job's state reads `cancelled` within ten seconds;
/// 2. no assistant row of that job reaches the chat after the cancel — not its worker's row, not
///    a result, not a file;
/// 3. the reply that reports the cancel cites that state.
#[tokio::test]
async fn cancelling_a_job_from_the_chat_stops_it_and_silences_it() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    a_running_job(&agent, &host).await;

    // 21:19:54: "cancel the Mersenne job".
    let cancel_msg = "01TESTASKJOBCANCELSTOP0001";
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(
                cancel_msg,
                "cancel the Mersenne job",
                "dev-mac",
                &UserMessageOpts::default(),
            )
            .unwrap();
    }
    let t0 = Instant::now();
    agent
        .handle_command(CHAT, cancel_cmd(cancel_msg))
        .await
        .unwrap();

    // 1. The state, inside ten seconds. The cancel is synchronous, so this is the budget rather
    //    than a wait; the loop is here so a slow box reports the state it reached.
    let mut state = String::new();
    while t0.elapsed() < Duration::from_secs(10) {
        state = agent.store.job(JOB).unwrap().unwrap().state;
        if state == "cancelled" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        state,
        "cancelled",
        "the job must read cancelled within ten seconds of the cancel, not {state} \
         ({:?} elapsed)",
        t0.elapsed()
    );

    // 3. The reply cites it. "I ran a command" is what the agent knew in round 6; the state is
    //    the only thing that makes "cancelled" true.
    let reply = {
        let h = host.lock().await;
        h.doc
            .messages()
            .into_iter()
            .find(|m| {
                m.role == "assistant"
                    && m.rest.get("inReplyTo").and_then(Value::as_str) == Some(cancel_msg)
            })
            .map(|m| m.id)
            .expect("the cancel is answered in the chat")
    };
    let text = row_text(&host, &reply);
    assert!(
        text.to_lowercase().contains("cancelled"),
        "the cancel reply must cite the state it read back: {text}"
    );
    assert!(
        text.to_lowercase().contains("hunting mersenne primes"),
        "and name the job it is about: {text}"
    );

    // 2. Nothing of the job reaches the chat from here on. Its worker's row is retired: hidden,
    //    settled, and no activity line left to drive a client's working indicator.
    {
        let h = host.lock().await;
        let worker_row = h
            .doc
            .message("01TESTROWJOBCANCELWORKER01")
            .expect("the worker row is still in the doc");
        assert_eq!(
            worker_row.rest.get("hidden").and_then(Value::as_bool),
            Some(true),
            "the cancelled job's worker row must draw nothing: {worker_row:?}"
        );
        assert_eq!(
            worker_row.status,
            MessageStatus::Done.as_str(),
            "and must not be left streaming"
        );
        assert!(
            worker_row
                .rest
                .get("activity")
                .and_then(Value::as_str)
                .is_none(),
            "nor keep an activity line: {worker_row:?}"
        );
    }
    let after_cancel = visible_assistant_rows(&host);

    // 21:43:39, in round 6: the worker, still alive, delivered its file. A cancelled job's
    // delivery is refused and writes no row.
    let dir = crate::media::out_dir(&agent.cfg);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("mersenne.txt"), b"2147483647\n").unwrap();
    let posted = crate::media::deliver_file(
        &agent,
        &json!({ "path": "out/mersenne.txt", "session_id": WORKER }),
    )
    .await;
    assert_eq!(
        posted["isError"],
        json!(true),
        "a cancelled job must not put a file in the chat: {posted}"
    );
    let told = posted["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        told.contains("NOT posted") && told.contains("cancelled"),
        "and the worker is told why: {told}"
    );
    assert_eq!(
        visible_assistant_rows(&host),
        after_cancel,
        "the cancelled job left a new row in the chat"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// The other half of bug S: the sentence. The main agent said the job was cancelled because it
/// had issued a command, and nothing checked that against the job rows. A claim of a cancel is
/// refused while a job of the chat is still open, and it goes out once the state agrees.
#[tokio::test]
async fn a_reported_cancel_has_to_match_the_job_state() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    a_running_job(&agent, &host).await;
    let _busy = agent.test_hold_turn(CHAT).await;

    // The round-6 sentence, word for word, with the job still running.
    let claim = "The Mersenne job is cancelled and no longer running.";
    let refused = agent
        .hook_send_message(claim, None, None)
        .await
        .expect_err("a cancel nothing cancelled must not reach the chat");
    assert_eq!(refused, CANCEL_CLAIM_REFUSAL);

    // The reply text and the ack carry the same claim to the user, so the route refuses it too.
    for args in [
        json!({ "action": "reply", "reply": claim }),
        json!({ "action": "act", "plan": "confirm the cancel", "ack": "Killed it — the Mersenne job is cancelled." }),
    ] {
        let err = agent
            .hook_route(&args)
            .await
            .expect_err("the route must refuse the same claim");
        assert_eq!(err, CANCEL_CLAIM_REFUSAL, "{args}");
    }

    // Progress about the job that is still running is what the turn should say instead.
    agent
        .hook_send_message(
            "The Mersenne job is still running; it is on its fourth exponent.",
            None,
            None,
        )
        .await
        .expect("an honest progress line is not a cancel claim");

    // Once the job really is cancelled, the sentence is true and it goes out.
    agent.store.set_job_state(JOB, "cancelled", None).unwrap();
    agent
        .hook_send_message(claim, None, None)
        .await
        .expect("with the state cancelled the claim is grounded");

    let _ = std::fs::remove_dir_all(&root);
}
