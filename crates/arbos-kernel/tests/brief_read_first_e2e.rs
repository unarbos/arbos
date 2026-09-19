//! QA `mt-26`: the kickoff brief said `Read first: .arbos/docs/project-context.md`
//! while the worker's prompt already carried that file whole, so a worker
//! that obeyed the brief spent a call re-reading what it had been handed.
//! The default read-first now leaves the file out when it is injected, and
//! says so; a place with no context beyond the template keeps the old line.

mod common;

use common::{Attach, start_kernel_replay_prepared, wait_for};
use std::path::Path;
use std::time::Duration;

fn worker_brief(place: &Path, worker: &str) -> Option<String> {
    let text = std::fs::read_to_string(
        place
            .join(".arbos/agents")
            .join(worker)
            .join("transcript.jsonl"),
    )
    .ok()?;
    text.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|e| e["kind"] == "wake" && e["brief"].is_string())
        .map(|e| e["brief"].as_str().unwrap_or("").to_string())
}

const REPLIES: &str = concat!(
    "{\"agent\":\"root\",\"content\":\"one worker\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"helper\",\"task\":\"say hello\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"started\"}\n",
    "{\"agent\":\"helper\",\"content\":\"hello\",\"delay_ms\":2500}\n",
    "{\"agent\":\"root\",\"content\":\"noted\"}\n",
);

fn run(name: &str, context: Option<&str>) -> String {
    let k = start_kernel_replay_prepared(name, REPLIES, "", |place| {
        std::fs::create_dir_all(place.join(".arbos/docs")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"coordinator\"\narchive_children = false\n",
        )
        .unwrap();
        if let Some(text) = context {
            std::fs::write(place.join(".arbos/docs/project-context.md"), text).unwrap();
        }
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "say hello"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert!(
        wait_for(Duration::from_secs(20), || worker_brief(&k.place, "helper")
            .is_some()),
        "the worker's first wake carries the brief"
    );
    worker_brief(&k.place, "helper").unwrap()
}

#[test]
fn a_context_the_prompt_carries_is_not_also_read_first() {
    let brief = run(
        "brief-read-first-injected",
        Some("+++\nowner = \"root\"\n+++\n# Project context\nGoal: greet politely.\n"),
    );
    let read_first = brief
        .lines()
        .find(|l| l.starts_with("Read first:"))
        .unwrap_or_default()
        .to_string();
    assert!(
        !read_first.contains("project-context.md,") && read_first.contains(".arbos/notes.md"),
        "the injected file is not read-first: {read_first}\n{brief}"
    );
    assert!(
        read_first.contains("already in your prompt"),
        "and the brief says why: {read_first}"
    );
}

#[test]
fn a_place_with_no_context_keeps_the_file_in_read_first() {
    let brief = run("brief-read-first-plain", None);
    assert!(
        brief.starts_with("Read first: .arbos/docs/project-context.md, then .arbos/notes.md"),
        "{brief}"
    );
}
