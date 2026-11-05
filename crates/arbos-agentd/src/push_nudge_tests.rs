//! How long a box waits to hear that something is waiting for it.
//!
//! A cold chat's room is opened by whichever arrives first: the 3 s registry listing or the 5 s
//! push poll. Both are periodic, so a push queued just after a tick waits a whole interval, and
//! the expected wait for the first of the two is about 1.2 s. That wait sits in front of the
//! room open, which is itself a websocket dial plus a catch-up round trip and cannot be argued
//! away. The wait in front of it can be.
//!
//! The box already holds a socket on every room it has open, and the relay already sends
//! server-initiated frames down them. This measures what happens when the relay uses one to say
//! "your queue is not empty": the same drain, started on a frame instead of on a timer.
//!
//! Both numbers are measured here, in one run, against the real client code — the sync client's
//! frame handling, the real `Incoming` dispatch, and the real drain. The mock is the relay.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use arbos_core::wire::{self, Frame, FrameType, NUDGE_PUSHES};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::routing::get;
use axum::{Json, Router};
use bytes::Bytes;
use serde_json::{json, Value};
use tokio::sync::broadcast;

use crate::status::tests::test_agent_with_incoming;

/// The queue the mock relay serves at `GET /projects/{id}/pushes`, and a count of the drains.
#[derive(Clone, Default)]
struct Queue {
    pushes: Arc<Mutex<Vec<Value>>>,
    drains: Arc<AtomicUsize>,
}

impl Queue {
    fn add(&self, chat_id: &str) {
        self.pushes.lock().unwrap().push(json!({
            "id": 1,
            "kind": "chat_open",
            "payload": { "chat_id": chat_id, "reason": "rows_pending" },
        }));
    }

    fn take(&self) -> Vec<Value> {
        self.drains.fetch_add(1, Ordering::SeqCst);
        std::mem::take(&mut *self.pushes.lock().unwrap())
    }
}

