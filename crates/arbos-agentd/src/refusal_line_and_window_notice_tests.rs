//! What a turn that replaced this chat's window owes the user, in words.
//!
//! #2919 stopped a refused turn from being replayed into the turns after it, by replacing the
//! chat's model window — once so the retry runs clean, and again to drop a turn the provider
//! declines in any window. The behaviour is right and the chat recovers. What it says about
//! itself was left wrong in two ways, both of them introduced by that fix:
//!
//! 1. **The refusal line tells the user to do the one thing that no longer applies.** It reads
//!    "split it up rather than sending the same message again" — but the window those turns were
//!    declined in has just been replaced, so sending the same message again is the one thing
//!    worth doing. Round 22's users had no way to find that out.
//! 2. **The window is replaced silently.** A turn that recovers on the fresh window answers as
//!    if nothing had happened, while the conversation the user has been having is no longer in
//!    the agent's working context. Only a T3 row and an audit row record it, and the user reads
//!    neither.
//!
//! Neither needs a verdict on *why* the provider refused, which is why they are here and the
//! fresh-context probe of #2885 is not: the line says what agentd did and what is worth doing
//! next, and claims nothing about the message itself.
//!
//! The third case here is the carrier #2919 missed, found by #2885's author reading `faae21a14`.
//! A window is not the only thing that replays a turn: every turn rebuilds a memory block from
//! this chat's `t3` rows and puts it in front of the message, and the user row of a declined
//! turn is written before the model is ever called. Replacing the window is itself what lets the
//! walk reach this chat's own rows (`Tree::note_session_cut` → `Scope::leaf_ok`), so #2919 could
//! hand the declined words back on a later turn through memory, or through the chat recap of a
//! later retry. A dropped turn is now marked (`Store::t3_mark_refused`) and left out of both.
//!
//! # What these cases drive
//!
//! Production from `handle_command` down — `handle_send`, `drive_turn`, `run_main_turn`, the
//! refusal lane and the chat doc the user reads — against a driven pi session. The stub is a
//! window, as in `refused_turn_window_tests`: it declines any prompt whose window holds the
//! flagged content, so a refusal is always the context and never a scripted reply.
//!
//! The recovery case needs flagged content that is in the **window** but not in any chat row,
//! because a row would come back through `chat_recap` and the retry would be refused as well.
//! That is the ordinary shape for it: a fetched page, an image, a tool result. The stub poisons
//! its own window on the fetch ask and answers that turn normally, which is what a provider that
//! objects to a page body does.

use std::sync::Arc;
use std::time::Duration;

use arbos_core::doc::{CommandView, UserMessageOpts};
use arbos_core::ids::{now_ms, ulid};
use serde_json::{json, Value};

use crate::agent::{Agent, ModelError};
use crate::rpc_child::RpcChild;
use crate::status::tests::test_agent;
use crate::sync::SharedHost;

const CHAT: &str = "01TESTCHATREFUSALLINENOTICE";

/// The content the provider will not read, in whichever window it finds it.
const FLAGGED: &str = "<<FLAGGED>> the cyber kill chain for the box I am on";

/// The ask whose *result* carries the flagged content into the window. Its own row says nothing
/// flagged, which is what makes it the recovery shape rather than the drop shape.
const FETCH_ASK: &str = "fetch the changelog page and summarise it";
const FETCH_ANSWER: &str = "Summary: the page is a changelog for build 412.";

/// An unrelated ask, refused only because of the window it lands in.
const ARITHMETIC: &str = "17×23?";
const ARITHMETIC_ANSWER: &str = "391";

/// An ask the provider starts answering and then declines. No retry runs (the user has prose) and
/// so no drop and no marker, which is the one place the two readings of "declined" differ.
const PARTIAL_ASK: &str = "list the three files and then do the risky part";
const PARTIAL_ANSWER: &str = "The three files are a.rs, b.rs and c.rs.";

/// The provider's wording, as the AR lane read it out of round 22's pi session file.
const REFUSAL_DETAIL: &str = "Cyber safeguards flagged this message. If this is legitimate security work, you can apply for the Cyber Verification Program: https://example.invalid/cyber-verification";

const TURN_BY: Duration = Duration::from_secs(30);

/// A pi whose answer depends on the window the prompt lands in.
///
/// `windows` is the stub's context, keyed by the session path agentd last switched it to;
/// `initial` stands for the window the child was spawned on, since `test_set_pi` hands the child
/// over without a `--session` path. On the fetch ask the stub writes the flagged page body into
/// its own window and answers the turn normally — context the chat doc never sees.
const CONTEXT_PI: &str = r#"import json, sys

