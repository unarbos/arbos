//! Bug AL at the two entry points a user's cancel actually arrives through, with the race forced
//! in code because no workload reaches it.
//!
//! # Why this file exists rather than another canary round
//!
//! #2857 fixed a race with three parts: an ask the chat has **accepted**, which owns **no job
//! row**, and which **has not started**. The physical canary on 0.1.200 established the first
//! two parts three times over and the third **never**, across three attempts that each held the
//! ask back a different way:
//!
//! * **Slots full** — four filler job workers live at once, the full `SUBAGENT_MAX_CONCURRENT`
//!   table. The ask was accepted, owned no row, and pi-main did it inline and immediately. The
//!   slot cap gates subagent job workers; it does not gate the main agent, so a full table holds
//!   a plain-phrased ask back not at all.
//! * **pi-main busy** — the ask's message id appears zero times in agentd's log. It took no turn
//!   lock and opened no turn row; it was absorbed into the turn that was already running.
//! * **Box asleep** — the wake path gave the ask a turn of its own, which ran and closed 8.3
//!   seconds *before* the cancel's turn opened.
//!
//! So the obstacle was never in the cancel path. It is in the turn machinery, and it sits
//! upstream of everything #2857 touches. Scenario A of the cancel canary is therefore recorded
//! as waived for 0.1.200 rather than passed, and these cases are what cover the race instead.
//!
//! # The state is reachable in code, and this is where
//!
//! [`Agent::start_job_for_send`] is the production path that accepts a row-less ask:
//! `handle_command` reads a `send` that arrived while a user turn held the lock, the quick lane
//! answers `Queued`, and the ask runs beside that turn as a subagent with `job: None`. From
//! there [`Agent::run_subagent`] holds it and parks it on `subagent_slots`, and **that park is
//! the window**. Every permit taken means the ask is accepted, owns no row, and cannot start
//! until a permit frees.
//!
//! The canary never combined the two levers it needs. Attempt 1 filled the slots with no user
//! turn running, so `start_job_for_send` declined at its `running_user_turn` check and the ask
//! queued for the main session. Attempt 2 had a user turn running with the slots free, so the
//! ask took a permit at once. The window opens only when a user turn is running **and** every
//! subagent slot is held, and no attempt arranged both.
//!
//! These cases arrange both, in code and deterministically: the permits are taken before the ask
//! is accepted, so there is no interval in which it could start, and the state is waited for
//! rather than slept past. The canary procedure's own scenario A says only "fill the slots",
//! which is why attempt 1 followed it exactly and still missed; a round that wants to reach this
//! from a workload must hold a user turn open at the same time.
//!
//! # What these add over `rowless_cancel_tests`
//!
//! #2857's own cases call [`Agent::cancel_pending_asks`] directly. That proves the function
//! reaches the ask; it proves nothing about the callers. Delete the call from `handle_stop_cmd`
//! and from `hook_stop_job` and every case in that file still passes, while the Stop button and
//! `stop_job("all")` both go back to missing the ask entirely — which is the production defect,
//! not a refactor hazard. These cases go through `handle_command` and `hook_stop_job` so the
//! wiring is what is under test, and they assert the sentence the user reads.
//!
//! # The revert check, so a green here can be trusted
//!
//! Both stages run as `cargo test -p arbos-agentd rowless_cancel`. Use `-p arbos-agentd` rather
//! than `--workspace`: the workspace suite needs system fontconfig that a cloud VM does not
//! have.
//!
//! **Stage 1 — unwire the entry points only.** Replace the `cancel_pending_asks` call in
//! `handle_stop_cmd` with an empty `pending`, and the one in `hook_stop_job`'s `all` branch
//! likewise. The hold and the post-slot guard stay. All four cases in `rowless_cancel_tests`
//! stay green; the two headline cases here go red, carrying the defect in its own words —
//! `"no running job matched; list_jobs shows what is open in this chat"` for `stop_job("all")`,
//! and an empty chat for the Stop button, each printing the ask that *was* held at the time.
//!
//! **Stage 2 — the whole of #2857.** Also remove the hold in `run_subagent` and the post-slot
//! `ask_was_cancelled` guard below it. The same two cases go red with the held set printed as
//! `[]`, and `a_stop_reaches_an_ask_that_owns_no_job_row` joins them.
//!
//! The chip case stays green in both stages, as it must: it pins behaviour #2857 did not change.

use std::sync::Arc;
use std::time::Duration;

use arbos_core::doc::{CommandView, UserMessageOpts};
use arbos_core::ids::{now_ms, ulid};
use serde_json::{json, Value};
use tokio::sync::OwnedSemaphorePermit;

use crate::agent::Agent;
use crate::status::tests::test_agent;
use crate::sync::SharedHost;

