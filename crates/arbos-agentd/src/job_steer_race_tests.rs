//! Bug AB: a correction for a running job lands after the job has already posted its file.
//!
//! Jacob on Test5, round 10 (2026-11-05, agentd 0.1.195 `adcd73573`, side chat
//! `01M6ME9A7EGV5PJ7DZW43AHTN5`, scenario `r10race`). The chat recorded this:
//!
//! | 08:36:57 | the job ask, `01M6P28EGA2FDHNJM19J9QD8MX` |
//! | 08:37:04 | the correction, `01M6P28FSY60H8WHKXX65AA4XP` |
//! | 08:37:09 | the main lane calls `steer_job`, five seconds after the correction arrived |
//! | 08:37:10 | the worker posts the **uncorrected** `race10.csv`, one second after the steer |
//!
//! The agent then refused to post the corrected file on its own and asked "Should I post it as a
//! second message?". `r10steer` and `r9steer3` are the same race with the other outcome: two and
//! three CSVs for one ask, and a third correction that hit a job which had already finished
//! ("Not steered … is done").
//!
//! **The report's hunch, checked against the code.** It says there is no hold between "a user
//! message for this chat arrived" and "the job delivers a file". That is exactly what
//! `media::deliver` read before this change: the only questions it asked about the job were
//! whether it was cancelled (bug S) and whether these same bytes were already in the chat. The
//! chat's inbound side had nothing to tell it either — `handle_command` takes the command,
//! claims a send-order ticket and routes, and nothing of that reaches a worker's delivery. So
//! the two halves of the race could not see each other, and the hunch holds.
//!
//! What the fix adds is `steer_gate.rs`: a routing mark per message in flight, a steer count per
//! job, and one `hold` on the delivery side that reads both.

use std::sync::Arc;
use std::time::Duration;

use arbos_core::doc::UserMessageOpts;
use arbos_core::ids::{now_ms, ulid};
use serde_json::{json, Value};

use crate::agent::Agent;
use crate::status::tests::test_agent;
use crate::steer_gate::Hold;
use crate::sync::SharedHost;

const CHAT: &str = "01TESTCHATJOBSTEERRACE0001";
const JOB: &str = "01TESTJOBSTEERRACE00000001";
const WORKER: &str = "01TESTWORKERSTEERRACE00001";
const ASK: &str = "01TESTASKJOBSTEERRACE00001";
const ROW: &str = "01TESTROWJOBSTEERWORKER001";

/// The hold the tests run at. Production waits 20 s (`config::STEER_HOLD_MS_DEFAULT`); every
/// branch of `SteerGate::hold` is the same code at either scale, and this one keeps the cap test
/// inside a second. Set per agent through the knob file, never through the environment, so these
/// tests can run beside each other.
const HOLD_MS: u64 = 600;

/// A running job of `CHAT` with its worker, as `run_subagent` leaves one mid-flight: the job row,
/// the pi worker session, and the hidden progress row that carries the job id. `subagent_msgs` is
/// what `media::target` resolves a worker's `session_id` through.
async fn a_running_job(agent: &Arc<Agent>, host: &SharedHost) {
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(
                ASK,
                "make me a CSV of the top 30 by rate",
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
            "Building the rate table",
            "other",
            "Write out/race10.csv with the top 30 by rate.",
            &[],
        )
        .unwrap();
    agent
        .store
        .insert_subagent(WORKER, "job-rate-table", "build the rate table", "pi", CHAT)
        .unwrap();
    agent.store.set_job_subagent(JOB, WORKER).unwrap();
    agent.store.set_job_state(JOB, "running", None).unwrap();
    {
        let h = host.lock().await;
        h.doc.start_assistant_message(ROW, ASK, "box:test");
        h.doc.set_message_fields(
            ROW,
            &[
                (
                    "job",
                    json!({ "id": JOB, "title": "Building the rate table", "kind": "other", "touches": [] }),
                ),
                ("worker", json!(true)),
                ("hidden", json!(true)),
            ],
        );
        h.doc.add_status_part(ROW, "working", "Working…");
    }
    agent
        .subagent_msgs
        .lock()
        .await
        .insert(WORKER.to_owned(), (CHAT.to_owned(), ROW.to_owned()));
}

