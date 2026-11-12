//! The 1,000-user load gate (docs/box-scaling.md Phase 4): the relay, whole, under the spec's
//! population, with every daemon a fake in this process. Ignored by `cargo test`; run it with
//! `scripts/ci/box-load-gate.sh`.
//!
//! What is real: the relay's database (a file, WAL, the reader pool), its room sockets (every
//! client is a WebSocket through `rooms::ws_room`, pushing Loro rows), the wake jobs and their
//! runner, placement, the resident cap, the idle supervisor and the cold-tier pass.
//!
//! What is modelled, each a knob (`ARBOS_LOAD_*`, [`Knobs`]): how long a daemon takes to wake,
//! sleep and fetch a box, and what the users do. A box is a socket: once its daemon has started
//! it, it dials its user's room with the box token the relay gave it, as agentd does, and the
//! relay calls it awake for that. A wake is counted from what woke the box (the chat coming on
//! screen where its node has room to spare, else the client's PUSH) to that socket being up; how long a real agentd takes to boot and dial is inside the daemon's wake time or not in
//! the model at all. The idle clock runs fast (`ARBOS_LOAD_IDLE_SPEEDUP`), so 15 idle minutes
//! fit a 10-minute run.
//!
//! CPU and memory are this process's, which is the relay and the 2,500 clients and the fake
//! daemons together: an upper bound on the relay's own.

use std::collections::{HashMap, HashSet, VecDeque};
use std::os::unix::fs::PermissionsExt;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message as TMsg;

use arbos_core::doc::{ChatDoc, UserMessageOpts};
use arbos_core::ids::{now_ms, ulid};
use arbos_core::wire::{BoxState, Frame, FrameType};

use crate::boxes::{self, Provider};
use crate::db::Chat;
use crate::people::tests_support::{person, project_of};
use crate::state::test_support::test_state;
use crate::state::{AppState, AuthMode};

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

fn knob<T: FromStr>(name: &str, default: T) -> T {
    match std::env::var(name) {
        Ok(v) => v
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("{name}={v} is not a value it takes")),
        Err(_) => default,
    }
}

/// The run. The defaults are the spec's gate where the spec names a number, a measurement where
/// there is one, and otherwise an assumption that the report prints beside the result.
#[derive(Debug, Clone, Copy)]
struct Knobs {
    /// Registered users, each with a box (spec: 1,000).
    boxes: usize,
    /// Client WebSockets (§4.4: ~2,500).
    sockets: usize,
    nodes: usize,
    /// Each node's `max_resident` (§4.2: 160).
    node_cap: usize,
    /// Boxes in use at once (spec: 150).
    warm: usize,
    /// Wakes of a sleeping box per minute (spec: 30).
    wakes_per_min: f64,
    /// Sleeping boxes woken at one instant, once, as the clock starts: every node is still full
    /// from admission then, so each of them takes the place of an idle box.
    burst: usize,
    minutes: f64,
    /// Daemon time of a clean wake and of a restore, ms. Measured on a 4 vCPU / 16 GB host with
    /// the real agent image: 180–200 and 210–220.
    clean_ms: u64,
    restore_ms: u64,
    /// Daemon time of a clean stop and of a checkpoint, ms. Assumed: the stop is agentd's own
    /// exit, the checkpoint §2's 0.81 s/GB at 1.5 GB.
    sleep_ms: u64,
    ckpt_ms: u64,
    /// Daemon time to bring a cold box back, ms (§2: ~8 s for 1.5 GB).
    fetch_ms: u64,
    /// From a chat coming on screen to the message sent, ms. Assumed.
    think_ms: u64,
    /// Rows a client pushes per minute while its box is in use. Assumed.
    rows_per_min: f64,
    /// How much faster than the wall the idle supervisor's clock runs.
    idle_speedup: i64,
    seed: u64,
}

impl Knobs {
    fn from_env() -> Self {
        Self {
            boxes: knob("ARBOS_LOAD_BOXES", 1000),
            sockets: knob("ARBOS_LOAD_SOCKETS", 2500),
            nodes: knob("ARBOS_LOAD_NODES", 2),
            node_cap: knob("ARBOS_LOAD_NODE_CAP", 160),
            warm: knob("ARBOS_LOAD_WARM", 150),
            wakes_per_min: knob("ARBOS_LOAD_WAKES_PER_MIN", 30.0),
            burst: knob("ARBOS_LOAD_BURST", 0),
            minutes: knob("ARBOS_LOAD_MINUTES", 10.0),
            clean_ms: knob("ARBOS_LOAD_CLEAN_WAKE_MS", 200),
            restore_ms: knob("ARBOS_LOAD_RESTORE_MS", 220),
            sleep_ms: knob("ARBOS_LOAD_SLEEP_MS", 500),
            ckpt_ms: knob("ARBOS_LOAD_CKPT_MS", 1200),
            fetch_ms: knob("ARBOS_LOAD_FETCH_MS", 8000),
            think_ms: knob("ARBOS_LOAD_THINK_MS", 10_000),
            rows_per_min: knob("ARBOS_LOAD_ROWS_PER_MIN", 60.0),
            idle_speedup: knob("ARBOS_LOAD_IDLE_SPEEDUP", 30),
            seed: knob("ARBOS_LOAD_SEED", 1),
        }
    }
}

/// xorshift64: the same run for the same seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// In `[0, 1)`.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    /// Exponential with this mean, kept inside `[mean / 20, 4 × mean]`.
    fn around(&mut self, mean: Duration) -> Duration {
        mean.mul_f64((-(1.0 - self.unit()).ln()).clamp(0.05, 4.0))
    }
}

// ---- the fake daemon -------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Kind {
    Clean,
    Ckpt,
    /// Fetched back from the object store since it last ran.
    Cold,
}

