//! Bug U: four asks for a background job give four jobs.
//!
//! Jacob, Test5 round 6 (2026-11-05, agentd 0.1.193, scenario `r6lock`, side chat
//! `01M6ME9A7EGV5PJ7DZW43AHTN5`). Four messages two seconds apart, each ending "as a background
//! job (not inline)": `itsdangerous`, `markupsafe`, `pluggy`, `more-itertools`. Only the first
//! and the last became jobs. The middle two arrived while a turn held the lock, were read by the
//! quick lane, came back with a line that could not stand ("quick reply did not end a sentence;
//! silent queue") and were handed to `start_job_for_send`, which answers an ask with a plain
//! subagent — no job row, no job id, nothing `list_jobs` or `stop_job` can name. The user asked
//! four times for a job and got two.
//!
//! The test drives the burst through the real entry point, [`crate::agent::Agent::handle_command`],
//! with a turn on the lock for the whole of it — the state that made the two middle asks take
//! the quick lane in the first place.

use std::time::Duration;

use arbos_core::doc::{CommandView, UserMessageOpts};
use arbos_core::ids::{now_ms, ulid};
use serde_json::{json, Value};

use crate::status::tests::test_agent;
use crate::sync::SharedHost;

/// A user message and the `send` command the apps push with it.
fn send_cmd(host: &SharedHost, text: &str) -> (String, CommandView) {
    let msg_id = ulid();
    let h = host.try_lock().expect("host free");
    h.doc
        .append_user_message(&msg_id, text, "dev-mac", &UserMessageOpts::default())
        .expect("user message");
    (
        msg_id.clone(),
        CommandView {
            id: ulid(),
            kind: "send".into(),
            payload: json!({ "messageId": msg_id }),
            issued_by: "dev-mac".into(),
            issued_at: now_ms(),
            status: "pending".into(),
        },
    )
}

/// The four packages Jacob asked about, in the words he used.
const BURST: [&str; 4] = ["itsdangerous", "markupsafe", "pluggy", "more-itertools"];

/// Seconds between the messages of the burst, as sent.
const APART: Duration = Duration::from_secs(2);

#[tokio::test]
async fn four_forced_job_asks_two_seconds_apart_give_four_jobs() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATFORCEDJOB000001";
    let host = agent.sync.open_room(chat).await.unwrap();
    // A turn holds the lock for the whole burst, as the first ask's turn did.
    let _busy = agent.test_hold_turn(chat).await;
    let mut asks = Vec::new();
    for (i, pkg) in BURST.iter().enumerate() {
        if i > 0 {
            tokio::time::sleep(APART).await;
        }
        let (msg_id, cmd) = send_cmd(
            &host,
            &format!("Install {pkg} and report its version, as a background job (not inline)."),
        );
        agent.handle_command(chat, cmd).await.unwrap();
        asks.push((*pkg, msg_id));
    }
    let jobs = agent.store.jobs_for_chat(chat).unwrap();
    let ids: std::collections::BTreeSet<&str> = jobs.iter().map(|j| j.id.as_str()).collect();
    assert_eq!(
        ids.len(),
        4,
        "four asks for a background job, four job ids: {:?}",
        jobs.iter()
            .map(|j| (j.title.as_str(), j.ask_message_id.as_str()))
            .collect::<Vec<_>>()
    );
    // One job per ask, each carrying that ask's own words — not three jobs for one message, and
    // not one job that merged the burst.
    for (pkg, msg_id) in &asks {
        let job = jobs
            .iter()
            .find(|j| &j.ask_message_id == msg_id)
            .unwrap_or_else(|| panic!("no job for the {pkg} ask"));
        assert!(job.brief.contains(*pkg), "{}", job.brief);
        assert!(
            job.title.contains(*pkg),
            "the status line names the ask: {}",
            job.title
        );
    }
    // Nothing was answered inline instead: no assistant row in this chat holds words.
    let spoken: Vec<String> = {
        let h = host.try_lock().expect("host free");
        h.doc
            .messages()
            .into_iter()
            .filter(|m| m.role == "assistant")
            .map(|m| m.text().trim().to_owned())
            .filter(|t| !t.is_empty())
            .collect()
    };
    assert!(
        spoken.is_empty(),
        "the asks became jobs, not inline answers: {spoken:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The quick lane is what swallowed two of the four, so it has to stand aside for a forced ask
/// and keep answering everything else. One chat, one held turn, two messages: the status
/// question is answered in a bubble as before; the forced ask writes no bubble and becomes a
/// job.
#[tokio::test]
async fn a_forced_ask_skips_the_quick_lane_and_a_status_question_still_takes_it() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATFORCEDJOB000002";
    let host = agent.sync.open_room(chat).await.unwrap();
    let _busy = agent.test_hold_turn(chat).await;
    let (asked, cmd) = send_cmd(&host, "what is going on now?");
    assert_eq!(
        agent.quick_reply(chat, &host, &cmd).await,
        crate::status::QuickOutcome::Answered,
        "a status question is still the quick lane's"
    );
    let (forced, cmd) = send_cmd(
        &host,
        "Install pluggy and report its version, as a background job (not inline).",
    );
    assert_eq!(
        agent.quick_reply(chat, &host, &cmd).await,
        crate::status::QuickOutcome::Queued,
        "a forced ask is not the quick lane's"
    );
    let rows = |msg_id: &str| -> Vec<String> {
        let h = host.try_lock().expect("host free");
        h.doc
            .messages()
            .into_iter()
            .filter(|m| {
                m.role == "assistant"
                    && m.rest.get("inReplyTo").and_then(Value::as_str) == Some(msg_id)
            })
            .map(|m| m.text().trim().to_owned())
            .filter(|t| !t.is_empty())
            .collect()
    };
    assert_eq!(rows(&asked).len(), 1, "the status answer is a bubble");
    assert!(
        rows(&forced).is_empty(),
        "the forced ask gets no quick bubble: {:?}",
        rows(&forced)
    );
    assert!(
        agent
            .start_job_for_send(chat, &host, &cmd, &mut None)
            .await
            .expect("job start"),
        "the queued forced ask becomes a job"
    );
    let jobs = agent.store.jobs_for_chat(chat).unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].ask_message_id, forced);
    let _ = std::fs::remove_dir_all(&root);
}

