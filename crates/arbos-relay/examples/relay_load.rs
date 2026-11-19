//! Load driver for a LOCAL relay: `USERS` test accounts, three room sockets each (two DMs and a
//! group of `GROUP`), presence every 15 s, `RATE` pushes of `BYTES`-long messages a second, then
//! `STORMS` reconnect storms (every socket drops and redials at once; one in three catches up
//! from seq 0). Prints p50/p99 of dial-to-caught-up and push-to-ack per phase.
//!
//!   arbos-relay --listen 127.0.0.1:7711 --data /tmp/x --admin-token adm_load &
//!   RELAY=127.0.0.1:7711 ADMIN=adm_load cargo run --release -p arbos-relay --example relay_load
//!
//! Never point it at a relay with real users: it creates accounts and chats through the admin API.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use arbos_core::doc::{ChatDoc, UserMessageOpts};
use arbos_core::wire::{Frame, FrameType};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message;

fn env<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(d)
}

#[derive(Default)]
struct Stats {
    connect_ms: Vec<f64>,
    ack_ms: Vec<f64>,
    errors: HashMap<String, u64>,
}

type Shared = Arc<Mutex<Stats>>;

fn report(phase: &str, stats: &Shared) {
    let s = std::mem::take(&mut *stats.lock().unwrap());
    let pct = |mut v: Vec<f64>| {
        v.sort_by(f64::total_cmp);
        let at = |p: f64| {
            *v.get((v.len() as f64 * p) as usize)
                .or(v.last())
                .unwrap_or(&0.0)
        };
        let (p50, p99, max) = (at(0.5), at(0.99), at(1.0));
        format!("n={} p50={p50:.1} p99={p99:.1} max={max:.1}", v.len())
    };
    let (connect, ack) = (pct(s.connect_ms), pct(s.ack_ms));
    println!(
        "{phase}: connect_ms[{connect}] ack_ms[{ack}] errors={:?}",
        s.errors
    );
}

fn send(ty: FrameType, header: Value, payload: Vec<u8>) -> Message {
    Message::Binary(Frame::new(ty, header, payload).encode())
}

/// One room socket of one device, redialled every time `generation` moves.
async fn socket(
    url: String,
    room: String,
    n: u64,
    push_every: Duration,
    stats: Shared,
    mut generation: watch::Receiver<u64>,
) {
    let device = format!("dev:load{n}");
    let doc = ChatDoc::new(Some(n + 1), false);
    let text = "m".repeat(env("BYTES", 40));
    let err = |stats: &Shared, code: &str| {
        *stats.lock().unwrap().errors.entry(code.into()).or_default() += 1
    };
    let (mut cursor, mut sent) = (0i64, 0u64);
    loop {
        // A third of the sockets come back with nothing cached: the whole history again.
        if *generation.borrow_and_update() > 0 && n % 3 == 0 {
            cursor = 0;
        }
        let dialled = Instant::now();
        let Ok((ws, _)) = tokio_tungstenite::connect_async(&url).await else {
            err(&stats, "dial");
            tokio::time::sleep(Duration::from_millis(250 + n % 500)).await;
            continue;
        };
        let (mut tx, mut rx) = ws.split();
        let hello = json!({ "cursor": cursor, "device": device, "room": room, "proto": arbos_core::PROTO_VERSION });
        let _ = tx.send(send(FrameType::Hello, hello, vec![])).await;
        let mut presence = tokio::time::interval(Duration::from_secs(15));
        // Spread the first push over the period so the sockets do not fire together.
        let first = push_every.mul_f64((n * 7919 % 997) as f64 / 997.0);
        let mut push = tokio::time::interval_at(tokio::time::Instant::now() + first, push_every);
        let mut in_flight: HashMap<String, Instant> = HashMap::new();
        let mut live = false;
        loop {
            tokio::select! {
                m = rx.next() => {
                    let Some(Ok(Message::Binary(b))) = m else { err(&stats, "closed"); break };
                    let Ok(Some(f)) = Frame::decode(b) else { continue };
                    match f.ty {
                        FrameType::State => {
                            let _ = tx.send(send(FrameType::RowsReq, json!({ "after": cursor, "excludeOwn": false }), vec![])).await;
                        }
                        FrameType::Row => cursor = cursor.max(f.header["seq"].as_i64().unwrap_or(0)),
                        FrameType::RowsDone if !live => {
                            live = true;
                            stats.lock().unwrap().connect_ms.push(dialled.elapsed().as_secs_f64() * 1e3);
                        }
                        FrameType::Ack => {
                            if let Some(t) = in_flight.remove(f.header["batchId"].as_str().unwrap_or("")) {
                                stats.lock().unwrap().ack_ms.push(t.elapsed().as_secs_f64() * 1e3);
                            }
                        }
                        FrameType::Error => err(&stats, f.header["code"].as_str().unwrap_or("error")),
                        _ => {}
                    }
                }
                _ = presence.tick() => {
                    let _ = tx.send(send(FrameType::Presence, json!({ "device": device }), vec![])).await;
                }
                _ = push.tick(), if live => {
                    sent += 1;
                    let (base, batch) = (doc.oplog_vv(), format!("b{n}-{sent}"));
                    let _ = doc.append_user_message(&format!("m{n}-{sent}"), &text, &device, &UserMessageOpts::default());
                    in_flight.insert(batch.clone(), Instant::now());
                    let _ = tx.send(send(FrameType::Push, json!({ "batchId": batch }), doc.export_updates_since(&base))).await;
                }
                _ = generation.changed() => break,
            }
        }
    }
}

