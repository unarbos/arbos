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
    // The worker's report lands after root's dispatch turn has closed, so
    // it opens the done turn that archives the worker. An instant report
    // folds into the running turn as a say, nothing archives, and the
    // 30 s wait below runs out (the fold-race family — #585, #606, #630,
    // #637; red on main at d644bdf0).
    "{\"content\":\"the codeword is xylophone\",\"delay_ms\":2500}\n",
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

/// iPhone loop, cycle 47: the sheet shows a worker by its *name* ("Run
/// J152618 verification command"), the app asked `history` under it, and
/// the kernel answered `total: 0` for eight of eight finished workers
/// whose records were on disk under their ids — a name is not a valid
/// id, so nothing was looked up. A name resolves to its worker, live or
/// archived, and the answer says which id it was; a name nobody has says
/// `unknown` rather than reading as an empty record.
#[test]
fn history_by_the_workers_name_finds_its_archived_record_and_an_unknown_name_says_so() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"one worker\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"Run J152618 verification command\",\"task\":\"say the codeword\"}}]}\n",
        // Pinned to the worker's id (the name's slug): unpinned, root's
        // step after the spawn took this line first on a loaded runner.
        "{\"agent\":\"run-j152618-verification-command\",\"content\":\"the codeword is marimba\",\"delay_ms\":2500}\n",
        "{\"agent\":\"root\",\"content\":\"the worker says marimba\"}\n",
    );
    let mut k = start_kernel_replay("history-by-name", replies);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "get the codeword"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let archive = k.place.join(".arbos/archive/agents");
    assert!(
        wait_for(Duration::from_secs(30), || std::fs::read_dir(&archive)
            .map(|rd| rd.count() > 0)
            .unwrap_or(false)),
        "the worker was archived"
    );
    let id = std::fs::read_dir(&archive)
        .unwrap()
        .flatten()
        .next()
        .unwrap()
        .file_name()
        .to_string_lossy()
        .to_string();
    assert_ne!(
        id, "Run J152618 verification command",
        "the id is not the name"
    );

    let mut b = Attach::connect(&k.url);
    let _ = b.wait(Duration::from_secs(5), |f| f["type"] == "snapshot");
    // The phone's request, under the name its sheet shows.
    let name = "Run J152618 verification command";
    b.send(serde_json::json!({"type":"history","agent":name,"since":0,"limit":50}));
    let mut replayed = Vec::new();
    let end = loop {
        let f = b
            .wait(Duration::from_secs(10), |f| {
                f["agent"] == name && (f["type"] == "replayed" || f["type"] == "history_end")
            })
            .expect("a reply to the history request");
        if f["type"] == "history_end" {
            break f;
        }
        replayed.push(f["event"].clone());
    };
    assert!(
        end["total"].as_u64().unwrap_or(0) > 0,
        "the record, not an empty page: {end}"
    );
    assert_eq!(end["archived"], true, "{end}");
    assert_eq!(end["id"], id, "the id it resolved to: {end}");
    assert_eq!(
        end["path"],
        format!("archive/agents/{id}/transcript.jsonl"),
        "{end}"
    );
    assert!(end.get("unknown").is_none(), "{end}");
    assert!(
        replayed
            .iter()
            .any(|e| e["kind"] == "assistant" && e["text"] == "the codeword is marimba"),
        "the worker's words are on the page: {replayed:#?}"
    );

    // A name nobody has: an empty page that says so.
    b.send(
        serde_json::json!({"type":"history","agent":"Nobody by this name","since":0,"limit":50}),
    );
    let end = b
        .wait(Duration::from_secs(10), |f| {
            f["type"] == "history_end" && f["agent"] == "Nobody by this name"
        })
        .expect("history_end");
    assert_eq!(end["total"], 0, "{end}");
    assert_eq!(
        end["unknown"], true,
        "an empty page is said to be no record at all: {end}"
    );
    let _ = k.child.kill();
}
