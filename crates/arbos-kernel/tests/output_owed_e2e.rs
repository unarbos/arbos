//! A research worker answered in its last words and never wrote the
//! `research.md` its brief named as Output (kickoff, 2026-09-15). The
//! brief's `Output:` paths are owed: a turn about to end with one missing
//! gets one nudge, the same way as an owed image; a worker that wrote the
//! file is not nudged.

mod common;

use common::{Attach, start_kernel_replay_prepared};
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

fn wait_for(timeout: Duration, mut ok: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    ok()
}

fn run(name: &str, worker: &str) -> common::Kernel {
    let replies = format!(
        concat!(
            "{{\"agent\":\"root\",\"content\":\"one worker\",\"calls\":[{{\"name\":\"spawn\",\"arguments\":{{\"name\":\"w1\",\"task\":\"research full duplex voice agents\",\"output\":\".arbos/docs/research.md\"}}}}]}}\n",
            "{{\"agent\":\"root\",\"content\":\"started\"}}\n",
            "{worker}",
            "{{\"agent\":\"root\",\"content\":\"noted\"}}\n",
        ),
        worker = worker
    );
    let mut k = start_kernel_replay_prepared(name, &replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"coordinator\"\narchive_children = false\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "research it and write research.md"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert!(
        wait_for(Duration::from_secs(40), || transcript(&k.place, "w1")
            .iter()
            .any(|e| e["kind"] == "turn_complete")),
        "w1's turn ends"
    );
    k
}

#[test]
fn a_worker_that_answers_without_the_output_file_is_nudged_once_and_writes_it() {
    let worker = concat!(
        "{\"content\":\"A full duplex voice agent listens and speaks at once. Sources: example.org.\"}\n",
        "{\"content\":\"writing it\",\"calls\":[{\"name\":\"write\",\"arguments\":{\"path\":\".arbos/docs/research.md\",\"contents\":\"# Full duplex\\n\\nListens and speaks at once.\\n\"}}]}\n",
        "{\"content\":\"done; docs/research.md\"}\n",
    );
    let mut k = run("output-owed", worker);
    let w1 = transcript(&k.place, "w1");
    let nudges: Vec<usize> = w1
        .iter()
        .enumerate()
        .filter(|(_, e)| e["kind"] == "nudge")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(nudges.len(), 1, "{w1:#?}");
    let nudge = &w1[nudges[0]];
    assert_eq!(nudge["reason"], "output owed");
    let text = nudge["text"].as_str().unwrap_or("");
    assert!(
        text.contains(".arbos/docs/research.md") && text.contains("write the file now"),
        "{text}"
    );
    let write_at = w1
        .iter()
        .position(|e| e["kind"] == "tool" && e["name"] == "write")
        .expect("the write after the nudge");
    assert!(write_at > nudges[0]);
    assert!(w1[write_at].get("error").is_none(), "{:#?}", w1[write_at]);
    assert_eq!(
        std::fs::read_to_string(k.place.join(".arbos/docs/research.md")).unwrap(),
        "# Full duplex\n\nListens and speaks at once.\n"
    );
    let _ = k.child.kill();
}

#[test]
fn a_worker_that_wrote_the_output_file_is_not_nudged() {
    let worker = concat!(
        "{\"content\":\"writing\",\"calls\":[{\"name\":\"write\",\"arguments\":{\"path\":\"docs/research.md\",\"contents\":\"# Full duplex\\n\"}}]}\n",
        "{\"content\":\"done; docs/research.md\"}\n",
    );
    let mut k = run("output-written", worker);
    let w1 = transcript(&k.place, "w1");
    assert!(
        !w1.iter().any(|e| e["kind"] == "nudge"),
        "no nudge when the file exists (written by its spoken name): {w1:#?}"
    );
    assert!(k.place.join(".arbos/docs/research.md").exists());
    let _ = k.child.kill();
}

