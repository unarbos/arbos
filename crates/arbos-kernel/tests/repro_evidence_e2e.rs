//! SWE-bench loop, cycle 11: a refused `pip download` exited non-zero and
//! became "reproduction 1", so an agent that had reproduced nothing
//! believed it had, and the done rule passed on a command that only
//! failed. With `ARBOS_REPRO_REQUIRED=1`, only a failing command that ran
//! code stands in for the reproduction the agent did not mark; a failed
//! listing or fetch does not, and the first edit is still refused.

mod common;

use common::{Attach, scratch_dir, spawn_with_env};
use std::time::Duration;

fn tools(place: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|e| e["kind"] == "tool" && e["ended"].is_number())
        .collect()
}

#[test]
fn a_failed_listing_is_not_a_reproduction_but_a_failed_run_is() {
    let replies = concat!(
        // A failing command that ran no code, then the first edit.
        "{\"agent\":\"root\",\"content\":\"\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"ls /nope-not-here\",\"description\":\"Look for the file\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"\",\"calls\":[{\"name\":\"write\",\"arguments\":{\"path\":\"fix.txt\",\"content\":\"fixed\",\"mechanism\":\"the lookup misses the file because the path is wrong; point it at the right one\"}}]}\n",
        // The failure itself, run; then the edit again.
        "{\"agent\":\"root\",\"content\":\"\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"python3 -c 'raise SystemExit(1)'\",\"description\":\"Run the failing snippet\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"\",\"calls\":[{\"name\":\"write\",\"arguments\":{\"path\":\"fix.txt\",\"content\":\"fixed\",\"mechanism\":\"the lookup misses the file because the path is wrong; point it at the right one\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"Done.\"}\n",
    );
    let dir = scratch_dir("repro-evidence");
    // A headless place: root does the work itself.
    std::fs::create_dir_all(dir.join("place/.arbos")).unwrap();
    std::fs::write(
        dir.join("place/.arbos/project.toml"),
        "schema = 2\n[root]\nrole = \"worker\"\n",
    )
    .unwrap();
    let file = dir.join("replies.jsonl");
    std::fs::write(&file, replies).unwrap();
    std::fs::write(dir.join("xdg/arbos/config.toml"), "trace = false\n").unwrap();
    let replies_path = file.display().to_string();
    let mut k = spawn_with_env(
        dir,
        &["--provider", "replay", "--replies", &replies_path],
        &[("ARBOS_REPRO_REQUIRED", "1")],
    );
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(
        serde_json::json!({"type":"user","agent":"root","text":"fix the lookup","attachments":[]}),
    );
    assert!(a.wait_turn("root", "idle", Duration::from_secs(60)));
    let calls = tools(&k.place);
    let writes: Vec<&serde_json::Value> = calls.iter().filter(|c| c["name"] == "write").collect();
    assert_eq!(writes.len(), 2, "{calls:#?}");
    let first = writes[0]["error"].as_str().unwrap_or("");
    assert!(
        first.contains("no failing reproduction is on record"),
        "the failed `ls` did not stand in: {first}"
    );
    assert!(
        writes[1]["error"].is_null(),
        "after a failing run, the edit proceeds: {:?}",
        writes[1]
    );
    let body = writes[1]["body"].as_str().unwrap_or("");
    assert!(
        body.contains("taken as a reproduction") && body.contains("python3"),
        "{body}"
    );
    assert!(k.place.join("fix.txt").exists());
    let _ = k.child.kill();
}
