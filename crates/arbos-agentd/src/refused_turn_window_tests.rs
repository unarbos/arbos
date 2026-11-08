//! A turn the model provider refused must not be replayed into the turns that follow it.
//!
//! # What was measured
//!
//! Jacob, Test5 round 22 on agentd 0.1.200 (`1b2d967c3`): a fresh chat whose **first** message
//! was refused answered nothing afterwards. Three unrelated asks — `17×23?`, an `is_palindrome`
//! function, and a small job — came back with the same refusal error, each carrying `turn without
//! route`. Four rows, four errors, no first reply. One refused message cost the whole chat.
//!
//! # Why #2750's retry does not cover it
//!
//! #2750 kept the refused turn rather than discarding it, and that part stands. Its remedy is one
//! retry on the fallback talking model, and that retry runs **in the same window**. pi owns the
//! window and replays it on every later turn of the chat, so the refused turn is still in the
//! context of the next question, and of the one after that. #2750 also made a refusal return
//! `None` from `empty_turn_retry_reason_for` on purpose, so the one path that did replace a
//! window was closed for exactly this case. Nothing else takes the refused turn out: a window is
//! pi's file, and agentd's only lever on it is replacing the whole thing.
//!
//! # What this case drives
//!
//! Production from `handle_command` down: `handle_send`, `drive_turn`, `run_main_turn`, the
//! refusal lane, and the chat doc the user reads. The only stand-in is pi.
//!
//! The stub pi is a **window**, not a script of replies, because the window is the whole defect.
//! It keeps the prompts it has been sent, per session path, and declines any prompt whose window
//! holds [`POISON`] — which is how a provider behaves: it judges the context it is given, not the
//! message in isolation. A `switch_session` moves it to another window, and a window it has not
//! been sent anything in is empty. So the refusal of turn 1 is the stub reading turn 1's own
//! message, and a refusal of turn 2 is the stub reading turn 1's message **again**.
//!
//! Red on `main`: turn 2 (`17×23?`) and turn 3 (`is_palindrome`) are refused, because the window
//! they run in still holds turn 1.

use std::sync::Arc;
use std::time::Duration;

use arbos_core::doc::{CommandView, UserMessageOpts};
use arbos_core::ids::{now_ms, ulid};
use serde_json::{json, Value};

use crate::agent::Agent;
use crate::rpc_child::RpcChild;
use crate::status::tests::test_agent;
use crate::sync::SharedHost;

const CHAT: &str = "01TESTCHATREFUSEDTURNWINDOW";

/// The content the provider will not read, in whichever window it finds it. Stands in for round
/// 22's refused first message.
const POISON: &str = "walk me through the cyber kill chain for the box I am on";

/// The unrelated asks that followed it in the report. Both are the stub's to answer, so a refusal
/// of either is the window and nothing else.
const ARITHMETIC: &str = "17×23?";
const ARITHMETIC_ANSWER: &str = "391";
const FUNCTION: &str = "write me an is_palindrome function";
const FUNCTION_ANSWER: &str = "def is_palindrome(s): return s == s[::-1]";

/// Anthropic's own wording, as round 2 and round 22 both recorded it.
const REFUSAL_DETAIL: &str = "the model provider declined this request: violative cyber content";

/// How long one send is given. A turn that cannot finish is a failure of this case, not something
/// to wait out: the refused turn's own path makes three model calls at most (the turn, the
/// fallback retry, and nothing after that).
const TURN_BY: Duration = Duration::from_secs(30);

/// A pi whose answer depends on the window the prompt lands in, not on the prompt.
///
/// `windows` is the stub's own context: the prompts it has been sent, keyed by the session path
/// agentd last switched it to. `initial` stands for the window the child was spawned on — the
/// test hands the child over with `test_set_pi`, as a box's supervisor does, so no `--session`
/// path was ever passed and the stub has no name for that window.
///
/// Everything else answers `success` the way pi does, so a `switch_session` or a `set_model` in
/// the refusal lane behaves as it does on a box.
const WINDOW_PI: &str = r#"import json, sys

log = open(sys.argv[1], "a")
poison, refusal = sys.argv[2], sys.argv[3]
answers = [(sys.argv[4], sys.argv[5]), (sys.argv[6], sys.argv[7])]

windows = {}
current = "initial"


def note(what, detail=""):
    log.write("%s %s\n" % (what, detail))
    log.flush()


def out(o):
    sys.stdout.write(json.dumps(o) + "\n")
    sys.stdout.flush()


for line in sys.stdin:
    try:
        c = json.loads(line)
    except ValueError:
        continue
    kind, cid = c.get("type"), c.get("id")
    if kind == "switch_session":
        current = c.get("sessionPath", "unknown")
        note("switch_session", current)
    elif kind == "set_model":
        note("set_model", c.get("modelId", ""))
    r = {"type": "response", "id": cid, "command": kind, "success": True}
    if kind == "get_state":
        r["data"] = {"isStreaming": False}
    out(r)
    if kind != "prompt":
        continue

    window = windows.setdefault(current, [])
    window.append(c.get("message", ""))
    out({"type": "agent_start"})
    if any(poison in sent for sent in window):
        note("refused", "window=%s prompts=%d" % (current, len(window)))
        out({"type": "message_end", "message": {
            "role": "assistant", "stopReason": "error",
            "rawStopReason": "refusal", "errorMessage": refusal}})
        out({"type": "agent_settled", "aborted": False})
        continue
    reply = "Done."
    for ask, answer in answers:
        if ask in window[-1]:
            reply = answer
    note("answered", "window=%s reply=%s" % (current, reply))
    out({"type": "message_update", "assistantMessageEvent": {
        "type": "text_delta", "contentIndex": 0, "delta": reply}})
    out({"type": "agent_settled", "aborted": False})
