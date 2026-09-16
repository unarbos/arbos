//! T3-01: a goal is an objective held until met. Its check runs on a
//! schedule; while it fails the agent is woken with the goal and the
//! check's output; when it passes the goal closes and the user is told.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::path::Path;
use std::time::{Duration, Instant};

fn transcript(place: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn goal_files(place: &Path) -> Vec<String> {
    std::fs::read_dir(place.join(".arbos/agents/root/subscriptions"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter(|t| t.contains("kind = \"goal\""))
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
fn a_goal_wakes_the_agent_while_its_check_fails_and_closes_when_it_passes() {
    // The check: a marker file exists. The agent's second turn creates it.
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"setting a goal\",\"calls\":[{\"name\":\"subscribe\",\"arguments\":{\"op\":\"add\",\"kind\":\"goal\",\"prompt\":\"the marker file exists\",\"cmd\":\"test -f marker.txt\",\"every\":\"30s\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"goal set\"}\n",
        "{\"agent\":\"root\",\"content\":\"working on it\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"touch marker.txt\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"made the marker\"}\n",
        "{\"agent\":\"root\",\"content\":\"the goal is met\"}\n",
    );
    let mut k = start_kernel_replay_prepared("goal", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\nname = \"g\"\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(
        serde_json::json!({"type": "user", "agent": "root", "text": "keep a marker file around"}),
    );
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert!(
        wait_for(Duration::from_secs(10), || !goal_files(&k.place).is_empty()),
        "the goal is on disk"
    );
    let file = goal_files(&k.place).remove(0);
    assert!(file.contains("cmd = \"test -f marker.txt\""), "{file}");

    // First check runs at once: not met → the agent is woken and works.
    assert!(
        wait_for(Duration::from_secs(40), || {
            transcript(&k.place).iter().any(|e| {
                e["text"]
                    .as_str()
                    .unwrap_or("")
                    .contains("Goal not yet met: the marker file exists")
            })
        }),
        "the failing check woke the agent: {:?}",
        transcript(&k.place)
    );
    assert!(wait_for(Duration::from_secs(30), || k
        .place
        .join("marker.txt")
        .exists()));

    // The next check passes: the goal closes, the user is told. The kernel
    // removes the goal's file, says the line, and writes the `Goal met`
    // transcript line as separate steps; a reader that stops at "the file is
    // gone" can see the transcript a step early (the CI flake of 2026-09-16:
    // the `say` was there, the `Goal met` line was not yet). So each thing is
    // waited for on its own, not inferred from the one before.
    let has_text = |needle: &str| {
        transcript(&k.place)
            .iter()
            .any(|e| e["text"].as_str().unwrap_or("").contains(needle))
    };
    assert!(
        wait_for(Duration::from_secs(70), || goal_files(&k.place).is_empty()),
        "the goal closes once its check passes: {:?}",
        goal_files(&k.place)
    );
    assert!(
        wait_for(Duration::from_secs(10), || has_text(
            "Goal met: the marker file exists"
        )),
        "the transcript records the goal as met: {:?}",
        transcript(&k.place)
    );
    assert!(
        wait_for(Duration::from_secs(10), || transcript(&k.place).iter().any(
            |e| e["kind"] == "say" && e["text"].as_str().unwrap_or("").contains("goal met")
        )),
        "the user hears it as a line: {:?}",
        transcript(&k.place)
    );
    assert!(
        wait_for(Duration::from_secs(10), || std::fs::read_to_string(
            k.place.join(".arbos/runtime/kernel.log")
        )
        .unwrap_or_default()
        .contains("goal_met")),
        "kernel.log records goal_met: {}",
        std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.log")).unwrap_or_default()
    );
    let _ = k.child.kill();
}
