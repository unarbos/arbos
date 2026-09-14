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

/// The project page's row for a worker (target `agents/<id>`) cannot say
/// "worker running" once the folder is in the archive: the archive step
/// checks it with the worker's last words and moves the link along. The
/// worker's name reads as words even when the model passed a slug.
#[test]
fn archiving_a_worker_retires_its_row_on_the_project_page() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"one worker\",\"calls\":[{\"name\":\"plan\",\"arguments\":{\"op\":\"add\",\"section\":\"Work\",\"text\":\"[Codeword](agents/code-word) — worker running\"}},{\"name\":\"spawn\",\"arguments\":{\"name\":\"code-word\",\"task\":\"say the codeword\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"started\"}\n",
        "{\"content\":\"the codeword is xylophone\"}\n",
        "{\"agent\":\"root\",\"content\":\"noted\"}\n",
    );
    let mut k = start_kernel_replay("archive-page-row", replies);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "get the codeword"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let page = k.place.join(".arbos/notes.md");
    assert!(
        wait_for(Duration::from_secs(10), || std::fs::read_to_string(&page)
            .unwrap_or_default()
            .contains("- [ ] [Codeword](agents/code-word) — worker running")),
        "root's row while the worker runs: {}",
        std::fs::read_to_string(&page).unwrap_or_default()
    );
    let agent_md = std::fs::read_to_string(k.place.join(".arbos/agents/code-word/agent.md"))
        .unwrap_or_default();
    assert!(
        agent_md.contains("Code word"),
        "the slug the model passed reads as words: {agent_md}"
    );

    let archived = k.place.join(".arbos/archive/agents/code-word");
    assert!(
        wait_for(Duration::from_secs(30), || archived
            .join("transcript.jsonl")
            .exists()),
        "the worker moves to the archive once root has read its done"
    );
    let text = std::fs::read_to_string(&page).unwrap_or_default();
    assert!(
        text.contains(
            "- [x] [Codeword](archive/agents/code-word) — worker finished: the codeword is xylophone"
        ),
        "the row is checked, says what the worker said, and links the archive: {text}"
    );
    assert_eq!(
        text.matches("[Codeword]").count(),
        1,
        "one row, not two: {text}"
    );
    let _ = k.child.kill();
}

/// A `wait=true` worker's report is the spawn result; no done file
/// follows, so the archive must come from the turn end instead. The
/// window hears about the move as `changed` frames and a fresh `tree`.
#[test]
fn a_waited_worker_is_archived_once_its_parents_turn_ends() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"asking\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"oracle\",\"task\":\"say the codeword\",\"wait\":true}}]}\n",
        "{\"content\":\"the codeword is xylophone\"}\n",
        "{\"agent\":\"root\",\"content\":\"got it\"}\n",
    );
    let mut k = start_kernel_replay("archive-waited", replies);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "ask the oracle"}));
    // The frames the window can act on, in order: the folder leaves
    // agents/, appears under archive/, and the tree no longer lists it.
    let removed = a.wait(Duration::from_secs(40), |f| {
        f["type"] == "changed" && f["path"] == "agents/oracle" && f["kind"] == "removed"
    });
    assert!(
        removed.is_some(),
        "a changed frame says the folder left agents/"
    );
    let tree = a
        .wait(Duration::from_secs(10), |f| f["type"] == "tree")
        .expect("a tree frame follows the archive");
    let ids: Vec<&str> = tree["tree"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n["id"].as_str())
        .collect();
    assert_eq!(ids, vec!["root"], "{ids:?}");
    assert!(
        k.place
            .join(".arbos/archive/agents/oracle/transcript.jsonl")
            .exists()
    );
    assert!(!k.place.join(".arbos/agents/oracle").exists());
    // Root's turn had ended first: the report was its tool result.
    let root = transcript(&k.place, "root");
    let spawn = root
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "spawn")
        .expect("spawn record");
    assert!(
        spawn["body"].as_str().unwrap_or("").contains("xylophone"),
        "{spawn:#?}"
    );
    assert!(root.iter().any(|e| e["kind"] == "turn_complete"));
    let _ = k.child.kill();
}
