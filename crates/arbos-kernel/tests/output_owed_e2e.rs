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