#[derive(Default, Clone, Copy)]
struct FakeBox {
    running: bool,
    /// The daemon has been asked to wake it and has not answered yet.
    waking: bool,
    ckpt: bool,
    ckpt_cold: bool,
    work_cold: bool,
    /// When the fetch that is bringing it back ends.
    fetching: Option<Instant>,
    fetched: bool,
}

struct Woke {
    user: String,
    kind: Kind,
    /// When the daemon was asked, when it had the box up, and when the box's socket was up.
    asked: Instant,
    done: Instant,
    at: Instant,
}

/// A daemon that answers what `arbos-boxd` answers (its README), after the time a real one takes.
struct FakeNode {
    k: Knobs,
    subnet: u8,
    disk_free_gb: f64,
    boxes: Mutex<HashMap<String, FakeBox>>,
    rng: Mutex<Rng>,
    woke: mpsc::UnboundedSender<Woke>,
    /// user → their room's address, the box token from `ensure`, and what ends the box's socket.
    rooms: Arc<Mutex<HashMap<String, (String, String)>>>,
    tokens: Mutex<HashMap<String, String>>,
    stops: Mutex<HashMap<String, oneshot::Sender<()>>>,
    /// The most boxes that were running or being woken here at one instant.
    most_up: AtomicUsize,
    sleeps: AtomicU64,
    archives: AtomicU64,
}

/// A box that has just been started: it dials its user's room as agentd does, trying again
/// while the relay does not know its token yet (a new box's row is written once it is up), and
/// holds the socket until its daemon stops it.
async fn box_socket(
    url: String,
    room: String,
    woke: mpsc::UnboundedSender<Woke>,
    up: Woke,
    stop: oneshot::Receiver<()>,
) {
    let mut tries = 0;
    let ws = loop {
        match tokio_tungstenite::connect_async(&url).await {
            Ok((ws, _)) => break ws,
            Err(_) if tries < 600 => tokio::time::sleep(Duration::from_millis(50)).await,
            Err(_) => return,
        }
        tries += 1;
    };
    let (mut sink, mut stream) = ws.split();
    let hello = json!({ "cursor": 0, "device": "box:load", "room": room,
        "proto": arbos_core::PROTO_VERSION });
    let _ = sink.send(frame(FrameType::Hello, hello, Vec::new())).await;
    let _ = woke.send(Woke {
        at: Instant::now(),
        ..up
    });
    tokio::pin!(stop);
    loop {
        tokio::select! {
            _ = &mut stop => break,
            msg = stream.next() => {
                let Some(Ok(TMsg::Binary(b))) = msg else {
                    break;
                };
                if Frame::decode(b).ok().flatten().map(|f| f.ty) == Some(FrameType::State) {
                    let after = json!({ "after": 0, "excludeOwn": true });
                    let _ = sink.send(frame(FrameType::RowsReq, after, Vec::new())).await;
                }
            }
        }
    }
    let _ = sink.close().await;
}

fn refused(status: StatusCode, why: &str) -> Response {
    (status, Json(json!({ "error": why }))).into_response()
}

