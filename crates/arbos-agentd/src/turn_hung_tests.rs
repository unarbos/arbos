//! A main turn whose child will not end its run is ended for it.
//!
//! At the deadline the box posts its notice and sends one abort. A child that acknowledges the
//! abort and keeps the run open used to hold the turn, and every later message in the workspace,
//! for as long as it lived.

use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::Ordering;
use std::time::Duration;

use arbos_core::doc::{CommandView, UserMessageOpts};
use arbos_core::ids::{now_ms, ulid};
use arbos_core::route::ValidRoute;
use serde_json::{json, Value};

use crate::agent::{helper_follows, TurnState, TURN_HUNG_NOTICE, TURN_LIMIT_NOTICE};
use crate::status::tests::test_agent_with_pi;
use crate::sync::SharedHost;

const CHAT: &str = "01TESTCHATTURNHUNG00000001";

/// What the first child does with its first prompt is in `mode`, beside the script. `hang`: it
/// starts a tool of its own (pid recorded) and never ends the run, whatever it is told; that
/// child also has a "server" running from before the turn, as an earlier turn would have left.
/// `busy`: it reports tool output five times a second for longer than the deadline and the
/// grace together, then answers. `slow`: it is silent past the deadline, ignores the abort, and
/// goes on reporting with gaps longer than the loop's tick before it answers. Every later child
/// answers at once.
const STUB_PI: &str = r#"#!/usr/bin/env python3
import json, os, subprocess, sys, threading, time

here = os.path.dirname(os.path.abspath(__file__))
counter = os.path.join(here, "starts")
try:
    n = int(open(counter).read().strip())
except Exception:
    n = 0
n += 1
with open(counter + ".tmp", "w") as f:
    f.write(str(n))
os.replace(counter + ".tmp", counter)

writing = threading.Lock()

def out(o):
    with writing:
        sys.stdout.write(json.dumps(o) + "\n")
        sys.stdout.flush()

def background(name):
    # A process in a group of its own, the way a tool runner starts a command; pid recorded.
    p = subprocess.Popen(["sleep", "600"], process_group=0,
                         stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    with open(os.path.join(here, name + ".tmp"), "w") as f:
        f.write(str(p.pid))
    os.replace(os.path.join(here, name + ".tmp"), os.path.join(here, name))

# What an earlier turn left running on purpose: a dev server, started before this turn began.
if n == 1 and open(os.path.join(here, "mode")).read().strip() == "hang":
    background("server")

hung = False
for line in sys.stdin:
    try:
        c = json.loads(line)
    except ValueError:
        continue
    t = c.get("type")
    r = {"type": "response", "id": c.get("id"), "command": t, "success": True}
    if t == "get_state":
        r["data"] = {"isStreaming": hung}
    out(r)
    if t == "get_state":
        # What the real extension writes once it is loaded; the box reads a child as ready by it.
        path = os.environ.get("ARBOS_AUDIT_PATH")
        if path:
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "a") as f:
                f.write(json.dumps({"kind": "extension_loaded", "ts": int(time.time() * 1000)}) + "\n")
    if t != "prompt":
        continue
    out({"type": "agent_start"})
    mode = open(os.path.join(here, "mode")).read().strip()
    if n == 1 and mode == "hang":
        hung = True
        background("tool")
        continue
    if n == 1 and mode == "busy":
        for _ in range(25):
            out({"type": "tool_execution_update", "toolCallId": "t1", "toolName": "bash", "partialResult": {}})
            time.sleep(0.2)
        out({"type": "message_update",
             "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": "the long build finished"}})
        out({"type": "agent_settled", "aborted": False})
        continue
    if n == 1 and mode == "slow":
        # On a thread, so the abort is acknowledged at once (and ignored) while the run goes on.
        def run():
            time.sleep(1.6)
            for _ in range(4):
                out({"type": "tool_execution_update", "toolCallId": "t1", "toolName": "bash", "partialResult": {}})
                time.sleep(1.3)
            out({"type": "message_update",
                 "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": "the slow run finished"}})
            out({"type": "agent_settled", "aborted": False})
        threading.Thread(target=run, daemon=True).start()
        continue
    out({"type": "message_update",
         "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": "a fresh child answered"}})
    out({"type": "agent_settled", "aborted": False})
"#;

