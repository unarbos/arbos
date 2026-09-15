//! qa-038: every remote spawn left an `arbos-kernel serve` running on the
//! machine after the parent kernel exited. A kernel started for a child
//! on another machine now runs with `--leash <span>`: once no client has
//! been attached for that span and nothing runs, it exits by itself. A
//! client attached keeps it; a place opened by the user has no leash.

mod common;

use common::{Attach, scratch_dir, spawn_with};
use std::time::{Duration, Instant};

fn exited_within(k: &mut common::Kernel, span: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < span {
        if let Ok(Some(_)) = k.child.try_wait() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

#[test]
fn a_leashed_kernel_exits_once_alone_and_idle_and_stays_while_attached() {
    let scratch = scratch_dir("leash");
    let replies = scratch.join("replies.jsonl");
    std::fs::write(&replies, "").unwrap();
    std::fs::write(scratch.join("xdg/arbos/config.toml"), "trace = false\n").unwrap();
    let mut k = spawn_with(
        scratch,
        &[
            "--provider",
            "replay",
            "--replies",
            &replies.display().to_string(),
            "--leash",
            "3s",
        ],
    );
    // A client attached: the leash does not run.
    {
        let mut a = Attach::connect(&k.url);
        assert!(
            a.wait(Duration::from_secs(5), |f| f["type"] == "hello")
                .is_some()
        );
        assert!(
            !exited_within(&mut k, Duration::from_secs(6)),
            "attached: the kernel stays"
        );
    }
    // The client left: alone and idle for the span, it goes.
    assert!(
        exited_within(&mut k, Duration::from_secs(15)),
        "alone: the kernel exits on its own"
    );
    let log =
        std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.log")).unwrap_or_default();
    assert!(
        log.contains("\"leash\""),
        "the leash is logged at start: {log}"
    );
    assert!(
        log.lines()
            .any(|l| l.contains("kernel_stop") && l.contains("leash")),
        "and the stop names it: {log}"
    );
}

#[test]
fn a_kernel_without_a_leash_stays() {
    let scratch = scratch_dir("no-leash");
    let replies = scratch.join("replies.jsonl");
    std::fs::write(&replies, "").unwrap();
    std::fs::write(scratch.join("xdg/arbos/config.toml"), "trace = false\n").unwrap();
    let mut k = spawn_with(
        scratch,
        &[
            "--provider",
            "replay",
            "--replies",
            &replies.display().to_string(),
        ],
    );
    assert!(!exited_within(&mut k, Duration::from_secs(5)));
    let _ = k.child.kill();
}
