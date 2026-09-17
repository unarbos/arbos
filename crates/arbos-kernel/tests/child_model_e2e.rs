//! T3-05: `child_model` in config.toml pins every spawned child to one
//! model, whatever the spawn call asked; root keeps its own.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::time::{Duration, Instant};

#[test]
fn child_model_pins_the_workers_model_and_leaves_root_alone() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"one worker\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"w1\",\"task\":\"say a word\",\"model\":\"expensive/model\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"started\"}\n",
        "{\"content\":\"word\"}\n",
    );
    let mut k = start_kernel_replay_prepared(
        "child-model",
        replies,
        "child_model = \"cheap/model\"\n",
        |place| {
            std::fs::create_dir_all(place.join(".arbos")).unwrap();
            std::fs::write(
                place.join(".arbos/project.toml"),
                "schema = 2\n[root]\nrole = \"coordinator\"\narchive_children = false\n",
            )
            .unwrap();
        },
    );
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "start a worker"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let log_path = k.place.join(".arbos/runtime/kernel.log");
    let start = Instant::now();
    let log = loop {
        let l = std::fs::read_to_string(&log_path).unwrap_or_default();
        if l.lines()
            .filter(|x| x.contains("prompt_size") && x.contains("\"agent\":\"w1\""))
            .count()
            >= 1
            || start.elapsed() > Duration::from_secs(30)
        {
            break l;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let w1_line = log
        .lines()
        .find(|x| x.contains("prompt_size") && x.contains("\"agent\":\"w1\""))
        .unwrap_or_else(|| panic!("{log}"));
    assert!(
        w1_line.contains("model=cheap/model"),
        "the worker runs on child_model: {w1_line}"
    );
    let root_line = log
        .lines()
        .find(|x| x.contains("prompt_size") && x.contains("\"agent\":\"root\""))
        .unwrap_or_else(|| panic!("{log}"));
    assert!(
        !root_line.contains("model=cheap/model"),
        "root keeps its own: {root_line}"
    );
    // The spawn call's choice is still on disk: lift child_model and it returns.
    let md = std::fs::read_to_string(k.place.join(".arbos/agents/w1/agent.md")).unwrap();
    assert!(md.contains("model: expensive/model"), "{md}");
    let _ = k.child.kill();
}
