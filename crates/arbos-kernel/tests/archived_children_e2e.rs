//! F-57: asked which workers were running and which archived, the
//! coordinator said it had none while the panel showed six archived. The
//! `agents` tool with no ids now lists finished workers from the archive
//! beside the live ones, and the standing context carries them (unit test
//! in the engine).

mod common;

use common::{Attach, start_kernel_replay};
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
    "{\"agent\":\"root\",\"content\":\"one worker\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"write-cli\",\"task\":\"say the codeword\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"started\"}\n",
    "{\"agent\":\"write-cli\",\"content\":\"Done: the codeword is xylophone.\"}\n",
    "{\"agent\":\"root\",\"content\":\"noted\"}\n",
    "{\"agent\":\"root\",\"content\":\"listing\",\"calls\":[{\"name\":\"agents\",\"arguments\":{}}]}\n",
    "{\"agent\":\"root\",\"content\":\"One finished worker: write-cli.\"}\n",
);

#[test]
fn the_agents_tool_lists_archived_workers_by_default() {
    let mut k = start_kernel_replay("archived-children", REPLIES);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "get the codeword"}));
    // The worker finishes, root's done turn runs, the worker is archived.
    assert!(common::wait_for(Duration::from_secs(30), || {
        k.place
            .join(".arbos/archive/agents/write-cli/agent.md")
            .exists()
            && transcript(&k.place, "root")
                .iter()
                .filter(|e| e["kind"] == "turn_complete")
                .count()
                >= 2
    }));
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "which workers ran, and which are archived?"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert!(common::wait_for(Duration::from_secs(5), || {
        transcript(&k.place, "root")
            .iter()
            .any(|e| e["kind"] == "tool" && e["name"] == "agents")
    }));
    let root = transcript(&k.place, "root");
    let call = root
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "agents")
        .unwrap();
    let body = call["body"].as_str().unwrap_or("");
    assert!(
        body.contains("write-cli"),
        "the archived worker is listed: {body}"
    );
    assert!(body.contains("archived"), "{body}");
    assert!(
        body.contains("codeword is xylophone"),
        "with its last words: {body}"
    );
    assert!(!body.contains("no workers"), "{body}");
    let _ = k.child.kill();
}
