//! qal-j37: a kernel killed in the middle of an append leaves the head of
//! a line with no newline. Every reader skips it — and the next kernel's
//! first append ran onto it, so that event (a `wake`, or the person's own
//! `user` line) was skipped too. Now the kernel cuts the headless line at
//! start, before anything appends, and says so on the transcript.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::time::Duration;

fn lines(place: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn unparseable(place: &std::path::Path) -> Vec<String> {
    lines(place)
        .into_iter()
        .filter(|l| !l.trim().is_empty() && serde_json::from_str::<serde_json::Value>(l).is_err())
        .collect()
}

#[test]
fn a_half_written_last_line_is_cut_at_start_and_the_next_turn_reads_whole() {
    let replies = "{\"agent\":\"root\",\"content\":\"after the crash\"}\n";
    let k = start_kernel_replay_prepared("headless-tail", replies, "", |place| {
        let dir = place.join(".arbos/agents/root");
        std::fs::create_dir_all(&dir).unwrap();
        arbos_core::Agent::root("root").save(&dir).unwrap();
        // Three whole events, then the head of a fourth with no newline:
        // what a SIGKILL mid-write leaves.
        let mut t = String::new();
        for (i, text) in ["one", "two", "three"].iter().enumerate() {
            t.push_str(&format!(
                "{{\"ts\":{},\"kind\":\"assistant\",\"text\":\"{text}\"}}\n",
                1_000_000 + i as i64
            ));
        }
        t.push_str("{\"ts\":1000003,\"kind\":\"assistant\",\"text\":\"hal");
        std::fs::write(dir.join("transcript.jsonl"), t).unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    // The cut happened before the first append: no headless line remains,
    // and the three whole events are still there.
    assert!(
        unparseable(&k.place).is_empty(),
        "the half line was cut at start: {:?}",
        lines(&k.place)
    );
    let said = lines(&k.place)
        .iter()
        .any(|l| l.contains("\"notice\"") && l.contains("ended in the middle of a line"));
    assert!(
        said,
        "the repair is said on the transcript: {:?}",
        lines(&k.place)
    );
    let log =
        std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.log")).unwrap_or_default();
    assert!(log.contains("transcript_repaired"), "{log}");

    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "AFTER-CRASH marker"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    // The bug's property: no event appended after the crash sits inside
    // an unparseable line. The wake and the user line both read.
    assert!(
        common::wait_for(Duration::from_secs(10), || {
            let ls = lines(&k.place);
            ls.iter().any(|l| l.contains("\"turn_complete\"")) && unparseable(&k.place).is_empty()
        }),
        "{:?}",
        lines(&k.place)
    );
    let ls = lines(&k.place);
    let readable = |kind: &str, marker: &str| {
        ls.iter().any(|l| {
            serde_json::from_str::<serde_json::Value>(l)
                .map(|v| v["kind"] == kind && v["text"].as_str().unwrap_or("").contains(marker))
                .unwrap_or(false)
        })
    };
    assert!(
        readable("wake", "AFTER-CRASH"),
        "the wake reads whole: {ls:?}"
    );
    assert!(
        readable("user", "AFTER-CRASH"),
        "the user line reads whole: {ls:?}"
    );
    assert!(
        readable("assistant", "three"),
        "the whole events before the cut stay: {ls:?}"
    );
    assert!(
        !ls.iter().any(|l| l.contains("\"hal")),
        "the half event is gone: {ls:?}"
    );
}

/// A record that ends properly is left exactly as it was: no cut, no
/// notice, no log line.
#[test]
fn a_whole_record_is_not_touched() {
    let k = start_kernel_replay_prepared("headless-tail-whole", "", "", |place| {
        let dir = place.join(".arbos/agents/root");
        std::fs::create_dir_all(&dir).unwrap();
        arbos_core::Agent::root("root").save(&dir).unwrap();
        std::fs::write(
            dir.join("transcript.jsonl"),
            "{\"ts\":1000000,\"kind\":\"assistant\",\"text\":\"one\"}\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    let ls = lines(&k.place);
    assert!(
        !ls.iter()
            .any(|l| l.contains("ended in the middle of a line")),
        "{ls:?}"
    );
    let log =
        std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.log")).unwrap_or_default();
    assert!(!log.contains("transcript_repaired"), "{log}");
}
