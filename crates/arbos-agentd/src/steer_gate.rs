//! A hold between "a user message for this chat arrived" and "a job of that chat puts its file in
//! the chat".
//!
//! Bug AB (Jacob, Test5 round 10, 2026-11-05, agentd 0.1.195 `adcd73573`, scenario `r10race`).
//! The job ask went out at 08:36:57 and the user's correction at 08:37:04. The main lane reached
//! `steer_job` at 08:37:09, five seconds later, and the worker posted the **uncorrected**
//! `race10.csv` at 08:37:10 — one second after the steer. The agent then had the corrected file
//! in hand and nothing honest to do with it, so it asked whether it should post a second
//! message. Related runs answered one ask with two and three CSVs.
//!
//! The two halves of that race could not see each other. A delivery asked only whether the job
//! was cancelled (bug S) and whether the same bytes were already posted; the inbound side took
//! the command, claimed its send-order place and routed, and told no one. This module is the
//! thing both halves read:
//!
//! - [`SteerGate::routing`] marks a user message as being routed, from the moment
//!   `handle_command` takes it until it is done with it. The mark is an RAII guard, so every exit
//!   of that function releases it — including the failing ones — and it also expires on its own
//!   after the hold, so a mark that leaks costs one hold and never a lost file.
//! - [`SteerGate::steer_delivered`] counts a correction handed to a job's worker, and
//!   [`SteerGate::steers_drained`] counts the worker's queue taking it.
//! - [`SteerGate::hold`] is what a delivery calls. It waits while a message of that chat is still
//!   being routed, and answers [`Hold::SteerFirst`] when a correction is sitting unread in the
//!   worker's session.
//! - [`SteerGate::steer_first_now`] is the same question without the wait, for the last look
//!   before a row is written — see "asked at the door is not asked at the counter" below.
//!
//! **Asked at the door is not asked at the counter (Jacob, Test5 round 12, `r12race`, agentd
//! 0.1.196 `508151f6a` — this module's own fix, on its first retest).** The first cut asked once,
//! at the top of `media::deliver`, and then let the delivery run on: `upload` to the relay,
//! `Agent::room`, and the wait for the chat's room lock — the lock the routing lane needs too, so
//! a delivery is *routinely* parked there while a correction is being routed. The row the user
//! sees is written at the end of all that. So a delivery could pass the gate while the chat was
//! quiet and write its row seconds later with the correction already in the worker's session: a
//! gate consulted strictly before the window it exists to cover. Round 12 recorded the ask at
//! 13:12:23, the correction at 13:12:24, and the uncorrected CSV in the chat at 13:12:29 — the
//! same second the steer reached the session.
//!
//! Both held surfaces now ask again on the far side of the upload, and once more under the room
//! lock through [`SteerGate::steer_first_now`], where awaiting is not allowed: the routing lane
//! takes that lock, so a delivery that waited for routing while holding it would deadlock.
//! Asking [`SteerGate::hold`] twice does not double the delay — [`SteerGate::look`] bounds the
//! wait by each mark's own age, so every call ends at `mark.since + cap` however many are made.
//!
//! **Why a refusal and not a longer wait.** pi hands a steer to a session between tool calls, so
//! a worker parked inside `deliver_file` cannot read the correction while that call is held. The
//! hold therefore ends the call instead: the worker returns, its session gives it the correction,
//! and it posts the corrected file. Exactly one delivery per correction is refused, so a worker
//! whose queue signal never comes posts on its next attempt. The failure mode of this whole
//! module is the old behaviour, never silence.
//!
//! **No coupling with the send-order gate (`send_order.rs`), re-checked against bug AA's fix
//! (#2791).** The surfaces held here — `media::deliver` and `chart::post` from a *worker* — take
//! no send-order ticket and wait on no `wait_turn`, so this hold cannot stack onto
//! [`crate::send_order::SEND_ORDER_WAIT`]: a held file holds no place, so it delays no later
//! message, and it waits behind no place, so no message delays it. The two lanes AA moved are on
//! the other side of that line. The receipt `start_job` posts for a forced ask and the result row
//! `write_job_result` writes directly are both main-lane writes, and the main turn is exempt from
//! the hold because it is the lane that does the routing. `job_steer_race_tests` asks the code
//! rather than trusting the reading.
//!
//! **A cancel inside the hold (bug Z's lane, #2790).** Nothing here outlives the job it holds for
//! and nothing here needs the job to exist. [`RouteMark`]'s `Drop` is owned by `handle_command`
//! and keyed by chat and message, so a job dying under a held delivery cannot strand a mark;
//! [`SteerGate::look`] reads marks and steer counts only, never the job row, so a delivery whose
//! job has just been killed still decides within `cap`; and `forget_job` runs on both of
//! `run_subagent`'s settle paths, the cancelled one included, so the counts go with the job.
//!
//! What the hold *did* break is bug S's rule. It put up to twenty seconds between
//! `media::deliver`'s "is this job cancelled" and the upload, and the question was asked only on
//! the near side, so a job stopped mid-hold posted its file anyway when the hold ended. Both held
//! surfaces now ask again on the far side, as `write_job_result` does around its own wait.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;
// The paused clock a test can drive, which `std::time::Instant` does not follow.
use tokio::time::Instant;

