//! A zero-byte `agent.md` — a file-provider placeholder, an interrupted
//! copy — used to parse as an agent with every default: unpaused,
//! writable, full allowlist, no parent. A paused read-only worker came
//! back as a free-running writer, in silence. Now a worker in that state
//! is not served and is named at boot, its transcript left alone; root's
//! is rewritten with defaults and the rewrite is said on its transcript.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::path::Path;
use std::time::Duration;

fn agent_dir(place: &Path, id: &str) -> std::path::PathBuf {
    place.join(".arbos/agents").join(id)
}

#[test]
fn a_worker_whose_agent_md_is_empty_is_not_served_and_is_named_not_reset() {
    let k = start_kernel_replay_prepared("empty-agent-md-worker", "", "", |place| {
        // Root, whole. A worker whose agent.md is zero bytes, with a
        // transcript that must survive.
        let root = agent_dir(place, "root");
        std::fs::create_dir_all(&root).unwrap();
        arbos_core::Agent::root("root").save(&root).unwrap();
        let w = agent_dir(place, "w1");
        std::fs::create_dir_all(&w).unwrap();
        std::fs::write(w.join("agent.md"), "").unwrap();
        std::fs::write(
            w.join("transcript.jsonl"),
            "{\"ts\":1000000,\"kind\":\"assistant\",\"text\":\"the worker's words\"}\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    let snap = a
        .wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
        .unwrap();
    let ids: Vec<String> = snap["tree"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["id"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(ids.contains(&"root".to_string()), "{ids:?}");
    assert!(
        !ids.contains(&"w1".to_string()),
        "an agent with an empty agent.md is not served: {ids:?}"
    );
    // Named at boot with the reason; nothing rewritten; the record intact.
    let log =
        std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.log")).unwrap_or_default();
    assert!(
        log.contains("agent_unlisted") && log.contains("agents/w1: agent.md: agent.md is empty"),
        "{log}"
    );
    let md = std::fs::read_to_string(agent_dir(&k.place, "w1").join("agent.md")).unwrap();
    assert_eq!(
        md, "",
        "the empty file is left for the person, not overwritten"
    );
    let t = std::fs::read_to_string(agent_dir(&k.place, "w1").join("transcript.jsonl")).unwrap();
    assert!(t.contains("the worker's words"), "{t}");
    // A frame for it is refused, not served as a default agent.
    a.send(serde_json::json!({"type": "user", "agent": "w1", "text": "hello"}));
    let err = a
        .wait(Duration::from_secs(5), |f| {
            f["type"] == "error" && f["agent"] == "w1"
        })
        .expect("refused");
    assert!(
        err["detail"]
            .as_str()
            .unwrap_or("")
            .contains("agent.md is empty")
            || err["detail"].as_str().unwrap_or("").contains("no agent"),
        "{err}"
    );
}

#[test]
fn roots_empty_agent_md_is_rewritten_with_defaults_and_said() {
    let k = start_kernel_replay_prepared("empty-agent-md-root", "", "", |place| {
        let root = agent_dir(place, "root");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("agent.md"), "").unwrap();
        std::fs::write(
            root.join("transcript.jsonl"),
            "{\"ts\":1000000,\"kind\":\"assistant\",\"text\":\"earlier words\"}\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some(),
        "the place still serves"
    );
    let root = agent_dir(&k.place, "root");
    let md = std::fs::read_to_string(root.join("agent.md")).unwrap();
    assert!(md.contains("name: root"), "rewritten with defaults: {md}");
    assert!(
        root.join("agent.md.unreadable").exists(),
        "the old file is kept"
    );
    let t = std::fs::read_to_string(root.join("transcript.jsonl")).unwrap();
    assert!(t.contains("earlier words"), "the record is untouched: {t}");
    assert!(
        t.contains("agent.md could not be read") && t.contains("agent.md.unreadable"),
        "the rewrite is said on the transcript: {t}"
    );
}