/// The ticket contract of bug O (`crate::send_order`): whoever owes the answer holds that
/// message's place in its chat's send order until it is no longer owed. The bound is the owning
/// **task**, not the answer's row — release is `SendTicket`'s `Drop`, and the ticket is a local
/// of the spawned body, so every ending releases it, including one that posts nothing. The test
/// below this one covers that ending.
///
/// A forced job is the case that makes the contract bite. `handle_command` returns having posted
/// nothing at all — the four-job test above asserts exactly that — so the answer to the ask is
/// the job's own result turn, which runs minutes later inside the job's task. The ticket
/// therefore travels with the job (`spawn_subagent_task`) instead of being dropped when
/// `handle_command` returns.
///
/// This test asserts the guarantee rather than the job count, in both directions. Were the
/// ticket dropped at the end of `handle_command`, the ask would leave the queue at job start:
/// `waiting` would read 1 instead of 2, the result turn would be free to post in front of the
/// older message, and — the sharpest of the three — the younger message would become the head
/// the moment the older one answered, with the forced answer still unposted. That is bug O
/// restored for the one path users ask for in words.
#[tokio::test]
async fn a_forced_jobs_answer_keeps_its_place_in_the_send_order() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATFORCEDJOB000004";
    let host = agent.sync.open_room(chat).await.unwrap();
    let _busy = agent.test_hold_turn(chat).await;
    // An older message, still owed its answer by whichever lane took it. ULIDs carry send order
    // in their timestamp, so each id here is minted a millisecond after the last.
    let older = ulid();
    tokio::time::sleep(Duration::from_millis(2)).await;
    let older_ticket = agent.send_order.join(chat, &older).expect("the older ask");
    let (forced, cmd) = send_cmd(
        &host,
        "Install pluggy and report its version, as a background job (not inline).",
    );
    agent.handle_command(chat, cmd).await.unwrap();
    assert_eq!(
        agent.store.jobs_for_chat(chat).unwrap().len(),
        1,
        "the ask became a job"
    );
    assert_eq!(
        agent.send_order.waiting(chat),
        2,
        "the ask is still owed an answer after handle_command returned, so it keeps its place"
    );
    // Its own answer waits behind the older message.
    assert!(!agent.send_order.is_turn(chat, &forced));
    assert!(
        tokio::time::timeout(
            Duration::from_millis(300),
            agent.await_send_order(chat, &forced, "job_result"),
        )
        .await
        .is_err(),
        "the job's result turn must not post in front of a message sent before it"
    );
    // A younger message waits behind the forced ask, not just behind the older one.
    tokio::time::sleep(Duration::from_millis(2)).await;
    let younger = ulid();
    let _younger_ticket = agent
        .send_order
        .join(chat, &younger)
        .expect("the younger ask");
    assert_eq!(agent.send_order.ahead(chat, &younger), 2);
    // The older message is answered. The forced ask takes the floor; the younger one still waits
    // for it, because the answer it is waiting for has not been posted yet.
    drop(older_ticket);
    assert!(
        agent.send_order.is_turn(chat, &forced),
        "the forced ask is the head once the older message is answered"
    );
    assert!(
        !agent.send_order.is_turn(chat, &younger),
        "a younger message cannot answer in front of a forced job whose answer is still owed"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The other end of the same contract: the place is held for as long as the job's task lives,
/// and no longer. A job that ends without posting anything — a failed start, a refusal, a cancel
/// — must still let the chat move, because nobody owes that ask an answer any more.
///
/// The cancel is the ending that is quick and certain to produce no answer: cancelled before its
/// worker could start, the task settles its rows, skips the inline fallback that would otherwise
/// answer the ask, and ends. The job row is asserted to still read `cancelled` so a lost race
/// (the worker failing first, which writes `failed`) cannot leave this test quietly passing on
/// the wrong path.
///
/// Were the place released only where an answer's row is posted, this test would hang on the
/// five-second timeout: the forced ask would hold its place for the life of the process and the
/// younger message behind it would never get the floor.
#[tokio::test]
async fn a_forced_jobs_place_is_released_when_its_task_ends_without_an_answer() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATFORCEDJOB000005";
    let host = agent.sync.open_room(chat).await.unwrap();
    let _busy = agent.test_hold_turn(chat).await;
    let (_forced, cmd) = send_cmd(
        &host,
        "Install markupsafe and report its version, as a background job (not inline).",
    );
    agent.handle_command(chat, cmd).await.unwrap();
    let job = agent.store.jobs_for_chat(chat).unwrap().remove(0);
    assert_eq!(
        agent.send_order.waiting(chat),
        1,
        "the handler has returned and the place is held by the job's task"
    );
    // Cancelled before the worker could fail of its own accord, which is what makes the ending
    // certain: a failed worker would run the inline fallback and answer the ask after all.
    agent
        .store
        .set_job_state(&job.id, "cancelled", None)
        .unwrap();
    // A message sent after the forced ask, which cannot take the floor until that ask's place
    // is released.
    let younger = ulid();
    let _younger_ticket = agent
        .send_order
        .join(chat, &younger)
        .expect("the younger ask");
    let freed = tokio::time::timeout(Duration::from_secs(5), async {
        while !agent.send_order.is_turn(chat, &younger) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert!(
        freed.is_ok(),
        "the job's task ended, so the chat must move on: waiting={}",
        agent.send_order.waiting(chat)
    );
    assert_eq!(
        agent
            .store
            .job(&job.id)
            .unwrap()
            .expect("the job row")
            .state,
        "cancelled",
        "the job ended by the cancel, which is the ending that posts no answer"
    );
    let spoken: Vec<String> = {
        let h = host.lock().await;
        h.doc
            .messages()
            .into_iter()
            .filter(|m| m.role == "assistant")
            .map(|m| m.text().trim().to_owned())
            .filter(|t| !t.is_empty())
            .collect()
    };
    assert!(
        spoken.is_empty(),
        "the place was released without any answer being posted: {spoken:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The per-chat cap (three) is what would have refused the fourth ask. A job the user asked for
/// in words passes it; a job the model starts off its own judgement still meets it.
#[tokio::test]
async fn the_per_chat_cap_refuses_the_models_fourth_job_and_not_the_users() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATFORCEDJOB000003";
    let host = agent.sync.open_room(chat).await.unwrap();
    let _busy = agent.test_hold_turn(chat).await;
    let brief = "Find the three best-reviewed surf schools near Santa Teresa; return each with its page URL, its prices, the sources and what could not be checked.";
    for title in [
        "Searching for schools",
        "Comparing prices",
        "Checking dates",
    ] {
        agent
            .hook_start_job(title, brief, "research", vec![])
            .await
            .expect("within the cap");
    }
    let err = agent
        .hook_start_job("A fourth one", brief, "research", vec![])
        .await
        .expect_err("the cap holds for a job the model chose");
    assert!(err.contains("cap"), "{err}");
    let (msg_id, cmd) = send_cmd(
        &host,
        "Install more-itertools and report its version, as a background job (not inline).",
    );
    assert!(
        agent
            .start_job_for_send(chat, &host, &cmd, &mut None)
            .await
            .expect("job start"),
        "the user asked for this one in words"
    );
    let jobs = agent.store.jobs_for_chat(chat).unwrap();
    assert_eq!(jobs.len(), 4);
    assert!(jobs.iter().any(|j| j.ask_message_id == msg_id));
    let _ = std::fs::remove_dir_all(&root);
}
