//! Process parity, slice 6: Cursor's "verify an artifact before showing
//! it". When root's reply links a file that does not exist, the kernel
//! opens one more turn for root with the missing paths; a reply that links
//! only real files (or workers, or URLs) passes untouched; the kernel's
//! own note never re-triggers itself.

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

#[test]
fn a_reply_that_links_a_missing_file_wakes_root_once_to_fix_it() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"The panel is up: ![panel](media/layout/panel.png) and the notes are in [the design](docs/design.md); worker [poem](agents/poem) is on it, see also https://example.com/x.\"}\n",
        "{\"agent\":\"root\",\"content\":\"The screenshot is not ready yet; the design is at [the design](docs/design.md).\"}\n",
        "{\"agent\":\"root\",\"content\":\"never reached\"}\n",
    );
    let mut k = start_kernel_replay_prepared("reply-links", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos/docs")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"coordinator\"\n",
        )
        .unwrap();
        std::fs::write(place.join(".arbos/docs/design.md"), "# design\n").unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "show me the panel"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    // The kernel's note opens a second turn; the reply fixes itself.
    assert!(
        common::wait_for(Duration::from_secs(30), || {
            transcript(&k.place, "root")
                .iter()
                .filter(|e| e["kind"] == "turn_complete")
                .count()
                >= 2
        }),
        "a second turn ran: {:?}",
        transcript(&k.place, "root")
    );
    // No third: the corrected reply links only a real file, and the
    // kernel's own note never re-triggers.
    std::thread::sleep(Duration::from_secs(3));
    let root = transcript(&k.place, "root");
    assert_eq!(
        root.iter().filter(|e| e["kind"] == "turn_complete").count(),
        2,
        "{root:?}"
    );
    let note = root
        .iter()
        .find(|e| {
            e["kind"] == "wake"
                && e["text"]
                    .as_str()
                    .unwrap_or("")
                    .starts_with("[kernel] your reply links files that do not exist")
        })
        .expect("the kernel's note opened the turn");
    let text = note["text"].as_str().unwrap();
    assert!(text.contains("`media/layout/panel.png`"), "{text}");
    assert!(
        !text.contains("design.md"),
        "an existing file is not named: {text}"
    );
    assert!(
        !text.contains("agents/poem"),
        "a worker link is not a file: {text}"
    );
    assert!(
        !text.contains("example.com"),
        "a URL is not checked: {text}"
    );
    assert!(
        root.iter()
            .any(|e| e["kind"] == "assistant" && e["text"] == "The screenshot is not ready yet; the design is at [the design](docs/design.md)."),
        "{root:?}"
    );
    assert!(!root.iter().any(|e| e["text"] == "never reached"));
    let log =
        std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.log")).unwrap_or_default();
    assert!(log.contains("reply_links_missing"), "{log}");
    let _ = k.child.kill();
}

#[test]
fn a_reply_with_only_real_links_is_not_questioned_and_workers_are_not_checked() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"one worker\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"Note the design\",\"task\":\"note it\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"Started; the design is [here](docs/design.md).\"}\n",
        "{\"agent\":\"note-the-design\",\"content\":\"done, see [shot](media/nothing/here.png)\"}\n",
        "{\"agent\":\"root\",\"content\":\"noted the done\"}\n",
        "{\"agent\":\"root\",\"content\":\"never reached\"}\n",
    );
    let mut k = start_kernel_replay_prepared("reply-links-ok", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos/docs")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"coordinator\"\narchive_children = false\n",
        )
        .unwrap();
        std::fs::write(place.join(".arbos/docs/design.md"), "# design\n").unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "note the design"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    // Root's second turn is the worker's done, nothing else.
    assert!(
        common::wait_for(Duration::from_secs(30), || {
            transcript(&k.place, "root")
                .iter()
                .filter(|e| e["kind"] == "turn_complete")
                .count()
                >= 2
        }),
        "{:?}",
        transcript(&k.place, "root")
    );
    std::thread::sleep(Duration::from_secs(3));
    let root = transcript(&k.place, "root");
    assert_eq!(
        root.iter().filter(|e| e["kind"] == "turn_complete").count(),
        2,
        "{root:?}"
    );
    assert!(
        !root.iter().any(|e| e["text"]
            .as_str()
            .unwrap_or("")
            .starts_with("[kernel] your reply links")),
        "{root:?}"
    );
    // The worker's dead link is its parent's to judge, not the kernel's.
    let worker = transcript(&k.place, "note-the-design");
    assert!(
        !worker.iter().any(|e| e["text"]
            .as_str()
            .unwrap_or("")
            .starts_with("[kernel] your reply links")),
        "{worker:?}"
    );
    let _ = k.child.kill();
}