/// Shorten the hold for one agent only, through the same knob file production reads.
fn hold_for(agent: &Arc<Agent>, ms: u64) {
    let f = agent.cfg.state_dir.join(crate::config::STEER_HOLD_MS_FILE);
    std::fs::write(&f, ms.to_string()).unwrap();
    assert_eq!(agent.cfg.steer_hold(), Duration::from_millis(ms));
}

/// Write `name` under `out/` and hand back the path `deliver_file` takes.
fn a_file(agent: &Arc<Agent>, name: &str, body: &str) -> String {
    let dir = crate::media::out_dir(&agent.cfg);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), body).unwrap();
    format!("out/{name}")
}

/// The file cards in the chat, as `(message id, attachment name)`, oldest first. This is what the
/// user sees: one card per CSV that was posted.
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

/// The worker's own `deliver_file` call, as its MCP session makes it.
async fn worker_delivers(agent: &Arc<Agent>, path: &str) -> Value {
    worker_delivers_as(agent, WORKER, path).await
}

/// Same, for a test with its own worker session.
async fn worker_delivers_as(agent: &Arc<Agent>, worker: &str, path: &str) -> Value {
    crate::media::deliver_file(agent, &json!({ "path": path, "session_id": worker })).await
}

/// The spec's test: **a steer routed 5 s after the message arrived still lands before the job's
/// `deliver_file`.**
///
/// This is also round 11's acceptance case, `r11race` (09:31:20 → 09:31:26). There the user's
/// message said "before you post it", arrived six seconds ahead of the delivery and lost anyway,
/// so narrowing the window proves nothing: the delivery itself has to be gated. That is what the
/// two assertions in the middle are for — while the correction is unrouted the chat has no card
/// and the delivery call has not returned.
///
/// The timings are r10race's and r11race's, scaled to the test hold: the correction arrives, the
/// worker's delivery starts a moment later, and the main lane only gets round to `steer_job`
/// after that. The uncorrected CSV must not reach the chat, and the one card the user ends up
/// with must be the corrected one.
#[tokio::test]
async fn a_steer_routed_after_the_delivery_started_still_lands_first() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    hold_for(&agent, HOLD_MS);
    a_running_job(&agent, &host).await;

    // 08:37:04 — the correction arrives. `handle_command` takes this mark before anything else
    // can await, and holds it until it has routed the message.
    let routing = agent.steer_gate.routing(CHAT, &ulid());

    // 08:37:10 — the worker is finished and posts. In round 10 this went straight through.
    let uncorrected = a_file(&agent, "race10.csv", "rank,name,rate\n1,a,9.9\n");
    let delivery = {
        let agent = agent.clone();
        tokio::spawn(async move { worker_delivers(&agent, &uncorrected).await })
    };

    // A fifth of the hold in, the delivery is still waiting rather than writing a row.
    tokio::time::sleep(Duration::from_millis(HOLD_MS / 5)).await;
    assert!(
        cards(&host).is_empty(),
        "the file went into the chat while the correction was still being routed: {:?}",
        cards(&host)
    );
    assert!(!delivery.is_finished(), "the delivery did not wait");

    // 08:37:09 — the main lane calls `steer_job`. The worker's session has the correction; it has
    // not read it yet, because it is inside the delivery call.
    agent.steer_gate.steer_delivered(JOB);
    let refused = tokio::time::timeout(Duration::from_millis(HOLD_MS * 10), delivery)
        .await
        .expect("the correction decides on its own; the delivery must not wait out the hold")
        .unwrap();
    drop(routing);

    assert_eq!(
        refused["isError"],
        json!(true),
        "the uncorrected file must not be posted: {refused}"
    );
    let told = refused["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        told.contains("NOT posted") && told.contains("correction"),
        "and the worker is told to take the correction first: {told}"
    );
    assert!(
        cards(&host).is_empty(),
        "no card may be in the chat yet: {:?}",
        cards(&host)
    );

    // The worker returns from the call, its session hands it the correction, and it posts the
    // corrected file. Different bytes, so the duplicate guard does not stand in the way.
    agent.steer_gate.steers_drained(JOB);
    let corrected = a_file(&agent, "race10.csv", "rank,name,rate,len\n1,a,9.9,30\n");
    let posted = worker_delivers(&agent, &corrected).await;
    assert!(
        posted.get("isError").is_none(),
        "the corrected file posts: {posted}"
    );

    let cards = cards(&host);
    assert_eq!(
        cards.len(),
        1,
        "exactly one CSV reaches the chat for one ask: {cards:?}"
    );
    let body = {
        let h = host.lock().await;
        h.doc
            .message(&cards[0].0)
            .map(|m| m.text())
            .unwrap_or_default()
    };
    assert!(body.is_empty() || !body.contains("superseded"), "{body}");
    let _ = std::fs::remove_dir_all(&root);
}

