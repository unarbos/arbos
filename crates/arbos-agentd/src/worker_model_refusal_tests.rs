//! Bug AR: a worker whose model call the provider refused was reported as the worker's own
//! fault, and the identical refused brief was handed to a second worker four seconds later.
//!
//! # What round 22 actually recorded
//!
//! Jacob's `r22cancel` asked for `tick22`: four process trees of six processes each, 24 `setsid`
//! and `nohup` launches. Job `01M6YKN6EBKNSKRNQ8870Z7SXZ`, first worker
//! `01M6YKN6ED7X64JT17NPP8J21H`. The job row's error read `worker ended without output (no
//! reply, no tool call)`, the warn log agreed, and the chat said "I couldn't finish that one".
//!
//! The worker's own pi session file says something else. Its single assistant message is
//! `stopReason: error`, `rawStopReason: refusal`, with the provider's text:
//!
//! > Cyber safeguards flagged this message. If this is legitimate security work, you can apply
//! > for the Cyber Verification Program: <https://support.claude.com/en/articles/14604842-cyber-verification-program>
//!
//! 41 output tokens, 2,382 ms, and the `bash` tool call it had begun was cut off mid-argument
//! (`"cd /work/01M6KTFT1X6"`). The retry worker `01M6YKNCSBASNZR4DZA7HBVEJ5` got one `bash` call
//! through and was then refused in the same words. So: not a crash, not a 5xx, not a worker that
//! did nothing — a provider refusal, twice, on the same bytes.
//!
//! # Why agentd could not see it
//!
//! `translate_rpc_event` discarded `Event::MessageEnd`. pi 1.2 reports a refusal, a context
//! overflow and a provider 5xx **only** there, as `stopReason: error` on the finished assistant
//! message — `agent.rs` has always read it for the main session, and the worker stream never
//! did. With the one event that carried the reason dropped, the run reached its settle with no
//! text, no tool call and no error, and `worker_failure` said the only thing left to say.
//!
//! # What these cases hold
//!
//! 1. the event translation surfaces the refusal (`a_refused_model_call_reaches_the_worker_stream`);
//! 2. the classifier tells a refusal from a 5xx (`a_refusal_is_permanent_a_five_hundred_is_not`),
//!    and keys on pi's `rawStopReason` rather than on the provider's wording
//!    (`a_refusal_pi_named_counts_even_when_its_prose_gives_nothing_away`);
//! 3. end to end through `run_subagent`, the job row carries the provider's own sentence and the
//!    job is `failed` rather than `queued` for a second identical attempt
//!    (`a_refused_worker_fails_with_the_providers_reason_and_is_not_retried`).

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use arbos_core::doc::UserMessageOpts;
use arbos_core::ids::ulid;
use arbos_core::route::Harness;
use arbos_core::rpc::Event;

use crate::agent::{model_error_is_permanent, Agent, JobSpec, ModelError};
use crate::harness::{translate_rpc_event, AgentEvent};

const CHAT: &str = "01TESTCHATWORKERREFUSAL001";
const JOB: &str = "01TESTJOBWORKERREFUSAL0001";
const ASK: &str = "01TESTASKWORKERREFUSAL0001";

/// The provider's own words, from `01M6YKN6ED7X64JT17NPP8J21H`'s session file. Kept verbatim
/// because the classifier reads this prose: a paraphrase would test a sentence Anthropic never
/// sent.
const CYBER_REFUSAL: &str = "Cyber safeguards flagged this message. If this is legitimate security work, you can apply for the Cyber Verification Program: https://support.claude.com/en/articles/14604842-cyber-verification-program.";

/// The `message_end` record pi wrote for that refusal, with the fields `ModelError::from_message`
/// reads.
fn refused_message_end() -> Event {
    serde_json::from_value(json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [{ "type": "toolCall", "id": "toolu_01Lkx8467yCCUMzApvqWmmwy", "name": "bash",
                          "arguments": { "command": "cd /work/01M6KTFT1X6" } }],
            "stopReason": "error",
            "rawStopReason": "refusal",
            "errorMessage": CYBER_REFUSAL,
            "durationMs": 2382,
        },
    }))
    .expect("pi's message_end record parses")
}

