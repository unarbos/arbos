//! The row-less cancel's answer describes the ask it cancelled and says nothing true about the
//! box-wide sweep it ran on the way — so it can print a positive all-clear over a sweep that
//! killed something, or over one that could not look.
//!
//! # The shape
//!
//! `hook_stop_job` runs `reap_and_note_unreachable` **before** it decides what to answer, so
//! every one of its three return paths is reporting a box-wide sweep that has already happened.
//! Two of them report that sweep's own evidence. The third — the one a cancelled row-less ask
//! takes — hardcodes it:
//!
//! ```text
//! let evidence = Evidence::Nothing;
//! …
//! %evidence,                        // always `nothing_to_check`
//! checked = evidence.proven(),      // derived from the constant
//! found = 0,                        // a literal
//! killed = 0,                       // a literal
//! survivors = ?unreachable.survivors,   // the one field that is the sweep's
//! ```
//!
//! The constant is a true statement about the **ask**: it was cancelled before it started, so no
//! process of it can exist. It is a false statement about the **sweep**, and the line does not
//! say which subject it has. A reader keyed on `evidence` — which is what the cancel canary's
//! stop-line check reads — takes `nothing_to_check` as the positive all-clear it is defined to
//! be, while the same call was off killing processes or failing to read a session.
//!
//! The survivor list is the tell. It is the sweep's, so the line already mixes subjects; it is
//! just that the only sweep field present is the one that is empty in the common case.
//!
//! # What #2867 already covers, and what it does not
//!
//! #2867 put `unconfirmed_because` and `sessions_not_read` on this line and into this answer, so
//! the **cause** of an unconfirmed sweep does reach a reader from here. That is not the same as
//! the verdict being right: the cause fields sit beside an `evidence` that still reads
//! `nothing_to_check` and a `checked` derived from it, which is a record that contradicts
//! itself. Closing a false all-clear by printing the reason beside it is weaker than not
//! printing the all-clear, and it leaves the field a canary keys on still lying.
//!
//! # Why this case kills a process rather than contending a lock
//!
//! The defect is one expression, so any sweep outcome that is not `Nothing` exposes it. The
//! cheapest one to reach deterministically is `Enumerated`: an orphan marked process whose
//! session has a subagent row and which no adapter holds a child for is collected by the reap
//! with no timing to win. It also gives the sharpest reading of the bug — **the box killed a
//! process during this very call and wrote down that there was nothing to check.**
//!
//! The `Unconfirmed` variant travels the same line and is fixed by the same expression, but
//! driving it through this entry point needs a real `stop()` left in flight to hold an adapter's
//! children map, which is `sweep_contention_tests`' machinery (#2871). It is not reproduced here
//! rather than reproduced badly.
//!
//! # Setup mirrored rather than shared
//!
//! The row-less ask is staged the way `rowless_cancel_entrypoint_tests` stages it — every
//! subagent permit taken **before** the ask is accepted, a user turn held, and the parked
//! `queued` row waited for. It is a copy on a chat id of its own, for the reason that file gives
//! for keeping its own cases apart: two chats cannot see each other's asks, and a shared fixture
//! would couple a case about bug AL's reachability to a case about what the answer says.
//!
//! Real processes with real marks, read back out of `/proc`. Linux-only, skipped elsewhere.

#![cfg(target_os = "linux")]

use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use arbos_core::doc::{CommandView, UserMessageOpts};
use arbos_core::ids::{now_ms, ulid};
use serde_json::{json, Value};
use tokio::sync::OwnedSemaphorePermit;

use crate::agent::Agent;
use crate::isolation::{marked_procs_all, WORKER_MARK_ENV};
use crate::status::tests::{test_agent, Journal};
use crate::sync::SharedHost;

const CHAT: &str = "01TESTCHATROWLESSEVID0001";

/// A plain ask, phrased so `forced_job::forces_a_job` does not read it as ordering a row. An ask
/// with a row takes a different path entirely and never reaches the return under test.
const ASK_TEXT: &str = "also count the files under /etc for me";

/// A real process carrying a worker mark, killed on the way out whatever the case does.
///
/// Held in its own struct so a failed assertion cannot leave a marked `sleep` on the machine for
/// the next run to read as part of its own measurement.
struct Orphan {
    pid: u32,
    child: std::process::Child,
}

