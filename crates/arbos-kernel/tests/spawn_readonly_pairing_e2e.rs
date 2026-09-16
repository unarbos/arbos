//! Symmetry cycle 14, F-56: the spawn guard judged the `Output:` line
//! alone, so the model dropped the line, kept `kind: explore`, and seven
//! read-only workers wrote nothing. The rule now judges the brief — steps
//! that change files, Output, a Show line, a commit or PR, a worktree — and
//! a read-only kind on a writing task is set right: the worker starts as
//! a writing worker with the brief as given, and the result says why. A
//! read-only kind on a question stays read-only.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::path::Path;
use std::time::Duration;

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

const REPLIES: &str = concat!(
    "{\"agent\":\"root\",\"content\":\"two workers\",\"calls\":[",
    "{\"name\":\"spawn\",\"arguments\":{\"name\":\"type-hints\",\"kind\":\"explore\",\"task\":\"Add type hints to every function in tally/\",\"wait\":false}},",
    "{\"name\":\"spawn\",\"arguments\":{\"name\":\"what-main-does\",\"kind\":\"explore\",\"task\":\"What does main.py do? Five lines.\",\"wait\":false}}",
    "]}\n",
    "{\"agent\":\"root\",\"content\":\"started\"}\n",
    "{\"agent\":\"type-hints\",\"content\":\"hints added\"}\n",
    "{\"agent\":\"what-main-does\",\"content\":\"it tallies lines\"}\n",
    "{\"agent\":\"root\",\"content\":\"noted\"}\n",
    "{\"agent\":\"root\",\"content\":\"noted again\"}\n",
);

#[test]
fn a_read_only_kind_on_a_writing_task_starts_as_a_writer_and_on_a_question_stays_read_only() {
    let mut k = start_kernel_replay_prepared("readonly-pairing", REPLIES, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"coordinator\"\narchive_children = false\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "add type hints, and tell me what main does"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let root = transcript(&k.place, "root");
    let spawns: Vec<&serde_json::Value> = root
        .iter()
        .filter(|e| e["kind"] == "tool" && e["name"] == "spawn")
        .collect();
    assert_eq!(spawns.len(), 2, "{root:#?}");
    for s in &spawns {
        assert!(s.get("error").is_none(), "neither spawn is refused: {s:#?}");
    }
    // The writing task: started as a writing worker, with the reason.
    let hints = spawns
        .iter()
        .find(|s| s["args"]["name"] == "type-hints")
        .unwrap();
    let body = hints["body"].as_str().unwrap_or("");
    assert!(
        body.starts_with("spawned type-hints:"),
        "no kind on the line: {body}"
    );
    assert!(
        body.contains("kind \"explore\" is read-only and this task writes")
            && body.contains("\"Add type hints to every\"")
            && body.contains("started as a writing worker"),
        "{body}"
    );
    assert!(common::wait_for(Duration::from_secs(10), || {
        k.place.join(".arbos/agents/type-hints/agent.md").exists()
    }));
    let md = std::fs::read_to_string(k.place.join(".arbos/agents/type-hints/agent.md")).unwrap();
    assert!(!md.contains("readonly: true"), "a writer: {md}");
    assert!(
        md.contains("write"),
        "the writing tools are on its list: {md}"
    );
    // The question: kind explore stands, read-only as designed.
    let q = spawns
        .iter()
        .find(|s| s["args"]["name"] == "what-main-does")
        .unwrap();
    let body = q["body"].as_str().unwrap_or("");
    assert!(body.contains("(kind explore)"), "{body}");
    assert!(!body.contains("started as a writing worker"), "{body}");
    let md =
        std::fs::read_to_string(k.place.join(".arbos/agents/what-main-does/agent.md")).unwrap();
    assert!(md.contains("readonly: true"), "{md}");
    // The tree says which is which, so a window can show it while it runs.
    let mut b = Attach::connect(&k.url);
    let snap = b
        .wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
        .unwrap();
    let node = |id: &str| -> serde_json::Value {
        snap["tree"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["id"] == id)
            .cloned()
            .unwrap()
    };
    assert_eq!(node("what-main-does")["agent_kind"], "explore");
    assert_eq!(node("what-main-does")["readonly"], true);
    assert!(
        node("type-hints").get("readonly").is_none(),
        "{}",
        node("type-hints")
    );
    assert!(node("type-hints").get("agent_kind").is_none());
    let _ = k.child.kill();
}