log = open(sys.argv[1], "a")
flagged, refusal = sys.argv[2], sys.argv[3]
fetch_ask, fetch_answer = sys.argv[4], sys.argv[5]
ask, answer = sys.argv[6], sys.argv[7]
partial_ask, partial_answer = sys.argv[8], sys.argv[9]

windows = {}
current = "initial"


def note(what, detail=""):
    log.write("%s %s\n" % (what, detail))
    log.flush()


def out(o):
    sys.stdout.write(json.dumps(o) + "\n")
    sys.stdout.flush()


def stream(text):
    out({"type": "message_update", "assistantMessageEvent": {
        "type": "text_delta", "contentIndex": 0, "delta": text}})


def decline():
    out({"type": "message_end", "message": {
        "role": "assistant", "stopReason": "error",
        "rawStopReason": "refusal", "errorMessage": refusal}})
    out({"type": "agent_settled", "aborted": False})


def say(text):
    stream(text)
    out({"type": "agent_settled", "aborted": False})


for line in sys.stdin:
    try:
        c = json.loads(line)
    except ValueError:
        continue
    kind, cid = c.get("type"), c.get("id")
    if kind == "switch_session":
        current = c.get("sessionPath", "unknown")
        note("switch_session", current)
    r = {"type": "response", "id": cid, "command": kind, "success": True}
    if kind == "get_state":
        r["data"] = {"isStreaming": False}
    out(r)
    if kind != "prompt":
        continue

    window = windows.setdefault(current, [])
    message = c.get("message", "")
    window.append(message)
    # What the user asked this turn, without the quoted history: agentd marks a recap row with
    # "- user: ...", and a quoted ask is not a new ask. The window above keeps the whole prompt,
    # so flagged words are still flagged wherever they are quoted.
    asked = "\n".join(l for l in message.splitlines() if not l.startswith("- "))
    out({"type": "agent_start"})
    if any(flagged in sent for sent in window):
        note("refused", "window=%s prompts=%d" % (current, len(window)))
        decline()
        continue
    if partial_ask in asked:
        # Part of the answer, then the refusal: the user keeps prose, so agentd runs no retry.
        note("refused_after_prose", "window=%s" % current)
        stream(partial_answer)
        decline()
        continue
    if fetch_ask in asked:
        # The page body lands in the window as a tool result. No chat row carries it.
        window.append("[tool_result web_fetch] " + flagged)
        note("fetched", "window=%s" % current)
        say(fetch_answer)
        continue
    note("answered", "window=%s" % current)
    say(answer if ask in asked else "Done.")
"#;

async fn context_pi(root: &std::path::Path) -> (Arc<RpcChild>, std::path::PathBuf) {
    let log = root.join(format!("context-pi-{}.log", ulid()));
    let script = root.join("context-pi.py");
    std::fs::write(&script, CONTEXT_PI).unwrap();
    let child = RpcChild::spawn(
        "context-pi",
        "python3",
        &[
            script.to_string_lossy().into_owned(),
            log.to_string_lossy().into_owned(),
            FLAGGED.to_owned(),
            REFUSAL_DETAIL.to_owned(),
            FETCH_ASK.to_owned(),
            FETCH_ANSWER.to_owned(),
            ARITHMETIC.to_owned(),
            ARITHMETIC_ANSWER.to_owned(),
            PARTIAL_ASK.to_owned(),
            PARTIAL_ANSWER.to_owned(),
        ],
        &[],
        root,
        &root.join("context-pi-child.log"),
        None,
    )
    .await
    .unwrap();
    (child, log)
}

