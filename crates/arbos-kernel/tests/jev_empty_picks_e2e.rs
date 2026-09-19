//! Jacob asked for more of Jev's picks. The Decisions door sends
//! `args: {}`, so a pick is only useful when the empty call is a real
//! call. Here each newly pickable tool is called with `{}` by the replay
//! model and does the safe thing: read takes the last file touched this
//! turn and says so; await waits on the newest job; transcript reads
//! myself when I have no worker; record reports status; subscribe lists;
//! todo shows. (The routing itself is pinned in the unit tests.)

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::time::Duration;

// Root (a coordinator: no await) does the reads; the worker w1 starts a
// job and awaits it with `{}`.
const REPLIES: &str = concat!(
    "{\"agent\":\"root\",\"content\":\"read it\",\"calls\":[{\"name\":\"read\",\"arguments\":{\"path\":\"a.txt\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"empty picks\",\"calls\":[{\"name\":\"read\",\"arguments\":{}},{\"name\":\"transcript\",\"arguments\":{}},{\"name\":\"record\",\"arguments\":{}},{\"name\":\"subscribe\",\"arguments\":{}},{\"name\":\"todo\",\"arguments\":{}}]}\n",
    "{\"agent\":\"root\",\"content\":\"done\"}\n",
    "{\"agent\":\"w1\",\"content\":\"start a job\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 2; echo job-done\",\"background\":true}}]}\n",
    "{\"agent\":\"w1\",\"content\":\"wait\",\"calls\":[{\"name\":\"await\",\"arguments\":{}}]}\n",
    "{\"agent\":\"w1\",\"content\":\"done\"}\n",
);

#[test]
fn each_newly_pickable_tool_does_the_safe_thing_on_an_empty_call() {
    let mut k = start_kernel_replay_prepared("jev-empty-picks", REPLIES, "", |place| {
        std::fs::write(place.join("a.txt"), "alpha\nbeta\n").unwrap();
        let w = place.join(".arbos/agents/w1");
        std::fs::create_dir_all(&w).unwrap();
        let mut worker = arbos_core::Agent::root("w1");
        worker.parent = Some(arbos_core::AgentId::new("root"));
        worker.save(&w).unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "go"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(60)));
    assert!(common::wait_for(Duration::from_secs(5), || {
        std::fs::read_to_string(k.place.join(".arbos/agents/root/transcript.jsonl"))
            .unwrap_or_default()
            .contains("turn_complete")
    }));
    let root: Vec<serde_json::Value> =
        std::fs::read_to_string(k.place.join(".arbos/agents/root/transcript.jsonl"))
            .unwrap()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
    let tools: Vec<(&str, &str, &str)> = root
        .iter()
        .filter(|e| e["kind"] == "tool")
        .map(|e| {
            (
                e["name"].as_str().unwrap_or(""),
                e["body"].as_str().unwrap_or(""),
                e["error"].as_str().unwrap_or(""),
            )
        })
        .collect();
    let find = |name: &str, nth: usize| {
        tools
            .iter()
            .filter(|(n, _, _)| *n == name)
            .nth(nth)
            .unwrap_or_else(|| panic!("{name} #{nth}: {tools:#?}"))
    };
    // read {}: the last file touched, said.
    let (_, body, err) = find("read", 1);
    assert!(err.is_empty(), "read {{}}: {err}");
    assert!(body.contains("no path in the call; used a.txt"), "{body}");
    assert!(body.contains("|alpha"), "{body}");
    // transcript {}: my newest worker (w1 is the only one).
    let (_, body, err) = find("transcript", 0);
    assert!(err.is_empty(), "transcript {{}}: {err}");
    assert!(body.starts_with("w1:"), "the worker's transcript: {body}");
    // record {}: status, not an error.
    let (_, body, err) = find("record", 0);
    assert!(err.is_empty(), "record {{}}: {err}");
    assert!(!body.is_empty(), "record status says something");
    // subscribe {}: a list (the weekly gc chore is internal, so it may be
    // empty) — not "kind is required".
    let (_, _, err) = find("subscribe", 0);
    assert!(err.is_empty(), "subscribe {{}}: {err}");
    // todo {}: show.
    let (_, _, err) = find("todo", 0);
    assert!(err.is_empty(), "todo {{}}: {err}");
    // The worker: await {} is the newest job — the one bash started.
    a.send(serde_json::json!({"type": "user", "agent": "w1", "text": "run and wait"}));
    assert!(a.wait_turn("w1", "idle", Duration::from_secs(60)));
    let w1: Vec<serde_json::Value> =
        std::fs::read_to_string(k.place.join(".arbos/agents/w1/transcript.jsonl"))
            .unwrap()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
    let awaited = w1
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "await")
        .unwrap_or_else(|| panic!("{w1:#?}"));
    assert!(awaited["error"].is_null(), "await {{}}: {awaited:#?}");
    // The bash call's own wait already read the echo; the await found
    // the job and saw it end.
    assert!(
        awaited["body"]
            .as_str()
            .unwrap_or("")
            .contains("Job j1 exited with code 0"),
        "{awaited:#?}"
    );
    let _ = k.child.kill();
}