async fn answer(State(node): State<Arc<FakeNode>>, req: Request) -> Response {
    let bearer = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok());
    if bearer != Some(&format!("Bearer {TOKEN}")) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let path = req.uri().path().to_owned();
    let force = req.uri().query() == Some("force=true");
    let body = axum::body::to_bytes(req.into_body(), 1 << 20)
        .await
        .unwrap_or_default();
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let ms = Duration::from_millis;
    let get = |user: &str| node.boxes.lock().unwrap().get(user).copied();
    let put = |user: &str, b: FakeBox| node.boxes.lock().unwrap().insert(user.to_owned(), b);
    match parts[..] {
        ["v1", "host", "status"] => {
            let boxes = node.boxes.lock().unwrap();
            let list: Vec<Value> = boxes
                .iter()
                .map(|(user, b)| {
                    json!({ "user_id": user, "running": b.running, "size_gb": 8,
                        "work_bytes": if b.work_cold { 0u64 } else { 500 << 20 },
                        "ckpt_bytes": if b.ckpt && !b.ckpt_cold { 1500u64 << 20 } else { 0 },
                        "work_cold": b.work_cold, "ckpt_cold": b.ckpt_cold })
                })
                .collect();
            Json(
                json!({ "mem_available_gb": 400.0, "swap_used_gb": 0.0, "load5": 1.0,
                "disk_free_gb": node.disk_free_gb,
                "resident_boxes": boxes.values().filter(|b| b.running).count(),
                "present_boxes": boxes.values().filter(|b| !b.work_cold).count(),
                "cold_tier": true, "boxes": list }),
            )
            .into_response()
        }
        ["v1", "boxes", user, "ensure"] => {
            let created = get(user).is_none();
            if created {
                put(user, FakeBox::default());
            }
            let env = serde_json::from_slice::<Value>(&body).unwrap_or_default();
            if let Some(token) = env["env"]["ARBOS_BOX_TOKEN"].as_str() {
                let mut tokens = node.tokens.lock().unwrap();
                tokens.insert(user.to_owned(), token.to_owned());
            }
            Json(json!({ "created": created, "size_gb": 8 })).into_response()
        }
        ["v1", "boxes", user, "wake"] => {
            let Some(mut b) = get(user) else {
                return refused(StatusCode::NOT_FOUND, "no such box");
            };
            if !b.running {
                let asked = Instant::now();
                // What the cap is a promise about: a box holds memory from here on.
                b.waking = true;
                let up = {
                    let mut boxes = node.boxes.lock().unwrap();
                    boxes.insert(user.to_owned(), b);
                    boxes.values().filter(|b| b.running || b.waking).count()
                };
                node.most_up.fetch_max(up, Ordering::Relaxed);
                // A cold box nobody asked back is fetched here, before it starts.
                if (b.work_cold || b.ckpt_cold) && !b.fetched {
                    tokio::time::sleep(ms(node.k.fetch_ms)).await;
                    b.fetched = true;
                }
                let restore = if b.ckpt {
                    node.k.restore_ms
                } else {
                    node.k.clean_ms
                };
                tokio::time::sleep(ms(restore)).await;
                let kind = match (b.fetched, b.ckpt) {
                    (true, _) => Kind::Cold,
                    (false, true) => Kind::Ckpt,
                    (false, false) => Kind::Clean,
                };
                let restored = b.ckpt;
                put(
                    user,
                    FakeBox {
                        running: true,
                        ..FakeBox::default()
                    },
                );
                let up = Woke {
                    user: user.to_owned(),
                    kind,
                    asked,
                    done: Instant::now(),
                    at: Instant::now(),
                };
                let room = node.rooms.lock().unwrap().get(user).cloned();
                let token = node.tokens.lock().unwrap().get(user).cloned();
                if let (Some((address, room)), Some(token)) = (room, token) {
                    let (stop, stopped) = oneshot::channel();
                    node.stops.lock().unwrap().insert(user.to_owned(), stop);
                    let url = format!("{address}?token={token}");
                    tokio::spawn(box_socket(url, room, node.woke.clone(), up, stopped));
                }
                let url = format!("http://10.{}.0.2:7788", node.subnet);
                return Json(json!({ "state": "warm", "restored": restored, "box_url": url }))
                    .into_response();
            }
            let url = format!("http://10.{}.0.2:7788", node.subnet);
            Json(json!({ "state": "warm", "restored": false, "box_url": url })).into_response()
        }
        ["v1", "boxes", user, "sleep"] => {
            let Some(mut b) = get(user) else {
                return refused(StatusCode::NOT_FOUND, "no such box");
            };
            let mut split = Value::Null;
            if b.running {
                // §3.2: about one sleep in twenty finds a user process and is a checkpoint.
                b.ckpt = !force && node.rng.lock().unwrap().unit() < 0.05;
                let took = if b.ckpt {
                    node.k.ckpt_ms
                } else {
                    node.k.sleep_ms
                };
                tokio::time::sleep(ms(took)).await;
                b.running = false;
                put(user, b);
                // The box is gone, and its socket with it.
                let stop = node.stops.lock().unwrap().remove(user);
                if let Some(stop) = stop {
                    let _ = stop.send(());
                }
                node.sleeps.fetch_add(1, Ordering::Relaxed);
                split = json!(300u64 << 20);
            }
            let state = match (b.work_cold || b.ckpt_cold, b.ckpt) {
                (true, _) => "cold",
                (false, true) => "asleep_ckpt",
                (false, false) => "asleep_clean",
            };
            Json(
                json!({ "state": state, "files_bytes": split, "scratch_bytes": split,
                "trash_bytes": split, "work_bytes": 500u64 << 20,
                "ckpt_bytes": if b.ckpt { 1500u64 << 20 } else { 0 } }),
            )
            .into_response()
        }
        ["v1", "boxes", user, "archive"] => {
            let part = serde_json::from_slice::<Value>(&body).unwrap_or_default();
            let Some(mut b) = get(user).filter(|b| !b.running) else {
                return refused(StatusCode::CONFLICT, "the box is awake");
            };
            match part["part"].as_str() {
                Some("ckpt") if b.ckpt => b.ckpt_cold = true,
                Some("work") => b.work_cold = true,
                _ => return refused(StatusCode::CONFLICT, "no such part"),
            }
            b.fetched = false;
            put(user, b);
            node.archives.fetch_add(1, Ordering::Relaxed);
            Json(json!({ "archived": true, "stored_bytes": 400u64 << 20 })).into_response()
        }
        ["v1", "boxes", user, "prefetch"] => {
            let Some(mut b) = get(user) else {
                return refused(StatusCode::NOT_FOUND, "no such box");
            };
            let cold = (b.work_cold || b.ckpt_cold) && !b.fetched;
            if cold {
                let done = *b
                    .fetching
                    .get_or_insert(Instant::now() + ms(node.k.fetch_ms));
                if Instant::now() >= done {
                    (b.fetching, b.fetched) = (None, true);
                }
                put(user, b);
            }
            Json(json!({ "cold": cold && !b.fetched })).into_response()
        }
        _ => refused(StatusCode::NOT_FOUND, "no such route"),
    }
}

// ---- the clients -----------------------------------------------------------------------------

enum Cmd {
    /// The chat came on screen, or left it.
    Focus(bool),
    /// One row; the answer is when its ACK arrived.
    Push(String, Vec<u8>, oneshot::Sender<Instant>),
}

#[derive(Default)]
struct Sockets {
    /// Connected and not yet closed.
    live: AtomicUsize,
    /// Caught up with their room.
    ready: AtomicUsize,
    errors: AtomicU64,
}

fn frame(ty: FrameType, header: Value, payload: Vec<u8>) -> TMsg {
    TMsg::Binary(Frame::new(ty, header, bytes::Bytes::from(payload)).encode())
}

