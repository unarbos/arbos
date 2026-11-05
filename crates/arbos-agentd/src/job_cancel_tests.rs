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
use arbos_core::ids::ulid;
use serde_json::{json, Value};

use crate::agent::{Agent, CANCEL_CLAIM_REFUSAL};
use crate::send_order::SEND_ORDER_WAIT;
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

/// A cancel retires the job's live rows even when the chat's room has gone cold — the window
/// where the hub lets a room go between the command resolving it and the retirement reaching it.
///
/// The rows are in the persisted doc whether the hub holds the room or not, so a retirement that
/// quietly no-opped would be undone by the next writer to reopen the chat: the worker's row
/// comes back from disk still streaming, and a client draws a working indicator for a dead job.
#[tokio::test]
async fn a_cancel_retires_the_jobs_rows_on_a_chat_whose_room_went_cold() {
    let (agent, root) = test_agent().await;
    let opened = agent.sync.open_room(CHAT).await.unwrap();
    a_running_job(&agent, &opened).await;

    // Cold, the shape a cap or an idle sweep leaves: persisted, out of the hub's map, and not
    // `closed` — `closed` marks a chat this agent was removed from and must never be reopened
    // into (#1440).
    agent.sync.persist(&opened).await.unwrap();
    agent.sync.hosts.lock().await.remove(CHAT);
    assert!(agent.sync.host(CHAT).await.is_none());
    assert!(!opened.lock().await.closed);

    // The same call the stop command makes, which is the only part of it that needs no room.
    let stopped = agent.stop_jobs(CHAT, Some(JOB)).await;
    assert_eq!(stopped.len(), 1);
    assert_eq!(stopped[0].state, "cancelled");

    // The retirement went where the rows are rather than skipping a chat the hub had let go.
    let live = agent
        .sync
        .host(CHAT)
        .await
        .expect("the cancel took the chat's room to retire its rows in");
    let retired = |host: &SharedHost| {
        let h = host.try_lock().expect("host free");
        let row = h
            .doc
            .message("01TESTROWJOBCANCELWORKER01")
            .expect("the worker row");
        (
            row.rest.get("hidden").and_then(Value::as_bool),
            row.status.clone(),
        )
    };
    assert_eq!(
        retired(&live),
        (Some(true), MessageStatus::Done.as_str().to_owned()),
        "a cancelled job's row is left streaming on a chat whose room was cold, so the next \
         reopen brings back a working indicator for a dead job"
    );
    // And when something does reopen the chat, what comes back from disk is settled: the
    // retirement is part of the doc, so there is no streaming row for a reopen to restore.
    agent.sync.persist(&live).await.unwrap();
    agent.sync.hosts.lock().await.remove(CHAT);
    let reopened = agent.sync.open_room(CHAT).await.unwrap();
    assert_eq!(
        retired(&reopened),
        (Some(true), MessageStatus::Done.as_str().to_owned()),
        "the reopened room brought the cancelled job's row back drawing and streaming"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// Bug O puts a per-chat FIFO in front of every reply, and a job's answer waits in it for up to
/// `SEND_ORDER_WAIT` *after* its worker has finished. A cancel landing inside that wait is the
/// one window the check in `run_subagent` cannot see: it ran before the cancel and found a
/// running job.
///
/// The ticket that waiting answer holds *is* the chat's ordering. So this pins both halves: the
/// suppressed answer writes no row, and it releases the place rather than stranding it — a
/// cancel that stalled the chat's order would be a worse failure than the late result it fixed.
#[tokio::test]
async fn a_cancel_inside_the_send_order_wait_posts_nothing_and_strands_no_ticket() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    a_running_job(&agent, &host).await;

    // An older message of the chat is being answered, so the job's answer is queued behind it.
    // `ASK` carries no readable send time, so it inherits the tail's key and can never jump the
    // queue — which is what puts it second here.
    let older = ulid();
    let holding = agent
        .send_order
        .join(CHAT, &older)
        .expect("the older message");
    assert!(
        agent
            .send_order
            .wait_turn(CHAT, &older, SEND_ORDER_WAIT)
            .await
    );
    let ticket = agent.send_order.join(CHAT, ASK).expect("the job's ask");
    assert_eq!(agent.send_order.ahead(CHAT, ASK), 1);

    // The job's own task, as `start_job_for_send` leaves it: the place travels with the answer
    // and is dropped when the answer is posted, or when this task gives up.
    let answering = {
        let agent = agent.clone();
        let host = host.clone();
        tokio::spawn(async move {
            let r = agent
                .write_job_result(
                    CHAT,
                    &host,
                    ASK,
                    "job-mersenne",
                    "done",
                    "Found M127.",
                    None,
                )
                .await;
            drop(ticket);
            r
        })
    };
    // Long enough to be past the model call and sitting in the wait, which the empty reply set
    // is the check on: an answer already written would make the rest of this measure nothing.
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(
        job_answers_to(&host, ASK).await,
        Vec::<String>::new(),
        "the answer was posted before the cancel, so this test proves nothing"
    );

    let cancel_msg = "01TESTASKJOBCANCELSTOP0003";
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
    agent
        .handle_command(CHAT, cancel_cmd(cancel_msg))
        .await
        .unwrap();

    // The cancel itself queues for nothing: a `stop` takes no place, so it lands however long
    // the chat's burst is. Were it gated, a cancel would be answerable only after the burst it
    // is meant to cut short.
    assert_eq!(
        agent.send_order.waiting(CHAT),
        2,
        "the stop took a place in the chat's send order"
    );

    // A third message, so what follows measures a real turn coming up and not an empty queue.
    let next = ulid();
    let _next_place = agent.send_order.join(CHAT, &next).expect("behind both");

    drop(holding);
    let t0 = Instant::now();
    answering
        .await
        .expect("the job's task")
        .expect("a suppressed answer is not a failure");
    assert!(
        t0.elapsed() < Duration::from_secs(5),
        "the cancelled answer sat out the whole ordering valve instead of returning: {:?}",
        t0.elapsed()
    );
    assert_eq!(
        job_answers_to(&host, ASK).await,
        Vec::<String>::new(),
        "the cancelled job answered in the chat after all"
    );

    // The place is free, so the message behind it is answered at once rather than after
    // `SEND_ORDER_WAIT`. This is the assertion a stranded ticket fails.
    assert_eq!(agent.send_order.waiting(CHAT), 1);
    let t1 = Instant::now();
    assert!(
        agent
            .send_order
            .wait_turn(CHAT, &next, Duration::from_secs(5))
            .await,
        "the cancelled job stranded its place and stalled the chat's order"
    );
    assert!(t1.elapsed() < Duration::from_secs(1), "{:?}", t1.elapsed());

    let _ = std::fs::remove_dir_all(&root);
}

/// A job the model started answers through its relay result turn, which is a writer of its own,
/// and it carries its own copy of what bug O created: `await_send_order` sits *inside*
/// `run_result_turn`, after every cancel check `run_subagent` makes on its way there, and holds
/// the answer for up to `SEND_ORDER_WAIT`. A cancel landing in that window would be followed by
/// the whole answer being delivered minutes later — bug S, on this path.
///
/// Bug AA moved a job the user asked for in words off this path — its result is written by
/// `write_job_result`, whose own cancel window
/// `a_cancel_inside_the_send_order_wait_posts_nothing_and_strands_no_ticket` covers — so this
/// case is the model-started job it was always also about. The wait is set up by hand below
/// because a relay turn's own cancel window is what is under test, whoever started the job.
///
/// Measured before the fix: the cancelled job ran the full relay turn and left a row in the
/// chat. A relay turn needs a live main agent to write the prose, which a test has not got, so
/// what is asserted is the row — the thing the user would have seen either way.
#[tokio::test]
async fn a_cancelled_job_delivers_nothing_through_its_relay_turn() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    a_running_job(&agent, &host).await;
    let spec = crate::agent::JobSpec {
        id: JOB.to_owned(),
        title: "Hunting Mersenne primes".to_owned(),
        kind: "code".to_owned(),
        brief: "Search for Mersenne primes and report what you find.".to_owned(),
        touches: Vec::new(),
        attempt: 0,
    };

    // An older message of the chat is being answered, so this answer queues behind it and the
    // relay turn parks in `await_send_order` exactly as it does in production.
    let older = ulid();
    let holding = agent
        .send_order
        .join(CHAT, &older)
        .expect("the older message");
    assert!(
        agent
            .send_order
            .wait_turn(CHAT, &older, SEND_ORDER_WAIT)
            .await
    );
    let ticket = agent.send_order.join(CHAT, ASK).expect("the job's ask");
    assert_eq!(agent.send_order.ahead(CHAT, ASK), 1);

    let before = visible_assistant_rows(&host);
    let relaying = {
        let agent = agent.clone();
        let host = host.clone();
        tokio::spawn(async move {
            let r = agent
                .run_result_turn(
                    CHAT,
                    &host,
                    ASK,
                    &crate::agent::ResultDelivery {
                        subagent_id: WORKER,
                        name: "job-hunt-mersenne-b2vc",
                        status: "done",
                        total_ms: 1_000,
                        raw: "Found M127.",
                        result: "Found M127.",
                        error: None,
                        job: Some(&spec),
                    },
                )
                .await;
            drop(ticket);
            r
        })
    };
    // Long enough to be parked in the wait rather than still building its prompt.
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(
        visible_assistant_rows(&host),
        before,
        "the answer was delivered before the cancel, so this test proves nothing"
    );

    let cancel_msg = "01TESTASKJOBCANCELSTOP0004";
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
    agent
        .handle_command(CHAT, cancel_cmd(cancel_msg))
        .await
        .unwrap();
    let with_stop_line = visible_assistant_rows(&host);

    drop(holding);
    let t0 = Instant::now();
    let _ = relaying.await.expect("the relay task");
    assert_eq!(
        visible_assistant_rows(&host),
        with_stop_line,
        "the cancelled job delivered through its relay turn after all"
    );
    assert!(
        t0.elapsed() < Duration::from_secs(5),
        "and it sat out the ordering valve to do it: {:?}",
        t0.elapsed()
    );
    assert_eq!(
        agent.send_order.waiting(CHAT),
        0,
        "the relay turn stranded the ask's place in the chat's send order"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// The other side of the same wait. A job already cancelled when its result turn starts must not
/// join the queue at all: it has nothing to post, so a place at the head would hold every
/// message behind it for up to `SEND_ORDER_WAIT` to reach a `return`.
#[tokio::test]
async fn a_relay_turn_for_an_already_cancelled_job_does_not_wait_for_its_turn() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    a_running_job(&agent, &host).await;
    agent.store.set_job_state(JOB, "cancelled", None).unwrap();
    let spec = crate::agent::JobSpec {
        id: JOB.to_owned(),
        title: "Hunting Mersenne primes".to_owned(),
        kind: "code".to_owned(),
        brief: "Search for Mersenne primes and report what you find.".to_owned(),
        touches: Vec::new(),
        attempt: 0,
    };

    // An older message holds the head, so anything that joins the queue here is stuck behind it.
    let older = ulid();
    let _holding = agent
        .send_order
        .join(CHAT, &older)
        .expect("the older message");
    assert!(
        agent
            .send_order
            .wait_turn(CHAT, &older, SEND_ORDER_WAIT)
            .await
    );
    let _ticket = agent.send_order.join(CHAT, ASK).expect("the job's ask");

    let t0 = Instant::now();
    agent
        .run_result_turn(
            CHAT,
            &host,
            ASK,
            &crate::agent::ResultDelivery {
                subagent_id: WORKER,
                name: "job-hunt-mersenne-b2vc",
                status: "done",
                total_ms: 1_000,
                raw: "Found M127.",
                result: "Found M127.",
                error: None,
                job: Some(&spec),
            },
        )
        .await
        .expect("a suppressed result turn is not a failure");
    assert!(
        t0.elapsed() < Duration::from_secs(1),
        "the cancelled job's result turn queued behind the older message: {:?}",
        t0.elapsed()
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// Bug T opens a second door from a job into its chat: `send_message` with no `to`, which posts
/// a standalone row through `post_to_job_chat`. A cancelled job may not use that one either.
///
/// Nothing in `run_subagent` covers it. Those checks run after the worker's turn has returned;
/// this path is called from *inside* that turn by the worker's own script, so the only thing
/// between it and the chat is the question it asks itself.
#[tokio::test]
async fn a_cancelled_job_cannot_post_a_progress_note_either() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    a_running_job(&agent, &host).await;

    let stopped = agent.stop_jobs(CHAT, Some(JOB)).await;
    assert_eq!(stopped.len(), 1);
    assert_eq!(stopped[0].state, "cancelled");
    let after_cancel = visible_assistant_rows(&host);

    // The worker's script, mid-run, reaching for the chat the way bug T made possible.
    let refused = agent
        .post_to_job_chat(WORKER, "Still polling the log; nothing new yet.")
        .await
        .expect_err("a cancelled job must not post a progress note");
    let said = refused.to_string();
    assert!(
        said.contains("cancelled") && said.contains("NOT posted"),
        "and the worker is told why, so it stops rather than retrying: {said}"
    );
    assert_eq!(
        visible_assistant_rows(&host),
        after_cancel,
        "the progress note left a row in the chat"
    );

    // The refusal is read off the job's state before the room is touched, so a cancelled job
    // does not even reopen a chat to be turned away from it. The cold shape is the one
    // `a_cancel_retires_the_jobs_rows_on_a_chat_whose_room_went_cold` pins: persisted, out of
    // the hub's map, and not `closed`.
    agent.sync.persist(&host).await.unwrap();
    agent.sync.hosts.lock().await.remove(CHAT);
    agent
        .post_to_job_chat(WORKER, "Still polling the log; nothing new yet.")
        .await
        .expect_err("still refused with the room cold");
    assert!(
        agent.sync.host(CHAT).await.is_none(),
        "a cancelled job's note reopened the room it was refused from"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// The answer bubbles a job wrote in reply to `msg_id`. `decision: "job"` is what
/// `write_job_result` stamps on the one row it posts, which is the row a cancel must prevent —
/// the worker's own hidden progress row replies to the same message and is not one.
async fn job_answers_to(host: &SharedHost, msg_id: &str) -> Vec<String> {
    let h = host.lock().await;
    h.doc
        .messages()
        .into_iter()
        .filter(|m| {
            m.rest.get("inReplyTo").and_then(Value::as_str) == Some(msg_id)
                && m.rest.get("decision").and_then(Value::as_str) == Some("job")
        })
        .map(|m| m.id)
        .collect()
}
