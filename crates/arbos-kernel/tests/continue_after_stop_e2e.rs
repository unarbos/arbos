//! F-96 (journey J09): after Stop, the coordinator woke on its workers'
//! "ended badly — stopped" reports, called the stop a failure and spawned
//! nothing. A stop is the user pausing: a stopped worker's report reads
//! "stopped by the user", is a note (no wake), and rides into the user's
//! next line ("Continue where you stopped") so the coordinator picks the
//! work back up.

mod common;

use common::{Attach, start_kernel_replay};
use std::time::Duration;

fn transcript(place: &std::path::Path, agent: &str) -> Vec<serde_json::Value> {
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
fn stopped_workers_report_a_pause_and_continue_carries_them_in() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"two workers\",\"calls\":[",
        "{\"name\":\"spawn\",\"arguments\":{\"name\":\"alpha\",\"task\":\"Do: bash `sleep 30; echo a`. Report done.\"}},",
        "{\"name\":\"spawn\",\"arguments\":{\"name\":\"beta\",\"task\":\"Do: bash `sleep 30; echo b`. Report done.\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"Both are working; I will report when they finish.\"}\n",
        "{\"agent\":\"alpha\",\"content\":\"working on a\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 30; echo a\",\"description\":\"Slow a\"}}]}\n",
        "{\"agent\":\"beta\",\"content\":\"working on b\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 30; echo b\",\"description\":\"Slow b\"}}]}\n",
        // root, on "Continue where you stopped." — resumes.
        "{\"agent\":\"root\",\"content\":\"Resuming: both workers were paused by your stop; picking the work back up.\"}\n",
    );
    let mut k = start_kernel_replay("continue-after-stop", replies);
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(serde_json::json!({"type":"user","agent":"root","text":"do both slow things","attachments":[]}));
    assert!(a.wait_turn("alpha", "running", Duration::from_secs(15)));
    assert!(a.wait_turn("beta", "running", Duration::from_secs(15)));
    std::thread::sleep(Duration::from_millis(1200));

    // The user's Stop, on the coordinator: the whole tree stops.
    a.send(serde_json::json!({"type":"stop","agent":"root"}));
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let done = ["root", "alpha", "beta"].iter().all(|id| {
            transcript(&k.place, id)
                .iter()
                .any(|e| e["kind"] == "turn_complete")
        });
        if done {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "everything stops");
        std::thread::sleep(Duration::from_millis(100));
    }
    for w in ["alpha", "beta"] {
        let t = transcript(&k.place, w);
        let stop = t
            .iter()
            .find(|e| e["kind"] == "interrupted")
            .unwrap_or_else(|| panic!("{w} was interrupted: {t:?}"));
        assert!(
            stop["detail"].as_str().unwrap().starts_with("stop"),
            "the desktop's 'Stopped by you' keys on this: {stop}"
        );
    }

    // The reports are notes in root's inbox — not wakes: root does not
    // run a turn to mourn them.
    std::thread::sleep(Duration::from_millis(2500));
    let inbox = k.place.join(".arbos/agents/root/inbox");
    let files: Vec<String> = std::fs::read_dir(&inbox)
        .unwrap()
        .flatten()
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .collect();
    assert_eq!(files.len(), 2, "one report per stopped worker: {files:?}");
    for f in &files {
        assert!(f.contains("wake = false"), "a note, not a wake: {f}");
        assert!(f.contains("Turn stopped by the user."), "{f}");
        assert!(f.contains("stopped by the user (Stop)"), "{f}");
        assert!(!f.contains("ended badly"), "{f}");
        assert!(f.contains("mode=request"), "says how to resume: {f}");
    }
    let root = transcript(&k.place, "root");
    assert_eq!(
        root.iter().filter(|e| e["kind"] == "turn_complete").count(),
        1,
        "root ran no turn on the stop reports: {root:?}"
    );
    for w in ["alpha", "beta"] {
        assert!(
            k.place
                .join(".arbos/agents")
                .join(w)
                .join("agent.md")
                .exists(),
            "{w} is still there to resume"
        );
    }

    // Continue Working: the user's line carries the reports in, and the
    // coordinator resumes.
    a.send(serde_json::json!({"type":"user","agent":"root","text":"Continue where you stopped.","attachments":[]}));
    let reply = a
        .wait(Duration::from_secs(20), |f| {
            f["type"] == "event"
                && f["agent"] == "root"
                && f["event"]["kind"] == "assistant"
                && f["event"]["text"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("Resuming"))
        })
        .expect("root resumes");
    let _ = reply;
    let root = transcript(&k.place, "root");
    let user_at = root
        .iter()
        .position(|e| e["kind"] == "user" && e["text"] == "Continue where you stopped.")
        .unwrap();
    let says: Vec<usize> = root
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            e["kind"] == "say"
                && e["text"]
                    .as_str()
                    .is_some_and(|t| t.contains("stopped by the user"))
        })
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        says.len(),
        2,
        "both reports are on root's transcript: {root:?}"
    );
    let prev_end = root[..user_at]
        .iter()
        .rposition(|e| e["kind"] == "turn_complete")
        .unwrap();
    assert!(
        says.iter().all(|&i| i > prev_end && i < user_at),
        "the reports ride into the user's turn, not a turn of their own: says {says:?}, previous end {prev_end}, user {user_at}"
    );
    assert!(
        std::fs::read_dir(&inbox).unwrap().count() == 0,
        "the notes were taken"
    );
    let _ = k.child.kill();
}