async fn send_cmd(host: &SharedHost, text: &str) -> (String, CommandView) {
    let msg_id = ulid();
    host.lock()
        .await
        .doc
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

async fn replies(host: &SharedHost, user_msg: &str) -> String {
    let h = host.lock().await;
    h.doc
        .messages()
        .into_iter()
        .filter(|m| {
            m.role == "assistant"
                && m.rest.get("inReplyTo").and_then(Value::as_str) == Some(user_msg)
                && m.rest.get("hidden").and_then(Value::as_bool) != Some(true)
        })
        .map(|m| m.text())
        .collect::<Vec<_>>()
        .join("\n")
}

/// A box whose turns have a one-second deadline and a one-second grace, on the stub above.
async fn a_box(
    mode: &str,
) -> (
    std::sync::Arc<crate::agent::Agent>,
    SharedHost,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let dir = std::env::temp_dir().join(format!("arbos-turn-hung-{}", ulid()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("mode"), mode).unwrap();
    let pi_bin = dir.join("stub-pi.py");
    std::fs::write(&pi_bin, STUB_PI).unwrap();
    std::fs::set_permissions(&pi_bin, std::fs::Permissions::from_mode(0o755)).unwrap();

    let (agent, root) = test_agent_with_pi(&pi_bin.display().to_string()).await;
    agent.turn_deadline_ms.store(1_000, Ordering::SeqCst);
    agent.turn_abort_grace_ms.store(1_000, Ordering::SeqCst);
    std::fs::write(
        agent.cfg.state_dir.join(crate::config::PI_BACKOFF_MS_FILE),
        "40",
    )
    .unwrap();
    agent
        .store
        .upsert_chat(CHAT, "hung turn", false, now_ms())
        .unwrap();
    let host = agent.sync.open_room(CHAT).await.unwrap();
    agent.start_pi(CHAT).await.expect("the first child boots");
    (agent, host, dir, root)
}

/// Running, as opposed to gone or killed and waiting for whoever inherited it to reap it.
fn alive(pid: i32) -> bool {
    let zombie = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            stat.rsplit_once(')')
                .map(|(_, rest)| rest.trim_start().starts_with('Z'))
        })
        .unwrap_or(false);
    !zombie && nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_whose_child_ignores_the_abort_ends_in_words_and_the_next_message_is_answered() {
    let (agent, host, dir, root) = a_box("hang").await;

    let (first, cmd) = send_cmd(&host, "do the thing that never returns").await;
    let ended = tokio::time::timeout(Duration::from_secs(30), agent.handle_command(CHAT, cmd))
        .await
        .is_ok();
    let said = replies(&host, &first).await;

    // The workspace is free again: the next message gets a fresh child and an answer.
    let (second, cmd) = send_cmd(&host, "are you there?").await;
    let answered = ended
        && tokio::time::timeout(Duration::from_secs(30), agent.handle_command(CHAT, cmd))
            .await
            .is_ok();
    let mut answer = String::new();
    for _ in 0..600 {
        answer = replies(&host, &second).await;
        if !answered || answer.contains("a fresh child answered") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&root);
    assert!(ended, "the turn was still held 30 s after a 1 s deadline");
    assert!(
        said.contains(TURN_HUNG_NOTICE),
        "the chat is told the turn was ended: {said:?}"
    );
    assert!(
        answer.contains("a fresh child answered"),
        "the next message is answered by a fresh child: {answer:?}"
    );
}

/// The tool that hung the turn is a process of its own, in a group of its own. Closing the
/// child does not reach it; the sweep of the child's session does. What the child was already
/// running when the turn began, a server an earlier turn started, is in that same session and
/// is left alone, as it is when a child is lost any other way.
#[tokio::test(flavor = "multi_thread")]
async fn the_tool_that_hung_the_turn_dies_with_it_and_an_earlier_turns_server_does_not() {
    let (agent, host, dir, root) = a_box("hang").await;
    let server: i32 = std::fs::read_to_string(dir.join("server"))
        .expect("the child had a server running before the turn")
        .parse()
        .unwrap();
    // Well clear of the start-time resolution (a clock tick) before the turn begins.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let (_, cmd) = send_cmd(&host, "do the thing that never returns").await;
    let ended = tokio::time::timeout(Duration::from_secs(30), agent.handle_command(CHAT, cmd))
        .await
        .is_ok();
    let tool: i32 = std::fs::read_to_string(dir.join("tool"))
        .expect("the hung child started its tool")
        .parse()
        .unwrap();
    let mut gone = false;
    for _ in 0..100 {
        gone = !alive(tool);
        if gone {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    if !gone {
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(tool),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
    let server_alive = alive(server);
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(server),
        nix::sys::signal::Signal::SIGKILL,
    );
    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&root);
    assert!(ended, "the turn was still held 30 s after a 1 s deadline");
    assert!(gone, "tool {tool} of the hung turn outlived it");
    assert!(
        server_alive,
        "server {server}, running since before the turn, was killed with it"
    );
}

/// The grace is silence, not time since the abort. A child that ignored the abort and is still
/// reporting, even with gaps longer than the loop's tick, is still working: the turn stays with
/// it until it ends its own run, on the child it started on, as it did before there was a grace
/// at all. (What that turn then shows in the chat is the deadline's business and unchanged.)
#[tokio::test(flavor = "multi_thread")]
async fn a_turn_that_ignores_the_abort_but_keeps_reporting_is_left_to_end_itself() {
    let (agent, host, dir, root) = a_box("slow").await;

    let (ask, cmd) = send_cmd(&host, "run the slow thing").await;
    let began = std::time::Instant::now();
    let ended = tokio::time::timeout(Duration::from_secs(60), agent.handle_command(CHAT, cmd))
        .await
        .is_ok();
    let took = began.elapsed();
    let said = replies(&host, &ask).await;
    let starts = std::fs::read_to_string(dir.join("starts")).unwrap_or_default();

    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&root);
    assert!(ended, "the turn did not end");
    assert!(
        !said.contains(TURN_HUNG_NOTICE),
        "a turn that was still reporting was ended as hung: {said:?}"
    );
    // The child's run is 1.6 s of silence and four reports 1.3 s apart.
    assert!(
        took >= Duration::from_secs(6),
        "the turn ended after {took:?}, before the child ended its run"
    );
    assert_eq!(
        starts.trim(),
        "1",
        "the child was replaced under a working turn"
    );
}

/// A turn that is still reporting is working, and none of the limits apply to it: not the
/// deadline's notice and abort, and not the grace that follows. They are read only in a full
/// second of silence, as they always were, so a long build that streams its output runs to its
/// end on the child it started on.
#[tokio::test(flavor = "multi_thread")]
async fn a_turn_that_keeps_reporting_runs_past_the_deadline_to_its_answer() {
    let (agent, host, dir, root) = a_box("busy").await;
    // The limits are read after this long with no event. Three seconds here, against a child
    // that reports five times a second, so a runner that stalls for a second or two does not
    // open the window by itself; an arm that fired on a timer instead would still fire well
    // inside the five seconds the child reports for.
    agent.turn_quiet_tick_ms.store(3_000, Ordering::SeqCst);

    let (ask, cmd) = send_cmd(&host, "run the long build").await;
    let ended = tokio::time::timeout(Duration::from_secs(30), agent.handle_command(CHAT, cmd))
        .await
        .is_ok();
    let said = replies(&host, &ask).await;
    let starts = std::fs::read_to_string(dir.join("starts")).unwrap_or_default();

    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&root);
    assert!(ended, "the turn did not end");
    assert!(
        said.contains("the long build finished"),
        "the turn ran to its own answer: {said:?}"
    );
    assert!(
        !said.contains(TURN_LIMIT_NOTICE) && !said.contains(TURN_HUNG_NOTICE),
        "a turn that was reporting was treated as over its limit: {said:?}"
    );
    assert_eq!(
        starts.trim(),
        "1",
        "the child was replaced under a working turn"
    );
}