/// One device's socket on its user's chat room, as an app holds it: HELLO, catch up, a PRESENCE
/// every half minute, and whatever it is told to send.
async fn socket(
    url: String,
    room: String,
    chat: String,
    phase: Duration,
    mut cmds: mpsc::UnboundedReceiver<Cmd>,
    sockets: Arc<Sockets>,
) {
    let Ok((ws, _)) = tokio_tungstenite::connect_async(&url).await else {
        sockets.errors.fetch_add(1, Ordering::Relaxed);
        return;
    };
    sockets.live.fetch_add(1, Ordering::Relaxed);
    let (mut sink, mut stream) = ws.split();
    let hello = json!({ "cursor": 0, "device": "dev:load", "room": room,
        "proto": arbos_core::PROTO_VERSION });
    let _ = sink.send(frame(FrameType::Hello, hello, Vec::new())).await;
    let mut focused = false;
    let mut unacked: VecDeque<oneshot::Sender<Instant>> = VecDeque::new();
    let every = Duration::from_secs(30);
    let mut beat = tokio::time::interval_at(tokio::time::Instant::now() + phase, every);
    let presence = |focused: bool| {
        let header = if focused {
            json!({ "focused_chat": chat })
        } else {
            json!({})
        };
        frame(FrameType::Presence, header, Vec::new())
    };
    loop {
        let sent = tokio::select! {
            _ = beat.tick() => sink.send(presence(focused)).await,
            cmd = cmds.recv() => match cmd {
                None => break,
                Some(Cmd::Focus(on)) => {
                    focused = on;
                    sink.send(presence(focused)).await
                }
                Some(Cmd::Push(batch, row, acked)) => {
                    unacked.push_back(acked);
                    sink.send(frame(FrameType::Push, json!({ "batchId": batch }), row)).await
                }
            },
            msg = stream.next() => {
                let Some(Ok(TMsg::Binary(b))) = msg else {
                    break;
                };
                let got = Frame::decode(b).ok().flatten();
                match got.as_ref().map(|f| f.ty) {
                    Some(FrameType::State) => {
                        let after = json!({ "after": 0, "excludeOwn": true });
                        sink.send(frame(FrameType::RowsReq, after, Vec::new())).await
                    }
                    Some(FrameType::RowsDone) => {
                        sockets.ready.fetch_add(1, Ordering::Relaxed);
                        Ok(())
                    }
                    Some(FrameType::Ack) => {
                        if let Some(acked) = unacked.pop_front() {
                            let _ = acked.send(Instant::now());
                        }
                        Ok(())
                    }
                    Some(FrameType::Error) => {
                        if sockets.errors.fetch_add(1, Ordering::Relaxed) == 0 {
                            let header = got.map(|f| f.header);
                            println!("  the first error a socket was sent: {header:?}");
                        }
                        Ok(())
                    }
                    _ => Ok(()),
                }
            }
        };
        if sent.is_err() {
            break;
        }
    }
    sockets.live.fetch_sub(1, Ordering::Relaxed);
}

struct User {
    id: String,
    project: String,
    chat: String,
    /// The device every row of this user's is pushed from.
    primary: mpsc::UnboundedSender<Cmd>,
    doc: Mutex<ChatDoc>,
    rows: AtomicU64,
}

#[derive(Default)]
struct Stats {
    wakes: Mutex<HashMap<Kind, Vec<f64>>>,
    /// A wake in its three parts: waiting for a slot (from what woke it to its daemon being
    /// asked), the daemon's wake, and the box's dial.
    parts: Mutex<[Vec<f64>; 3]>,
    /// How long the sender of the message waited for the box: nothing, for one that the chat
    /// coming on screen had already woken.
    waited: Mutex<Vec<f64>>,
    wakes_lost: AtomicU64,
    acks: Mutex<Vec<f64>>,
    acks_lost: AtomicU64,
    idle_sleeps: AtomicU64,
}

struct Run {
    k: Knobs,
    state: AppState,
    users: Vec<Arc<User>>,
    stats: Stats,
    /// When what may wake a box happened (the chat on screen, or the PUSH if that did not), and
    /// when the PUSH was sent.
    waking: Mutex<HashMap<String, (Instant, Option<Instant>)>>,
    /// Users whose box is in use, or about to be.
    busy: Mutex<HashSet<String>>,
    rng: Mutex<Rng>,
    over: AtomicBool,
}

impl Run {
    /// Push one message row from the user's device; how long its ACK took.
    async fn push(&self, u: &User) -> Option<Duration> {
        let n = u.rows.fetch_add(1, Ordering::Relaxed);
        let row = {
            let doc = u.doc.lock().unwrap();
            let before = doc.oplog_vv();
            if n == 0 {
                doc.init_meta(&u.chat, &u.project, 1);
            }
            let text = "A message of about the length people type, give or take a clause.";
            doc.append_user_message(
                &format!("m{n}"),
                text,
                "dev:load",
                &UserMessageOpts::default(),
            )
            .ok()?;
            doc.export_updates_since(&before)
        };
        let (acked, ack) = oneshot::channel();
        let sent = Instant::now();
        u.primary
            .send(Cmd::Push(format!("b{n}"), row, acked))
            .ok()?;
        let at = tokio::time::timeout(Duration::from_secs(30), ack)
            .await
            .ok()?
            .ok()?;
        Some(at - sent)
    }

    async fn push_counted(&self, u: &User) {
        match self.push(u).await {
            Some(took) => self.stats.acks.lock().unwrap().push(took.as_secs_f64()),
            None => drop(self.stats.acks_lost.fetch_add(1, Ordering::Relaxed)),
        }
    }

    /// A box in use until `ends`: a turn runs (the box's keepalive says so) and rows arrive.
    async fn session(&self, u: &User, ends: Instant) {
        let row_every = Duration::from_secs_f64(60.0 / self.k.rows_per_min);
        let mut rows = tokio::time::interval(row_every);
        let mut alive = tokio::time::interval(Duration::from_secs(30));
        let end = tokio::time::sleep_until(ends.into());
        tokio::pin!(end);
        loop {
            tokio::select! {
                _ = &mut end => break,
                _ = rows.tick() => self.push_counted(u).await,
                _ = alive.tick() => drop(boxes::keepalive(&self.state, &u.id, true).await),
            }
            if self.over.load(Ordering::Relaxed) {
                break;
            }
        }
        let _ = boxes::keepalive(&self.state, &u.id, false).await;
        // The idle clock runs fast: at its speed this user's last row is long ago.
        self.state.last_client_push.lock().await.remove(&u.id);
        let _ = u.primary.send(Cmd::Focus(false));
        self.busy.lock().unwrap().remove(&u.id);
    }

