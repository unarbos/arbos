//! QA draft 6e9ce33ec3 (inbox:attach-bind-token, kernel 354413e0): the
//! same tool, `await`, was called six times running at the 300 s ceiling;
//! the turn went round rather than on. The ceiling's only suggestion was
//! "Await again". The third ceiling in a row on the same job now says the
//! job is not finishing on its own — a server, a watcher, or a hang — and
//! not to await it again.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::time::Duration;

const REPLIES: &str = concat!(
    "{\"agent\":\"w1\",\"content\":\"start\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 600\",\"background\":true}}]}\n",
    "{\"agent\":\"w1\",\"content\":\"wait 1\",\"calls\":[{\"name\":\"await\",\"arguments\":{\"id\":\"j1\",\"wait_ms\":300}}]}\n",
    "{\"agent\":\"w1\",\"content\":\"wait 2\",\"calls\":[{\"name\":\"await\",\"arguments\":{\"id\":\"j1\",\"wait_ms\":300}}]}\n",
    "{\"agent\":\"w1\",\"content\":\"wait 3\",\"calls\":[{\"name\":\"await\",\"arguments\":{\"id\":\"j1\",\"wait_ms\":300}}]}\n",
    "{\"agent\":\"w1\",\"content\":\"done\"}\n",
);

#[test]
fn the_third_ceiling_in_a_row_says_the_job_is_not_finishing_and_not_to_await_again() {
    let mut k = start_kernel_replay_prepared("await-loop", REPLIES, "", |place| {
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
    a.send(serde_json::json!({"type": "user", "agent": "w1", "text": "run and wait"}));
    assert!(a.wait_turn("w1", "idle", Duration::from_secs(60)));
    let w1: Vec<serde_json::Value> =
        std::fs::read_to_string(k.place.join(".arbos/agents/w1/transcript.jsonl"))
            .unwrap()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
    let awaits: Vec<&str> = w1
        .iter()
        .filter(|e| e["kind"] == "tool" && e["name"] == "await")
        .map(|e| e["body"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(awaits.len(), 3, "{w1:#?}");
    assert!(
        awaits[0].contains("Still waiting") && awaits[0].contains("Await again"),
        "{}",
        awaits[0]
    );
    assert!(awaits[1].contains("Still waiting"), "{}", awaits[1]);
    // The third: the truth, and no invitation to go round again.
    assert!(
        awaits[2].contains("after 3 awaits in a row this turn"),
        "{}",
        awaits[2]
    );
    assert!(awaits[2].contains("with no new output"), "{}", awaits[2]);
    assert!(awaits[2].contains("Do not await it again"), "{}", awaits[2]);
    assert!(!awaits[2].contains("Await again"), "{}", awaits[2]);
    let _ = k.child.kill();
}
