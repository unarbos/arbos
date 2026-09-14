//! The live line beside an agent's name: `status "…"` from the agent, the
//! kernel's guess from the tool in flight when it has not said, cleared
//! at the turn's end; on the wire as `status` frames and in the tree.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::time::Duration;

#[test]
fn the_agents_own_line_wins_the_kernel_guesses_otherwise_and_idle_clears_it() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"starting\",\"calls\":[{\"name\":\"status\",\"arguments\":{\"step\":\"Reading project context and secrets inventory\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"working\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 2; echo ok\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"done one\"}\n",
        "{\"agent\":\"root\",\"content\":\"second\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 2; echo again\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"done two\"}\n",
    );
    let mut k = start_kernel_replay_prepared("status", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\nname = \"s\"\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "go"}));

    // The agent's own line arrives as a status frame and lands on disk.
    let said = a
        .wait(Duration::from_secs(20), |f| {
            f["type"] == "status" && f["agent"] == "root"
        })
        .expect("a status frame");
    assert_eq!(
        said["step"],
        "Reading project context and secrets inventory"
    );
    assert_eq!(said["source"], "agent");
    let file = k.place.join(".arbos/agents/root/status.toml");
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(
        text.contains("step = \"Reading project context and secrets inventory\""),
        "{text}"
    );
    // A fresh attach mid-turn sees it in the tree.
    let mut b = Attach::connect(&k.url);
    let snap = b
        .wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
        .unwrap();
    let root_node = snap["tree"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "root")
        .unwrap();
    assert_eq!(
        root_node["step"], "Reading project context and secrets inventory",
        "{snap}"
    );
    // The bash step that follows does not overwrite what the agent said.
    let next = a
        .wait(Duration::from_secs(20), |f| {
            f["type"] == "status" && f["agent"] == "root"
        })
        .expect("the next status frame");
    assert_eq!(
        next["step"], "",
        "the turn's end clears the line, and nothing derived came before it: {next}"
    );
    assert!(!file.exists(), "idle: no status file");

    // Second turn: no `status` call, so the kernel says what runs.
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "again"}));
    let derived = a
        .wait(Duration::from_secs(20), |f| {
            f["type"] == "status" && f["agent"] == "root" && f["step"] != ""
        })
        .expect("a derived status frame");
    assert_eq!(derived["source"], "derived");
    assert!(
        derived["step"]
            .as_str()
            .unwrap_or("")
            .starts_with("Running sleep 2; echo again"),
        "{derived}"
    );
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    // The transcript shows the tool call like any other, with its result.
    let t = std::fs::read_to_string(k.place.join(".arbos/agents/root/transcript.jsonl")).unwrap();
    assert!(
        t.contains("\"name\":\"status\"") && t.contains("Status: Reading project context"),
        "{t}"
    );
    let _ = k.child.kill();
}
