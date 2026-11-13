//! An acknowledgement is not an answer: a turn that promised and lost its child must not be
//! written down as a turn that delivered.
//!
//! # The arm
//!
//! `drive_turn` ends a turn that lost the main pi child by asking whether it had already spoken
//! (`agent.rs`, bug V):
//!
//! ```text
//! let spoke = answer_delivered(&turn);
//! match &result {
//!     Err(e) if lost_child && spoke => (MessageStatus::Done, None),
//!     Err(e)                        => (MessageStatus::Error, Some(e.to_string())),
//! }
//! ```
//!
//! Bug V's own case is the right one to keep: run 2's act turn had acked **and started its job**
//! before the child went, so the words were in the chat and the work was under way, and rewriting
//! that row to `error` told the user nothing true.
//!
//! **`answer_delivered` only measures the first half of that.** It counted a non-empty
//! `ack_text`, and `ack_text` is filled by `write_ack` from the route call's streaming arguments
//! — before anything is shown, and whether or not anything ever is. On an `act` route nothing
//! ever is: `ack_bubble` returns `None` for `act`, so the sentence goes to the working row's
//! activity, which the end of the turn clears.
//!
//! So a turn whose route call never closed could acknowledge a request, show the user nothing
//! that outlives the turn, start nothing, and be recorded as having answered.
//!
//! # Why nothing else caught it
//!
//! Four mechanisms could have, and each declined for a different reason:
//!
//! | | |
//! |---|---|
//! | `promote_act_ack` | returns early unless `route` is `Some(Act)`, and this turn has no route |
//! | `quiet_in_shared_chat` | requires `ack_text` to be **empty**, and it is not |
//! | `trim_wordless_result_row` | requires `ack_posted`, and it is **false** |
//! | `answers_message` | has no text requirement, so the empty `done` row counts as an answer |
//!
//! The middle two read the same `TurnState` and reach opposite conclusions: one decides the turn
//! spoke, the other decides it posted nothing. **That disagreement is the defect**, and the fix
//! is to settle which of them is right rather than to add a third opinion.
//!
//! `promote_act_ack` is why this is not a narrow window. It exists to turn an act turn's activity
//! line into a real bubble once the work has run a few seconds, so one would expect the exposure
//! to end there — but it is gated on a resolved `Act` route, which this shape never has. The
//! timer never applies.
//!
//! # What the empty row then costs
//!
//! `answers_message` is what `handle_command` asks before re-running a send that lost its child,
//! and it requires only an assistant row that replies to the message, is not `error`, is not
//! `cancelled` and is not hidden. A finished wordless row satisfies all four. So the row the turn
//! left behind is itself what stops the message being tried again — the user is shown nothing and
//! the send is never repeated.
//!
//! # What these cases measure
//!
//! A scripted pi child opens a `route` tool call, streams its `ack` argument, and never closes
//! the call, so nothing is routed and no job or subagent can exist. It is then lost, and a
//! healthy one is put back as the box's own supervisor would. Everything below that is
//! production: `handle_command`, `handle_send`, `drive_turn`, `run_main_turn` and the chat doc
//! the user reads.
//!
//! The premise is measured as well as the outcome. The live `TurnState` is read at the instant
//! the child is taken away, and the case only means anything if that state is *acknowledgement
//! produced, acknowledgement not posted*. Without it this would be the plain
//! `lost_child && !spoke` shape, green since bug V, and the whole thing would be a vacuous pass.

use std::sync::Arc;
use std::time::Duration;

use arbos_core::doc::{CommandView, UserMessageOpts};
use arbos_core::ids::{now_ms, ulid};
use serde_json::{json, Value};

use crate::agent::{answer_delivered, Agent, TurnState};
use crate::rpc_child::RpcChild;
use crate::status::tests::{test_agent, Journal};
use crate::sync::SharedHost;

const CHAT: &str = "01TESTCHATACKISNOTANANSWER";

/// What the first child acknowledges with. An `act` ack, which is the shape `JOBS_SECTION`
/// directs the model to for anything over about ten seconds — so this is the ordinary path for
/// job work, not an exotic one.
const ACK_ARGS: &str = r#"{"action": "act", "ack": "Starting the benchmark now.", "#;

