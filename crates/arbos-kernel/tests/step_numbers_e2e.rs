//! M-14 / the layout worker's ask: a window pairs the text it streamed
//! with the settled `assistant` line by heuristics, and a turn with
//! several steps — text, a tool, more text — or a client attached mid-turn
//! could double a paragraph. Now every `assistant_delta` and
//! `thinking_delta` carries `step` (1-based model step within the turn),
//! and the settled `assistant`, `thinking`, and `tool` lines of that step
//! carry the same number.

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
    "{\"agent\":\"root\",\"content\":\"First, a look.\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"echo one\",\"description\":\"Echo one\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"Then another.\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"echo two\",\"description\":\"Echo two\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"Done: one and two.\"}\n",
);

#[test]
fn deltas_and_settled_lines_of_one_step_carry_the_same_number() {
    let mut k = start_kernel_replay("step-numbers", REPLIES);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "two echoes"}));
    // Gather every frame until the turn is idle.
    let mut deltas: Vec<(u64, String)> = Vec::new();
    let idle = a.wait(Duration::from_secs(30), |f| {
        if f["type"] == "assistant_delta" && f["agent"] == "root" {
            deltas.push((
                f["step"].as_u64().unwrap_or(0),
                f["text"].as_str().unwrap_or("").to_string(),
            ));
        }
        f["type"] == "turn" && f["agent"] == "root" && f["state"] == "idle"
    });
    assert!(idle.is_some());
    assert!(common::wait_for(Duration::from_secs(5), || {
        transcript(&k.place, "root")
            .iter()
            .any(|e| e["kind"] == "turn_complete")
    }));
    // Deltas: the replay provider streams each reply's text as one chunk;
    // the three replies are steps 1, 2, 3.
    let by_step = |n: u64| -> String {
        deltas
            .iter()
            .filter(|(s, _)| *s == n)
            .map(|(_, t)| t.as_str())
            .collect::<String>()
    };
    assert!(
        deltas.iter().all(|(s, _)| *s >= 1),
        "no delta without a step: {deltas:?}"
    );
    assert!(by_step(1).contains("First, a look."), "{deltas:?}");
    assert!(by_step(2).contains("Then another."), "{deltas:?}");
    assert!(by_step(3).contains("Done: one and two."), "{deltas:?}");

    // Settled lines: the assistant line and the tool call of a step share
    // its number; the step is absent (0) on nothing this turn wrote.
    let root = transcript(&k.place, "root");
    let step_of = |kind: &str, needle: &str| -> u64 {
        root.iter()
            .find(|e| {
                e["kind"] == kind
                    && (e["text"].as_str().is_some_and(|t| t.contains(needle))
                        || e["args"]["command"]
                            .as_str()
                            .is_some_and(|c| c.contains(needle)))
            })
            .and_then(|e| e["step"].as_u64())
            .unwrap_or(0)
    };
    assert_eq!(step_of("assistant", "First, a look."), 1);
    assert_eq!(step_of("tool", "echo one"), 1);
    assert_eq!(step_of("assistant", "Then another."), 2);
    assert_eq!(step_of("tool", "echo two"), 2);
    assert_eq!(step_of("assistant", "Done: one and two."), 3);
    // The wake and the user line are the kernel's, not a step's.
    assert!(
        root.iter()
            .filter(|e| e["kind"] == "user" || e["kind"] == "wake")
            .all(|e| e.get("step").is_none()),
        "{root:#?}"
    );
    let _ = k.child.kill();
}