/// **The dropped event.** On today's `main` this is `[]`, which is the whole of bug AR: the only
/// record naming the refusal is thrown away, so the run that follows has nothing to blame but
/// the worker.
#[test]
fn a_refused_model_call_reaches_the_worker_stream() {
    let out = translate_rpc_event(&refused_message_end());
    match out.as_slice() {
        [AgentEvent::ModelError { detail, refusal }] => {
            assert!(
                *refusal,
                "pi said `rawStopReason: refusal` itself; the stream must carry that rather than \
                 leave every later lane to read the prose"
            );
            assert_eq!(detail, CYBER_REFUSAL, "the provider's text, unaltered");
        }
        other => panic!("a refused model call must reach the worker's stream: {other:?}"),
    }
}

/// An ordinary assistant message is not an error, and must not be turned into one: a worker
/// writes one of these per turn, and a `ModelError` on each would fail every healthy run.
#[test]
fn an_ordinary_assistant_message_end_says_nothing() {
    let ok: Event = serde_json::from_value(json!({
        "type": "message_end",
        "message": { "role": "assistant", "stopReason": "endTurn",
                     "content": [{ "type": "text", "text": "Started the four trees." }] },
    }))
    .unwrap();
    assert!(translate_rpc_event(&ok).is_empty());

    // An abort is agentd's own doing — a stop, a steer, the turn deadline — and is announced
    // where it happens. `ModelError::from_message` already drops it; this holds that it stays
    // dropped on the worker path too, where an abort is how a cancel ends a run.
    let aborted: Event = serde_json::from_value(json!({
        "type": "message_end",
        "message": { "role": "assistant", "stopReason": "error", "rawStopReason": "aborted",
                     "errorMessage": "This operation was aborted" },
    }))
    .unwrap();
    assert!(
        translate_rpc_event(&aborted).is_empty(),
        "a cancel must not be reported to the user as a provider failure"
    );
}

/// The classification the retry decision rests on. A refusal and a context limit are answers to
/// the request; a 529 is an answer to the moment.
#[test]
fn a_refusal_is_permanent_a_five_hundred_is_not() {
    assert!(
        model_error_is_permanent(CYBER_REFUSAL, true),
        "pi's own refusal flag is enough"
    );
    assert!(
        model_error_is_permanent(CYBER_REFUSAL, false),
        "and so is the prose on its own (\"flagged\"), so a build that loses the flag still \
         reads this right"
    );
    assert!(model_error_is_permanent("prompt is too long: 214000 tokens", false));
    assert!(model_error_is_permanent("No space left on device (os error 28)", false));

    assert!(
        !model_error_is_permanent("529 overloaded_error", false),
        "an overloaded provider is exactly what a fresh worker is for"
    );
    assert!(!model_error_is_permanent("connection reset by peer", false));
}

/// **The flag is the key, not the wording.** `detail_is_refusal` catches round 22's text on the
/// word "flagged", which is luck rather than design; the signal agentd can rely on is pi's
/// `rawStopReason: refusal`, which is the provider saying so itself.
///
/// So a refusal worded in a way no keyword list predicted must still be permanent — otherwise
/// the next provider rewording quietly puts bug AR back, with a second worker bought for a brief
/// that cannot be run.
#[test]
fn a_refusal_pi_named_counts_even_when_its_prose_gives_nothing_away() {
    let bland = "the request was declined by upstream policy";
    assert!(
        !ModelError::detail_is_refusal(bland),
        "the premise of this case: the prose test finds nothing here"
    );
    assert!(
        model_error_is_permanent(bland, true),
        "and the flag alone must still settle it"
    );
    assert!(
        !model_error_is_permanent(bland, false),
        "while the same words with no flag stay retryable, so this is really testing the flag"
    );

    // And the line the user reads is the refusal line, which names what happened and what gets
    // through, rather than "ask me again in a moment" for something that will not clear.
    let line = ModelError {
        detail: bland.to_owned(),
        refusal: true,
    }
    .chat_line_for("Your");
    assert!(line.contains("declined"), "{line}");
    assert!(
        line.contains("split it up"),
        "a refusal has one way through and the line must say it: {line}"
    );
}

/// A pi that refuses the worker's first model call the way the provider refused `tick22`'s, and
/// answers the main session normally so the inline fallback this now takes can finish.
///
/// The role split is read from `ARBOS_SESSION_ROLE`, which `PiAdapter::envs` sets to `subagent`
/// for a worker, so the case does not have to tell the two children apart by argv.
const REFUSING_PI: &str = r#"#!/usr/bin/env python3
import json, os, sys

