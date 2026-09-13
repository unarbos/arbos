//! End-to-end checks on the real `arbos-kernel serve` binary. These are the
//! QA loop's scenarios, kept where CI runs.

mod common;

use common::{Attach, lock_path, sigint, start_kernel, wait_exit};
use std::time::{Duration, Instant};

/// qa-003: a 4 MB prompt used to make the serve loop re-parse megabytes
/// of transcript five times a second and the plan several times per turn,
/// so Ctrl-C went unanswered for more than ten seconds and the lock file
/// stayed behind after the kill.
#[test]
fn a_huge_prompt_does_not_stop_ctrl_c_from_ending_the_kernel_cleanly() {
    let mut k = start_kernel("huge");
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );

    let big = format!("Reply OK.\n{}", "lorem ipsum ".repeat(350_000));
    assert!(big.len() > 4_000_000);
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": big}));
    assert!(
        a.wait_turn("root", "idle", Duration::from_secs(60)),
        "the huge turn never reached idle"
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "Reply FINE."}));
    assert!(
        a.wait_turn("root", "idle", Duration::from_secs(60)),
        "the follow-up turn never reached idle"
    );

    let asked = Instant::now();
    sigint(&k.child);
    let code = wait_exit(&mut k.child, Duration::from_secs(4));
    if code.is_none() {
        let _ = k.child.kill();
    }
    assert_eq!(
        code,
        Some(0),
        "kernel must exit 0 within 4s of Ctrl-C (took {:?}, None = still running)",
        asked.elapsed()
    );
    assert!(
        !lock_path(&k.place).exists(),
        "a clean stop removes .arbos/lock"
    );
}
