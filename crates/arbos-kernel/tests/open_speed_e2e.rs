//! How long the two opens take, measured the same way each time.
//!
//! "Opening a Terminal is very slow, and opening the Browser is very slow"
//! is a claim about wall-clock time, so it is answered with wall-clock time
//! rather than with a reading of the code. Every number here is taken from
//! the client's side of the attach socket — the same place the window sits —
//! and printed with `--nocapture`, so a run before a change and a run after
//! it are comparable line for line.
//!
//! Five measures, because "slow" is five different questions:
//!
//! - **shell → board**: the kernel minted the pty and said so. What the
//!   window waits for before it can draw a terminal tab at all.
//! - **shell → first pty**: the prompt is on the wire. What a person means
//!   by "the terminal opened".
//! - **browse → board**: the row exists. The tab can be drawn.
//! - **browse → picture**: the first frame of the page.
//! - **shell behind a browse**: while a browser is coming up, is the kernel
//!   still answering anything else? `Frame::Browse` used to run Chromium's
//!   whole cold start on the kernel's one frame loop, so every other client
//!   frame — a keystroke into a terminal, a message to an agent — queued
//!   behind it, and the blocking HTTP client on that path panicked the
//!   kernel outright. A `shell` sent one frame behind a `browse` has
//!   nothing to do with Chromium, so its board says what the kernel is
//!   doing while Chromium starts.

mod common;

use common::{Attach, Kernel, sigint, start_kernel_replay, wait_exit};
use std::time::{Duration, Instant};

/// Generous ceilings. The point of this file is the printed numbers; the
/// assertions only catch a return to the old behaviour, so they sit far
/// above a healthy run and well below what was measured before the fix.
const BOARD_BUDGET: Duration = Duration::from_millis(3_000);
const PTY_BUDGET: Duration = Duration::from_millis(6_000);
/// Chromium's own cold start is not ours to speed up, so the picture keeps
/// a wide budget. What must not happen is the *kernel* waiting on it.
const PICTURE_BUDGET: Duration = Duration::from_secs(45);

fn has_chrome() -> bool {
    ["chromium", "google-chrome", "chromium-browser"]
        .iter()
        .any(|b| which::which(b).is_ok())
}

fn ms(at: Instant) -> u128 {
    at.elapsed().as_millis()
}

/// Ask the kernel to stop, and wait for it.
///
/// Not `child.kill()`, which the rest of these tests use: that is SIGKILL,
/// and a kernel that never returns from `run` never drops its `BrowserHub`,
/// so the Chromium it started outlives it. Two headless browsers left
/// resident for the remaining twenty minutes of a suite is load every test
/// after this one pays for, on a runner with four cores.
fn stop(k: &mut Kernel) {
    sigint(&k.child);
    if wait_exit(&mut k.child, Duration::from_secs(20)).is_none() {
        let _ = k.child.kill();
    }
}

/// Terminal: the board frame, then the shell's own first output.
#[test]
fn opening_a_terminal_is_prompt() {
    let mut k = start_kernel_replay("open-speed-terminal", "");
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(10), |f| f["type"] == "snapshot")
            .is_some()
    );

    let asked = Instant::now();
    a.send(serde_json::json!({"type": "shell"}));
    let board = a
        .wait(Duration::from_secs(30), |f| {
            f["type"] == "board" && f["panel"] == "terminal" && f["action"] == "open"
        })
        .expect("a board frame for the shell");
    let board_ms = ms(asked);
    let id = board["terminal_ids"][0].as_str().unwrap().to_string();
    assert!(
        a.wait(Duration::from_secs(30), |f| {
            f["type"] == "pty" && f["page"] == id.as_str()
        })
        .is_some(),
        "no pty output for {id}"
    );
    let pty_ms = ms(asked);

    println!("MEASURE terminal board_ms={board_ms} first_pty_ms={pty_ms}");
    assert!(
        board_ms <= BOARD_BUDGET.as_millis(),
        "shell → board took {board_ms} ms (budget {} ms)",
        BOARD_BUDGET.as_millis()
    );
    assert!(
        pty_ms <= PTY_BUDGET.as_millis(),
        "shell → first pty took {pty_ms} ms (budget {} ms)",
        PTY_BUDGET.as_millis()
    );
    stop(&mut k);
}

/// Browser: the row, then whether the kernel is still answering while
/// Chromium comes up, then the first picture of the page.
///
/// One test and one kernel for all three, because a second one would be a
/// second Chromium. The kernel's browser starts once and lives as long as
/// the kernel does, so the cost of asking these questions separately is
/// another browser resident for the rest of the suite.
#[test]
fn opening_a_browser_is_prompt_and_does_not_stop_the_kernel() {
    if !has_chrome() {
        eprintln!("no chrome here; skipping");
        return;
    }
    let mut k = start_kernel_replay("open-speed-browser", "");
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(10), |f| f["type"] == "snapshot")
            .is_some()
    );

    let asked = Instant::now();
    a.send(serde_json::json!({"type": "browse", "url": "about:blank"}));
    // One frame behind the browse, and nothing to do with Chromium: the
    // only thing that can hold this up is the kernel itself being busy.
    a.send(serde_json::json!({"type": "shell"}));
    assert!(
        a.wait(Duration::from_secs(30), |f| {
            f["type"] == "board" && f["panel"] == "browser" && f["action"] == "open"
        })
        .is_some(),
        "no board frame for the page"
    );
    let board_ms = ms(asked);
    assert!(
        a.wait(Duration::from_secs(60), |f| {
            f["type"] == "board" && f["panel"] == "terminal" && f["action"] == "open"
        })
        .is_some(),
        "no board frame for the shell asked for behind the browse"
    );
    let shell_ms = ms(asked);
    let picture = a.wait(PICTURE_BUDGET, |f| {
        f["type"] == "browser" && f["page"] == "b1"
    });
    let picture_ms = ms(asked);

    match &picture {
        Some(_) => println!(
            "MEASURE browser board_ms={board_ms} shell_behind_browse_ms={shell_ms} picture_ms={picture_ms}"
        ),
        // Chrome is on the machine but cannot come up here (no display
        // parts, a locked-down container). The other two numbers stand,
        // and the one this change is about is the middle one.
        None => println!(
            "MEASURE browser board_ms={board_ms} shell_behind_browse_ms={shell_ms} picture_ms=none (chrome did not come up)"
        ),
    }
    stop(&mut k);

    assert!(
        board_ms <= BOARD_BUDGET.as_millis(),
        "browse → board took {board_ms} ms (budget {} ms)",
        BOARD_BUDGET.as_millis()
    );
    assert!(
        shell_ms <= BOARD_BUDGET.as_millis(),
        "a shell asked for behind a browse waited {shell_ms} ms for the kernel (budget {} ms)",
        BOARD_BUDGET.as_millis()
    );
    if let Some(picture) = picture {
        assert!(
            picture["screenshot"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "the first browser frame carries no picture: {picture}"
        );
    }
}
