//! qa-012: when writes start failing (disk full, quota, `ulimit -f`), the
//! kernel must stay up and leave no half-written line. It used to die on
//! SIGXFSZ mid-write, leaving a truncated plan.jsonl and a stale lock.

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
fn a_write_past_the_size_limit_does_not_kill_the_kernel_or_leave_half_a_line() {
    // Children inherit the limit; the test process restores its own after the spawn.
    set_fsize(256 * 1024);
    let mut k = start_kernel("fsize");
    set_fsize(libc::RLIM_INFINITY);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );

    let big = format!("Reply OK. {}", "x".repeat(300 * 1024));
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": big}));
    std::thread::sleep(Duration::from_secs(2));
    assert!(
        k.child.try_wait().unwrap().is_none(),
        "kernel must survive a failed write (was: killed by SIGXFSZ)"
    );

    let root = k.place.join(".arbos").join("agents").join("root");
    for name in ["plan.jsonl", "transcript.jsonl"] {
        let path = root.join(name);
        if let Ok(text) = std::fs::read_to_string(&path) {
            assert!(
                text.is_empty() || text.ends_with('\n'),
                "{name} ends mid-line: {} bytes",
                text.len()
            );
            for line in text.lines() {
                assert!(
                    serde_json::from_str::<serde_json::Value>(line).is_ok(),
                    "{name} has an unparseable line"
                );
            }
        }
    }

    // Small work still goes through.
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "hello"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let _ = k.child.kill();
}
