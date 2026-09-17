//! Process parity, slice 2: Cursor's typed inline helpers and its "one
//! coordinator child per area" rule. `spawn kind=explore` runs a read-only
//! helper inline — the spawn call returns its answer — and the helper has
//! no write tools. `spawn role=coordinator` makes an area coordinator that
//! keeps the coordinator's tools, runs its own worker, and reports once.

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

fn coordinator_place(place: &Path) {
    std::fs::create_dir_all(place.join(".arbos")).unwrap();
    std::fs::write(
        place.join(".arbos/project.toml"),
        "schema = 2\n[root]\nrole = \"coordinator\"\narchive_children = false\n",
    )
    .unwrap();
    std::fs::write(place.join("answer.txt"), "the port is 4242\n").unwrap();
}

#[test]
fn an_explore_helper_runs_inline_read_only_and_its_answer_is_the_spawn_result() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"asking\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"kind\":\"explore\",\"name\":\"Find the port\",\"task\":\"which port does answer.txt name?\"}}]}\n",
        "{\"agent\":\"find-the-port\",\"content\":\"looking\",\"calls\":[{\"name\":\"grep\",\"arguments\":{\"pattern\":\"port\",\"path\":\"answer.txt\"}},{\"name\":\"write\",\"arguments\":{\"path\":\"scratch.txt\",\"content\":\"x\"}}]}\n",
        "{\"agent\":\"find-the-port\",\"content\":\"answer.txt:1 names port 4242\"}\n",
        "{\"agent\":\"root\",\"content\":\"It is 4242.\"}\n",
        "{\"agent\":\"root\",\"content\":\"noted\"}\n",
    );
    let mut k = start_kernel_replay_prepared("helper-explore", replies, "", coordinator_place);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "which port does answer.txt name?"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(60)));
    let root = transcript(&k.place, "root");
    let spawn = root
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "spawn")
        .expect("spawn on root's transcript");
    let body = spawn["body"].as_str().unwrap_or("");
    assert!(
        body.contains("find-the-port reports:") && body.contains("port 4242"),
        "an inline helper's answer is the spawn result: {body}"
    );
    // The helper is read-only: grep ran, write was refused.
    let helper = transcript(&k.place, "find-the-port");
    let grep = helper
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "grep")
        .expect("grep ran");
    assert!(grep.get("error").is_none(), "{grep:#?}");
    let write = helper
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "write")
        .expect("write was attempted");
    assert!(
        write.get("error").is_some(),
        "an explore helper cannot write: {write:#?}"
    );
    assert!(!k.place.join("scratch.txt").exists());
    let agent_md =
        std::fs::read_to_string(k.place.join(".arbos/agents/find-the-port/agent.md")).unwrap();
    assert!(agent_md.contains("kind: explore"), "{agent_md}");
    assert!(agent_md.contains("readonly: true"), "{agent_md}");
    assert!(agent_md.contains("role: worker"), "{agent_md}");
    let _ = k.child.kill();
}

#[test]
fn an_area_coordinator_child_runs_its_own_worker_and_reports_once() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"one area\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"role\":\"coordinator\",\"name\":\"Run the voice area\",\"task\":\"own the voice area\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"area started\"}\n",
        "{\"agent\":\"run-the-voice-area\",\"content\":\"splitting\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"Write the echo gate\",\"task\":\"write it\"}},{\"name\":\"bash\",\"arguments\":{\"command\":\"echo hi > forbidden.txt\"}}]}\n",
        "{\"agent\":\"run-the-voice-area\",\"content\":\"worker running\"}\n",
        "{\"agent\":\"write-the-echo-gate\",\"content\":\"gate written\"}\n",
        "{\"agent\":\"run-the-voice-area\",\"content\":\"Voice area done: the echo gate is written.\"}\n",
        "{\"agent\":\"root\",\"content\":\"noted the area\"}\n",
        "{\"agent\":\"root\",\"content\":\"noted\"}\n",
    );
    let mut k = start_kernel_replay_prepared("helper-area", replies, "", coordinator_place);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "run the voice area"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(60)));
    let area = "run-the-voice-area";
    // The worker's report reaches the area coordinator once. Whether it
    // opens a second turn or folds into the first depends on whether the
    // worker finishes before the area's own turn ends — a loaded runner
    // decides that, so the turn count is not the fact to wait for.
    assert!(
        common::wait_for(Duration::from_secs(60), || {
            let t = transcript(&k.place, area);
            t.iter()
                .any(|e| e["kind"] == "say" && e["from"] == "write-the-echo-gate")
                && t.last().is_some_and(|e| e["kind"] == "turn_complete")
        }),
        "the worker's report reached the area coordinator, and the turn holding it ended: {:?}",
        transcript(&k.place, area)
    );
    let area_t = transcript(&k.place, area);
    assert_eq!(
        area_t
            .iter()
            .filter(|e| e["kind"] == "say" && e["from"] == "write-the-echo-gate")
            .count(),
        1,
        "once, not once per turn: {area_t:?}"
    );
    let agent_md =
        std::fs::read_to_string(k.place.join(".arbos/agents").join(area).join("agent.md")).unwrap();
    assert!(
        agent_md.contains("role: coordinator"),
        "saved on disk: {agent_md}"
    );
    // Its own worker exists under it; its bash ran only as the coordinator's
    // quick command would (the file is not in the store, so the write guard
    // is bash's affair — but the tool set is the coordinator's).
    let worker_md =
        std::fs::read_to_string(k.place.join(".arbos/agents/write-the-echo-gate/agent.md"))
            .expect("the area spawned its own worker");
    assert!(
        worker_md.contains(&format!("parent: {area}")),
        "{worker_md}"
    );
    let place = arbos_core::Place::new(&k.place);
    let mut agent = arbos_core::Agent::load(&place.agent_dir(area)).unwrap();
    arbos_core::project::apply_role(&place, &mut agent);
    assert!(agent.may("spawn") && agent.may("say") && agent.may("plan"));
    assert!(
        !agent.may("undo"),
        "the coordinator tool set, as root has it"
    );
    // The worker's done went to the area coordinator, not to root.
    let area_t = transcript(&k.place, area);
    assert!(
        area_t
            .iter()
            .any(|e| e["text"].as_str().unwrap_or("").contains("gate written")),
        "the worker's done reached its parent, the area: {area_t:?}"
    );
    let root = transcript(&k.place, "root");
    assert!(
        !root
            .iter()
            .any(|e| e["text"].as_str().unwrap_or("").contains("gate written")),
        "root never hears the area's interim done: {root:?}"
    );
    assert!(
        common::wait_for(Duration::from_secs(30), || {
            transcript(&k.place, "root")
                .iter()
                .any(|e| e["text"].as_str().unwrap_or("").contains("Voice area done"))
        }),
        "root hears the area once, with the combined result"
    );
    let _ = k.child.kill();
}
