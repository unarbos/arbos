//! Bug V: a message that lands as a turn ends is answered, and the turn that just ended keeps
//! its row.
//!
//! Jacob on Test5, round 7 (2026-11-05, agentd 0.1.193 `3595b9fe0`, side chat
//! `01M6ME9A7EGV5PJ7DZW43AHTN5`, scenario `r7correct`, **run 2** — the rerun after our box was
//! reset; Test5's own box was never restarted, so the crashes are Test5's). He corrected a
//! running job 0.2 s after the act turn's last row. The chat recorded this, in this order:
//!
//! | 00:09:35.298 | the act turn's ack row |
//! | 00:09:35.463 | `command failed {cmd: send, error: pi main child exited mid-turn}` |
//! | 00:09:35.577 | the correction, `01M6N57DJKBTRKETN9EF423B2V` |
//! | 00:09:35.833 | `main turn ended before pi settled; the next prompt reclaims pi first` |
//! | 00:09:35.834 | `turn_metrics route none 0 ms` |
//! | 00:09:35.878 | `Broken pipe (os error 32)` |
//! | 00:09:37.128, 00:09:41.583 | `pi main child is gone; respawning`, twice |
//!
//! The correction was never answered and never retried. The act turn, which had already acked
//! and started its job, was rewritten to `error`. The same chain ran again at 00:13:37.434 and
//! lost `01M6N5ET4Z7XNG8QWM76YGS9BJ`. In run 1 the identical correction 0.35 s after a turn end
//! steered the job cleanly, so it is a race and not a deterministic failure.
//!
//! **Where the errno comes from.** `RpcChild::call` writes the command to the child's stdin and
//! passes the write's error up untouched. A child that has already exited leaves a closed pipe,
//! so the write is `EPIPE` and the chat showed the user "Broken pipe (os error 32)". Nothing
//! about that is a judgement on the message.
//!
//! **What these tests stand in for.** A test box has no pi binary (`pi_bin` points at nothing),
//! so there is no main session and no supervisor to respawn one. Both cases script a child that
//! speaks the RPC protocol and run the box's supervisor rule — respawn a dead child — as a task
//! of the test. Everything else is production: `handle_command`, `handle_send`, `drive_turn`,
//! `run_main_turn` and the chat doc the user reads.

use std::sync::Arc;
use std::time::{Duration, Instant};

use arbos_core::doc::{CommandView, UserMessageOpts};
use arbos_core::ids::{now_ms, ulid};
use serde_json::{json, Value};

use crate::agent::Agent;
use crate::rpc_child::RpcChild;
use crate::status::tests::test_agent;
use crate::sync::SharedHost;

/// The window Jacob's correction landed in: 0.2 s after the act turn's last row.
const RACE_WINDOW: Duration = Duration::from_millis(200);

/// How long the fix is given to answer the lost message, from the spec's success criterion.
const ANSWER_BY: Duration = Duration::from_secs(15);

/// How long the box's supervisor takes to notice a dead child, as `start_pi` polls.
const SUPERVISOR_POLL: Duration = Duration::from_secs(2);

/// A pi child that answers the RPC protocol well enough to drive one main turn.
///
/// `reply` is streamed as a `text_delta`, which is how the model's prose reaches a turn. With
/// `die_before_settling` the child writes that prose and then exits without `agent_settled` —
/// the shape of run 2's act turn, where `run_main_turn` sees the child go while it is still
/// listening and reports `pi main child exited mid-turn`.
async fn scripted_pi(
    root: &std::path::Path,
    tag: &str,
    reply: &str,
    die_before_settling: bool,
) -> Arc<RpcChild> {
    let script = root.join("bug-v-fake-pi.py");
    std::fs::write(
        &script,
        r#"import sys, json
reply = sys.argv[1]
die = sys.argv[2] == "1"

def out(o):
    sys.stdout.write(json.dumps(o) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    try:
        c = json.loads(line)
    except ValueError:
        continue
    t = c.get("type")
    r = {"type": "response", "id": c.get("id"), "command": t, "success": True}
    if t == "get_state":
        r["data"] = {"isStreaming": False}
    out(r)
    if t != "prompt":
        continue
    out({"type": "agent_start"})
    out({"type": "message_update",
         "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": reply}})
    if die:
        sys.exit(0)
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
            if die_before_settling { "1" } else { "0" }.to_owned(),
        ],
        &[],
        root,
        &root.join(format!("fake-pi-{tag}.log")),
        None,
    )
    .await
    .unwrap()
}

