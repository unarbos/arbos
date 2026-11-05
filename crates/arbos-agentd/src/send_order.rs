//! One FIFO per chat of the user messages that are still owed an answer, in send order.
//!
//! Bug O (Jacob, Test5 round 4, 2026-11-04, agentd 0.1.193): 30 messages sent about three
//! seconds apart were all answered, and none was lost, but after the tenth the answers came
//! back scrambled — `1..10, 14, 17, 23, 13, 18, 20, 26, 15, …` — with a 37-second silence in
//! the middle while thirteen of them queued.
//!
//! Nothing dropped them; three paths took them with no shared order. The main lane spun on
//! `turn_lock.try_lock()`, which is a lottery and not a queue: thirteen waiters each retrying
//! every 50 ms are served in whatever order the spins fall, and one can be passed over many
//! times. Beside it, up to `job_max_per_workspace()` asks ran as parallel jobs and posted their
//! answers at whatever moment each finished. A third path waited on a box-wide count of waiting
//! sends, which a burst in one chat never let fall to zero.
//!
//! This module is the single order all of those read. A message joins its chat's queue when
//! `handle_command` takes the command, and whichever lane ends up answering it holds the
//! returned [`SendTicket`] until that answer's row is posted. Every lane waits for its message
//! to reach the head before it opens a row, so replies are posted in send order while the work
//! behind them still runs in parallel.
//!
//! Order is not held at any price. A lane waits [`SEND_ORDER_WAIT`] and then posts anyway,
//! because a finished answer kept silent behind a long turn reads as a dropped message.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arbos_core::ids::is_ulid;
use tokio::sync::Notify;
// The paused clock a test can drive, which `std::time::Instant` does not follow. The waits in
// this module are minutes long, so a test that could not advance time could not reach them.
use tokio::time::Instant;

/// How long a lane holds a finished answer for the message ahead of it before posting anyway.
///
/// Two minutes, and what it weighs is silence against order rather than working against wedged
/// (Jacob, 2026-11-05). A head is not required to be stuck for this to fire: a turn can
/// legitimately run far longer than two minutes — round 4's `clean` task ran sixteen — and an
/// answer that is finished and sitting silent behind one of those reads as a dropped message,
/// which is worse than arriving slightly out of order. Past this point the answer goes out.
///
/// So this is not a deadlock valve and must not be explained as one, and in particular it is no
/// longer tied to `TURN_DEADLINE_SECS`: the deadline is how long a turn may run, which is a
/// different question from how long someone will wait in silence for an answer that exists.
///
/// Crossing it is logged with the lane and how many messages were still ahead. It fires often by
/// design, and that log is also how a regression in the ordering itself would show up.
pub(crate) const SEND_ORDER_WAIT: Duration = Duration::from_secs(120);

/// Backstop re-check for [`SendOrder::wait_turn`], so no wait depends on a wakeup alone.
const ORDER_POLL: Duration = Duration::from_millis(200);

/// Leading characters of a ULID that hold its millisecond timestamp (48 bits, Crockford base32).
const SENT_MS_CHARS: usize = 10;

/// Send order of one message: its ULID's timestamp first, then the order this box took the
/// command.
///
/// The id is a ULID the sender mints at send time, and its first ten characters are the
/// millisecond it was sent, Crockford base32, which sorts lexicographically. So a command that
/// reaches this box out of order still lands where it was sent.
///
/// The remaining sixteen characters are *random*, not a counter, so two messages sent inside the
/// same millisecond sort by chance — which is how 30 ULIDs minted in a loop come out shuffled.
/// Those fall back to `arrival`: the order this box took the commands, which is the best thing
/// known about two messages the sender could not separate. An id of any other spelling has no
/// timestamp to read, so it inherits the key of the queue's tail and can never jump the queue.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Order {
    sent_ms: String,
    arrival: u64,
}

struct Entry {
    msg_id: String,
    order: Order,
    /// A lane has the floor for this message: [`SendOrder::try_claim`] let it through and it is
    /// writing the answer. A message that joins later never takes a claimed place, whatever its
    /// own key says, so a sender whose clock runs behind cannot displace an answer in flight.
    claimed: bool,
}

