//! Specific local observations of actual approval/creation ownership APIs. No external effects.
extern crate self as arbos_core;
mod generated;
include!("restart_path.rs");
pub use generated::ids;

use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::future::Future;
use std::io::{self, BufRead, Write};
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::task::{Context, Poll, Waker};

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Delivery {
        id: u32,
        initial: String,
        events: Vec<u8>,
    },
    Approval {
        id: u32,
        legacy_expired: bool,
        events: Vec<u8>,
    },
    Creation {
        id: u32,
        events: Vec<u8>,
    },
}
struct Actor {
    user: &'static str,
    holding: Arc<AtomicBool>,
    future: Pin<Box<dyn Future<Output = ()>>>,
}
fn poll_actors(actors: &mut BTreeMap<u8, Actor>) {
    let mut context = Context::from_waker(Waker::noop());
    // Finite deterministic poll schedules, not a model of the mutex or a fairness proof.
    for _ in 0..=actors.len() {
        for actor in actors.values_mut() {
            assert!(matches!(
                actor.future.as_mut().poll(&mut context),
                Poll::Pending
            ));
        }
    }
}
fn create_actor(signal: Arc<generated::creation::RestartSignal>, user: &'static str) -> Actor {
    let holding = Arc::new(AtomicBool::new(false));
    let observed = holding.clone();
    let future = Box::pin(async move {
        let mut permit = signal.admit_creation(user).await;
        let _guard = generated::creation::CreatingGuard::new(&mut permit);
        observed.store(true, Ordering::SeqCst);
        std::future::pending::<()>().await;
    });
    Actor {
        user,
        holding,
        future,
    }
}
fn creation(events: &[u8]) -> Value {
    let signal = Arc::new(generated::creation::RestartSignal::default());
    let mut actors = BTreeMap::<u8, Actor>::new();
    let mut steps = Vec::new();
    for &event in events {
        let action = match event {
            0 | 1 | 2 | 7 | 9 => {
                let (id, user) = match event {
                    0 => (0, "a"),
                    1 => (1, "a"),
                    2 => (2, "b"),
                    7 => (3, "a"),
                    _ => (4, "b"),
                };
                if let std::collections::btree_map::Entry::Vacant(entry) = actors.entry(id) {
                    entry.insert(create_actor(signal.clone(), user));
                    "Started"
                } else {
                    "Existing"
                }
            }
            3 | 4 | 5 | 8 | 10 => {
                let id = match event {
                    3 => 0,
                    4 => 1,
                    5 => 2,
                    8 => 3,
                    _ => 4,
                };
                if actors.remove(&id).is_some() {
                    "Dropped"
                } else {
                    "Absent"
                }
            }
            11 => {
                actors.clear();
                "DroppedAll"
            }
            _ => "Polled",
        };
        poll_actors(&mut actors);
        let mut markers = signal
            .creating
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        markers.sort();
        let observed=actors.iter().map(|(&id,actor)|json!({"id":id,"user":actor.user,"holding":actor.holding.load(Ordering::SeqCst)})).collect::<Vec<_>>();
        steps.push(json!({"action":action,"actors":observed,"markers":markers}));
    }
    drop(actors);
    let remaining = signal.creating.lock().unwrap().len();
    json!({"steps":steps,"remaining_markers":remaining})
}
fn approval(legacy_expired: bool, events: &[u8]) -> anyhow::Result<Value> {
    let store = generated::store::Store::fresh()?;
    store.insert_approval(
        "fixture",
        "synthetic-chat",
        "synthetic-message",
        "Synthetic card",
        "other",
        &json!({}),
    )?;
    if legacy_expired {
        store.with(|c| {
            c.execute(
                "UPDATE approvals SET state='expired',expires_at=1,decided_at=2 WHERE id='fixture'",
                [],
            )
            .map(|_| ())
        })?;
    }
    let mut steps = Vec::new();
    for &event in events {
        let result = match event {
            0 | 1 | 2 | 3 | 8 => {
                let (state, claim) = match event {
                    0 => ("approved", true),
                    1 => ("approved", false),
                    2 => ("rejected", false),
                    3 => ("cancelled", false),
                    _ => ("rejected", true),
                };
                match store.decide_approval(
                    "fixture",
                    state,
                    Some("synthetic owner"),
                    Some("synthetic reason"),
                    claim,
                ) {
                    Ok(true) => "True",
                    Ok(false) => "False",
                    Err(_) => "Error",
                }
            }
            4 => match store.consume_grant("fixture") {
                Ok(true) => "True",
                Ok(false) => "False",
                Err(_) => "Error",
            },
            5..=7 => {
                store.with(|c| c.execute_batch("DROP TRIGGER IF EXISTS fixture_claim_failure"))?;
                if event == 5 {
                    store.with(|c|c.execute_batch("CREATE TRIGGER fixture_claim_failure BEFORE UPDATE OF grant_consumed_at ON approvals BEGIN SELECT RAISE(ABORT,'synthetic claim failure'); END"))?;
                }
                if event == 6 {
                    store.with(|c|c.execute_batch("CREATE TRIGGER fixture_claim_failure BEFORE UPDATE OF grant_consumed_at ON approvals BEGIN SELECT RAISE(IGNORE); END"))?;
                }
                "Fixture"
            }
            _ => "Observed",
        };
        let row = store
            .approval("fixture")?
            .ok_or_else(|| anyhow::anyhow!("missing fixture row"))?;
        steps.push(json!({"result":result,"row":row}));
    }
    Ok(json!(steps))
}
fn reply(raw: &[u8]) -> Option<Value> {
    let request: Request = serde_json::from_slice(raw).ok()?;
    match request {
        Request::Delivery {
            id,
            initial,
            events,
        } => {
            if !matches!(initial.as_str(), "inline" | "polled" | "deferred")
                || events.len() > 64
                || events.iter().any(|&event| event > 3)
            {
                return None;
            }
            Some(json!({"id":id,"observations":generated::delivery::observe(&initial,&events)}))
        }
        Request::Approval {
            id,
            legacy_expired,
            events,
        } => {
            if events.len() > 64 || events.iter().any(|&event| event > 9) {
                return None;
            }
            approval(legacy_expired, &events)
                .ok()
                .map(|observations| json!({"id":id,"observations":observations}))
        }
        Request::Creation { id, events } => {
            if events.len() > 64 || events.iter().any(|&event| event > 12) {
                return None;
            }
            Some(json!({"id":id,"observations":creation(&events)}))
        }
    }
}
fn main() -> io::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut line = Vec::new();
    loop {
        line.clear();
        let mut oversized = false;
        loop {
            let bytes = input.fill_buf()?;
            if bytes.is_empty() {
                break;
            }
            let used = bytes
                .iter()
                .position(|&byte| byte == b'\n')
                .map_or(bytes.len(), |index| index + 1);
            let ended = bytes[used - 1] == b'\n';
            if line.len() + used <= 8192 && !oversized {
                line.extend_from_slice(&bytes[..used]);
            } else {
                oversized = true;
            }
            input.consume(used);
            if ended {
                break;
            }
        }
        if line.is_empty() && !oversized {
            break;
        }
        let response = if oversized { None } else { reply(&line) }
            .unwrap_or_else(|| json!({"id":null,"error":"Invalid trace"}));
        writeln!(output, "{response}")?;
        output.flush()?;
    }
    Ok(())
}
