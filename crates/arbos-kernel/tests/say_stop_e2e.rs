//! T3-09: `say mode=stop` ends a worker's turn now and keeps what it had:
//! the turn is interrupted with the parent's words, its job dies, and the
//! done message carries the last words before the stop.

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
        std::thread::sleep(Duration::from_millis(200));
    }
    ok()
}

#[test]
fn a_parent_stops_a_worker_and_gets_its_partial_result() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"one worker\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"w1\",\"task\":\"count slowly\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"started\"}\n",
        "{\"agent\":\"w1\",\"content\":\"so far: alpha, beta\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 40\",\"wait_ms\":60000}}]}\n",
        "{\"agent\":\"w1\",\"content\":\"never reached\"}\n",
        "{\"agent\":\"root\",\"content\":\"stopping it\",\"calls\":[{\"name\":\"say\",\"arguments\":{\"to\":\"w1\",\"mode\":\"stop\",\"text\":\"enough, report what you have\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"stop sent\"}\n",
        "{\"agent\":\"root\",\"content\":\"got the partial result\"}\n",
    );
    let mut k = start_kernel_replay_prepared("say-stop", replies, "", |place| {
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
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "start a slow worker"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    // The worker is mid-turn: its bash step (a 40 s sleep) is a running
    // job. Its words so far are streamed, not yet on the transcript — the
    // stop is what writes them.
    let jobs = k.place.join(".arbos/agents/w1/jobs");
    assert!(
        wait_for(Duration::from_secs(30), || {
            std::fs::read_dir(&jobs)
                .map(|rd| rd.flatten().any(|e| e.path().join("out.log").exists()))
                .unwrap_or(false)
        }),
        "the worker's job started"
    );
    std::thread::sleep(Duration::from_millis(500));
    let stopped_at = Instant::now();
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "stop the worker"}));

    // The worker's turn ends soon, interrupted, with the parent's words.
    assert!(
        wait_for(Duration::from_secs(20), || {
            transcript(&k.place, "w1")
                .iter()
                .any(|e| e["kind"] == "interrupted")
        }),
        "the worker's turn was interrupted: {:?}",
        transcript(&k.place, "w1")
    );
    assert!(
        stopped_at.elapsed() < Duration::from_secs(20),
        "not the 40 s sleep"
    );
    let w1 = transcript(&k.place, "w1");
    let interrupted = w1.iter().find(|e| e["kind"] == "interrupted").unwrap();
    assert!(
        interrupted["detail"]
            .as_str()
            .unwrap_or("")
            .contains("stopped by root: enough, report what you have"),
        "{interrupted}"
    );
    assert!(!w1.iter().any(|e| e["text"] == "never reached"));
    assert!(
        w1.iter()
            .any(|e| e["kind"] == "assistant" && e["text"] == "so far: alpha, beta"),
        "the words before the stop are kept: {w1:?}"
    );

    // Root hears the done with the partial result.
    assert!(
        wait_for(Duration::from_secs(30), || {
            transcript(&k.place, "root").iter().any(|e| {
                e["text"]
                    .as_str()
                    .unwrap_or("")
                    .contains("Last words before the stop: so far: alpha, beta")
            })
        }),
        "the done carries the partial result: {:?}",
        transcript(&k.place, "root")
    );
    let root = transcript(&k.place, "root");
    let say = root
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "say")
        .expect("say tool line");
    assert!(
        say["body"].as_str().unwrap_or("").starts_with("Stopping"),
        "{say:#?}"
    );
    let log =
        std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.log")).unwrap_or_default();
    assert!(log.contains("turn_stopped_by_parent"), "{log}");
    let _ = k.child.kill();
}
