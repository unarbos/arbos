//! The layout worker's standing pass (2026-09-14): a fork claimed the
//! original's workers as its own (and, forked from one of them, itself);
//! coordinator wording reached workers, which fanned out again; the
//! kernel's empty-reply line drew as a user bubble; `rewound` waited for
//! the file restore.

mod common;

use common::{Attach, restart_replay, start_kernel_replay, start_kernel_replay_prepared};
use std::path::Path;
use std::time::{Duration, Instant};

fn transcript(place: &Path, agent: &str) -> Vec<serde_json::Value> {
    let path = place
        .join(".arbos")
        .join("agents")
        .join(agent)
        .join("transcript.jsonl");
    std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn wait_transcript(
    place: &Path,
    agent: &str,
    timeout: Duration,
    pred: impl Fn(&[serde_json::Value]) -> bool,
) -> Vec<serde_json::Value> {
    let start = Instant::now();
    loop {
        let t = transcript(place, agent);
        if pred(&t) || start.elapsed() > timeout {
            return t;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

fn snapshot(k: &common::Kernel) -> Attach {
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a
}

/// Item 3: the model's empty reply gets a `nudge` line the window draws
/// dim, never a `user` line the user did not type.
#[test]
fn an_empty_reply_is_nudged_not_quoted_as_the_user() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"\"}\n",
        "{\"agent\":\"root\",\"content\":\"all done\"}\n",
    );
    let mut k = start_kernel_replay("standing-nudge", replies);
    let mut a = snapshot(&k);
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "go"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(20)));
    let t = wait_transcript(&k.place, "root", Duration::from_secs(10), |t| {
        t.iter()
            .any(|e| e["kind"] == "assistant" && e["text"] == "all done")
    });
    let users: Vec<String> = t
        .iter()
        .filter(|e| e["kind"] == "user")
        .map(|e| e["text"].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(
        users,
        vec!["go".to_string()],
        "no kernel line as the user: {users:?}"
    );
    let nudges: Vec<&serde_json::Value> = t.iter().filter(|e| e["kind"] == "nudge").collect();
    assert_eq!(nudges.len(), 1, "{t:?}");
    assert!(
        nudges[0]["text"]
            .as_str()
            .unwrap_or("")
            .contains("reply was empty")
    );
    // The model still saw it and went on.
    assert!(
        t.iter()
            .any(|e| e["kind"] == "assistant" && e["text"] == "all done")
    );
    let _ = k.child.kill();
}

/// Items 1 and 2: a child is saved as a worker (item 2); a fork of the
/// parent carries the spawn record without its child link, so the fork
/// claims no worker (item 1); an agent whose parent line loops is
/// reported top-level, and the parent's own replay keeps the claim only
/// while the child still calls it parent.
#[test]
fn a_fork_claims_no_worker_and_no_agent_is_its_own_ancestor() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"one worker\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"w1\",\"task\":\"say one word\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"started\"}\n",
        "{\"content\":\"word\"}\n",
    );
    // The test reads w1's folder after its done: keep it where it is
    // (finished workers are archived by default).
    let mut k = start_kernel_replay_prepared("standing-fork", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"coordinator\"\narchive_children = false\n",
        )
        .unwrap();
    });
    let mut a = snapshot(&k);
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "start one worker"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let root = wait_transcript(&k.place, "root", Duration::from_secs(20), |t| {
        t.iter().any(|e| e["kind"] == "tool" && e["child"] == "w1")
    });
    assert!(
        root.iter()
            .any(|e| e["kind"] == "tool" && e["child"] == "w1"),
        "the original keeps its claim: {root:?}"
    );

    // Item 2: the child is a worker on disk, with no coordinator text.
    let place = arbos_core::Place::new(&k.place);
    let w1 = arbos_core::Agent::load(&place.agent_dir("w1")).unwrap();
    assert_eq!(w1.role.as_deref(), Some("worker"));
    let agent_md = std::fs::read_to_string(place.agent_dir("w1").join("agent.md")).unwrap();
    assert!(agent_md.contains("role: worker"), "{agent_md}");

    // Item 1: the fork's copy names no child.
    let fork = arbos_core::files::fork_chat(&place, "root").unwrap();
    let copy = transcript(&k.place, fork.id.as_str());
    let spawns: Vec<&serde_json::Value> = copy
        .iter()
        .filter(|e| e["kind"] == "tool" && e["name"] == "spawn")
        .collect();
    assert_eq!(spawns.len(), 1, "{copy:?}");
    assert!(spawns[0].get("child").is_none(), "{:?}", spawns[0]);
    assert!(
        spawns[0]["body"]
            .as_str()
            .unwrap_or("")
            .contains("forked from"),
        "{:?}",
        spawns[0]
    );
    // And the kernel's replay of the fork claims none either.
    a.send(serde_json::json!({"type": "history", "agent": fork.id.as_str(), "limit": 200}));
    let replayed = a.wait(Duration::from_secs(5), |f| {
        f["type"] == "replayed" && f["event"]["name"] == "spawn"
    });
    let replayed = replayed.expect("the fork's spawn record replays");
    assert!(replayed["event"].get("child").is_none(), "{replayed}");

    // A parent line that loops: w1 now says its parent is w1. The tree
    // reports it top-level and root's replay no longer claims it.
    let md = agent_md.replace("parent: root", "parent: w1");
    assert_ne!(md, agent_md);
    std::fs::write(place.agent_dir("w1").join("agent.md"), md).unwrap();
    let mut b = snapshot(&k);
    // The snapshot's tree is the one taken on attach.
    let snap = {
        let mut c = Attach::connect(&k.url);
        c.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .unwrap()
    };
    let w1_node = snap["tree"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "w1")
        .expect("w1 in the tree");
    assert!(
        w1_node["parent"].is_null(),
        "an agent is never its own ancestor: {w1_node}"
    );
    b.send(serde_json::json!({"type": "history", "agent": "root", "limit": 200}));
    let replayed = b
        .wait(Duration::from_secs(5), |f| {
            f["type"] == "replayed" && f["event"]["name"] == "spawn"
        })
        .expect("root's spawn record replays");
    assert!(
        replayed["event"].get("child").is_none(),
        "a child that no longer calls root parent is not claimed: {replayed}"
    );
    let _ = k.child.kill();
}