/// How many of `q` are ahead of `msg_id`. `None` when it is not queued: nothing orders a retry,
/// a box-issued send, or a job whose ask was answered long ago.
fn place(q: &[Entry], msg_id: &str) -> Option<usize> {
    q.iter().position(|e| e.msg_id == msg_id)
}

/// The per-chat queues. One per box; `Agent` holds it.
#[derive(Default)]
pub(crate) struct SendOrder {
    chats: Mutex<HashMap<String, Vec<Entry>>>,
    arrivals: AtomicU64,
    changed: Notify,
}

impl SendOrder {
    /// Give `msg_id` its place in `chat_id`'s queue, held until the returned ticket is dropped.
    /// `None` when the message is queued already, so a retry or a second command for the same
    /// message cannot take two places.
    pub(crate) fn join(self: &Arc<Self>, chat_id: &str, msg_id: &str) -> Option<SendTicket> {
        {
            let mut chats = self.chats.lock().unwrap_or_else(|e| e.into_inner());
            let q = chats.entry(chat_id.to_owned()).or_default();
            if q.iter().any(|e| e.msg_id == msg_id) {
                return None;
            }
            let arrival = self.arrivals.fetch_add(1, Ordering::SeqCst);
            let sent_ms = match is_ulid(msg_id) {
                true => msg_id[..SENT_MS_CHARS].to_owned(),
                false => q
                    .last()
                    .map(|e| e.order.sent_ms.clone())
                    .unwrap_or_default(),
            };
            let order = Order { sent_ms, arrival };
            let claimed = q.iter().filter(|e| e.claimed).count();
            let at = q.partition_point(|e| e.order <= order).max(claimed);
            q.insert(
                at,
                Entry {
                    msg_id: msg_id.to_owned(),
                    order,
                    claimed: false,
                },
            );
        }
        self.changed.notify_waiters();
        Some(SendTicket {
            order: self.clone(),
            chat_id: chat_id.to_owned(),
            msg_id: msg_id.to_owned(),
        })
    }

    /// Messages of `chat_id` still owed an answer.
    pub(crate) fn waiting(&self, chat_id: &str) -> usize {
        let chats = self.chats.lock().unwrap_or_else(|e| e.into_inner());
        chats.get(chat_id).map(Vec::len).unwrap_or(0)
    }

    /// Messages of `chat_id` sent before `msg_id` that are still owed an answer. Zero when
    /// `msg_id` is not queued, which is the same thing a lane does with it: nothing orders it.
    pub(crate) fn ahead(&self, chat_id: &str, msg_id: &str) -> usize {
        let chats = self.chats.lock().unwrap_or_else(|e| e.into_inner());
        chats
            .get(chat_id)
            .and_then(|q| place(q, msg_id))
            .unwrap_or(0)
    }

    /// `msg_id` may be answered now, and this lane takes the floor for it so a message that
    /// joins afterwards cannot be put in front of the answer being written.
    fn try_claim(&self, chat_id: &str, msg_id: &str) -> bool {
        let mut chats = self.chats.lock().unwrap_or_else(|e| e.into_inner());
        let Some(q) = chats.get_mut(chat_id) else {
            return true;
        };
        match place(q, msg_id) {
            None => true,
            Some(0) => {
                q[0].claimed = true;
                true
            }
            Some(_) => false,
        }
    }

    /// The read [`Self::try_claim`] makes, without taking the floor. For tests: a lane that asks
    /// always means to answer, so production has no use for the question on its own.
    #[cfg(test)]
    pub(crate) fn is_turn(&self, chat_id: &str, msg_id: &str) -> bool {
        let chats = self.chats.lock().unwrap_or_else(|e| e.into_inner());
        match chats.get(chat_id).and_then(|q| place(q, msg_id)) {
            None | Some(0) => true,
            Some(_) => false,
        }
    }