/// The box's pi supervisor, reduced to the rule bug V turns on: a dead main child is replaced.
///
/// `start_pi`'s loop does this on a two-second poll, and the test keeps that cadence so the
/// re-run waits for a respawn rather than finding one already there. Every replacement answers
/// and settles normally, which is what a healthy box gives the next prompt.
fn stand_in_supervisor(
    agent: &Arc<Agent>,
    root: std::path::PathBuf,
    chat: &'static str,
    reply: &'static str,
) -> tokio::task::JoinHandle<()> {
    let agent = agent.clone();
    tokio::spawn(async move {
        let mut n = 0;
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if agent.test_pi_is_live().await {
                continue;
            }
            tokio::time::sleep(SUPERVISOR_POLL).await;
            if agent.test_pi_is_live().await {
                continue;
            }
            n += 1;
            let fresh = scripted_pi(&root, &format!("respawn{n}"), reply, false).await;
            agent.test_set_pi(Some(fresh), chat).await;
        }
    })
}

/// A user message and the `send` command the apps push with it.
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

/// Every assistant row the apps draw that answers `user_msg`. `hidden` rows are skipped by both
/// clients, so a retired row is not a row the user sees.
async fn visible_answers(host: &SharedHost, user_msg: &str) -> Vec<(String, String)> {
    let h = host.lock().await;
    h.doc
        .messages()
        .into_iter()
        .filter(|m| {
            m.role == "assistant"
                && m.rest.get("inReplyTo").and_then(Value::as_str) == Some(user_msg)
                && m.rest.get("hidden").and_then(Value::as_bool) != Some(true)
        })
        .map(|m| (m.id.clone(), m.text()))
        .collect()
}

/// Every error part in the chat, as `code: text`. Bug V left four of these behind.
async fn error_parts(host: &SharedHost) -> Vec<String> {
    let h = host.lock().await;
    h.doc
        .messages()
        .into_iter()
        .flat_map(|m| m.parts.clone())
        .filter(|p| p.get("kind").and_then(Value::as_str) == Some("error"))
        .map(|p| {
            format!(
                "{}: {}",
                p.get("code").and_then(Value::as_str).unwrap_or(""),
                p.get("message").and_then(Value::as_str).unwrap_or("")
            )
        })
        .collect()
}

async fn row_status(host: &SharedHost, msg_id: &str) -> String {
    let h = host.lock().await;
    h.doc
        .message(msg_id)
        .map(|m| m.status.clone())
        .unwrap_or_default()
}