const CHAT: &str = "01TESTCHATROWLESSENTRY0001";

/// A chat no case acts on, used only to hold the control row that proves the job query reads.
const CONTROL_CHAT: &str = "01TESTCHATROWLESSCONTROL01";

/// A plain ask. Deliberately **not** phrased as a background job: `forced_job::forces_a_job`
/// reads that wording as forcing a row, and an ask with a row is a different path entirely
/// (#2850) that `stop_jobs` can already see.
const ASK_TEXT: &str = "also count the files under /etc for me";

/// Every status part on every assistant row, as `code: message`.
///
/// The headline assertion is about something that must *not* be here, so the list itself is the
/// evidence: a failure prints what the ask actually did rather than reporting an unmet
/// expectation. "It never started" is unobservable; "here is everything it did, and `Working…`
/// is not among it" is not.
async fn status_parts(host: &SharedHost) -> Vec<String> {
    let h = host.lock().await;
    h.doc
        .messages()
        .into_iter()
        .filter(|m| m.role == "assistant")
        .flat_map(|m| m.parts.clone())
        .filter_map(|p: Value| {
            let code = p.get("code")?.as_str()?.to_owned();
            let message = p.get("message")?.as_str()?.to_owned();
            Some(format!("{code}: {message}"))
        })
        .collect()
}

/// The text of every assistant row written in reply to `user_msg_id`.
async fn replies_to(host: &SharedHost, user_msg_id: &str) -> Vec<String> {
    let h = host.lock().await;
    h.doc
        .messages()
        .into_iter()
        .filter(|m| {
            m.role == "assistant"
                && m.rest.get("inReplyTo").and_then(Value::as_str) == Some(user_msg_id)
        })
        .map(|m| m.text())
        .collect()
}

/// A user message in `host`, and the `send` command the relay delivers for it.
async fn a_user_message(host: &SharedHost, text: &str) -> (String, CommandView) {
    let msg_id = ulid();
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(&msg_id, text, "dev-mac", &UserMessageOpts::default())
            .unwrap();
    }
    let cmd = CommandView {
        id: ulid(),
        kind: "send".into(),
        payload: json!({ "messageId": msg_id }),
        issued_by: "dev-mac".into(),
        issued_at: now_ms(),
        status: "pending".into(),
    };
    (msg_id, cmd)
}

/// The Stop button: a `stop` command naming no job.
async fn a_bare_stop(host: &SharedHost) -> (String, CommandView) {
    let msg_id = ulid();
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(&msg_id, "stop", "dev-mac", &UserMessageOpts::default())
            .unwrap();
    }
    let cmd = CommandView {
        id: ulid(),
        kind: "stop".into(),
        payload: json!({ "messageId": msg_id }),
        issued_by: "dev-mac".into(),
        issued_at: now_ms(),
        status: "pending".into(),
    };
    (msg_id, cmd)
}

async fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    for _ in 0..400 {
        if ready() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("{what} did not happen within 10s");
}

/// How long to wait for the hold before giving up on seeing it.
///
/// Only ever spent when the hold is absent, which is the diagnostic run against pre-#2857 code.
/// With the fix in place the hold appears in milliseconds and nothing here waits.
const HOLD_BUDGET: Duration = Duration::from_secs(5);

