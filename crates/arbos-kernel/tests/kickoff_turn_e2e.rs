//! Cursor runs a turn the moment a Project is created — "Setting up
//! environment", then a greeting — with no user prompt. A client sends
//! `kickoff` when it opens a place for the first time; root runs one
//! bounded turn: look at the folder, seed the context file and the page,
//! greet in two lines. No spawn, no ask. A second `kickoff` is a no-op.

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

#[test]
fn the_first_open_runs_one_kickoff_turn_that_greets_and_seeds_the_store() {
    let replies = concat!(
        // The model tries a spawn and an ask on the way (refused), then
        // does the turn as briefed.
        "{\"agent\":\"root\",\"content\":\"looking around\",\"calls\":[",
        "{\"name\":\"bash\",\"arguments\":{\"command\":\"ls -la; cat README* 2>/dev/null | head -40; git log --oneline 2>/dev/null | head -5\",\"description\":\"Look around the new place\"}},",
        "{\"name\":\"spawn\",\"arguments\":{\"name\":\"explore\",\"task\":\"map the repo\"}},",
        "{\"name\":\"ask\",\"arguments\":{\"question\":\"What are we building?\"}}",
        "]}\n",
        "{\"agent\":\"root\",\"content\":\"seeding\",\"calls\":[",
        "{\"name\":\"write\",\"arguments\":{\"path\":\"docs/project-context.md\",\"contents\":\"+++\\nowner = \\\"root\\\"\\n+++\\n# Project context\\n\\n## Goal\\n(not stated yet — the user's first ask sets it)\\n\\n## Resources\\n- A small Python project: hello.py, on main.\\n\"}},",
        "{\"name\":\"plan\",\"arguments\":{\"op\":\"set\",\"items\":[{\"label\":\"Kickoff\",\"target\":\"docs/project-context.md\",\"readout\":\"ready; waiting for the first ask\"}]}}",
        "]}\n",
        "{\"agent\":\"root\",\"content\":\"Hey Jacob — this place is ready: a small Python project with hello.py on main.\\nTell me what to work on; if you want me to work differently, say so and I will remember.\"}\n",
    );
    let mut k = start_kernel_replay_prepared("kickoff-turn", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(place.join(".arbos/user.md"), "name: Jacob\n").unwrap();
        std::fs::write(place.join("hello.py"), "print(1)\n").unwrap();
        std::fs::write(place.join("README.md"), "# Toy\n").unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    assert!(transcript(&k.place, "root").is_empty(), "a fresh place");
    a.send(serde_json::json!({"type": "kickoff"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(40)));

    let root = transcript(&k.place, "root");
    let wake = root
        .iter()
        .find(|e| e["kind"] == "wake")
        .expect("the kickoff wake");
    assert_eq!(wake["wake"], "kickoff");
    let brief = wake["text"].as_str().unwrap_or("");
    assert!(brief.contains("The user's name is Jacob"), "{brief}");
    assert!(brief.contains("No spawn, no ask"), "{brief}");
    // The look-around ran, with its label; the spawn and the ask did not.
    let tool = |name: &str| -> &serde_json::Value {
        root.iter()
            .find(|e| e["kind"] == "tool" && e["name"] == name)
            .unwrap_or_else(|| panic!("no {name} record: {root:#?}"))
    };
    assert!(tool("bash").get("error").is_none());
    assert_eq!(tool("bash")["label"], "Look around the new place");
    assert!(
        tool("spawn")["error"]
            .as_str()
            .unwrap_or("")
            .contains("not in the kickoff turn"),
        "{:#?}",
        tool("spawn")
    );
    assert!(
        tool("ask")["error"]
            .as_str()
            .unwrap_or("")
            .contains("not in the kickoff turn"),
        "{:#?}",
        tool("ask")
    );
    assert!(!k.place.join(".arbos/agents/explore").exists());
    // The store is seeded.
    let context = std::fs::read_to_string(k.place.join(".arbos/docs/project-context.md")).unwrap();
    assert!(context.contains("hello.py"), "{context}");
    let page = std::fs::read_to_string(k.place.join(".arbos/notes.md")).unwrap();
    assert!(
        page.contains(
            "- [ ] [Kickoff](docs/project-context.md) — ready; waiting for the first ask"
        ),
        "{page}"
    );
    // The greeting: two lines, the name, no more.
    let greeting = root
        .iter()
        .rev()
        .find(|e| e["kind"] == "assistant" && !e["text"].as_str().unwrap_or("").is_empty())
        .unwrap()["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(greeting.starts_with("Hey Jacob —"), "{greeting}");
    assert_eq!(greeting.lines().count(), 2, "{greeting}");
    assert_eq!(
        root.iter().filter(|e| e["kind"] == "turn_complete").count(),
        1
    );

    // A second kickoff (the place opened again) files nothing.
    a.send(serde_json::json!({"type": "kickoff"}));
    std::thread::sleep(Duration::from_millis(800));
    let again = transcript(&k.place, "root");
    assert_eq!(
        again.iter().filter(|e| e["kind"] == "wake").count(),
        1,
        "one kickoff wake ever: {again:#?}"
    );
    let _ = k.child.kill();
}