impl Drop for Orphan {
    fn drop(&mut self) {
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(self.pid as i32),
            nix::sys::signal::Signal::SIGKILL,
        );
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Work the box can prove is abandoned: a marked process whose session has a subagent row and
/// which no adapter holds a child for, so `session_is_over` answers `Some(true)` for it.
///
/// This is what makes the reap behind the cancel `Enumerated` rather than `Nothing`. The session
/// id is unique per run because `/proc` is the whole machine and other cases run their own
/// marked trees beside this one.
async fn an_orphan_the_sweep_will_collect(agent: &Arc<Agent>) -> Orphan {
    let session = ulid();
    let mark = format!("pi:{session}");
    let child = Command::new("sleep")
        .arg("300")
        .env(WORKER_MARK_ENV, &mark)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the orphan starts");
    let pid = child.id();
    let orphan = Orphan { pid, child };

    agent
        .store
        .insert_subagent(
            &session,
            "job-orphaned",
            "a worker that died with work still running",
            "pi",
            CHAT,
        )
        .unwrap();

    wait_for("the orphan carries its mark in /proc", || {
        marked_procs_all()
            .expect("/proc must be readable, or this case measures an empty box")
            .into_iter()
            .any(|(p, m)| p == pid && m == mark)
    })
    .await;
    orphan
}

fn is_running(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some((_, after_comm)) = stat.rsplit_once(')') else {
        return false;
    };
    !matches!(after_comm.split_whitespace().next(), Some("Z") | Some("X"))
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

/// A user message in `host`, and the `send` command the relay delivers for it.
async fn a_user_message(host: &SharedHost, text: &str) -> CommandView {
    let msg_id = ulid();
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(&msg_id, text, "dev-mac", &UserMessageOpts::default())
            .unwrap();
    }
    CommandView {
        id: ulid(),
        kind: "send".into(),
        payload: json!({ "messageId": msg_id }),
        issued_by: "dev-mac".into(),
        issued_at: now_ms(),
        status: "pending".into(),
    }
}

/// Accept a row-less ask through the production send path and leave it parked on a slot it
/// cannot have. The permits are taken before the ask is accepted, so there is no interval in
/// which it could start.
async fn an_accepted_ask_that_cannot_start(
    agent: &Arc<Agent>,
    host: &SharedHost,
) -> (OwnedSemaphorePermit, String) {
    let every_slot = agent
        .subagent_slots
        .clone()
        .acquire_many_owned(agent.subagent_slots.available_permits() as u32)
        .await
        .expect("the slots are open");

    let cmd = a_user_message(host, ASK_TEXT).await;
    let accepted = agent
        .start_job_for_send(CHAT, host, &cmd, &mut None)
        .await
        .expect("the send path answers");
    assert!(accepted, "the ask must be accepted beside the running turn");

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
    let queued = agent
        .store
        .subagents()
        .unwrap()
        .into_iter()
        .find(|s| s.chat_id.as_deref() == Some(CHAT) && s.status == "queued")
        .expect("the queued ask");

    assert!(
        agent.store.jobs_for_chat(CHAT).unwrap().is_empty(),
        "the ask must own no job row, or this is #2850's path and not the return under test"
    );
    (every_slot, queued.id)
}

/// The `stop_job answered` line this call left, or a panic naming what was logged instead.
fn the_stop_line(journal: &Journal) -> String {
    let text = journal.text();
    text.lines()
        .find(|l| l.contains("stop_job answered"))
        .map(str::to_owned)
        .unwrap_or_else(|| {
            panic!("no `stop_job answered` line was logged at all. The journal held: {text}")
        })
}

/// A cancel that reaches a row-less ask, while the sweep it runs on the way collects an orphan.
///
/// # What fails without the fix, and how it reads
///
/// The orphan dies — the sweep is real and does its work on both sides of this change — and the
/// line the same call writes says `evidence=nothing_to_check`, `checked=false`, `found=0`,
/// `killed=0`. Every assertion below then prints that line beside the pid the box had just
/// killed, which is the defect in its own output: a positive all-clear over a kill.
#[tokio::test]
async fn a_cancelled_rowless_ask_reports_the_sweep_that_ran_beneath_it() {
    let (agent, _root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    // Both halves of "a user turn is running": the turn lock, and a turn state for this chat.
    let _busy = agent.test_hold_turn(CHAT).await;

    let orphan = an_orphan_the_sweep_will_collect(&agent).await;
    let orphan_pid = orphan.pid;
    let (every_slot, subagent_id) = an_accepted_ask_that_cannot_start(&agent, &host).await;

    // Scoped to the one call, as the other journal cases scope theirs: everything above logs
    // too, and a capture spanning the setup would make the assertions below search it.
    let journal = Journal::default();
    let answer = {
        let _capture = journal.capture_info();
        agent
            .hook_stop_job("all")
            .await
            .expect("the cancel reaches the row-less ask and answers")
    };

    // The precondition, scored rather than assumed. Without it an assertion about what the
    // sweep reported would be measuring a sweep that never ran, which is the error this whole
    // family of bugs is made of.
    wait_for("the sweep collects the orphan", || !is_running(orphan_pid)).await;

    // The answer first, because it needs no capture to read and so cannot go quiet. Without the
    // fix the key is absent entirely: the sweep reached this answer as a survivor list and
    // nothing else, so a kill the user's own cancel performed was invisible from it.
    assert_eq!(
        answer
            .get("unnamed_processes_killed")
            .and_then(Value::as_u64),
        Some(1),
        "the answer must report the kill this cancel made, after it killed {orphan_pid}. \
         It said: {answer}"
    );
    // Unchanged by this fix, and asserted so a regression cannot trade one subject for the
    // other: the ask itself is still reported as cancelled.
    assert_eq!(
        answer.get("asks_cancelled").and_then(Value::as_u64),
        Some(1),
        "the row-less ask must still be reported as cancelled. It said: {answer}"
    );

    // Then the line, which is what the cancel canary's stop-line check reads.
    let line = the_stop_line(&journal);
    assert!(
        !line.contains("evidence=nothing_to_check"),
        "the call killed {orphan_pid} and recorded that there was nothing to check: {line}"
    );
    assert!(
        !line.contains("found=0"),
        "the sweep enumerated the orphan and the line says it found nothing: {line}"
    );
    assert!(
        !line.contains("killed=0"),
        "the sweep killed {orphan_pid} and the line says it killed nothing: {line}"
    );

    drop(every_slot);
    drop(subagent_id);
}
