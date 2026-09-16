//! subnet120 from the phone (2026-09-16): the transcript showed raw
//! `<invoke>` tool-call markup as prose — a model writing a call as text.
//! The markup is cut before the line reaches the transcript (the words
//! around it stay), the model is nudged once, and the real call follows.

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

const MARKUP: &str = "I'll list the folder first.\n\n<function_calls>\n<invoke name=\"bash\">\n<parameter name=\"command\">ls</parameter>\n</invoke>\n</function_calls>";

#[test]
fn invoke_markup_never_reaches_the_transcript_and_the_model_is_nudged_once() {
    let replies = format!(
        "{{\"agent\":\"root\",\"content\":{m}}}\n{{\"agent\":\"root\",\"content\":\"listing\",\"calls\":[{{\"name\":\"bash\",\"arguments\":{{\"command\":\"echo listed\",\"description\":\"List the folder\"}}}}]}}\n{{\"agent\":\"root\",\"content\":\"Done: the folder is listed.\"}}\n",
        m = serde_json::to_string(MARKUP).unwrap()
    );
    let mut k = start_kernel_replay("tool-markup", &replies);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "list the folder"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert!(common::wait_for(Duration::from_secs(5), || {
        transcript(&k.place, "root")
            .iter()
            .any(|e| e["kind"] == "turn_complete")
    }));
    let root = transcript(&k.place, "root");
    let text =
        std::fs::read_to_string(k.place.join(".arbos/agents/root/transcript.jsonl")).unwrap();
    assert!(
        !text.contains("<invoke"),
        "no markup on the transcript: {text}"
    );
    assert!(
        !text.contains("<function_calls>") && !text.contains("<parameter"),
        "{text}"
    );
    let first = root
        .iter()
        .find(|e| e["kind"] == "assistant")
        .unwrap_or_else(|| panic!("{root:#?}"));
    assert_eq!(
        first["text"], "I'll list the folder first.",
        "the prose around it stays"
    );
    let nudges: Vec<&serde_json::Value> = root
        .iter()
        .filter(|e| e["kind"] == "nudge" && e["reason"] == "tool call written as text")
        .collect();
    assert_eq!(nudges.len(), 1, "{root:#?}");
    assert!(
        nudges[0]["text"]
            .as_str()
            .unwrap()
            .contains("markup was not kept")
    );
    // The real call then ran, and the turn ended normally.
    assert!(
        root.iter()
            .any(|e| e["kind"] == "tool" && e["name"] == "bash" && e["error"].is_null()),
        "{root:#?}"
    );
    assert!(
        !root
            .iter()
            .any(|e| e["kind"] == "nudge" && e["reason"] == "empty reply"),
        "a markup-only reply is not read as silence: {root:#?}"
    );
    let _ = k.child.kill();
}