/// What the respawned child answers with once the send is run again.
const SECOND_REPLY: &str = "The benchmark finished: 12,400 ops/s.";

/// How long the re-run is given to produce an answer. `PI_RESPAWN_WAIT` is 45 s, so a test that
/// let the wait expire would spend that in the suite for no signal; the supervisor below hands a
/// child back instead, which is also what a real box does.
const ANSWER_BY: Duration = Duration::from_secs(20);

/// A pi child that opens a `route` tool call, streams the `ack` argument of it, and then never
/// closes the call.
///
/// Leaving the call open is the point: `route` is never validated, so `TurnState::route` stays
/// `None`, nothing is dispatched, and the only thing the turn produced is the acknowledgement.
///
/// It does not exit on its own — [`lose_the_child_once_it_has_acked`] ends it, and that function
/// says why.
async fn acking_pi(root: &std::path::Path) -> Arc<RpcChild> {
    scripted_pi(root, "ack-only", ACK_ARGS, true).await
}

/// `reply` is streamed as a `text_delta` when `ack_only` is false — a healthy child answering a
/// message — and as the open `route` call's arguments when it is true.
async fn scripted_pi(
    root: &std::path::Path,
    tag: &str,
    reply: &str,
    ack_only: bool,
) -> Arc<RpcChild> {
    let script = root.join(format!("ack-fake-pi-{tag}.py"));
    std::fs::write(
        &script,
        r#"import sys, json
reply = sys.argv[1]
ack_only = sys.argv[2] == "1"

def note(s):
    sys.stderr.write(s + "\n")
    sys.stderr.flush()

def out(o):
    sys.stdout.write(json.dumps(o) + "\n")
    sys.stdout.flush()

def event(e):
    out({"type": "message_update", "assistantMessageEvent": e})

note("up")
for line in sys.stdin:
    try:
        c = json.loads(line)
    except ValueError:
        note("unparsed: " + line[:120])
        continue
    t = c.get("type")
    note("got " + str(t))
    r = {"type": "response", "id": c.get("id"), "command": t, "success": True}
    if t == "get_state":
        r["data"] = {"isStreaming": False}
    out(r)
    if t != "prompt":
        continue
    out({"type": "agent_start"})
    if ack_only:
        event({"type": "toolcall_start", "contentIndex": 0,
               "id": "tc-route-1", "toolName": "route"})
        event({"type": "toolcall_delta", "contentIndex": 0, "delta": reply})
        continue
    event({"type": "text_delta", "contentIndex": 0, "delta": reply})
    out({"type": "agent_settled", "aborted": False})
"#,
    )
    .unwrap();
    RpcChild::spawn(
        &format!("fake-pi-{tag}"),
        "python3",
        &[
            script.to_string_lossy().into_owned(),
            reply.to_owned(),
            if ack_only { "1" } else { "0" }.to_owned(),
        ],
        &[],
        root,
        &root.join(format!("fake-pi-{tag}.log")),
        None,
    )
    .await
    .unwrap()
}