/// The relay's half of a room session, reduced to what the client needs to reach `caught_up`:
/// answer `Hello` with `State`, answer the `RowsReq` that follows with an empty `RowsDone`.
/// After that the socket stays open and carries whatever the test sends down `nudges`.
async fn mock_room(mut socket: WebSocket, mut nudges: broadcast::Receiver<()>) {
    loop {
        tokio::select! {
            got = socket.recv() => {
                let Some(Ok(Message::Binary(b))) = got else { return };
                let Ok(Some(f)) = Frame::decode(b) else { continue };
                let reply = match f.ty {
                    FrameType::Hello => Frame::new(
                        FrameType::State,
                        serde_json::to_value(wire::State {
                            head_seq: 0,
                            seq_floor: 0,
                            checkpoint_seq: 0,
                            checkpoint_size: 0,
                            row_count: 0,
                            row_bytes: 0,
                            box_state: wire::BoxState::Awake,
                        })
                        .unwrap(),
                        Bytes::new(),
                    ),
                    FrameType::RowsReq => {
                        Frame::new(FrameType::RowsDone, json!({ "headSeq": 0 }), Bytes::new())
                    }
                    _ => continue,
                };
                if socket.send(Message::Binary(reply.encode())).await.is_err() {
                    return;
                }
            }
            _ = nudges.recv() => {
                let f = Frame::new(
                    FrameType::Nudge,
                    json!({ "kind": NUDGE_PUSHES }),
                    Bytes::new(),
                );
                if socket.send(Message::Binary(f.encode())).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// Wait until the box's room session is up, so a nudge sent now has a socket to ride.
async fn wait_for_socket(nudges: &broadcast::Sender<()>) {
    for _ in 0..400 {
        if nudges.receiver_count() > 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the box never opened its room session");
}

/// Wait until the box has opened a room for `chat_id`, and say how long that took.
async fn time_to_open(
    agent: &Arc<crate::agent::Agent>,
    chat_id: &str,
    give_up: Duration,
) -> Duration {
    let t0 = Instant::now();
    while t0.elapsed() < give_up {
        if agent.sync.open_ms(chat_id).await.is_some() {
            return t0.elapsed();
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the box never acted on the push for {chat_id} within {give_up:?}");
}

/// A box against a mock relay that serves the push queue and one room, with the box's own
/// `Incoming` dispatch running: the path a frame takes on a real box, end to end, minus the
/// relay. `home_chat` is the room whose socket the nudges ride.
async fn box_with_mock_relay(
    home_chat: &str,
) -> (
    Queue,
    broadcast::Sender<()>,
    Arc<crate::agent::Agent>,
    PathBuf,
) {
    let queue = Queue::default();
    let (nudge_tx, _) = broadcast::channel::<()>(4);
    let q = queue.clone();
    let tx = nudge_tx.clone();
    let extra = Router::new()
        .route(
            "/projects/{id}/pushes",
            get(move || {
                let q = q.clone();
                async move { Json(Value::Array(q.take())) }
            }),
        )
        .route(
            "/room/{project}/{chat}",
            get(move |ws: WebSocketUpgrade| {
                let tx = tx.clone();
                async move { ws.on_upgrade(move |s| mock_room(s, tx.subscribe())) }
            }),
        );
    let (agent, root, mut incoming) = test_agent_with_incoming(extra).await;
    let agent2 = agent.clone();
    tokio::spawn(async move {
        while let Some(inc) = incoming.recv().await {
            crate::workspace::dispatch_incoming(&agent2, inc);
        }
    });
    agent.sync.open_room(home_chat).await.unwrap();
    wait_for_socket(&nudge_tx).await;
    (queue, nudge_tx, agent, root)
}

/// Both paths, in one run, so the numbers are comparable: the same push, the same drain, the
/// same handler, reached once by the timer and once by a frame on a socket that was open the
/// whole time.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_nudge_beats_the_push_poll_to_the_same_queued_push() {
    let (queue, nudge_tx, agent, root) = box_with_mock_relay("01TESTCHATNUDGEHOME000001").await;

    // Before: the queue is filled and nothing says so. The box finds out on its next tick.
    queue.add("01TESTCHATNUDGEPOLLED0001");
    agent.spawn_push_poller();
    let by_poll = time_to_open(&agent, "01TESTCHATNUDGEPOLLED0001", Duration::from_secs(20)).await;

    // After: same queue, same drain, but the relay says so on the open socket. The poller has
    // just ticked, so its next one is a full interval away and cannot be what answers this.
    queue.add("01TESTCHATNUDGENUDGED0001");
    let t0 = Instant::now();
    nudge_tx.send(()).unwrap();
    let by_nudge = time_to_open(&agent, "01TESTCHATNUDGENUDGED0001", Duration::from_secs(20)).await;
    let sent_to_open = t0.elapsed();

    println!(
        "discovery wait: {} ms by the poll, {} ms by the nudge (frame sent to room open: {} ms)",
        by_poll.as_millis(),
        by_nudge.as_millis(),
        sent_to_open.as_millis()
    );
    assert!(
        by_poll >= Duration::from_secs(1),
        "the poll was expected to cost most of its interval, took {by_poll:?}"
    );
    assert!(
        by_nudge <= Duration::from_millis(500),
        "a nudge on an open socket should be immediate, took {by_nudge:?}"
    );
    assert!(
        by_nudge * 4 < by_poll,
        "{by_nudge:?} is not decisively faster than {by_poll:?}"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// The nudge is a hint, not a delivery: it says the queue is not empty and nothing more. A box
/// that acts on it must still get everything from the queue, so a nudge that arrives with an
/// empty queue costs one request and changes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_nudge_with_nothing_queued_is_harmless() {
    let (queue, nudge_tx, _agent, root) = box_with_mock_relay("01TESTCHATNUDGEHOME000002").await;

    for _ in 0..5 {
        nudge_tx.send(()).unwrap();
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let drains = queue.drains.load(Ordering::SeqCst);
    assert!(drains >= 1, "the nudge was acted on at all");
    assert!(
        drains <= 5,
        "five nudges must not multiply into more requests than nudges, got {drains}"
    );
    let _ = std::fs::remove_dir_all(root);
}