/// Accept a row-less ask through the production send path and leave it unstarted. Returns the
/// permits that gate it, the ask's session id, and the held set as it reads at that moment.
///
/// The permits are taken **before** the ask is accepted, so there is no interval in which it
/// could have started and no timing to get right.
///
/// # What this synchronises on, and why it is not the hold
///
/// The `queued` row that `run_subagent` writes when it finds no free slot is the state itself —
/// accepted, no job row, parked — and it predates #2857, so waiting on it works on both sides of
/// the fix. The held set is read afterwards and **returned rather than asserted on**: it is
/// #2857's own mechanism, so a precondition that demanded it would turn a run against pre-#2857
/// code into a test-setup failure instead of letting the behaviour below speak.
///
/// The hold is still waited for, because the fixed build must have it in hand before the stop
/// lands. `queued` is written immediately before the hold, so returning on `queued` alone would
/// leave a window in which a stop could arrive first — and the tests would then go red for a
/// reason that is not the defect.
async fn an_accepted_ask_that_cannot_start(
    agent: &Arc<Agent>,
    host: &SharedHost,
) -> (OwnedSemaphorePermit, String, Vec<String>) {
    let every_slot = agent
        .subagent_slots
        .clone()
        .acquire_many_owned(agent.subagent_slots.available_permits() as u32)
        .await
        .expect("the slots are open");
    assert_eq!(
        agent.subagent_slots.available_permits(),
        0,
        "the ask must find no slot to take"
    );

    let (_user_msg_id, cmd) = a_user_message(host, ASK_TEXT).await;
    let accepted = agent
        .start_job_for_send(CHAT, host, &cmd, &mut None)
        .await
        .expect("the send path answers");
    assert!(
        accepted,
        "the production path must accept this ask beside the running turn — the canary's \
         'accepted' part, which it did reach"
    );

    // Accepted and parked on a slot it cannot have: the part no workload reached.
    let queued = {
        let probe = agent.clone();
        wait_for("the ask parks on a slot it cannot have", move || {
            probe
                .store
                .subagents()
                .unwrap_or_default()
                .iter()
                .any(|s| s.chat_id.as_deref() == Some(CHAT) && s.status == "queued")
        })
        .await;
        agent
            .store
            .subagents()
            .unwrap()
            .into_iter()
            .find(|s| s.chat_id.as_deref() == Some(CHAT) && s.status == "queued")
            .expect("the queued ask")
    };

    // The remaining part of the precondition, scored rather than assumed — this is a part the
    // canary did establish, and it must still be true here or the case is testing #2850's path.
    //
    // Scored against a control, because `is_empty()` on its own is also the answer a query that
    // can see nothing would give. A row is planted on an unrelated chat and read back through
    // the same call in the same breath, which separates "this chat owns no job" from "this query
    // answers empty for everyone" — and pins the keying besides, since a read blind to `chat_id`
    // would hand the planted row back under `CHAT`.
    let control = ulid();
    agent
        .store
        .insert_job(&control, CONTROL_CHAT, &ulid(), "control", "ask", "", &[])
        .expect("the control row is written");
    // Cancelled on the spot: an open row on an unrelated chat is something a runner or a sweep
    // could take an interest in, and this one exists only to be read back.
    agent
        .store
        .set_job_state(&control, "cancelled", None)
        .expect("the control row is closed");
    assert_eq!(
        agent.store.jobs_for_chat(CONTROL_CHAT).unwrap().len(),
        1,
        "control: the job query must be able to see a row at all, or the next assertion holds \
         for a reason that has nothing to do with this ask"
    );
    assert!(
        agent.store.jobs_for_chat(CHAT).unwrap().is_empty(),
        "the ask must own no job row, or `stop_jobs` could see it and the race is a different one"
    );

    let mut held = Vec::new();
    let deadline = std::time::Instant::now() + HOLD_BUDGET;
    while std::time::Instant::now() < deadline {
        held = agent.test_held_asks(CHAT).await;
        if !held.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    (every_slot, queued.id, held)
}

/// Let the ask through and wait for it to settle, however it settles.
async fn release_and_settle(
    agent: &Arc<Agent>,
    every_slot: OwnedSemaphorePermit,
    subagent_id: &str,
    settled: &str,
) {
    drop(every_slot);
    let agent2 = agent.clone();
    let id = subagent_id.to_owned();
    let want = settled.to_owned();
    wait_for(&format!("the ask settles at {settled}"), move || {
        agent2
            .store
            .subagent(&id)
            .ok()
            .flatten()
            .is_some_and(|row| row.status == want)
    })
    .await;
}

/// The Stop button reaches an ask the send path accepted and that never started.
///
/// # What fails without #2857, and how it reads
///
/// Without the hold there is nothing for `cancel_pending_asks` to return, so `handle_stop_cmd`
/// writes no stop line at all — `write_stop_line`'s gate suppresses it when no job was stopped,
/// no ask was cancelled and the reap found nothing — and when the slot frees the ask proceeds
/// into its work and writes `working: Working…`. Both assertions below then report the
/// production defect in its own terms: the chat was told nothing, and the ask the user stopped
/// ran anyway.
#[tokio::test]
async fn the_stop_button_reaches_an_ask_accepted_but_not_yet_started() {
    let (agent, _root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    // A user turn must be running for the send path to take the ask beside it. This guard is
    // both halves of that: it holds `turn_lock` and leaves a turn state for this chat.
    let _busy = agent.test_hold_turn(CHAT).await;

    let (every_slot, subagent_id, held) = an_accepted_ask_that_cannot_start(&agent, &host).await;

    let (stop_msg, stop) = a_bare_stop(&host).await;
    agent
        .handle_command(CHAT, stop)
        .await
        .expect("the stop is handled");

    // The user's own proof, which is the only one they get: a row-less ask leaves no job chip to
    // watch go grey, so the stop line is the whole of what the chat says about it.
    let lines = replies_to(&host, &stop_msg).await;
    assert!(
        lines
            .iter()
            .any(|l| l.contains("had not started yet was cancelled before it began")),
        "the stop must tell the chat it reached the ask. The chat said: {lines:?}; the ask \
         {subagent_id} was held as {held:?} when the stop landed"
    );

    release_and_settle(&agent, every_slot, &subagent_id, "idle").await;

    let parts = status_parts(&host).await;
    assert!(
        !parts.iter().any(|p| p.ends_with("Working…")),
        "the ask must not begin its work after the Stop button; everything it did was: {parts:?}"
    );
    assert!(
        parts.iter().any(|p| p.starts_with("stopped:")),
        "and it must say it stopped rather than end silently: {parts:?}"
    );
}

/// `stop_job("all")` from the main turn reaches the same ask, and stops answering that nothing
/// matched.
///
/// # What fails without #2857, and how it reads
///
/// `hook_stop_job` returns `Err("no running job matched…")` — truthfully, about the rows, and
/// silently about the ask — so the `expect` below fails carrying that string. That refusal is
/// exactly what made Jacob's round 18 cancel unprovable from the transcript: the model read it
/// and told the user nothing was found, while the ask it could not see started afterwards.
#[tokio::test]
async fn stop_job_all_reaches_an_ask_accepted_but_not_yet_started() {
    let (agent, _root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    let _busy = agent.test_hold_turn(CHAT).await;

    let (every_slot, subagent_id, held) = an_accepted_ask_that_cannot_start(&agent, &host).await;

    let answer = agent.hook_stop_job("all").await.unwrap_or_else(|e| {
        panic!(
            "a stop that cancelled an ask has not matched nothing, and answered: {e:?}. The ask \
             {subagent_id} was held as {held:?} when the stop landed"
        )
    });
    assert_eq!(
        answer["asks_cancelled"],
        json!(1),
        "the answer must count the ask the job path could not see: {answer}"
    );
    assert_eq!(
        answer["stopped"],
        json!([]),
        "and must claim no job, because there was none: {answer}"
    );
    // No sweep is claimed, deliberately: an ask stopped before it started has no process tree,
    // so a `processes_checked: true` here would be vacuously true rather than honest.
    assert!(
        answer.get("processes_checked").is_none(),
        "nothing was checked because nothing existed to check: {answer}"
    );

    release_and_settle(&agent, every_slot, &subagent_id, "idle").await;

    let parts = status_parts(&host).await;
    assert!(
        !parts.iter().any(|p| p.ends_with("Working…")),
        "the ask must not begin its work after `stop_job(\"all\")`; everything it did was: \
         {parts:?}"
    );
}

/// The job chip's stop names one job, and a row-less ask is not one — so it must leave the ask
/// running.
///
/// The cheapest way to pass both cases above is to cancel every held ask on every stop. That
/// would make the chip's "stop this job" silently end unrelated work the user never pointed at,
/// which is a worse bug than the one #2857 fixed. `handle_stop_cmd` passes an empty pending list
/// on that branch for this reason, and nothing else pins it.
#[tokio::test]
async fn the_job_chip_stop_leaves_a_row_less_ask_alone() {
    let (agent, _root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    let _busy = agent.test_hold_turn(CHAT).await;

    let (every_slot, subagent_id, held) = an_accepted_ask_that_cannot_start(&agent, &host).await;

    let chip = CommandView {
        id: ulid(),
        kind: "stop".into(),
        payload: json!({ "messageId": ulid(), "jobId": "01TESTJOBTHECHIPNAMED00001" }),
        issued_by: "dev-mac".into(),
        issued_at: now_ms(),
        status: "pending".into(),
    };
    agent
        .handle_command(CHAT, chip)
        .await
        .expect("the chip stop is handled");

    // The baseline must be a real held set, or the comparison below is `[] == []`. Measured:
    // with `test_held_asks` stubbed to report nothing held — which is what a regression in the
    // hold itself looks like — this case passed green without the line below.
    //
    // [`an_accepted_ask_that_cannot_start`] returns the set without asserting it, and is right
    // not to: demanding it there would turn a run against pre-#2857 code into a setup failure
    // for the two cases above, which read `held` only as diagnostic text. This case is the one
    // outside that reasoning's range, because the held set is its subject rather than its
    // context, so the baseline is scored here instead.
    assert!(
        !held.is_empty(),
        "the ask was never held, so there is no held set for the chip stop to leave alone"
    );
    assert_eq!(
        agent.test_held_asks(CHAT).await,
        held,
        "a stop naming one job must leave the held set exactly as it found it"
    );
    assert_eq!(
        agent.store.subagent(&subagent_id).unwrap().unwrap().status,
        "queued",
        "and the ask itself is untouched by it"
    );

    // It settles at `running` rather than `idle`: nothing cancelled it, so it takes the slot and
    // goes into its work.
    release_and_settle(&agent, every_slot, &subagent_id, "running").await;
    let parts = status_parts(&host).await;
    assert!(
        parts.iter().any(|p| p.ends_with("Working…")),
        "so it still starts when its slot frees: {parts:?}"
    );
}