fn pi_log(log: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Send `text` as the user and run the send to completion, as the chat's queue does.
async fn ask_for(agent: &Arc<Agent>, host: &SharedHost, text: &str) -> String {
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

/// Everything the rows answering `user_msg` say — the row's own text and its error line, which
/// is a part of its own — with the row's status.
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

fn said(rows: &[(String, String)]) -> String {
    rows.iter()
        .map(|(s, t)| format!("[{s}] {t}"))
        .collect::<Vec<_>>()
        .join(" | ")
}

/// A turn that recovers on a replaced window says so.
///
/// Red before the change: the row holds the answer and nothing else, so the user believes the
/// agent still holds the conversation it was reading a moment ago.
#[tokio::test]
async fn a_turn_that_recovered_on_a_fresh_window_says_the_earlier_turns_left_it() {
    let (agent, root) = test_agent().await;
    agent
        .store
        .upsert_chat(CHAT, "stress", false, now_ms())
        .unwrap();
    let host = agent.sync.open_room(CHAT).await.unwrap();
    let (pi, log) = context_pi(&root).await;
    agent.test_set_pi(Some(pi.clone()), CHAT).await;

    // The page body is flagged, not the ask: this turn is answered, and the window is poisoned
    // by its result. Nothing is replaced, so nothing is announced.
    let fetch = ask_for(&agent, &host, FETCH_ASK).await;
    let fetch_rows = answers(&host, &fetch).await;
    assert!(
        fetch_rows
            .iter()
            .any(|(status, text)| status != "error" && text.contains("changelog for build 412")),
        "the fetch turn is answered: {} / pi: {:?}",
        said(&fetch_rows),
        pi_log(&log)
    );
    assert!(
        !said(&fetch_rows).contains("fresh working context"),
        "and a turn that kept its window must not claim it lost one: {}",
        said(&fetch_rows)
    );

    // The unrelated ask is refused on the window it lands in, recovers on the replaced one, and
    // owes the user both facts: the answer, and that it came from a window holding none of this
    // chat.
    let sums = ask_for(&agent, &host, ARITHMETIC).await;
    let rows = answers(&host, &sums).await;
    assert!(
        rows.iter()
            .any(|(status, text)| status != "error" && text.contains(ARITHMETIC_ANSWER)),
        "the ask recovers on the fresh window: {} / pi: {:?}",
        said(&rows),
        pi_log(&log)
    );
    assert!(
        said(&rows).contains("That answer came from a fresh working context"),
        "and the user is told their working context went: {} / pi: {:?}",
        said(&rows),
        pi_log(&log)
    );
    assert!(
        said(&rows).contains("read them back out of this Project's history"),
        "with the one thing they can do about it: {}",
        said(&rows)
    );

    let lines = pi_log(&log);
    assert_eq!(
        lines.iter().filter(|l| l.starts_with("refused ")).count(),
        1,
        "the window is replaced once and the retry is answered there: {lines:?}"
    );
    pi.shutdown().await;
}

/// A turn whose refusal survived the replaced window tells the user to send it again.
///
/// Red before the change: the row reads "split it up rather than sending the same message again",
/// which is advice against the one thing that now works.
#[tokio::test]
async fn a_dropped_refusal_tells_the_user_to_send_it_again_not_to_split_it() {
    let (agent, root) = test_agent().await;
    agent
        .store
        .upsert_chat(CHAT, "stress", false, now_ms())
        .unwrap();
    let host = agent.sync.open_room(CHAT).await.unwrap();
    let (pi, log) = context_pi(&root).await;
    agent.test_set_pi(Some(pi.clone()), CHAT).await;

    // The flagged content is the user's own message, so it rides the retry as well and the turn
    // ends refused, with the window dropped (#2919).
    let refused = ask_for(&agent, &host, FLAGGED).await;
    let rows = answers(&host, &refused).await;
    let line = said(&rows);
    assert!(
        rows.iter().any(|(status, _)| status == "error"),
        "the refusal still stands and still reaches the user: {line}"
    );
    assert!(
        line.contains("reset this chat's working context") && line.contains("worth sending again"),
        "the context went, so the line says that and that the message is worth sending again: \
         {line} / pi: {:?}",
        pi_log(&log)
    );
    assert!(
        !line.contains("split it up"),
        "and must not tell the user to do the one thing that no longer applies: {line}"
    );
    // Without a probe agentd cannot know whether the message or the history was declined, so the
    // line must claim neither that the message was fine nor that the re-send will work.
    for promise in [
        "that is what gets through",
        "will work",
        "was fine",
        "nothing wrong",
    ] {
        assert!(
            !line.contains(promise),
            "the line promises an outcome it cannot know ({promise:?}): {line}"
        );
    }
    assert!(
        !line.contains("fresh working context"),
        "the line carries this turn's news; the recovery notice would be a second voice: {line}"
    );
    pi.shutdown().await;
}

/// A dropped turn leaves this chat's memory, not only its window.
///
/// Red before the change: the declined words stay an ordinary `user` row, and replacing the
/// window is itself what opens this chat's own rows to the memory walk, so they can ride in front
/// of a later turn — or into the recap of a later retry — and have that turn declined too.
///
/// The row itself stays: the words are the user's own, so the transcript, `t3_thread` and a
/// deliberate history read all keep them. Only what agentd offers back on its own changes.
#[tokio::test]
async fn a_dropped_refusal_leaves_this_chats_memory_as_well_as_its_window() {
    let (agent, root) = test_agent().await;
    agent
        .store
        .upsert_chat(CHAT, "stress", false, now_ms())
        .unwrap();
    let host = agent.sync.open_room(CHAT).await.unwrap();
    let (pi, log) = context_pi(&root).await;
    agent.test_set_pi(Some(pi.clone()), CHAT).await;

    let refused = ask_for(&agent, &host, FLAGGED).await;
    let rows = agent.store.t3_thread(CHAT, None, None, 50).unwrap();
    let mine: Vec<&crate::store::T3Row> =
        rows.iter().filter(|r| r.text.contains(FLAGGED)).collect();
    assert_eq!(
        mine.len(),
        1,
        "the declined words are still in the thread, once: {rows:?}"
    );
    assert_eq!(
        mine[0].kind,
        crate::store::T3_KIND_REFUSED,
        "and marked, so the memory walk passes over them: {:?} / pi: {:?}",
        mine[0],
        pi_log(&log)
    );

    // The other carrier into a later turn: the recap a retry puts in front of its prompt.
    let next = ulid();
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(&next, ARITHMETIC, "dev-mac", &UserMessageOpts::default())
            .expect("user message");
    }
    let recap = {
        let h = host.lock().await;
        crate::agent::chat_recap(&h.doc.messages(), &next, "Arbos", CHAT)
    };
    assert!(
        !recap.contains(FLAGGED),
        "a later turn's recap must not hand the declined turn back: {recap}"
    );
    assert!(
        !recap.contains(&refused),
        "including by way of its row id: {recap}"
    );
    pi.shutdown().await;
}

/// The message ids t3 holds as declined turns — the marker's reading of "this turn was refused".
fn marked_in_store(agent: &Arc<Agent>) -> Vec<String> {
    agent
        .store
        .with(|c| {
            let mut st = c.prepare(
                "SELECT json_extract(source, '$.message_id') FROM t3
                 WHERE thread_id = ?1 AND kind = ?2 ORDER BY id",
            )?;
            let rows = st.query_map(
                rusqlite::params![CHAT, crate::store::T3_KIND_REFUSED],
                |r| r.get::<_, Option<String>>(0),
            )?;
            Ok(rows.filter_map(Result::ok).flatten().collect::<Vec<_>>())
        })
        .unwrap()
}

/// Two mechanisms carry one concept, and this is the test that fails when they drift apart.
///
/// The memory walk goes by the t3 row's kind (`Store::t3_mark_refused`); the recap goes by the
/// transcript's `model_refused` code (`refused_user_rows`). They cannot be one mechanism — the
/// marker is permanent, so it is only earned by a refusal that survived a clean-window retry on a
/// turn the user's own words opened, while the recap has to skip refusals that never earn a
/// retry. So the contract is a containment: **a marked turn is always recap-excluded, never the
/// reverse**, and neither touches a turn that delivered prose.
///
/// Both shapes are driven here against a live session rather than asserted over a `TurnState`,
/// because what broke the second one was neither mechanism but the order of the turn's own
/// writers: with the send tool on, the prose was still held when the drop was decided, so an
/// answered turn looked silent and was marked.
#[tokio::test]
async fn the_two_readings_of_a_refused_turn_agree_where_they_overlap() {
    let (agent, root) = test_agent().await;
    agent
        .store
        .upsert_chat(CHAT, "stress", false, now_ms())
        .unwrap();
    let host = agent.sync.open_room(CHAT).await.unwrap();
    let (pi, log) = context_pi(&root).await;
    agent.test_set_pi(Some(pi.clone()), CHAT).await;

    // Refused in every window and nothing delivered: dropped, so both readings have it.
    let dropped = ask_for(&agent, &host, FLAGGED).await;
    // Refused after prose: no retry, so no drop and no marker — the recap still has to skip it,
    // because the user's words are what the provider declined either way.
    let partial = ask_for(&agent, &host, PARTIAL_ASK).await;
    let partial_rows = answers(&host, &partial).await;
    assert!(
        said(&partial_rows).contains(PARTIAL_ANSWER),
        "the partial turn keeps the prose it got out, which is why neither mechanism takes it: \
         {} / pi: {:?}",
        said(&partial_rows),
        pi_log(&log)
    );

    let marked = marked_in_store(&agent);
    assert_eq!(
        marked,
        vec![dropped.clone()],
        "the marker is earned by the dropped turn alone: {marked:?}"
    );

    let rows = { host.lock().await.doc.messages() };
    let skipped = crate::agent::refused_user_rows(&rows);
    for id in &marked {
        assert!(
            skipped.contains(id),
            "a turn the walk will not offer must not ride the recap either: {id} missing from \
             {skipped:?}"
        );
    }
    assert!(
        !skipped.contains(&partial),
        "a turn that answered is not a declined turn to either of them: {skipped:?}"
    );
    // And the recap is built from that reading, so the two carriers agree in the output too.
    let next = ulid();
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(&next, ARITHMETIC, "dev-mac", &UserMessageOpts::default())
            .expect("user message");
    }
    let recap = {
        let h = host.lock().await;
        crate::agent::chat_recap(&h.doc.messages(), &next, "Arbos", CHAT)
    };
    assert!(
        !recap.contains(FLAGGED),
        "the marked turn is out of the recap as well: {recap}"
    );
    assert!(
        recap.contains(PARTIAL_ANSWER),
        "the answered turn is still this chat's history: {recap}"
    );
    pi.shutdown().await;
}

