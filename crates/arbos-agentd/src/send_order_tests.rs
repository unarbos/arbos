//! Bug O's own test: a burst of user messages is answered in the order it was sent.
//!
//! Jacob on Test5, round 4 (2026-11-04, agentd 0.1.193 / 3595b9fe0, chat
//! 01M6ME9A7EGV5PJ7DZW43AHTN5). He sent 30 messages about three seconds apart, 19:12:48 to
//! 19:14:12. All 30 were answered and none was lost, but the answers came back
//! `1..10, 14, 17, 23, 13, 18, 20, 26, 15, 27, 28, 29, 30, 25, 21, 24, 19, 16, 11, 22, 12`,
//! with a 37-second silence from 19:13:17 to 19:13:54 while thirteen of them queued.
//!
//! The main lane is the one this file drives. `Agent::acquire_turn_lock` spun on
//! `turn_lock.try_lock()` every 50 ms, which is a lottery and not a queue: thirteen waiters
//! are served in whatever order the spins fall, and one of them can be passed over again and
//! again — which is both the scrambling and the silence. It now waits for the message's turn in
//! its chat's FIFO first (`send_order.rs`), so only the oldest unanswered message contends.
//!
//! The test is written at the gate rather than through a whole turn because a test box has no
//! pi (`pi_bin` points at nothing), so no main session can run. What it asserts is what the
//! round-4 grade measured: the order the answers were let out in, and the longest gap between
//! two of them.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use arbos_core::approval::{ApprovalOutcome, ApprovalRequest, Risk};
use arbos_core::doc::{CommandView, MessageStatus, MessageView, UserMessageOpts};
use arbos_core::ids::{now_ms, ulid};
use serde_json::{json, Value};
use tokio::time::Instant;

use crate::agent::{retire_if_wordless, retire_wordless_rows, JobSpec};
use crate::send_order::SEND_ORDER_WAIT;
use crate::status::tests::{test_agent, Journal, FIRST_REPLY_GATE_SECS};
use crate::sync::SharedHost;

/// How long #2759's inline approval wait holds the chat's main turn, measured on Jacob's box in
/// Test5 rounds 4 and 5: the pi extension polls `/hook/approval-status` for fourteen minutes
/// before it gives up, and the gated tool call is blocked for all of it.
///
/// Only a floor for these tests, and a historical one: `Agent::request_approval` now waits the
/// park grace and then lets go, leaving the card open. These cases keep the long wait as the
/// scenario they were written against — the valve has to hold under a head-of-queue wait of any
/// length, and a wait that ends on its own would make them pass for the wrong reason.
const INLINE_APPROVAL_WAIT: Duration = Duration::from_secs(14 * 60);

/// The measured burst, scaled so a test can run it: 30 sends arriving one after another, each
/// taking longer to answer than the gap to the next, so the queue grows the way it did on
/// Jacob's box. `SEND_GAP` stands for his three seconds and `TURN` for a turn of about five.
///
/// The stagger is the point, not decoration. With every lane started in the same instant the old
/// `try_lock` spin is accidentally fair — the waiters sleep 50 ms in lockstep and wake in the
/// order they were spawned — and a test built that way passes against the bug. Arrivals spread
/// across the poll window put the waiters out of step, which is the production shape.
const SEND_GAP: Duration = Duration::from_millis(30);
const TURN: Duration = Duration::from_millis(120);

/// How long a burst case waits for its lanes before it gives up on them.
///
/// Three times the valve, because a lane may legitimately wait the whole of
/// [`SEND_ORDER_WAIT`] and then answer out of turn — that is the designed behaviour, not a
/// fault, so the bound has to clear it with room to spare. Past that a lane has nothing left
/// to wait on: its `turn_lock` spin only ever waits on a holder the case itself releases.
///
/// The bound exists so that a defect reads as a failed assertion naming the lane rather than as
/// a test binary that never returns, which is far more expensive to debug in CI. On the paused
/// cases it costs no real time, and a true deadlock there leaves every task idle, which makes
/// tokio advance the clock to this timeout and fail the case at once.
const LANES_DONE_BY: Duration = Duration::from_secs(SEND_ORDER_WAIT.as_secs() * 3);

/// Join every lane, or fail naming the one that did not finish.
async fn join_lanes(lanes: Vec<tokio::task::JoinHandle<()>>) {
    for (n, lane) in lanes.into_iter().enumerate() {
        tokio::time::timeout(LANES_DONE_BY, lane)
            .await
            .unwrap_or_else(|_| panic!("lane {} did not answer within {LANES_DONE_BY:?}", n + 1))
            .expect("lane");
    }
}

