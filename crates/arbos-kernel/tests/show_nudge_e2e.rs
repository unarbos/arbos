//! Kickoff item 3, the last mile: with the Show line in its brief the
//! worker made the image two runs in five. Now the image is a numbered
//! step of the brief's `Do` list, and a turn that ends with the image
//! still owed gets one nudge before its report goes out.

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

fn has_chrome() -> bool {
    [
        "chromium",
        "google-chrome",
        "chromium-browser",
        "google-chrome-stable",
    ]
    .iter()
    .any(|b| which::which(b).is_ok())
}

fn start(name: &str, worker_replies: &str) -> common::Kernel {
    let replies = format!(
        concat!(
            "{{\"agent\":\"root\",\"content\":\"one worker\",\"calls\":[{{\"name\":\"spawn\",\"arguments\":{{\"name\":\"w1\",\"task\":\"run hello.py\",\"do\":\"1. Run python3 hello.py.\\n2. Capture output.\"}}}}]}}\n",
            "{{\"agent\":\"root\",\"content\":\"started\"}}\n",
            "{worker}",
            "{{\"agent\":\"root\",\"content\":\"noted\"}}\n",
        ),
        worker = worker_replies
    );
    start_kernel_replay_prepared(name, &replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"coordinator\"\narchive_children = false\n",
        )
        .unwrap();
    })
}

fn drive(k: &mut common::Kernel) {
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "run hello.py and show me the output"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert!(
        wait_for(Duration::from_secs(40), || transcript(&k.place, "w1")
            .iter()
            .any(|e| e["kind"] == "turn_complete")),
        "w1's turn ends"
    );
}

#[test]
fn a_worker_that_ends_without_the_image_is_nudged_once_and_then_makes_it() {
    if !has_chrome() {
        eprintln!("no chrome here; skipping");
        return;
    }
    // The worker reports in words; nudged, it renders the output.
    let worker = concat!(
        "{\"content\":\"hello.py printed: hello\"}\n",
        "{\"content\":\"rendering it\",\"calls\":[{\"name\":\"screenshot\",\"arguments\":{\"target\":\"text\",\"title\":\"$ python3 hello.py\",\"text\":\"hello\"}}]}\n",
        "{\"content\":\"done; the image is in images/\"}\n",
    );
    let mut k = start("show-nudge", worker);
    drive(&mut k);
    let w1 = transcript(&k.place, "w1");
    // The brief: the image is step 3, after the two given.
    let brief = w1
        .iter()
        .find(|e| e["kind"] == "wake" && e["text"].is_string())
        .and_then(|e| e["text"].as_str())
        .unwrap_or("");
    assert!(
        brief.contains("  2. Capture output.\n  3. The user asked to see the result"),
        "{brief}"
    );
    assert!(brief.contains("screenshot target:\"text\""), "{brief}");
    assert!(
        brief.contains("Show: The user asked to see this"),
        "{brief}"
    );
    // One nudge, then the image, then the turn ends.
    let nudges: Vec<&serde_json::Value> = w1.iter().filter(|e| e["kind"] == "nudge").collect();
    assert_eq!(nudges.len(), 1, "{w1:#?}");
    assert_eq!(nudges[0]["reason"], "image owed");
    assert!(
        nudges[0]["text"]
            .as_str()
            .unwrap_or("")
            .contains("screenshot target:\"text\""),
        "{:?}",
        nudges[0]
    );
    let shot_at = w1
        .iter()
        .position(|e| e["kind"] == "tool" && e["name"] == "screenshot")
        .expect("the screenshot call after the nudge");
    assert!(w1[shot_at].get("error").is_none(), "{:#?}", w1[shot_at]);
    let nudge_at = w1.iter().position(|e| e["kind"] == "nudge").unwrap();
    assert!(shot_at > nudge_at, "the image came after the nudge");
    let images = std::fs::read_dir(k.place.join(".arbos/agents/w1/images"))
        .map(|d| d.count())
        .unwrap_or(0);
    assert_eq!(images, 1);
    let _ = k.child.kill();
}

#[test]
fn a_worker_that_makes_the_image_is_not_nudged() {
    if !has_chrome() {
        eprintln!("no chrome here; skipping");
        return;
    }
    let worker = concat!(
        "{\"content\":\"rendering\",\"calls\":[{\"name\":\"screenshot\",\"arguments\":{\"target\":\"text\",\"title\":\"$ python3 hello.py\",\"text\":\"hello\"}}]}\n",
        "{\"content\":\"done; see images/\"}\n",
    );
    let mut k = start("show-no-nudge", worker);
    drive(&mut k);
    let w1 = transcript(&k.place, "w1");
    assert!(
        !w1.iter().any(|e| e["kind"] == "nudge"),
        "no nudge when the image was made: {w1:#?}"
    );
    let _ = k.child.kill();
}
