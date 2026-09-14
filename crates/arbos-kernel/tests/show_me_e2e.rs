//! Kickoff item 3: "run it and show me" reached the worker as "run … and
//! report", and no image was made. The user's ask to see travels with
//! the brief now, judged on their own words for the turn that spawned.

mod common;

use common::{Attach, start_kernel_replay};
use std::path::Path;
use std::time::{Duration, Instant};

fn brief_of(place: &Path, agent: &str, timeout: Duration) -> String {
    let path = place
        .join(".arbos")
        .join("agents")
        .join(agent)
        .join("transcript.jsonl");
    let start = Instant::now();
    loop {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let brief = text
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            // The brief is the child's first wake (its text) — or a user
            // line on kernels that deliver it that way.
            .find(|e| (e["kind"] == "wake" || e["kind"] == "user") && e["text"].is_string())
            .and_then(|e| e["text"].as_str().map(str::to_owned));
        if let Some(b) = brief {
            return b;
        }
        if start.elapsed() > timeout {
            panic!("no brief reached {agent}");
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

fn run(name: &str, user: &str, task: &str) -> String {
    let replies = format!(
        concat!(
            "{{\"agent\":\"root\",\"content\":\"one worker\",\"calls\":[{{\"name\":\"spawn\",\"arguments\":{{\"name\":\"w1\",\"task\":\"{task}\"}}}}]}}\n",
            "{{\"agent\":\"root\",\"content\":\"started\"}}\n",
            "{{\"content\":\"done\"}}\n",
        ),
        task = task
    );
    let mut k = start_kernel_replay(name, &replies);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": user}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let brief = brief_of(&k.place, "w1", Duration::from_secs(20));
    let _ = k.child.kill();
    brief
}

#[test]
fn show_me_travels_with_the_brief() {
    let brief = run(
        "show-me-yes",
        "run toy-repo/hello.py, fix what breaks, and show me the output",
        "Run hello.py in toy-repo, fix the error, report the output.",
    );
    assert!(
        brief.contains("Show: The user asked to see this"),
        "{brief}"
    );
    assert!(brief.contains(".arbos/media/<topic>/"), "{brief}");
    // Where the line sits: after Output, before Report.
    let show = brief.find("Show:").unwrap();
    assert!(brief.find("Output:").unwrap() < show && show < brief.find("Report:").unwrap());
}

#[test]
fn a_plain_ask_adds_no_show_line() {
    let brief = run(
        "show-me-no",
        "run toy-repo/hello.py and fix what breaks",
        "Run hello.py in toy-repo, fix the error, report the output.",
    );
    assert!(!brief.contains("Show:"), "{brief}");
}