    /// Wait until `msg_id` may be answered. `false` when `bound` ran out first and the caller is
    /// about to answer out of order; see [`SEND_ORDER_WAIT`] for why that is allowed to happen.
    pub(crate) async fn wait_turn(&self, chat_id: &str, msg_id: &str, bound: Duration) -> bool {
        let t0 = Instant::now();
        loop {
            // Registered before the check, so a message leaving the queue between the two is a
            // wakeup this wait already holds rather than one it missed.
            let changed = self.changed.notified();
            if self.try_claim(chat_id, msg_id) {
                return true;
            }
            let left = match bound.checked_sub(t0.elapsed()) {
                Some(d) if !d.is_zero() => d,
                _ => return false,
            };
            let _ = tokio::time::timeout(left.min(ORDER_POLL), changed).await;
        }
    }

    fn done(&self, chat_id: &str, msg_id: &str) {
        {
            let mut chats = self.chats.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(q) = chats.get_mut(chat_id) {
                q.retain(|e| e.msg_id != msg_id);
                if q.is_empty() {
                    chats.remove(chat_id);
                }
            }
        }
        self.changed.notify_waiters();
    }
}

/// One message's place in its chat's queue, held until this is dropped.
///
/// Whichever lane answers the message owns the ticket. The main session keeps it for the length
/// of `Agent::handle_command`; a plain subagent run beside a busy turn moves it into the spawned
/// task, so the queue advances when that subagent's bubble is posted and not when the command
/// handler returns. Dropping it on any path — an error, a cancelled turn, a panic unwinding the
/// task — releases the place, so a failed message can never hold the chat.
///
/// **A place is held for an answer, never for the work behind one.** A job used to hold its ask's
/// place until its *result* row, which is minutes of work away, and bug AA is the bill: twelve
/// later messages queued behind four workers, each paying the whole of [`SEND_ORDER_WAIT`] and
/// coming back out of order, while the result turn that would have freed those places was itself
/// waiting for the same messages to go quiet. A job now answers its ask with a receipt at job
/// start and releases the place there; the result is a later row this queue does not order.
pub(crate) struct SendTicket {
    order: Arc<SendOrder>,
    chat_id: String,
    msg_id: String,
}

