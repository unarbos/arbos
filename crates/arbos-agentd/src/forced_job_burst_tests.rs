//! Bug AA: a burst of forced background-job asks answers every one of them, and holds nothing
//! behind them.
//!
//! Jacob on Test5, round 10 (2026-11-05, agentd 0.1.195 / adcd73573, side chat
//! 01M6ME9A7EGV5PJ7DZW43AHTN5, scenario `r10burst`). Four asks "As a background job (not
//! inline) …" arrived one to two seconds apart (08:39:57–08:40:04) while a turn was busy, then
//! twelve more messages one every two seconds (08:40:06–08:40:30).
//!
//! All four answers were computed, correctly, inside sixteen seconds. The user saw none of them
//! for six and a half minutes (visible rows at 08:46:28–08:46:44). Meanwhile every one of the
//! twelve later messages hit `send order: the messages ahead have not answered yet; posting out
//! of turn` with `waited_s` 120, took 124 to 150 seconds, and came back scrambled.
//!
//! Nothing was slow. Two waits were pointed at each other:
//!
//! 1. The ask kept its place in the chat's FIFO until the job's **result** row was posted, so
//!    twelve messages queued behind four workers.
//! 2. Those messages, waiting, held `Agent::sends_in_flight` above zero — and the result's relay
//!    turn only takes the turn lock when no user send is in flight and none is owed its first
//!    assistant row. So the row that would have released the places could not be written until
//!    the places stopped blocking the messages.
//!
//! A circular wait, left only by each message timing out at two minutes. It defeated #2774 (one
//! FIFO per chat, so a burst is answered in send order) and left #2778 (an explicit
//! background-job ask always becomes a job) half working, so the cases below pin what those two
//! bought as well as AA's own guarantee.
//!
//! The fix is two rows where there was one: a job answers its ask with a receipt at job start
//! and releases the place there, and its result is its own row, written by the direct model call
//! `write_job_result` rather than by a turn that waits for the main session.
//!
//! The burst is driven through the real entry point, `Agent::handle_command`, with a turn on the
//! lock for the whole of it. The twelve messages behind it are driven at the send-order gate, for
//! the reason `send_order_tests.rs` gives: a test box has no pi, so no main session can run.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use arbos_core::doc::{CommandView, MessageView, UserMessageOpts};
use arbos_core::ids::{now_ms, ulid};
use serde_json::{json, Value};
use tokio::time::Instant;

use crate::send_order::SEND_ORDER_WAIT;
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

/// The rows of `msg_id` a client would draw: `hidden` rows are skipped by both apps, so a hidden
/// row is not a row the user saw — which is the whole measurement of this bug.
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

/// The gap between the sends of a burst. Milliseconds rather than Jacob's seconds, but never
/// zero: a ULID's first ten characters are the millisecond it was minted, and that is what
/// carries the send order between boxes, so ids from one millisecond sort by chance.
const APART: Duration = Duration::from_millis(5);

/// The four asks, in the shape Jacob sent them.
const BURST: [&str; 4] = [
    "What is 739 * 736807367 + 4615229, as a background job (not inline).",
    "Reduce 1281/815350484991 to lowest terms, as a background job (not inline).",
    "How many primes are under 39000? As a background job (not inline).",
    "Multiply 20481280021333333332 by 1000, as a background job (not inline).",
];

/// How long one of the twelve messages behind the burst may wait for its turn.
///
/// Jacob's grade asks for under fifteen seconds each. This is far tighter, because nothing in
/// these lanes does any work: a message that is not blocked reaches the head as soon as the one
/// in front of it releases, so anything near a second means it is queued behind a worker.
const PROMPT: Duration = Duration::from_secs(1);

/// Send the burst into a chat whose turn is already busy, as Jacob's four arrived. Returns the
/// ask message ids, in send order.
async fn the_burst(agent: &Arc<crate::agent::Agent>, host: &SharedHost, chat: &str) -> Vec<String> {
    let mut asks = Vec::new();
    for (n, text) in BURST.iter().enumerate() {
        if n > 0 {
            tokio::time::sleep(APART).await;
        }
        let (msg_id, cmd) = send_cmd(host, text);
        agent.handle_command(chat, cmd).await.unwrap();
        asks.push(msg_id);
    }
    asks
}