/// qal-j04: the brief says `Output: .arbos/docs/CHANGELOG.md`; the worker
/// writes `CHANGELOG.md` at the project root, where the user asked for
/// it. That is delivered — no nudge. And a worker that is nudged anyway
/// (an older path) may not move or delete the file to match the brief:
/// the bash is refused and the file stays.
#[test]
fn a_deliverable_written_where_the_task_put_it_is_delivered_and_never_moved() {
    // Delivered by name at the repo root: no nudge, the file stays.
    let worker = concat!(
        "{\"content\":\"writing\",\"calls\":[{\"name\":\"write\",\"arguments\":{\"path\":\"CHANGELOG.md\",\"contents\":\"## Fix\\n- area() corrected\\n\"}}]}\n",
        "{\"content\":\"done; CHANGELOG.md at the repo root\"}\n",
    );
    let replies = format!(
        concat!(
            "{{\"agent\":\"root\",\"content\":\"one worker\",\"calls\":[{{\"name\":\"spawn\",\"arguments\":{{\"name\":\"w1\",\"task\":\"add a CHANGELOG.md at the repo root describing the fix\",\"output\":\".arbos/docs/CHANGELOG.md\"}}}}]}}\n",
            "{{\"agent\":\"root\",\"content\":\"started\"}}\n",
            "{worker}",
            "{{\"agent\":\"root\",\"content\":\"noted\"}}\n",
        ),
        worker = worker
    );
    let mut k = start_kernel_replay_prepared("output-delivered-elsewhere", &replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"coordinator\"\narchive_children = false\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "add a changelog"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert!(
        wait_for(Duration::from_secs(40), || transcript(&k.place, "w1")
            .iter()
            .any(|e| e["kind"] == "turn_complete")),
        "w1's turn ends"
    );
    let w1 = transcript(&k.place, "w1");
    assert!(
        !w1.iter()
            .any(|e| e["kind"] == "nudge" && e["reason"] == "output owed"),
        "the file exists where the task put it: {w1:#?}"
    );
    assert!(k.place.join("CHANGELOG.md").exists());
    assert!(
        !k.place.join(".arbos/docs/CHANGELOG.md").exists(),
        "not copied into the store"
    );
    let _ = k.child.kill();

    // Nudged anyway (the file was written under a different name), the
    // worker tries to move the user's file to the brief's path: refused;
    // the file stays; the worker reports the real path.
    let worker = concat!(
        "{\"content\":\"writing\",\"calls\":[{\"name\":\"write\",\"arguments\":{\"path\":\"CHANGES.md\",\"contents\":\"## Fix\\n\"}}]}\n",
        "{\"content\":\"done; CHANGES.md\"}\n",
        "{\"content\":\"moving it\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"mkdir -p .arbos/docs && mv CHANGES.md .arbos/docs/CHANGELOG.md\",\"description\":\"Move to the brief's path\"}}]}\n",
        "{\"content\":\"CHANGES.md stays at the repo root; that is the changelog.\"}\n",
    );
    let replies = format!(
        concat!(
            "{{\"agent\":\"root\",\"content\":\"one worker\",\"calls\":[{{\"name\":\"spawn\",\"arguments\":{{\"name\":\"w1\",\"task\":\"add a changelog\",\"output\":\".arbos/docs/CHANGELOG.md\"}}}}]}}\n",
            "{{\"agent\":\"root\",\"content\":\"started\"}}\n",
            "{worker}",
            "{{\"agent\":\"root\",\"content\":\"noted\"}}\n",
        ),
        worker = worker
    );
    let mut k = start_kernel_replay_prepared("output-never-moved", &replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"coordinator\"\narchive_children = false\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "add a changelog"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert!(
        wait_for(Duration::from_secs(40), || transcript(&k.place, "w1")
            .iter()
            .any(|e| e["kind"] == "turn_complete")),
        "w1's turn ends"
    );
    let w1 = transcript(&k.place, "w1");
    let nudge = w1
        .iter()
        .find(|e| e["kind"] == "nudge" && e["reason"] == "output owed")
        .expect("nudged: a different name");
    assert!(
        nudge["text"]
            .as_str()
            .unwrap()
            .contains("never move or delete a file"),
        "{nudge}"
    );
    let mv = w1
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "bash")
        .expect("the move was attempted");
    let err = mv["error"].as_str().expect("and refused");
    assert!(
        err.contains("refused")
            && err.contains("CHANGES.md")
            && err.contains("never a reason to relocate"),
        "{err}"
    );
    assert!(k.place.join("CHANGES.md").exists(), "the user's file stays");
    assert!(!k.place.join(".arbos/docs/CHANGELOG.md").exists());
    let _ = k.child.kill();
}