/// Backstop re-check, so no wait depends on a wakeup alone.
const POLL: Duration = Duration::from_millis(50);

/// What [`SteerGate::hold`] decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hold {
    /// Nothing for this chat is in flight, or the hold ran out: post the file.
    Clear,
    /// A correction for this job is in its worker's session and the worker has not read it: this
    /// delivery is the uncorrected one.
    SteerFirst,
}

/// The refusal a worker reads in place of posting the file the user has already corrected.
///
/// It has to be an instruction and not an apology: the model is inside a tool call, and what
/// happens next is whatever this sentence tells it to do. Posting nothing and saying nothing is
/// what round 10 did ("Should I post it as a second message?").
pub(crate) const STEER_FIRST_REFUSAL: &str = "NOT posted — a correction from the user for this job arrived while you were working and is waiting in your session, so this file is the uncorrected one. Nothing went into the chat and nothing is owed to the user yet. End this call, read the correction, redo the work with it applied and post the corrected file with deliver_file. Post one file, not two.";

/// The error `media::deliver` / `chart::post` fail with when the gate answers
/// [`Hold::SteerFirst`], as its own type so the tool wrapper can tell it from a real failure and
/// pass [`STEER_FIRST_REFUSAL`] through word for word.
#[derive(Debug)]
pub(crate) struct SteerFirst;

impl std::fmt::Display for SteerFirst {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(STEER_FIRST_REFUSAL)
    }
}

impl std::error::Error for SteerFirst {}

/// Is this the gate's refusal rather than a failed delivery?
pub(crate) fn is_steer_first(e: &anyhow::Error) -> bool {
    e.downcast_ref::<SteerFirst>().is_some()
}

/// One job's corrections.
#[derive(Default, Debug)]
struct JobSteers {
    /// Corrections handed to the job's worker session (`hook_steer_job`).
    delivered: u64,
    /// Corrections the worker's session has taken off its queue.
    read: u64,
    /// The newest correction a delivery was already refused for. One refusal per correction: a
    /// refusal that repeats is a file that never arrives.
    refused: u64,
}

impl JobSteers {
    fn unread(&self) -> bool {
        self.delivered > self.read && self.delivered > self.refused
    }
}

/// A user message of `chat_id` that the main lane has not finished routing.
struct Mark {
    chat_id: String,
    msg_id: String,
    since: Instant,
}

#[derive(Default)]
struct State {
    marks: Vec<Mark>,
    jobs: HashMap<String, JobSteers>,
}

/// The per-chat routing marks and per-job correction counts. One per box; `Agent` holds it.
#[derive(Default)]
pub(crate) struct SteerGate {
    state: Mutex<State>,
    changed: Notify,
}

impl SteerGate {
    /// Mark `msg_id` of `chat_id` as being routed, until the returned guard is dropped.
    ///
    /// Taken by `handle_command` for the message kinds that can become a steer, and only while
    /// the chat has a job open — a chat with nothing running has nothing to hold.
    pub(crate) fn routing(self: &Arc<Self>, chat_id: &str, msg_id: &str) -> RouteMark {
        {
            let mut s = self.lock();
            s.marks.push(Mark {
                chat_id: chat_id.to_owned(),
                msg_id: msg_id.to_owned(),
                since: Instant::now(),
            });
        }
        RouteMark {
            gate: self.clone(),
            chat_id: chat_id.to_owned(),
            msg_id: msg_id.to_owned(),
        }
    }

