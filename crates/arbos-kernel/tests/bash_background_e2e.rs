//! F-37 / F-43: "run this exact command and show me its output as it
//! arrives" — a six-step loop with `sleep 2` — came back after "step 1"
//! when the model marked it `background:true`, and the user saw one line
//! of six. `background` is for a server; a loop runs attached to its end
//! whatever the flag says, and the result says so. A real server still
//! goes to a job at once.

mod common;

use common::{Attach, start_kernel_replay};
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

const REPLIES: &str = concat!(
    "{\"agent\":\"root\",\"content\":\"Running the loop\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"for i in 1 2 3 4 5 6; do echo \\\"step $i\\\"; sleep 1; done\",\"background\":true,\"description\":\"Execute a loop with delayed output\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"The command has finished.\"}\n",
    "{\"agent\":\"root\",\"content\":\"Starting the server\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"while true; do sleep 1; done\",\"background\":true,\"description\":\"Keep a watcher running\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"Running.\"}\n",
);

#[test]
fn a_loop_marked_background_runs_attached_to_its_end_and_a_server_does_not() {
    let mut k = start_kernel_replay("bash-background", REPLIES);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    let started = Instant::now();
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "Run this exact shell command and show me its output as it arrives: for i in 1 2 3 4 5 6; do echo \"step $i\"; sleep 1; done"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(60)));
    assert!(
        started.elapsed() >= Duration::from_secs(5),
        "the call stayed attached for the six steps: {:?}",
        started.elapsed()
    );
    let root = transcript(&k.place, "root");
    let call = root
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "bash")
        .unwrap_or_else(|| panic!("{root:#?}"));
    let body = call["body"].as_str().unwrap_or("");
    for i in 1..=6 {
        assert!(body.contains(&format!("step {i}")), "step {i} in: {body}");
    }
    assert!(
        !body.contains("as job"),
        "not handed to a job after the first line: {body}"
    );
    assert!(
        body.contains("background:true is for a server"),
        "the result says why it ran attached: {body}"
    );

    // A watcher: background as asked, back within a moment.
    let started = Instant::now();
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "start the watcher"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "a server goes to a job at once: {:?}",
        started.elapsed()
    );
    let root = transcript(&k.place, "root");
    let call = root
        .iter()
        .filter(|e| e["kind"] == "tool" && e["name"] == "bash")
        .nth(1)
        .unwrap_or_else(|| panic!("{root:#?}"));
    let body = call["body"].as_str().unwrap_or("");
    assert!(body.contains("Running as job"), "{body}");
    let _ = k.child.kill();
}
