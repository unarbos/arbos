//! Symmetry loop, cycle 3: asked to run one shell command and show its
//! output, Cursor's coordinator ran it; ours had no shell and told the
//! user about its role. The coordinator keeps `bash` for that one quick
//! command (and the contract says never to explain the role split); undo
//! stays a worker's.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::time::Duration;

fn transcript(place: &std::path::Path, agent: &str) -> Vec<serde_json::Value> {
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

#[test]
fn a_coordinator_runs_the_one_command_the_user_asks_to_see() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"running it\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"for i in 1 2 3; do echo \\\"step $i\\\"; done\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"Three steps, as above.\"}\n",
    );
    let mut k = start_kernel_replay_prepared("coordinator-shell", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"coordinator\"\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({
        "type": "user", "agent": "root",
        "text": "Run this exact shell command and show me its output: for i in 1 2 3; do echo \"step $i\"; done"
    }));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let root = transcript(&k.place, "root");
    let bash = root
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "bash")
        .expect("the coordinator's bash call is on its transcript");
    assert!(
        bash.get("error").is_none(),
        "the call ran instead of being refused: {bash:#?}"
    );
    let body = bash["body"].as_str().unwrap_or("");
    assert!(
        body.contains("step 1") && body.contains("step 3"),
        "the output came back: {body}"
    );
    // The role still narrows the roster: no undo.
    let place = arbos_core::Place::new(&k.place);
    let mut agent = arbos_core::Agent::load(&place.agent_dir("root")).unwrap();
    arbos_core::project::apply_role(&place, &mut agent);
    assert!(agent.may("bash") && agent.may("spawn"));
    assert!(!agent.may("undo"));
    let _ = k.child.kill();
}

/// Symmetry cycle 6: asked to run the test suite, the coordinator ran it
/// itself. A build or a test run in the coordinator's shell is refused
/// with the way out; the quick command still runs.
#[test]
fn a_coordinators_test_run_is_refused_and_its_quick_command_is_not() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"running the suite\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"python3 -m pytest -q\"}},{\"name\":\"bash\",\"arguments\":{\"command\":\"ls -la | wc -l\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"noted\"}\n",
    );
    let mut k = start_kernel_replay_prepared("coordinator-pytest", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"coordinator\"\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({
        "type": "user", "agent": "root",
        "text": "Run the test suite with python3 -m pytest -q and report the result."
    }));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let root = transcript(&k.place, "root");
    let bashes: Vec<&serde_json::Value> = root
        .iter()
        .filter(|e| e["kind"] == "tool" && e["name"] == "bash")
        .collect();
    assert_eq!(bashes.len(), 2, "{root:#?}");
    let pytest = bashes
        .iter()
        .find(|b| b["args"]["command"] == "python3 -m pytest -q")
        .expect("the pytest call");
    let err = pytest["error"].as_str().unwrap_or("");
    assert!(
        err.contains("a test run is a worker's job") && err.contains("spawn a worker"),
        "refused with the way out: {pytest:#?}"
    );
    let ls = bashes
        .iter()
        .find(|b| b["args"]["command"] == "ls -la | wc -l")
        .expect("the ls call");
    assert!(ls.get("error").is_none(), "the quick command runs: {ls:#?}");
    let _ = k.child.kill();
}