/// Each of the four forced asks has its one visible row as soon as its job starts.
///
/// This is AA's own symptom, measured where the user met it: four answers that existed by
/// 08:40:16 and six and a half minutes of nothing in the chat.
#[tokio::test]
async fn the_r10_burst_answers_every_forced_ask_at_job_start() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATBURSTJOBACK00001";
    let host = agent.sync.open_room(chat).await.unwrap();
    // A turn holds the lock for the whole burst, which is the state that sent these four asks
    // down the busy lane in the first place.
    let _busy = agent.test_hold_turn(chat).await;
    let asks = the_burst(&agent, &host, chat).await;

    // #2778's guarantee, unchanged: four asks for a background job, four real jobs.
    let jobs = agent.store.jobs_for_chat(chat).unwrap();
    assert_eq!(
        jobs.len(),
        4,
        "four asks, four job ids: {:?}",
        jobs.iter().map(|j| j.title.as_str()).collect::<Vec<_>>()
    );

    for (n, ask) in asks.iter().enumerate() {
        let rows = visible_replies(&host, ask);
        assert_eq!(
            rows.len(),
            1,
            "ask {} has one visible row, not {}: {:?}",
            n + 1,
            rows.len(),
            rows.iter().map(|m| m.text()).collect::<Vec<_>>()
        );
        let job = jobs
            .iter()
            .find(|j| &j.ask_message_id == ask)
            .unwrap_or_else(|| panic!("no job for ask {}", n + 1));
        let text = rows[0].text();
        assert!(
            text.contains(&job.title),
            "the row names the work it started: {text:?} vs {:?}",
            job.title
        );
    }

    let _ = std::fs::remove_dir_all(&root);
}