impl Drop for SendTicket {
    fn drop(&mut self) {
        self.order.done(&self.chat_id, &self.msg_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbos_core::ids::ulid;

    /// A ULID minted at least a millisecond after the last one, so a test's ids carry the send
    /// order in their timestamp the way a real burst's do. Ids minted in a tight loop share one
    /// millisecond and sort by chance, which `same_millisecond_sends_keep_arrival_order` covers.
    async fn later_ulid() -> String {
        tokio::time::sleep(Duration::from_millis(2)).await;
        ulid()
    }

    /// A job that finishes early waits for the older message's answer; one that finishes late
    /// does not hold the message behind it once its ticket is dropped.
    #[tokio::test]
    async fn an_early_finisher_waits_for_the_older_message() {
        let order = Arc::new(SendOrder::default());
        let chat = "01TESTCHATSENDORDER0000001";
        let first = later_ulid().await;
        let second = later_ulid().await;
        let t1 = order.join(chat, &first).expect("first");
        let _t2 = order.join(chat, &second).expect("second");
        assert!(order.is_turn(chat, &first));
        assert!(!order.is_turn(chat, &second));
        assert_eq!(order.ahead(chat, &second), 1);
        assert!(
            !order
                .wait_turn(chat, &second, Duration::from_millis(50))
                .await,
            "the younger message must not be let through while the older one is owed an answer"
        );
        drop(t1);
        assert!(
            order
                .wait_turn(chat, &second, Duration::from_millis(50))
                .await
        );
    }

    /// A command that reaches this box out of order is still answered in send order: the older
    /// message slots in ahead of the younger one that got here first.
    #[tokio::test]
    async fn a_late_command_keeps_its_sent_place() {
        let order = Arc::new(SendOrder::default());
        let chat = "01TESTCHATSENDORDER0000002";
        let sent_first = later_ulid().await;
        let sent_second = later_ulid().await;
        let _late = order.join(chat, &sent_second).expect("second");
        let _early = order.join(chat, &sent_first).expect("first");
        assert!(order.is_turn(chat, &sent_first));
        assert!(!order.is_turn(chat, &sent_second));
    }

    /// Two messages the sender could not separate — the same millisecond, so random ULID tails —
    /// are answered in the order this box took them, not by whichever tail sorts first.
    #[tokio::test]
    async fn same_millisecond_sends_keep_arrival_order() {
        let order = Arc::new(SendOrder::default());
        let chat = "01TESTCHATSENDORDER0000007";
        // One millisecond, and tails that descend, so only the arrival order can sort these.
        // Written out rather than minted in a loop: `Ulid::new` is random, not monotonic, so a
        // loop proves the point only by luck.
        let sent_ms = ulid()[..SENT_MS_CHARS].to_owned();
        let ids: Vec<String> = "ZYXWVTSRQPNM"
            .chars()
            .map(|c| format!("{sent_ms}{}", String::from(c).repeat(16)))
            .collect();
        assert!(ids.iter().all(|id| is_ulid(id)), "{ids:?}");
        assert!(
            ids.windows(2).all(|w| w[1] < w[0]),
            "the tails must sort against the send order, or this proves nothing"
        );
        let tickets: Vec<SendTicket> = ids
            .iter()
            .map(|id| order.join(chat, id).expect("a fresh place"))
            .collect();
        for (n, (id, ticket)) in ids.iter().zip(tickets).enumerate() {
            assert!(order.is_turn(chat, id), "message {n} is not at the head");
            drop(ticket);
        }
    }

    /// A message that joins while an answer is being written does not take the floor from it,
    /// however early its own timestamp says it was sent.
    #[tokio::test]
    async fn a_claimed_place_is_not_displaced() {
        let order = Arc::new(SendOrder::default());
        let chat = "01TESTCHATSENDORDER0000008";
        let behind = later_ulid().await;
        let answering = later_ulid().await;
        let _t = order.join(chat, &answering).expect("answering");
        assert!(order.wait_turn(chat, &answering, ORDER_POLL).await);
        let _late = order.join(chat, &behind).expect("behind");
        assert!(
            order.is_turn(chat, &answering),
            "the answer in flight keeps the floor"
        );
        assert!(!order.is_turn(chat, &behind));
    }

    /// Each chat has its own order: a burst in one never holds another's reply.
    #[tokio::test]
    async fn chats_do_not_order_each_other() {
        let order = Arc::new(SendOrder::default());
        let busy = "01TESTCHATSENDORDER0000003";
        let quiet = "01TESTCHATSENDORDER0000004";
        let held = ulid();
        let _hold = order.join(busy, &held).expect("held");
        let _behind = order.join(busy, &ulid()).expect("behind");
        let other = ulid();
        let _own = order.join(quiet, &other).expect("own");
        assert!(order.is_turn(quiet, &other));
        assert_eq!(order.waiting(busy), 2);
        assert_eq!(order.waiting(quiet), 1);
    }

    /// Nothing orders a message that never joined — a retry, or a job whose ask is long answered.
    #[tokio::test]
    async fn an_unqueued_message_is_not_gated() {
        let order = Arc::new(SendOrder::default());
        let chat = "01TESTCHATSENDORDER0000005";
        let _held = order.join(chat, &ulid()).expect("held");
        assert!(order.is_turn(chat, "01NEVERJOINEDTHISQUEUE000"));
        assert!(order.is_turn("01TESTCHATNOQUEUEATALL0000", &ulid()));
    }

    /// The same message cannot take two places, so a second command for it is not a deadlock.
    #[tokio::test]
    async fn one_place_per_message() {
        let order = Arc::new(SendOrder::default());
        let chat = "01TESTCHATSENDORDER0000006";
        let id = ulid();
        let _first = order.join(chat, &id).expect("first");
        assert!(order.join(chat, &id).is_none());
        assert_eq!(order.waiting(chat), 1);
    }
}