/// The rewind pass needed a 6 s settle: `rewound` waited for the git
/// restore. Now the cut is announced at once; the restore reports after.
#[test]
fn rewound_arrives_before_the_file_restore() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"first\"}\n",
        "{\"agent\":\"root\",\"content\":\"second\"}\n",
    );
    let mut k = start_kernel_replay("standing-rewind", replies);
    // Checkpoints (what a rewind cuts back to) need the place to be a git
    // repository with a commit.
    for args in [
        vec!["init", "-q"],
        vec![
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "start",
        ],
    ] {
        let ok = std::process::Command::new("git")
            .args(&args)
            .current_dir(&k.place)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }
    let mut a = snapshot(&k);
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "one"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(20)));
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "two"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(20)));
    let before = transcript(&k.place, "root").len();

    let sent = Instant::now();
    a.send(serde_json::json!({"type": "rewind", "agent": "root", "turn": 2, "files": true}));
    let first = a
        .wait(Duration::from_secs(10), |f| {
            f["type"] == "rewound" && f["agent"] == "root"
        })
        .expect("a rewound frame");
    let waited = sent.elapsed();
    assert!(
        first["restored"].is_null(),
        "the first rewound announces the cut, not the restore: {first}"
    );
    assert!(waited < Duration::from_secs(3), "rewound took {waited:?}");
    let after = transcript(&k.place, "root").len();
    assert!(
        after < before,
        "the transcript is already cut: {before} -> {after}"
    );
    // The restore reports on its own, as a second rewound or an error.
    let follow = a.wait(Duration::from_secs(20), |f| {
        (f["type"] == "rewound" && !f["restored"].is_null()) || f["type"] == "error"
    });
    assert!(follow.is_some(), "the file restore must report");

    // qa-032: the cut takes the turn whole, wake included. What remains
    // ends on turn 1's turn_complete, and a restart fires nothing.
    let cut = transcript(&k.place, "root");
    assert!(
        cut.last().is_some_and(|e| e["kind"] == "turn_complete"),
        "the transcript must end on a finished turn, not a dangling wake: {cut:?}"
    );
    assert_eq!(
        cut.iter().filter(|e| e["kind"] == "wake").count(),
        1,
        "only turn 1's wake remains: {cut:?}"
    );
    let mut k2 = restart_replay(&mut k, "{\"agent\":\"root\",\"content\":\"unprompted\"}\n");
    let mut b = snapshot(&k2);
    assert!(
        b.wait(Duration::from_secs(3), |f| f["type"] == "turn"
            && f["agent"] == "root")
            .is_none(),
        "no turn may start on restart after a rewind"
    );
    let after_restart = transcript(&k2.place, "root");
    assert_eq!(after_restart.len(), cut.len(), "{after_restart:?}");
    let _ = k2.child.kill();
}