/// Loses the child the moment it has acknowledged, and stands in for the box's pi supervisor by
/// putting a healthy one back.
///
/// **Both halves are ordered on the ack rather than on the clock**, and that is the whole reason
/// this task exists rather than a child that exits by itself.
///
/// A child that acknowledges and then exits by itself leaves the agent two signals on two paths —
/// the streamed delta and the end of the process — and on a loaded machine the second can win.
/// The turn then takes the `lost_child && !spoke` branch, which has been green since bug V, and
/// the case quietly measures a path it was not written for. That is not hypothetical: it is how
/// this case first failed, once under the whole suite and not once on its own.
///
/// So the scripted child does not exit. It is closed here, once the live `TurnState` is holding
/// the acknowledgement, which makes *acknowledged, then lost* a fact of the run rather than a
/// hope about scheduling. The turn's own fields are the signal rather than a log line, because
/// `maybe_split_ack` decides separately whether to say anything, and this case is about the ack
/// existing, not about it being announced.
///
/// The healthy child goes in immediately afterwards, so the re-run's `wait_for_ready_pi` is
/// satisfied at once instead of sitting out `PI_RESPAWN_WAIT`'s 45 seconds — which is the same
/// service a real box's supervisor does for a real user.
fn lose_the_child_once_it_has_acked(
    agent: &Arc<Agent>,
    root: std::path::PathBuf,
    acking: Arc<RpcChild>,
) -> (tokio::task::JoinHandle<()>, Acked) {
    let agent = agent.clone();
    let acked: Acked = Arc::default();
    let seen = acked.clone();
    (
        tokio::spawn(async move {
            loop {
                if let Some(t) = agent.turn.lock().await.as_ref() {
                    if !t.ack_text.trim().is_empty() {
                        *seen.lock().expect("ack slot") = Some((t.ack_text.clone(), t.ack_posted));
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            acking.shutdown().await;
            let fresh = scripted_pi(&root, "respawn", SECOND_REPLY, false).await;
            agent.test_set_pi(Some(fresh), CHAT).await;
        }),
        acked,
    )
}

/// The first attempt's `(ack_text, ack_posted)`, read off the live `TurnState` at the instant the
/// child is taken away. The turn is cleared when it ends, so the premise has to be captured while
/// it is true rather than inferred afterwards.
type Acked = Arc<std::sync::Mutex<Option<(String, bool)>>>;

/// What the scripted children were doing when a case gave up, for a failure nobody can reproduce
/// by hand. The first child's own log is the part worth having: a timeout with a live child that
/// logged nothing is a child that never got its prompt, which is a different fault from a child
/// that got it and said the wrong thing.
fn scripted_pi_state(root: &std::path::Path, acking: &Arc<RpcChild>) -> String {
    let logs: Vec<String> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "log"))
        .map(|p| {
            let body = std::fs::read_to_string(&p).unwrap_or_default();
            format!("--- {} ---\n{}", p.display(), body.trim())
        })
        .collect();
    // `is_alive` is the reader task's view, and a reader that never ran reports a dead child as
    // alive. The kernel is the second opinion, and the two disagreeing is itself the diagnosis.
    let pid = acking.pid();
    let in_proc = pid.is_some_and(|p| std::path::Path::new(&format!("/proc/{p}")).exists());
    format!(
        "first child: pid={pid:?}, reader says alive={}, kernel says present={in_proc}\n{}",
        acking.is_alive(),
        logs.join("\n")
    )
}

/// Whether the turn ever saw the child's words. An empty `ack_text` says the streamed arguments
/// never reached the turn at all, which is a different fault from arguments that arrived and were
/// read wrongly.
async fn turn_state_line(agent: &Arc<Agent>) -> String {
    match agent.turn.lock().await.as_ref() {
        None => "no turn in flight".into(),
        Some(t) => format!(
            "ack_text={:?} streamed={:?} route={:?} row_closed={}",
            t.ack_text, t.streamed_text, t.route, t.row_closed
        ),
    }
}

async fn send_cmd(host: &SharedHost, text: &str) -> (String, CommandView) {
    let msg_id = ulid();
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(&msg_id, text, "dev-mac", &UserMessageOpts::default())
            .expect("user message");
    }
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

/// Every assistant row the apps draw that answers `user_msg`. Hidden rows are skipped by both
/// clients, so a retired row is not a row the user sees.
async fn visible_answers(host: &SharedHost, user_msg: &str) -> Vec<(String, String, String)> {
    let h = host.lock().await;
    h.doc
        .messages()
        .into_iter()
        .filter(|m| {
            m.role == "assistant"
                && m.rest.get("inReplyTo").and_then(Value::as_str) == Some(user_msg)
                && m.rest.get("hidden").and_then(Value::as_bool) != Some(true)
        })
        .map(|m| (m.id.clone(), m.status.clone(), m.text()))
        .collect()
}

/// The measurement. A turn acknowledges the request, loses its child before the route call
/// closes, and so starts nothing and leaves nothing on screen.
///
/// The whole chain is asserted, not the predicate: the row the attempt opened is retired rather
/// than finished `done`, the loss is recorded as `pi_child_lost`, the send is run again, and the
/// user ends with one answer that came from the second child.
#[tokio::test]
async fn an_acknowledged_turn_that_started_nothing_is_not_an_answer() {
    let (agent, root) = test_agent().await;
    let journal = Journal::default();
    let _capture = journal.capture_info();
    let host = agent.sync.open_room(CHAT).await.unwrap();

    let acking = acking_pi(&root).await;
    agent.test_set_pi(Some(acking.clone()), CHAT).await;
    let (_loss, acked) = lose_the_child_once_it_has_acked(&agent, root.clone(), acking.clone());

    let (msg_id, cmd) = send_cmd(&host, "run the long benchmark for me").await;
    let answered = tokio::time::timeout(ANSWER_BY, agent.handle_command(CHAT, cmd)).await;
    let log = journal.text();
    assert!(
        answered.is_ok(),
        "the re-run must answer without waiting out `PI_RESPAWN_WAIT`.\n{}\nturn: {}\nJournal:\n{log}",
        scripted_pi_state(&root, &acking),
        turn_state_line(&agent).await
    );

    // The premise, and what stops this becoming a pass about something else. The first attempt
    // must really have acknowledged the ask and really not have posted it — that is the state
    // the arm misreads. Without it the child contributed nothing at all, which is the
    // `lost_child && !spoke` case that has been green since bug V, and the whole case would be
    // asserting a path it never took.
    let acked = acked.lock().expect("ack slot").clone();
    assert!(
        matches!(&acked, Some((text, posted)) if !text.trim().is_empty() && !posted),
        "premise: the lost attempt had produced an ack and had not posted one, got {acked:?}"
    );

    // 1. The attempt's row is not kept as an answer. This is the line the arm writes when it
    //    takes the keep-done branch, and it must not appear for a turn that started nothing.
    assert!(
        !log.contains("its row keeps `done`"),
        "an acknowledgement with nothing behind it must not keep the row as an answer. \
         Journal:\n{log}"
    );

    // 2. The loss is recorded as what it is, and the row is retired.
    assert!(
        log.contains("row retired, send re-run"),
        "the lost-child path must retire the row and re-run the send. Journal:\n{log}"
    );
    let lost = agent
        .store
        .t3_thread(CHAT, None, None, 200)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.kind == "pi_child_lost")
        .count();
    assert_eq!(
        lost, 1,
        "exactly one `pi_child_lost` record for the one child this case loses"
    );

    // 3. The send was run again, and the user has the second child's answer rather than an empty
    //    row. One row, not two: the retired one is hidden and must not be drawn beside it.
    let answers = visible_answers(&host, &msg_id).await;
    assert_eq!(
        answers.len(),
        1,
        "the ask ends with exactly one row the user sees: {answers:?}"
    );
    assert_eq!(
        answers[0].1, "done",
        "and it is a finished row: {answers:?}"
    );
    assert!(
        answers[0].2.contains("12,400 ops/s"),
        "and it holds the answer the second child gave, not an empty row and not an error: {:?}",
        answers[0]
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The narrowing itself, pinned apart from the chain so a future reader can see which of the two
/// fields the arm is meant to read and why they are not interchangeable.
///
/// `write_ack` fills `ack_text` from the route call's streaming arguments, before anything is
/// shown and whether or not anything ever is. `ack_posted` is set only where a bubble is actually
/// appended to the row. The arm is about what the chat holds, so it is the second it wants.
#[test]
fn a_buffered_acknowledgement_is_not_a_delivered_one() {
    let mut t = TurnState::claim(CHAT, "m-1", false);
    t.ack_text = "Starting the benchmark now.".into();
    assert!(
        !t.ack_posted,
        "an ack on the activity line is never posted as a bubble"
    );
    assert!(
        !answer_delivered(&t),
        "an acknowledgement the chat does not hold is not an answer"
    );

    t.ack_posted = true;
    assert!(
        answer_delivered(&t),
        "once it is a bubble in the chat it is, and bug V's row must still keep its `done`"
    );
}