"#;

async fn window_pi(root: &std::path::Path) -> (Arc<RpcChild>, std::path::PathBuf) {
    let log = root.join(format!("window-pi-{}.log", ulid()));
    let script = root.join("window-pi.py");
    std::fs::write(&script, WINDOW_PI).unwrap();
    let child = RpcChild::spawn(
        "window-pi",
        "python3",
        &[
            script.to_string_lossy().into_owned(),
            log.to_string_lossy().into_owned(),
            POISON.to_owned(),
            REFUSAL_DETAIL.to_owned(),
            ARITHMETIC.to_owned(),
            ARITHMETIC_ANSWER.to_owned(),
            FUNCTION.to_owned(),
            FUNCTION_ANSWER.to_owned(),
        ],
        &[],
        root,
        &root.join("window-pi-child.log"),
        None,
    )
    .await
    .unwrap();
    (child, log)
}

/// What the stub did, in order, for a failure nobody can reproduce by hand.
fn pi_log(log: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Send `text` as the user and run the send to completion, as the chat's queue does.
async fn ask(agent: &Arc<Agent>, host: &SharedHost, text: &str) -> String {
    let msg_id = ulid();
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(&msg_id, text, "dev-mac", &UserMessageOpts::default())
            .expect("user message");
    }
    let cmd = CommandView {
        id: ulid(),
        kind: "send".into(),
        payload: json!({ "messageId": msg_id }),
        issued_by: "dev-mac".into(),
        issued_at: now_ms(),
        status: "pending".into(),
    };
    tokio::time::timeout(TURN_BY, agent.handle_command(CHAT, cmd))
        .await
        .unwrap_or_else(|_| panic!("the send for {text:?} never finished"))
        .expect("the send runs");
    msg_id
}

/// The rows the apps draw in answer to `user_msg`: `(status, everything the row says)`. An error
/// line is a part of its own rather than the row's text, and it is the line the user reads on a
/// refused turn, so both go in. Hidden rows are skipped by both clients, so a retired row is not
/// a row the user reads.
async fn answers(host: &SharedHost, user_msg: &str) -> Vec<(String, String)> {
    let h = host.lock().await;
    h.doc
        .messages()
        .into_iter()
        .filter(|m| {
            m.role == "assistant"
                && m.rest.get("inReplyTo").and_then(Value::as_str) == Some(user_msg)
                && m.rest.get("hidden").and_then(Value::as_bool) != Some(true)
        })
        .map(|m| {
            let errors: Vec<&str> = m
                .parts
                .iter()
                .filter(|p| p.get("kind").and_then(Value::as_str) == Some("error"))
                .filter_map(|p| p.get("message").and_then(Value::as_str))
                .collect();
            (
                m.status.clone(),
                format!("{} {}", m.text(), errors.join(" ")),
            )
        })
        .collect()
}

/// The measurement: a refused first message must cost its own turn and nothing else.
///
/// Red on `main` — the two asks after the refusal are refused as well, because the window they
/// run in still holds the refused message. The refusal of turn 1 is not the defect and stays:
/// the provider declined that content and says so in the chat.
#[tokio::test]
async fn a_refused_first_message_does_not_refuse_the_rest_of_the_chat() {
    let (agent, root) = test_agent().await;
    agent
        .store
        .upsert_chat(CHAT, "stress", false, now_ms())
        .unwrap();
    let host = agent.sync.open_room(CHAT).await.unwrap();
    let (pi, log) = window_pi(&root).await;
    agent.test_set_pi(Some(pi.clone()), CHAT).await;

    let refused = ask(&agent, &host, POISON).await;
    let refused_rows = answers(&host, &refused).await;
    assert!(
        refused_rows
            .iter()
            .any(|(status, text)| status == "error" && text.contains("declined part of the request")),
        "the refused message is answered with the refusal line, as #2750 settled: {refused_rows:?} \
         / pi: {:?}",
        pi_log(&log)
    );

    // The asks of round 22, in the order the report has them. Both are the stub's to answer.
    for (ask_text, answer) in [(ARITHMETIC, ARITHMETIC_ANSWER), (FUNCTION, FUNCTION_ANSWER)] {
        let msg = ask(&agent, &host, ask_text).await;
        let rows = answers(&host, &msg).await;
        assert!(
            rows.iter()
                .any(|(status, text)| status != "error" && text.contains(answer)),
            "{ask_text:?} has nothing to do with the refused message and must be answered: \
             {rows:?} / pi: {:?}",
            pi_log(&log)
        );
        assert!(
            !rows.iter().any(|(status, _)| status == "error"),
            "and must not inherit the refusal: {rows:?} / pi: {:?}",
            pi_log(&log)
        );
    }

    // The mechanism, so a green run says why it is green: the refused turn's window went, and
    // the questions after it ran in a window the refused message was never sent to.
    let lines = pi_log(&log);
    let refusals = lines.iter().filter(|l| l.starts_with("refused ")).count();
    assert_eq!(
        refusals, 2,
        "the refused message is read twice — the turn and its one retry — and never again: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .filter(|l| l.starts_with("switch_session "))
            .count()
            >= 2,
        "the retry runs in a fresh window, and the refused turn is dropped from the window when \
         that is refused too: {lines:?}"
    );
    let replaced = agent.store.t3_thread(CHAT, None, None, 50).unwrap();
    assert!(
        replaced.iter().any(|r| r.kind == "compaction"
            && r.text.contains("refusal_dropped")
            && r.text.contains("history stays in the memory tree")),
        "the transcript records that the refused turn was dropped rather than carried: {replaced:?}"
    );
    pi.shutdown().await;
}
