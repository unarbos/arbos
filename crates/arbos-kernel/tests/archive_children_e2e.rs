//! Audit item 23 / fix 12: finished workers leave `.arbos/agents/` for
//! `.arbos/archive/agents/` once the parent has read their done message —
//! on by default now that a window closes a chat whose agent is gone
//! (#129/#138) instead of reconnecting to it. Their history still reads:
//! `grep scope=history` walks the archive too, and the path the done
//! message named (`.arbos/agents/<id>/…`) resolves into the archive.

mod common;

use common::{Attach, start_kernel_replay, start_kernel_replay_prepared};
use std::path::Path;
use std::time::{Duration, Instant};

fn transcript(place: &Path, agent: &str) -> Vec<serde_json::Value> {
    std::fs::read_to_string(
        place
            .join(".arbos/agents")
            .join(agent)
            .join("transcript.jsonl"),
    )
    .unwrap_or_default()
    .lines()
    .filter_map(|l| serde_json::from_str(l).ok())
    .collect()
}

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

const REPLIES: &str = concat!(
    "{\"agent\":\"root\",\"content\":\"one worker\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"w1\",\"task\":\"say the codeword\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"started\"}\n",
    "{\"content\":\"the codeword is xylophone\"}\n",
    "{\"agent\":\"root\",\"content\":\"checking\",\"calls\":[{\"name\":\"read\",\"arguments\":{\"path\":\".arbos/agents/w1/transcript.jsonl\"}},{\"name\":\"grep\",\"arguments\":{\"pattern\":\"xylophone\",\"scope\":\"history\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"noted\"}\n",
);

#[test]
fn a_finished_worker_is_archived_by_default_and_its_history_still_reads() {
    let mut k = start_kernel_replay("archive-default", REPLIES);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "get the codeword"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));

    let archived = k.place.join(".arbos/archive/agents/w1");
    assert!(
        wait_for(Duration::from_secs(30), || archived
            .join("transcript.jsonl")
            .exists()),
        "w1 must move to the archive once root has read its done"
    );
    assert!(!k.place.join(".arbos/agents/w1").exists());
    // The tree no longer lists it.
    let mut b = Attach::connect(&k.url);
    let snap = b
        .wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
        .unwrap();
    let ids: Vec<&str> = snap["tree"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n["id"].as_str())
        .collect();
    assert_eq!(ids, vec!["root"], "{ids:?}");

    // Root's done turn read the old path and grepped history: both hit.
    assert!(wait_for(Duration::from_secs(30), || {
        transcript(&k.place, "root")
            .iter()
            .filter(|e| e["kind"] == "turn_complete")
            .count()
            >= 2
    }));
    let root = transcript(&k.place, "root");
    let read = root
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "read")
        .expect("read tool line");
    assert!(read.get("error").is_none(), "{read:#?}");
    assert!(
        read["body"].as_str().unwrap_or("").contains("xylophone"),
        "{read:#?}"
    );
    let grep = root
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "grep")
        .expect("grep tool line");
    assert!(
        grep["body"].as_str().unwrap_or("").contains("w1 · line"),
        "{grep:#?}"
    );
    let _ = k.child.kill();
}

#[test]
fn archive_children_false_keeps_the_folder_where_it_was() {
    let mut k = start_kernel_replay_prepared("archive-off", REPLIES, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\nname = \"archive-off\"\n\n[root]\nrole = \"coordinator\"\narchive_children = false\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "get the codeword"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert!(wait_for(Duration::from_secs(30), || {
        transcript(&k.place, "root")
            .iter()
            .filter(|e| e["kind"] == "turn_complete")
            .count()
            >= 2
    }));
    assert!(k.place.join(".arbos/agents/w1/transcript.jsonl").exists());
    assert!(!k.place.join(".arbos/archive/agents/w1").exists());
    let _ = k.child.kill();
}