/// `r11steer`: two corrections two seconds apart, and the job answered with **two CSVs** —
/// 09:29:14 with the first change only, 09:29:30 with both. One ask, one file.
///
/// The second message is the point. It arrives while the first is still being routed, and the
/// delivery that lands between them is exactly the 09:29:14 card — so the hold has to span both
/// messages and not end with the first. Then the corrections reach the worker together and one
/// CSV goes out.
///
/// It discriminates against the hold being gone at all; it does not discriminate against the
/// deadline inside `SteerGate::look` being read off the oldest live mark rather than the newest,
/// because either way the gate waits while *any* mark of the chat is live and the figure is only
/// a sleep hint. Checked by making that change and watching this stay green.
#[tokio::test]
async fn two_corrections_two_seconds_apart_answer_with_one_csv() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT7).await.unwrap();
    hold_for(&agent, HOLD_MS);
    a_job_in(&agent, &host, CHAT7, JOB7, WORKER7, ASK7, ROW7).await;

    // 09:29:0x — "top 20, and length at least 5", then the files column two seconds later.
    let first = agent.steer_gate.routing(CHAT7, &ulid());
    tokio::time::sleep(Duration::from_millis(HOLD_MS / 3)).await;
    let second = agent.steer_gate.routing(CHAT7, &ulid());

    // The worker is finished with the first change applied and nothing else — r11steer's first
    // CSV. It must not become a card.
    let half_done = a_file(&agent, "steer11.csv", "rank,name\n1,aaaaa\n");
    let delivery = {
        let agent = agent.clone();
        tokio::spawn(async move { worker_delivers_as(&agent, WORKER7, &half_done).await })
    };

    // The first message has been in flight longer than the gap between the two, so a hold that
    // ran from the oldest mark would be letting this through about now.
    tokio::time::sleep(Duration::from_millis(HOLD_MS / 2)).await;
    assert!(
        cards(&host).is_empty(),
        "the half-corrected CSV went out between the two corrections: {:?}",
        cards(&host)
    );
    assert!(
        !delivery.is_finished(),
        "the delivery stopped waiting while a correction was still being routed"
    );

    // Both corrections reach the worker together.
    agent.steer_gate.steer_delivered(JOB7);
    let refused = tokio::time::timeout(Duration::from_millis(HOLD_MS * 10), delivery)
        .await
        .expect("the corrections decide; the delivery must not wait out the hold")
        .unwrap();
    drop(first);
    drop(second);
    assert_eq!(refused["isError"], json!(true), "{refused}");

    agent.steer_gate.steers_drained(JOB7);
    let done = a_file(&agent, "steer11.csv", "rank,name,files\n1,aaaaa,3\n");
    let posted = worker_delivers_as(&agent, WORKER7, &done).await;
    assert!(posted.get("isError").is_none(), "{posted}");
    assert_eq!(
        cards(&host).len(),
        1,
        "one ask, one CSV: {:?}",
        cards(&host)
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The first thing to be careful about: **the cap must hold.** A hold on delivery that can be
/// exceeded is a file that never arrives, which is worse than the wrong file.
///
/// A message is routed and never finishes routing — the shape of a main turn that runs for
/// minutes, and of any path that drops its mark late. No steer ever comes. The delivery waits the
/// hold and then posts, once, with the cap crossing logged.
#[tokio::test]
async fn a_routing_mark_that_never_clears_delays_the_file_by_the_cap_and_no_more() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT2).await.unwrap();
    hold_for(&agent, HOLD_MS);
    a_job_in(&agent, &host, CHAT2, JOB2, WORKER2, ASK2, ROW2).await;

    // Held for the whole test: nothing clears this one.
    let _stuck = agent.steer_gate.routing(CHAT2, &ulid());

    let path = a_file(&agent, "cap.csv", "a,b\n1,2\n");
    let t0 = std::time::Instant::now();
    // Bounded here as well as in the gate. A cap that stops working makes this delivery wait for
    // ever, and a test that hangs says nothing; this one says the cap is gone.
    let posted = tokio::time::timeout(
        Duration::from_millis(HOLD_MS * 10),
        crate::media::deliver_file(&agent, &json!({ "path": path, "session_id": WORKER2 })),
    )
    .await
    .expect("the hold never ended: the cap no longer bounds a delivery");
    let waited = t0.elapsed();

    assert!(
        posted.get("isError").is_none(),
        "past the cap the file goes out; a held mark must never swallow it: {posted}"
    );
    assert!(
        waited >= Duration::from_millis(HOLD_MS),
        "it waited {waited:?}, less than the {HOLD_MS} ms hold"
    );
    assert!(
        waited < Duration::from_millis(HOLD_MS * 4),
        "it waited {waited:?}; the cap is {HOLD_MS} ms and must bound the wait"
    );
    assert_eq!(cards(&host).len(), 1, "and the card is there once");
    let _ = std::fs::remove_dir_all(&root);
}

