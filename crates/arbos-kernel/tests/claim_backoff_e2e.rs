//! qa-018: a node whose claim cannot be written (disk full, size limit)
//! used to be re-claimed every tick, each time leaving an open attempt,
//! with no error anywhere. Now the attempt is closed, the client is told,
//! and the node waits a minute.

mod common;

use common::{Attach, start_kernel};
use std::time::Duration;

fn set_fsize(cur: libc::rlim_t) {
    let l = libc::rlimit {
        rlim_cur: cur,
        rlim_max: libc::RLIM_INFINITY,
    };
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_FSIZE, &l) }, 0);
}

#[test]
fn an_unwritable_claim_closes_its_attempt_and_backs_off() {
    set_fsize(256 * 1024);
    let mut k = start_kernel("claim");
    set_fsize(libc::RLIM_INFINITY);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );

    // The inbox write (one node) fits; the claim's rewrite of the same node
    // pushes plan.jsonl past the limit.
    let big = format!("Reply OK. {}", "x".repeat(200 * 1024));
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": big}));
    let err = a.wait(Duration::from_secs(20), |f| {
        f["type"] == "error" && f["agent"] == "root"
    });
    assert!(
        err.is_some(),
        "the client must be told the node could not start"
    );
    std::thread::sleep(Duration::from_secs(12));

    let root = k.place.join(".arbos").join("agents").join("root");
    let attempts: Vec<serde_json::Value> = std::fs::read_to_string(root.join("attempts.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let mut latest = std::collections::BTreeMap::new();
    for at in &attempts {
        latest.insert(at["id"].as_str().unwrap_or("").to_string(), at.clone());
    }
    let open: Vec<_> = latest
        .values()
        .filter(|at| at["ended_ms"].is_null())
        .collect();
    assert!(
        latest.len() <= 2,
        "claims kept retrying: {} attempts in ~15s",
        latest.len()
    );
    assert!(open.is_empty(), "attempts left open: {open:?}");
    let log = std::fs::read_to_string(k.place.join(".arbos").join("runtime").join("kernel.log"))
        .unwrap_or_default();
    assert!(
        log.contains("claim_failed"),
        "kernel.log has no claim_failed line"
    );
    let _ = k.child.kill();
}