#[tokio::main]
async fn main() {
    let relay: String = env("RELAY", "127.0.0.1:7711".to_owned());
    let admin: String = env("ADMIN", "adm_load".to_owned());
    let (users, group): (usize, usize) = (env("USERS", 1000), env("GROUP", 20));
    let (rate, secs, storms): (f64, u64, u64) =
        (env("RATE", 20.0), env("SECS", 60), env("STORMS", 1));
    let http = reqwest::Client::new();
    let post = |path: &str, token: &str, body: Value| {
        let req = http
            .post(format!("http://{relay}{path}"))
            .bearer_auth(token)
            .json(&body);
        async move { req.send().await.unwrap().json::<Value>().await.unwrap() }
    };
    let run = arbos_core::ids::ulid()[20..].to_ascii_lowercase();
    let handle = |i: usize| format!("l{run}-{i}");
    let mut tokens = Vec::new();
    for i in 0..users {
        let v = post("/admin/users/test", &admin, json!({ "handle": handle(i) })).await;
        tokens.push(v["access_token"].as_str().expect("test user").to_owned());
    }
    // rooms[i]: the chats user i holds a socket on.
    let mut rooms: Vec<Vec<String>> = vec![Vec::new(); users];
    for i in 0..users {
        let peer = (i + 1) % users;
        let v = post("/chats/dm", &tokens[i], json!({ "handle": handle(peer) })).await;
        let id = v["id"].as_str().expect("dm").to_owned();
        rooms[i].push(id.clone());
        rooms[peer].push(id);
    }
    for g in (0..users).step_by(group) {
        let members: Vec<String> = (g + 1..(g + group).min(users)).map(handle).collect();
        let v = post(
            "/chats",
            &tokens[g],
            json!({ "kind": "group", "members": members, "title": "load" }),
        )
        .await;
        let id = v["id"].as_str().expect("group").to_owned();
        (g..(g + group).min(users)).for_each(|i| rooms[i].push(id.clone()));
    }
    let sockets: usize = rooms.iter().map(Vec::len).sum();
    println!("{users} users, {sockets} sockets, {rate} pushes/s");
    let stats = Shared::default();
    let (storm, generation) = watch::channel(0u64);
    let push_every = Duration::from_secs_f64(sockets as f64 / rate);
    let mut n = 0u64;
    for (i, chats) in rooms.iter().enumerate() {
        for chat in chats {
            let url = format!("ws://{relay}/room/people/{chat}?token={}", tokens[i]);
            tokio::spawn(socket(
                url,
                format!("people/{chat}"),
                n,
                push_every,
                stats.clone(),
                generation.clone(),
            ));
            n += 1;
            if n % 200 == 0 {
                tokio::time::sleep(Duration::from_millis(100)).await; // ramp: 2,000 dials/s
            }
        }
    }
    tokio::time::sleep(Duration::from_secs(5)).await;
    report("ramp", &stats);
    tokio::time::sleep(Duration::from_secs(secs)).await;
    report("steady", &stats);
    for s in 1..=storms {
        storm.send(s).unwrap();
        tokio::time::sleep(Duration::from_secs(secs.min(30))).await;
        report("storm", &stats);
    }
}