/// The P1 itself: the main pi child dies the moment an act turn finishes its last row, and the
/// next message arrives inside the same 200 ms window.
///
/// Three things are asserted, and each is one of Jacob's sentences:
///
/// 1. the second message gets a normal reply — not `route none`, not "Broken pipe";
/// 2. the first turn's row, which had already spoken, stays `done`;
/// 3. no `turn_failed` row is left in the chat at all.
#[tokio::test]
async fn a_message_sent_as_the_pi_child_dies_is_answered_and_the_finished_row_stays_done() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATTURNENDRACE00001";
    let host = agent.sync.open_room(chat).await.unwrap();

    // The act turn's own child: it writes the turn's words and then goes, exactly as run 2's
    // did between the ack row at 00:09:35.298 and the warning 165 ms later.
    let dying = scripted_pi(&root, "act", FIRST_REPLY, true).await;
    agent.test_set_pi(Some(dying), chat).await;
    let _supervisor = stand_in_supervisor(&agent, root.to_path_buf(), chat, SECOND_REPLY);

    let (first_msg, first_cmd) = send_cmd(&host, "make a CSV of n squared up to 5000").await;
    let _ = agent.handle_command(chat, first_cmd).await;
    let turn_ended = Instant::now();

    // The act turn said its piece before the child went: that is what makes its row an answer.
    let first_rows = visible_answers(&host, &first_msg).await;
    assert_eq!(
        first_rows.len(),
        1,
        "the act turn leaves one row the user reads: {first_rows:?}"
    );
    assert!(
        first_rows[0].1.contains("cubes"),
        "and that row holds the turn's words: {:?}",
        first_rows[0]
    );
    assert!(
        !agent.test_pi_is_live().await,
        "the child has to be gone before the next message, or this is not bug V's race"
    );

    // 00:09:35.577: the correction, 114 ms after the send command failed.
    assert!(
        turn_ended.elapsed() < RACE_WINDOW,
        "the second message must be enqueued inside the race window, not after it: {:?}",
        turn_ended.elapsed()
    );
    let (second_msg, second_cmd) = send_cmd(&host, "cube them, not square").await;
    let sent = Instant::now();
    agent
        .handle_command(chat, second_cmd)
        .await
        .expect("the correction is answered, not failed");

    // 1. The message that bug V lost is answered.
    let answers = visible_answers(&host, &second_msg).await;
    assert_eq!(
        answers.len(),
        1,
        "the correction gets exactly one answer row: {answers:?}"
    );
    assert!(
        answers[0].1.contains("cubed"),
        "and the answer is the model's, not an error line: {:?}",
        answers[0]
    );
    assert!(
        sent.elapsed() < ANSWER_BY,
        "the answer took {:?}, past the {ANSWER_BY:?} the fix is given",
        sent.elapsed()
    );

    // 2. The turn that had already answered keeps its row.
    assert_eq!(
        row_status(&host, &first_rows[0].0).await,
        "done",
        "the act turn had already spoken; losing the child afterwards must not rewrite its row"
    );

    // 3. Nothing in the chat tells the user something failed.
    let errors = error_parts(&host).await;
    assert!(
        errors.is_empty(),
        "no error row may be left behind, and none may name the errno: {errors:?}"
    );
    // The place in the send order is only this test's to count while nothing else holds one.
    // A job keeps its ask's place until its own task has answered, so a job running in this
    // chat would make the next assertion read 1 for a reason that is not a bug. Nothing here
    // starts one — the scripted child calls no tool, and the burst path needs a running user
    // turn — and this says so out loud rather than leaving it to be rediscovered.
    assert!(
        agent
            .store
            .jobs_for_chat(chat)
            .unwrap_or_default()
            .is_empty(),
        "no job runs in this chat, so the send order holds only what this test put there"
    );
    assert_eq!(
        agent.send_order.waiting(chat),
        0,
        "both messages released their place in the chat's send order"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The act turn's words. Plain statement prose: the boilerplate filter drops tool narration, so
/// an opening like "Let me build that" would never reach the row.
const FIRST_REPLY: &str = "The squares are building now, and the cubes come next.";

/// What the respawned child answers the correction with.
const SECOND_REPLY: &str = "Done — the numbers are cubed, not squared.";

/// The job half of bug V: the workers of job `01M6N57CCJ6YZP596M3QZ6XJFT` died at 00:09:50.965
/// and 00:09:59.584, each right after a codemode MCP call, and the job fell through to an
/// inline `job_fallback` that ran the brief the user had already corrected — it posted the n²
/// table when the ask was n³.
///
/// **What the code says about the report's hunch.** A worker's codemode MCP call is an HTTP
/// request to the workspace's own listener (`Config::mcp_url`, bound in `workspace.rs`), and
/// `deliver_file` resolves its target from `subagent_msgs` by session id. Nothing on that path
/// reaches `Agent::pi`, so the bridge is *not* tied to the main child's lifetime and killing the
/// main child does not kill a worker's MCP call. What ties them is the workspace cgroup: one
/// `memory.max` over every child (`isolation.rs`), with the kernel taking the largest task. That
/// is a shared cause, not a dependency — and the respawn line printing twice, at 00:09:37.128
/// and 00:09:41.583, says the box was still taking pi children twenty-five seconds in.
///
/// So this asserts the two things that actually decide the outcome:
///
/// 1. a fresh worker is not started into the window that has just killed the last one — while
///    the main pi child is down there is no retry worker and no `job_fallback`;
/// 2. a correction the user sent reaches whoever takes the work on next, instead of the brief
///    being run as first written.
#[tokio::test]
async fn a_job_whose_worker_dies_with_the_main_pi_child_waits_for_it_and_keeps_the_correction() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATTURNENDRACE00003";
    let host = agent.sync.open_room(chat).await.unwrap();

    // 00:09:50: the main pi child is gone and the box has not got one back yet.
    let dead = scripted_pi(&root, "job", FIRST_REPLY, true).await;
    dead.shutdown().await;
    agent.test_set_pi(Some(dead), chat).await;

    // A turn holds the chat, which is the state a job's ask arrives in.
    let _busy = agent.test_hold_turn(chat).await;
    let (_ask, cmd) = send_cmd(
        &host,
        "Write a CSV of n squared up to 5000, as a background job (not inline).",
    )
    .await;
    agent.handle_command(chat, cmd).await.unwrap();

    // The job's first worker cannot start (this box has no pi binary), which is the same
    // `child exited` outcome run 2's first worker reached.
    let job = wait_for(Duration::from_secs(10), || {
        agent
            .store
            .jobs_for_chat(chat)
            .unwrap_or_default()
            .into_iter()
            .find(|j| j.restarts > 0)
    })
    .await
    .expect("the job's first worker fails and the job is put back in the queue");
    assert_eq!(job.state, "queued", "the job is waiting for a fresh worker");

    // The other side of the send-order contract, and the reason the lost-message re-run cannot
    // reach this path: `start_job_for_send` answered the command, so `handle_command` returned
    // `Ok` and the re-run — which only runs on a child error — was never reached. The ask's
    // place went with its ack at job start (bug AA), so the chat is owed nothing even while the
    // job is between workers: a job's own waiting is never the chat's.
    assert_eq!(
        agent.send_order.waiting(chat),
        0,
        "the ack answered the ask, so the job must hold no place while it waits for a worker"
    );

    // 1. Nothing is started into the window, and the job is not given up on either.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        retry_workers(&agent, chat).is_empty(),
        "no fresh worker may be started while the main pi child is still gone"
    );
    assert!(
        !fell_back(&agent, chat),
        "and the job must not fall through to the inline fallback during the window"
    );

    // The user's correction, which in run 2 arrived around the same moment.
    agent.test_note_job_steer(&job.id, CORRECTION);

    // The box gets a pi child back, as it did at 00:09:41.583.
    let fresh = scripted_pi(&root, "jobrespawn", SECOND_REPLY, false).await;
    agent.test_set_pi(Some(fresh), chat).await;

    // 2. The fresh worker is started now, and it is told about the correction.
    let retry = wait_for(Duration::from_secs(20), || {
        retry_workers(&agent, chat).into_iter().next()
    })
    .await
    .expect("a fresh worker starts once the box has a pi child again");
    let brief = agent
        .store
        .t3_thread(&retry, None, None, 10)
        .unwrap_or_default()
        .into_iter()
        .find(|r| r.kind == "user")
        .map(|r| r.text)
        .expect("the fresh worker's brief is on its own thread");
    assert!(
        brief.contains(CORRECTION),
        "the fresh worker runs the corrected ask, not the brief as first written: {brief}"
    );
    assert!(
        brief.contains("override the brief above"),
        "and it is told which of the two wins: {brief}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The correction Jacob sent, in the shape r7correct used.
const CORRECTION: &str = "cube them, not square, and add a line chart";

/// The subagent ids of this chat's fresh-worker attempts. `spawn_subagent_task` names a job's
/// retry `job-retry-<last four of the job id>`.
fn retry_workers(agent: &Arc<Agent>, chat: &str) -> Vec<String> {
    agent
        .store
        .subagents()
        .unwrap_or_default()
        .into_iter()
        .filter(|s| s.chat_id.as_deref() == Some(chat) && s.name.starts_with("job-retry"))
        .map(|s| s.id)
        .collect()
}

/// Has this chat given up on its job and done the work inline? `run_job_fallback_turn` writes
/// the `job_fallback` line before it runs.
fn fell_back(agent: &Arc<Agent>, chat: &str) -> bool {
    agent
        .store
        .t3_thread(chat, None, None, 200)
        .unwrap_or_default()
        .iter()
        .any(|r| r.kind == "job_fallback")
}

/// Poll `f` until it yields a value or `bound` runs out.
async fn wait_for<T>(bound: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let t0 = Instant::now();
    loop {
        if let Some(v) = f() {
            return Some(v);
        }
        if t0.elapsed() >= bound {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The replay from the spec: a start_job ask, then three messages at the turn's end, +0, +300
/// and +600 ms. All three must be answered and none may leave a `turn_failed` row.
#[tokio::test]
async fn three_messages_at_a_turn_end_are_all_answered_with_no_failed_rows() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATTURNENDRACE00002";
    let host = agent.sync.open_room(chat).await.unwrap();

    let dying = scripted_pi(&root, "act3", FIRST_REPLY, true).await;
    agent.test_set_pi(Some(dying), chat).await;
    let _supervisor = stand_in_supervisor(&agent, root.to_path_buf(), chat, SECOND_REPLY);

    // An ask in the shape the replay used. It runs inline rather than as a job: the burst path
    // needs a user turn to be running, and this chat has none, so nothing here takes a second
    // place in the send order.
    let (_ask, ask_cmd) = send_cmd(&host, "run the sieve as a job").await;
    let _ = agent.handle_command(chat, ask_cmd).await;

    let mut sent = Vec::new();
    for (n, gap) in [0u64, 300, 600].into_iter().enumerate() {
        if gap > 0 {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        let (msg, cmd) = send_cmd(&host, &format!("correction {}", n + 1)).await;
        agent
            .handle_command(chat, cmd)
            .await
            .unwrap_or_else(|e| panic!("message {} at +{gap} ms failed: {e}", n + 1));
        sent.push(msg);
    }
    for (n, msg) in sent.iter().enumerate() {
        let answers = visible_answers(&host, msg).await;
        assert_eq!(
            answers.len(),
            1,
            "message {} is answered once: {answers:?}",
            n + 1
        );
        assert!(
            answers[0].1.contains("cubed"),
            "message {} gets the model's words: {:?}",
            n + 1,
            answers[0]
        );
    }
    let errors = error_parts(&host).await;
    assert!(errors.is_empty(), "0 turn_failed rows: {errors:?}");
    let _ = std::fs::remove_dir_all(&root);
}