/// 30 sends, answered 1 through 30 in order, with no gap over ten seconds.
#[tokio::test]
async fn a_burst_of_thirty_is_answered_in_send_order() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATSENDORDER0000030";
    let host = agent.sync.open_room(chat).await.unwrap();
    // Minted and appended in send order, as a client's burst is. At least a millisecond apart,
    // because a ULID's timestamp is what carries the send order between boxes.
    let mut ids: Vec<String> = Vec::new();
    for n in 1..=30 {
        tokio::time::sleep(Duration::from_millis(2)).await;
        let id = ulid();
        host.lock()
            .await
            .doc
            .append_user_message(
                &id,
                &format!("reply with only {n}"),
                "dev-mac",
                &UserMessageOpts::default(),
            )
            .unwrap();
        ids.push(id);
    }
    let answered: Arc<Mutex<Vec<(usize, Instant)>>> = Arc::new(Mutex::new(Vec::new()));
    let mut lanes = Vec::new();
    for (n, id) in ids.into_iter().enumerate() {
        let agent = agent.clone();
        let answered = answered.clone();
        lanes.push(tokio::spawn(async move {
            tokio::time::sleep(SEND_GAP * n as u32).await;
            // The message takes its place in the chat's send order, as `handle_command` does
            // when it takes the command, and holds it until this lane has answered.
            let ticket = agent.send_order.join(chat, &id).expect("a fresh place");
            let turn = agent.acquire_turn_lock(chat, &id, false).await;
            tokio::time::sleep(TURN).await;
            answered
                .lock()
                .expect("answers")
                .push((n + 1, Instant::now()));
            drop(turn);
            drop(ticket);
        }));
    }
    join_lanes(lanes).await;

    let answered = answered.lock().expect("answers").clone();
    let order: Vec<usize> = answered.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        order,
        (1..=30).collect::<Vec<_>>(),
        "the answers must come back in send order"
    );
    let worst = answered
        .windows(2)
        .map(|w| w[1].1.duration_since(w[0].1))
        .max()
        .expect("29 gaps between 30 answers");
    assert!(
        worst < Duration::from_secs(10),
        "the longest gap between two answers was {worst:?}"
    );
    assert_eq!(
        agent.send_order.waiting(chat),
        0,
        "every place is released once its message is answered"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The round-6 shape, which the thirty-message case does not reach: a job holds the chat for the
/// whole arrival window, so the burst queues entire and is then released all at once.
///
/// Jacob, Test5 round 6, `r6busyburst`: fourteen messages sent 21:18:52–21:19:25 with a job
/// running throughout drew no reply at all until 21:19:27, and then came back
/// `2, 4, 1, 7, 3, 14…20, 6, 12, 10, 13, 11, 8, 5, 9` — the tail answered ahead of the middle,
/// with message 9 waiting 105 seconds. Two of that run's details are the reason this is written
/// out separately from [`a_burst_of_thirty_is_answered_in_send_order`], where turns finish as
/// they go and only a few lanes ever wait together:
///
/// - **All fourteen wait at once.** That is what let the tail overtake the middle; the lottery
///   picks from everyone waiting, so the spread is widest when the queue is fullest.
/// - **They still arrive out of phase**, over 33 seconds, which is what makes the lottery unfair
///   in the first place. Released in lockstep the old 50 ms spin is accidentally fair and a test
///   written that way passes against the bug, so the stagger is kept.
///
/// With the FIFO wait taken out of `acquire_turn_lock` this case answers
/// `3, 10, 5, 12, 7, 2, 14, 9, 4, 11, 6, 1, 13, 8` — round 6's shape, tail ahead of middle.
#[tokio::test]
async fn a_burst_that_queues_behind_a_job_is_answered_in_send_order() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATSENDORDER0000014";
    let host = agent.sync.open_room(chat).await.unwrap();
    // The job that was running throughout: its ask arrived before the burst, so it holds both
    // the chat's turn and the head of the chat's order while the burst piles up behind it.
    let job_ask = ulid();
    let job_place = agent
        .send_order
        .join(chat, &job_ask)
        .expect("the job's ask");
    let job_turn = agent.acquire_turn_lock(chat, &job_ask, false).await;

    let mut ids: Vec<String> = Vec::new();
    for n in 1..=14 {
        tokio::time::sleep(Duration::from_millis(2)).await;
        let id = ulid();
        host.lock()
            .await
            .doc
            .append_user_message(
                &id,
                &format!("reply with only {n}"),
                "dev-mac",
                &UserMessageOpts::default(),
            )
            .unwrap();
        ids.push(id);
    }
    let answered: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));
    let mut lanes = Vec::new();
    for (n, id) in ids.into_iter().enumerate() {
        let agent = agent.clone();
        let answered = answered.clone();
        lanes.push(tokio::spawn(async move {
            tokio::time::sleep(SEND_GAP * n as u32).await;
            let ticket = agent.send_order.join(chat, &id).expect("a fresh place");
            let turn = agent.acquire_turn_lock(chat, &id, false).await;
            answered.lock().expect("answers").push(n + 1);
            drop(turn);
            drop(ticket);
        }));
    }
    // Every message has arrived and is waiting before anything is allowed to answer, which is
    // the state `r6busyburst` was in at 21:19:25.
    tokio::time::sleep(SEND_GAP * 15 + TURN).await;
    assert_eq!(
        agent.send_order.waiting(chat),
        15,
        "the job's ask plus the whole burst must be queued before it lets go, or this is not \
         round 6's shape"
    );
    assert!(
        answered.lock().expect("answers").is_empty(),
        "nothing may be answered while the job holds the chat"
    );
    drop(job_turn);
    drop(job_place);
    join_lanes(lanes).await;

    assert_eq!(
        answered.lock().expect("answers").clone(),
        (1..=14).collect::<Vec<_>>(),
        "a burst released all at once must still be answered in send order"
    );
    assert_eq!(agent.send_order.waiting(chat), 0);
    let _ = std::fs::remove_dir_all(&root);
}