/// The second failure path: **a steer the worker never reads must not block the file for ever.**
///
/// `steers_drained` is the signal that the worker's queue took the correction, and it comes from
/// the worker's own session. A session that dies, or a harness that never reports the queue,
/// never sends it. One delivery per steer is refused and the next goes through, so the worst case
/// is the old behaviour (the file arrives) and never silence.
#[tokio::test]
async fn an_unread_steer_blocks_one_delivery_and_never_the_next() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT3).await.unwrap();
    hold_for(&agent, HOLD_MS);
    a_job_in(&agent, &host, CHAT3, JOB3, WORKER3, ASK3, ROW3).await;

    let cap = Duration::from_millis(HOLD_MS);
    agent.steer_gate.steer_delivered(JOB3);
    assert_eq!(
        bounded_hold(&agent, CHAT3, JOB3, cap).await,
        Hold::SteerFirst,
        "the first delivery after a correction is refused"
    );
    assert_eq!(
        bounded_hold(&agent, CHAT3, JOB3, cap).await,
        Hold::Clear,
        "and the next one goes through, read or not: a refusal that repeats is a lost file"
    );

    // A second correction is a second refusal, not an unbounded one.
    agent.steer_gate.steer_delivered(JOB3);
    assert_eq!(
        bounded_hold(&agent, CHAT3, JOB3, cap).await,
        Hold::SteerFirst
    );
    assert_eq!(bounded_hold(&agent, CHAT3, JOB3, cap).await, Hold::Clear);

    // A worker that did read it is not refused at all.
    agent.steer_gate.steer_delivered(JOB3);
    agent.steer_gate.steers_drained(JOB3);
    assert_eq!(
        bounded_hold(&agent, CHAT3, JOB3, cap).await,
        Hold::Clear,
        "a correction the worker has taken is not a reason to refuse its file"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The third failure path, and the one that decides whether any of this runs in production:
/// **`handle_command` takes the mark for a chat with a job open, and clears it on every exit,
/// including the ones that fail.**
///
/// A mark never taken means the race is still open; a mark that leaks turns the hold into the cap
/// for every file that job posts afterwards. This drives the real command handler. The chat's job
/// cap is zero so the busy-lane path hands the send to the main session rather than answering it
/// as a job, and the test holds the turn lock, so the command is deterministically in flight
/// while the mark is read. Releasing the lock lets it run on to the exit this box can reach: no
/// pi child, so `handle_send` fails where the model call would be, and the mark must still be
/// gone.
#[tokio::test]
async fn the_routing_mark_is_taken_by_the_command_and_released_however_it_ends() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT4).await.unwrap();
    a_job_in(&agent, &host, CHAT4, JOB4, WORKER4, ASK4, ROW4).await;
    *agent.job_cap.lock().unwrap() = Some(0);

    let msg_id = ulid();
    let cmd = {
        let h = host.lock().await;
        h.doc
            .append_user_message(
                &msg_id,
                "sort by length too, not just rate",
                "dev-mac",
                &UserMessageOpts::default(),
            )
            .unwrap();
        arbos_core::doc::CommandView {
            id: ulid(),
            kind: "send".into(),
            payload: json!({ "messageId": msg_id }),
            issued_by: "dev-mac".into(),
            issued_at: now_ms(),
            status: "pending".into(),
        }
    };
    let busy = agent.test_hold_turn(CHAT4).await;
    let arrived = std::time::Instant::now();
    let command = {
        let agent = agent.clone();
        tokio::spawn(async move { agent.handle_command(CHAT4, cmd).await })
    };

    // The command is routing and cannot finish while the turn lock is held, so this is a read
    // rather than a race.
    let marked = wait_until(Duration::from_secs(10), || {
        agent.steer_gate.marks(CHAT4) == 1
    })
    .await;
    assert!(
        marked,
        "handle_command took no routing mark for a chat with a job open"
    );

    // Which is what the delivery side waits on. The wait runs from when the message arrived, not
    // from when the delivery asked, so it ends one hold after the mark went up however many
    // deliveries ask in between.
    assert_eq!(
        bounded_hold(&agent, CHAT4, JOB4, Duration::from_millis(HOLD_MS)).await,
        Hold::Clear
    );
    assert!(
        arrived.elapsed() >= Duration::from_millis(HOLD_MS),
        "the mark held nothing: the delivery was let through {:?} after the message arrived",
        arrived.elapsed()
    );

    drop(busy);
    let _ = command.await;
    assert!(
        wait_until(Duration::from_secs(10), || agent.steer_gate.marks(CHAT4)
            == 0)
        .await,
        "the routing mark outlived the command that took it"
    );

    // And with nothing in flight the hold is not a wait at all.
    let t0 = std::time::Instant::now();
    assert_eq!(
        bounded_hold(&agent, CHAT4, JOB4, Duration::from_secs(20)).await,
        Hold::Clear
    );
    assert!(
        t0.elapsed() < Duration::from_secs(1),
        "a quiet chat must not pay the hold: {:?}",
        t0.elapsed()
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `SteerGate::hold`, with the cap it is given enforced from outside as well. A hold that outlasts
/// its own cap is the failure this whole change is warned about, and a test that hangs on it says
/// nothing.
async fn bounded_hold(agent: &Arc<Agent>, chat: &str, job: &str, cap: Duration) -> Hold {
    tokio::time::timeout(cap * 10, agent.steer_gate.hold(chat, job, cap))
        .await
        .unwrap_or_else(|_| panic!("the hold outlasted ten times its {cap:?} cap"))
}

/// Poll `f` until it holds or `bound` runs out.
async fn wait_until(bound: Duration, mut f: impl FnMut() -> bool) -> bool {
    let t0 = std::time::Instant::now();
    loop {
        if f() {
            return true;
        }
        if t0.elapsed() >= bound {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// `r10steer`'s third correction: it reached a job that had finished seconds earlier and was
/// told "Not steered … is done", so the correction was simply lost and the user kept the stale
/// CSV. A job that finished and posted inside the replace window takes the steer, marks the card
/// it posted superseded, and goes again.
#[tokio::test]
async fn a_steer_for_a_job_that_just_posted_replaces_the_file_instead_of_failing() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT5).await.unwrap();
    a_job_in(&agent, &host, CHAT5, JOB5, WORKER5, ASK5, ROW5).await;

    // The job posts its CSV and finishes, as r10steer's did at 08:33:42.
    let path = a_file(&agent, "steer10.csv", "rank,name\n1,a\n");
    let posted =
        crate::media::deliver_file(&agent, &json!({ "path": path, "session_id": WORKER5 })).await;
    assert!(posted.get("isError").is_none(), "{posted}");
    let card = cards(&host);
    assert_eq!(card.len(), 1, "{card:?}");
    agent.store.set_job_state(JOB5, "done", None).unwrap();

    // The correction, nine seconds later by r10steer's clock and inside the window here.
    let steered = agent
        .test_steer_job_from(CHAT5, JOB5, "only entries longer than 30 characters")
        .await
        .expect("a job that posted seconds ago takes the correction");
    assert_eq!(
        steered["state"], "replacing",
        "the steer must not come back as a failure: {steered}"
    );
    assert_eq!(steered["superseded_message_id"], json!(card[0].0));

    // The card the user is looking at says it has been replaced, so the two CSVs cannot be read
    // as two answers.
    let superseded = {
        let h = host.lock().await;
        h.doc
            .message(&card[0].0)
            .and_then(|m| m.rest.get("superseded").and_then(Value::as_bool))
    };
    assert_eq!(
        superseded,
        Some(true),
        "the earlier card is not marked superseded"
    );

    // The job is open again and carries the correction, so whoever takes it on runs the corrected
    // ask (the bug V rule) rather than the brief as first written.
    let row = agent.store.job(JOB5).unwrap().unwrap();
    assert_eq!(row.state, "running", "the job reopened");
    assert!(
        row.steers
            .iter()
            .any(|s| s.contains("longer than 30 characters")),
        "the correction is on the job row: {:?}",
        row.steers
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A job that finished long ago is still refused: "replace" is for the file the user is looking
/// at, not for restarting yesterday's work.
#[tokio::test]
async fn a_steer_for_a_job_that_finished_long_ago_is_still_refused() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT6).await.unwrap();
    a_job_in(&agent, &host, CHAT6, JOB6, WORKER6, ASK6, ROW6).await;

    let path = a_file(&agent, "old.csv", "rank,name\n1,a\n");
    let posted =
        crate::media::deliver_file(&agent, &json!({ "path": path, "session_id": WORKER6 })).await;
    assert!(posted.get("isError").is_none(), "{posted}");
    agent.store.set_job_state(JOB6, "done", None).unwrap();
    // Ended five minutes ago, well past the window.
    agent
        .store
        .with(|c| {
            c.execute(
                "UPDATE jobs SET ended_at = ?2 WHERE id = ?1",
                rusqlite::params![JOB6, now_ms() - 300_000],
            )
        })
        .unwrap();

    let err = agent
        .test_steer_job_from(CHAT6, JOB6, "redo it with the length filter")
        .await
        .expect_err("a job that finished five minutes ago is not replaced");
    assert!(
        err.contains("done"),
        "and the model is told what state it is in: {err}"
    );
    assert_eq!(
        agent.store.job(JOB6).unwrap().unwrap().state,
        "done",
        "the job must not reopen"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// **The hold does not stack with the send order (bug AA's lane).** Asked of the real code,
/// because this is the one way a hold on delivery could cost minutes rather than seconds.
///
/// Nothing is held inside a send-order place: a worker's delivery never joins the chat's queue,
/// so while a file waits the queue is empty, and the next message is its own turn at once rather
/// than 20 s behind this file. And the lanes AA moved — the receipt `start_job` posts for a
/// forced ask, and the result row `write_job_result` writes directly — are main-lane writes, so
/// the gate is not on their path at all; the main-turn delivery here stands for them, and goes
/// out while a correction of the same chat is still being routed.
///
/// Both assertions discriminate. A hold taken while a ticket is held leaves `waiting` at one and
/// puts the later message behind it; a hold that forgot to exempt the main turn makes the second
/// delivery wait out the cap.
#[tokio::test]
async fn a_held_delivery_takes_no_place_in_the_chats_send_order() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT8).await.unwrap();
    hold_for(&agent, HOLD_MS);
    a_job_in(&agent, &host, CHAT8, JOB8, WORKER8, ASK8, ROW8).await;

    let correction = ulid();
    let routing = agent.steer_gate.routing(CHAT8, &correction);
    let held = a_file(&agent, "coupling.csv", "rank,name\n1,a\n");
    let delivery = {
        let agent = agent.clone();
        tokio::spawn(async move { worker_delivers_as(&agent, WORKER8, &held).await })
    };
    tokio::time::sleep(Duration::from_millis(HOLD_MS / 4)).await;
    assert!(!delivery.is_finished(), "the file is not being held");

    // The file is waiting, and the chat's queue knows nothing about it.
    assert_eq!(
        agent.send_order.waiting(CHAT8),
        0,
        "a held delivery is holding a place in the send order, so every later message now waits \
         behind it as well as behind its own turn"
    );

    // So the next message is answerable the moment it arrives: the hold adds nothing to the two
    // minutes of `SEND_ORDER_WAIT` it could have been added to.
    let later = ulid();
    let _ticket = agent
        .send_order
        .join(CHAT8, &later)
        .expect("a fresh message takes a place");
    assert!(
        agent.send_order.is_turn(CHAT8, &later),
        "the later message is queued behind the held file"
    );
    tokio::time::timeout(
        Duration::from_millis(HOLD_MS / 4),
        agent.await_send_order(CHAT8, &later, "test"),
    )
    .await
    .expect("waiting for this message's turn outlasted the file it should not be waiting for");

    // And the main lane posts into the same chat while the correction is still unrouted, which is
    // what AA's receipt and result rows do. The turn carries the job, so this is the main lane
    // relaying that job's answer — the one main-lane delivery the gate could have caught.
    let _busy = agent.test_hold_turn_for(CHAT8, &later).await;
    agent.turn.lock().await.as_mut().unwrap().job_id = Some(JOB8.to_owned());
    let main = a_file(&agent, "receipt.csv", "rank,name\n1,b\n");
    let posted = tokio::time::timeout(
        Duration::from_millis(HOLD_MS / 2),
        crate::media::deliver_file(&agent, &json!({ "path": main })),
    )
    .await
    .expect("the main lane's own post was held by the gate")
    .clone();
    assert!(posted.get("isError").is_none(), "{posted}");

    drop(routing);
    let _ = tokio::time::timeout(Duration::from_millis(HOLD_MS * 10), delivery)
        .await
        .expect("the worker's file never arrived");
    let _ = std::fs::remove_dir_all(&root);
}

/// **A cancel that lands inside the hold is honoured (bug S's rule, bug Z's lane).**
///
/// The hold put up to twenty seconds between "is this job cancelled" and the upload, and the
/// question was asked only on the near side. So a user who stopped a job while its file was
/// waiting got the file anyway, which is the thing `media::deliver` refuses on every other path
/// — and bug Z's cancel, which kills the job's process group and reports what survived, says
/// nothing of a cancelled job reaches the chat. `write_job_result` asks on both sides of its own
/// wait for exactly this reason; this asks on both sides of ours.
///
/// Cancelled through `Agent::stop_jobs`, the path the model's `stop_job` takes, so the state the
/// delivery reads is the state a real cancel writes.
#[tokio::test]
async fn a_job_cancelled_while_its_file_is_held_posts_nothing() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT9).await.unwrap();
    hold_for(&agent, HOLD_MS);
    a_job_in(&agent, &host, CHAT9, JOB9, WORKER9, ASK9, ROW9).await;

    // A message of this chat is being routed, so the worker's file waits.
    let _routing = agent.steer_gate.routing(CHAT9, &ulid());
    let path = a_file(&agent, "cancelled.csv", "rank,name\n1,a\n");
    let delivery = {
        let agent = agent.clone();
        tokio::spawn(async move { worker_delivers_as(&agent, WORKER9, &path).await })
    };
    tokio::time::sleep(Duration::from_millis(HOLD_MS / 4)).await;
    assert!(!delivery.is_finished(), "the file is not being held");

    // "stop that" — mid-hold, which is where the old code could not see it.
    agent.stop_jobs(CHAT9, Some(JOB9)).await;
    assert_eq!(
        agent.store.job(JOB9).unwrap().unwrap().state,
        "cancelled",
        "the stop did not cancel the job, so this test is not testing a cancel"
    );

    let out = tokio::time::timeout(Duration::from_millis(HOLD_MS * 10), delivery)
        .await
        .expect("the held delivery never returned after the cancel")
        .unwrap();
    assert_eq!(
        out["isError"],
        json!(true),
        "the file of a job the user stopped went out when the hold ended: {out}"
    );
    assert!(
        cards(&host).is_empty(),
        "a cancelled job put a file in the chat: {:?}",
        cards(&host)
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Same shape as [`a_running_job`], for the tests that need their own chat.
#[allow(clippy::too_many_arguments)]
async fn a_job_in(
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
            .append_user_message(ask, "make me a CSV", "dev-mac", &UserMessageOpts::default())
            .unwrap();
    }
    agent
        .store
        .insert_job(
            job,
            chat,
            ask,
            "Building a table",
            "other",
            "Write a CSV.",
            &[],
        )
        .unwrap();
    agent
        .store
        .insert_subagent(worker, "job-a-table", "build a table", "pi", chat)
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
                    json!({ "id": job, "title": "Building a table", "kind": "other", "touches": [] }),
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

const CHAT2: &str = "01TESTCHATJOBSTEERRACE0002";
const JOB2: &str = "01TESTJOBSTEERRACE00000002";
const WORKER2: &str = "01TESTWORKERSTEERRACE00002";
const ASK2: &str = "01TESTASKJOBSTEERRACE00002";
const ROW2: &str = "01TESTROWJOBSTEERWORKER002";

const CHAT3: &str = "01TESTCHATJOBSTEERRACE0003";
const JOB3: &str = "01TESTJOBSTEERRACE00000003";
const WORKER3: &str = "01TESTWORKERSTEERRACE00003";
const ASK3: &str = "01TESTASKJOBSTEERRACE00003";
const ROW3: &str = "01TESTROWJOBSTEERWORKER003";

const CHAT4: &str = "01TESTCHATJOBSTEERRACE0004";
const JOB4: &str = "01TESTJOBSTEERRACE00000004";
const WORKER4: &str = "01TESTWORKERSTEERRACE00004";
const ASK4: &str = "01TESTASKJOBSTEERRACE00004";
const ROW4: &str = "01TESTROWJOBSTEERWORKER004";

const CHAT5: &str = "01TESTCHATJOBSTEERRACE0005";
const JOB5: &str = "01TESTJOBSTEERRACE00000005";
const WORKER5: &str = "01TESTWORKERSTEERRACE00005";
const ASK5: &str = "01TESTASKJOBSTEERRACE00005";
const ROW5: &str = "01TESTROWJOBSTEERWORKER005";

const CHAT9: &str = "01TESTCHATJOBSTEERRACE0009";
const JOB9: &str = "01TESTJOBSTEERRACE00000009";
const WORKER9: &str = "01TESTWORKERSTEERRACE00009";
const ASK9: &str = "01TESTASKJOBSTEERRACE00009";
const ROW9: &str = "01TESTROWJOBSTEERWORKER009";

const CHAT8: &str = "01TESTCHATJOBSTEERRACE0008";
const JOB8: &str = "01TESTJOBSTEERRACE00000008";
const WORKER8: &str = "01TESTWORKERSTEERRACE00008";
const ASK8: &str = "01TESTASKJOBSTEERRACE00008";
const ROW8: &str = "01TESTROWJOBSTEERWORKER008";

const CHAT7: &str = "01TESTCHATJOBSTEERRACE0007";
const JOB7: &str = "01TESTJOBSTEERRACE00000007";
const WORKER7: &str = "01TESTWORKERSTEERRACE00007";
const ASK7: &str = "01TESTASKJOBSTEERRACE00007";
const ROW7: &str = "01TESTROWJOBSTEERWORKER007";

const CHAT6: &str = "01TESTCHATJOBSTEERRACE0006";
const JOB6: &str = "01TESTJOBSTEERRACE00000006";
const WORKER6: &str = "01TESTWORKERSTEERRACE00006";
const ASK6: &str = "01TESTASKJOBSTEERRACE00006";
const ROW6: &str = "01TESTROWJOBSTEERWORKER006";
