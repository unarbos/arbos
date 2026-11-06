//! Bug AG, Test5 rounds 13–19: an ask that said "produce exactly ONE CSV" ended with about
//! eight.
//!
//! Eight is not a random number. It is the number of corrections the user sent. Each correction
//! arrived while the work was in flight, and each one produced a file, because a correction that
//! cannot reach the worker already holding the task becomes a *new ask* — `start_job_for_send`
//! gives it its own worker, that worker redoes the job from scratch, and it posts its own CSV.
//! So this is a fan-out, not a prompt or a parsing fault: the words are read correctly and then
//! delivered to the wrong place.
//!
//! The box already knows what the right answer looks like. `steer_job_in` puts a correction into
//! the running job's session and marks the card that job already posted superseded —
//! "one ask, one file", in `correction_reached_worker`'s own words. Two gates kept corrections
//! off that path:
//!
//! 1. `handle_steer` only knew about turns. With no turn of this chat on the lock it fell
//!    through to `handle_send`, even when a job of that chat was holding the very task being
//!    corrected.
//! 2. `quick_reply` demoted a `[guide]` to a `[task]` whenever `snap.steerable` was false, and
//!    `steerable` is set only while a *turn* holds the lock (`status.rs`). With a job as the
//!    running work, every correction was demoted before it could reach gate 1 at all.
//!
//! A third, smaller one sits behind them and is tested here too: `mark_quick_noted` was spent by
//! a guide, although the ration it holds is the one-status-*bubble*-per-wait ration and a guide
//! writes no bubble. After one guide, every later message of the same wait returned early from
//! `already_quick_noted` with no model call and no decision row — unread, and queued. `r18steer`
//! shows it directly: four of its seven corrections have a `quick_reply` row and the three after
//! them have none.
//!
//! What stops each case passing falsely:
//!
//! - the routing case asserts an observable the old code cannot produce — the card already in
//!   the chat is marked superseded and the existing job carries the correction — rather than an
//!   outcome enum. `matches!(outcome, Steered | Queued)` accepts both the bug and the fix;
//! - it also counts the job rows of the chat before and after, so a "fix" that steered the job
//!   *and* started another worker would still fail;
//! - the demotion case reads the `quick_reply` decision row's own `side` field, which records
//!   what the lane decided. Asserting on the returned `QuickOutcome` would not: a guide that
//!   fails to steer and a task that queues both come back `Queued`;
//! - the ration case uses two *different* corrections and keys on each one's own message id, so
//!   it cannot pass by finding the first message's row twice.

use std::sync::Arc;
use std::time::Duration;

use arbos_core::doc::{CommandView, UserMessageOpts};
use arbos_core::ids::ulid;
use serde_json::{json, Value};

use crate::agent::Agent;
use crate::status::tests::test_agent;
use crate::sync::SharedHost;

const CHAT: &str = "01TESTCHATCORRECTROUTE0001";
const JOB: &str = "01TESTJOBCORRECTROUTE00001";
const WORKER: &str = "01TESTWORKERCORRECTROUTE01";
const ASK: &str = "01TESTASKCORRECTROUTE00001";
const ROW: &str = "01TESTROWCORRECTROUTEWRK01";

const CHAT2: &str = "01TESTCHATCORRECTROUTE0002";
const JOB2: &str = "01TESTJOBCORRECTROUTE00002";
const WORKER2: &str = "01TESTWORKERCORRECTROUTE02";
const ASK2: &str = "01TESTASKCORRECTROUTE00002";
const ROW2: &str = "01TESTROWCORRECTROUTEWRK02";

const CHAT3: &str = "01TESTCHATCORRECTROUTE0003";

/// A cap on one `quick_reply`, so a case that wedges fails instead of hanging the suite. It is
/// well over the lane's own waits: 3 s of busy grace, an 8 s call budget, and — once a turn is
/// held and the guide is accepted — the 20 s `Agent::pi` wait for a child this box cannot have.
const LANE_CAP: Duration = Duration::from_secs(45);

/// Round 13's ask, in the words it was sent in.
const THE_ASK: &str = "Produce exactly ONE CSV of the licence lines.";

/// A correction, in the shape the quick lane's stub reads as a `[guide]`.
const CHANGE_ONE: &str = "Change: stop at 20, not 40.";
const CHANGE_TWO: &str = "Change: stop at 20, not 40 — and drop the header row as well.";

