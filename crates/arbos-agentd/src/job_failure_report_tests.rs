//! Bug AJ: a job that loses every worker tells the user so, instead of vanishing.
//!
//! Jacob on Test5, round 16 (2026-11-05, agentd 0.1.197 / ef90488f8, side chat
//! 01M6ME9A7EGV5PJ7DZW43AHTN5, scenario `r16burst`). Eight "as a background job (not inline)"
//! compute asks went in together. The workspace cgroup went over `memory.max`, the kernel's OOM
//! killer took the **main** pi child five times over fifty seconds, and every worker's pipe
//! broke with it (`Broken pipe (os error 32)`, `pi main child exited mid-turn`).
//!
//! Two of the eight answers posted. **Six posted nothing at all** — not a result, not a failure,
//! not an error row. Five of those six left a `jobs` row reading `failed`, with the right error
//! string in it, that the user could not see. The box then answered twenty-two trivia questions
//! (18:37:57–18:39:18) and withdrew its own degraded mark, still owing six answers.
//!
//! **The inversion this pins.** There are two job paths and the reporting guarantee is on the
//! wrong one:
//!
//! - `start_job_for_send` (an ask run beside a busy turn) gets **no** retry, and ends by writing
//!   a `job_failed` row into the doc whenever nothing else answered the ask. It always speaks.
//! - `spawn_subagent_task` with a `JobSpec` (a real `start_job`) **does** get the crash-loop
//!   protection — one fresh worker, and `wait_for_live_pi` before it starts. But it settles its
//!   worker rows deliberately quietly and hands every user-facing word to
//!   `run_job_fallback_turn`. That turn runs through `drive_turn`, so it needs the main pi
//!   child: the one process most likely to be dead at the moment it is needed. When it fails,
//!   the error is logged and the task returns. Nothing reaches the chat.
//!
//! So the path with the retry protection is the one that can go silent, because its only
//! reporting mechanism depends on the thing that just died.
//!
//! The box already records whether an answer reached the chat — `jobs.delivered_key`, set by all
//! three delivery paths — and has never once read it. These cases make that field the condition.
//!
//! No pi binary exists in a test box, and `shutting_down` makes every wait for one give up at
//! once rather than holding a case for `PI_RESPAWN_WAIT`. That is AJ's box: a job whose workers
//! cannot run and whose inline fallback cannot run either.

use std::sync::atomic::Ordering;
use std::time::Duration;

use arbos_core::doc::{MessageView, UserMessageOpts};
use arbos_core::ids::{now_ms, ulid};
use serde_json::Value;
use tokio::time::Instant;

use crate::status::tests::test_agent;
use crate::sync::SharedHost;

/// How long a case waits for the worker task to run out of workers and settle. Two failed
/// workers and a failed fallback turn, none of which waits for anything in a test box.
const SETTLE: Duration = Duration::from_secs(30);

/// The rows a client would actually draw for `msg_id`: `hidden` rows are skipped by both apps,
/// so a hidden row is not a row the user saw — which is the whole measurement of this bug.
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

async fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < SETTLE {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("{what} did not happen within {SETTLE:?}");
}

/// One of r16burst's eight asks, on a box whose pi child cannot be had: the job starts, both of
/// its workers fail, and the inline fallback fails too.
async fn burst_ask(
    chat: &str,
) -> (
    std::sync::Arc<crate::agent::Agent>,
    std::path::PathBuf,
    SharedHost,
    String,
    String,
) {
    let (agent, root) = test_agent().await;
    agent
        .store
        .upsert_chat(chat, "r16burst", false, now_ms())
        .unwrap();
    let host = agent.sync.open_room(chat).await.unwrap();
    // AJ's box, after the OOM killer: no main pi child, and no prospect of one.
    agent.shutting_down.store(true, Ordering::SeqCst);

    let ask = ulid();
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(
                &ask,
                "As a background job (not inline), compute 7000! and post the number of digits.",
                "dev-mac",
                &UserMessageOpts::default(),
            )
            .expect("user message");
    }

    let started = agent
        .start_job(
            chat,
            &host,
            &ask,
            "Computing 7000 factorial",
            "Compute 7000! exactly and report how many decimal digits the result has.",
            "other",
            vec![],
            // The user asked for a background job in words, and the ack's place in the send
            // order is owed by another lane — so nothing is posted at job start. Every visible
            // row a case finds below was therefore written by the failure path.
            true,
            &mut None,
        )
        .await
        .expect("the job starts");
    let job_id = started["job_id"]
        .as_str()
        .expect("the job row has an id")
        .to_owned();

    (agent, root, host, ask, job_id)
}