/// Order is not held at any price (Jacob's ruling, 2026-11-05): a finished answer behind a turn
/// that is merely slow posts once `SEND_ORDER_WAIT` has passed.
///
/// Round 4's `clean` task held its turn for sixteen minutes. A job finishing beside one of those
/// used to answer at once and out of order; ordering on its own would keep the finished answer
/// silent for the whole sixteen minutes, which reads to the person waiting as a dropped message.
/// This pins the middle: the answer waits its turn, and then it goes out.
///
/// What the ceiling below asserts is that the valve is short enough to be about silence rather
/// than about a wedge. It is the assertion the 900-second version could not pass, and the reason
/// this case is written out instead of being left to the ordering tests, which 900 also passed.
/// The clock is paused, so the wait costs no real time and the measurement is the virtual one.
#[tokio::test(start_paused = true)]
async fn a_finished_answer_behind_a_long_turn_posts_after_the_valve() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATSENDORDER0000032";
    let _host = agent.sync.open_room(chat).await.unwrap();
    // The long turn takes the floor and never answers while this test runs.
    let long_turn = ulid();
    let _holding = agent
        .send_order
        .join(chat, &long_turn)
        .expect("the long turn");
    assert!(
        agent
            .send_order
            .wait_turn(chat, &long_turn, SEND_ORDER_WAIT)
            .await
    );
    // The ask that ran beside it, finished, holding its answer.
    let finished = ulid();
    let _place = agent
        .send_order
        .join(chat, &finished)
        .expect("the ask behind it");
    assert_eq!(agent.send_order.ahead(chat, &finished), 1);

    let t0 = Instant::now();
    agent.await_send_order(chat, &finished, "job").await;
    let waited = t0.elapsed();

    assert!(
        waited >= SEND_ORDER_WAIT,
        "the answer jumped the order after only {waited:?}"
    );
    assert!(
        waited < Duration::from_secs(180),
        "a finished answer stayed silent for {waited:?} behind a turn that is only slow"
    );
    assert_eq!(
        agent.send_order.ahead(chat, &finished),
        1,
        "the long turn has still not answered, so this really did post out of turn"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The reply rows written under `user_msg_id`, with the host lock taken and released.
async fn replies_to(host: &SharedHost, user_msg_id: &str) -> Vec<MessageView> {
    host.lock()
        .await
        .doc
        .messages()
        .into_iter()
        .filter(|m| {
            m.role == "assistant"
                && m.rest.get("inReplyTo").and_then(Value::as_str) == Some(user_msg_id)
        })
        .collect()
}

/// Both entry points of the wordless-row rule, run over every assistant row in the chat: #2770's
/// turn-finish sweep with nothing exempted, and #2771's single-row rule, which needs no
/// `rows_opened` membership and so can be pointed at anything. Returns how many rows were
/// retired, which is only ever reported in a failure.
async fn sweep_every_row(host: &SharedHost) -> usize {
    let h = host.lock().await;
    let every_row: Vec<String> = h
        .doc
        .messages()
        .into_iter()
        .filter(|m| m.role == "assistant")
        .map(|m| m.id)
        .collect();
    retire_wordless_rows(&h.doc, &every_row, "")
        + every_row
            .iter()
            .filter(|id| retire_if_wordless(&h.doc, id))
            .count()
}

/// The wordless-row rule cannot hide an answer this FIFO is holding — asked as a question and
/// answered with the case built, because the shape is a fair thing to fear.
///
/// The rule retires a finished row that shows nothing, and "a row opened and then left empty for
/// up to two minutes" is a plain description of what a held answer would look like if the hold
/// were placed after the row.
///
/// It is placed before. Each of the four gates sits in front of the `start_assistant_message`
/// (or, for the main lane and the job fallback, in front of the turn lock that precedes any row),
/// so while a lane waits there is no row in the chat for the rule to find. And when the row does
/// come, `write_job_result` opens it, fills it and finishes it inside one `host` guard, so no
/// reader can see it between those steps.
///
/// Both halves are checked against the real `write_job_result`, and under both entry points the
/// merged tree has. #2770's turn-finish sweep is run harder than any turn runs it: every
/// assistant row in the chat offered as a row the turn opened, with a `last` that exempts none.
/// #2771's `retire_if_wordless` is the same rule for one row, lifted out so `run_subagent` can
/// apply it where no `rows_opened` set is involved at all, and it is run here over every row in
/// the chat for the same reason — where the rule is applied has grown twice now, so the case is
/// written against a rule that may be applied anywhere rather than against today's call sites.
/// Split that one guard in two and this test goes red.
#[tokio::test]
async fn the_turn_finish_sweep_cannot_hide_a_held_answer() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATSENDORDER0000033";
    let host = agent.sync.open_room(chat).await.unwrap();
    let older = ulid();
    tokio::time::sleep(Duration::from_millis(2)).await;
    let younger = ulid();
    for (id, text) in [(&older, "the long one"), (&younger, "reply with only 2")] {
        host.lock()
            .await
            .doc
            .append_user_message(id, text, "dev-mac", &UserMessageOpts::default())
            .unwrap();
    }
    // The older message holds the head, as a turn in flight does.
    let holding = agent
        .send_order
        .join(chat, &older)
        .expect("the older message");
    assert!(
        agent
            .send_order
            .wait_turn(chat, &older, SEND_ORDER_WAIT)
            .await
    );
    let _place = agent
        .send_order
        .join(chat, &younger)
        .expect("the ask behind it");

    // The ask ran as a job beside that turn and has finished. Its answer is written and waiting.
    let job = {
        let agent = agent.clone();
        let host = host.clone();
        let younger = younger.clone();
        tokio::spawn(async move {
            agent
                .write_job_result(
                    chat,
                    &host,
                    &younger,
                    "ask-reply-with-only-2",
                    "done",
                    "The answer is 2.",
                    None,
                )
                .await
        })
    };

    // While the hold is on, the sweep has nothing of this answer to find.
    let mut swept = 0;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert!(
            replies_to(&host, &younger).await.is_empty(),
            "the held answer already has a row, so the sweep can reach it while it waits"
        );
        swept += sweep_every_row(&host).await;
    }

    // The turn ahead answers. From here the row may appear at any moment, and every look at it
    // has to find it already carrying its answer.
    drop(holding);
    while !job.is_finished() {
        for m in replies_to(&host, &younger).await {
            assert!(
                m.has_visible_content(),
                "the answer's row was readable while still empty, so the sweep could hide it"
            );
        }
        swept += sweep_every_row(&host).await;
        tokio::task::yield_now().await;
    }
    job.await.expect("the job task").expect("the answer");

    let rows = replies_to(&host, &younger).await;
    assert_eq!(rows.len(), 1, "one answer for the held ask");
    assert!(
        rows[0].has_visible_content(),
        "the held answer reached the chat empty"
    );
    assert_ne!(
        rows[0].rest.get("hidden").and_then(Value::as_bool),
        Some(true),
        "the sweep hid the held answer after {swept} row(s) retired"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A user message appended to the chat, and the `send` command a client issues for it.
async fn send_cmd(host: &SharedHost, text: &str) -> (String, CommandView) {
    let msg_id = ulid();
    host.lock()
        .await
        .doc
        .append_user_message(&msg_id, text, "dev-mac", &UserMessageOpts::default())
        .unwrap();
    let cmd = CommandView {
        id: ulid(),
        kind: "send".into(),
        payload: json!({ "messageId": msg_id }),
        issued_by: "dev-mac".into(),
        issued_at: now_ms(),
        status: "pending".into(),
    };
    (msg_id, cmd)
}

/// The card a destructive shell call raises, as `describe_tool` shapes one.
fn a_card() -> ApprovalRequest {
    ApprovalRequest {
        title: "Run rm -rf build".into(),
        summary: "rm -rf build".into(),
        risk: Risk::Destructive,
        payload: json!({ "kind": "tool_call", "tool": "bash", "input": { "command": "rm -rf build" } }),
    }
}

/// A turn of `chat` blocked in the inline approval wait, as a gated tool call leaves it: the
/// card is posted and undecided, and the returned handle finishes only once it is decided.
///
/// The caller holds the turn lock and the message's place in the send order, which is what the
/// real turn holds across this wait.
async fn an_inline_approval_wait(
    agent: &Arc<crate::agent::Agent>,
    chat: &'static str,
) -> tokio::task::JoinHandle<Option<ApprovalOutcome>> {
    let card = {
        let agent = agent.clone();
        tokio::spawn(async move { agent.request_approval(chat, a_card()).await })
    };
    // The wait has begun when the card is in the chat, which is also what the quick lane reads.
    while !agent.approval_pending_for(chat).await {
        tokio::task::yield_now().await;
    }
    card
}

/// Interaction 1 with #2759 (Jacob, 2026-11-05), and the one the valve exists for.
///
/// #2759 added an inline approval wait that holds the chat's main turn until the card is
/// answered — fourteen minutes, measured twice. That turn is a head-of-line turn of exactly the
/// shape this valve was lowered to 120 seconds for, so the two features meet directly: the head
/// of the chat's FIFO is a turn that will not answer for a quarter of an hour, and behind it
/// sits an answer that is already written.
///
/// What it has to do is post at the valve. The wait is a plain `tokio::time` bound in
/// `SendOrder::wait_turn` and depends on nothing the head does, so the card stays undecided for
/// the whole case: the answer is released *past* the approval wait, not after it. The clock is
/// paused, so two minutes of waiting cost no real time.
///
/// Three things are checked beyond the wait itself. The card is still pending when the answer
/// goes out, or the release proved nothing. The crossing is in the log with a measured
/// `waited_s`, which is how often this happens in production and how a regression in the
/// ordering would show. And nothing in the pair deadlocks: the blocked call lets go of its own
/// accord when the park grace passes, leaving the card open and answerable, which is how an
/// unanswered card stopped being able to hold anything at all.
#[tokio::test(start_paused = true)]
async fn the_valve_releases_an_answer_held_by_an_inline_approval_wait() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATSENDORDER0000034";
    let host = agent.sync.open_room(chat).await.unwrap();
    let older = ulid();
    tokio::time::sleep(Duration::from_millis(2)).await;
    let younger = ulid();
    for (id, text) in [
        (&older, "clear out the build directory"),
        (&younger, "reply with only 2"),
    ] {
        host.lock()
            .await
            .doc
            .append_user_message(id, text, "dev-mac", &UserMessageOpts::default())
            .unwrap();
    }

    // The head: the older message's turn, holding the lock and the floor, blocked on its card.
    let _turn = agent.test_hold_turn_for(chat, &older).await;
    let holding = agent
        .send_order
        .join(chat, &older)
        .expect("the older message");
    assert!(
        agent
            .send_order
            .wait_turn(chat, &older, SEND_ORDER_WAIT)
            .await
    );
    let card = an_inline_approval_wait(&agent, chat).await;

    // Behind it: the ask that ran as a job beside that turn, finished, holding its answer.
    let behind = agent
        .send_order
        .join(chat, &younger)
        .expect("the ask behind it");
    assert_eq!(agent.send_order.ahead(chat, &younger), 1);
    let journal = Journal::default();
    let t0 = Instant::now();
    {
        let _logs = journal.capture();
        agent.await_send_order(chat, &younger, "job").await;
    }
    let waited = t0.elapsed();

    assert!(
        waited >= SEND_ORDER_WAIT,
        "the answer jumped the order after only {waited:?}"
    );
    assert!(
        waited < INLINE_APPROVAL_WAIT,
        "the answer was held {waited:?}, which is the approval wait and not the valve"
    );
    assert!(
        agent.approval_pending_for(chat).await,
        "the card was decided during the wait, so this measured something else"
    );
    assert_eq!(
        agent.send_order.ahead(chat, &younger),
        1,
        "the head has still not answered, so this really did post out of turn"
    );
    let logged = journal.text();
    for part in [
        "posting out of turn",
        "lane=\"job\"",
        "ahead=1",
        "waited_s=120",
    ] {
        assert!(
            logged.contains(part),
            "the crossing log is missing {part}: {logged}"
        );
    }

    // Nothing is wedged, and the way it is not wedged has changed: the blocked tool call was
    // released by its own park when the grace passed, without anybody deciding anything. A
    // valve that released the answer by breaking the wait would still fail the assertions above.
    let id = agent
        .store
        .pending_approvals()
        .unwrap()
        .into_iter()
        .find(|r| r.chat_id == chat)
        .expect("the card")
        .id;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), card)
            .await
            .expect("the blocked tool call was never released")
            .expect("the card task"),
        None,
        "the wait ends undecided: nothing was approved, so nothing may run"
    );
    // And the card it let go of is still there to be answered, which is the whole point of
    // letting go rather than closing it.
    assert_eq!(agent.store.approval(&id).unwrap().unwrap().state, "pending");
    agent
        .decide_approval(&id, ApprovalOutcome::Approved, "dev-mac")
        .await
        .unwrap();
    assert_eq!(
        agent.store.approval(&id).unwrap().unwrap().state,
        "approved"
    );
    drop(behind);
    drop(holding);
    assert_eq!(agent.send_order.waiting(chat), 0);
    let _ = std::fs::remove_dir_all(&root);
}