    /// A correction was handed to `job_id`'s worker session.
    pub(crate) fn steer_delivered(&self, job_id: &str) {
        self.lock()
            .jobs
            .entry(job_id.to_owned())
            .or_default()
            .delivered += 1;
        self.changed.notify_waiters();
    }

    /// `job_id`'s worker took its queued corrections (pi reports an empty steering queue once it
    /// has handed them to the model).
    pub(crate) fn steers_drained(&self, job_id: &str) {
        let mut s = self.lock();
        if let Some(j) = s.jobs.get_mut(job_id) {
            j.read = j.delivered;
        }
        self.changed.notify_waiters();
    }

    /// Drop a finished job's counts. Called when a job settles, so a long-lived box does not keep
    /// a row per job it has ever run.
    pub(crate) fn forget_job(&self, job_id: &str) {
        self.lock().jobs.remove(job_id);
    }

    /// Is a correction of `job_id` sitting unread right now? Asked without waiting.
    ///
    /// The last look before a row is written, which happens under the chat's room lock. That
    /// lock is one the routing lane takes as well, so a delivery holding it may not await
    /// anything that routing has to finish first — [`Self::hold`] under the lock would deadlock
    /// against the turn it is waiting for. This reads the one fact that needs no waiting.
    ///
    /// Consumes the same one-refusal-per-correction budget as [`Self::hold`], so a worker whose
    /// queue signal never arrives posts on its next attempt rather than never.
    pub(crate) fn steer_first_now(&self, job_id: &str) -> bool {
        take_unread(&mut self.lock(), job_id)
    }

    /// Hold a delivery of `job_id` until the chat has nothing in flight, or `cap` runs out.
    ///
    /// `cap` is a hard bound by construction and not a deadline this loop polices: a mark older
    /// than `cap` is not a reason to wait, so a mark nothing ever releases delays one delivery by
    /// `cap` and then stops mattering. The clock is `tokio::time`'s, so a test can drive it.
    pub(crate) async fn hold(&self, chat_id: &str, job_id: &str, cap: Duration) -> Hold {
        let t0 = Instant::now();
        loop {
            // Registered before the look, so a mark released between the two is a wakeup this
            // wait already holds rather than one it missed.
            let changed = self.changed.notified();
            match self.look(chat_id, job_id, cap) {
                Look::Decided(h) => {
                    let waited = t0.elapsed();
                    if waited >= cap {
                        tracing::warn!(
                            chat_id,
                            job = job_id,
                            waited_ms = waited.as_millis() as u64,
                            cap_ms = cap.as_millis() as u64,
                            decision = ?h,
                            "steer hold: a message of this chat is still being routed; posting anyway"
                        );
                    } else if waited > Duration::ZERO || h == Hold::SteerFirst {
                        tracing::info!(
                            chat_id,
                            job = job_id,
                            waited_ms = waited.as_millis() as u64,
                            decision = ?h,
                            "steer hold"
                        );
                    }
                    return h;
                }
                Look::Wait(left) => {
                    let _ = tokio::time::timeout(left.min(POLL), changed).await;
                }
            }
        }
    }

    /// One read of the state: decide, or say how much longer there is to wait.
    fn look(&self, chat_id: &str, job_id: &str, cap: Duration) -> Look {
        let mut s = self.lock();
        // An unread correction decides on its own: there is nothing to learn from waiting for the
        // rest of the routing, because the answer the routing would give has arrived.
        if take_unread(&mut s, job_id) {
            return Look::Decided(Hold::SteerFirst);
        }
        // Wait while any message of this chat is still live, until the last of them is `cap` old.
        // A message that arrives during the wait extends it; one older than `cap` counts for
        // nothing, which is what makes the bound hold without a deadline to police.
        let left = s
            .marks
            .iter()
            .filter(|m| m.chat_id == chat_id)
            .filter_map(|m| cap.checked_sub(m.since.elapsed()))
            .filter(|d| !d.is_zero())
            .max();
        match left {
            // Only a sleep hint, and `hold` shortens it to `POLL` anyway: the decision is made by
            // the next look, never by this figure.
            Some(d) => Look::Wait(d),
            None => Look::Decided(Hold::Clear),
        }
    }

