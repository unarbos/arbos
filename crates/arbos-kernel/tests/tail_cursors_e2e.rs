//! Desktop F-180: on half the launches of a place with history, every
//! chat doubled. The serve loop's tail cursors started at line one, so
//! the first tick broadcast every agent's whole record as live `event`
//! frames, and a window that attached during that tick drew the record
//! twice. The cursors now stand at each transcript's end at boot: a
//! client attaching early hears nothing of the record as live frames
//! (the record is `history`'s), only what is appended after boot. An
//! agent born after boot — a worker spawned live — still has its first
//! lines broadcast from line one.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::time::{Duration, Instant};

fn seeded(n: usize, text: &str) -> String {
    let mut out = String::new();
    for i in 1..=n {
        out.push_str(&format!(
            "{{\"kind\":\"assistant\",\"text\":\"{text} {i}\",\"ts\":{}}}\n",
            1_789_500_000_000i64 + i as i64
        ));
    }
    out
}

fn agent(place: &std::path::Path, id: &str, lines: usize) {
    let dir = place.join(".arbos/agents").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("agent.md"),
        format!("---\nname: {id}\nmodel: inherit\n---\n"),
    )
    .unwrap();
    std::fs::write(dir.join("transcript.jsonl"), seeded(lines, id)).unwrap();
}

#[test]
fn an_early_client_hears_nothing_of_the_record_as_live_frames() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"Still here.\"}\n",
        "{\"agent\":\"root\",\"content\":\"one worker\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"fresh\",\"task\":\"say hi\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"started\"}\n",
        // The worker lives long enough for the tick to see it (an instant
        // one is archived before the next tick lists it).
        "{\"agent\":\"fresh\",\"content\":\"pausing\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 2\",\"description\":\"Wait a moment\"}}]}\n",
        "{\"agent\":\"fresh\",\"content\":\"Hi.\"}\n",
    );
    let k = start_kernel_replay_prepared("tail-cursors", replies, "", |place| {
        agent(place, "root", 400);
        agent(place, "old-worker", 300);
    });
    // Attach at once, as the desktop does the moment kernel.json appears.
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    // Through several ticks: not one line of either record arrives live.
    let leaked = a.wait(Duration::from_millis(2500), |f| {
        f["type"] == "event"
            && (f["agent"] == "old-worker"
                || (f["agent"] == "root" && f["event"]["seq"].as_u64().unwrap_or(0) <= 400))
    });
    assert!(
        leaked.is_none(),
        "a recorded line came as a live frame: {leaked:?}"
    );

    // What is appended after boot is live, numbered after the record.
    a.send(
        serde_json::json!({"type":"user","agent":"root","text":"are you there?","attachments":[]}),
    );
    let live = a
        .wait(Duration::from_secs(20), |f| {
            f["type"] == "event"
                && f["agent"] == "root"
                && f["event"]["kind"] == "assistant"
                && f["event"]["text"] == "Still here."
        })
        .expect("the live line");
    assert!(
        live["event"]["seq"].as_u64().unwrap() > 400,
        "numbered after the record: {live}"
    );
    // (The `turn idle` frame came before the tick's event and was drained
    // with it; the next prompt queues behind the turn either way.)

    // A worker born now: its first line is live, from line one.
    a.send(serde_json::json!({"type":"user","agent":"root","text":"spawn one","attachments":[]}));
    let deadline = Instant::now() + Duration::from_secs(30);
    let first = loop {
        // A recorded line (with its `seq`), not a live emit (a tool
        // starting, streamed text) which carries none.
        let f = a.wait(Duration::from_secs(5), |f| {
            f["type"] == "event" && f["agent"] == "fresh" && f["event"]["seq"].is_u64()
        });
        if let Some(f) = f {
            break f;
        }
        assert!(
            Instant::now() < deadline,
            "the worker's first line never came live"
        );
    };
    assert_eq!(first["event"]["seq"].as_u64().unwrap(), 1, "{first}");
    assert_eq!(first["event"]["kind"], "wake", "{first}");
}