def out(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

worker = os.environ.get("ARBOS_SESSION_ROLE") == "subagent"
REFUSAL = "@REFUSAL@"

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    c = json.loads(line)
    kind, cid = c.get("type"), c.get("id")
    if kind == "get_state":
        out({"type": "response", "id": cid, "command": kind, "success": True,
             "data": {"isStreaming": False, "sessionFile": "@ROOT@/s.jsonl"}})
        continue
    out({"type": "response", "id": cid, "command": kind, "success": True, "data": None})
    if kind != "prompt":
        continue
    if worker:
        # 41 output tokens, a tool call cut off mid-argument, and the refusal on message_end.
        # No text delta and no tool_execution_start: nothing else ever reached agentd.
        out({"type": "message_end", "message": {
            "role": "assistant",
            "content": [{"type": "toolCall", "id": "toolu_01Lkx", "name": "bash",
                         "arguments": {"command": "cd /work/01M6KTFT1X6"}}],
            "stopReason": "error", "rawStopReason": "refusal", "errorMessage": REFUSAL,
            "durationMs": 2382}})
    else:
        out({"type": "message_update", "assistantMessageEvent":
             {"type": "text_delta", "contentIndex": 0, "delta": "I could not start those."}})
    out({"type": "agent_settled", "aborted": False})
"#;

struct Fixture {
    agent: Arc<Agent>,
    root: std::path::PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

async fn boot() -> Fixture {
    use std::os::unix::fs::PermissionsExt;
    let root = std::env::temp_dir().join(format!("arbos-ar-refusal-{}", ulid()));
    std::fs::create_dir_all(&root).unwrap();
    let pi_bin = root.join("refusing-pi.py");
    std::fs::write(
        &pi_bin,
        REFUSING_PI
            .replace("@REFUSAL@", CYBER_REFUSAL)
            .replace("@ROOT@", &root.display().to_string()),
    )
    .unwrap();
    std::fs::set_permissions(&pi_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (agent, _state) =
        crate::status::tests::test_agent_with_pi(&pi_bin.display().to_string()).await;
    Fixture { agent, root }
}

/// **The end-to-end case.** A job whose worker the provider refuses must record the provider's
/// own sentence and must not buy a second worker for the same bytes.
///
/// Both assertions fail on today's `main`, and they fail in the two ways Jacob saw:
///
/// * the row's error reads `worker ended without output (no reply, no tool call)` — a statement
///   about the worker, when the worker was declined;
/// * the row's state is `queued`, which is `run_subagent` putting the job back for a fresh
///   worker with the identical brief. That is the 4-second double attempt of round 22, and the
///   second attempt was refused in the same words.
///
/// With the fix the state is `failed` and the job goes straight to the inline fallback, which is
/// the one route left that is not the refused request again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_worker_fails_with_the_providers_reason_and_is_not_retried() {
    let f = boot().await;
    let agent = f.agent.clone();
    agent.start_pi(CHAT).await.expect("the main pi child starts");
    let host = agent.sync.open_room(CHAT).await.unwrap();
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(
                ASK,
                "build four tick22 process trees, six processes each",
                "dev-mac",
                &UserMessageOpts::default(),
            )
            .unwrap();
    }
    let brief = "Build four independent 6-process trees, tick22a to tick22d, with setsid and nohup launches.";
    agent
        .store
        .insert_job(JOB, CHAT, ASK, "Building the tick22 trees", "code", brief, &[])
        .unwrap();

    // The fallback turn that now follows is not what this case is about, and it needs the turn
    // lock and the send order; the row it asserts on is written before it starts.
    let _ = tokio::time::timeout(
        Duration::from_secs(60),
        agent.run_subagent(
            CHAT,
            &host,
            ASK,
            None,
            "job-ar",
            "build the tick22 trees",
            brief,
            Some(Harness::Pi),
            false,
            false,
            Some(JobSpec {
                id: JOB.to_owned(),
                title: "Building the tick22 trees".to_owned(),
                kind: "code".to_owned(),
                brief: brief.to_owned(),
                touches: vec![],
                attempt: 0,
            }),
        ),
    )
    .await;

    let row = agent.store.job(JOB).unwrap().unwrap();
    let error = row.error.unwrap_or_default();
    assert!(
        error.contains("Cyber safeguards flagged this message"),
        "the row must carry the provider's own reason; it read {error:?}"
    );
    assert!(
        !error.contains("worker ended without output"),
        "and must not blame the worker for a call it was never allowed to make: {error:?}"
    );
    assert_eq!(
        row.state, "failed",
        "a refused brief is refused again, so the job must not go back in the queue for a \
         second worker with the same bytes (error: {error:?})"
    );
    assert_eq!(
        row.restarts, 0,
        "and no restart may be counted for a worker that was never worth buying"
    );
}
