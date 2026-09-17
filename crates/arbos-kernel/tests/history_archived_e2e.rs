//! M-27 (iPhone loop): `history` for a worker that has finished answered
//! `history_end {total: 0}`, so the phone's worker chat said "Nothing on
//! record yet" under "Done" — the record had moved to
//! `archive/agents/<id>/` with the folder. Now the lines come from there,
//! and `history_end` says `archived: true` with the transcript's path.

mod common;

use common::{Attach, start_kernel_replay};
use std::path::Path;
use std::time::{Duration, Instant};

const REPLIES: &str = concat!(
    "{\"agent\":\"root\",\"content\":\"one worker\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"w1\",\"task\":\"say the codeword\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"started\"}\n",
    "{\"content\":\"the codeword is xylophone\"}\n",
    "{\"agent\":\"root\",\"content\":\"noted\"}\n",
);

fn wait_for(timeout: Duration, mut ok: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    ok()
}

#[test]
fn history_for_an_archived_worker_replays_its_record_and_says_so() {
    let mut k = start_kernel_replay("history-archived", REPLIES);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "get the codeword"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let archived: &Path = &k.place.join(".arbos/archive/agents/w1/transcript.jsonl");
    assert!(
        wait_for(Duration::from_secs(30), || archived.exists()),
        "w1 archived once root read its done"
    );
    assert!(!k.place.join(".arbos/agents/w1").exists());

    // The phone's request, verbatim.
    let mut b = Attach::connect(&k.url);
    let _ = b.wait(Duration::from_secs(5), |f| f["type"] == "snapshot");
    b.send(serde_json::json!({"type":"history","agent":"w1","since":0,"limit":50}));
    let mut replayed = Vec::new();
    let end = loop {
        let f = b
            .wait(Duration::from_secs(10), |f| {
                f["agent"] == "w1" && (f["type"] == "replayed" || f["type"] == "history_end")
            })
            .expect("a reply to the history request");
        if f["type"] == "history_end" {
            break f;
        }
        replayed.push(f["event"].clone());
    };
    assert!(
        end["total"].as_u64().unwrap_or(0) > 0,
        "not an empty page: {end}"
    );
    assert_eq!(end["archived"], true, "{end}");
    assert_eq!(end["path"], "archive/agents/w1/transcript.jsonl", "{end}");
    assert!(
        replayed
            .iter()
            .any(|e| e["kind"] == "assistant" && e["text"] == "the codeword is xylophone"),
        "the worker's words are on the page: {replayed:#?}"
    );
    let _ = k.child.kill();
}

#[test]
fn history_for_a_live_agent_is_not_marked_archived() {
    let mut k = start_kernel_replay("history-live", "{\"agent\":\"root\",\"content\":\"hi\"}\n");
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot");
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "hello"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    a.send(serde_json::json!({"type":"history","agent":"root","since":0,"limit":50}));
    let end = a
        .wait(Duration::from_secs(10), |f| {
            f["type"] == "history_end" && f["agent"] == "root"
        })
        .unwrap();
    assert!(end.get("archived").is_none(), "absent when live: {end}");
    assert!(end.get("path").is_none(), "{end}");
    let _ = k.child.kill();
}