/// The twelve messages sent behind the burst are answered in send order, each on its turn.
///
/// The knock-on, and #2774's guarantee restored. The four asks no longer hold a place, so there
/// is nothing for the twelve to queue behind; before the fix each one waited the whole of
/// `SEND_ORDER_WAIT` and the answers came back scrambled.
#[tokio::test]
async fn the_twelve_messages_behind_the_r10_burst_are_answered_in_send_order() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATBURSTJOBACK00004";
    let host = agent.sync.open_room(chat).await.unwrap();
    let _busy = agent.test_hold_turn(chat).await;
    let asks = the_burst(&agent, &host, chat).await;
    assert_eq!(asks.len(), 4);

    // The assertion the bug fails outright: the asks are answered, so none of them may still be
    // holding a place in the chat's FIFO while its worker runs.
    assert_eq!(
        agent.send_order.waiting(chat),
        0,
        "a job is holding its ask's place until its result, which is what stalled the chat"
    );

    let mut ids = Vec::new();
    for _ in 0..12 {
        tokio::time::sleep(APART).await;
        ids.push(ulid());
    }
    let answered: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));
    let mut lanes = Vec::new();
    for (n, id) in ids.iter().cloned().enumerate() {
        let agent = agent.clone();
        let answered = answered.clone();
        lanes.push(tokio::spawn(async move {
            let ticket = agent.send_order.join(chat, &id).expect("a fresh place");
            let in_turn = agent.send_order.wait_turn(chat, &id, PROMPT).await;
            answered.lock().expect("answers").push(n + 1);
            drop(ticket);
            in_turn
        }));
    }
    for (n, lane) in lanes.into_iter().enumerate() {
        let in_turn = tokio::time::timeout(SEND_ORDER_WAIT, lane)
            .await
            .unwrap_or_else(|_| panic!("message {} never reached its turn", n + 1))
            .expect("lane");
        assert!(
            in_turn,
            "message {} was let out by the valve running out, not by its turn: \
             a `send order … waited_s` warn in the log",
            n + 1
        );
    }
    assert_eq!(
        answered.lock().expect("answers").clone(),
        (1..=12).collect::<Vec<_>>(),
        "the twelve must be answered in the order they were sent"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// A forced job's result is posted while the chat is still busy, because it is written directly
/// and not by a turn that waits for the main session.
///
/// The condition is the one `relay_slot` and `open_turn_row` refuse a relay row on: a turn on the
/// lock, and a user message owed its first assistant row. In `r10burst` that condition held for
/// six and a half minutes and four finished answers waited it out.
#[tokio::test]
async fn a_forced_jobs_result_lands_in_a_busy_chat_and_a_relay_turn_does_not() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATBURSTJOBACK00002";
    let host = agent.sync.open_room(chat).await.unwrap();
    let _busy = agent.test_hold_turn(chat).await;

    let (ask, cmd) = send_cmd(
        &host,
        "How many primes are under 39000? As a background job (not inline).",
    );
    agent.handle_command(chat, cmd).await.unwrap();
    let job = agent.store.jobs_for_chat(chat).unwrap().remove(0);
    assert_eq!(visible_replies(&host, &ask).len(), 1, "the ack");

    // A message of this chat that has just arrived and has no assistant row yet: a relay turn
    // must not write in front of it (bug M), and that is exactly why a result must not be one.
    let (owed, _owed_cmd) = send_cmd(&host, "and what about 40000?");
    assert!(visible_replies(&host, &owed).is_empty());

    let t0 = Instant::now();
    agent
        .write_job_result(
            chat,
            &host,
            &ask,
            "job-how-many-primes-ab12",
            "done",
            "4203 primes are under 39000.",
            None,
        )
        .await
        .expect("the result is written");
    assert!(
        t0.elapsed() < Duration::from_secs(40),
        "the result waited for the chat to go quiet: {:?}",
        t0.elapsed()
    );
    let rows = visible_replies(&host, &ask);
    assert_eq!(
        rows.len(),
        2,
        "the ack and then the result, two rows: {:?}",
        rows.iter().map(|m| m.text()).collect::<Vec<_>>()
    );

    // The contrast, and the reason the routing in `run_subagent` had to change: the relay turn a
    // job's result used to take writes nothing at all while that message is owed its row.
    let relaying = {
        let agent = agent.clone();
        let host = host.clone();
        let ask = ask.clone();
        tokio::spawn(async move {
            agent
                .run_result_turn(
                    chat,
                    &host,
                    &ask,
                    &crate::agent::ResultDelivery {
                        subagent_id: "01TESTWORKERBURSTJOBACK001",
                        name: "job-how-many-primes-ab12",
                        status: "done",
                        total_ms: 7_000,
                        raw: "4203 primes are under 39000.",
                        result: "4203 primes are under 39000.",
                        error: None,
                        job: Some(&crate::agent::JobSpec {
                            id: job.id.clone(),
                            title: job.title.clone(),
                            kind: job.kind.clone(),
                            brief: job.brief.clone(),
                            touches: Vec::new(),
                            attempt: 0,
                        }),
                    },
                )
                .await
        })
    };
    assert!(
        tokio::time::timeout(Duration::from_secs(5), relaying)
            .await
            .is_err(),
        "a relay turn is still held by the busy chat — were it not, this test would be \
         measuring nothing and the routing change would be pointless"
    );
    assert_eq!(
        visible_replies(&host, &ask).len(),
        2,
        "and it wrote no row while it was held"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// The round-11 lead, settled: an empty quick reply strands nothing, so the stall is not its
/// doing.
///
/// Round 11 (2026-11-05, `r11burst`) logged `quick reply could not stand as a status line; silent
/// queue … chars 0` immediately before each of the ten stalled messages, which reads as a named
/// path answering with nothing and queueing it silently. It is not AA, and this case is what says
/// so:
///
/// * The quick lane takes no place in the chat's send order and releases none. It is read before
///   `handle_command`'s own ticket does any waiting, and `QuickOutcome::Queued` means exactly
///   what it says — the main session answers the message, holding the place it already has.
/// * A silently queued message is therefore owed an answer, correctly, and reaches the head as
///   soon as nothing older is owed one. Here that is at once, because the forced asks ahead of it
///   were answered by their receipts.
///
/// The warn appeared before every stalled message because every mid-turn message is read by the
/// quick lane first and then falls to the main lane, where the wait is. An empty call is its own
/// small fault — a model call spent for no output — and it is not this one.
#[tokio::test]
async fn an_empty_quick_reply_queues_silently_and_strands_no_place() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATBURSTJOBACK00006";
    let host = agent.sync.open_room(chat).await.unwrap();
    let _busy = agent.test_hold_turn(chat).await;

    // `chars 0` at the unit it is decided: a status tag with no sentence after it cannot stand,
    // so the lane has nothing to post and says so.
    assert_eq!(
        crate::status::settle_quick(crate::status::SideKind::Status, "", false),
        (crate::status::SideKind::Task, None),
        "an empty status line is what produced the round-11 warn"
    );

    // The burst, so the chat is in r11burst's state: three jobs open and a turn on the lock.
    let asks = the_burst(&agent, &host, chat).await;
    assert_eq!(agent.store.jobs_for_chat(chat).unwrap().len(), 4);
    assert_eq!(asks.len(), 4);

    let before = agent.send_order.waiting(chat);
    let (quiet, cmd) = send_cmd(&host, crate::status::tests::EMPTY_STATUS_ASK);
    assert_eq!(
        agent.quick_reply(chat, &host, &cmd).await,
        crate::status::QuickOutcome::Queued,
        "the empty reply queues for the main session"
    );
    assert!(
        visible_replies(&host, &quiet).is_empty(),
        "and posts no row, which is the silent part"
    );
    assert_eq!(
        agent.send_order.waiting(chat),
        before,
        "the quick lane neither took a place nor freed one"
    );

    // The message's own place, as `handle_command` holds it. Nothing older is owed an answer —
    // the receipts settled the four asks — so it is answered now rather than at the valve.
    let ticket = agent.send_order.join(chat, &quiet).expect("its own place");
    assert_eq!(agent.send_order.ahead(chat, &quiet), 0);
    assert!(
        agent
            .send_order
            .wait_turn(chat, &quiet, Duration::from_millis(200))
            .await,
        "a silently queued message waits only for messages that are genuinely owed an answer"
    );
    drop(ticket);
    assert_eq!(agent.send_order.waiting(chat), 0);

    let _ = std::fs::remove_dir_all(&root);
}

/// The forced-job signal on the ask, read the same way `hook_start_job` / `start_job` read it for
/// the start receipt. Result delivery no longer branches on this (#2964): every `JobSpec` posts
/// through `write_job_result`. The selector still decides whether an ask gets a start receipt.
#[tokio::test]
async fn the_forced_signal_is_read_off_the_ask() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATBURSTJOBACK00005";
    let host = agent.sync.open_room(chat).await.unwrap();

    let (forced, _) = send_cmd(&host, BURST[0]);
    let (plain, _) = send_cmd(&host, "Write a CSV of n squared up to 5000.");
    let (counted, _) = send_cmd(
        &host,
        "As a second background job, check the staging folder.",
    );

    assert!(agent.ask_forced_a_job(&host, &forced).await);
    assert!(agent.ask_forced_a_job(&host, &counted).await);
    assert!(
        !agent.ask_forced_a_job(&host, &plain).await,
        "a job the model chose off its own judgement is not a forced ask"
    );
    assert!(
        !agent
            .ask_forced_a_job(&host, "01NOSUCHMESSAGEINTHISDOC01")
            .await,
        "a message that is gone forces nothing"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// #2964 / feedback AG5fMKSEGcm19R9A3FU1oKw: act→job research ("Find me the best sushi in la")
/// is not a forced ask, so it used to take `run_result_turn`. A silent relay left the findings
/// only on the hidden worker. Every job now posts through `write_job_result`, so the user's
/// visible reply is the summary — even while the chat's turn lock is held.
#[tokio::test]
async fn a_research_jobs_findings_land_as_a_visible_reply() {
    use crate::status::tests::{RESEARCH_JOB_RESULT_ANSWER, RESEARCH_JOB_RESULT_ASK};

    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATBURSTJOBACK00007";
    let host = agent.sync.open_room(chat).await.unwrap();
    let _busy = agent.test_hold_turn(chat).await;

    let (ask, _) = send_cmd(&host, RESEARCH_JOB_RESULT_ASK);
    assert!(
        !agent.ask_forced_a_job(&host, &ask).await,
        "premise: the sushi ask is not forced wording"
    );

    // A wordless Done result row, as the act→job main turn leaves when nothing was posted.
    {
        let h = host.lock().await;
        let empty = ulid();
        h.doc
            .start_assistant_message(&empty, &ask, &agent.cfg.device_id);
        h.doc
            .finish_message(&empty, arbos_core::doc::MessageStatus::Done);
    }
    assert!(
        visible_replies(&host, &ask).is_empty(),
        "premise: only a wordless Done so far"
    );

    let notes = "\
## Best sushi in LA
| Place | Why |
| --- | --- |
| [Sushi Gen](https://example.com/sushigen) | Consistently top-rated omakase downtown |
| [n/naka](https://example.com/nnaka) | Reservation-only; critics' pick for kaiseki-style sushi |
| [Kato](https://example.com/kato) | Strong value omakase westside |

Sources checked: Yelp top-rated, Eater LA map. Caveat: waitlists move weekly.";

    let t0 = Instant::now();
    agent
        .write_job_result(
            chat,
            &host,
            &ask,
            "job-finding-sushi-ab12",
            "done",
            notes,
            None,
        )
        .await
        .expect("the research findings are written");
    assert!(
        t0.elapsed() < Duration::from_secs(40),
        "the findings waited for the chat to go quiet: {:?}",
        t0.elapsed()
    );

    let rows = visible_replies(&host, &ask);
    assert_eq!(
        rows.len(),
        1,
        "one visible summary under the ask: {:?}",
        rows.iter().map(|m| m.text()).collect::<Vec<_>>()
    );
    let prose = rows[0].text();
    assert_eq!(
        prose.trim(),
        RESEARCH_JOB_RESULT_ANSWER,
        "the talking-model summary is the bubble — not the hidden worker row, not silence"
    );
    assert_eq!(
        rows[0].rest.get("decision").and_then(Value::as_str),
        Some("job"),
        "posted as a job result row"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// `r10status`: "As a second background job …" and "As a third background job …" are background
/// jobs, so each becomes a real job — one `list_jobs` can name and `stop_job` can end.
///
/// Measured on 0.1.195: only the first of the three became a job, and only two minutes after it
/// was asked. The other two ran as quick-lane subagents, with no job row and no id, so "cancel
/// the longest" had nothing to cancel and was acted on after the work had already finished.
#[tokio::test]
async fn the_r10_status_asks_each_become_a_job_that_can_be_cancelled() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATBURSTJOBACK00003";
    let host = agent.sync.open_room(chat).await.unwrap();
    let _busy = agent.test_hold_turn(chat).await;

    let asks = [
        "As a background job, count the files under /srv and report the total.",
        "As a second background job, give me the status of the staging folder.",
        "As a third background job, tell me which host answered slowest.",
    ];
    let mut sent = Vec::new();
    for (n, text) in asks.iter().enumerate() {
        if n > 0 {
            tokio::time::sleep(APART).await;
        }
        let (msg_id, cmd) = send_cmd(&host, text);
        agent.handle_command(chat, cmd).await.unwrap();
        sent.push(msg_id);
    }

    let jobs = agent.store.jobs_for_chat(chat).unwrap();
    assert_eq!(
        jobs.len(),
        3,
        "three explicit background-job asks, three jobs: {:?}",
        jobs.iter().map(|j| j.title.as_str()).collect::<Vec<_>>()
    );
    for (n, ask) in sent.iter().enumerate() {
        let job = jobs
            .iter()
            .find(|j| &j.ask_message_id == ask)
            .unwrap_or_else(|| panic!("ask {} became no job", n + 1));
        assert!(
            job.is_open(),
            "job {} is open, so `list_jobs` shows it: {}",
            n + 1,
            job.state
        );
        // The title is the ask without the instruction, so the count is gone with the cue — a
        // status line reading "As a second background job" would name the wrong thing.
        assert!(
            !job.title.to_lowercase().contains("background"),
            "the status line keeps the ask, not the instruction: {:?}",
            job.title
        );
        assert_eq!(
            visible_replies(&host, ask).len(),
            1,
            "and the ask is acknowledged"
        );
    }

    // "Cancel the longest": the oldest of the three, by name, through the sanctioned path.
    let longest = jobs
        .iter()
        .min_by_key(|j| j.created_at)
        .expect("a job to cancel");
    let stopped = agent.stop_jobs(chat, Some(&longest.id)).await;
    assert_eq!(stopped.len(), 1, "the named job was found and stopped");
    assert_eq!(
        stopped[0].state, "cancelled",
        "a cancel is a state, not a command that was issued"
    );
    assert!(
        agent
            .store
            .jobs_for_chat(chat)
            .unwrap()
            .iter()
            .filter(|j| j.is_open())
            .count()
            == 2,
        "the other two keep running"
    );

    let _ = std::fs::remove_dir_all(&root);
}