    /// Test seam: messages of `chat_id` still being routed. The leak check — `handle_command` owes
    /// this zero on every exit.
    #[cfg(test)]
    pub(crate) fn marks(&self, chat_id: &str) -> usize {
        self.lock()
            .marks
            .iter()
            .filter(|m| m.chat_id == chat_id)
            .count()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

enum Look {
    Decided(Hold),
    Wait(Duration),
}

/// Is a correction of `job_id` unread, and if so claim the refusal for it.
///
/// Shared by [`SteerGate::look`] and [`SteerGate::steer_first_now`] so the waiting and
/// non-waiting looks cannot answer the same state differently.
fn take_unread(s: &mut State, job_id: &str) -> bool {
    if !s.jobs.get(job_id).is_some_and(JobSteers::unread) {
        return false;
    }
    let j = s.jobs.entry(job_id.to_owned()).or_default();
    j.refused = j.delivered;
    true
}

/// One message's routing mark, held until this is dropped.
///
/// Dropping it on any path — a refusal, an error, a cancelled turn, a panic unwinding the task —
/// releases the mark, so a message that failed can never hold a job's file.
pub(crate) struct RouteMark {
    gate: Arc<SteerGate>,
    chat_id: String,
    msg_id: String,
}

impl Drop for RouteMark {
    fn drop(&mut self) {
        {
            let mut s = self.gate.lock();
            s.marks
                .retain(|m| !(m.chat_id == self.chat_id && m.msg_id == self.msg_id));
        }
        self.gate.changed.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHAT: &str = "01TESTCHATSTEERGATE0000001";
    const JOB: &str = "01TESTJOBSTEERGATE00000001";

    /// A chat with nothing in flight does not wait, and the mark it does take comes back off.
    #[tokio::test]
    async fn a_quiet_chat_is_not_held() {
        let gate = Arc::new(SteerGate::default());
        let cap = Duration::from_secs(20);
        let t0 = std::time::Instant::now();
        assert_eq!(gate.hold(CHAT, JOB, cap).await, Hold::Clear);
        assert!(t0.elapsed() < Duration::from_millis(500), "it waited");

        let mark = gate.routing(CHAT, "01TESTMSGSTEERGATE00000001");
        assert_eq!(gate.marks(CHAT), 1);
        drop(mark);
        assert_eq!(gate.marks(CHAT), 0);
    }

    /// Another chat's message holds nothing here: the marks are per chat, like the send order's
    /// queues.
    #[tokio::test]
    async fn chats_do_not_hold_each_other() {
        let gate = Arc::new(SteerGate::default());
        let _busy = gate.routing("01TESTCHATSTEERGATE0000002", "01TESTMSGSTEERGATE00000002");
        let t0 = std::time::Instant::now();
        assert_eq!(
            gate.hold(CHAT, JOB, Duration::from_secs(20)).await,
            Hold::Clear
        );
        assert!(t0.elapsed() < Duration::from_millis(500), "it waited");
    }

    /// The mark expires at the cap, whether or not anything releases it, and the wait is bounded
    /// by the message's age rather than restarted by each delivery.
    #[tokio::test]
    async fn a_mark_older_than_the_cap_holds_nothing() {
        let gate = Arc::new(SteerGate::default());
        let cap = Duration::from_millis(200);
        let _stuck = gate.routing(CHAT, "01TESTMSGSTEERGATE00000003");
        assert_eq!(gate.hold(CHAT, JOB, cap).await, Hold::Clear);
        // The mark is still there and still holds nothing: the second delivery pays no wait.
        assert_eq!(gate.marks(CHAT), 1);
        let t0 = std::time::Instant::now();
        assert_eq!(gate.hold(CHAT, JOB, cap).await, Hold::Clear);
        assert!(t0.elapsed() < Duration::from_millis(100), "it waited again");
    }
}