/// A running job of `chat` with its worker and the hidden progress row that carries the job id,
/// as `run_subagent` leaves one mid-flight. `subagent_msgs` is what `media::target` resolves a
/// worker's `session_id` through.
#[allow(clippy::too_many_arguments)]
async fn a_running_job(
    agent: &Arc<Agent>,
    host: &SharedHost,
    chat: &str,
    job: &str,
    worker: &str,
    ask: &str,
    row: &str,
) {
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(ask, THE_ASK, "dev-mac", &UserMessageOpts::default())
            .unwrap();
    }
    agent
        .store
        .insert_job(
            job,
            chat,
            ask,
            "Counting licence lines",
            "other",
            "Write exactly one CSV of the licence lines.",
            &[],
        )
        .unwrap();
    agent
        .store
        .insert_subagent(worker, "job-one-csv", "count the licence lines", "pi", chat)
        .unwrap();
    agent.store.set_job_subagent(job, worker).unwrap();
    agent.store.set_job_state(job, "running", None).unwrap();
    {
        let h = host.lock().await;
        h.doc.start_assistant_message(row, ask, "box:test");
        h.doc.set_message_fields(
            row,
            &[
                (
                    "job",
                    json!({ "id": job, "title": "Counting licence lines", "kind": "other", "touches": [] }),
                ),
                ("worker", json!(true)),
                ("hidden", json!(true)),
            ],
        );
        h.doc.add_status_part(row, "working", "Working…");
    }
    agent
        .subagent_msgs
        .lock()
        .await
        .insert(worker.to_owned(), (chat.to_owned(), row.to_owned()));
}

fn a_file(agent: &Arc<Agent>, name: &str, body: &str) -> String {
    let dir = crate::media::out_dir(&agent.cfg);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), body).unwrap();
    format!("out/{name}")
}

/// The file cards in the chat, as `(message id, attachment name)`. One card per CSV the user
/// sees, which is the whole measurement of this bug.
fn cards(host: &SharedHost) -> Vec<(String, String)> {
    let h = host.try_lock().expect("host free");
    h.doc
        .messages()
        .into_iter()
        .flat_map(|m| {
            let id = m.id.clone();
            m.parts
                .clone()
                .into_iter()
                .filter(|p| p.get("kind").and_then(Value::as_str) == Some("attachment"))
                .filter_map(move |p| {
                    p.pointer("/attachment/name")
                        .and_then(Value::as_str)
                        .map(|n| (id.clone(), n.to_owned()))
                })
        })
        .collect()
}

fn send_cmd(host: &SharedHost, text: &str) -> (String, CommandView) {
    let msg_id = ulid();
    let h = host.try_lock().expect("host free");
    h.doc
        .append_user_message(&msg_id, text, "dev-mac", &UserMessageOpts::default())
        .unwrap();
    (
        msg_id.clone(),
        CommandView {
            id: ulid(),
            kind: "send".into(),
            payload: json!({ "messageId": msg_id }),
            issued_by: "dev-mac".into(),
            issued_at: arbos_core::ids::now_ms(),
            status: "pending".into(),
        },
    )
}

/// The `quick_reply` decision rows of a chat, as `(message id, side)`. The lane writes one per
/// message it classifies, and the `side` field is the decision itself.
fn decisions(agent: &Arc<Agent>, chat: &str) -> Vec<(String, String)> {
    agent
        .store
        .t3_thread_in_order(chat, 200)
        .unwrap_or_default()
        .into_iter()
        .filter(|(r, _)| r.kind == "quick_reply")
        .filter_map(|(_, meta)| {
            let m = meta?;
            Some((
                m.get("message_id")?.as_str()?.to_owned(),
                m.get("side")?.as_str()?.to_owned(),
            ))
        })
        .collect()
}