/// The marker hides the row from the memory walk and from nothing else.
///
/// The failure this guards against is silent: a reader that filters `kind = 'user'` does not
/// error when a row changes kind, it just stops seeing it. The readers that must keep seeing it
/// are the deliberate ones — `history_search` with `kind: "user"` (the only caller that passes a
/// kind at all), the secret sweep, the inspect window, `history_read` — so `t3_search_fts` treats
/// the marker as a user row, and nothing filtered on kind stands between `t3_thread` and its
/// caller.
#[test]
fn a_marked_turn_is_still_there_for_every_deliberate_reader() {
    let dir = std::env::temp_dir().join(format!("arbos-marked-reads-{}", ulid()));
    std::fs::create_dir_all(&dir).unwrap();
    let store = crate::store::Store::open(&dir.join("agentd.db")).unwrap();
    let row = store
        .t3_append(
            CHAT,
            None,
            Some(CHAT),
            "user",
            Some("user"),
            "the osprey nesting platform plans",
            Some(&json!({ "message_id": "m-1" })),
        )
        .unwrap();
    assert_eq!(store.t3_mark_refused(CHAT, "m-1").unwrap(), 1);

    let thread = store.t3_thread(CHAT, None, None, 10).unwrap();
    assert!(
        thread.iter().any(|r| r.id == row),
        "history_read's reader keeps the row: {thread:?}"
    );

    for kind in [None, Some("user"), Some(crate::store::T3_KIND_REFUSED)] {
        let hits = store
            .t3_search("osprey", Some(CHAT), kind, 10, 0)
            .unwrap_or_default();
        assert!(
            hits.iter().any(|(r, _, _)| r.id == row),
            "a search for {kind:?} still finds a declined turn: {hits:?}"
        );
    }
    // And the one reader that must not: the walk's keyword nomination.
    let any = store
        .t3_search_any(&["osprey".to_owned()], 10)
        .unwrap_or_default();
    assert!(
        any.iter().any(|(r, _, _)| r.id == row),
        "the search itself is kind-blind; the walk filters on the row's kind instead: {any:?}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// The two forms of the line, and the one input that chooses between them.
#[test]
fn the_refusal_line_turns_on_whether_the_window_was_replaced() {
    let refused = ModelError::from_message(&json!({
        "role": "assistant", "stopReason": "error",
        "rawStopReason": "refusal", "errorMessage": REFUSAL_DETAIL,
    }))
    .expect("a refused message");

    let kept = refused.chat_line_for("Jacob", false);
    assert!(
        kept.contains("split it up rather than sending the same message again"),
        "a window that was left alone keeps #2750's line, which is right for it: {kept}"
    );

    let replaced = refused.chat_line_for("Jacob", true);
    assert!(replaced.contains("worth sending again"), "{replaced}");
    assert!(!replaced.contains("split it up"), "{replaced}");
    // The ack path reads as one message with the bubble above it, so it must not disagree.
    assert_eq!(refused.chat_line_after_ack("Jacob", true), replaced);
    assert_eq!(refused.chat_line_after_ack("Jacob", false), kept);

    // Nothing else moves on this input. A failure that is not a refusal has no window advice to
    // give, and a budget block is not fixed by a fresh window.
    let failed = ModelError {
        detail: "529 overloaded_error".to_owned(),
        refusal: false,
    };
    assert_eq!(
        failed.chat_line_for("Jacob", true),
        failed.chat_line_for("Jacob", false)
    );
    let broke = ModelError {
        detail: "credits_exhausted: this workspace's credits are used up".to_owned(),
        refusal: false,
    };
    assert_eq!(
        broke.chat_line_for("Jacob", true),
        broke.chat_line_for("Jacob", false)
    );
}