/// The hung notice is held back only when a helper really follows the turn. A reply, or a turn
/// that never chose a route, has nobody behind it even when the deadline set `escalate`.
#[test]
fn only_a_turn_with_a_helper_behind_it_is_handed_off() {
    let turn = |route: Option<ValidRoute>, escalate: bool, act_to_job: bool| {
        let mut t = TurnState::claim(CHAT, "user-msg", false);
        t.route = route;
        t.escalate = escalate;
        t.act_to_job = act_to_job;
        t
    };
    let act = || {
        Some(ValidRoute::Act {
            plan: "run it".into(),
        })
    };
    let reply = || Some(ValidRoute::Reply { reply: None });
    let wake = || {
        Some(ValidRoute::Wake {
            subagent_id: "s1".into(),
            brief: "go on".into(),
        })
    };
    assert!(helper_follows(&turn(act(), true, false)));
    assert!(helper_follows(&turn(act(), false, true)));
    assert!(helper_follows(&turn(wake(), false, false)));
    assert!(!helper_follows(&turn(act(), false, false)));
    assert!(!helper_follows(&turn(reply(), true, false)));
    assert!(!helper_follows(&turn(None, true, false)));
    let mut stopped = turn(act(), true, false);
    stopped.stopped = true;
    assert!(!helper_follows(&stopped));
}