/// **Gate 1.** A correction arrives with no turn of this chat on the lock, which is what every
/// correction in rounds 13–19 did: the work was a background job, and a job does not hold the
/// turn lock. It has to reach the job that is already doing the task.
#[tokio::test]
async fn a_correction_with_no_turn_running_reaches_the_chats_job() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    a_running_job(&agent, &host, CHAT, JOB, WORKER, ASK, ROW).await;

    // The job posts its first CSV and settles, as round 13's did before the corrections began.
    let path = a_file(&agent, "licences-ag.csv", "rank,name\n1,a\n");
    let posted =
        crate::media::deliver_file(&agent, &json!({ "path": path, "session_id": WORKER })).await;
    assert!(posted.get("isError").is_none(), "{posted}");
    let card = cards(&host);
    assert_eq!(card.len(), 1, "one CSV so far: {card:?}");
    agent.store.set_job_state(JOB, "done", None).unwrap();

    let jobs_before = agent.store.jobs_for_chat(CHAT).unwrap().len();
    assert!(
        agent.turn.lock().await.is_none(),
        "the scenario is a correction with no turn running; a held turn tests gate 2, not this"
    );

    let (_msg, cmd) = send_cmd(&host, CHANGE_ONE);
    // Not `expect`: what this case measures is what the chat holds afterwards, and the old path
    // fails on its own account first — `handle_send` wants a pi child, which a test box has
    // none of — which would hide the symptom behind a harness panic.
    let took = agent.handle_steer(CHAT, &host, &cmd).await;

    // The observable the old path cannot produce. `handle_send` writes no supersede.
    let superseded = {
        let h = host.lock().await;
        h.doc
            .message(&card[0].0)
            .and_then(|m| m.rest.get("superseded").and_then(Value::as_bool))
    };
    assert_eq!(
        superseded,
        Some(true),
        "the CSV the user is correcting is still standing, so the corrected one lands beside it \
         and the chat shows two answers to one ask"
    );

    // The correction is on the job that was already doing the work…
    let row = agent.store.job(JOB).unwrap().unwrap();
    assert!(
        row.steers.iter().any(|s| s.contains("stop at 20")),
        "the job does not carry the correction: {:?}",
        row.steers
    );

    // …and no second worker was started for it. A fix that steered the job and *also* queued the
    // words as a fresh ask would clear every assertion above and still produce the extra CSV.
    assert_eq!(
        agent.store.jobs_for_chat(CHAT).unwrap().len(),
        jobs_before,
        "a correction started another job; that job is the extra CSV"
    );

    assert!(took.is_ok(), "the correction was not taken: {took:?}");

    let _ = std::fs::remove_dir_all(&root);
}

/// **Gate 2.** The quick lane reads the correction as a `[guide]`, and then has to keep it one.
/// With a job as the running work there is no steerable turn, and the demotion to `[task]` sent
/// every correction of rounds 13–19 down the queue-a-new-ask path before gate 1 could see it.
#[tokio::test]
async fn a_guide_is_not_demoted_just_because_the_running_work_is_a_job() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT2).await.unwrap();
    a_running_job(&agent, &host, CHAT2, JOB2, WORKER2, ASK2, ROW2).await;

    // The job has already posted a CSV, as round 13's had by the time the corrections began.
    let path = a_file(&agent, "licences-ag2.csv", "rank,name\n1,a\n");
    let posted =
        crate::media::deliver_file(&agent, &json!({ "path": path, "session_id": WORKER2 })).await;
    assert!(posted.get("isError").is_none(), "{posted}");

    // The job stays running: that is what makes the box busy, and being busy is what sends the
    // message through the quick lane at all.
    let (msg, cmd) = send_cmd(&host, CHANGE_ONE);
    let _ = tokio::time::timeout(LANE_CAP, agent.quick_reply(CHAT2, &host, &cmd))
        .await
        .expect("the quick lane settles inside its own waits");

    let seen = decisions(&agent, CHAT2);
    let side = seen
        .iter()
        .find(|(id, _)| *id == msg)
        .map(|(_, s)| s.as_str());
    assert_eq!(
        side,
        Some("guide"),
        "the lane read a correction and then filed it as new work: {seen:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// **The ration.** `r18steer` sent seven corrections into one wait and the box classified four.
/// The one-per-wait ration exists to cap status *bubbles*; a guide writes none, so spending it
/// on a guide left every later correction unread and queued — and a queued correction is a new
/// worker with a new file.
#[tokio::test]
async fn a_second_correction_in_the_same_wait_is_still_read() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT3).await.unwrap();
    // A real turn on the lock, so the guides are accepted rather than demoted: this case is
    // about what the first guide spends, not about where a guide can land.
    let _busy = agent.test_hold_turn(CHAT3).await;

    let (first, cmd1) = send_cmd(&host, CHANGE_ONE);
    let _ = tokio::time::timeout(LANE_CAP, agent.quick_reply(CHAT3, &host, &cmd1))
        .await
        .expect("the quick lane settles inside its own waits");

    let (second, cmd2) = send_cmd(&host, CHANGE_TWO);
    let _ = tokio::time::timeout(LANE_CAP, agent.quick_reply(CHAT3, &host, &cmd2))
        .await
        .expect("the quick lane settles inside its own waits");

    let seen = decisions(&agent, CHAT3);
    assert_eq!(
        seen.iter()
            .find(|(id, _)| *id == first)
            .map(|(_, s)| s.as_str()),
        Some("guide"),
        "the first correction was not read as a correction: {seen:?}"
    );
    // Keyed on the second message's own id: the first one's row must not be able to stand in
    // for it.
    assert_eq!(
        seen.iter()
            .find(|(id, _)| *id == second)
            .map(|(_, s)| s.as_str()),
        Some("guide"),
        "the second correction of the wait was never classified; it queues as new work and \
         becomes its own worker: {seen:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
}