/// Interaction 2, first half: "how is it going?" during an approval wait.
///
/// The fair queue changed who waits, not only the order they wait in. The old `try_lock` spin
/// served waiters by chance; now every waiter queues behind the head deterministically. With a
/// head held for fourteen minutes that is the moment a person asks how it is going, so the quick
/// lane has to stay out of the queue — and it is the one row that does, marked `quick` and owed
/// an answer inside `FIRST_REPLY_GATE_SECS` rather than behind a turn.
///
/// The whole command goes through `handle_command`, so the exemption is exercised where
/// production applies it: the question takes its place in the FIFO behind the held message, and
/// the quick row is written anyway. The snapshot the quick call reads is checked too — during an
/// approval wait it must say so, since an answer that cannot name what the agent is waiting for
/// is not worth exempting.
#[tokio::test]
async fn a_status_question_is_answered_while_the_turn_waits_for_approval() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATSENDORDER0000035";
    let host = agent.sync.open_room(chat).await.unwrap();
    let older = ulid();
    host.lock()
        .await
        .doc
        .append_user_message(
            &older,
            "clear out the build directory",
            "dev-mac",
            &UserMessageOpts::default(),
        )
        .unwrap();
    let _turn = agent.test_hold_turn_for(chat, &older).await;
    let _holding = agent
        .send_order
        .join(chat, &older)
        .expect("the older message");
    assert!(
        agent
            .send_order
            .wait_turn(chat, &older, SEND_ORDER_WAIT)
            .await
    );
    let _card = an_inline_approval_wait(&agent, chat).await;
    // #2773's `wait_status_line`: the wait now names the call it is waiting on, so the quick
    // answer can repeat it. Asserted on both halves rather than on the whole string, because
    // what matters here is that the line says a wait is on and says what for.
    let activity = agent
        .busy_snapshot(chat, &host)
        .await
        .expect("a turn in an approval wait reads as busy")
        .activity
        .expect("the quick lane cannot say what the agent is waiting for");
    assert!(
        activity.contains("Waiting for your approval") && activity.contains("rm -rf build"),
        "the wait must name itself and the call it is waiting on: {activity}"
    );

    let (asked, cmd) = send_cmd(&host, "what is going on now?").await;
    let t0 = Instant::now();
    agent.handle_command(chat, cmd).await.unwrap();
    let elapsed = t0.elapsed();

    let rows = replies_to(&host, &asked).await;
    assert_eq!(rows.len(), 1, "one quick row, no main-session row");
    assert_eq!(
        rows[0].rest.get("decision").and_then(Value::as_str),
        Some("quick"),
        "the status answer did not come from the quick lane"
    );
    assert_eq!(
        rows[0].rest.get("quick").and_then(Value::as_bool),
        Some(true),
        "the row that skips the send order must say it is the quick one"
    );
    assert!(
        elapsed < Duration::from_secs(FIRST_REPLY_GATE_SECS),
        "the status question took {elapsed:?} with the turn in an approval wait"
    );
    assert!(
        agent.approval_pending_for(chat).await && replies_to(&host, &older).await.is_empty(),
        "the held turn answered meanwhile, so the quick row was not exempted from anything"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Interaction 2, second half: a deterministic queue must not starve anyone.
///
/// Serving waiters in a fixed order is what the lottery could not do, and it is also the way a
/// fixed order can be worse: with the head held for fourteen minutes, every lane behind it waits
/// on the same message instead of taking its chances. So the bound has to hold for all of them
/// at once — five finished answers behind one approval wait, each released at the valve and in
/// send order, none left behind.
///
/// Each lane starts waiting a second after the one before it, as production does: a lane waits
/// from the moment its own answer is written, not from the moment the head stalled.
#[tokio::test(start_paused = true)]
async fn no_lane_behind_an_approval_wait_waits_longer_than_the_valve() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATSENDORDER0000036";
    let host = agent.sync.open_room(chat).await.unwrap();
    let head = ulid();
    host.lock()
        .await
        .doc
        .append_user_message(
            &head,
            "clear out the build directory",
            "dev-mac",
            &UserMessageOpts::default(),
        )
        .unwrap();
    let _turn = agent.test_hold_turn_for(chat, &head).await;
    let _holding = agent.send_order.join(chat, &head).expect("the head");
    assert!(
        agent
            .send_order
            .wait_turn(chat, &head, SEND_ORDER_WAIT)
            .await
    );
    let _card = an_inline_approval_wait(&agent, chat).await;

    let mut ids = Vec::new();
    for _ in 1..=5 {
        tokio::time::sleep(Duration::from_millis(2)).await;
        ids.push(ulid());
    }
    let released: Arc<Mutex<Vec<(usize, Duration)>>> = Arc::new(Mutex::new(Vec::new()));
    let mut lanes = Vec::new();
    for (n, id) in ids.iter().cloned().enumerate() {
        let agent = agent.clone();
        let released = released.clone();
        let ticket = agent.send_order.join(chat, &id).expect("a fresh place");
        lanes.push(tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(n as u64)).await;
            let t0 = Instant::now();
            agent.await_send_order(chat, &id, "job").await;
            released
                .lock()
                .expect("the releases")
                .push((n + 1, t0.elapsed()));
            drop(ticket);
        }));
    }
    join_lanes(lanes).await;

    let released = released.lock().expect("the releases").clone();
    assert_eq!(
        released.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
        (1..=5).collect::<Vec<_>>(),
        "the lanes behind the approval wait were released out of send order"
    );
    for (n, waited) in &released {
        assert_eq!(
            *waited, SEND_ORDER_WAIT,
            "lane {n} waited {waited:?}, so the valve does not bound every waiter"
        );
        assert!(
            *waited < INLINE_APPROVAL_WAIT,
            "lane {n} waited {waited:?}, which is the approval wait and not the valve"
        );
    }
    assert!(
        agent.approval_pending_for(chat).await,
        "the card was decided meanwhile, so the head was not really held"
    );
    assert_eq!(
        agent.send_order.waiting(chat),
        1,
        "only the head is still owed an answer"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Interaction 3 with #2761: can its queued-behind row be written after the answer it describes?
///
/// It falls the safe way, and not by timing. The row is posted through `post_chat_line`, which
/// this FIFO does not gate — so the question is a fair one — but the row exists only when
/// `job_lock_blocker` found another job holding a key this one needs, and the new job's own task
/// then parks in `acquire_job_locks` on that same key. The thing that makes the notice true is
/// the thing that stops the answer: while the blocker holds, the job cannot start, so it has
/// nothing to say yet.
///
/// That is what this case measures. The notice is posted, and the job's step is read back as
/// "Waiting for <blocker>" — written inside the lock wait, so it is proof from the job's own
/// task that it is parked there and not running.
///
/// The FIFO is no part of this and must not be claimed as one: the notice and the answer are
/// both replies to the same user message, and this queue orders messages against each other, not
/// rows inside one. The gate in `write_job_result` is checked here to pass straight through for
/// exactly that reason. It cannot make the pair worse either, since it can only ever delay the
/// answer, which is the row that has to come second.
#[tokio::test]
async fn the_queued_behind_row_cannot_follow_the_answer_it_describes() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATSENDORDER0000037";
    let host = agent.sync.open_room(chat).await.unwrap();
    let (ask, _cmd) = send_cmd(&host, "fix the failing parser tests in alpha/").await;
    let _turn = agent.test_hold_turn_for(chat, &ask).await;
    let holding = agent.send_order.join(chat, &ask).expect("the ask");
    assert!(
        agent
            .send_order
            .wait_turn(chat, &ask, SEND_ORDER_WAIT)
            .await
    );
    std::fs::create_dir_all(agent.cfg.work_dir.join("alpha").join(".git")).unwrap();

    // Another job already holds the keys the new one needs, which is the only way the notice is
    // written at all.
    let holder = JobSpec {
        id: "job-holder".into(),
        title: "Reformatting everything".into(),
        kind: "code".into(),
        brief: "Run the formatter over the project.".into(),
        touches: vec!["path:/".into(), "git".into()],
        attempt: 0,
    };
    agent
        .store
        .insert_job(
            &holder.id,
            chat,
            &ask,
            &holder.title,
            "code",
            &holder.brief,
            &holder.touches,
        )
        .unwrap();
    assert!(!agent.acquire_job_locks(&holder).await.is_empty());

    let started = agent
        .hook_start_job(
            "Fixing the alpha tests",
            "Fix the failing parser tests in alpha/, run the suite, and return the diff.",
            "code",
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(started["queued_behind"]["job_id"], "job-holder");
    let queued_job = started["job_id"].as_str().expect("the job id").to_owned();
    let notice = replies_to(&host, &ask)
        .await
        .into_iter()
        .find(|m| {
            m.rest
                .get("deliveryKey")
                .and_then(Value::as_str)
                .is_some_and(|k| k == format!("job:{queued_job}:queued"))
        })
        .expect("the queued-behind notice was posted");
    assert!(
        notice.text().contains("is queued behind"),
        "{:?}",
        notice.text()
    );

    // The barrier, from the job's own task: it is parked in `acquire_job_locks`, which is the
    // step it writes while it waits there. A job parked at the lock has run nothing and so has
    // no answer to post.
    let mut step = String::new();
    for _ in 0..200 {
        step = agent
            .store
            .job(&queued_job)
            .unwrap()
            .map(|j| j.last_step)
            .unwrap_or_default();
        if step == format!("Waiting for {}", holder.title) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        step,
        format!("Waiting for {}", holder.title),
        "the queued job is not parked on the blocker's lock, so nothing orders its answer"
    );
    assert!(
        !replies_to(&host, &ask).await.iter().any(|m| m
            .rest
            .get("decision")
            .and_then(Value::as_str)
            == Some("job")),
        "the queued job answered while it was still parked on the lock"
    );

    // And when the answer does come, it comes after. The send order adds nothing to it — the
    // notice and the answer are the same message's rows — and that is checked rather than
    // assumed, because a wait here would be a wait for this message's own turn.
    let t0 = Instant::now();
    agent
        .write_job_result(
            chat,
            &host,
            &ask,
            "ask-fix-the-failing-parser",
            "done",
            "The parser tests pass now.",
            None,
        )
        .await
        .unwrap();
    assert!(
        t0.elapsed() < Duration::from_secs(1),
        "the answer waited for its own message's turn"
    );
    let rows = replies_to(&host, &ask).await;
    let place = |id: &str| rows.iter().position(|m| m.id == id).expect("the row");
    let answer = rows
        .iter()
        .find(|m| m.rest.get("decision").and_then(Value::as_str) == Some("job"))
        .expect("the job's answer");
    assert!(
        place(&notice.id) < place(&answer.id),
        "the user read \"queued behind\" about an answer they already had"
    );

    agent.release_job_locks(&holder.id).await;
    drop(holding);
    let _ = std::fs::remove_dir_all(&root);
}

/// The answer still lands when the chat's room is dropped while that answer waits its turn.
///
/// A lane resolves its `SharedHost` once — a job task clones it before it spawns — and then
/// holds it across the work and across this wait. Nothing in the hub can see the send order
/// (`send_order.rs` knows nothing of rooms, by design), so a room with a reply queued looks as
/// idle as an empty one, and a write through the handle resolved earlier would land in a
/// `ChatDoc` the hub has let go of: the row is in memory, the user never sees it, and nothing
/// fails. `Agent::room_for_reply` takes the handle again after the wait, which is what this
/// asserts, in the terms the user reads — the answer is in the chat.
///
/// The drop here is the shape a cap or idle eviction has: persisted, out of the hub's map, and
/// *not* `closed`, since `closed` is what marks a chat this agent was removed from and must
/// never be reopened into (#1440). Nothing in the tree evicts rooms yet, so this is the whole
/// cost of the case — two lines, and no cap or timer to drive.
#[tokio::test]
async fn an_answer_lands_in_a_room_that_was_dropped_while_it_waited() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATSENDORDER0000038";
    let dropped = agent.sync.open_room(chat).await.unwrap();
    let older = ulid();
    tokio::time::sleep(Duration::from_millis(2)).await;
    let younger = ulid();
    for (id, text) in [(&older, "the long one"), (&younger, "reply with only 2")] {
        dropped
            .lock()
            .await
            .doc
            .append_user_message(id, text, "dev-mac", &UserMessageOpts::default())
            .unwrap();
    }
    let holding = agent
        .send_order
        .join(chat, &older)
        .expect("the older message");
    assert!(
        agent
            .send_order
            .wait_turn(chat, &older, SEND_ORDER_WAIT)
            .await
    );
    let _place = agent
        .send_order
        .join(chat, &younger)
        .expect("the ask behind it");

    // The job task carries the handle it resolved before any of this.
    let job = {
        let agent = agent.clone();
        let host = dropped.clone();
        let younger = younger.clone();
        tokio::spawn(async move {
            agent
                .write_job_result(
                    chat,
                    &host,
                    &younger,
                    "ask-reply-with-only-2",
                    "done",
                    "The answer is 2.",
                    None,
                )
                .await
        })
    };
    // Long enough for the task to be past its model call and sitting in the wait, which the
    // empty reply set below is the check on.
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(
        replies_to(&dropped, &younger).await.is_empty(),
        "the answer was written before the room was dropped, so this measured nothing"
    );
    agent.sync.persist(&dropped).await.unwrap();
    // Dropped the way the idle close and the cap drop a room, flag and all, rather than by
    // reaching into the map: the real closer does mark the handle, and the point of the record
    // below is that marking the handle is no longer what decides anything.
    agent.sync.test_close_cold(chat).await;
    assert!(agent.sync.host(chat).await.is_none());
    // The contract `Agent::room_for_reply` is correct under, written out where the drop happens:
    // a room let go of because it is cold records nothing against the chat, because that record
    // is the only thing that tells this from a chat the agent was removed from. A closer that
    // records here turns every reply in this window into a row nobody will ever read.
    assert!(
        agent.sync.may_open(chat).await,
        "a cold drop recorded the chat as left for good, which reads as a removal and loses the reply"
    );

    drop(holding);
    job.await.expect("the job task").expect("the answer");

    let live = agent
        .sync
        .host(chat)
        .await
        .expect("the answer did not reopen the chat's room");
    assert!(
        !Arc::ptr_eq(&live, &dropped),
        "the hub handed back the handle it had dropped"
    );
    let rows = replies_to(&live, &younger).await;
    assert_eq!(
        rows.len(),
        1,
        "the answer is not in the chat the user reads"
    );
    assert!(
        rows[0].has_visible_content(),
        "the reopened room has the answer's row but not its words"
    );
    assert_eq!(
        rows[0].rest.get("decision").and_then(Value::as_str),
        Some("job"),
        "the row in the reopened room is not the job's answer"
    );
    assert!(
        replies_to(&dropped, &younger).await.is_empty(),
        "the answer went into the doc the hub had let go of"
    );

    // And the loss itself, because without it the empty set above reads the same as a write that
    // never happened. The dropped handle is a live `ChatDoc`: it takes the row, reports nothing
    // wrong, and the row is nowhere the user can reach — no session pushes that doc and no
    // reopen loads it. That is what makes this class of bug invisible, and it is the thing the
    // re-acquire above is for.
    let lost = ulid();
    {
        let h = dropped.lock().await;
        h.doc
            .start_assistant_message(&lost, &younger, &agent.cfg.device_id);
        let part = h
            .doc
            .add_text_part(&lost, "text")
            .expect("the dropped doc took a text part");
        h.doc
            .append_text(&lost, &part, "A second answer, through the stale handle.");
        h.doc.finish_message(&lost, MessageStatus::Done);
    }
    assert!(
        replies_to(&dropped, &younger)
            .await
            .iter()
            .any(|m| m.id == lost && m.has_visible_content() && m.status == "done"),
        "the stale write failed, so an absent row there would prove nothing either way"
    );
    assert!(
        !replies_to(&live, &younger)
            .await
            .iter()
            .any(|m| m.id == lost),
        "the stale doc and the live room are the same document, so nothing was ever at risk"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The other half of the same rule, and the reason the re-acquire is not a plain `open_room`:
/// a chat this agent was *removed* from is never reopened to post into.
///
/// `close_room` is that removal (`people_agent_removed`), and it records the chat as one this
/// box may not open again. Reopening there would spawn a room task the relay answers 403 to,
/// every 16 seconds, until the box restarts — which is #1440, already fixed once. So the answer
/// to a chat we have left goes nowhere, exactly as it does today, and the room stays shut.
#[tokio::test]
async fn an_answer_does_not_reopen_a_chat_the_agent_was_removed_from() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATSENDORDER0000039";
    let left = agent.sync.open_room(chat).await.unwrap();
    let (ask, _cmd) = send_cmd(&left, "reply with only 2").await;
    agent.sync.close_room(chat).await;
    assert!(
        !agent.sync.may_open(chat).await,
        "close_room must record the chat, since that is what tells a removal from an eviction"
    );

    let journal = Journal::default();
    {
        let _logs = journal.capture();
        agent
            .write_job_result(
                chat,
                &left,
                &ask,
                "ask-reply-with-only-2",
                "done",
                "The answer is 2.",
                None,
            )
            .await
            .unwrap();
    }

    assert!(
        agent.sync.host(chat).await.is_none(),
        "the answer reopened a chat the agent had been removed from"
    );
    // Declining to reopen is never silent, which is what makes a broken `closed` contract
    // findable at all: if a cold drop ever marks a room closed, this is the line that says a
    // reply was lost and the counter beside it is what a real-box measurement would see.
    let logged = journal.text();
    assert!(
        logged.contains("nothing will push it")
            && logged.contains("a cold drop must record nothing"),
        "a dropped reply was declined without saying so: {logged}"
    );
    // The write succeeded and the row is unreachable, which is the same loss as above and here
    // the right outcome: the agent is not in this chat any more, and the only alternative is a
    // room that redials a relay refusing it.
    let rows = replies_to(&left, &ask).await;
    assert_eq!(
        rows.len(),
        1,
        "the answer was written somewhere other than the handle the lane held"
    );
    assert!(
        rows[0].has_visible_content(),
        "the write into the closed handle did not land, so this proves nothing"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A voice send keeps its old way in: it must be able to preempt the turn of a caller who has
/// hung up, so it never queues behind the chat's typed messages.
#[tokio::test]
async fn a_voice_send_does_not_queue_behind_typed_messages() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATSENDORDER0000031";
    let _host = agent.sync.open_room(chat).await.unwrap();
    let typed = ulid();
    let _held = agent.send_order.join(chat, &typed).expect("typed");
    let spoken = ulid();
    let _place = agent.send_order.join(chat, &spoken).expect("spoken");
    let got = tokio::time::timeout(
        Duration::from_secs(1),
        agent.acquire_turn_lock(chat, &spoken, true),
    )
    .await;
    assert!(
        got.is_ok(),
        "a voice send waited for an older typed message's answer"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Remove, re-add, then a cold drop: the one sequence where a flag on the handle and a record on
/// the chat disagree, and the record is the one that is right.
///
/// `H1` is resolved before the wait, as this whole lane is about. The chat is left, so `H1` is
/// marked and the chat recorded; the agent is re-added, so the record is cleared and a fresh
/// `H2` opens; then the chat goes cold, so `H2` leaves the hub with nothing recorded against it.
/// The lane resumes on `H1`, finds the hub empty, and must reopen — the agent is a live member of
/// this chat and its answer is owed.
///
/// A handle-local flag cannot get this right. `H1.closed` is still true from the removal, so
/// reading it declines the reply and counts `reply_dropped_closed_room` against a chat nobody
/// left. That is why the decision reads [`crate::sync::SyncHub::may_open`], which is keyed on
/// the chat and cleared by the rejoin, and it is what this test discriminates: restore the flag
/// read and it fails here while every other case still passes.
#[tokio::test]
async fn a_reply_through_a_stale_handle_survives_a_rejoin_and_a_cold_drop() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATSENDORDER0000040";
    let stale = agent.sync.open_room(chat).await.unwrap();
    let (ask, _cmd) = send_cmd(&stale, "reply with only 2").await;
    agent.sync.persist(&stale).await.unwrap();

    // Removed from the chat: the handle this lane carries is marked, and so is the chat.
    agent.sync.close_room(chat).await;
    assert!(
        stale.lock().await.closed,
        "premise: the removal marked the handle the lane still holds"
    );
    assert!(
        !agent.sync.may_open(chat).await,
        "premise: and recorded the chat, so no writer may open it"
    );

    // Re-added, on the relay's word. `note_people_chats` is what calls this on a live box, and
    // the ordering it has to keep is `a_rejoin_lifts_the_refusal_before_the_room_it_opens_can_be_seen`.
    agent.sync.mark_rejoined(chat).await;
    let fresh = agent
        .sync
        .open_room(chat)
        .await
        .expect("the re-add can open the room again");
    agent.sync.persist(&fresh).await.unwrap();

    // And now the chat goes cold, which records nothing: the hub is empty and the only marked
    // handle left in the world is the stale one this lane holds.
    agent.sync.test_close_cold(chat).await;
    assert!(
        agent.sync.host(chat).await.is_none(),
        "premise: the cold drop emptied the hub for this chat"
    );
    assert!(
        agent.sync.may_open(chat).await,
        "premise: and left the chat openable, because the agent is still in it"
    );

    let journal = Journal::default();
    {
        let _logs = journal.capture();
        agent
            .write_job_result(
                chat,
                &stale,
                &ask,
                "ask-reply-with-only-2",
                "done",
                "The answer is 2.",
                None,
            )
            .await
            .unwrap();
    }

    let live = agent
        .sync
        .host(chat)
        .await
        .expect("the reply reopened the room it is owed to");
    assert!(
        replies_to(&live, &ask)
            .await
            .iter()
            .any(|m| m.has_visible_content() && m.status == "done"),
        "the answer is not in the room the relay reads, so it went through the stale handle"
    );
    // The warn is emitted immediately before `post_timing("reply_dropped_closed_room", …)`, so
    // its absence here is the counter's absence.
    assert!(
        !journal.text().contains("nothing will push it"),
        "a reply to a chat the agent is a member of was declined and counted as dropped"
    );
    drop(fresh);
    let _ = std::fs::remove_dir_all(&root);
}