    /// A user comes back: the chat on screen, a message a while later, and once the box is up,
    /// a session on it.
    async fn arrival(self: Arc<Self>, u: Arc<User>, session: Duration) {
        let looked = Instant::now();
        let _ = u.primary.send(Cmd::Focus(true));
        // The look wakes the box when its node has room to spare, at once or at a later beat
        // of the device's presence: the wake is timed from when the relay took it up, as near
        // as a look every 10 ms tells.
        let mut woken = false;
        while looked.elapsed() < Duration::from_millis(self.k.think_ms) {
            if !woken && self.state.box_state(&u.id).await.0 != BoxState::Asleep {
                woken = true;
                let mut waking = self.waking.lock().unwrap();
                waking.insert(u.id.clone(), (Instant::now(), None));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let sent = Instant::now();
        // Still asleep: the PUSH is what wakes it.
        if self.state.box_state(&u.id).await.0 == BoxState::Asleep {
            let mut waking = self.waking.lock().unwrap();
            waking.insert(u.id.clone(), (sent, Some(sent)));
        } else if let Some(w) = self.waking.lock().unwrap().get_mut(&u.id) {
            w.1 = Some(sent);
        }
        self.push_counted(&u).await;
        // Unprefetched from cold is the slowest wake there is; twice that is a lost one.
        let patience = Duration::from_millis(60_000 + 4 * self.k.fetch_ms);
        let asked = Instant::now();
        while self.state.box_state(&u.id).await.0 != BoxState::Awake {
            if asked.elapsed() > patience || self.over.load(Ordering::Relaxed) {
                self.stats.wakes_lost.fetch_add(1, Ordering::Relaxed);
                self.waking.lock().unwrap().remove(&u.id);
                self.busy.lock().unwrap().remove(&u.id);
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        self.session(&u, Instant::now() + session).await;
    }
}

// ---- measuring -------------------------------------------------------------------------------

/// The `p`th percentile of the samples, in their own unit.
fn pct(samples: &mut [f64], p: f64) -> f64 {
    if samples.is_empty() {
        return f64::NAN;
    }
    samples.sort_by(f64::total_cmp);
    samples[((samples.len() as f64 * p / 100.0).ceil() as usize).clamp(1, samples.len()) - 1]
}

/// This process's CPU seconds so far and the most memory it has had resident, in MB. Asked of
/// the kernel: past some 10,000 open descriptors macOS does not start a `ps` to ask.
fn cpu_and_rss() -> (f64, f64) {
    use nix::sys::resource::{getrusage, UsageWho};
    let used = getrusage(UsageWho::RUSAGE_SELF).expect("getrusage");
    let secs = |t: nix::sys::time::TimeVal| t.tv_sec() as f64 + t.tv_usec() as f64 / 1e6;
    // Bytes on macOS, kilobytes on Linux.
    let per_mb = if cfg!(target_os = "macos") {
        1024.0 * 1024.0
    } else {
        1024.0
    };
    (
        secs(used.user_time()) + secs(used.system_time()),
        used.max_rss() as f64 / per_mb,
    )
}

fn open_files() -> usize {
    std::fs::read_dir("/dev/fd")
        .map(Iterator::count)
        .unwrap_or(0)
}

/// Where a line of the gate stands.
fn verdict(name: &str, value: String, pass: bool) -> bool {
    println!(
        "  {:<4} {name:<46} {value}",
        if pass { "ok" } else { "FAIL" }
    );
    pass
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "the load gate: minutes of wall clock; run scripts/ci/box-load-gate.sh"]
async fn the_thousand_user_load_gate() {
    let k = Knobs::from_env();
    println!("\nbox load gate, docs/box-scaling.md Phase 4\n  {k:?}");
    assert!(k.boxes > k.warm && k.sockets >= k.boxes && k.nodes >= 1 && k.nodes <= 40);

    // The nodes: each a fake daemon on loopback, configured the way an operator would.
    let dir = std::env::temp_dir().join(format!("arbos-load-gate-{}", ulid()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = |name: &str| dir.join(name).to_str().unwrap().to_owned();
    std::fs::write(file("token"), TOKEN).unwrap();
    let private = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(file("token"), private).unwrap();
    let (woke, mut woken) = mpsc::unbounded_channel();
    let rooms: Arc<Mutex<HashMap<String, (String, String)>>> = Arc::default();
    let mut fakes = Vec::new();
    let mut urls = Vec::new();
    for i in 0..k.nodes {
        let node = Arc::new(FakeNode {
            k,
            subnet: 207 + i as u8,
            // Every node but the first is short of disk, so sleeping disks go cold too (§3.3).
            disk_free_gb: if i == 0 { 900.0 } else { 200.0 },
            boxes: Mutex::default(),
            rng: Mutex::new(Rng(k.seed.wrapping_mul(0x9e37_79b9_7f4a_7c15)
                + i as u64
                + 1)),
            woke: woke.clone(),
            rooms: rooms.clone(),
            tokens: Mutex::default(),
            stops: Mutex::default(),
            most_up: AtomicUsize::new(0),
            sleeps: AtomicU64::new(0),
            archives: AtomicU64::new(0),
        });
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        urls.push(format!("http://{}", l.local_addr().unwrap()));
        let app = Router::new().fallback(answer).with_state(node.clone());
        tokio::spawn(async move { axum::serve(l, app).await });
        fakes.push(node);
    }
    let others: Vec<Value> = (1..k.nodes)
        .map(|i| {
            json!({ "id": format!("node-{i}"), "url": urls[i], "token_file": file("token"),
                "max_resident": k.node_cap, "box_subnet": format!("10.{}.0.0/16", 207 + i) })
        })
        .collect();
    std::fs::write(file("nodes.json"), json!(others).to_string()).unwrap();
    let mut state = test_state(AuthMode::Github);
    let nodes = file("nodes.json");
    let boxd =
        crate::boxd::Boxd::from_files(&urls[0], &file("token"), Some(k.node_cap), Some(&nodes));
    let cfg = Arc::get_mut(&mut state.cfg).expect("one holder");
    cfg.boxd = Some(boxd.unwrap());
    cfg.box_idle_grace_ms = 30_000;

    // The relay's side: the room route, and the wake runner and the sweeper (box liveness, the
    // presence flush) as `main` starts them.
    let app = Router::new()
        .route("/room/{project}/{chat}", get(crate::rooms::ws_room))
        .with_state(state.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await });
    boxes::spawn_wake_runner(state.clone()).await;
    crate::boxd::spawn_headroom(&state);
    crate::rooms::spawn_sweeper(state.clone()).await;

    // The people: a user, a project, a chat, and two or three devices each on the chat's room.
    let sockets = Arc::new(Sockets::default());
    let connecting = Instant::now();
    let mut rng = Rng(k.seed.max(1));
    let mut users = Vec::new();
    let mut passive = Vec::new();
    for i in 0..k.boxes {
        let (user, _) = person(&state, 500_000 + i as i64);
        let project = project_of(&state, &user, "load");
        let chat = Chat {
            id: ulid(),
            project_id: project.clone(),
            title: "t".into(),
            is_pinned: false,
            position: 0,
            created_at: now_ms(),
            doc_epoch: 1,
            kind: "chat".into(),
            participants: Vec::new(),
            forked_from: None,
        };
        state.db.insert_chat(&chat).unwrap();
        let doc = ChatDoc::new(Some(1_000 + i as u64), false);
        let devices = k.sockets / k.boxes + usize::from(i < k.sockets % k.boxes);
        let mut primary = None;
        for _ in 0..devices {
            let token = format!("adv_{}", ulid().to_ascii_lowercase());
            let hash = arbos_core::auth::hash(&token);
            state
                .db
                .insert_device(&ulid(), "t", &hash, &user, "paircode")
                .unwrap();
            let room = format!("{project}/{}", chat.id);
            let address = format!("ws://{addr}/room/{room}");
            let mut rooms = rooms.lock().unwrap();
            rooms.insert(user.clone(), (address.clone(), room.clone()));
            drop(rooms);
            let url = format!("{address}?token={token}");
            let (tx, cmds) = mpsc::unbounded_channel();
            let phase = Duration::from_millis(rng.next() % 30_000);
            tokio::spawn(socket(
                url,
                room,
                chat.id.clone(),
                phase,
                cmds,
                sockets.clone(),
            ));
            match primary {
                None => primary = Some(tx),
                Some(_) => passive.push(tx),
            }
        }
        users.push(Arc::new(User {
            id: user,
            project,
            chat: chat.id,
            primary: primary.expect("a device"),
            doc: Mutex::new(doc),
            rows: AtomicU64::new(0),
        }));
        if i % 64 == 63 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    while sockets.ready.load(Ordering::Relaxed) < k.sockets {
        assert!(
            connecting.elapsed() < Duration::from_secs(120),
            "{} of {} sockets caught up ({} failed): is `ulimit -n` high enough?",
            sockets.ready.load(Ordering::Relaxed),
            k.sockets,
            sockets.errors.load(Ordering::Relaxed)
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    println!(
        "  {} users, {} sockets caught up in {:.1} s",
        k.boxes,
        k.sockets,
        connecting.elapsed().as_secs_f64()
    );

    let run = Arc::new(Run {
        k,
        state: state.clone(),
        users,
        stats: Stats::default(),
        waking: Mutex::default(),
        busy: Mutex::default(),
        rng: Mutex::new(rng),
        over: AtomicBool::new(false),
    });
    // A wake is over when the box's socket is on its room.
    let timing = run.clone();
    tokio::spawn(async move {
        while let Some(w) = woken.recv().await {
            let sent = timing.waking.lock().unwrap().remove(&w.user);
            if let Some((sent, pushed)) = sent {
                let secs = |a: Instant, b: Instant| b.saturating_duration_since(a).as_secs_f64();
                let mut parts = timing.stats.parts.lock().unwrap();
                parts[0].push(secs(sent, w.asked));
                parts[1].push(secs(w.asked, w.done));
                parts[2].push(secs(w.done, w.at));
                let waited = pushed.map_or(0.0, |p| secs(p, w.at));
                timing.stats.waited.lock().unwrap().push(waited);
                let took = secs(sent, w.at);
                let mut wakes = timing.stats.wakes.lock().unwrap();
                wakes.entry(w.kind).or_default().push(took);
            }
        }
    });

    // Admission: every user's first box, eight users at a time, through the call a sign-in and
    // `POST /machine/start` make. The nodes fill long before the last of them.
    let admitting = Instant::now();
    let made = futures::stream::iter(run.users.clone())
        .map(|u| {
            let state = state.clone();
            async move {
                let t = Instant::now();
                let made = boxes::ensure_user_box(&state, &u.id, Provider::Boxd).await;
                made.map(|_| t.elapsed().as_secs_f64())
                    .map_err(|e| e.to_string())
            }
        })
        .buffer_unordered(8)
        .collect::<Vec<_>>()
        .await;
    let refusals: Vec<&String> = made.iter().filter_map(|m| m.as_ref().err()).collect();
    let mut create_s: Vec<f64> = made
        .iter()
        .filter_map(|m| m.as_ref().ok().copied())
        .collect();
    let admitted_in = admitting.elapsed().as_secs_f64();
    let evicted_admitting: u64 = fakes.iter().map(|f| f.sleeps.load(Ordering::Relaxed)).sum();
    let waitlist = state.db.waitlist().unwrap().len();
    let cap = crate::boxd::capacity(&state)
        .await
        .expect("the nodes answer");
    println!(
        "  admission: {} boxes in {admitted_in:.0} s, create p50 {:.2} s p95 {:.2} s, {evicted_admitting} put to sleep at the cap",
        create_s.len(),
        pct(&mut create_s, 50.0),
        pct(&mut create_s, 95.0),
    );
    for node in &cap.nodes {
        println!(
            "    {}: {} awake of {}, {} present",
            node.id, node.resident, node.max_resident, node.present
        );
    }
    if let Some(e) = refusals.first() {
        println!("  {} boxes refused; the first: {e}", refusals.len());
    }

    // The sleepers have been asleep for anything up to a month, and the cold-tier pass has had
    // ten turns at them.
    for b in state.db.list_boxes().unwrap() {
        if state.box_state(&b.user_id).await.0 != BoxState::Asleep {
            continue;
        }
        let days = match run.rng.lock().unwrap().unit() {
            x if x < 0.7 => 3.0 * x / 0.7,
            x if x < 0.9 => 3.0 + 11.0 * (x - 0.7) / 0.2,
            x => 14.0 + 16.0 * (x - 0.9) / 0.1,
        };
        let mut row = b;
        row.last_stopped_at = Some(now_ms() - (days * 86_400_000.0) as i64);
        state.db.upsert_box(&row).unwrap();
    }
    for _ in 0..10 {
        crate::boxd::tier_tick(&state).await;
    }
    let archived_before: u64 = fakes
        .iter()
        .map(|f| f.archives.load(Ordering::Relaxed))
        .sum();

    // `warm` of the awake boxes are in use, each for as long as keeps that many in use at the
    // spec's rate of arrivals.
    let session = Duration::from_secs_f64(60.0 * k.warm as f64 / k.wakes_per_min);
    let steady = Duration::from_secs_f64(60.0 * k.minutes);
    let started = Instant::now();
    let mut in_use = 0;
    for u in &run.users {
        if in_use < k.warm && state.box_state(&u.id).await.0 == BoxState::Awake {
            in_use += 1;
            run.busy.lock().unwrap().insert(u.id.clone());
            let ends = started + run.rng.lock().unwrap().around(session);
            let (run, u) = (run.clone(), u.clone());
            tokio::spawn(async move { run.session(&u, ends).await });
        }
    }

    // The write lock, sampled every 5 ms from a thread of its own: how long a writer that
    // arrives at that moment waits for it (§4.4).
    let probe_db = state.db.clone();
    let probing = run.clone();
    let probe = std::thread::spawn(move || {
        let mut waits = Vec::new();
        while !probing.over.load(Ordering::Relaxed) {
            let t = Instant::now();
            let _ = probe_db.with(|_| Ok(()));
            waits.push(t.elapsed().as_secs_f64() * 1e3);
            std::thread::sleep(Duration::from_millis(5));
        }
        waits
    });
    // The idle supervisor and the cold-tier pass, as `main` runs them but on a faster clock.
    let idle = run.clone();
    tokio::spawn(async move {
        let mut idle_since: HashMap<String, i64> = HashMap::new();
        let step = Duration::from_secs(5);
        while !idle.over.load(Ordering::Relaxed) {
            tokio::time::sleep(step).await;
            let due_at = now_ms() - crate::boxd::IDLE_MS;
            let due: Vec<String> = idle_since
                .iter()
                .filter(|(_, t)| **t <= due_at)
                .map(|(u, _)| u.clone())
                .collect();
            crate::boxd::idle_tick(&idle.state, &mut idle_since).await;
            for user in due {
                if idle.state.box_state(&user).await.0 == BoxState::Asleep {
                    idle.stats.idle_sleeps.fetch_add(1, Ordering::Relaxed);
                }
            }
            let skipped = step.as_millis() as i64 * (idle.k.idle_speedup - 1);
            idle_since.values_mut().for_each(|t| *t -= skipped);
        }
    });
    let tier = run.clone();
    tokio::spawn(async move {
        while !tier.over.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_secs(30)).await;
            crate::boxd::tier_tick(&tier.state).await;
        }
    });

    let (cpu_before, _) = cpu_and_rss();
    crate::db::held::take();
    let slept_before: u64 = fakes.iter().map(|f| f.sleeps.load(Ordering::Relaxed)).sum();
    let mut arrivals = tokio::time::interval(Duration::from_secs_f64(60.0 / k.wakes_per_min));
    let mut asked = 0u64;
    let mut warm_seen = Vec::new();
    let mut burst = k.burst;
    while started.elapsed() < steady {
        arrivals.tick().await;
        // The burst is the first arrival; they all think for as long, so all send at once.
        for _ in 0..std::mem::take(&mut burst).max(1) {
            // A sleeping user nobody is already waiting on.
            let mut pick = None;
            for _ in 0..200 {
                let u = run.users[run.rng.lock().unwrap().below(run.users.len())].clone();
                let free = !run.busy.lock().unwrap().contains(&u.id);
                if free && state.box_state(&u.id).await.0 == BoxState::Asleep {
                    pick = Some(u);
                    break;
                }
            }
            let Some(u) = pick else {
                continue;
            };
            asked += 1;
            run.busy.lock().unwrap().insert(u.id.clone());
            let length = run.rng.lock().unwrap().around(session);
            tokio::spawn(run.clone().arrival(u, length));
        }
        let mut up = 0;
        for u in &run.users {
            let st = state.box_state(&u.id).await.0;
            up += usize::from(matches!(st, BoxState::Awake | BoxState::Waking));
        }
        warm_seen.push(up as f64);
    }
    // The last arrivals get their answer before the count is taken.
    tokio::time::sleep(Duration::from_millis(k.think_ms + 5_000)).await;
    let wall = started.elapsed().as_secs_f64();
    let (cpu_after, rss_mb) = cpu_and_rss();
    let held = crate::db::held::take();
    let fds = open_files();
    let live = sockets.live.load(Ordering::Relaxed);
    run.over.store(true, Ordering::Relaxed);
    let mut lock_ms = probe.join().unwrap();

    let slept: u64 = fakes
        .iter()
        .map(|f| f.sleeps.load(Ordering::Relaxed))
        .sum::<u64>()
        - slept_before;
    let idle_sleeps = run.stats.idle_sleeps.load(Ordering::Relaxed);
    let archived: u64 = fakes
        .iter()
        .map(|f| f.archives.load(Ordering::Relaxed))
        .sum();
    let mut wakes = std::mem::take(&mut *run.stats.wakes.lock().unwrap());
    let mut parts = std::mem::take(&mut *run.stats.parts.lock().unwrap());
    let mut waited = std::mem::take(&mut *run.stats.waited.lock().unwrap());
    let ahead = waited.iter().filter(|w| **w == 0.0).count();
    let most_up = fakes.iter().map(|f| f.most_up.load(Ordering::Relaxed));
    let most_up = most_up.max().unwrap_or(0);
    let mut acks = std::mem::take(&mut *run.stats.acks.lock().unwrap());
    let lost = run.stats.wakes_lost.load(Ordering::Relaxed);
    let acks_lost = run.stats.acks_lost.load(Ordering::Relaxed);
    let errors = sockets.errors.load(Ordering::Relaxed);
    let waitlist_after = state.db.waitlist().unwrap().len();

    println!("\n  steady state, {wall:.0} s:");
    println!(
        "    wakes asked {asked}, lost {lost}; boxes awake at an arrival: mean {:.0}, most {:.0}",
        warm_seen.iter().sum::<f64>() / warm_seen.len().max(1) as f64,
        warm_seen.iter().copied().fold(0.0, f64::max)
    );
    for kind in [Kind::Clean, Kind::Ckpt, Kind::Cold] {
        let s = wakes.entry(kind).or_default();
        println!(
            "    wake {kind:?}: n {}, p50 {:.3} s, p95 {:.3} s, p99 {:.3} s, max {:.3} s",
            s.len(),
            pct(s, 50.0),
            pct(s, 95.0),
            pct(s, 99.0),
            pct(s, 100.0)
        );
    }
    for (part, name) in parts
        .iter_mut()
        .zip(["wait for a slot", "daemon wake", "box dial"])
    {
        println!(
            "      {name}: p50 {:.3} s, p95 {:.3} s, p99 {:.3} s, max {:.3} s",
            pct(part, 50.0),
            pct(part, 95.0),
            pct(part, 99.0),
            pct(part, 100.0)
        );
    }
    println!(
        "    the sender's wait for the box: p50 {:.3} s, p95 {:.3} s, p99 {:.3} s, max {:.3} s; {ahead} of {} were up before the message",
        pct(&mut waited, 50.0),
        pct(&mut waited, 95.0),
        pct(&mut waited, 99.0),
        pct(&mut waited, 100.0),
        waited.len()
    );
    println!("    most boxes up on a node {most_up} of {}", k.node_cap);
    println!("    sleeps {slept}: {idle_sleeps} idle, {} at the cap; archived {} ({archived_before} before the clock started)",
        slept.saturating_sub(idle_sleeps), archived);
    println!("    PUSH to ACK: n {}, p50 {:.2} ms, p95 {:.2} ms, p99 {:.2} ms, max {:.1} ms; unacked {acks_lost}",
        acks.len(), pct(&mut acks, 50.0) * 1e3, pct(&mut acks, 95.0) * 1e3, pct(&mut acks, 99.0) * 1e3, pct(&mut acks, 100.0) * 1e3);
    println!(
        "    write lock wait: n {}, p50 {:.3} ms, p99 {:.3} ms, max {:.2} ms",
        lock_ms.len(),
        pct(&mut lock_ms, 50.0),
        pct(&mut lock_ms, 99.0),
        pct(&mut lock_ms, 100.0)
    );
    for (site, (n, ms, longest)) in held.iter().take(8) {
        println!(
            "      held by {site}: {n} times, {:.1} ms a second, {:.3} ms each, {longest:.2} ms at most",
            ms / wall,
            ms / *n as f64
        );
    }
    println!(
        "    sockets open {live} of {}, socket errors {errors}, descriptors {fds}",
        k.sockets
    );
    println!(
        "    process (relay + clients + fake daemons): {:.2} cores, {rss_mb:.0} MB resident at most",
        (cpu_after - cpu_before) / wall
    );

    println!("\n  gate:");
    let clean = wakes.entry(Kind::Clean).or_default();
    let clean_p95 = pct(clean, 95.0);
    let ckpt = wakes.entry(Kind::Ckpt).or_default();
    let (ckpt_n, ckpt_p95) = (ckpt.len(), pct(ckpt, 95.0));
    let lock_p99 = pct(&mut lock_ms, 99.0);
    let passed = [
        verdict(
            "waitlist == 0, every user has a box",
            format!(
                "{} waitlisted, {} refused",
                waitlist + waitlist_after,
                refusals.len()
            ),
            waitlist + waitlist_after == 0 && refusals.is_empty() && cap.has_room,
        ),
        verdict(
            "no node over its cap at any instant",
            format!("{most_up} of {}", k.node_cap),
            most_up <= k.node_cap,
        ),
        verdict(
            "p95 Asleep-Clean wake < 1.0 s",
            format!("{clean_p95:.3} s"),
            clean_p95 < 1.0,
        ),
        // A run too short to see a checkpointed box wake says so and does not fail on it.
        verdict(
            "p95 Asleep-Checkpointed wake < 2.0 s",
            format!("{ckpt_p95:.3} s (n {ckpt_n})"),
            ckpt_n == 0 || ckpt_p95 < 2.0,
        ),
        verdict(
            "SQLite write lock wait p99 < 10 ms",
            format!("{lock_p99:.3} ms"),
            lock_p99 < 10.0,
        ),
        verdict(
            "every socket held, every wake answered",
            format!("{live} sockets, {lost} wakes lost, {acks_lost} rows unacked"),
            live == k.sockets && lost == 0 && acks_lost == 0 && errors == 0,
        ),
    ];
    drop(passive);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        passed.iter().all(|p| *p),
        "the load gate did not pass; see the lines marked FAIL"
    );
}