/// The measurement. Five of r16burst's rows ended `failed`, with the error in them, and the
/// user was told nothing. A job that has run out of every way to do the work owes the chat a
/// row saying so.
#[tokio::test(flavor = "multi_thread")]
async fn a_job_that_loses_every_worker_tells_the_user_it_died() {
    let chat = "01TESTCHATJOBLOSTWORKER01";
    let (agent, root, host, ask, job_id) = burst_ask(chat).await;

    let (s, j) = (agent.clone(), job_id.clone());
    wait_until("the job ran out of workers and settled", move || {
        s.store
            .job(&j)
            .ok()
            .flatten()
            .is_some_and(|r| r.state == "failed")
    })
    .await;

    // The half that already worked in r16burst: the store knew. This is the control — without
    // it a silent chat could just mean the job never ran.
    let row = agent.store.job(&job_id).unwrap().expect("the job row");
    assert_eq!(row.state, "failed");
    assert!(
        !row.error.clone().unwrap_or_default().trim().is_empty(),
        "the row records why it died: {row:?}"
    );

    // The half that did not. `wait_until` above only proves the row settled; the row can settle
    // a beat before the reporting path runs, so the chat is given the same budget.
    let h = host.clone();
    let a = ask.clone();
    wait_until("the chat was told the job died", move || {
        !visible_replies(&h, &a).is_empty()
    })
    .await;

    let told = visible_replies(&host, &ask);
    assert_eq!(
        told.len(),
        1,
        "exactly one row, not a retry's worth: {:?}",
        told.iter().map(MessageView::text).collect::<Vec<_>>()
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// `jobs.delivered_key` is how the box knows an answer reached the chat. Every delivery path
/// sets it and, until this bug, nothing ever read it — which is exactly why five rows could sit
/// `failed` and undelivered with no part of the box noticing.
///
/// Reading it is what keeps the backstop from speaking twice: a job whose answer did land must
/// come out of the failure path silent.
#[tokio::test(flavor = "multi_thread")]
async fn the_failure_row_is_recorded_as_delivered_so_it_is_written_once() {
    let chat = "01TESTCHATJOBLOSTWORKER02";
    let (agent, root, host, ask, job_id) = burst_ask(chat).await;

    let (s, j) = (agent.clone(), job_id.clone());
    wait_until("the job settled and the chat was told", move || {
        s.store
            .job(&j)
            .ok()
            .flatten()
            .is_some_and(|r| r.state == "failed" && r.delivered_key.is_some())
    })
    .await;

    let row = agent.store.job(&job_id).unwrap().expect("the job row");
    assert_eq!(
        row.delivered_key.as_deref(),
        Some(format!("job:{job_id}:lost").as_str()),
        "the row names which path answered for it, so a later sweep can tell it apart from a \
         result and from a fallback turn"
    );
    assert_eq!(
        visible_replies(&host, &ask).len(),
        1,
        "and the row it stands for is in the chat"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// A job row that has ended, with a chat holding whatever `rows` put in it: the state the
/// backstop has to judge. Driven directly rather than through a worker, because what these two
/// cases measure is the decision, and a live worker cannot be made to lose its race on cue.
async fn lost_job_with(
    chat: &str,
    rows: impl AsyncFnOnce(&std::sync::Arc<crate::agent::Agent>, &SharedHost, &str, &str),
) -> (
    std::sync::Arc<crate::agent::Agent>,
    std::path::PathBuf,
    SharedHost,
    String,
    String,
) {
    let (agent, root) = test_agent().await;
    agent
        .store
        .upsert_chat(chat, "r16burst", false, now_ms())
        .unwrap();
    let host = agent.sync.open_room(chat).await.unwrap();

    let ask = ulid();
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(
                &ask,
                "As a background job (not inline), compute 9^12000 and post the digit count.",
                "dev-mac",
                &UserMessageOpts::default(),
            )
            .expect("user message");
    }

    let job_id = ulid();
    let brief = "Compute 9^12000 exactly and report how many decimal digits it has.";
    agent
        .store
        .insert_job(
            &job_id,
            chat,
            &ask,
            "Computing 9^12000",
            "other",
            brief,
            &[],
        )
        .expect("the job row");
    agent
        .store
        .set_job_state(&job_id, "failed", Some("Broken pipe (os error 32)"))
        .expect("the worker died");

    rows(&agent, &host, &ask, &job_id).await;

    let spec = crate::agent::JobSpec {
        id: job_id.clone(),
        title: "Computing 9^12000".into(),
        kind: "other".into(),
        brief: brief.into(),
        touches: vec![],
        attempt: crate::agent::JOB_WORKER_RETRIES,
    };
    agent
        .report_lost_job(chat, &host, &ask, &spec, "Broken pipe (os error 32)")
        .await;

    (agent, root, host, ask, job_id)
}

/// Every forced ask in `r16burst` was acknowledged — "…is running in the background now, I'll
/// post the result here when it lands" — and then six of them died without another word. So the
/// ack is the one row that must never be read as the answer: treating it as one would leave the
/// bug exactly where it was, with a reassuring line and nothing behind it.
#[tokio::test(flavor = "multi_thread")]
async fn a_jobs_own_acknowledgement_is_not_mistaken_for_its_answer() {
    let chat = "01TESTCHATJOBLOSTWORKER03";
    let (agent, root, host, ask, job_id) = lost_job_with(chat, async |agent, host, ask, job_id| {
        agent
            .post_chat_line(
                host,
                "01TESTCHATJOBLOSTWORKER03",
                ask,
                &format!("job:{job_id}:ack"),
                "\"Computing 9^12000\" is running in the background now — I'll post the result here when it lands.",
            )
            .await;
    })
    .await;

    let rows = visible_replies(&host, &ask);
    assert_eq!(
        rows.len(),
        2,
        "the ack stands and the failure row joins it: {:?}",
        rows.iter().map(MessageView::text).collect::<Vec<_>>()
    );
    assert!(
        rows.iter()
            .any(|m| m.rest.get("deliveryKey").and_then(Value::as_str)
                != Some(format!("job:{job_id}:ack").as_str())),
        "one of them is not the receipt"
    );
    assert_eq!(
        agent
            .store
            .job(&job_id)
            .unwrap()
            .expect("the job row")
            .delivered_key
            .as_deref(),
        Some(format!("job:{job_id}:lost").as_str()),
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// The other side of the same judgement. A fallback turn that wrote prose and then failed has
/// already told the user; a second row would say it twice. The job is marked delivered instead,
/// so the unread-until-now `delivered_key` ends up true for both ways an answer can arrive.
#[tokio::test(flavor = "multi_thread")]
async fn a_job_whose_ask_was_already_answered_says_nothing_more() {
    let chat = "01TESTCHATJOBLOSTWORKER04";
    let (agent, root, host, ask, job_id) =
        lost_job_with(chat, async |agent, host, ask, _job_id| {
            agent
                .post_chat_line(
                    host,
                    "01TESTCHATJOBLOSTWORKER04",
                    ask,
                    "turn:inline",
                    "9^12000 has 11,452 digits.",
                )
                .await;
        })
        .await;

    let rows = visible_replies(&host, &ask);
    assert_eq!(
        rows.len(),
        1,
        "the answer that landed is the only row: {:?}",
        rows.iter().map(MessageView::text).collect::<Vec<_>>()
    );
    assert_eq!(
        agent
            .store
            .job(&job_id)
            .unwrap()
            .expect("the job row")
            .delivered_key
            .as_deref(),
        Some(format!("job:{job_id}:answered").as_str()),
        "and the row records that it was answered, so no later reader takes it for a lost job"
    );

    let _ = std::fs::remove_dir_all(&root);
}
